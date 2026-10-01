//! The directories, as a client sees them: storage for signed entries and
//! nothing more. A directory that lies can withhold or replay, and asking
//! more than one is how that is caught. `dd` and the app share this.

use anyhow::{Context as _, Result, anyhow, bail};
use identity::SignedEntry;

/// the directories a client asks unless told otherwise: the front door's,
/// the one name that answers from anywhere. A device off the owner's
/// tailnet and not yet on the fleet's network reaches nothing else, and
/// signing up, signing in and joining the network all start here. The
/// other boxes hold copies, pulled from this one.
pub const DEFAULT: [&str; 1] = ["https://home.commonty.org/_dd/directory"];

/// a proxy every client this process builds goes through: the app's way
/// into the fleet's own network (app/src/net.rs), a SOCKS5 one that
/// resolves names on its side (the network's own names). None: straight out.
#[derive(Clone)]
pub struct Proxy {
    /// host:port
    pub addr: String,
    pub user: String,
    pub password: String,
}

static VIA: std::sync::RwLock<Option<Proxy>> = std::sync::RwLock::new(None);

pub fn via(p: Option<Proxy>) {
    if let Ok(mut v) = VIA.write() {
        *v = p;
    }
}

pub fn proxy() -> Option<Proxy> {
    VIA.read().ok().and_then(|v| v.clone())
}

/// What this process says it is, so a member's Devices tab can tell their
/// devices apart: "dd/<version> (<os>)" unless the program says otherwise
/// (the app does, app/src/lib.rs).
static AGENT: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

pub fn set_agent(a: String) {
    if let Ok(mut v) = AGENT.write() {
        *v = Some(a);
    }
}

fn agent() -> String {
    AGENT
        .read()
        .ok()
        .and_then(|v| v.clone())
        .unwrap_or_else(|| {
            format!(
                "dd/{} ({})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS
            )
        })
}

/// a client: no redirects, a short timeout, through the network's proxy
/// when this process is on it
pub fn http() -> Result<reqwest::Client> {
    builder()?
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(Into::into)
}

/// the same, for callers that set their own limits
pub fn builder() -> Result<reqwest::ClientBuilder> {
    let mut b = reqwest::Client::builder().user_agent(agent());
    if let Some(p) = proxy() {
        // socks5h: the name goes to the proxy unresolved
        let url = format!("socks5h://{}:{}@{}", p.user, p.password, p.addr);
        b = b.proxy(reqwest::Proxy::all(&url)?);
    }
    Ok(b)
}

/// What each directory has for `name`; a directory that cannot be reached is
/// an error entry, not a missing one, because the two mean different things.
pub async fn fetch(dirs: &[String], name: &str) -> Vec<(String, Result<Option<SignedEntry>>)> {
    let mut out = Vec::new();
    let http = match http() {
        Ok(h) => h,
        Err(e) => return vec![(String::new(), Err(e))],
    };
    for d in dirs {
        let url = format!("{}/{name}", d.trim_end_matches('/'));
        let r = async {
            let r = http.get(&url).send().await?;
            match r.status().as_u16() {
                404 => Ok(None),
                200 => Ok(Some(r.json::<SignedEntry>().await?)),
                s => Err(anyhow!("{s} {}", r.text().await.unwrap_or_default())),
            }
        }
        .await;
        out.push((d.clone(), r));
    }
    out
}

/// What a client already knows about whose entry this should be.
///
/// A directory is storage. It can withhold, it can replay, and it can hand
/// back something it made up - so the version number alone is an invitation:
/// the highest one wins, and anybody may claim any number.
pub enum Anchor<'a> {
    /// this machine holds the root already (its own keyring, or a pin
    /// written when the device was admitted)
    Root(&'a str),
    /// the member ids in the signed release: the entry must be one of them
    Members(&'a [String]),
    /// nothing is known yet. Every directory asked has to agree on the
    /// root, and what comes back is worth pinning.
    FirstSight,
}

/// The entry for a name, believed only as far as it can be checked.
///
/// Verifies every answer, refuses the lot if two directories disagree about
/// whose root the name has, then applies the anchor. This is the one place
/// that decides; before it there were four rules in four crates and the
/// weakest of them set the price.
pub fn resolve(
    found: &[(String, Result<Option<SignedEntry>>)],
    anchor: Anchor,
) -> Result<SignedEntry> {
    let mut good: Vec<&SignedEntry> = Vec::new();
    for (dir, r) in found {
        let Ok(Some(e)) = r else { continue };
        match identity::verify(e) {
            Ok(()) => good.push(e),
            Err(err) => bail!("{dir} served an entry that does not verify: {err}"),
        }
    }
    let first = *good.first().context("no directory has that entry")?;
    if let Some(other) = good.iter().find(|e| e.entry.root != first.entry.root) {
        bail!(
            "the directories disagree about {}: one says {}, another {}",
            first.entry.name,
            first.entry.root,
            other.entry.root
        );
    }
    match anchor {
        Anchor::Root(want) if first.entry.root != want => {
            bail!(
                "{} is not the root this machine holds for {}",
                first.entry.root,
                first.entry.name
            )
        }
        Anchor::Members(ids) => {
            let id = identity::member_id(&first.entry.root);
            if !ids.contains(&id) {
                bail!("{} is not in the member list", first.entry.name);
            }
        }
        _ => {}
    }
    good.into_iter()
        .max_by_key(|e| e.entry.version)
        .cloned()
        .context("no directory has that entry")
}

/// The newest entry any directory holds, checked against nothing yet: the
/// caller decides what it must match (its own root, or a recovery key).
///
/// Prefer `resolve`. What is left here is the publishing path, which is
/// looking for its own last version rather than believing anything.
pub fn newest(found: &[(String, Result<Option<SignedEntry>>)]) -> Option<SignedEntry> {
    found
        .iter()
        .filter_map(|(_, r)| r.as_ref().ok().and_then(|o| o.clone()))
        .max_by_key(|e| e.entry.version)
}

/// every name a directory lists
pub async fn names(dir: &str) -> Result<Vec<String>> {
    let v: Vec<serde_json::Value> = http()?
        .get(dir)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(v.into_iter()
        .filter_map(|l| l["name"].as_str().map(str::to_string))
        .collect())
}

/// what one directory said to a publish
pub enum Took {
    Accepted,
    Refused(String),
    Unreachable(String),
}

/// Push to every directory, one verdict each. Every directory accepting is
/// success; a directory left behind is one that will later serve a stale
/// entry as current, so the caller treats anything less as an error.
pub async fn publish(dirs: &[String], signed: &SignedEntry) -> Result<Vec<(String, Took)>> {
    let http = http()?;
    let mut out = Vec::new();
    for d in dirs {
        let url = format!("{}/{}", d.trim_end_matches('/'), signed.entry.name);
        let took = match http.put(&url).json(signed).send().await {
            Ok(r) if r.status().is_success() => Took::Accepted,
            Ok(r) => {
                let s = r.status();
                Took::Refused(format!("{s} {}", r.text().await.unwrap_or_default().trim()))
            }
            Err(e) => Took::Unreachable(e.to_string()),
        };
        out.push((d.clone(), took));
    }
    Ok(out)
}

/// publish, and fail unless every directory took it
pub async fn publish_all(dirs: &[String], signed: &SignedEntry) -> Result<Vec<(String, Took)>> {
    let out = publish(dirs, signed).await?;
    let failed = out
        .iter()
        .filter(|(_, t)| !matches!(t, Took::Accepted))
        .count();
    if failed > 0 {
        bail!(
            "{failed} of {} directories did not take the update",
            dirs.len()
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use identity::{Device, Entry, encode_public, fingerprint, generate, sign};

    fn entry(name: &str, k: &identity::SigningKey, version: u64) -> SignedEntry {
        let p = encode_public(&generate().verifying_key());
        sign(
            Entry {
                name: name.into(),
                root: encode_public(&k.verifying_key()),
                recovery: encode_public(&generate().verifying_key()),
                devices: vec![Device {
                    fingerprint: fingerprint(&p),
                    public_key: p,
                    added: 1,
                }],
                passkeys: vec![],
                grant: None,
                libraries: vec![],
                version,
                updated: 1,
            },
            k,
        )
        .unwrap()
    }

    fn said(v: Vec<(&str, SignedEntry)>) -> Vec<(String, Result<Option<SignedEntry>>)> {
        v.into_iter()
            .map(|(d, e)| (d.to_string(), Ok(Some(e))))
            .collect()
    }

    /// A directory is storage. The version number is its own claim, so the
    /// highest one is an invitation rather than an answer.
    #[test]
    fn a_lying_directory_cannot_out_number_an_honest_one() {
        let hers = generate();
        let theirs = generate();

        // both honest, one stale: the same root, so the newer one stands
        let found = said(vec![
            ("a", entry("alice", &hers, 3)),
            ("b", entry("alice", &hers, 7)),
        ]);
        assert_eq!(
            resolve(&found, Anchor::FirstSight).unwrap().entry.version,
            7
        );

        // one lying, and louder: refused outright rather than believed
        let found = said(vec![
            ("a", entry("alice", &hers, 7)),
            ("b", entry("alice", &theirs, u64::MAX)),
        ]);
        let e = resolve(&found, Anchor::FirstSight).unwrap_err();
        assert!(format!("{e}").contains("disagree"), "{e}");

        // the root this machine holds is the last word
        let found = said(vec![("a", entry("alice", &theirs, 9))]);
        let mine = encode_public(&hers.verifying_key());
        assert!(resolve(&found, Anchor::Root(&mine)).is_err());
        let ok = encode_public(&theirs.verifying_key());
        assert!(resolve(&found, Anchor::Root(&ok)).is_ok());

        // and the released member list, where there is one
        let ids = [identity::member_id(&ok)];
        assert!(resolve(&found, Anchor::Members(&ids)).is_ok());
        assert!(resolve(&found, Anchor::Members(&[])).is_err());
    }

    /// An entry that does not verify is not an entry, however it arrived.
    #[test]
    fn an_unsigned_entry_is_refused() {
        let hers = generate();
        let mut forged = entry("alice", &hers, 4);
        forged.entry.version = 5; // the signature covered 4
        let found = said(vec![("a", forged)]);
        let e = resolve(&found, Anchor::FirstSight).unwrap_err();
        assert!(format!("{e}").contains("does not verify"), "{e}");
    }
}
