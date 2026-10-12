//! `dd policy announce`: a change to the fleet's policies, mailed to every
//! member at their <name>@<domain> at least 30 days before it takes effect,
//! and written down in docs/policies/changes.md. The mail goes out from the
//! box that writes to members (roles/_member-mail.nix); no address but the
//! alias is ever in hand here or there (docs/dev/leaving.md, Announcing).

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};

/// the least notice a change may be given, in days
pub const NOTICE_DAYS: i64 = 30;

/// days since 1970-01-01 of a YYYY-MM-DD
pub fn day_number(s: &str) -> Result<i64> {
    let p: Vec<&str> = s.split('-').collect();
    ensure!(
        p.len() == 3 && p[0].len() == 4 && p[1].len() == 2 && p[2].len() == 2,
        "a date is YYYY-MM-DD: {s}"
    );
    let (y, m, d): (i64, i64, i64) = (p[0].parse()?, p[1].parse()?, p[2].parse()?);
    ensure!(
        (1..=12).contains(&m) && (1..=31).contains(&d),
        "no such date: {s}"
    );
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let n = era * 146097 + doe - 719468;
    ensure!(
        crate::member::date(n as u64 * 86400) == s,
        "no such date: {s}"
    );
    Ok(n)
}

/// A change takes effect no sooner than 30 days after the day it is sent.
pub fn check_notice(today: i64, takes_effect: &str) -> Result<()> {
    let at = day_number(takes_effect)?;
    let soonest = crate::member::date((today + NOTICE_DAYS) as u64 * 86400);
    if at < today + NOTICE_DAYS {
        bail!(
            "{takes_effect} is {} days away; members are told at least {NOTICE_DAYS} days ahead, so {soonest} is the soonest",
            at - today
        );
    }
    Ok(())
}

/// Who is told: everyone the release lets in (on the list, or in by an
/// invite the release key signed) and not shut out or gone by their own
/// word. Names only; each is written to at <name>@<domain>.
pub fn recipients(
    entries: &[identity::SignedEntry],
    members: &crate::member::Members,
    release: Option<&ed25519_dalek::VerifyingKey>,
    left: &BTreeSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = entries
        .iter()
        .filter(|e| identity::valid_name(&e.entry.name) && !e.entry.name.starts_with("guest"))
        .filter(|e| !left.contains(&e.entry.name))
        .filter(|e| identity::is_member(&e.entry, &members.members, &members.revoked, release))
        .map(|e| e.entry.name.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// the title: the file's first `# ` line, or its first line
fn title(text: &str) -> String {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("A change to Commonty's policies");
    first
        .trim_start_matches('#')
        .trim()
        .chars()
        .take(120)
        .collect()
}

pub struct Announce<'a> {
    pub repo: &'a str,
    pub file: &'a Path,
    pub takes_effect: &'a str,
    pub dirs: &'a [String],
    pub dry_run: bool,
}

pub async fn announce(a: Announce<'_>) -> Result<()> {
    let today = (identity::now() / 86400) as i64;
    check_notice(today, a.takes_effect)?;
    let text =
        std::fs::read_to_string(a.file).with_context(|| format!("reading {}", a.file.display()))?;
    ensure!(!text.trim().is_empty(), "{} is empty", a.file.display());
    let root = Path::new(a.repo);
    let commit = git(root, &["rev-parse", "--short=12", "HEAD"])?;
    let policy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/policy.json"))?)?;
    let domain = policy["domain"]
        .as_str()
        .context("fleet/policy.json has no domain")?;
    let release = std::fs::read_to_string(root.join("fleet/release.pub"))
        .ok()
        .and_then(|s| identity::decode_public(s.trim()).ok());
    let members = crate::member::read(a.repo)?;

    let (entries, left) = directory_now(a.dirs).await?;
    let to = recipients(&entries, &members, release.as_ref(), &left);
    ensure!(
        !to.is_empty(),
        "nobody to tell: no member found in the directory"
    );

    let subject = format!("Commonty: {}", title(&text));
    let body = format!(
        "{}\n\n--\nThis takes effect on {}. Nothing changes before then.\nThe policies, and every change to them: https://{domain}/docs/policies/changes\n(change {commit})\n",
        text.trim_end(),
        a.takes_effect
    );
    let sent = crate::member::date(identity::now());
    let job = serde_json::json!({
        "id": format!("{sent}-{commit}"),
        "subject": subject,
        "body": body,
        "to": to,
        "takes_effect": a.takes_effect,
        "commit": commit,
    });
    println!(
        "{subject}\ntakes effect {}\nto {} member(s): {}",
        a.takes_effect,
        to.len(),
        to.join(", ")
    );
    if a.dry_run {
        return Ok(());
    }
    let addr = mail_box(root)?;
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(format!("admin@{addr}"))
        .arg(format!(
            "sudo -n tee /var/lib/dd-announce/outbox/.{0}.json >/dev/null && sudo -n mv /var/lib/dd-announce/outbox/.{0}.json /var/lib/dd-announce/outbox/{0}.json",
            job["id"].as_str().unwrap_or_default()
        ))
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("ssh's stdin")?
        .write_all(serde_json::to_string(&job)?.as_bytes())?;
    ensure!(
        child.wait()?.success(),
        "{addr} did not take the announcement"
    );
    record(root, &sent, a.takes_effect, &title(&text), &commit)?;
    println!(
        "handed to {addr}, which mails each one within five minutes (journalctl -u dd-announce there)\ncommit docs/policies/changes.md"
    );
    Ok(())
}

/// the list's names, each one's entry, and who has left by their own word
async fn directory_now(dirs: &[String]) -> Result<(Vec<identity::SignedEntry>, BTreeSet<String>)> {
    let d = dirs.first().context("no directory")?;
    let http = crate::who::http()?;
    // the list and the statements are members': read as this device
    let listed = directory::names(d.trim_end_matches('/')).await?;
    let base = d.trim_end_matches('/');
    let base = base.strip_suffix("/_dd/directory").unwrap_or(base);
    let mut entries = Vec::new();
    let mut left = BTreeSet::new();
    for n in &listed {
        if let Some(e) = crate::who::newest(&crate::who::fetch(&dirs[..1], n).await) {
            entries.push(e);
        }
        if let Ok(r) = directory::read(&http, &format!("{base}/_dd/leaving/{n}"))
            .send()
            .await
            && r.status().is_success()
            && let Ok(v) = r.json::<serde_json::Value>().await
            && v["statement"]["leave"]["back"].as_bool() != Some(true)
        {
            left.insert(n.to_string());
        }
    }
    Ok((entries, left))
}

/// a box with the vault role, whose units write to members, of this fleet
fn mail_box(root: &Path) -> Result<String> {
    let boxes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/boxes.json"))?)?;
    for (_, b) in boxes.as_object().into_iter().flatten() {
        let roles = b["roles"].as_array().cloned().unwrap_or_default();
        if roles.iter().any(|r| r == "vault")
            && let Some(t) = b["net"].as_str()
            && t.parse::<std::net::IpAddr>().is_ok()
        {
            return Ok(t.to_string());
        }
    }
    bail!("no box in fleet/boxes.json writes to members (the vault role)")
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    ensure!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

/// a line in docs/policies/changes.md, newest first under the heading
fn record(root: &Path, sent: &str, takes_effect: &str, title: &str, commit: &str) -> Result<()> {
    let p = root.join("docs/policies/changes.md");
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let line =
        format!("- {title}: announced {sent}, takes effect {takes_effect} (change `{commit}`)\n");
    let new = match old.find("\n- ") {
        Some(i) => format!("{}{line}{}", &old[..=i], &old[i + 1..]),
        None => format!("{}\n{line}", old.trim_end()),
    };
    std::fs::write(&p, new).with_context(|| format!("writing {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_counted_in_days() {
        assert_eq!(day_number("1970-01-01").unwrap(), 0);
        assert_eq!(day_number("2026-10-11").unwrap(), 20737);
        assert!(day_number("2026-02-30").is_err());
        assert!(day_number("11/10/2026").is_err());
    }

    #[test]
    fn a_change_is_announced_at_least_thirty_days_ahead() {
        let today = day_number("2026-10-11").unwrap();
        assert!(check_notice(today, "2026-11-10").is_ok());
        assert!(check_notice(today, "2027-01-01").is_ok());
        let e = check_notice(today, "2026-11-09").unwrap_err().to_string();
        assert!(e.contains("2026-11-10"), "{e}");
        assert!(check_notice(today, "2026-10-11").is_err());
        assert!(check_notice(today, "2026-09-01").is_err());
    }

    fn entry(name: &str, root: &ed25519_dalek::SigningKey) -> identity::SignedEntry {
        let public = identity::encode_public(&root.verifying_key());
        identity::sign(
            identity::Entry {
                name: name.into(),
                root: public.clone(),
                recovery: String::new(),
                recovered_from: vec![],
                devices: vec![identity::Device {
                    fingerprint: "f".into(),
                    public_key: public,
                    added: 1,
                }],
                passkeys: vec![],
                grant: None,
                libraries: vec![],
                terms: None,
                removed_boxes: vec![],
                version: 1,
                updated: 1,
            },
            root,
        )
        .unwrap()
    }

    #[test]
    fn only_members_are_told() {
        let keys: Vec<_> = (0..5).map(|_| identity::generate()).collect();
        let es = vec![
            entry("ann", &keys[0]),
            entry("bob", &keys[1]),
            entry("cat", &keys[2]),
            entry("guest-x", &keys[3]),
            entry("dan", &keys[4]),
        ];
        let id = |i: usize| identity::member_id(&es[i].entry.root);
        let m = crate::member::Members {
            members: vec![id(0), id(1), id(3), id(4)],
            revoked: vec![id(1)],
            ..Default::default()
        };
        let left = BTreeSet::from(["dan".to_string()]);
        // bob is shut out, cat is not on the list, the guest is no member, dan left
        assert_eq!(recipients(&es, &m, None, &left), vec!["ann".to_string()]);
        assert_eq!(
            recipients(&es, &m, None, &BTreeSet::new()),
            vec!["ann".to_string(), "dan".to_string()]
        );
    }

    #[test]
    fn the_record_keeps_the_newest_first() {
        let d = std::env::temp_dir().join(format!("dd-policy-{}", std::process::id()));
        std::fs::create_dir_all(d.join("docs/policies")).unwrap();
        std::fs::write(d.join("docs/policies/changes.md"), "# Changes\n\nIntro.\n").unwrap();
        record(&d, "2026-10-11", "2026-11-15", "One", "aaa").unwrap();
        record(&d, "2026-10-12", "2026-11-20", "Two", "bbb").unwrap();
        let s = std::fs::read_to_string(d.join("docs/policies/changes.md")).unwrap();
        let (two, one) = (s.find("Two").unwrap(), s.find("One").unwrap());
        assert!(two < one, "{s}");
        assert!(s.starts_with("# Changes\n\nIntro.\n"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
