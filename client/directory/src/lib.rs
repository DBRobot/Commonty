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
pub const DEFAULT: [&str; 1] = [identity::fleet::DIRECTORY];

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

/// Where a read's credential comes from: this device's token for its own
/// name, minted for directory reads at the one host given and nowhere else.
/// The directory answers members, an entry's own keys and nobody else
/// (docs/dev/gate.md).
pub type TokenSource = std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

static TOKEN: std::sync::RwLock<Option<TokenSource>> = std::sync::RwLock::new(None);

/// every read from here on carries what `t` mints (account::read_as_device)
pub fn read_with(t: Option<TokenSource>) {
    if let Ok(mut v) = TOKEN.write() {
        *v = t;
    }
}

fn token(url: &str) -> Option<String> {
    let host = reqwest::Url::parse(url).ok()?.host_str()?.to_lowercase();
    let t = TOKEN.read().ok()?.clone()?;
    t(&host)
}

/// a read: this device's token, and a proof by each key given
fn read_request(
    http: &reqwest::Client,
    url: &str,
    name: Option<&str>,
    keys: &[&identity::SigningKey],
) -> reqwest::RequestBuilder {
    let mut r = http.get(url);
    if let Some(t) = token(url) {
        r = r.bearer_auth(t);
    }
    if let Some(name) = name {
        let at = identity::now();
        for k in keys {
            r = r.header(
                identity::READ_PROOF_HEADER,
                identity::read_proof(k, name, at),
            );
        }
    }
    r
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
/// A directory that will not say (401: not this device's to read) is one too.
pub async fn fetch(dirs: &[String], name: &str) -> Vec<(String, Result<Option<SignedEntry>>)> {
    fetch_proven(dirs, name, &[]).await
}

/// The same, proven by keys in the entry: its root or paper key, for a
/// machine that holds one and is not (yet) one of its devices.
pub async fn fetch_proven(
    dirs: &[String],
    name: &str,
    keys: &[&identity::SigningKey],
) -> Vec<(String, Result<Option<SignedEntry>>)> {
    let mut out = Vec::new();
    let http = match http() {
        Ok(h) => h,
        Err(e) => return vec![(String::new(), Err(e))],
    };
    for d in dirs {
        let url = format!("{}/{name}", d.trim_end_matches('/'));
        let r = async {
            let r = read_request(&http, &url, Some(name), keys).send().await?;
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
    /// the member list in the signed release (or a root this machine
    /// pinned, as the one id on it): the entry's root is on it, or follows
    /// from one that is by recoveries the handovers prove
    /// (identity::anchored). Someone let in by an invite the release does
    /// not list yet is not anchored until it does.
    Released {
        members: &'a [String],
        revoked: &'a [String],
        handovers: &'a [identity::Handover],
    },
    /// nothing is known yet. Every directory asked has to agree on the
    /// root, and what comes back is worth pinning - or, where a directory
    /// could choose the root, worth showing a person to confirm.
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
    if let Anchor::Root(want) = anchor
        && first.entry.root != want
    {
        bail!(
            "{} is not the root this machine holds for {}",
            first.entry.root,
            first.entry.name
        )
    }
    let newest = good
        .into_iter()
        .max_by_key(|e| e.entry.version)
        .cloned()
        .context("no directory has that entry")?;
    if let Anchor::Released {
        members,
        revoked,
        handovers,
    } = anchor
    {
        identity::anchored(&newest, handovers, members, revoked)?;
    }
    Ok(newest)
}

/// The recoveries each directory took for `name` (identity::Handover), all
/// of them: a proof is checked, not believed, so one more does no harm, and
/// a directory that has none, or is not asked, just proves less.
pub async fn handovers(
    dirs: &[String],
    name: &str,
    keys: &[&identity::SigningKey],
) -> Vec<identity::Handover> {
    let Ok(http) = http() else { return vec![] };
    let mut out = Vec::new();
    for d in dirs {
        let url = format!("{}/{name}/recoveries", d.trim_end_matches('/'));
        let got = async {
            read_request(&http, &url, Some(name), keys)
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .json::<Vec<identity::Handover>>()
                .await
                .ok()
        }
        .await;
        out.extend(got.unwrap_or_default());
    }
    out
}

/// The entry for `name`, anchored to a member list (`Anchor::Released`):
/// fetched, with every directory's handovers for it.
pub async fn resolve_released(
    dirs: &[String],
    name: &str,
    list: &identity::MemberList,
) -> Result<SignedEntry> {
    let found = fetch(dirs, name).await;
    let handovers = handovers(dirs, name, &[]).await;
    resolve(
        &found,
        Anchor::Released {
            members: &list.members,
            revoked: &list.revoked,
            handovers: &handovers,
        },
    )
}

/// The release key a member list must be signed by: fleet/release.pub as
/// this build was made with it, unless the program says otherwise (`dd`
/// for its tests, DD_RELEASE_PUB). Never anything a box says.
static RELEASE_KEY: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

pub fn trust_release(key: Option<String>) {
    if let Ok(mut v) = RELEASE_KEY.write() {
        *v = key;
    }
}

pub fn release_key() -> Result<identity::VerifyingKey> {
    let k = RELEASE_KEY
        .read()
        .ok()
        .and_then(|v| v.clone())
        .unwrap_or_else(|| identity::fleet::RELEASE_KEY.to_string());
    identity::decode_public(&k).context("the release key")
}

/// The newest member list any directory's box has from the releases, signed
/// by the release key (`release_key`): `/_dd/members` beside each
/// directory. A box can withhold it or hand back an older one; the caller
/// that keeps state holds it to the newest it saw (account::released).
pub async fn member_list(dirs: &[String]) -> Result<identity::MemberList> {
    let key = release_key()?;
    let http = http()?;
    let mut best: Option<identity::MemberList> = None;
    let mut why = Vec::new();
    for d in dirs {
        let base = d.trim_end_matches('/');
        let base = base.strip_suffix("/_dd/directory").unwrap_or(base);
        let got = async {
            let s: identity::SignedMemberList = read(&http, &format!("{base}/_dd/members"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            identity::verify_member_list(&s, &key)?;
            anyhow::Ok(s.list)
        }
        .await;
        match got {
            Ok(l) if best.as_ref().is_none_or(|b| l.counter > b.counter) => best = Some(l),
            Ok(_) => {}
            Err(e) => why.push(format!("{d}: {e:#}")),
        }
    }
    best.with_context(|| {
        format!(
            "no signed member list from any directory:\n  {}",
            why.join("\n  ")
        )
    })
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
    let v: Vec<serde_json::Value> = read_request(&http()?, dir, None, &[])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(v.into_iter()
        .filter_map(|l| l["name"].as_str().map(str::to_string))
        .collect())
}

/// a GET of something else a directory box keeps for members (its
/// /_dd/leaving statements): with this device's token, as every read here
pub fn read(http: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    read_request(http, url, None, &[])
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
                recovered_from: vec![],
                devices: vec![Device {
                    fingerprint: fingerprint(&p),
                    public_key: p,
                    added: 1,
                }],
                passkeys: vec![],
                grant: None,
                libraries: vec![],
                terms: None,
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
        let released = |members, revoked| Anchor::Released {
            members,
            revoked,
            handovers: &[],
        };
        assert!(resolve(&found, released(&ids, &[])).is_ok());
        assert!(resolve(&found, released(&[], &[])).is_err());
        assert!(resolve(&found, released(&ids, &ids)).is_err());
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
