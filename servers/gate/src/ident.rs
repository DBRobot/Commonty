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
use std::time::{Duration, Instant};

/// An answer about someone that may not be known yet. Unknown is never
/// No: what cannot be undone (a device off the network, a session ended,
/// anything deleted) is done only on a No, and never while it is Unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Unknown,
}

impl Answer {
    pub fn of(b: bool) -> Self {
        if b { Self::Yes } else { Self::No }
    }
    /// either one: Yes if either is, No only if both are
    pub fn or(self, other: impl FnOnce() -> Answer) -> Answer {
        match self {
            Self::Yes => Self::Yes,
            a => match (a, other()) {
                (_, Self::Yes) => Self::Yes,
                (Self::No, Self::No) => Self::No,
                _ => Self::Unknown,
            },
        }
    }
}

/// how long since the last good read the agreed state still counts as
/// fresh: past it, dd-control has not answered for a while and what is
/// held may be behind
pub const STALE: Duration = Duration::from_secs(90);
/// how far behind now an answer's agreed time may be and still be the
/// fleet's current state, not a dd-control still catching up on the log
pub const BEHIND_MS: u64 = 120_000;

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
        // the judge keeps a v1 item as the signed entry itself
        // (state/ident FromV1's Raw), the contract says `{"entry": ...}`:
        // either reads
        if self.item.get("signature").is_some() {
            return serde_json::from_value(self.item.clone()).ok();
        }
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
    /// the agreed time of the state answered (ms)
    #[serde(default)]
    pub time: u64,
    #[serde(default)]
    pub members: HashMap<String, Member>,
    /// what became of each recent item, by its item hash (the judge's
    /// ItemHash, the gate's item_hash): an item here is in the log already
    #[serde(default)]
    pub outcomes: HashMap<String, String>,
    /// box -> whose it is and the ticket it was enlisted with
    #[serde(default)]
    pub enlisted: HashMap<String, Owner>,
    /// box -> whose ticket it joined with, not enlisted yet (since: when
    /// it joined)
    #[serde(default)]
    pub waiting: HashMap<String, Owner>,
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
    reads: Mutex<Reads>,
}

/// how the reads have gone: what says whether the state held is the
/// fleet's current one (fresh) or only the last one seen
#[derive(Default, Clone, Copy)]
struct Reads {
    /// the last read dd-control answered (a state, or 304: still the same)
    ok: Option<Instant>,
    /// the last state answered was at the fleet's current agreed time, not
    /// one dd-control was still catching up to
    current: bool,
    /// fresh without a break since then
    since: Option<Instant>,
}

impl Reads {
    fn fresh(&self, now: Instant) -> bool {
        self.current && self.ok.is_some_and(|t| now.duration_since(t) <= STALE)
    }
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
            reads: Mutex::new(Reads::default()),
        }
    }

    /// Read, current and recent: what may be acted on where acting cannot
    /// be undone. Last read a while ago, or behind the fleet, it is only
    /// what was last seen - good for letting someone in, never for putting
    /// anyone out.
    pub fn fresh(&self) -> bool {
        self.state().is_some() && self.reads.lock().unwrap().fresh(Instant::now())
    }

    /// how long it has been fresh without a break; None while it is not
    pub fn fresh_for(&self) -> Option<Duration> {
        let now = Instant::now();
        let r = *self.reads.lock().unwrap();
        (self.state().is_some() && r.fresh(now)).then(|| now.duration_since(r.since.unwrap_or(now)))
    }

    /// a read answered: `current` is None for a 304 (as current as the
    /// state it confirms)
    fn answered(&self, current: Option<bool>) {
        let now = Instant::now();
        let mut r = self.reads.lock().unwrap();
        let was = r.fresh(now);
        r.ok = Some(now);
        if let Some(c) = current {
            r.current = c;
        }
        if !r.fresh(now) {
            r.since = None;
        } else if !was || r.since.is_none() {
            r.since = Some(now);
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

    /// One read: the whole state, or nothing new since the height held.
    /// Until a state at the fleet's current time has come, every read asks
    /// for the whole of it: a 304 says only "the same as you hold".
    pub async fn refresh(&self) -> anyhow::Result<bool> {
        let held = self.state();
        let current = self.reads.lock().unwrap().current;
        let mut req = self.http.get(&self.url);
        if let Some(s) = held.as_ref().filter(|_| current) {
            req = req.query(&[("since", s.height)]);
        }
        let res = req.send().await?;
        if res.status() == reqwest::StatusCode::NOT_MODIFIED {
            if held.is_some() {
                self.answered(None);
            }
            return Ok(false);
        }
        let res = res.error_for_status()?;
        let s: State = res.json().await?;
        // never back: a dd-control that started again and is replaying the
        // log answers an older state, which says less than the one held
        if let Some(h) = &held
            && s.height < h.height
        {
            anyhow::bail!(
                "dd-control answered height {}, behind the {} held",
                s.height,
                h.height
            );
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let current = s.time > 0 && now_ms.saturating_sub(s.time) <= BEHIND_MS;
        if !current {
            eprintln!(
                "identity: dd-control's state is {} s behind; not acting on it yet",
                now_ms.saturating_sub(s.time) / 1000
            );
        }
        *self.state.lock().unwrap() = Some(Arc::new(s));
        self.answered(Some(current));
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

    /// as if just read, current
    #[cfg(test)]
    pub fn set(&self, s: State) {
        *self.state.lock().unwrap() = Some(Arc::new(s));
        self.answered(Some(true));
    }

    /// as last read `ago`, current then
    #[cfg(test)]
    pub fn read_ago(&self, ago: Duration) {
        let mut r = self.reads.lock().unwrap();
        r.ok = Instant::now().checked_sub(ago);
        r.since = r.ok;
    }

    /// as read now, but behind the fleet (a dd-control catching up)
    #[cfg(test)]
    pub fn set_behind(&self, s: State) {
        *self.state.lock().unwrap() = Some(Arc::new(s));
        self.answered(Some(false));
    }
}

/// The shadow log's one line for a check the two answers differ on: who,
/// which check, the replay's answer and the agreed state's, and what the
/// agreed state has of them (status, access, why their boxes count or not).
/// The same line is written once in ten minutes: a check made on every
/// request would otherwise fill the journal with it.
pub fn shadow_differs(
    name: &str,
    check: &str,
    replay: &dyn std::fmt::Display,
    agreed: &dyn std::fmt::Display,
    m: Option<&Member>,
) -> bool {
    let line = format!(
        "identity shadow: {name}: {check}: replay {replay}, agreed {agreed} ({})",
        agreed_said(m)
    );
    static SEEN: std::sync::LazyLock<Mutex<HashMap<String, std::time::Instant>>> =
        std::sync::LazyLock::new(Default::default);
    let now = std::time::Instant::now();
    let mut seen = SEEN.lock().unwrap();
    seen.retain(|_, at| now.duration_since(*at) < SHADOW_AGAIN);
    if seen.len() >= 4096 || seen.contains_key(&line) {
        return false;
    }
    seen.insert(line.clone(), now);
    eprintln!("{line}");
    true
}

/// how long the shadow log keeps quiet about a line it wrote
pub const SHADOW_AGAIN: Duration = Duration::from_secs(600);

/// what the agreed state has of someone, for the shadow log: public words
/// only (a status, an access, box names and why each counts)
pub fn agreed_said(m: Option<&Member>) -> String {
    let Some(m) = m else {
        return "agreed state: no such name".into();
    };
    let boxes: Vec<String> = m
        .enlisted
        .iter()
        .map(|b| {
            format!(
                "{} {}",
                b["box"].as_str().unwrap_or("?"),
                b["why"].as_str().unwrap_or("?")
            )
        })
        .collect();
    let mut s = format!("agreed status {}, access {}", m.status, m.access);
    if !boxes.is_empty() {
        s.push_str(&format!(", boxes: {}", boxes.join("; ")));
    }
    s
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

    /// one string field of the body, as it came: where an item goes,
    /// before its signature is checked; never believed on its own
    pub fn field(&self, key: &str) -> Option<&str> {
        self.body.get(key).and_then(|n| n.as_str())
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
    fn unknown_is_never_no() {
        use Answer::*;
        for (a, b, want) in [
            (Yes, Yes, Yes),
            (Yes, No, Yes),
            (Yes, Unknown, Yes),
            (No, Yes, Yes),
            (Unknown, Yes, Yes),
            (No, No, No),
            (No, Unknown, Unknown),
            (Unknown, No, Unknown),
            (Unknown, Unknown, Unknown),
        ] {
            assert_eq!(a.or(|| b), want, "{a:?} or {b:?}");
        }
        assert_eq!(Answer::of(true), Yes);
        assert_eq!(Answer::of(false), No);
    }

    /// A stand-in dd-control: /identity answers what `answer` holds (a
    /// status and a body), or nothing at all while it holds None
    async fn control(
        answer: Arc<Mutex<Option<(u16, String)>>>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        use axum::{Router, extract::RawQuery, routing::get};
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = asked.clone();
        let app = Router::new().route(
            "/identity",
            get(move |RawQuery(q): RawQuery| {
                let answer = answer.clone();
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push(q.unwrap_or_default());
                    let (code, body) = answer
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or((503, String::new()));
                    (axum::http::StatusCode::from_u16(code).unwrap(), body)
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}"), asked)
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn state_at(height: u64, time: u64) -> String {
        serde_json::json!({
            "height": height, "time": time,
            "members": {"david": {"status": "member", "access": "invite"}},
        })
        .to_string()
    }

    #[tokio::test]
    async fn fresh_only_once_read_current_and_recent() {
        let answer: Arc<Mutex<Option<(u16, String)>>> = Arc::default();
        let (url, asked) = control(answer.clone()).await;
        let a = Agreed::new(&url);
        // down: nothing held, nothing fresh
        assert!(a.refresh().await.is_err());
        assert!(a.state().is_none() && !a.fresh() && a.fresh_for().is_none());

        // up, but still replaying an old stretch of the log: held, not fresh,
        // and asked for whole again (a 304 would only say "the same")
        *answer.lock().unwrap() = Some((200, state_at(5, now_ms() - 10 * BEHIND_MS)));
        assert!(a.refresh().await.unwrap());
        assert!(a.state().is_some() && !a.fresh());
        a.refresh().await.unwrap();
        assert!(
            asked.lock().unwrap().iter().all(|q| q.is_empty()),
            "{asked:?}"
        );

        // caught up: fresh, and from now on only asked what changed
        *answer.lock().unwrap() = Some((200, state_at(9, now_ms())));
        a.refresh().await.unwrap();
        assert!(a.fresh());
        assert!(a.fresh_for().unwrap() < Duration::from_secs(5));
        *answer.lock().unwrap() = Some((304, String::new()));
        assert!(!a.refresh().await.unwrap());
        assert!(a.fresh(), "a 304 says the state held is still the one");
        assert_eq!(asked.lock().unwrap().last().unwrap(), "since=9");

        // dd-control started again and answers from further back: refused,
        // the newer state is kept
        *answer.lock().unwrap() = Some((200, state_at(3, now_ms())));
        assert!(a.refresh().await.is_err());
        assert_eq!(a.state().unwrap().height, 9);

        // and not answering for a while: what is held is no longer fresh
        *answer.lock().unwrap() = None;
        assert!(a.refresh().await.is_err());
        a.read_ago(STALE + Duration::from_secs(1));
        assert!(!a.fresh() && a.fresh_for().is_none());
        assert!(a.state().is_some(), "still held, for letting people in");
    }

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
    fn a_v1_item_reads_as_the_judge_keeps_it_and_as_the_contract_says() {
        let k = identity::generate();
        let e = identity::sign(
            identity::Entry {
                name: "ann".into(),
                root: identity::encode_public(&k.verifying_key()),
                recovery: String::new(),
                recovered_from: vec![],
                devices: vec![],
                passkeys: vec![],
                grant: None,
                libraries: vec![],
                terms: None,
                removed_boxes: vec![],
                version: 1,
                updated: 1,
            },
            &k,
        )
        .unwrap();
        for item in [
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({ "entry": e }),
        ] {
            let m = Member {
                item,
                ..Default::default()
            };
            assert_eq!(m.entry().unwrap().entry.name, "ann");
        }
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
