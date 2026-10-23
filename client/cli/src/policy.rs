//! `dd policy announce`: a change to the fleet's policies, mailed to every
//! member at their <name>@<domain> at least 30 days before it takes effect,
//! and written down in docs/policies/changes.md. It travels in the signed
//! release (fleet/announcements/<id>.json), and the box that writes to
//! members mails it from there once (modules/member-mail.nix): nobody logs
//! in to a box to send it. No address but the alias is ever in hand here or
//! there (docs/dev/leaving.md, Announcing).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

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

/// Who is told: every member, not shut out or gone by their own word.
/// Names only; each is written to at <name>@<domain>. Each entry with what
/// the directory proved of its recoveries; held to the list as a client
/// holds one (identity::anchored), a recovery since followed from the root
/// listed for the name (identity::follows), or an invite the release key
/// signed, proved by the root the entry has now: a directory's word for
/// earlier roots lets nobody in. A member the list does not name - by a box
/// of theirs - is in `agreed`, by name and the root the agreed state gives
/// the services to (directory::agreed_member), and counts under that root
/// alone.
pub fn recipients(
    entries: &[(identity::SignedEntry, Vec<identity::Handover>)],
    members: &crate::member::Members,
    release: Option<&ed25519_dalek::VerifyingKey>,
    left: &BTreeSet<String>,
    agreed: &BTreeSet<(String, String)>,
) -> Vec<String> {
    let m = members;
    let let_in = |e: &identity::SignedEntry, h: &[identity::Handover]| {
        let name = &e.entry.name;
        if identity::member_ids(&e.entry)
            .iter()
            .any(|id| m.revoked.contains(id))
        {
            return false;
        }
        identity::anchored(e, &m.members, &m.revoked, &m.names).is_ok()
            // the agreed state's word counts for a name the list gives
            // nobody: one it gives someone is theirs, by their root or a
            // recovery proved from it (below), and nobody else's
            || (identity::listed_as(name, &m.members, &m.names).is_none()
                && agreed.contains(&(name.clone(), e.entry.root.clone())))
            || identity::listed_as(name, &m.members, &m.names)
                .is_some_and(|from| identity::follows(e, h, from).is_ok())
            || (e.entry.recovered_from.is_empty()
                && identity::verify(e).is_ok()
                && release.is_some_and(|r| identity::verify_grant(&e.entry, r).is_ok()))
    };
    let mut out: Vec<String> = entries
        .iter()
        .filter(|(e, _)| identity::valid_name(&e.entry.name) && !e.entry.name.starts_with("guest"))
        .filter(|(e, _)| !left.contains(&e.entry.name))
        .filter(|(e, h)| let_in(e, h))
        .map(|(e, _)| e.entry.name.clone())
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

    let (entries, left, agreed) = directory_now(a.dirs).await?;
    let to = recipients(&entries, &members, release.as_ref(), &left, &agreed);
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
    // in the release: the box mails it once it is there, and refuses it
    // there too under 30 days' notice
    let dir = root.join("fleet/announcements");
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{}.json", job["id"].as_str().unwrap_or_default()));
    ensure!(
        !out.exists(),
        "{} is already there: one announcement a day per change",
        out.display()
    );
    std::fs::write(&out, serde_json::to_string_pretty(&job)? + "\n")
        .with_context(|| format!("writing {}", out.display()))?;
    record(root, &sent, a.takes_effect, &title(&text), &commit)?;
    println!(
        "written to {}\ncommit it and docs/policies/changes.md, and release: the box that writes to members mails each one within five minutes of switching (journalctl -u dd-announce there)",
        out.display()
    );
    Ok(())
}

/// the list's names, each one's entry, who has left by their own word,
/// and (name, root) for each one the agreed state gives the services to
async fn directory_now(
    dirs: &[String],
) -> Result<(
    Vec<(identity::SignedEntry, Vec<identity::Handover>)>,
    BTreeSet<String>,
    BTreeSet<(String, String)>,
)> {
    let d = dirs.first().context("no directory")?;
    let http = crate::who::http()?;
    // the list and the statements are members': read as this device
    let listed = directory::names(d.trim_end_matches('/')).await?;
    let base = d.trim_end_matches('/');
    let base = base.strip_suffix("/_dd/directory").unwrap_or(base);
    let mut entries = Vec::new();
    let mut left = BTreeSet::new();
    let mut agreed = BTreeSet::new();
    for n in &listed {
        if let Ok(e) = directory::resolve(
            &crate::who::fetch(&dirs[..1], n).await,
            directory::Anchor::FirstSight,
        ) {
            // every directory asked, as a client holds anyone the list does
            // not name (directory::resolve_member): one down is a no
            if directory::agreed_member(dirs, n, &e.entry.root)
                .await
                .is_ok()
            {
                agreed.insert((n.to_string(), e.entry.root.clone()));
            }
            entries.push((e, directory::handovers(&dirs[..1], n, &[]).await));
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
    Ok((entries, left, agreed))
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

    /// a directory stand-in over plain HTTP: path -> (status, body);
    /// anything else 404
    fn stand_in(routes: Vec<(String, u16, String)>) -> String {
        use std::io::{BufRead as _, Write as _};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let a = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { continue };
                let mut r = std::io::BufReader::new(c.try_clone().unwrap());
                let mut first = String::new();
                let _ = r.read_line(&mut first);
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let path = first.split(' ').nth(1).unwrap_or("").to_string();
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, s, b)| (*s, b.clone()))
                    .unwrap_or((404, String::new()));
                let _ = write!(
                    c,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{a}/_dd/directory")
    }

    /// A member the release's list does not name - by a box of theirs - is
    /// held to the agreed state as every directory says it: each that
    /// answers says yes for the root they agree on, and at least one
    /// answers. The list's revocation beats it.
    #[tokio::test]
    async fn a_member_by_a_box_resolves_on_every_directorys_word() {
        let ann = identity::generate();
        let cat = identity::generate();
        let cat_e = entry("cat", &cat);
        let other = identity::encode_public(&identity::generate().verifying_key());
        let list = |revoked: Vec<String>| identity::MemberList {
            counter: 1,
            members: vec![identity::member_id(&identity::encode_public(
                &ann.verifying_key(),
            ))],
            revoked,
            names: Default::default(),
        };
        let dir = |standing: (u16, serde_json::Value)| {
            stand_in(vec![
                (
                    "/_dd/directory/cat".into(),
                    200,
                    serde_json::to_string(&cat_e).unwrap(),
                ),
                (
                    "/_dd/directory/cat/standing".into(),
                    standing.0,
                    standing.1.to_string(),
                ),
            ])
        };
        let yes = (
            200,
            serde_json::json!({"member": true, "root": cat_e.entry.root}),
        );
        let no = (
            200,
            serde_json::json!({"member": false, "root": cat_e.entry.root}),
        );
        let elsewhere = (200, serde_json::json!({"member": true, "root": other}));
        let unread = (503, serde_json::json!("not read yet"));
        let ok = |dirs: Vec<String>, l: identity::MemberList| async move {
            directory::resolve_released(&dirs, "cat", &l).await
        };
        assert!(ok(vec![dir(yes.clone())], list(vec![])).await.is_ok());
        assert!(
            ok(vec![dir(yes.clone()), dir(yes.clone())], list(vec![]))
                .await
                .is_ok()
        );
        // one says no, or names another root: not a member
        assert!(
            ok(vec![dir(yes.clone()), dir(no.clone())], list(vec![]))
                .await
                .is_err()
        );
        assert!(
            ok(vec![dir(elsewhere), dir(yes.clone())], list(vec![]))
                .await
                .is_err()
        );
        // one that says nothing (not going by the agreed state), or is down,
        // is a no too: never a pass on the others' word
        assert!(
            ok(vec![dir(unread.clone()), dir(yes.clone())], list(vec![]))
                .await
                .is_err()
        );
        assert!(ok(vec![dir(unread.clone())], list(vec![])).await.is_err());
        let down = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let a = l.local_addr().unwrap();
            drop(l);
            format!("http://{a}/_dd/directory")
        };
        assert!(
            ok(vec![down, dir(yes.clone())], list(vec![]))
                .await
                .is_err()
        );
        // one without her entry at all, though it says she is a member
        let no_entry = stand_in(vec![(
            "/_dd/directory/cat/standing".into(),
            yes.0,
            yes.1.to_string(),
        )]);
        assert!(
            ok(vec![no_entry, dir(yes.clone())], list(vec![]))
                .await
                .is_err()
        );
        // the list gives "cat" to another root: the agreed state's word for
        // this one is not enough without a recovery proved from it
        let theirs = identity::member_id(&other);
        let mut taken = list(vec![]);
        taken.members.push(theirs.clone());
        taken
            .names
            .insert(theirs.clone(), identity::name_tag(&theirs, "cat"));
        assert!(ok(vec![dir(yes.clone())], taken).await.is_err());
        // shut out by the list, whatever the directories say
        let id = identity::member_id(&cat_e.entry.root);
        let e = ok(vec![dir(yes)], list(vec![id])).await.unwrap_err();
        assert!(format!("{e:#}").contains("shuts cat out"), "{e:#}");
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
        let listed = [0, 1, 3, 4];
        let m = crate::member::Members {
            members: listed.iter().map(|i| id(*i)).collect(),
            revoked: vec![id(1)],
            names: listed
                .iter()
                .map(|i| (id(*i), identity::name_tag(&id(*i), &es[*i].entry.name)))
                .collect(),
            ..Default::default()
        };
        let mut es: Vec<_> = es.into_iter().map(|e| (e, vec![])).collect();
        // a box's entries: ann's root under another name, and a root of
        // its own claiming ann's as the one it replaced
        es.push((entry("eve", &keys[0]), vec![]));
        let boxed = identity::generate();
        let mut f = entry("fay", &boxed);
        f.entry.recovered_from = vec![es[0].0.entry.root.clone()];
        es.push((identity::sign(f.entry, &boxed).unwrap(), vec![]));
        let left = BTreeSet::from(["dan".to_string()]);
        // bob is shut out, cat is not on the list, the guest is no member,
        // dan left, eve and fay are the box's
        let none = BTreeSet::new();
        assert_eq!(
            recipients(&es, &m, None, &left, &none),
            vec!["ann".to_string()]
        );
        assert_eq!(
            recipients(&es, &m, None, &BTreeSet::new(), &none),
            vec!["ann".to_string(), "dan".to_string()]
        );
        // cat, a member by a box of hers as the agreed state says it, is
        // told under the root it gave the services to and no other; bob,
        // shut out by the list, is not, whatever it says
        let cat_root = es[2].0.entry.root.clone();
        let agreed = BTreeSet::from([
            ("cat".to_string(), cat_root),
            ("bob".to_string(), es[1].0.entry.root.clone()),
        ]);
        assert_eq!(
            recipients(&es, &m, None, &left, &agreed),
            vec!["ann".to_string(), "cat".to_string()]
        );
        let elsewhere = BTreeSet::from([("cat".to_string(), es[0].0.entry.root.clone())]);
        assert_eq!(
            recipients(&es, &m, None, &left, &elsewhere),
            vec!["ann".to_string()]
        );
        // a name the list gives someone: the agreed state's word for another
        // root under it is no one's (dan is listed; this is not his root)
        let mut es2 = es.clone();
        let other = identity::generate();
        es2[4] = (entry("dan", &other), vec![]);
        let dans = BTreeSet::from([("dan".to_string(), es2[4].0.entry.root.clone())]);
        assert_eq!(
            recipients(&es2, &m, None, &BTreeSet::new(), &dans),
            vec!["ann".to_string()]
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
