//! The network's door: an admitted device asks for a key to join the
//! fleet's own network, the gate asks Headscale for one in that member's
//! name, single use, minutes to live. The control server itself is behind
//! the same host; this only vouches for who is asking.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};

use std::collections::HashMap;
use std::sync::Mutex;

use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use p256::pkcs8::DecodePublicKey;

use crate::App;

/// how long a join key is good for: long enough to hand it over, no more
const TTL: Duration = Duration::from_secs(600);

/// A node the reaper would take off
#[derive(Debug, Clone)]
struct Off {
    id: String,
    /// headscale's id as a number, for the order when `made` is missing
    num: u64,
    /// when headscale made it (RFC 3339, sorts as text)
    made: String,
    label: String,
    why: String,
    /// whose: the person, or the computer's owner
    whose: String,
    /// a person's own device (not a computer's node): what the brake counts
    person: bool,
}

impl Off {
    fn names(off: &[Off]) -> String {
        off.iter()
            .map(|o| format!("{} ({})", o.label, o.why))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The state a reaper pass goes by, as the brake sees it: the agreed
/// state's height (0 off agreed) and who has access in it
pub type Mark = (u64, std::collections::BTreeSet<String>);

/// What holds the reaper back (App::reap_guard)
pub struct Guard<'a> {
    /// whose devices never come off (reap_keep): the fleet's admin and the
    /// people the release names. One of them reading as no member is a fault
    /// to look into, not a reason to cut them off
    pub keep: std::collections::BTreeSet<String>,
    /// why nothing may come off now, if anything holds it
    pub hold: Option<String>,
    /// who has access by the state this pass goes by: what the brake holds
    /// against the last state the reaper acted on
    pub members: std::collections::BTreeSet<String>,
    /// the sanity brake on the state itself; None: none (tests)
    pub brake: Option<Arc<Brake>>,
    /// the height of the state `members` is from
    pub height: u64,
    /// the state as it is now. A pass judges each device as it goes, by the
    /// state of that moment; one that moved since `members` was read would
    /// be judged by one state and braked by another, so it takes nothing off
    pub now: Option<&'a (dyn Fn() -> Mark + Sync)>,
}

impl Guard<'_> {
    #[cfg(test)]
    pub fn open() -> Self {
        Self {
            keep: Default::default(),
            hold: None,
            members: Default::default(),
            brake: None,
            height: 0,
            now: None,
        }
    }

    /// whether the state moved since `members` was read
    fn moved(&self) -> bool {
        self.now.is_some_and(|now| {
            let (h, m) = now();
            h != self.height || m != self.members
        })
    }
}

/// The sanity brake on the state the reaper goes by. A person known to be
/// no member loses every device, however many; but a state that has
/// shrunk sharply since the last one acted on (empty, or more than a
/// quarter of its members gone at once), or one that would turn more than a
/// couple of people out in one pass, is more likely wrong than true: then
/// nothing comes off, the last good state is kept, and the brake is on - a
/// loud log line every pass, and `dd_reap_brake 1` in this box's facts,
/// which the fleet's alerts mail - until the state is consistent again or
/// the fleet's owner acknowledges that exact state (POST
/// /_dd/network/reap-brake, lib.rs reap_brake_ack). An empty state, never.
pub struct Brake {
    state: Mutex<BrakeState>,
    /// where the last good state is kept across restarts
    file: Option<std::path::PathBuf>,
    /// the facts file the alerts read (textfile collector)
    facts: Option<std::path::PathBuf>,
    /// the most people one pass turns out
    most_people: usize,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct BrakeState {
    /// who had access in the last state the reaper acted on
    acted: Option<std::collections::BTreeSet<String>>,
    /// on since (s), why, and the refused state's hash (`refused_hash`)
    #[serde(skip)]
    on: Option<(u64, String, String)>,
    /// the owner's acknowledgement: of that refused state alone, until (s)
    #[serde(skip)]
    ack: Option<(String, u64)>,
}

/// the brake's word for a state that gives nobody access
const EMPTY: &str = "the state gives nobody access";

/// how long an owner's acknowledgement waits for its pass
pub const ACK_FOR: u64 = 600;

/// What an acknowledgement is tied to: the state refused (who has access)
/// and who it would turn out, so an ack of one state never lets another
/// through
pub fn refused_hash(
    members: &std::collections::BTreeSet<String>,
    people: &std::collections::BTreeSet<String>,
) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"commonty reap-brake v1\0");
    for m in members {
        h.update(m.as_bytes());
        h.update(b"\0");
    }
    h.update(b"\0");
    for p in people {
        h.update(p.as_bytes());
        h.update(b"\0");
    }
    format!("{:x}", h.finalize())
}

/// why an acknowledgement was not taken
#[derive(Debug, PartialEq, Eq)]
pub enum AckRefused {
    /// the brake is off: nothing to acknowledge
    Off,
    /// not the state the brake refused (it changed since, or a guess)
    NotThatState,
    /// an empty state is never reaped from, acknowledged or not
    Empty,
}

impl Brake {
    pub fn new(
        file: Option<std::path::PathBuf>,
        facts: Option<std::path::PathBuf>,
        most_people: usize,
    ) -> Self {
        let acted = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice::<BrakeState>(&b).ok())
            .and_then(|s| s.acted);
        let b = Self {
            state: Mutex::new(BrakeState {
                acted,
                ..Default::default()
            }),
            file,
            facts,
            most_people,
        };
        b.report();
        b
    }

    /// from the environment: VERIFY_REAP_PEOPLE (2), VERIFY_REAP_FACTS
    pub fn from_env(state_dir: &std::path::Path) -> Self {
        Self::new(
            Some(state_dir.join("reap-acted.json")),
            std::env::var("VERIFY_REAP_FACTS")
                .ok()
                .filter(|s| !s.is_empty())
                .map(Into::into),
            env_secs("VERIFY_REAP_PEOPLE", 2) as usize,
        )
    }

    /// on, and why
    pub fn on(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .on
            .as_ref()
            .map(|(_, w, _)| w.clone())
    }

    /// on: why, and the hash an acknowledgement must name
    pub fn refused(&self) -> Option<(String, String)> {
        self.state
            .lock()
            .unwrap()
            .on
            .as_ref()
            .map(|(_, w, h)| (w.clone(), h.clone()))
    }

    /// The owner's word that the state the brake refused is right: taken
    /// only while the brake is on, only for that exact state (`hash`), and
    /// only for the next ACK_FOR seconds; never for an empty state
    pub fn acknowledge(&self, hash: &str, now: u64) -> Result<(), AckRefused> {
        let mut st = self.state.lock().unwrap();
        let Some((_, why, h)) = st.on.as_ref() else {
            return Err(AckRefused::Off);
        };
        if h != hash {
            return Err(AckRefused::NotThatState);
        }
        if why == EMPTY {
            return Err(AckRefused::Empty);
        }
        st.ack = Some((hash.to_string(), now + ACK_FOR));
        eprintln!(
            "network: the owner acknowledged the state the brake refused; the next pass within {ACK_FOR} s goes by it if the state is still that one"
        );
        Ok(())
    }

    /// May a pass that would turn `people` out go by `members`? Ok, and
    /// `members` is the last good state from now; or why not, and the brake
    /// is on
    fn check(
        &self,
        members: &std::collections::BTreeSet<String>,
        people: &std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        let hash = refused_hash(members, people);
        // an acknowledgement is spent on its pass, and counts only for the
        // state it named, before it runs out
        let acked = st
            .ack
            .take()
            .is_some_and(|(h, until)| h == hash && crate::session::now() <= until);
        let why = if members.is_empty() {
            Some(EMPTY.to_string())
        } else if acked {
            None
        } else if let Some(lost) = st.acted.as_ref().and_then(|a| {
            let lost = a.difference(members).count();
            (lost * 4 > a.len()).then_some((lost, a.len()))
        }) {
            Some(format!(
                "{} of the {} people with access in the last state acted on have none in this one",
                lost.0, lost.1
            ))
        } else if people.len() > self.most_people {
            Some(format!(
                "{} people would be turned out in one pass, more than {}: {}",
                people.len(),
                self.most_people,
                people.iter().cloned().collect::<Vec<_>>().join(", ")
            ))
        } else {
            None
        };
        match why {
            Some(w) => {
                let since = st
                    .on
                    .as_ref()
                    .map_or_else(crate::session::now, |(t, _, _)| *t);
                st.on = Some((since, w.clone(), hash));
                drop(st);
                self.report();
                Err(w)
            }
            None => {
                let was = st.on.take();
                st.acted = Some(members.clone());
                if let Some(f) = &self.file {
                    let b = serde_json::to_vec(&*st).unwrap_or_default();
                    let tmp = f.with_extension("tmp");
                    if let Err(e) = std::fs::write(&tmp, b).and_then(|()| std::fs::rename(&tmp, f))
                    {
                        eprintln!("network: keeping the last good state: {e}");
                    }
                }
                drop(st);
                if was.is_some() {
                    eprintln!("network: the state is consistent again; the brake is off");
                }
                self.report();
                Ok(())
            }
        }
    }

    /// `dd_reap_brake` for the fleet's alerts
    fn report(&self) {
        let Some(f) = &self.facts else { return };
        let on = self.state.lock().unwrap().on.as_ref().map(|(t, _, _)| *t);
        let text = format!(
            "# HELP dd_reap_brake 1 while the network reaper refuses a state that looks wrong\n# TYPE dd_reap_brake gauge\ndd_reap_brake {}\n# TYPE dd_reap_brake_since_seconds gauge\ndd_reap_brake_since_seconds {}\n",
            u8::from(on.is_some()),
            on.unwrap_or(0)
        );
        let tmp = f.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, f)) {
            eprintln!("network: writing {}: {e}", f.display());
        }
    }
}

fn env_secs(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// how often the reaper runs; VERIFY_REAP_SECS shortens it, for a test
pub fn reap_every() -> Duration {
    Duration::from_secs(env_secs("VERIFY_REAP_SECS", 300).max(1))
}

/// how long who-is-who must have been known before anything comes off
/// (VERIFY_REAP_GRACE_SECS): a gate that just started, or just read the
/// agreed state again after an outage, waits it out
pub fn reap_grace() -> Duration {
    Duration::from_secs(env_secs("VERIFY_REAP_GRACE_SECS", 120))
}

/// the people the release names (fleet/policy.json owner and people),
/// VERIFY_PEOPLE, comma separated
pub fn people_named() -> std::collections::BTreeSet<String> {
    std::env::var("VERIFY_PEOPLE")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Who the reaper never takes off: the people the release names and the
/// fleet's admin, whom a wrong state must not lock out. Not a box's host:
/// a host whose membership is revoked loses their devices like anyone, and
/// the brake and the state-moved guard hold a state that drops them wrongly
pub fn reap_keep(
    people: std::collections::BTreeSet<String>,
    admin: Option<&str>,
) -> std::collections::BTreeSet<String> {
    let mut keep = people;
    keep.extend(admin.map(str::to_string));
    keep
}

pub struct Door {
    /// headscale's api, on this box
    api: String,
    /// what a device is told to join
    control_url: String,
    key: String,
    /// read at each call when set: headscale makes it where it runs, and
    /// under failover that is whichever box holds its lease, so a box
    /// without it yet still starts its gate
    key_file: Option<std::path::PathBuf>,
    http: reqwest::Client,
    /// the boxes that may ask for a key of their own: the key each signs
    /// with (in its TPM) and the age recipient the answer is sealed to (its
    /// secrets key, also in its TPM)
    boxes: HashMap<String, BoxKeys>,
    /// age, with whatever plugin the recipients need on its path
    age: String,
    /// when each box last had a key, and the time it signed for it
    asked: Mutex<HashMap<String, (u64, u64)>>,
    /// each remote computer's node, by the key it joined with (its
    /// preauth key's id): whose computer, and which. What a computer's
    /// node is to the policy and the reaper comes from here, never from a
    /// name the guest gives itself
    bound: Mutex<HashMap<String, Bound>>,
    /// where `bound` is kept across restarts; None: in memory
    bound_file: Option<std::path::PathBuf>,
    /// each computer's last fresh agreed state, and when it was read (ms):
    /// what its node is kept or taken off by while dd-control is unread
    last_good: Mutex<HashMap<String, (Snapshot, i64)>>,
    /// the computers the release lets at the boxes' admin ports
    /// (policy.json computers.admin)
    admin_computers: std::collections::HashSet<String>,
    /// the fleet's admin: only their computers count on that list
    admin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bound {
    pub owner: String,
    pub id: String,
    /// the box it was given to, holding the computer's lease: the node is
    /// that box's guest's, and goes when the lease moves on
    #[serde(rename = "box", default)]
    pub on: String,
    /// when its key was made (s): a key never used is forgotten after a day
    pub made: u64,
}

/// a remote computer's node's tag (nix/modules/net/headscale.nix)
const COMPUTER_TAG: &str = "tag:computer";
/// a computer the release lists (policy.json computers.admin), its owner the
/// fleet's admin: beside tag:computer, the box-admin ports that tag has not
const ADMIN_COMPUTER_TAG: &str = "tag:admin-computer";

/// a box installer's node, on the network for its disk's backup alone
const INSTALLER_TAG: &str = "tag:installer";
/// how long an installer's node may stay: a whole disk's backup, and no more
const INSTALLER_MAX_SECS: u64 = 24 * 3600;

pub struct BoxKeys {
    signs: VerifyingKey,
    recipient: String,
}

/// how far a box's clock may be from this one's
const SKEW: u64 = 120;
/// one key a box a minute
const EVERY: u64 = 60;

pub fn from_env() -> Result<Option<Door>> {
    let Ok(api) = std::env::var("VERIFY_HEADSCALE_API") else {
        return Ok(None);
    };
    let key_file = std::path::PathBuf::from(
        std::env::var("VERIFY_HEADSCALE_KEY_FILE").context("VERIFY_HEADSCALE_KEY_FILE")?,
    );
    let bound_file = std::env::var("VERIFY_DIR")
        .ok()
        .map(|d| std::path::Path::new(&d).join("computer-nodes.json"));
    Ok(Some(Door {
        api: api.trim_end_matches('/').to_string(),
        control_url: std::env::var("VERIFY_HEADSCALE_URL").context("VERIFY_HEADSCALE_URL")?,
        key: String::new(),
        key_file: Some(key_file),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?,
        boxes: match std::env::var("VERIFY_NETWORK_BOXES") {
            Ok(f) => boxes_from(&std::fs::read_to_string(&f).with_context(|| f.clone())?)?,
            Err(_) => HashMap::new(),
        },
        age: std::env::var("VERIFY_AGE").unwrap_or_else(|_| "age".into()),
        asked: Mutex::default(),
        bound: Mutex::new(
            bound_file
                .as_ref()
                .and_then(|f| std::fs::read(f).ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
        ),
        bound_file,
        last_good: Mutex::default(),
        admin_computers: std::env::var("VERIFY_NETWORK_ADMIN_COMPUTERS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        admin: std::env::var("VERIFY_ADMIN").ok().filter(|s| !s.is_empty()),
    }))
}

/// {"node2": {"key": "<base64 SPKI P-256>", "recipient": "age1..."}}
fn boxes_from(json: &str) -> Result<HashMap<String, BoxKeys>> {
    let v: HashMap<String, serde_json::Value> = serde_json::from_str(json)?;
    v.into_iter()
        .map(|(name, b)| Ok((name.clone(), box_keys(&name, &b)?)))
        .collect()
}

/// one box's keys: {"key": "<base64 SPKI P-256>", "recipient": "age1..."}
fn box_keys(name: &str, b: &serde_json::Value) -> Result<BoxKeys> {
    use base64::Engine;
    let der = base64::engine::general_purpose::STANDARD
        .decode(b["key"].as_str().context("key")?)
        .with_context(|| format!("{name}'s key"))?;
    let signs = VerifyingKey::from_public_key_der(&der)
        .map_err(|e| anyhow::anyhow!("{name}'s key: {e}"))?;
    let recipient = b["recipient"].as_str().context("recipient")?.to_string();
    Ok(BoxKeys { signs, recipient })
}

/// a name a box joins under on its own key (state.JoinName): j and ten hex
fn joined_name(name: &str) -> bool {
    name.len() == 11
        && name.starts_with('j')
        && name[1..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// what a box signs (servers/system/control/cmd/dd-control, joinMessage)
fn join_message(name: &str, time: u64) -> Vec<u8> {
    format!("commonty box join v1\0{name}\0{time}").into_bytes()
}

/// what a box signs to ask for a network key for the remote computer it
/// runs (dd-control, computerJoinMessage)
fn computer_join_message(name: &str, id: &str, time: u64) -> Vec<u8> {
    format!("commonty computer join v1\0{name}\0{id}\0{time}").into_bytes()
}

/// a TPM signs plain r||s, a test's soft key DER: either will do
fn signature(b64: &str) -> Option<Signature> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64.trim_end_matches('='))
        .ok()?;
    Signature::from_slice(&raw)
        .or_else(|_| Signature::from_der(&raw))
        .ok()
}

impl Door {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let key = match &self.key_file {
            Some(f) => std::fs::read_to_string(f)
                .with_context(|| {
                    format!("headscale's api key ({}): not on this box yet", f.display())
                })?
                .trim()
                .to_string(),
            None => self.key.clone(),
        };
        let mut r = self
            .http
            .request(method, format!("{}{path}", self.api))
            .bearer_auth(key);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await?;
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("headscale: {status} {text}");
        }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }

    /// the member's user in headscale, made on first ask
    async fn user_id(&self, name: &str) -> Result<u64> {
        let v = self
            .call(
                reqwest::Method::GET,
                &format!("/api/v1/user?name={name}"),
                None,
            )
            .await?;
        // matched by name here: the filter is not trusted to
        if let Some(u) = v["users"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|u| u["name"].as_str() == Some(name))
        {
            return id_of(u);
        }
        let v = self
            .call(
                reqwest::Method::POST,
                "/api/v1/user",
                Some(serde_json::json!({ "name": name })),
            )
            .await?;
        id_of(&v["user"])
    }

    /// a node's tags, as headscale 0.28 gives them (tags), or an older one
    fn tags(n: &serde_json::Value) -> Vec<&str> {
        ["tags", "forcedTags", "validTags"]
            .iter()
            .flat_map(|k| n[*k].as_array().into_iter().flatten())
            .filter_map(|t| t.as_str())
            .collect()
    }

    /// a remote computer's node, by either of a computer's tags
    fn is_computer(tags: &[&str]) -> bool {
        tags.contains(&COMPUTER_TAG) || tags.contains(&ADMIN_COMPUTER_TAG)
    }

    /// the tags a computer's key carries: an id on the release's list that
    /// is not the fleet's admin's own gets no key at all, whoever made a
    /// computer by that id
    fn computer_tags(&self, id: &str, owner: &str) -> Result<&'static [&'static str], String> {
        if !self.admin_computers.contains(id) {
            return Ok(&[COMPUTER_TAG]);
        }
        if self.admin.as_deref() != Some(owner) {
            return Err(format!(
                "computer {id} is listed for the admin ports and is not the fleet's admin's"
            ));
        }
        Ok(&[COMPUTER_TAG, ADMIN_COMPUTER_TAG])
    }

    /// whose remote computer a node is, by the key it joined with
    fn bound_to(&self, n: &serde_json::Value) -> Option<Bound> {
        if !Self::is_computer(&Self::tags(n)) {
            return None;
        }
        let kid = id_of(&n["preAuthKey"]).ok()?.to_string();
        self.bound.lock().unwrap().get(&kid).cloned()
    }

    fn keep_bound(&self) {
        let Some(f) = &self.bound_file else { return };
        let b = serde_json::to_vec(&*self.bound.lock().unwrap()).unwrap_or_default();
        let tmp = f.with_extension("tmp");
        if std::fs::write(&tmp, b)
            .and_then(|_| std::fs::rename(&tmp, f))
            .is_err()
        {
            eprintln!("network: could not keep {}", f.display());
        }
    }

    fn node_id(n: &serde_json::Value) -> Option<String> {
        match &n["id"] {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(i) => Some(i.to_string()),
            _ => None,
        }
    }

    /// Every person's machines' addresses on the network, in one ask of the
    /// control server rather than one a person. A remote computer's node
    /// counts as its owner's, by the key it joined with.
    pub async fn addresses_by_owner(
        &self,
    ) -> Result<std::collections::HashMap<String, Vec<String>>> {
        let v = self
            .call(reqwest::Method::GET, "/api/v1/node", None)
            .await?;
        let mut out: std::collections::HashMap<String, Vec<String>> = Default::default();
        for n in v["nodes"].as_array().into_iter().flatten() {
            let tags = Self::tags(n);
            let user = match self.bound_to(n) {
                Some(b) => b.owner,
                // no other tagged node is anyone's
                None if !tags.is_empty() => continue,
                None => match n["user"]["name"].as_str() {
                    Some(u) => u.to_string(),
                    None => continue,
                },
            };
            out.entry(user).or_default().extend(
                n["ipAddresses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str().map(str::to_string)),
            );
        }
        Ok(out)
    }

    /// Take every device off the network whose owner is no longer a member.
    ///
    /// A node outlives everything that let it join: revoke the member, and
    /// their laptop stays on the network with whatever the policy lets it
    /// reach, for good. Boxes are left alone - they are tagged, and they
    /// joined on the boxes' key, not a person's.
    ///
    /// `member` answers for a person (App::network_answer): a node comes off
    /// only on a No. Unknown - the agreed state not read yet, behind, or a
    /// name it does not have - leaves it on, and says so. `guard` holds the
    /// rest back: nobody it names comes off, and nothing at all while it
    /// says hold, or when more would come off at once than it allows.
    ///
    /// A remote computer's node goes too once its computer is no longer
    /// agreed up, or its owner no longer a member: `computer` says what the
    /// agreed state has of an id (owner, up), Err while it is unread, when
    /// none goes. One whose key the gate never made is no one's.
    pub async fn reap(
        &self,
        member: impl Fn(&str) -> crate::ident::Answer,
        control: Option<&str>,
        guard: &Guard<'_>,
    ) -> Result<usize> {
        use crate::ident::Answer;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let v = self
            .call(reqwest::Method::GET, "/api/v1/node", None)
            .await?;
        let mut used = std::collections::HashSet::new();
        let mut seen = std::collections::HashSet::new();
        let mut off: Vec<Off> = Vec::new();
        for n in v["nodes"].as_array().into_iter().flatten() {
            let tags = Self::tags(n);
            let label = n["givenName"]
                .as_str()
                .or(n["name"].as_str())
                .unwrap_or("?")
                .to_string();
            let whose;
            let user = if Self::is_computer(&tags) {
                let b = self.bound_to(n);
                if let Ok(k) = id_of(&n["preAuthKey"]) {
                    used.insert(k.to_string());
                }
                let keep = match (&b, control) {
                    // no agreed state here at all: no computer has a node
                    (None, _) | (_, None) => false,
                    (Some(b), Some(c)) => {
                        // the same box still holding its lease: a node given
                        // to the guest on one box is not another's. Its
                        // owner's membership comes off it only on a No
                        let judge = |snap: &Snapshot| {
                            qualifies(snap, &b.on).is_ok_and(|o| o == b.owner)
                                && member(&b.owner) != Answer::No
                                // the admin ports only while the release says
                                && (!tags.contains(&ADMIN_COMPUTER_TAG)
                                    || self
                                        .computer_tags(&b.id, &b.owner)
                                        .is_ok_and(|t| t.contains(&ADMIN_COMPUTER_TAG)))
                        };
                        match read_snapshot(&self.http, c, &b.id).await {
                            Read::Refused => false,
                            Read::Got(snap) if fresh(&snap, now_ms).is_ok() => {
                                let ok = judge(&snap);
                                self.last_good
                                    .lock()
                                    .unwrap()
                                    .insert(b.id.clone(), (snap, now_ms));
                                ok
                            }
                            // nothing fresh to go by: its last good word, for
                            // a while, then off
                            Read::Got(_) | Read::Unread => {
                                match self.last_good.lock().unwrap().get(&b.id) {
                                    Some((snap, at)) if now_ms - at <= LAST_GOOD_MS => judge(snap),
                                    _ => false,
                                }
                            }
                        }
                    }
                };
                if let Some(b) = &b {
                    seen.insert(b.id.clone());
                }
                if keep {
                    continue;
                }
                whose = b.as_ref().map(|b| b.owner.clone()).unwrap_or_default();
                b.map(|b| format!("{}'s computer {}", b.owner, b.id))
                    .unwrap_or_else(|| "no one's computer".into())
            } else if tags.contains(&INSTALLER_TAG) {
                let made = n["createdAt"].as_str().and_then(unix_of);
                let old = made.is_none_or(|m| (now_ms / 1000) as u64 > m + INSTALLER_MAX_SECS);
                if !old {
                    continue;
                }
                whose = String::new();
                "an installer's, done with".to_string()
            } else if !tags.is_empty() {
                continue;
            } else {
                let Some(user) = n["user"]["name"].as_str() else {
                    continue;
                };
                match member(user) {
                    Answer::Yes => continue,
                    Answer::Unknown => {
                        eprintln!(
                            "network: kept {label} - whether {user} is a member is not known"
                        );
                        continue;
                    }
                    Answer::No => {}
                }
                if guard.keep.contains(user) {
                    eprintln!(
                        "network: NOT removing {label}: {user} reads as no member, but the release names {user}; kept. Look into it"
                    );
                    continue;
                }
                whose = user.to_string();
                format!("{user} is not a member")
            };
            let id = match &n["id"] {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(i) => i.to_string(),
                _ => continue,
            };
            off.push(Off {
                made: n["createdAt"].as_str().unwrap_or_default().to_string(),
                num: id.parse().unwrap_or(u64::MAX),
                id,
                label,
                why: user,
                person: !Self::is_computer(&tags),
                whose,
            });
        }
        let mut gone = 0;
        // judged while the state moved: some devices by the old one, some
        // by the new, and the brake would weigh the old. The next pass
        // judges the new state whole, the brake first
        let moved = guard.moved();
        if moved && !off.is_empty() {
            eprintln!(
                "network: NOT reaping this pass: the state changed while it was judged; the next pass judges the new one. {} device(s) would have come off: {}",
                off.len(),
                Off::names(&off)
            );
            off.clear();
        }
        if !off.is_empty()
            && let Some(why) = &guard.hold
        {
            eprintln!(
                "network: NOT reaping: {why}; {} device(s) would come off: {}",
                off.len(),
                Off::names(&off)
            );
            off.clear();
        }
        // a person known to be no member loses every device; the brake
        // holds a state that looks wrong
        if guard.hold.is_none()
            && !moved
            && let Some(brake) = &guard.brake
        {
            let people = off
                .iter()
                .filter(|o| o.person)
                .map(|o| o.whose.clone())
                .collect();
            if let Err(why) = brake.check(&guard.members, &people) {
                eprintln!(
                    "network: ALERT: NOT reaping, the brake is on: {why}. The last good state is kept; nothing comes off until the state is consistent again or the owner acknowledges this state (POST /_dd/network/reap-ack {{\"state\": \"{}\"}}, signed in as the owner). {} device(s) would have come off: {}",
                    brake.refused().map(|(_, h)| h).unwrap_or_default(),
                    off.len(),
                    Off::names(&off)
                );
                off.clear();
            }
        }
        off.sort_by(|a, b| (&a.made, a.num).cmp(&(&b.made, b.num)));
        for (i, o) in off.iter().enumerate() {
            if guard.moved() {
                eprintln!(
                    "network: the state changed while devices came off; stopped, the next pass judges the rest: {}",
                    Off::names(&off[i..])
                );
                break;
            }
            self.call(
                reqwest::Method::DELETE,
                &format!("/api/v1/node/{}", o.id),
                None,
            )
            .await?;
            eprintln!("network: removed {} - {}", o.label, o.why);
            gone += 1;
        }
        self.last_good
            .lock()
            .unwrap()
            .retain(|id, _| seen.contains(id));
        // a key made and never used is forgotten after a day
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let before = self.bound.lock().unwrap().len();
        self.bound
            .lock()
            .unwrap()
            .retain(|k, b| used.contains(k) || now < b.made + 86400);
        if self.bound.lock().unwrap().len() != before {
            self.keep_bound();
        }
        Ok(gone)
    }

    /// one key, one device, ten minutes
    pub async fn join_key(&self, name: &str) -> Result<(String, u64)> {
        self.mint(name, &[]).await
    }

    async fn mint(&self, name: &str, tags: &[&str]) -> Result<(String, u64)> {
        self.mint_any(name, tags, false)
            .await
            .map(|(k, e, _)| (k, e))
    }

    /// a key whose node leaves the network when it disconnects
    async fn mint_ephemeral(&self, name: &str, tags: &[&str]) -> Result<(String, u64)> {
        self.mint_any(name, tags, true)
            .await
            .map(|(k, e, _)| (k, e))
    }

    /// a key, when it expires, and its id (what a node that joins with it
    /// names as its preAuthKey): a key with no id could bind no node
    async fn mint_id(&self, name: &str, tags: &[&str]) -> Result<(String, u64, u64)> {
        let (k, e, id) = self.mint_any(name, tags, false).await?;
        Ok((k, e, id.context("headscale gave no key id")?))
    }

    async fn mint_any(
        &self,
        name: &str,
        tags: &[&str],
        ephemeral: bool,
    ) -> Result<(String, u64, Option<u64>)> {
        let user = self.user_id(name).await?;
        let expires = SystemTime::now() + TTL;
        let stamp = humantime(expires);
        let v = self
            .call(
                reqwest::Method::POST,
                "/api/v1/preauthkey",
                Some(serde_json::json!({
                    "user": user,
                    "reusable": false,
                    "ephemeral": ephemeral,
                    "expiration": stamp,
                    "aclTags": tags
                })),
            )
            .await?;
        let key = v["preAuthKey"]["key"]
            .as_str()
            .context("headscale gave no key")?
            .to_string();
        let kid = id_of(&v["preAuthKey"]).ok();
        Ok((
            key,
            expires
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            kid,
        ))
    }
}

/// A box's own key to the network, sealed to the box.
///
/// The box signs the time with the key in its TPM; the answer is a
/// single-use key, tagged a box, encrypted to the secrets key in that same
/// TPM. Whoever carries the answer (the front door, Cloudflare) holds
/// ciphertext only, and a copied request gets nothing a second time.
///
/// A box that joined on its own key (state/join.go) is looked up in the
/// agreed state instead (`joined`), and its key is tagged a joined box:
/// the network lets it reach the boxes' stores and nothing more.
pub(crate) async fn box_key(
    door: &Door,
    name: &str,
    time: u64,
    sig: &str,
    now: u64,
    joined: Option<&BoxKeys>,
) -> Result<String, (StatusCode, String)> {
    // a joined box under a user of its own, named for it: what says which
    // node it is, never the hostname the node gives itself
    let (keys, tag, user) = match door.boxes.get(name) {
        Some(k) => (k, "tag:box", "boxes".to_string()),
        None => (
            joined.ok_or((StatusCode::FORBIDDEN, "not a box of this fleet".into()))?,
            "tag:joined",
            format!("joined-{name}"),
        ),
    };
    if time.abs_diff(now) > SKEW {
        return Err((StatusCode::FORBIDDEN, "the time is off".into()));
    }
    let sig = signature(sig).ok_or((StatusCode::BAD_REQUEST, "no signature".into()))?;
    keys.signs
        .verify(&join_message(name, time), &sig)
        .map_err(|_| (StatusCode::FORBIDDEN, "not signed by that box".into()))?;
    {
        // once a minute, and each signed time once: a request copied off
        // the wire is refused for good, not just for the minute
        let mut asked = door.asked.lock().unwrap();
        if let Some(&(at, signed)) = asked.get(name) {
            if now < at + EVERY {
                return Err((StatusCode::TOO_MANY_REQUESTS, "one a minute".into()));
            }
            if time <= signed {
                return Err((StatusCode::FORBIDDEN, "that request was used".into()));
            }
        }
        asked.insert(name.to_string(), (now, time));
    }
    let bad = |e: anyhow::Error| (StatusCode::BAD_GATEWAY, format!("{e:#}"));
    let (key, _) = door.mint(&user, &[tag]).await.map_err(bad)?;
    seal(&door.age, &keys.recipient, &key).map_err(bad)
}

/// armoured age, to one recipient
fn seal(age: &str, recipient: &str, plain: &str) -> Result<String> {
    use std::io::Write;
    let mut c = std::process::Command::new(age)
        .args(["-a", "-r", recipient])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("running age")?;
    c.stdin
        .take()
        .context("age's stdin")?
        .write_all(plain.as_bytes())?;
    let out = c.wait_with_output()?;
    if !out.status.success() {
        bail!("age: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// headscale gives ids as strings in json
fn id_of(u: &serde_json::Value) -> Result<u64> {
    match &u["id"] {
        serde_json::Value::String(s) => s.parse().context("user id"),
        serde_json::Value::Number(n) => n.as_u64().context("user id"),
        _ => bail!("headscale gave no user id"),
    }
}

/// RFC 3339, whole seconds, for a timestamp field
fn humantime(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn civil(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = secs / 86400;
    let rem = secs % 86400;
    // days since 1970-01-01 to civil date (Howard Hinnant's algorithm)
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y as u64,
        m as u64,
        d as u64,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
    )
}

/// POST /_dd/network/join, bearer device token -> { control_url, key, expires }
pub(crate) async fn join(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(door) = app.network.as_ref() else {
        return (StatusCode::NOT_FOUND, "this box has no network door").into_response();
    };
    let Some(token) = crate::bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "a device token is required").into_response();
    };
    let user = match app.verify_biscuit(token, "access") {
        Ok(u) => u,
        Err(e) => return (StatusCode::UNAUTHORIZED, e.to_string()).into_response(),
    };
    // a key on the fleet's network is not something signing up earns: the
    // network carries boxes, and every box trusts the interface it arrives
    // on. Membership is the release's word, and the demo never leaves the
    // browser it was made in.
    // A guest joins too: their devices come under the same rule as a
    // member's (the web port, which is public anyway, and the game ports,
    // which the games box opens per server to the players invited).
    // and nothing new while who is who is not known (Unknown is not yes)
    match app.network_answer(&user) {
        crate::ident::Answer::Yes
            if (app.member(&user) || app.guest(&user)) && user != crate::pages::DEMO_USER => {}
        crate::ident::Answer::Unknown => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "this box is still reading who is who; try again in a moment",
            )
                .into_response();
        }
        _ => return (StatusCode::FORBIDDEN, "not a member of this fleet").into_response(),
    }
    if let Some(r) = app.no_new_session() {
        return r;
    }
    match door.join_key(&user).await {
        Ok((key, expires)) => Json(serde_json::json!({
            "control_url": door.control_url,
            "key": key,
            "expires": expires,
            "user": user
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

/// POST /_dd/network/controller, bearer device token -> { control_url, key, expires }
///
/// A household's home controller's key to the network (docs/dev/home-
/// controller.md), asked for by the member pairing it, from the app, and
/// carried to the Pi over the house network. Single use, tagged a
/// controller and made in the boxes' user, so the reaper leaves it and the
/// policy lets it reach the boxes' front doors and nothing else; members'
/// devices reach it on 443 alone. A member's, never a guest's.
pub(crate) async fn controller_join(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(door) = app.network.as_ref() else {
        return (StatusCode::NOT_FOUND, "this box has no network door").into_response();
    };
    let Some(token) = crate::bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "a device token is required").into_response();
    };
    let user = match app.verify_biscuit(token, "access") {
        Ok(u) => u,
        Err(e) => return (StatusCode::UNAUTHORIZED, e.to_string()).into_response(),
    };
    if app.member_answer(&user) == crate::ident::Answer::Unknown {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "this box is still reading who is who; try again in a moment",
        )
            .into_response();
    }
    if !app.member(&user) || user == crate::pages::DEMO_USER {
        return (
            StatusCode::FORBIDDEN,
            "only a member sets up a home controller",
        )
            .into_response();
    }
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut asked = door.asked.lock().unwrap();
        let at = format!("controller:{user}");
        if asked.get(&at).is_some_and(|&(t, _)| now < t + EVERY) {
            return (StatusCode::TOO_MANY_REQUESTS, "one a minute").into_response();
        }
        asked.insert(at, (now, 0));
    }
    if let Some(r) = app.no_new_session() {
        return r;
    }
    match door.mint("boxes", &["tag:controller"]).await {
        Ok((key, expires)) => Json(serde_json::json!({
            "control_url": door.control_url,
            "key": key,
            "expires": expires
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct ComputerAsk {
    #[serde(rename = "box")]
    name: String,
    id: String,
    time: u64,
    sig: String,
}

/// A remote computer's key to the network (docs/dev/remote-computers.md),
/// asked for by the box that runs it as it starts it, and handed into the
/// guest by that box alone: never to a device. Signed by the box's TPM,
/// for one computer; given only while the computer is agreed up and kept to
/// its owner's own boxes, and only to the box that holds its lease. The
/// key is single use and lives ten minutes; its node is tagged a computer,
/// bound here to the computer and its owner, and the one node that
/// computer has: one before it comes off. The answer opens only with the
/// box's secrets key.
///
/// `snap`: the computer and its lease from one agreed state (dd-control's
/// /computer-lease), Err when it could not be read.
pub(crate) async fn computer_key(
    door: &Door,
    ask: &ComputerAsk,
    now: u64,
    snap: Result<Snapshot, ()>,
) -> Result<String, (StatusCode, String)> {
    let keys = door
        .boxes
        .get(&ask.name)
        .ok_or((StatusCode::FORBIDDEN, "not a box of this fleet".into()))?;
    if ask.id.len() != 16
        || !ask
            .id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err((StatusCode::BAD_REQUEST, "not a computer's id".into()));
    }
    if ask.time.abs_diff(now) > SKEW {
        return Err((StatusCode::FORBIDDEN, "the time is off".into()));
    }
    let sig = signature(&ask.sig).ok_or((StatusCode::BAD_REQUEST, "no signature".into()))?;
    keys.signs
        .verify(&computer_join_message(&ask.name, &ask.id, ask.time), &sig)
        .map_err(|_| (StatusCode::FORBIDDEN, "not signed by that box".into()))?;
    let snap = snap.map_err(|()| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "the agreed state could not be read; try again".to_string(),
        )
    })?;
    let owner = grant(&snap, &ask.name, now.saturating_mul(1000) as i64)?;
    let tags = door
        .computer_tags(&ask.id, &owner)
        .map_err(|why| (StatusCode::FORBIDDEN, why))?;
    {
        let mut asked = door.asked.lock().unwrap();
        let at = format!("computer:{}:{}", ask.name, ask.id);
        if let Some(&(t, signed)) = asked.get(&at) {
            if now < t + EVERY {
                return Err((StatusCode::TOO_MANY_REQUESTS, "one a minute".into()));
            }
            if ask.time <= signed {
                return Err((StatusCode::FORBIDDEN, "that request was used".into()));
            }
        }
        asked.insert(at, (now, ask.time));
    }
    let bad = |e: anyhow::Error| (StatusCode::BAD_GATEWAY, format!("{e:#}"));
    // one node a computer: the one it had comes off before its new key
    let v = door
        .call(reqwest::Method::GET, "/api/v1/node", None)
        .await
        .map_err(bad)?;
    for n in v["nodes"].as_array().into_iter().flatten() {
        if door.bound_to(n).is_some_and(|b| b.id == ask.id)
            && let Some(nid) = Door::node_id(n)
        {
            door.call(
                reqwest::Method::DELETE,
                &format!("/api/v1/node/{nid}"),
                None,
            )
            .await
            .map_err(bad)?;
        }
    }
    // and a key made for it before and never used goes too: one way onto
    // the network per computer, at any time
    let earlier: Vec<String> = door
        .bound
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, b)| b.id == ask.id)
        .map(|(k, _)| k.clone())
        .collect();
    for k in earlier {
        door.call(
            reqwest::Method::POST,
            "/api/v1/preauthkey/expire",
            Some(serde_json::json!({ "id": k })),
        )
        .await
        .map_err(bad)?;
        door.bound.lock().unwrap().remove(&k);
    }
    let (key, expires, kid) = door.mint_id("boxes", tags).await.map_err(bad)?;
    door.bound.lock().unwrap().insert(
        kid.to_string(),
        Bound {
            owner,
            id: ask.id.clone(),
            on: ask.name.clone(),
            made: now,
        },
    );
    door.keep_bound();
    let answer = serde_json::json!({
        "control_url": door.control_url,
        "key": key,
        "expires": expires,
        "hostname": format!("computer-{}", ask.id),
    });
    seal(&door.age, &keys.recipient, &answer.to_string()).map_err(bad)
}

/// A computer and its lease as one agreed state has them (dd-control's
/// /computer-lease)
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Snapshot {
    height: u64,
    #[serde(rename = "leaseHeight")]
    lease_height: u64,
    /// the agreed time, ms
    time: i64,
    computer: Option<SnapComputer>,
    lease: Option<SnapLease>,
    /// the release's owner of the box holding the lease (box.json), when
    /// it is a host of computers at all
    #[serde(rename = "hostOwner")]
    host_owner: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct SnapComputer {
    owner: String,
    up: bool,
    consent: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct SnapLease {
    holder: String,
    expires: i64,
}

/// how old the agreed state may be and still grant anything
const FRESH_MS: i64 = 30_000;

/// Whether `box_name` may hand a network key into this computer now: the
/// one rule for a key and for keeping its node. A computer kept to its
/// owner's own boxes, agreed up, whose lease that box holds, unexpired at
/// the agreed time, and that box its owner's by the release (box.json), as
/// placement has it - all from one agreed state, fresh. A computer on
/// anyone's box gets the internet only: that box's owner could read a key
/// out of it. The owner, or why not.
fn grant(snap: &Snapshot, box_name: &str, now_ms: i64) -> Result<String, (StatusCode, String)> {
    fresh(snap, now_ms)?;
    qualifies(snap, box_name)
}

/// one agreed state, and a fresh one
fn fresh(snap: &Snapshot, now_ms: i64) -> Result<(), (StatusCode, String)> {
    if snap.height == 0 || snap.height != snap.lease_height {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "the computer and its lease are not from one agreed state".into(),
        ));
    }
    if now_ms.saturating_sub(snap.time) > FRESH_MS {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "the agreed state is not fresh".into(),
        ));
    }
    Ok(())
}

/// what that state says, as of its own time
fn qualifies(snap: &Snapshot, box_name: &str) -> Result<String, (StatusCode, String)> {
    let no = |why: &str| Err((StatusCode::FORBIDDEN, why.to_string()));
    let Some(c) = &snap.computer else {
        return Err((StatusCode::NOT_FOUND, "no such remote computer".into()));
    };
    if !c.up {
        return no("that computer is not agreed up");
    }
    if c.consent != "own" {
        return no("only a computer kept to its owner's own boxes joins the network");
    }
    match &snap.lease {
        Some(l) if l.holder == box_name && l.expires > snap.time => {}
        Some(l) if l.holder == box_name => return no("its lease has lapsed"),
        _ => return no("that box does not hold the computer's lease"),
    }
    if snap.host_owner.as_deref() != Some(c.owner.as_str()) {
        return no("that box is not the computer's owner's");
    }
    Ok(c.owner.clone())
}

/// What dd-control answered for one computer: its state; a refusal (a
/// 4xx: a bad or unknown id), which is a decision; or nothing to go by
/// (it did not answer, a 5xx, not what it should be)
pub(crate) enum Read {
    Got(Snapshot),
    Refused,
    Unread,
}

async fn read_snapshot(http: &reqwest::Client, control: &str, id: &str) -> Read {
    let r = match http
        .get(format!("{}/computer-lease", control.trim_end_matches('/')))
        .query(&[("id", id)])
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Read::Unread,
    };
    if r.status().is_client_error() {
        return Read::Refused;
    }
    if !r.status().is_success() {
        return Read::Unread;
    }
    match r.json().await {
        Ok(s) => Read::Got(s),
        Err(_) => Read::Unread,
    }
}

/// how long a computer's node is kept on its last good word while the
/// agreed state cannot be read: past that, it comes off
const LAST_GOOD_MS: i64 = 10 * 60 * 1000;

/// POST /_dd/network/computer, {box, id, time, sig} -> the key, sealed to that box
pub(crate) async fn computer_join(
    State(app): State<Arc<App>>,
    Json(ask): Json<ComputerAsk>,
) -> Response {
    let Some(door) = app.network.as_ref() else {
        return (StatusCode::NOT_FOUND, "this box has no network door").into_response();
    };
    // nothing new while who is who is not known
    if let Some(r) = app.no_new_session() {
        return r;
    }
    let snap = match app.control.as_deref() {
        Some(control) => match read_snapshot(&door.http, control, &ask.id).await {
            Read::Got(s) => Ok(s),
            Read::Refused | Read::Unread => Err(()),
        },
        None => Err(()),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match computer_key(door, &ask, now, snap).await {
        Ok(sealed) => sealed.into_response(),
        Err((status, why)) => (status, why).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct BoxAsk {
    #[serde(rename = "box")]
    name: String,
    time: u64,
    sig: String,
}

/// POST /_dd/network/box, {box, time, sig} -> the key, sealed to that box
pub(crate) async fn box_join(State(app): State<Arc<App>>, Json(ask): Json<BoxAsk>) -> Response {
    let Some(door) = app.network.as_ref() else {
        return (StatusCode::NOT_FOUND, "this box has no network door").into_response();
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // a box the release does not list: as the agreed state has it, if it
    // joined (this box's dd-control)
    let mut joined = None;
    if !door.boxes.contains_key(&ask.name)
        && joined_name(&ask.name)
        && let Some(control) = &app.control
    {
        let st: Option<serde_json::Value> = async {
            crate::http()
                .get(format!("{}/state", control.trim_end_matches('/')))
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .ok()?
                .json()
                .await
                .ok()
        }
        .await;
        joined = st
            .as_ref()
            .and_then(|s| s["joined"].get(&ask.name))
            .and_then(|j| box_keys(&ask.name, j).ok());
    }
    match box_key(door, &ask.name, ask.time, &ask.sig, now, joined.as_ref()).await {
        Ok(sealed) => sealed.into_response(),
        Err((code, why)) => (code, why).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct InstallerAsk {
    ticket: String,
    recipient: String,
}

/// What an installer's ask is good for: the member and ticket id, when the
/// ticket is a member's join ticket narrowed by its holder to a network key
/// sealed to `recipient` (setup::narrow_ticket). Narrowed, it opens no
/// pending backup and gets no key sealed to anyone else, so whoever
/// carries the ask (the front door, Cloudflare) can do nothing with it.
pub(crate) fn installer_ticket(
    entry_of: impl Fn(&str) -> anyhow::Result<Option<identity::SignedEntry>>,
    token: &str,
    recipient: &str,
    now: u64,
) -> anyhow::Result<(String, String)> {
    anyhow::ensure!(
        recipient.len() <= 100
            && recipient.starts_with("age1")
            && recipient[4..]
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "not an age recipient"
    );
    let blocks = biscuit_auth::UnverifiedBiscuit::from_base64(token)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .block_count();
    // the whole ticket never comes this way: it is the pending backup's
    anyhow::ensure!(blocks >= 2, "a ticket not narrowed to this ask");
    let (user, _) =
        crate::verify_device_token_at(entry_of, token, "join", &installer_here(recipient), now)?;
    let id = crate::member_boxes::ticket_of(token)
        .ok_or_else(|| anyhow::anyhow!("not one ticket fact"))?;
    Ok((user, id))
}

/// the `here` a narrowed ticket names: a key sealed to this recipient
pub(crate) fn installer_here(recipient: &str) -> String {
    format!("installer-network:{recipient}")
}

/// POST /_dd/network/installer, {ticket, recipient} -> a key for one box
/// installer, sealed to `recipient`: single-use, ten minutes, its node
/// ephemeral and tagged an installer, which the network lets reach the
/// boxes' front doors and nothing else. For a disk's backup before the
/// installer wipes it (nix/installer/backup.sh); a ticket a box was
/// enlisted with gets none.
pub(crate) async fn installer_join(
    State(app): State<Arc<App>>,
    Json(ask): Json<InstallerAsk>,
) -> Response {
    let Some(door) = app.network.as_ref() else {
        return (StatusCode::NOT_FOUND, "this box has no network door").into_response();
    };
    // nothing new while who is who is not known
    if let Some(r) = app.no_new_session() {
        return r;
    }
    let now = identity::now();
    let (user, id) = match installer_ticket(|u| app.entry(u), &ask.ticket, &ask.recipient, now) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("network: an installer's ask refused: {e:#}");
            return (StatusCode::FORBIDDEN, "not a join ticket for this").into_response();
        }
    };
    if app.directory.ident.is_some() {
        match crate::member_boxes::enlisted(&app) {
            Some(e) if e.values().any(|o| o.ticket == id) => {
                return (StatusCode::FORBIDDEN, "a box was set up with that ticket")
                    .into_response();
            }
            Some(_) => {}
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "who is who is not known yet",
                )
                    .into_response();
            }
        }
    }
    {
        let mut asked = door.asked.lock().unwrap();
        let k = format!("installer {id}");
        if let Some(&(at, _)) = asked.get(&k)
            && now < at + EVERY
        {
            return (StatusCode::TOO_MANY_REQUESTS, "one a minute").into_response();
        }
        asked.insert(k, (now, now));
    }
    let bad = |e: anyhow::Error| (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response();
    let key = match door.mint_ephemeral("boxes", &[INSTALLER_TAG]).await {
        Ok((k, _)) => k,
        Err(e) => return bad(e),
    };
    eprintln!("network: an installer's key for {user}'s ticket {id}");
    match seal(&door.age, &ask.recipient, &key) {
        Ok(sealed) => sealed.into_response(),
        Err(e) => bad(e),
    }
}

/// seconds since 1970 of an RFC 3339 time as headscale gives it
/// (2026-11-03T04:05:06Z, or with a fraction or an offset of +00:00)
fn unix_of(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if rest != "Z" && rest != "+00:00" {
        return None;
    }
    // days from civil (Howard Hinnant's algorithm)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn headscale_times_read_as_seconds() {
        assert_eq!(unix_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_of("2026-11-03T04:05:06Z"), Some(1793678706));
        assert_eq!(unix_of("2026-11-03T04:05:06.123456789Z"), Some(1793678706));
        assert_eq!(unix_of("2026-11-03T04:05:06+00:00"), Some(1793678706));
        assert_eq!(unix_of("2000-02-29T12:00:00Z"), Some(951825600));
        assert_eq!(unix_of("2026-11-03T04:05:06+05:00"), None);
        assert_eq!(unix_of(""), None);
    }

    /// A fake headscale with three nodes: a box, a member's laptop and a
    /// stranger's phone. Only the phone may go.
    #[tokio::test]
    async fn only_a_non_members_devices_come_off() {
        use axum::{
            Json, Router,
            extract::{Path, State},
            routing::{delete, get},
        };
        use std::sync::{Arc, Mutex};

        let deleted: Arc<Mutex<Vec<String>>> = Arc::default();
        let nodes = serde_json::json!({ "nodes": [
            { "id": "1", "givenName": "node1", "user": { "name": "tagged-devices" }, "tags": ["tag:box"] },
            { "id": "2", "givenName": "laptop", "user": { "name": "david" }, "tags": [] },
            { "id": "3", "givenName": "phone", "user": { "name": "stranger" } },
        ]});
        let app = Router::new()
            .route(
                "/api/v1/node",
                get(move || async move { Json(nodes.clone()) }),
            )
            .route(
                "/api/v1/node/{id}",
                delete(
                    |State(d): State<Arc<Mutex<Vec<String>>>>, Path(id): Path<String>| async move {
                        d.lock().unwrap().push(id);
                        Json(serde_json::json!({}))
                    },
                ),
            )
            .with_state(deleted.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let door = Door {
            api: format!("http://{addr}"),
            control_url: String::new(),
            key: "k".into(),
            key_file: None,
            http: reqwest::Client::new(),
            boxes: HashMap::new(),
            age: "age".into(),
            asked: Mutex::default(),
            bound: Mutex::default(),
            bound_file: None,
            last_good: Mutex::default(),
            admin_computers: Default::default(),
            admin: None,
        };
        let gone = door
            .reap(
                |u| crate::ident::Answer::of(u == "david"),
                None,
                &Guard::open(),
            )
            .await
            .unwrap();
        assert_eq!(gone, 1);
        assert_eq!(*deleted.lock().unwrap(), vec!["3".to_string()]);
    }

    /// A fake headscale holding `nodes`, and what was deleted from it (gone
    /// from the list as well)
    async fn headscale_with(
        nodes: serde_json::Value,
    ) -> (Door, Arc<std::sync::Mutex<Vec<String>>>) {
        use axum::{
            Json, Router,
            extract::{Path, State},
            routing::{delete, get},
        };
        type Held = (
            Arc<std::sync::Mutex<serde_json::Value>>,
            Arc<std::sync::Mutex<Vec<String>>>,
        );
        let deleted: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let held: Held = (Arc::new(std::sync::Mutex::new(nodes)), deleted.clone());
        let app = Router::new()
            .route(
                "/api/v1/node",
                get(|State((n, _)): State<Held>| async move { Json(n.lock().unwrap().clone()) }),
            )
            .route(
                "/api/v1/node/{id}",
                delete(
                    |State((n, d)): State<Held>, Path(id): Path<String>| async move {
                        if let Some(a) = n.lock().unwrap()["nodes"].as_array_mut() {
                            a.retain(|x| x["id"] != id.as_str());
                        }
                        d.lock().unwrap().push(id);
                        Json(serde_json::json!({}))
                    },
                ),
            )
            .with_state(held);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let door = Door {
            api: format!("http://{addr}"),
            control_url: String::new(),
            key: "k".into(),
            key_file: None,
            http: reqwest::Client::new(),
            boxes: HashMap::new(),
            age: "age".into(),
            asked: Mutex::default(),
            bound: Mutex::default(),
            bound_file: None,
            last_good: Mutex::default(),
            admin_computers: Default::default(),
            admin: None,
        };
        (door, deleted)
    }

    fn person_node(id: &str, name: &str, user: &str, made: &str) -> serde_json::Value {
        serde_json::json!({ "id": id, "givenName": name, "user": { "name": user }, "tags": [], "createdAt": made })
    }

    /// The incident of 2026-10-22: the gate started before dd-control
    /// answered, read everyone as no member, and took the fleet owner's
    /// laptop off. Unknown keeps a device on; a hold keeps every device on;
    /// a person the release names is never taken off
    #[tokio::test]
    async fn nothing_comes_off_on_what_is_not_known() {
        use crate::ident::Answer;
        let (door, deleted) = headscale_with(serde_json::json!({ "nodes": [
            { "id": "1", "givenName": "node1", "user": { "name": "tagged-devices" }, "tags": ["tag:box"] },
            person_node("2", "laptop", "david", "2026-01-01T00:00:00Z"),
            person_node("3", "phone", "kim", "2026-02-01T00:00:00Z"),
            person_node("4", "tablet", "kim", "2026-03-01T00:00:00Z"),
        ]}))
        .await;
        let answers = |u: &str| match u {
            "david" => Answer::Yes,
            _ => Answer::No,
        };
        let gone = || deleted.lock().unwrap().clone();

        // not known yet: nobody comes off, the owner least of all
        assert_eq!(
            door.reap(|_| Answer::Unknown, None, &Guard::open())
                .await
                .unwrap(),
            0
        );
        assert!(gone().is_empty());

        // held (not settled, or in the grace): nothing, though kim is out
        let held = Guard {
            hold: Some("not known yet".into()),
            ..Guard::open()
        };
        assert_eq!(door.reap(answers, None, &held).await.unwrap(), 0);
        assert!(gone().is_empty());

        // a person the release names, read as no member: kept, whatever
        let named = Guard {
            keep: ["david".to_string()].into(),
            ..Guard::open()
        };
        assert_eq!(door.reap(|_| Answer::No, None, &named).await.unwrap(), 2);
        assert_eq!(gone(), vec!["3".to_string(), "4".to_string()]);
    }

    fn brake_guard(brake: &Arc<Brake>, members: &[&str]) -> Guard<'static> {
        Guard {
            members: members.iter().map(|m| m.to_string()).collect(),
            brake: Some(brake.clone()),
            ..Guard::open()
        }
    }

    fn facts(what: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("dd-brake-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn set(xs: &[&str]) -> std::collections::BTreeSet<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    /// The owner's acknowledgement counts only while the brake is on, only
    /// for the exact state it refused, only for a while, and never for an
    /// empty state
    #[test]
    fn an_ack_is_for_that_refused_state_alone_and_not_for_long() {
        let now = crate::session::now();
        let b = Brake::new(None, None, 2);
        let all = set(&["ann", "bob", "cy", "dee"]);
        b.check(&all, &set(&[])).unwrap();
        // off: nothing to acknowledge
        assert_eq!(b.acknowledge("x", now), Err(AckRefused::Off));

        // a drop to one member: refused, and an ack must name it
        let one = set(&["ann"]);
        let out = set(&["bob", "cy", "dee"]);
        assert!(b.check(&one, &out).is_err());
        let (_, h) = b.refused().unwrap();
        assert_eq!(h, refused_hash(&one, &out));
        assert_eq!(b.acknowledge("guess", now), Err(AckRefused::NotThatState));

        // acknowledged, but the next state is another one: void, still on
        b.acknowledge(&h, now).unwrap();
        let other = set(&["ann", "zed"]);
        assert!(b.check(&other, &out).is_err());
        // and spent: the first state again is refused too
        assert!(b.check(&one, &out).is_err());

        // acknowledged, and run out before its pass: void
        let (_, h) = b.refused().unwrap();
        b.acknowledge(&h, now - ACK_FOR - 1).unwrap();
        assert!(b.check(&one, &out).is_err());

        // acknowledged, in time, the same state: it goes through, once
        b.acknowledge(&h, now).unwrap();
        assert!(b.check(&one, &out).is_ok());
        assert!(b.on().is_none());

        // empty: never, acknowledged or not
        let none = set(&[]);
        assert!(b.check(&none, &set(&["ann"])).is_err());
        let (_, h) = b.refused().unwrap();
        assert_eq!(b.acknowledge(&h, now), Err(AckRefused::Empty));
        b.state.lock().unwrap().ack = Some((h, now + ACK_FOR));
        assert!(b.check(&none, &set(&["ann"])).is_err(), "even a forced ack");
    }

    /// A box's host is not on the keep list (reap_keep): kept while a
    /// member, their devices off once their membership is revoked, and a
    /// state that drops the host with enough others is held by the brake
    #[tokio::test]
    async fn a_revoked_host_loses_their_devices_and_the_brake_still_holds() {
        use crate::ident::Answer;
        let (door, deleted) = headscale_with(serde_json::json!({ "nodes": [
            person_node("1", "laptop", "david", "2026-01-01T00:00:00Z"),
            person_node("2", "phone", "kim", "2026-01-02T00:00:00Z"),
            person_node("3", "other", "kim", "2026-01-03T00:00:00Z"),
            person_node("4", "tablet", "ann", "2026-01-04T00:00:00Z"),
            person_node("5", "pc", "bob", "2026-01-05T00:00:00Z"),
        ]}))
        .await;
        let d = facts("host");
        let brake = Arc::new(Brake::new(
            Some(d.join("acted.json")),
            Some(d.join("f.prom")),
            2,
        ));
        // kim hosts this box (VERIFY_OWNER) and a box elsewhere; david is
        // the fleet's admin, sarah a person the release names
        let keep = reap_keep(set(&["sarah"]), Some("david"));
        assert_eq!(keep, set(&["david", "sarah"]));
        let guard = |members: &[&str]| Guard {
            keep: keep.clone(),
            ..brake_guard(&brake, members)
        };
        let by = |m: &'static [&'static str]| move |u: &str| Answer::of(m.contains(&u));

        // the host is a member: every device stays
        const ALL: &[&str] = &["david", "kim", "ann", "bob", "cy", "dee", "eve", "fay"];
        assert_eq!(door.reap(by(ALL), None, &guard(ALL)).await.unwrap(), 0);
        assert!(deleted.lock().unwrap().is_empty());

        // a state that drops the host and two more at once (3 of 8): the
        // brake holds it, and the host keeps every device
        const SHRUNK: &[&str] = &["david", "cy", "dee", "eve", "fay"];
        for _ in 0..2 {
            assert_eq!(
                door.reap(by(SHRUNK), None, &guard(SHRUNK)).await.unwrap(),
                0
            );
            assert!(deleted.lock().unwrap().is_empty());
            assert!(brake.on().is_some());
        }

        // the host's membership is revoked, alone: both of kim's devices
        // come off, nobody else's
        const NO_KIM: &[&str] = &["david", "ann", "bob", "cy", "dee", "eve", "fay"];
        assert_eq!(
            door.reap(by(NO_KIM), None, &guard(NO_KIM)).await.unwrap(),
            2
        );
        assert_eq!(
            *deleted.lock().unwrap(),
            vec!["2".to_string(), "3".to_string()]
        );
        assert!(brake.on().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// One person known to be no member loses every device, however many
    #[tokio::test]
    async fn a_revoked_person_loses_all_six_devices() {
        use crate::ident::Answer;
        let mut nodes = vec![person_node("1", "laptop", "david", "2026-01-01T00:00:00Z")];
        for i in 0..6 {
            nodes.push(person_node(
                &format!("{}", 10 + i),
                &format!("kim{i}"),
                "kim",
                &format!("2026-02-0{}T00:00:00Z", i + 1),
            ));
        }
        let (door, deleted) = headscale_with(serde_json::json!({ "nodes": nodes })).await;
        let d = facts("six");
        let brake = Arc::new(Brake::new(
            Some(d.join("acted.json")),
            Some(d.join("f.prom")),
            2,
        ));
        let members = ["david", "ann", "bob", "cy"];
        let g = brake_guard(&brake, &members);
        let answers = |u: &str| Answer::of(u != "kim");
        assert_eq!(door.reap(answers, None, &g).await.unwrap(), 6);
        assert_eq!(deleted.lock().unwrap().len(), 6);
        assert!(brake.on().is_none());
        let f = std::fs::read_to_string(d.join("f.prom")).unwrap();
        assert!(f.contains("dd_reap_brake 0"), "{f}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An agreed state that suddenly drops most of its members is more
    /// likely wrong than true: nothing comes off, pass after pass, and the
    /// alert stays up; restored, the brake is off and only who is really
    /// out goes. The owner's word lets a state through as it is
    #[tokio::test]
    async fn a_state_that_drops_most_members_removes_nothing_until_restored() {
        use crate::ident::Answer;
        let (door, deleted) = headscale_with(serde_json::json!({ "nodes": [
            person_node("1", "laptop", "david", "2026-01-01T00:00:00Z"),
            person_node("2", "phone", "ann", "2026-01-02T00:00:00Z"),
            person_node("3", "tablet", "bob", "2026-01-03T00:00:00Z"),
            person_node("4", "pc", "cy", "2026-01-04T00:00:00Z"),
            person_node("5", "watch", "kim", "2026-01-05T00:00:00Z"),
        ]}))
        .await;
        let d = facts("drop");
        let acted = d.join("acted.json");
        let brake = Arc::new(Brake::new(Some(acted.clone()), Some(d.join("f.prom")), 2));
        let keep = |g: Guard<'static>| Guard {
            keep: ["david".to_string()].into(),
            ..g
        };
        let all = ["david", "ann", "bob", "cy", "dee"];
        // a good state, nobody out: it is the one acted on
        let good = |u: &str| Answer::of(all.contains(&u));
        let g = keep(brake_guard(&brake, &all));
        assert_eq!(
            door.reap(|u| Answer::of(u != "kim").or(|| good(u)), None, &g)
                .await
                .unwrap(),
            1
        );
        assert_eq!(*deleted.lock().unwrap(), vec!["5".to_string()]);
        deleted.lock().unwrap().clear();

        // the state drops everyone but the owner: nothing, every pass
        let bad = |u: &str| Answer::of(u == "david");
        for _ in 0..3 {
            let g = keep(brake_guard(&brake, &["david"]));
            assert_eq!(door.reap(bad, None, &g).await.unwrap(), 0);
            assert!(deleted.lock().unwrap().is_empty());
            assert!(
                brake.on().unwrap().contains("4 of the 5"),
                "{:?}",
                brake.on()
            );
            let f = std::fs::read_to_string(d.join("f.prom")).unwrap();
            assert!(f.contains("dd_reap_brake 1"), "{f}");
        }
        // an empty state, the same
        let g = keep(brake_guard(&brake, &[]));
        assert_eq!(door.reap(|_| Answer::No, None, &g).await.unwrap(), 0);
        assert!(brake.on().is_some());
        // a gate started again keeps the last good state, and the brake
        let again = Arc::new(Brake::new(Some(acted.clone()), Some(d.join("f.prom")), 2));
        let g = keep(brake_guard(&again, &["david"]));
        assert_eq!(door.reap(bad, None, &g).await.unwrap(), 0);
        assert!(again.on().is_some());

        // restored: the brake is off, and nobody is out
        let g = keep(brake_guard(&brake, &all));
        assert_eq!(door.reap(good, None, &g).await.unwrap(), 0);
        assert!(brake.on().is_none());
        let f = std::fs::read_to_string(d.join("f.prom")).unwrap();
        assert!(f.contains("dd_reap_brake 0"), "{f}");

        // three people out at once, from a state that keeps most: held too
        let few = ["david", "dee"];
        let shrunk_a_little = ["david", "ann", "bob", "cy"];
        let g = keep(brake_guard(&brake, &shrunk_a_little));
        assert_eq!(
            door.reap(|u| Answer::of(few.contains(&u)), None, &g)
                .await
                .unwrap(),
            0
        );
        assert!(brake.on().unwrap().contains("3 people"), "{:?}", brake.on());
        // the owner says that state is right: the next pass goes by it
        let (_, h) = brake.refused().unwrap();
        brake.acknowledge(&h, crate::session::now()).unwrap();
        let g = keep(brake_guard(&brake, &shrunk_a_little));
        assert_eq!(
            door.reap(|u| Answer::of(few.contains(&u)), None, &g)
                .await
                .unwrap(),
            3
        );
        assert!(brake.on().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// CI run 882: the agreed state dropped ivy (half of who has access)
    /// while a pass was waiting on headscale. The pass had read who has
    /// access before (ivy in), judged ivy's tablet by the state after (ivy
    /// out), and the brake, weighing the first, let the tablet go. A pass
    /// the state moves under takes nothing off; the next one is braked
    #[tokio::test]
    async fn a_pass_the_state_moves_under_takes_nothing_off() {
        use crate::ident::Answer;
        use std::sync::atomic::{AtomicBool, Ordering};
        let (door, deleted) = headscale_with(serde_json::json!({ "nodes": [
            person_node("1", "laptop", "sarah", "2026-01-01T00:00:00Z"),
            person_node("2", "tablet", "ivy", "2026-01-02T00:00:00Z"),
        ]}))
        .await;
        let brake = Arc::new(Brake::new(None, None, 2));
        let state = std::sync::Mutex::new((5u64, set(&["sarah", "ivy"])));
        let now = || state.lock().unwrap().clone();
        let guard = || {
            let (height, members) = now();
            Guard {
                members,
                height,
                brake: Some(brake.clone()),
                now: Some(&now),
                ..Guard::open()
            }
        };
        let answers = |u: &str| Answer::of(state.lock().unwrap().1.contains(u));
        // a good state, acted on
        assert_eq!(door.reap(answers, None, &guard()).await.unwrap(), 0);

        // the state drops ivy after the pass read it, before it judged her
        let g = guard();
        let dropped = AtomicBool::new(false);
        let landing = |u: &str| {
            if !dropped.swap(true, Ordering::SeqCst) {
                *state.lock().unwrap() = (6, set(&["sarah"]));
            }
            answers(u)
        };
        assert_eq!(door.reap(landing, None, &g).await.unwrap(), 0);
        assert!(deleted.lock().unwrap().is_empty(), "{:?}", deleted);
        assert!(brake.on().is_none(), "nothing weighed: {:?}", brake.on());

        // the next pass weighs the new state whole: the brake
        for _ in 0..3 {
            assert_eq!(door.reap(answers, None, &guard()).await.unwrap(), 0);
            assert!(deleted.lock().unwrap().is_empty());
            assert!(
                brake.on().unwrap().contains("1 of the 2"),
                "{:?}",
                brake.on()
            );
        }
    }

    /// The state moving while devices come off stops the pass there: the
    /// rest are judged again, by the new state, next pass
    #[tokio::test]
    async fn the_state_moving_mid_removal_stops_the_pass() {
        use crate::ident::Answer;
        let (door, deleted) = headscale_with(serde_json::json!({ "nodes": [
            person_node("1", "laptop", "sarah", "2026-01-01T00:00:00Z"),
            person_node("2", "phone", "kim", "2026-01-02T00:00:00Z"),
            person_node("3", "tablet", "kim", "2026-01-03T00:00:00Z"),
        ]}))
        .await;
        let brake = Arc::new(Brake::new(None, None, 2));
        let members = set(&["sarah", "ann", "bob", "cy"]);
        // a new state lands once the first device is off
        let now = || {
            let h = if deleted.lock().unwrap().is_empty() {
                5
            } else {
                6
            };
            (h, members.clone())
        };
        let g = Guard {
            members: members.clone(),
            height: 5,
            brake: Some(brake.clone()),
            now: Some(&now),
            ..Guard::open()
        };
        let answers = |u: &str| Answer::of(u != "kim");
        assert_eq!(door.reap(answers, None, &g).await.unwrap(), 1);
        assert_eq!(*deleted.lock().unwrap(), vec!["2".to_string()]);
    }

    /// A box's key: only for a box of the fleet, signed by that box, now,
    /// and once a minute; what comes back went through age to the box.
    #[tokio::test]
    async fn a_box_gets_a_key_sealed_to_it() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        use base64::Engine;
        use p256::ecdsa::{SigningKey, signature::Signer};

        let app = Router::new()
            .route(
                "/api/v1/user",
                get(|| async {
                    Json(serde_json::json!({ "users": [{ "id": "7", "name": "boxes" }] }))
                }),
            )
            .route(
                "/api/v1/preauthkey",
                post(|Json(b): Json<serde_json::Value>| async move {
                    assert_eq!(b["aclTags"], serde_json::json!(["tag:box"]));
                    assert_eq!(b["reusable"], false);
                    Json(serde_json::json!({ "preAuthKey": { "key": "the-key" } }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // age that only marks what it was given, and to whom
        let dir = std::env::temp_dir().join(format!("box-join-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let age = dir.join("age");
        std::fs::write(&age, "#!/bin/sh\nprintf 'sealed to %s: ' \"$3\"; cat\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&age, std::fs::Permissions::from_mode(0o755)).unwrap();

        let node2 = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let stranger = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let mut boxes = HashMap::new();
        boxes.insert(
            "node2".to_string(),
            BoxKeys {
                signs: *node2.verifying_key(),
                recipient: "age1node2".into(),
            },
        );
        let door = Door {
            api: format!("http://{addr}"),
            control_url: String::new(),
            key: "k".into(),
            key_file: None,
            http: reqwest::Client::new(),
            boxes,
            age: age.display().to_string(),
            asked: Mutex::default(),
            bound: Mutex::default(),
            bound_file: None,
            last_good: Mutex::default(),
            admin_computers: Default::default(),
            admin: None,
        };
        let sign = |k: &SigningKey, name: &str, t: u64| {
            let s: Signature = k.sign(&join_message(name, t));
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.to_bytes())
        };
        let now = 1_790_000_000;
        let refused = |r: Result<String, (StatusCode, String)>| r.unwrap_err().0;

        assert_eq!(
            refused(box_key(&door, "node9", now, &sign(&node2, "node9", now), now, None).await),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            refused(
                box_key(
                    &door,
                    "node2",
                    now,
                    &sign(&stranger, "node2", now),
                    now,
                    None
                )
                .await
            ),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            refused(
                box_key(
                    &door,
                    "node2",
                    now - 600,
                    &sign(&node2, "node2", now - 600),
                    now,
                    None
                )
                .await
            ),
            StatusCode::FORBIDDEN
        );
        // signed for another time than it claims
        assert_eq!(
            refused(
                box_key(
                    &door,
                    "node2",
                    now,
                    &sign(&node2, "node2", now - 1),
                    now,
                    None
                )
                .await
            ),
            StatusCode::FORBIDDEN
        );

        let sealed = box_key(&door, "node2", now, &sign(&node2, "node2", now), now, None)
            .await
            .unwrap();
        assert_eq!(sealed, "sealed to age1node2: the-key");
        assert_eq!(
            refused(
                box_key(
                    &door,
                    "node2",
                    now + 5,
                    &sign(&node2, "node2", now + 5),
                    now + 5,
                    None
                )
                .await
            ),
            StatusCode::TOO_MANY_REQUESTS
        );
        // the same request a minute later still gets nothing
        assert_eq!(
            refused(
                box_key(
                    &door,
                    "node2",
                    now,
                    &sign(&node2, "node2", now),
                    now + 61,
                    None
                )
                .await
            ),
            StatusCode::FORBIDDEN
        );
        // the DER a soft key writes is taken too
        let der: Signature = node2.sign(&join_message("node2", now + 61));
        let der = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(der.to_der().as_bytes());
        assert!(
            box_key(&door, "node2", now + 61, &der, now + 61, None)
                .await
                .is_ok()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A fake headscale: the nodes it has, the keys it made (their tags),
    /// and the nodes taken off
    #[derive(Default)]
    struct Fake {
        /// dd-control's /computer-lease, by id; an id it does not know, 404
        snaps: Mutex<HashMap<String, serde_json::Value>>,
        /// whether dd-control answers at all (a 503 when not)
        down: Mutex<bool>,
        /// the keys expired, by id
        expired: Mutex<Vec<String>>,
        nodes: Mutex<Vec<serde_json::Value>>,
        made: Mutex<Vec<serde_json::Value>>,
        deleted: Mutex<Vec<String>>,
    }

    async fn fake_headscale(f: Arc<Fake>) -> String {
        use axum::{
            Json, Router,
            extract::{Path, State},
            routing::{delete, get, post},
        };
        let app = Router::new()
            .route(
                "/api/v1/user",
                get(|| async {
                    Json(serde_json::json!({ "users": [{ "id": "7", "name": "boxes" }] }))
                }),
            )
            .route(
                "/api/v1/preauthkey",
                post(
                    |State(f): State<Arc<Fake>>, Json(b): Json<serde_json::Value>| async move {
                        let mut made = f.made.lock().unwrap();
                        made.push(b);
                        let id = made.len() + 40;
                        Json(serde_json::json!({ "preAuthKey": { "key": format!("key-{id}"), "id": id.to_string() } }))
                    },
                ),
            )
            .route(
                "/api/v1/preauthkey/expire",
                post(
                    |State(f): State<Arc<Fake>>, Json(b): Json<serde_json::Value>| async move {
                        f.expired.lock().unwrap().push(b["id"].as_str().unwrap_or("").to_string());
                        Json(serde_json::json!({}))
                    },
                ),
            )
            .route(
                "/computer-lease",
                get(
                    |State(f): State<Arc<Fake>>,
                     axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| async move {
                        if *f.down.lock().unwrap() {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        match f.snaps.lock().unwrap().get(&q["id"]) {
                            Some(v) => Json(v.clone()).into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        }
                    },
                ),
            )
            .route(
                "/api/v1/node",
                get(|State(f): State<Arc<Fake>>| async move {
                    Json(serde_json::json!({ "nodes": f.nodes.lock().unwrap().clone() }))
                }),
            )
            .route(
                "/api/v1/node/{id}",
                delete(
                    |State(f): State<Arc<Fake>>, Path(id): Path<String>| async move {
                        f.deleted.lock().unwrap().push(id);
                        Json(serde_json::json!({}))
                    },
                ),
            )
            .with_state(f);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn door_at(api: String, boxes: HashMap<String, BoxKeys>, age: String) -> Door {
        Door {
            api,
            control_url: "https://headscale.test.invalid".into(),
            key: "k".into(),
            key_file: None,
            http: reqwest::Client::new(),
            boxes,
            age,
            asked: Mutex::default(),
            bound: Mutex::default(),
            bound_file: None,
            last_good: Mutex::default(),
            admin_computers: Default::default(),
            admin: None,
        }
    }

    /// A box headscale has not run on has no api key file: its gate still
    /// starts, a call says why it cannot reach headscale, and once headscale
    /// comes here and writes its key the next call carries it
    #[tokio::test]
    async fn a_gate_without_the_headscale_key_starts_and_reads_it_when_it_comes() {
        use axum::{Json, Router, http::HeaderMap, routing::get};
        let app = Router::new().route(
            "/api/v1/node",
            get(|h: HeaderMap| async move {
                let auth = h
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                Json(serde_json::json!({ "auth": auth }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let file = std::env::temp_dir().join(format!("headscale-api-key-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let mut door = door_at(format!("http://{addr}"), HashMap::new(), "age".into());
        door.key_file = Some(file.clone());
        let err = door
            .call(reqwest::Method::GET, "/api/v1/node", None)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not on this box yet"),
            "{err:#}"
        );
        std::fs::write(&file, "fresh\n").unwrap();
        let got = door
            .call(reqwest::Method::GET, "/api/v1/node", None)
            .await
            .unwrap();
        assert_eq!(got["auth"], "Bearer fresh");
        let _ = std::fs::remove_file(&file);
    }

    fn computer_node(id: &str, kid: &str, ip: &str) -> serde_json::Value {
        serde_json::json!({ "id": id, "givenName": format!("n{id}"), "user": { "name": "boxes" },
            "tags": ["tag:computer"], "preAuthKey": { "id": kid }, "ipAddresses": [ip] })
    }

    /// A computer on the release's list gets the admin ports' tag beside its
    /// own, if it is the fleet's admin's; another's by that id gets no key,
    /// and one not listed its own tag alone. Off the list, its node goes.
    #[tokio::test]
    async fn the_admin_ports_tag_goes_to_the_admins_listed_computer_alone() {
        let mut door = door_at("http://unused".into(), HashMap::new(), "age".into());
        door.admin_computers = ["aa".to_string()].into();
        door.admin = Some("david".into());
        assert_eq!(
            door.computer_tags("aa", "david").unwrap(),
            [COMPUTER_TAG, ADMIN_COMPUTER_TAG]
        );
        assert!(door.computer_tags("aa", "ann").is_err());
        assert_eq!(door.computer_tags("bb", "david").unwrap(), [COMPUTER_TAG]);
        assert_eq!(door.computer_tags("bb", "ann").unwrap(), [COMPUTER_TAG]);
        door.admin = None;
        assert!(door.computer_tags("aa", "david").is_err());

        let f = Arc::new(Fake::default());
        let api = fake_headscale(f.clone()).await;
        let mut door = door_at(api.clone(), HashMap::new(), "age".into());
        door.admin_computers = ["a".to_string()].into();
        door.admin = Some("ann".into());
        for (kid, id) in [("1", "a"), ("2", "b")] {
            door.bound.lock().unwrap().insert(
                kid.into(),
                Bound {
                    owner: "ann".into(),
                    id: id.into(),
                    on: "node1".into(),
                    made: 0,
                },
            );
        }
        let tagged = |id: &str, kid: &str| {
            let mut n = computer_node(id, kid, "100.64.0.10");
            n["tags"] = serde_json::json!([COMPUTER_TAG, ADMIN_COMPUTER_TAG]);
            n
        };
        // an admin tag with no computer's beside it is still a computer's
        let mut bare = computer_node("12", "99", "100.64.0.12");
        bare["tags"] = serde_json::json!([ADMIN_COMPUTER_TAG]);
        *f.nodes.lock().unwrap() = vec![tagged("10", "1"), tagged("11", "2"), bare];
        let by = door.addresses_by_owner().await.unwrap();
        assert_eq!(
            by["ann"].len(),
            2,
            "both its owner's, by the keys they used"
        );
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let up = serde_json::json!({ "height": 3, "leaseHeight": 3, "time": ms,
            "computer": { "owner": "ann", "up": true, "consent": "own" },
            "lease": { "holder": "node1", "expires": ms + 15_000 }, "hostOwner": "ann" });
        *f.snaps.lock().unwrap() =
            HashMap::from([("a".to_string(), up.clone()), ("b".to_string(), up)]);
        door.reap(|_| crate::ident::Answer::Yes, Some(&api), &Guard::open())
            .await
            .unwrap();
        // a is listed and stays; b holds the tag and is off the list; the
        // bare one no key of the gate's ever made
        let mut gone = f.deleted.lock().unwrap().clone();
        gone.sort();
        assert_eq!(gone, ["11", "12"].map(String::from).to_vec());
    }

    /// A remote computer's network key: only to the box that signed for it,
    /// holds its lease, while it is agreed up and kept to its owner's own
    /// boxes; sealed to that box, bound to the computer and its owner, and
    /// the computer's one node: the one before comes off
    #[tokio::test]
    async fn a_computer_gets_a_key_only_through_the_box_that_runs_it() {
        use base64::Engine;
        use p256::ecdsa::{SigningKey, signature::Signer};
        use std::os::unix::fs::PermissionsExt;

        let f = Arc::new(Fake::default());
        let api = fake_headscale(f.clone()).await;
        let dir = std::env::temp_dir().join(format!("computer-join-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let age = dir.join("age");
        std::fs::write(&age, "#!/bin/sh\nprintf 'sealed to %s: ' \"$3\"; cat\n").unwrap();
        std::fs::set_permissions(&age, std::fs::Permissions::from_mode(0o755)).unwrap();
        let node1 = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let node2 = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let mut boxes = HashMap::new();
        for (n, k) in [("node1", &node1), ("node2", &node2)] {
            boxes.insert(
                n.to_string(),
                BoxKeys {
                    signs: *k.verifying_key(),
                    recipient: format!("age1{n}"),
                },
            );
        }
        let door = door_at(api, boxes, age.display().to_string());
        let id = "0123456789abcdef";
        let ask = |k: &SigningKey, name: &str, t: u64| {
            let s: Signature = k.sign(&computer_join_message(name, id, t));
            ComputerAsk {
                name: name.into(),
                id: id.into(),
                time: t,
                sig: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.to_bytes()),
            }
        };
        let now = 1_790_000_000;
        let now_ms = now as i64 * 1000;
        // the computer and its lease, as one fresh agreed state has them
        let snap = |f: &dyn Fn(&mut serde_json::Value)| {
            let mut v = serde_json::json!({
                "height": 9, "leaseHeight": 9, "time": now_ms - 1000,
                "computer": { "owner": "ann", "up": true, "consent": "own" },
                "lease": { "holder": "node1", "expires": now_ms + 10_000 },
                "hostOwner": "ann",
            });
            f(&mut v);
            Ok(serde_json::from_value::<Snapshot>(v).unwrap())
        };
        let same = |_: &mut serde_json::Value| {};
        let refused = |r: Result<String, (StatusCode, String)>| r.unwrap_err().0;
        let a = ask(&node1, "node1", now);

        // signed by another box, or asked by a box that is not the holder
        let forged = ask(&node2, "node1", now);
        assert_eq!(
            refused(computer_key(&door, &forged, now, snap(&same)).await),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            refused(computer_key(&door, &ask(&node2, "node2", now), now, snap(&same)).await),
            StatusCode::FORBIDDEN
        );
        for (why, change, code) in [
            ("unread", None, StatusCode::SERVICE_UNAVAILABLE),
            (
                "two states",
                Some(
                    &(|v: &mut serde_json::Value| v["leaseHeight"] = 8.into())
                        as &dyn Fn(&mut serde_json::Value),
                ),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                "stale",
                Some(&|v: &mut serde_json::Value| v["time"] = (now_ms - 60_000).into()),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                "none",
                Some(&|v: &mut serde_json::Value| v["computer"] = serde_json::Value::Null),
                StatusCode::NOT_FOUND,
            ),
            (
                "down",
                Some(&|v: &mut serde_json::Value| v["computer"]["up"] = false.into()),
                StatusCode::FORBIDDEN,
            ),
            (
                "anyone's",
                Some(&|v: &mut serde_json::Value| v["computer"]["consent"] = "any".into()),
                StatusCode::FORBIDDEN,
            ),
            (
                "no lease",
                Some(&|v: &mut serde_json::Value| v["lease"] = serde_json::Value::Null),
                StatusCode::FORBIDDEN,
            ),
            (
                "lapsed",
                Some(&|v: &mut serde_json::Value| v["lease"]["expires"] = (now_ms - 2000).into()),
                StatusCode::FORBIDDEN,
            ),
            (
                "not her box",
                Some(&|v: &mut serde_json::Value| v["hostOwner"] = "bob".into()),
                StatusCode::FORBIDDEN,
            ),
            (
                "no host",
                Some(&|v: &mut serde_json::Value| v["hostOwner"] = serde_json::Value::Null),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let s = match change {
                None => Err(()),
                Some(c) => snap(c),
            };
            assert_eq!(
                refused(computer_key(&door, &a, now, s).await),
                code,
                "{why}"
            );
        }
        assert!(f.made.lock().unwrap().is_empty());

        let sealed = computer_key(&door, &a, now, snap(&same)).await.unwrap();
        let answer: serde_json::Value =
            serde_json::from_str(sealed.strip_prefix("sealed to age1node1: ").unwrap()).unwrap();
        assert_eq!(answer["key"], "key-41");
        assert_eq!(answer["hostname"], format!("computer-{id}"));
        assert_eq!(
            f.made.lock().unwrap()[0]["aclTags"],
            serde_json::json!(["tag:computer"])
        );
        assert_eq!(
            door.bound.lock().unwrap().get("41"),
            Some(&Bound {
                owner: "ann".into(),
                id: id.into(),
                on: "node1".into(),
                made: now
            })
        );
        // once a minute
        assert_eq!(
            refused(
                computer_key(&door, &ask(&node1, "node1", now + 5), now + 5, snap(&same)).await
            ),
            StatusCode::TOO_MANY_REQUESTS
        );
        // its node joined; asked again later, that node comes off first
        f.nodes
            .lock()
            .unwrap()
            .push(computer_node("9", "41", "100.64.0.9"));
        let later = now + 61;
        let fresh = |v: &mut serde_json::Value| {
            v["time"] = (later as i64 * 1000).into();
            v["lease"]["expires"] = (later as i64 * 1000 + 10_000).into();
        };
        computer_key(&door, &ask(&node1, "node1", later), later, snap(&fresh))
            .await
            .unwrap();
        assert_eq!(*f.deleted.lock().unwrap(), vec!["9".to_string()]);
        // the key before it expired, and no longer bound
        assert_eq!(*f.expired.lock().unwrap(), vec!["41".to_string()]);
        assert!(!door.bound.lock().unwrap().contains_key("41"));
        assert!(door.bound.lock().unwrap().contains_key("42"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// a key headscale gave no id for is no key: its node could be no one's
    #[tokio::test]
    async fn a_key_without_an_id_is_refused() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        let app = Router::new()
            .route(
                "/api/v1/user",
                get(|| async {
                    Json(serde_json::json!({ "users": [{ "id": "7", "name": "boxes" }] }))
                }),
            )
            .route(
                "/api/v1/preauthkey",
                post(|| async { Json(serde_json::json!({ "preAuthKey": { "key": "k" } })) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let door = door_at(format!("http://{addr}"), HashMap::new(), "age".into());
        assert!(door.mint_id("boxes", &[COMPUTER_TAG]).await.is_err());
    }

    /// A computer's node goes once its computer is not agreed up, or its
    /// owner is no member, or the gate never made its key; it stays while
    /// the computers are unread. Its address is its owner's.
    #[tokio::test]
    async fn a_computers_node_lives_only_as_long_as_its_grant() {
        // the reaper takes one person's at a time; which comes off, not the order
        let sorted = |d: &Mutex<Vec<String>>| {
            let mut v = d.lock().unwrap().clone();
            v.sort();
            v
        };
        let f = Arc::new(Fake::default());
        let api = fake_headscale(f.clone()).await;
        let door = door_at(api.clone(), HashMap::new(), "age".into());
        for (kid, owner, id) in [("1", "ann", "a"), ("2", "ann", "b"), ("3", "bob", "d")] {
            door.bound.lock().unwrap().insert(
                kid.into(),
                Bound {
                    owner: owner.into(),
                    id: id.into(),
                    on: "node1".into(),
                    made: 0,
                },
            );
        }
        *f.nodes.lock().unwrap() = vec![
            computer_node("10", "1", "100.64.0.10"),
            computer_node("11", "2", "100.64.0.11"),
            computer_node("12", "3", "100.64.0.12"),
            computer_node("13", "99", "100.64.0.13"),
            serde_json::json!({ "id": "14", "user": { "name": "ann" }, "tags": [], "ipAddresses": ["100.64.0.14"] }),
        ];
        let by = door.addresses_by_owner().await.unwrap();
        assert_eq!(by["ann"], vec!["100.64.0.10", "100.64.0.11", "100.64.0.14"]);
        assert_eq!(by["bob"], vec!["100.64.0.12"]);
        assert!(!by.contains_key("boxes"));

        // no agreed state here at all: no computer has a node
        door.reap(
            |u| crate::ident::Answer::of(u == "ann"),
            None,
            &Guard::open(),
        )
        .await
        .unwrap();
        assert_eq!(
            sorted(&f.deleted),
            ["10", "11", "12", "13"].map(String::from).to_vec()
        );
        f.deleted.lock().unwrap().clear();

        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let snap = |owner: &str, up: bool, consent: &str| {
            serde_json::json!({ "height": 3, "leaseHeight": 3, "time": ms,
                "computer": { "owner": owner, "up": up, "consent": consent },
                "lease": { "holder": "node1", "expires": ms + 15_000 }, "hostOwner": owner })
        };
        // a: ann's, up, on her box; b: down; d: bob, no member; e: ann's,
        // on anyone's box; f: ann's, its lease lapsed; g: unknown (404)
        for (kid, id) in [("4", "e"), ("5", "f"), ("6", "g")] {
            door.bound.lock().unwrap().insert(
                kid.into(),
                Bound {
                    owner: "ann".into(),
                    id: id.into(),
                    on: "node1".into(),
                    made: 0,
                },
            );
        }
        for (n, kid) in [("15", "4"), ("16", "5"), ("17", "6")] {
            f.nodes
                .lock()
                .unwrap()
                .push(computer_node(n, kid, "100.64.0.99"));
        }
        let mut lapsed = snap("ann", true, "own");
        lapsed["lease"]["expires"] = (ms - 1).into();
        *f.snaps.lock().unwrap() = HashMap::from([
            ("a".to_string(), snap("ann", true, "own")),
            ("b".to_string(), snap("ann", false, "own")),
            ("d".to_string(), snap("bob", true, "own")),
            ("e".to_string(), snap("ann", true, "any")),
            ("f".to_string(), lapsed),
        ]);
        let gone = door
            .reap(
                |u| crate::ident::Answer::of(u == "ann"),
                Some(&api),
                &Guard::open(),
            )
            .await
            .unwrap();
        assert_eq!(gone, 6);
        assert_eq!(
            sorted(&f.deleted),
            ["11", "12", "13", "15", "16", "17"]
                .map(String::from)
                .to_vec()
        );

        // its lease moved to another box of ann's: the node given on node1 goes
        f.deleted.lock().unwrap().clear();
        *f.nodes.lock().unwrap() = vec![computer_node("10", "1", "100.64.0.10")];
        let mut moved = snap("ann", true, "own");
        moved["lease"]["holder"] = "node2".into();
        f.snaps.lock().unwrap().insert("a".into(), moved);
        assert_eq!(
            door.reap(
                |u| crate::ident::Answer::of(u == "ann"),
                Some(&api),
                &Guard::open()
            )
            .await
            .unwrap(),
            1
        );
        f.snaps
            .lock()
            .unwrap()
            .insert("a".into(), snap("ann", true, "own"));
        // and back on node1, it stays
        f.deleted.lock().unwrap().clear();
        assert_eq!(
            door.reap(
                |u| crate::ident::Answer::of(u == "ann"),
                Some(&api),
                &Guard::open()
            )
            .await
            .unwrap(),
            0
        );
        // whether ann is a member not known (the gate just started): it stays
        assert_eq!(
            door.reap(
                |_| crate::ident::Answer::Unknown,
                Some(&api),
                &Guard::open()
            )
            .await
            .unwrap(),
            0
        );

        // dd-control down: a's node stays on its last good word, for a while
        f.deleted.lock().unwrap().clear();
        *f.nodes.lock().unwrap() = vec![computer_node("10", "1", "100.64.0.10")];
        *f.down.lock().unwrap() = true;
        assert_eq!(
            door.reap(
                |u| crate::ident::Answer::of(u == "ann"),
                Some(&api),
                &Guard::open()
            )
            .await
            .unwrap(),
            0
        );
        // past the while, it comes off
        door.last_good.lock().unwrap().get_mut("a").unwrap().1 -= LAST_GOOD_MS + 1;
        assert_eq!(
            door.reap(
                |u| crate::ident::Answer::of(u == "ann"),
                Some(&api),
                &Guard::open()
            )
            .await
            .unwrap(),
            1
        );
        // and with no last good word at all, at once
        door.last_good.lock().unwrap().clear();
        f.deleted.lock().unwrap().clear();
        assert_eq!(
            door.reap(
                |u| crate::ident::Answer::of(u == "ann"),
                Some(&api),
                &Guard::open()
            )
            .await
            .unwrap(),
            1
        );
        // a last good word that says it is down takes it off as well
        *f.down.lock().unwrap() = false;
        f.snaps
            .lock()
            .unwrap()
            .insert("a".into(), snap("ann", false, "own"));
        door.reap(
            |u| crate::ident::Answer::of(u == "ann"),
            Some(&api),
            &Guard::open(),
        )
        .await
        .unwrap();
        *f.down.lock().unwrap() = true;
        f.deleted.lock().unwrap().clear();
        assert_eq!(
            door.reap(
                |u| crate::ident::Answer::of(u == "ann"),
                Some(&api),
                &Guard::open()
            )
            .await
            .unwrap(),
            1
        );
    }

    #[test]
    fn stamps() {
        assert_eq!(humantime(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            humantime(UNIX_EPOCH + Duration::from_secs(1_790_000_000)),
            "2026-09-21T14:13:20Z"
        );
    }
}
