//! The member list: who may use the services. `fleet/members.json` in the
//! repo: ids let in and ids shut out, a sha256 of each person's root, and
//! for each id a tag of the name it was confirmed under
//! (identity::name_tag), so it carries no names in the clear and a client
//! can tell a root stands behind that name and no other. It reaches the boxes in the signed release; a box
//! checks it and cannot add to it. An invite (`dd invite`) is the other way
//! in: a grant in the person's own entry, made with a code the release key
//! signed; the revoked list is what shuts one of those out again.
//!
//! Nobody is removed: a person leaves by their own word alone. The one
//! thing done to a member is a suspension for breaking the terms (`dd
//! member suspend`): revoked, with the reason in the list for everyone to
//! read, and for 90 days only read access to their own things, after which
//! they are deleted as on leaving (servers/gate/src/leaving.rs). Lifted in
//! those 90 days, nothing is lost.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

#[derive(Default, Serialize, Deserialize)]
pub struct Members {
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub revoked: Vec<String>,
    /// when each revoked id was suspended, unix seconds: their things are
    /// deleted from every box 90 days later (docs/dev/leaving.md)
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub removed: std::collections::BTreeMap<String, u64>,
    /// id -> why it is suspended, as the member and everyone read it
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub suspended: std::collections::BTreeMap<String, String>,
    /// id -> identity::name_tag(id, name), set by `dd member add <name>`
    /// once the operator confirmed the id: clients anchor a root only under
    /// the name listed for it
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub names: std::collections::BTreeMap<String, String>,
}

fn path(repo: &str) -> PathBuf {
    Path::new(repo).join("fleet/members.json")
}

pub fn read(repo: &str) -> Result<Members> {
    let p = path(repo);
    if !p.exists() {
        return Ok(Members::default());
    }
    let raw = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
    parse(&raw).with_context(|| format!("parsing {}", p.display()))
}

/// the file's contents; none at all is nobody
pub fn parse(raw: &[u8]) -> Result<Members> {
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(Members::default());
    }
    // the first shape was a bare list
    if let Ok(list) = serde_json::from_slice::<Vec<String>>(raw) {
        return Ok(Members {
            members: list,
            ..Default::default()
        });
    }
    Ok(serde_json::from_slice(raw)?)
}

fn write(repo: &str, mut m: Members) -> Result<()> {
    m.members.sort();
    m.members.dedup();
    m.revoked.sort();
    m.revoked.dedup();
    let p = path(repo);
    std::fs::write(&p, format!("{}\n", serde_json::to_string_pretty(&m)?))
        .with_context(|| format!("writing {}", p.display()))
}

fn is_id(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// A member id as given, or a name looked up in the directories and
/// confirmed by the person running this: the id is about to be written into
/// fleet/members.json and released to every box, and a directory can serve
/// an entry of its own making under any name, which would be it naming who
/// joins or who is shut out. So the root's id is shown, and taken only once
/// the operator says it is the one the person's own device shows (`dd
/// identity show`), or `--confirm` names it. The entry for a name, too:
/// only the entry the operator confirmed.
async fn confirmed(
    who: &str,
    dirs: &[String],
    m: &Members,
    confirm: Option<&str>,
) -> Result<(String, Option<identity::SignedEntry>)> {
    if is_id(who) {
        return Ok((who.to_string(), None));
    }
    let found = crate::who::fetch(dirs, who).await;
    let e = directory::resolve(&found, directory::Anchor::FirstSight)
        .with_context(|| format!("no usable entry for {who}"))?;
    let id = identity::member_id(&e.entry.root);
    let handovers = directory::handovers(dirs, who, &[]).await;
    println!(
        "{who}: root {}, member id\n  {id}\n  {}",
        identity::fingerprint(&e.entry.root),
        standing(&e, &handovers, m)
    );
    match confirm {
        Some(c) if c == id => {}
        Some(c) => bail!("--confirm {c} is not the member id the directories have for {who}"),
        None if std::io::stdin().is_terminal() => {
            print!("is that the member id {who}'s own device shows (`dd identity show`)? [y/N] ");
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if !matches!(line.trim(), "y" | "Y" | "yes") {
                bail!("not confirmed; nothing changed");
            }
        }
        None => bail!("without a terminal, --confirm <member id> says it is {who}'s"),
    }
    Ok((id, Some(e)))
}

/// what the repo's own list makes of an entry, as far as this machine can
/// check it (identity::anchored, identity::follows)
fn standing(e: &identity::SignedEntry, handovers: &[identity::Handover], m: &Members) -> String {
    let name = &e.entry.name;
    if let Err(why) = identity::anchored(e, &m.members, &m.revoked, &m.names) {
        return match identity::listed_as(name, &m.members, &m.names) {
            Some(from) if identity::follows(e, handovers, from).is_ok() => format!(
                "recovered from {from}, the root the list has for {name}, every step proven: adding lists this one instead"
            ),
            _ => format!("not anchored to the list: {why}"),
        };
    }
    format!("the list's root for {name}")
}

/// the ids the list names `name` by
fn tagged<'a>(m: &'a Members, name: &str) -> Vec<&'a String> {
    m.names
        .iter()
        .filter(|(id, t)| **t == identity::name_tag(id, name))
        .map(|(id, _)| id)
        .collect()
}

pub async fn add(repo: &str, who: &str, dirs: &[String], confirm: Option<&str>) -> Result<()> {
    let mut m = read(repo)?;
    let (id, entry) = confirmed(who, dirs, &m, confirm).await?;
    match &entry {
        // only an id given as itself comes off the revoked list: a
        // directory's word for which roots a name had is no reason to
        None if m.revoked.contains(&id) => {
            lift(&mut m, &id);
            println!("{id} is no longer revoked");
        }
        Some(e) => {
            let shut: Vec<String> = identity::member_ids(&e.entry)
                .into_iter()
                .filter(|x| m.revoked.contains(x))
                .collect();
            if !shut.is_empty() {
                bail!(
                    "{who} is revoked by {}: `dd member add <id>` with that id, if it is theirs, lets it back in",
                    shut.join(", ")
                );
            }
        }
        None => {}
    }
    if m.members.contains(&id) {
        println!("already a member: {id}");
    } else {
        m.members.push(id.clone());
        println!("added {id}");
    }
    // the name the operator confirmed this root under, and none other: one
    // current root per name, so a root this one replaced (a recovery's)
    // leaves the list and no client holds an entry to it again. Off the
    // list, not revoked: a removal of any root a person has had shuts the
    // person out at every box (identity::is_member)
    match &entry {
        Some(_) => {
            let old: Vec<String> = tagged(&m, who)
                .into_iter()
                .filter(|x| **x != id)
                .cloned()
                .collect();
            for x in &old {
                m.members.retain(|y| y != x);
                m.names.remove(x);
                println!("{x} is no longer listed: {id} is {who}'s root now");
            }
            m.names.insert(id.clone(), identity::name_tag(&id, who));
            println!("listed as {who}");
        }
        None if !m.names.contains_key(&id) => println!(
            "no name is listed for {id}: until `dd member add <their name> --confirm {id}` lists it, no device holds their entry to the release, and `dd release publish` refuses"
        ),
        None => {}
    }
    write(repo, m)?;
    println!("commit fleet/members.json and release; every box then lets them in");
    Ok(())
}

/// Off the revoked list: a suspension lifted. Within its 90 days nothing
/// of theirs was deleted, so nothing is lost
fn lift(m: &mut Members, id: &str) {
    m.revoked.retain(|x| x != id);
    m.removed.remove(id);
    m.suspended.remove(id);
}

/// Suspended for breaking the terms: off the list and onto the revoked one,
/// so an invite they redeemed counts for nothing either, with the time and
/// the reason. By a name the list has, every root listed under it, as the
/// file says; by a name it does not, the one confirmed.
pub async fn suspend(
    repo: &str,
    who: &str,
    reason: &str,
    dirs: &[String],
    confirm: Option<&str>,
) -> Result<()> {
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("a suspension says why: --reason");
    }
    let mut m = read(repo)?;
    let mut ids: Vec<String> = if is_id(who) {
        vec![]
    } else {
        tagged(&m, who).into_iter().cloned().collect()
    };
    if ids.is_empty() {
        ids.push(confirmed(who, dirs, &m, confirm).await?.0);
    }
    ids.retain(|id| !m.revoked.contains(id) || m.members.contains(id));
    if ids.is_empty() {
        bail!("already suspended: {who}");
    }
    for id in &ids {
        m.members.retain(|x| x != id);
        if !m.revoked.contains(id) {
            m.revoked.push(id.clone());
        }
        m.removed.entry(id.clone()).or_insert_with(identity::now);
        m.suspended.insert(id.clone(), reason.to_string());
        println!("suspended {id}");
    }
    write(repo, m)?;
    println!(
        "commit fleet/members.json and release; every box then ends their sign-ins, lets them read and take their own things for 90 days, and deletes what is theirs after that unless `dd member unsuspend` lifts it"
    );
    Ok(())
}

/// A suspension lifted, for the exact id given or the one a name is
/// confirmed as: back on the list, nothing lost
pub async fn unsuspend(
    repo: &str,
    who: &str,
    dirs: &[String],
    confirm: Option<&str>,
) -> Result<()> {
    let mut m = read(repo)?;
    let (id, _) = confirmed(who, dirs, &m, confirm).await?;
    if !m.revoked.contains(&id) {
        bail!("{id} is not suspended");
    }
    lift(&mut m, &id);
    if !m.members.contains(&id) {
        m.members.push(id.clone());
    }
    write(repo, m)?;
    println!("{id} is no longer suspended: commit fleet/members.json and release");
    Ok(())
}

/// Every id on the list, and every entry in the first directory that
/// carries a redeemed invite, with the name where one is found. Names are
/// looked up, and stored only as tags.
pub async fn list(repo: &str, dirs: &[String]) -> Result<()> {
    let m = read(repo)?;
    let mut names = std::collections::HashMap::new();
    let mut invited = Vec::new();
    if let Some(d) = dirs.first()
        && let Ok(listed) = directory::names(d.trim_end_matches('/')).await
    {
        for n in &listed {
            let Ok(e) = directory::resolve(
                &crate::who::fetch(&dirs[..1], n).await,
                directory::Anchor::FirstSight,
            ) else {
                continue;
            };
            let id = identity::member_id(&e.entry.root);
            // a name beside a listed id only where the entry anchors to the
            // list: a directory can name any entry it makes up anything
            if identity::anchored(&e, &m.members, &m.revoked, &m.names).is_ok() {
                names.insert(id.clone(), n.to_string());
            } else if let Some(from) = identity::listed_as(n, &m.members, &m.names) {
                let handovers = directory::handovers(&dirs[..1], n, &[]).await;
                if identity::follows(&e, &handovers, from).is_ok() {
                    names.insert(
                        from.clone(),
                        format!(
                            "{n}  recovered since: `dd member add {n}` lists the root they have now"
                        ),
                    );
                }
            } else if m.members.contains(&id) && !m.names.contains_key(&id) {
                names.insert(
                    id.clone(),
                    format!("{n}  name not listed: `dd member add {n}` lists it"),
                );
            }
            if let Some(g) = &e.entry.grant {
                invited.push((id, n.to_string(), g.redeemed));
            }
        }
    }
    let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| "-".into());
    for id in &m.members {
        println!("{id}  {}", name(id));
    }
    for (id, n, t) in &invited {
        if names.contains_key(id) || m.revoked.contains(id) {
            continue;
        }
        println!(
            "{id}  {n}  invited {}, not anchored: unverified until a release lists it",
            date(*t)
        );
    }
    for id in &m.revoked {
        match m.suspended.get(id) {
            Some(why) => println!(
                "{id}  {}  suspended {}: {why}",
                name(id),
                m.removed.get(id).map(|t| date(*t)).unwrap_or_default()
            ),
            None => println!("{id}  {}  revoked", name(id)),
        }
    }
    Ok(())
}

pub(crate) fn date(t: u64) -> String {
    // days since the epoch to y-m-d, no calendar crate for one line
    let days = (t / 86400) as i64;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// `dd invite`: a code for one person, good for `ttl`. The code derives a
/// key; the invite names that key's public half, signed by the release key,
/// and goes to every directory. The code goes to the person, by you.
pub async fn invite(ttl: &str, dirs: &[String], release: &ed25519_dalek::SigningKey) -> Result<()> {
    let secs = parse_ttl(ttl)?;
    let code = identity::new_code();
    let key = identity::code_key(&code);
    let now = identity::now();
    let signed = identity::sign_invite(
        identity::Invite {
            public_key: identity::encode_public(&key.verifying_key()),
            issued: now,
            expires: now + secs,
        },
        release,
    )?;
    let http = crate::who::http()?;
    let mut ok = 0;
    for d in dirs {
        let base = d.trim_end_matches('/');
        let base = base.strip_suffix("/_dd/directory").unwrap_or(base);
        use base64::Engine as _;
        let url = format!(
            "{base}/_dd/invite/{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(signed.invite.public_key.as_bytes())
        );
        match http.put(&url).json(&signed).send().await {
            Ok(r) if r.status().is_success() => {
                ok += 1;
                println!("  {d}: holds it");
            }
            Ok(r) => println!(
                "  {d}: refused ({} {})",
                r.status(),
                r.text().await.unwrap_or_default().trim()
            ),
            Err(e) => println!("  {d}: unreachable ({e})"),
        }
    }
    if ok == 0 {
        bail!("no directory took the invite");
    }
    println!(
        "\ncode: {code}\ngood for {ttl}, one person. They type it on the join page, or on their waiting page if they already have an account."
    );
    Ok(())
}

fn parse_ttl(s: &str) -> Result<u64> {
    let (n, unit) = s.split_at(s.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len());
    let n: u64 = n
        .parse()
        .with_context(|| format!("ttl {s}: not a number"))?;
    Ok(match unit {
        "s" => n,
        "m" | "" => n * 60,
        "h" => n * 3600,
        _ => bail!("ttl {s}: use s, m or h"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lifted_suspension_leaves_nothing_behind() {
        let mut m = Members::default();
        m.revoked.push("abc".into());
        m.removed.insert("abc".into(), 7);
        m.suspended.insert("abc".into(), "spam".into());
        m.revoked.push("def".into());
        lift(&mut m, "abc");
        assert_eq!(m.revoked, vec!["def".to_string()]);
        assert!(m.removed.is_empty() && m.suspended.is_empty());
    }

    #[test]
    fn the_reason_is_in_the_file() {
        let mut m = Members::default();
        m.suspended.insert("abc".into(), "spam".into());
        let back = parse(serde_json::to_string(&m).unwrap().as_bytes()).unwrap();
        assert_eq!(back.suspended.get("abc").map(String::as_str), Some("spam"));
    }
}
