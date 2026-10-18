//! Who is who, as the control plane judged it (docs/dev/identity-state.md):
//! the agreed identity state dd-control serves at /identity. The gate reads
//! it instead of judging the directory itself, once `VERIFY_IDENTITY_FROM`
//! says so:
//!
//! - `replay` (today): the gate's own replay of the log (directory.rs);
//! - `shadow`: both, the replay's answer served, every difference logged;
//! - `agreed`: the agreed state alone.
//!
//! The state is kept here as last fetched, refreshed in the background
//! (`?since=<height>` answers 304 when nothing changed), so a lookup on a
//! request is a read of memory, never a wait on the control plane.
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum From {
    #[default]
    Replay,
    Shadow,
    Agreed,
}

impl From {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "" | "replay" => Some(Self::Replay),
            "shadow" => Some(Self::Shadow),
            "agreed" => Some(Self::Agreed),
            _ => None,
        }
    }
}

/// One person, as the agreed state has them (contract §2). Fields this
/// gate does not read are left out; unknown ones are ignored.
#[derive(Clone, Debug, Deserialize, Default)]
pub struct Member {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub ids: Vec<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub left: u64,
    #[serde(default)]
    pub suspended: Option<String>,
    #[serde(default)]
    pub boxes: Vec<String>,
    /// the item as agreed, verbatim: a v1 `{"entry": SignedEntry}` or a v2
    /// envelope whose body is the entry's fields
    #[serde(default)]
    pub item: serde_json::Value,
    /// what they may use: "invite", "box" or "none" (state/ident access.go)
    #[serde(default)]
    pub access: String,
    /// their enlisted boxes, each with whether it keeps their access
    #[serde(default)]
    pub enlisted: Vec<serde_json::Value>,
}

impl Member {
    /// The person's entry, from the item that was agreed. Nothing is
    /// checked here: the judge did that, once.
    pub fn entry(&self) -> Option<identity::SignedEntry> {
        if let Some(e) = self.item.get("entry") {
            return serde_json::from_value(e.clone()).ok();
        }
        if self.item.get("v").and_then(|v| v.as_u64()) == Some(2)
            && self.item.get("kind").and_then(|k| k.as_str()) == Some("entry")
        {
            use base64::Engine;
            let body = base64::engine::general_purpose::STANDARD
                .decode(self.item.get("body")?.as_str()?)
                .ok()?;
            let entry: identity::Entry = serde_json::from_slice(&body).ok()?;
            return Some(identity::SignedEntry {
                entry,
                signature: self.item.get("sig")?.as_str()?.to_string(),
                recovery_signature: self
                    .item
                    .get("recovery_sig")
                    .and_then(|s| s.as_str())
                    .map(str::to_string),
            });
        }
        None
    }
    pub fn is_member(&self) -> bool {
        self.status == "member"
    }
    /// The fleet's services are theirs: an invite, or a box that counts
    pub fn has_access(&self) -> bool {
        self.access == "invite" || self.access == "box"
    }
    /// Signed up and not shut out (left, suspended, removed): whatever
    /// their access, what is theirs alone (their boxes, a home controller)
    pub fn not_banned(&self) -> bool {
        self.status == "member" || self.status == "none"
    }
    pub fn has_left(&self) -> bool {
        self.status == "left"
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct State {
    #[serde(default)]
    pub height: u64,
    #[serde(default)]
    pub members: HashMap<String, Member>,
    /// what became of each recent item, by its item hash (the judge's
    /// ItemHash, the gate's item_hash): an item here is in the log already
    #[serde(default)]
    pub outcomes: HashMap<String, String>,
    /// box -> whose it is and the ticket it was enlisted with
    #[serde(default)]
    pub enlisted: HashMap<String, Owner>,
}

/// An enlisted box's member and the ticket it was enlisted with
#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
pub struct Owner {
    #[serde(default)]
    pub ticket: String,
    #[serde(default)]
    pub owner: String,
    /// enlisted, agreed ms
    #[serde(default)]
    pub since: u64,
}

/// The agreed identity state as last read from dd-control.
pub struct Agreed {
    url: String,
    http: reqwest::Client,
    state: Mutex<Option<Arc<State>>>,
}

impl Agreed {
    /// `control` is dd-control's address on this box (VERIFY_CONTROL)
    pub fn new(control: &str) -> Self {
        Self {
            url: format!("{}/identity", control.trim_end_matches('/')),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("an http client"),
            state: Mutex::new(None),
        }
    }

    /// None until the first read has come back: a reader then knows
    /// nothing, which is not the same as knowing a name is nobody's
    pub fn state(&self) -> Option<Arc<State>> {
        self.state.lock().unwrap().clone()
    }

    pub fn member(&self, name: &str) -> Option<Member> {
        self.state()?.members.get(name).cloned()
    }

    /// One read: the whole state, or nothing new since the height held
    pub async fn refresh(&self) -> anyhow::Result<bool> {
        let since = self.state().map(|s| s.height);
        let mut req = self.http.get(&self.url);
        if let Some(h) = since {
            req = req.query(&[("since", h)]);
        }
        let res = req.send().await?;
        if res.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(false);
        }
        let res = res.error_for_status()?;
        let s: State = res.json().await?;
        *self.state.lock().unwrap() = Some(Arc::new(s));
        Ok(true)
    }

    /// Kept fresh for as long as the gate runs: a second between reads,
    /// longer after a failure
    pub async fn follow(self: Arc<Self>) {
        let mut wait = Duration::from_secs(1);
        loop {
            match self.refresh().await {
                Ok(_) => wait = Duration::from_secs(1),
                Err(e) => {
                    eprintln!("identity: dd-control did not answer: {e:#}");
                    wait = (wait * 2).min(Duration::from_secs(30));
                }
            }
            tokio::time::sleep(wait).await;
        }
    }

    #[cfg(test)]
    pub fn set(&self, s: State) {
        *self.state.lock().unwrap() = Some(Arc::new(s));
    }
}

/// What two answers about one person differ in, for the shadow log: field
/// names and values that are public (names, versions, fingerprints, key
/// ids), never a secret
pub fn entry_differences(
    replay: Option<&identity::SignedEntry>,
    agreed: Option<&identity::SignedEntry>,
) -> Vec<String> {
    let (r, a) = match (replay, agreed) {
        (None, None) => return vec![],
        (Some(_), None) => return vec!["entry: replay has one, agreed none".into()],
        (None, Some(_)) => return vec!["entry: agreed has one, replay none".into()],
        (Some(r), Some(a)) => (&r.entry, &a.entry),
    };
    let mut d = vec![];
    if r.root != a.root {
        d.push("root".into());
    }
    if r.recovery != a.recovery {
        d.push("recovery".into());
    }
    if r.version != a.version {
        d.push(format!(
            "version: replay {}, agreed {}",
            r.version, a.version
        ));
    }
    if r.recovered_from != a.recovered_from {
        d.push("recovered_from".into());
    }
    let fps = |e: &identity::Entry| {
        let mut v: Vec<String> = e.devices.iter().map(|x| x.fingerprint.clone()).collect();
        v.sort();
        v
    };
    if fps(r) != fps(a) {
        d.push(format!("devices: replay {:?}, agreed {:?}", fps(r), fps(a)));
    }
    let pks = |e: &identity::Entry| {
        let mut v: Vec<String> = e.passkeys.iter().map(|x| x.id.clone()).collect();
        v.sort();
        v
    };
    if pks(r) != pks(a) {
        d.push(format!(
            "passkeys: replay {:?}, agreed {:?}",
            pks(r),
            pks(a)
        ));
    }
    if r.grant.is_some() != a.grant.is_some() {
        d.push("grant".into());
    }
    let libs = |e: &identity::Entry| {
        let mut v: Vec<String> = e.libraries.iter().map(|x| x.id.clone()).collect();
        v.sort();
        v
    };
    if libs(r) != libs(a) {
        d.push("libraries".into());
    }
    d
}

/// A v2 envelope (contract §1), as far as the gate needs to know it: that
/// it is one, its kind, and the name its body is about. The gate checks no
/// signature: the control plane judges it.
pub struct Envelope {
    pub kind: String,
    body: serde_json::Value,
    body_b64: String,
    raw: Vec<u8>,
}

impl Envelope {
    /// a JSON object with "v": 2 (a v1 item never has a "v")
    pub fn is_one(bytes: &[u8]) -> bool {
        serde_json::from_slice::<serde_json::Value>(bytes)
            .ok()
            .and_then(|v| v.get("v").and_then(|x| x.as_u64()))
            == Some(2)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        use base64::Engine;
        let v: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| format!("not json: {e}"))?;
        if v.get("v").and_then(|x| x.as_u64()) != Some(2) {
            return Err("not a v2 envelope".into());
        }
        let kind = v
            .get("kind")
            .and_then(|k| k.as_str())
            .ok_or("an envelope with no kind")?
            .to_string();
        if v.get("sig")
            .and_then(|s| s.as_str())
            .is_none_or(str::is_empty)
        {
            return Err("an envelope with no signature".into());
        }
        let body_b64 = v
            .get("body")
            .and_then(|b| b.as_str())
            .ok_or("an envelope with no body")?
            .to_string();
        let b = base64::engine::general_purpose::STANDARD
            .decode(&body_b64)
            .map_err(|e| format!("body is not base64: {e}"))?;
        let body: serde_json::Value =
            serde_json::from_slice(&b).map_err(|e| format!("body is not json: {e}"))?;
        Ok(Self {
            kind,
            body,
            body_b64,
            raw: bytes.to_vec(),
        })
    }

    /// the name the item is about: an entry's or a leaving's `name`
    pub fn name(&self) -> Option<&str> {
        self.field("name")
    }

    /// a string field of the body
    pub fn field(&self, k: &str) -> Option<&str> {
        self.body.get(k).and_then(|n| n.as_str())
    }

    /// the body as it travels, base64
    pub fn body_b64(&self) -> &str {
        &self.body_b64
    }

    /// the one item hash (identity::item_hash): what was signed and its
    /// signatures, read from the envelope as it came - one item however its
    /// outer JSON is written
    pub fn signed_hash(&self) -> String {
        serde_json::from_slice::<serde_json::Value>(&self.raw)
            .map(|v| identity::item_hash(&v))
            .unwrap_or_default()
    }

    /// the item's name in the log, for a writer waiting on its outcome:
    /// sha256 of the bytes as submitted
    pub fn hash(&self) -> String {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(&self.raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse() {
        assert_eq!(From::parse(""), Some(From::Replay));
        assert_eq!(From::parse("shadow"), Some(From::Shadow));
        assert_eq!(From::parse("agreed"), Some(From::Agreed));
        assert_eq!(From::parse("judge"), None);
    }

    fn env(kind: &str, body: serde_json::Value) -> Vec<u8> {
        use base64::Engine;
        serde_json::to_vec(&serde_json::json!({
            "v": 2, "kind": kind,
            "body": base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&body).unwrap()),
            "sig": "c2ln",
        }))
        .unwrap()
    }

    #[test]
    fn an_envelope_is_told_from_a_v1_item() {
        let e = env("entry", serde_json::json!({"name": "ann", "version": 2}));
        assert!(Envelope::is_one(&e));
        assert!(!Envelope::is_one(
            br#"{"entry": {"name": "ann"}, "signature": "x"}"#
        ));
        let p = Envelope::parse(&e).unwrap();
        assert_eq!(p.kind, "entry");
        assert_eq!(p.name(), Some("ann"));
        assert_eq!(p.hash().len(), 64);
    }

    #[test]
    fn a_broken_envelope_is_refused() {
        assert!(
            Envelope::parse(br#"{"v": 2, "kind": "entry", "body": "!!", "sig": "x"}"#).is_err()
        );
        assert!(Envelope::parse(br#"{"v": 2, "kind": "entry", "body": "e30="}"#).is_err());
        assert!(Envelope::parse(br#"{"v": 2, "body": "e30=", "sig": "x"}"#).is_err());
    }
}
