//! The directory: a person's signed entry, stored and served by any box.
//!
//! No credential admits an update. What admits it is a signature by the key
//! already on file, so a box running only this has nothing to lose but the
//! copies, and a box that wants to lie can only withhold or replay.
//!
//! Boxes share it through the control plane's agreed log; none pulls
//! another. Reading it takes a credential (docs/dev/gate.md, the directory).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

pub struct Directory {
    dir: PathBuf,
    /// signed invites, by the code's public key; next to the entries
    invites: PathBuf,
    /// sign-ups that have not followed through yet: this box's alone
    held: PathBuf,
    /// every recovery this box took, by name (identity::Handover): what a
    /// client follows a name from a root it knows to the one it has now
    recoveries: PathBuf,
    /// where the release's signed member list is published (the releases
    /// branch); served at /_dd/members for clients to check themselves
    member_list: Option<String>,
    /// the member list as last fetched, and when
    member_list_seen: std::sync::Mutex<Option<(std::time::Instant, Option<String>)>>,
    /// held by the one fetch in flight: a burst of requests asks the forge once
    member_list_fetch: tokio::sync::Mutex<()>,
    /// the release key: what signs an invite. None: grants cannot be
    /// checked here and are refused
    release: Option<ed25519_dalek::VerifyingKey>,
    http: reqwest::Client,
    /// the release's member list; None: everyone with an entry counts
    members: Option<crate::Members>,
    /// keys that may read any one entry, never the list: the mail Worker's.
    /// None configured: single entries are anyone's, as before
    readers: Vec<String>,
    /// who a request's credential says it is: the full gate's sessions and
    /// device tokens. Unset: device tokens alone (identify_token)
    caller: std::sync::OnceLock<Caller>,
    /// the names this box answers to, from its configuration: a token for
    /// any other audience is no token here, whatever Host a request says
    names: Vec<String>,
    /// held by every write of an entry, from its last check to its rename
    writing: std::sync::Mutex<()>,
    /// where a member's name is dropped when their sessions in the other
    /// services on this box are to end (VERIFY_ENDED): a device or passkey
    /// removed, or "sign out everywhere else". Passwords reads it
    /// (modules/vault/vaultwarden.nix). None: nothing else to tell.
    ended: Option<PathBuf>,
    /// the control plane, when the directory is the agreed log's
    /// (VERIFY_CONTROL): writes go there, and this box's copy is what the
    /// log says, replayed through the same rules on every box
    agreed: Option<Agreed>,
    /// entries as last read, by name, with the file's time and size then:
    /// every request asks whose it is, and the file rarely changes
    read: std::sync::Mutex<
        std::collections::HashMap<String, (std::time::SystemTime, u64, identity::SignedEntry)>,
    >,
    /// who has left by their own word, and what this box did about it
    pub leaving: crate::leaving::Store,
    /// where who-is-who comes from (VERIFY_IDENTITY_FROM, ident.rs): this
    /// box's replay, the control plane's agreed state, or both compared
    pub ident_from: crate::ident::From,
    /// the agreed identity state, read from dd-control: set for shadow and
    /// agreed
    pub ident: Option<Arc<crate::ident::Agreed>>,
    /// whose each remote computer is, read from dd-control beside it
    pub computers: Option<Arc<crate::computers::Owners>>,
    /// what stands in front of the directory's and leaving's PUTs
    /// (docs/dev/gate.md, "Who is who"): one limiter by visitor, the
    /// per-name allowance, and the v2 items already sent on
    guard: std::sync::Mutex<PutGuard>,
}

/// a PUT's most: the control plane takes no bigger item
pub const ENVELOPE_MAX: usize = 64 * 1024;
/// PUTs a minute from one visitor, v1 and v2 alike, counted before a byte
/// of the body is read
pub const PUTS_PER_SOURCE: usize = 20;
/// v2 items a minute for one name, and by one signing key (one key may be
/// the root of several names), counted once the signature verified
pub const ENVELOPES_PER_NAME: usize = 6;
pub const ENVELOPES_PER_KEY: usize = 6;
/// how many visitors or names a window holds at once
pub const WINDOW_MAX: usize = 4096;
/// a v2 item sent on is not sent again for a day; the list holds at most this
const SENT_MAX: usize = 1 << 16;
const SENT_SECS: u64 = 24 * 3600;

#[derive(Default)]
pub struct PutGuard {
    by_source: Window,
    by_name: Window,
    by_key: Window,
    sent: Sent,
}

/// how many times each key went in the last minute, for at most WINDOW_MAX
/// keys: when full, the quiet go first, then the one quiet longest, so a
/// flood of new keys never shuts a newcomer out
#[derive(Default)]
pub(crate) struct Window(
    std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>,
);

impl Window {
    pub(crate) fn take(&mut self, key: &str, most: usize, now: std::time::Instant) -> bool {
        let minute = Duration::from_secs(60);
        let m = &mut self.0;
        if !m.contains_key(key) && m.len() >= WINDOW_MAX {
            m.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) <= minute));
            if m.len() >= WINDOW_MAX
                && let Some(k) = m
                    .iter()
                    .min_by_key(|(_, q)| q.back().copied())
                    .map(|(k, _)| k.clone())
            {
                m.remove(&k);
            }
        }
        let q = m.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) > minute) {
            q.pop_front();
        }
        if q.len() >= most {
            return false;
        }
        q.push_back(now);
        true
    }
}

/// The v2 items sent on, by sha256 of what was signed, with when. The file
/// and memory never differ: a change is written (to a temporary file, then
/// renamed over) before memory takes it, and a list that cannot be read or
/// written makes it `broken` - every v2 PUT is then refused (503), never
/// let through unchecked.
#[derive(Default)]
struct Sent {
    items: std::collections::VecDeque<(String, u64)>,
    file: Option<PathBuf>,
    broken: Option<String>,
}

impl Sent {
    fn load(file: PathBuf) -> Self {
        let (items, broken) = match std::fs::read(&file) {
            Ok(b) => match serde_json::from_slice(&b) {
                Ok(v) => (v, None),
                Err(e) => (Default::default(), Some(format!("{}: {e}", file.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Default::default(), None),
            Err(e) => (Default::default(), Some(format!("{}: {e}", file.display()))),
        };
        Self {
            items,
            file: Some(file),
            broken,
        }
    }

    fn ok(&self) -> std::result::Result<(), String> {
        match &self.broken {
            Some(why) => Err(format!("the list of items sent is unreadable ({why})")),
            None => Ok(()),
        }
    }

    fn has(&self, hash: &str, now: u64) -> std::result::Result<bool, String> {
        self.ok()?;
        Ok(self
            .items
            .iter()
            .any(|(h, at)| h == hash && now.saturating_sub(*at) <= SENT_SECS))
    }

    fn write(
        &mut self,
        next: std::collections::VecDeque<(String, u64)>,
    ) -> std::result::Result<(), String> {
        self.ok()?;
        if let Some(f) = &self.file {
            let tmp = f.with_extension("tmp");
            let res = serde_json::to_vec(&next)
                .map_err(|e| e.to_string())
                .and_then(|b| std::fs::write(&tmp, b).map_err(|e| e.to_string()))
                .and_then(|()| std::fs::rename(&tmp, f).map_err(|e| e.to_string()));
            if let Err(e) = res {
                self.broken = Some(format!("{}: {e}", f.display()));
                return self.ok();
            }
        }
        self.items = next;
        Ok(())
    }

    fn add(&mut self, hash: String, now: u64) -> std::result::Result<(), String> {
        let mut next: std::collections::VecDeque<(String, u64)> = self
            .items
            .iter()
            .filter(|(_, at)| now.saturating_sub(*at) <= SENT_SECS)
            .cloned()
            .collect();
        next.push_back((hash, now));
        while next.len() > SENT_MAX {
            next.pop_front();
        }
        self.write(next)
    }

    fn remove(&mut self, hash: &str) -> std::result::Result<(), String> {
        let next = self
            .items
            .iter()
            .filter(|(h, _)| h != hash)
            .cloned()
            .collect();
        self.write(next)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Listed {
    pub name: String,
    pub version: u64,
}

impl Directory {
    pub fn open(dir: PathBuf, release: Option<ed25519_dalek::VerifyingKey>) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let invites = dir
            .parent()
            .map(|p| p.join("invites"))
            .unwrap_or_else(|| dir.join("invites"));
        std::fs::create_dir_all(&invites)?;
        let held = dir
            .parent()
            .map(|p| p.join("held"))
            .unwrap_or_else(|| dir.join("held"));
        std::fs::create_dir_all(&held)?;
        let recoveries = dir
            .parent()
            .map(|p| p.join("recoveries"))
            .unwrap_or_else(|| dir.join("recoveries"));
        std::fs::create_dir_all(&recoveries)?;
        let leaving = crate::leaving::Store::open(Some(
            dir.parent()
                .map(|p| p.join("leaving.json"))
                .unwrap_or_else(|| dir.join("leaving.json")),
        ));
        Ok(Self {
            leaving,
            ended: std::env::var_os("VERIFY_ENDED").map(PathBuf::from),
            read: Default::default(),
            dir,
            invites,
            held,
            recoveries,
            member_list: None,
            member_list_seen: Default::default(),
            member_list_fetch: Default::default(),
            release,
            members: None,
            readers: vec![],
            caller: Default::default(),
            names: vec![],
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            writing: std::sync::Mutex::new(()),
            agreed: None,
            ident_from: crate::ident::From::Replay,
            ident: None,
            computers: None,
            guard: Default::default(),
        })
    }

    /// The directory as the control plane's agreed log has it. `control` is dd-control's address on this box, `submit` its
    /// socket for this box's items, `voters` the boxes whose gates must all
    /// acknowledge a new name before it is anyone's, `me` this box.
    pub fn agreed(
        mut self,
        control: String,
        submit: PathBuf,
        voters: Vec<String>,
        me: String,
    ) -> Result<Self> {
        let replay_file = self
            .dir
            .parent()
            .map(|p| p.join("replay.json"))
            .unwrap_or_else(|| self.dir.join("replay.json"));
        let replay = match std::fs::read(&replay_file) {
            Ok(b) => serde_json::from_slice(&b).context("replay.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Replay::default(),
            Err(e) => return Err(e.into()),
        };
        // what this box held before the log: served until the log has it
        let legacy = self
            .dir
            .parent()
            .map(|p| p.join("directory-before-log"))
            .unwrap_or_else(|| self.dir.join("before-log"));
        if !legacy.exists() && replay.next == 0 {
            std::fs::rename(&self.dir, &legacy)?;
            std::fs::create_dir_all(&self.dir)?;
        }
        let migrated = AtomicBool::new(replay.migrated);
        self.agreed = Some(Agreed {
            control,
            submit,
            voters,
            me,
            legacy,
            replay_file,
            replay: std::sync::Mutex::new(replay),
            changed: tokio::sync::Notify::new(),
            migrated,
        });
        Ok(self)
    }

    /// Where who-is-who comes from: in shadow and agreed, the control
    /// plane's agreed identity state at `control` (dd-control on this box)
    pub fn identity_from(mut self, from: crate::ident::From, control: Option<&str>) -> Self {
        self.ident_from = from;
        // the envelopes already sent on, kept across a restart
        let sent = self
            .dir
            .parent()
            .map(|p| p.join("envelopes-sent.json"))
            .unwrap_or_else(|| self.dir.join("envelopes-sent.json"));
        self.guard.lock().unwrap().sent = Sent::load(sent);
        if from != crate::ident::From::Replay
            && let Some(c) = control
        {
            self.ident = Some(Arc::new(crate::ident::Agreed::new(c)));
            self.computers = Some(Arc::new(crate::computers::Owners::new(c)));
        }
        self
    }

    /// The person as the agreed state has them; None when this box does
    /// not read it, or has not read it yet
    pub fn agreed_member(&self, name: &str) -> Option<crate::ident::Member> {
        self.ident.as_ref()?.member(name)
    }

    /// Whether the agreed state has been read at all: until it has, it
    /// knows nothing, which is not "nobody"
    pub(crate) fn agreed_known(&self) -> bool {
        self.ident.as_ref().is_some_and(|a| a.state().is_some())
    }

    /// The person's own signed entry: this box's replay, the agreed state,
    /// or the replay's checked against the agreed state (ident.rs)
    pub fn entry(&self, name: &str) -> Result<Option<identity::SignedEntry>> {
        use crate::ident::From;
        match self.ident_from {
            From::Replay => self.replay_entry(name),
            From::Agreed => {
                if !identity::valid_name(name) || !self.agreed_known() {
                    return Ok(None);
                }
                Ok(self.agreed_member(name).and_then(|m| m.entry()))
            }
            From::Shadow => {
                let r = self.replay_entry(name)?;
                if identity::valid_name(name) && self.agreed_known() {
                    let a = self.agreed_member(name).and_then(|m| m.entry());
                    for d in crate::ident::entry_differences(r.as_ref(), a.as_ref()) {
                        eprintln!("identity shadow: {name}: {d}");
                    }
                }
                Ok(r)
            }
        }
    }

    /// Whether the person has left by their own word: as entry() decides
    pub fn has_left(&self, name: &str) -> bool {
        use crate::ident::From;
        let r = || self.leaving.has_left(name);
        match self.ident_from {
            From::Replay => r(),
            From::Agreed => self.agreed_member(name).is_some_and(|m| m.has_left()),
            From::Shadow => {
                let replay = r();
                if self.agreed_known() {
                    let m = self.agreed_member(name);
                    let agreed = m.as_ref().is_some_and(|m| m.has_left());
                    if replay != agreed {
                        crate::ident::shadow_differs(name, "left", &replay, &agreed, m.as_ref());
                    }
                }
                replay
            }
        }
    }

    /// The replay's entry, or this box's held sign-up: what membership as
    /// the replay judges it reads, whatever the gate serves
    pub fn replay_or_held(&self, name: &str) -> Option<identity::SignedEntry> {
        self.replay_entry(name)
            .ok()
            .flatten()
            .or_else(|| self.held(name))
    }

    /// The person's own signed entry, as last accepted by this box's replay.
    fn replay_entry(&self, name: &str) -> Result<Option<identity::SignedEntry>> {
        // the name is a path component here, and this is reached from a PUT
        // before the accept rule has had a look at it
        if !identity::valid_name(name) {
            return Ok(None);
        }
        let mut p = self.dir.join(format!("{name}.json"));
        if let Some(a) = &self.agreed
            && !p.exists()
            && !a.migrated.load(Ordering::Relaxed)
        {
            p = a.legacy.join(format!("{name}.json"));
        }
        let meta = match std::fs::metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.read.lock().unwrap().remove(name);
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let stamp = (meta.modified()?, meta.len());
        let known = self
            .read
            .lock()
            .unwrap()
            .get(name)
            .filter(|(t, l, _)| (*t, *l) == stamp)
            .map(|(_, _, e)| e.clone());
        let e = match known {
            Some(e) => e,
            None => {
                let e: identity::SignedEntry = match std::fs::read(&p) {
                    Ok(b) => serde_json::from_slice(&b)?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(e.into()),
                };
                self.read
                    .lock()
                    .unwrap()
                    .insert(name.to_string(), (stamp.0, stamp.1, e.clone()));
                e
            }
        };
        if guest_expired(&e.entry) {
            // a probe's account, past its time: gone, name free again
            let _ = std::fs::remove_file(&p);
            self.read.lock().unwrap().remove(name);
            return Ok(None);
        }
        Ok(Some(e))
    }

    pub fn list(&self) -> Result<Vec<Listed>> {
        let mut out = Vec::new();
        for f in std::fs::read_dir(&self.dir)? {
            let f = f?;
            let Some(name) = f
                .file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_string)
            else {
                continue;
            };
            if !identity::valid_name(&name) {
                continue;
            }
            if let Ok(Some(e)) = self.entry(&name) {
                out.push(Listed {
                    name,
                    version: e.entry.version,
                });
            }
        }
        Ok(out)
    }

    /// The write, with the checks made again under the lock: a pull or a
    /// sign-up awaits the network between reading the entry and writing
    /// it, and whatever landed meanwhile - a device removed - is what the
    /// new one has to follow, not what was read before.
    fn commit(
        &self,
        signed: &identity::SignedEntry,
        grant: impl FnOnce(Option<&identity::SignedEntry>) -> std::result::Result<(), String>,
    ) -> std::result::Result<(), (StatusCode, String)> {
        let _w = self.writing.lock().unwrap_or_else(|p| p.into_inner());
        // with the agreed log, what the log has given the name, never the
        // copy from before the log (that would refuse the very entry being
        // migrated as not newer than itself)
        let current = match &self.agreed {
            Some(_) => Ok(self.agreed_entry(&signed.entry.name, identity::now())),
            None => self.entry(&signed.entry.name),
        }
        .map_err(|e| {
            eprintln!("directory: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, String::new())
        })?;
        identity::accept(current.as_ref(), signed)
            .map_err(|e| (StatusCode::FORBIDDEN, format!("refused: {e}")))?;
        grant(current.as_ref()).map_err(|e| (StatusCode::CONFLICT, e))?;
        self.claims_free(signed)
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        self.store(signed).map_err(|e| {
            eprintln!("directory: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, String::new())
        })?;
        if let Some(c) = &current
            && c.entry.root != signed.entry.root
            && let Err(e) = self.keep_handover(c, signed)
        {
            eprintln!("directory: keeping {}'s recovery: {e:#}", c.entry.name);
        }
        if current.is_some_and(|c| lost_a_key(&c.entry, &signed.entry)) {
            self.end_elsewhere(&signed.entry.name);
        }
        Ok(())
    }

    /// The member's sessions in the other services end; theirs to sign in
    /// to again, through the gate.
    pub fn end_elsewhere(&self, name: &str) {
        let Some(d) = &self.ended else { return };
        if !identity::valid_name(name) {
            return;
        }
        if let Err(e) = std::fs::write(d.join(name), b"") {
            eprintln!("directory: ending {name}'s sessions elsewhere: {e}");
        }
    }

    fn store(&self, signed: &identity::SignedEntry) -> Result<()> {
        let name = &signed.entry.name;
        anyhow::ensure!(identity::valid_name(name), "bad name");
        let p = self.dir.join(format!("{name}.json"));
        let tmp = self.dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(signed)?)?;
        std::fs::rename(&tmp, &p)?;
        // two writes in the same instant with the same length look alike
        self.read.lock().unwrap().remove(name.as_str());
        Ok(())
    }

    /// a recovery just taken, after the ones before it
    fn keep_handover(
        &self,
        before: &identity::SignedEntry,
        after: &identity::SignedEntry,
    ) -> Result<()> {
        // each takes the paper key on file, so a long run is a person's
        // own; the newest are what a recovery follows from a listed root
        const KEPT: usize = 32;
        let mut all = self.handovers(&after.entry.name);
        all.push(identity::Handover {
            before: before.clone(),
            after: after.clone(),
        });
        if all.len() > KEPT {
            all.drain(..all.len() - KEPT);
        }
        let p = self.recoveries.join(format!("{}.json", after.entry.name));
        let tmp = self.recoveries.join(format!(".{}.tmp", after.entry.name));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&all)?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    /// every recovery this box took for the name, oldest first
    pub fn handovers(&self, name: &str) -> Vec<identity::Handover> {
        if !identity::valid_name(name) {
            return vec![];
        }
        std::fs::read(self.recoveries.join(format!("{name}.json")))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// The release's member list as published, if it is signed by the
    /// release key this box knows; refetched every ten minutes (a minute
    /// while there is none). The client checks it again: this is only so
    /// a box does not pass on what it can already tell is not one.
    async fn member_list(&self) -> Option<String> {
        let url = self.member_list.as_ref()?;
        let cached = || {
            let seen = self.member_list_seen.lock().unwrap();
            let (at, seen) = seen.as_ref()?;
            let fresh = if seen.is_some() { 600 } else { 60 };
            (at.elapsed() < Duration::from_secs(fresh)).then(|| seen.clone())
        };
        if let Some(seen) = cached() {
            return seen;
        }
        let _one = self.member_list_fetch.lock().await;
        if let Some(seen) = cached() {
            return seen;
        }
        let read = async {
            let mut r = self
                .http
                .get(url)
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            // ids and tags, a few hundred bytes a member: anything past this
            // is not a member list
            const MAX: usize = 1 << 20;
            let mut body = Vec::new();
            while let Some(c) = r.chunk().await.ok()? {
                body.extend_from_slice(&c);
                if body.len() > MAX {
                    eprintln!("member list refused: over {MAX} bytes");
                    return None;
                }
            }
            let raw = String::from_utf8(body).ok()?;
            let s: identity::SignedMemberList = serde_json::from_str(&raw).ok()?;
            if let Some(k) = &self.release
                && let Err(e) = identity::verify_member_list(&s, k)
            {
                eprintln!("member list refused: {e}");
                return None;
            }
            Some(raw)
        };
        let seen = read.await;
        *self.member_list_seen.lock().unwrap() = Some((std::time::Instant::now(), seen.clone()));
        seen
    }

    /// A library id belongs to the entry that claimed it first. Ids are
    /// public - they ride in entries anyone may read - and the gate asks
    /// the entry whether it owns one, so without this rule naming someone
    /// else's id in your own entry makes you its owner.
    fn claims_free(&self, new: &identity::SignedEntry) -> std::result::Result<(), String> {
        if new.entry.libraries.is_empty() {
            return Ok(());
        }
        let listed = self.list().map_err(|e| {
            eprintln!("directory: {e:#}");
            "cannot read the directory".to_string()
        })?;
        for l in listed {
            if l.name == new.entry.name {
                continue;
            }
            let Ok(Some(e)) = self.entry(&l.name) else {
                continue;
            };
            for theirs in &e.entry.libraries {
                if new.entry.libraries.iter().any(|ours| ours.id == theirs.id) {
                    return Err(format!("library {} is already {}'s", theirs.id, l.name));
                }
            }
        }
        Ok(())
    }

    /// A sign-up that has not followed through: no code, and not on the
    /// member list. It is kept on the box that took it and nowhere else -
    /// never served from the directory, never pulled by a peer - so a name
    /// is not claimed across the fleet by someone who may never come back.
    /// It becomes an entry when it earns one: the member
    /// list naming it. Unfollowed, it is gone after a week and the name is
    /// free again.
    pub fn hold(
        &self,
        signed: &identity::SignedEntry,
    ) -> std::result::Result<(), (StatusCode, String)> {
        let name = &signed.entry.name;
        if !identity::valid_name(name) {
            return Err((StatusCode::BAD_REQUEST, "bad name".into()));
        }
        if let Err(e) = identity::accept(None, signed) {
            return Err((StatusCode::FORBIDDEN, format!("refused: {e}")));
        }
        identity::agreed_terms(&signed.entry)
            .map_err(|e| (StatusCode::FORBIDDEN, format!("refused: {e}")))?;
        no_passkey_keys(signed).map_err(|e| (StatusCode::FORBIDDEN, e))?;
        let taken = || (StatusCode::CONFLICT, "that name is taken".to_string());
        if self.entry(name).ok().flatten().is_some() {
            return Err(taken());
        }
        match self.held(name) {
            Some(h) if h.entry.root != signed.entry.root => return Err(taken()),
            Some(_) => {}
            // a name free again: whatever an earlier holder said is not this one's
            None => self.leaving.forget_said(name),
        }
        let write = || -> Result<()> {
            let tmp = self.held.join(format!(".{name}.tmp"));
            std::fs::write(&tmp, serde_json::to_vec_pretty(signed)?)?;
            std::fs::rename(&tmp, self.held.join(format!("{name}.json")))?;
            Ok(())
        };
        write().map_err(|e| {
            eprintln!("directory: holding {name}: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, String::new())
        })?;
        eprintln!("directory: holding {name} until it follows through");
        Ok(())
    }

    /// a held sign-up, if it is still within its time
    pub fn held(&self, name: &str) -> Option<identity::SignedEntry> {
        if !identity::valid_name(name) {
            return None;
        }
        let p = self.held.join(format!("{name}.json"));
        let e: identity::SignedEntry = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
        let ttl = if is_guest(name) { GUEST_TTL } else { HELD_TTL };
        if e.entry.updated + ttl < identity::now() {
            let _ = std::fs::remove_file(&p);
            return None;
        }
        Some(e)
    }

    pub fn unhold(&self, name: &str) {
        if identity::valid_name(name) {
            let _ = std::fs::remove_file(self.held.join(format!("{name}.json")));
        }
    }

    /// every held name still within its time
    pub fn held_names(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.held) else {
            return vec![];
        };
        rd.flatten()
            .filter_map(|f| {
                f.file_name()
                    .to_str()
                    .and_then(|n| n.strip_suffix(".json"))
                    .map(str::to_string)
            })
            .filter(|n| self.held(n).is_some())
            .collect()
    }
}

/// who a request's credential names, if it names anyone this box believes
pub type Caller = Box<dyn Fn(&HeaderMap) -> Option<String> + Send + Sync>;

impl Directory {
    pub fn with_members(mut self, members: Option<crate::Members>) -> Self {
        self.members = members;
        self
    }

    pub fn with_member_list(mut self, url: Option<String>) -> Self {
        self.member_list = url;
        self
    }

    pub fn with_readers(mut self, readers: Vec<String>) -> Self {
        self.readers = readers;
        self
    }

    pub fn with_names(mut self, names: Vec<String>) -> Self {
        self.names = names;
        self
    }

    /// Where a token is being used, for its audience check: the name the
    /// request was asked under when that is one of this box's own, and
    /// otherwise none, which no audience matches. The header picks among
    /// the configured names; it never adds one.
    pub fn here(&self, headers: &HeaderMap) -> String {
        let asked = crate::asked_host(headers);
        if self.names.contains(&asked) {
            asked
        } else {
            String::new()
        }
    }

    /// the full gate's way of telling who a request is (App::identify_how)
    pub fn set_caller(&self, c: Caller) {
        let _ = self.caller.set(c);
    }

    /// The fleet's services are theirs. Replaying: on the release's list
    /// (or let in by a code the owner signed), and not revoked; with no
    /// list, anyone with an entry. Agreed: the agreed state's access, an
    /// invite or a box that counts.
    pub fn member(&self, name: &str) -> bool {
        self.has_access(name, "member", || {
            let Some(e) = self.replay_entry(name).ok().flatten() else {
                return false;
            };
            let Some(m) = &self.members else {
                return true;
            };
            identity::is_member(&e.entry, &m.members, &m.revoked, self.release())
        })
    }

    /// Whether the fleet's services are this person's, where who-is-who
    /// comes from (ident.rs): `replay` is this box's own judgement; agreed,
    /// the agreed state's access (an invite, or a box of theirs that
    /// counts), nobody's while it is not read; shadow, the replay's, with a
    /// line in the log where the two differ naming who, `check`, and both
    /// answers.
    pub fn has_access(&self, name: &str, check: &str, replay: impl FnOnce() -> bool) -> bool {
        use crate::ident::From;
        let agreed = |m: Option<&crate::ident::Member>| m.is_some_and(|m| m.has_access());
        match self.ident_from {
            From::Replay => replay(),
            From::Agreed => self.agreed_known() && agreed(self.agreed_member(name).as_ref()),
            From::Shadow => {
                let r = replay();
                if self.agreed_known() {
                    let m = self.agreed_member(name);
                    let a = agreed(m.as_ref());
                    if r != a {
                        crate::ident::shadow_differs(name, check, &r, &a, m.as_ref());
                    }
                }
                r
            }
        }
    }

    /// The agreed state's word on someone, for a client holding an entry
    /// the release's list does not name (GET /_dd/directory/{name}/standing,
    /// /_dd/controller/standing/{name}): the root the agreed state has for
    /// them and whether it gives them the services. Only where the agreed
    /// state is what this box goes by (agreed): replaying, or in shadow,
    /// it says nothing, and a client holds them to the list alone as before.
    pub fn standing(&self, name: &str) -> Option<(String, bool, String)> {
        if self.ident_from != crate::ident::From::Agreed || !self.agreed_known() {
            return None;
        }
        let m = self.agreed_member(name)?;
        let root = m.entry()?.entry.root;
        Some((root, m.has_access(), m.access.clone()))
    }

    fn identify(&self, headers: &HeaderMap) -> Option<String> {
        match self.caller.get() {
            Some(c) => c(headers),
            None => {
                let tok = crate::bearer(headers)?;
                crate::verify_device_token(
                    |u| self.entry(u),
                    tok,
                    crate::read_operation(tok),
                    &self.here(headers),
                )
                .ok()
                .map(|(u, _)| u)
            }
        }
    }

    /// May this request read the list (`name` None) or one entry? The list
    /// is members'. An entry is a member's to read, its owner's, a holder's
    /// of a key in it (root, paper key, a device), or a reader key's; and
    /// anyone's while no reader key is configured.
    pub fn may_read(&self, headers: &HeaderMap, name: Option<&str>) -> bool {
        if let Some(u) = self.identify(headers)
            && (name == Some(u.as_str()) || self.member(&u))
        {
            return true;
        }
        let Some(name) = name else { return false };
        if self.readers.is_empty() {
            return true;
        }
        let now = identity::now();
        let proven: Vec<String> = headers
            .get_all(identity::READ_PROOF_HEADER)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| identity::check_read_proof(v, name, now))
            .collect();
        if proven.is_empty() {
            return false;
        }
        if proven.iter().any(|k| self.readers.contains(k)) {
            return true;
        }
        let Some(e) = self.entry(name).ok().flatten() else {
            return false;
        };
        let e = &e.entry;
        proven.iter().any(|k| {
            *k == e.root || *k == e.recovery || e.devices.iter().any(|d| d.public_key == *k)
        })
    }
}

pub fn router(d: Arc<Directory>) -> Router {
    Router::new()
        .route("/_dd/directory", get(list))
        .route("/_dd/directory/{name}", get(get_entry).put(put_entry))
        .route(
            "/_dd/computers/{kind}",
            axum::routing::put(crate::computers::put),
        )
        .route("/_dd/directory/{name}/recoveries", get(get_recoveries))
        .route("/_dd/directory/{name}/standing", get(get_standing))
        .route("/_dd/members", get(get_members))
        .route("/_dd/invite/{key}", get(get_invite).put(put_invite))
        .with_state(d)
}

/// 401 whether or not the name exists: a refusal says nothing about it
pub(crate) fn refused() -> Response {
    (StatusCode::UNAUTHORIZED, "the directory is for members").into_response()
}

async fn list(State(d): State<Arc<Directory>>, headers: HeaderMap) -> Response {
    if !d.may_read(&headers, None) {
        return refused();
    }
    match d.list() {
        Ok(l) => Json(l).into_response(),
        Err(e) => {
            eprintln!("directory: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn get_entry(
    State(d): State<Arc<Directory>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !identity::valid_name(&name) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !d.may_read(&headers, Some(&name)) {
        return refused();
    }
    match d.entry(&name) {
        Ok(Some(e)) => Json(e).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            eprintln!("directory: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The recoveries this box took for a name: read as the entry is
async fn get_recoveries(
    State(d): State<Arc<Directory>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !identity::valid_name(&name) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !d.may_read(&headers, Some(&name)) {
        return refused();
    }
    Json(d.handovers(&name)).into_response()
}

/// Whether the services are this person's, by the root their entry has:
/// read as the entry is. A client holds an entry to the release's list;
/// someone the list does not name (a member by a box) it holds to this,
/// asked of every directory it knows (client/directory standing)
async fn get_standing(
    State(d): State<Arc<Directory>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !identity::valid_name(&name) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !d.may_read(&headers, Some(&name)) {
        return refused();
    }
    standing_answer(&d, &name)
}

/// a standing, or why there is none: 503 where this box does not go by
/// the agreed state or has not read it (a client then hears nothing, which
/// is not "nobody"), 404 for a name the agreed state does not have
pub(crate) fn standing_answer(d: &Directory, name: &str) -> Response {
    if d.ident_from != crate::ident::From::Agreed || !d.agreed_known() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "this box does not go by the agreed state, or has not read it yet",
        )
            .into_response();
    }
    match d.standing(name) {
        Some((root, member, access)) => (
            [("cache-control", "no-store")],
            Json(serde_json::json!({
                "name": name, "root": root, "member": member, "access": access, "from": "agreed",
            })),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The signed member list, for anyone: member ids and revoked ids, which
/// fleet/members.json in the open repository carries already, and a paper
/// key on a new machine has nothing else to show for itself
async fn get_members(State(d): State<Arc<Directory>>) -> Response {
    match d.member_list().await {
        Some(raw) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            raw,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `key` is the invite's public key, base64url of the base64 text.
fn invite_key(key: &str) -> Option<String> {
    use base64::Engine as _;
    String::from_utf8(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(key)
            .ok()?,
    )
    .ok()
}

async fn get_invite(State(d): State<Arc<Directory>>, Path(key): Path<String>) -> Response {
    let Some(key) = invite_key(&key) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match d.invite(&key) {
        Some(inv) => Json(inv).into_response(),
        None => (StatusCode::NOT_FOUND, "no such invite, or it has expired").into_response(),
    }
}

async fn put_invite(
    State(d): State<Arc<Directory>>,
    Path(key): Path<String>,
    Json(inv): Json<identity::SignedInvite>,
) -> Response {
    if invite_key(&key).as_deref() != Some(inv.invite.public_key.as_str()) {
        return (StatusCode::BAD_REQUEST, "key in path and invite differ").into_response();
    }
    match d.put_invite_anywhere(&inv).await {
        Ok(()) => {
            eprintln!("directory: invite held until {}", inv.invite.expires);
            StatusCode::OK.into_response()
        }
        Err(e) => (StatusCode::FORBIDDEN, e).into_response(),
    }
}

/// who sent a request, for the envelope limits: the address nginx saw the
/// connection from, in X-Dd-Peer, which nginx sets on every request it
/// passes to the gate over whatever a client sent (strip-cookie.nix). Never
/// X-Real-IP or X-Forwarded-For: those are the client's to say. A request
/// that did not come through nginx is this box's own
pub fn source_of(headers: &HeaderMap) -> String {
    headers
        .get("x-dd-peer")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 64)
        .map(str::to_string)
        .unwrap_or_else(|| "local".into())
}

/// The start of every directory or leaving PUT: the visitor's allowance,
/// then the body, read only up to the most an item may be
pub async fn read_put(
    d: &Directory,
    headers: &HeaderMap,
    body: axum::body::Body,
) -> std::result::Result<axum::body::Bytes, Response> {
    if !d.put_allowed(headers) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "too many from here; wait a minute",
        )
            .into_response());
    }
    axum::body::to_bytes(body, ENVELOPE_MAX)
        .await
        .map_err(|_| (StatusCode::PAYLOAD_TOO_LARGE, "too big").into_response())
}

async fn put_entry(
    State(d): State<Arc<Directory>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    let body = match read_put(&d, &headers, body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    // a v2 envelope goes to the control plane as it came (ident.rs)
    if crate::ident::Envelope::is_one(&body) {
        return match d.submit_envelope("entry", &name, &body).await {
            Ok(hash) => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "submitted": true, "item": hash })),
            )
                .into_response(),
            Err((status, why)) => (status, why).into_response(),
        };
    }
    let signed: identity::SignedEntry = match serde_json::from_slice(&body) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("not an entry: {e}"),
            )
                .into_response();
        }
    };
    if signed.entry.name != name {
        return (StatusCode::BAD_REQUEST, "name in path and entry differ").into_response();
    }
    let version = signed.entry.version;
    // a name this box has never seen is a sign-up, which says what it
    // agreed to; an update, or an entry from before that was asked, need not
    if matches!(d.entry(&name), Ok(None))
        && let Err(e) = identity::agreed_terms(&signed.entry)
    {
        return (StatusCode::FORBIDDEN, format!("refused: {e}")).into_response();
    }
    match d.admit(signed).await {
        Ok(()) => Json(serde_json::json!({ "accepted": true, "version": version })).into_response(),
        Err((status, why)) => (status, why).into_response(),
    }
}

/// What a member publishes from now on carries no key for a passkey: any
/// page of the domain can ask a passkey for its PRF, and so for whatever
/// was sealed to it. Entries from before stay readable and still move
/// between boxes; their next version, made by an app or a dd that knows
/// this, drops those keys (account::reseal, identity::drop_passkey_keys).
pub(crate) fn no_passkey_keys(signed: &identity::SignedEntry) -> std::result::Result<(), String> {
    if identity::holds_passkey_keys(&signed.entry) {
        return Err(
            "refused: this entry keeps a key for a passkey; update the app or dd, which drop them"
                .into(),
        );
    }
    Ok(())
}

/// a device or passkey the old entry had and the new one does not
fn lost_a_key(old: &identity::Entry, new: &identity::Entry) -> bool {
    old.devices
        .iter()
        .any(|d| !new.devices.iter().any(|n| n.fingerprint == d.fingerprint))
        || old
            .passkeys
            .iter()
            .any(|p| !new.passkeys.iter().any(|n| n.id == p.id))
}

/// Names that begin with `guest` are for probes: a walk of the fleet as a
/// stranger, after a release. Nobody keeps one; a box drops the entry this
/// long after its last update, grant and all, and never pulls an old one.
pub const GUEST_PREFIX: &str = "guest";
pub const GUEST_TTL: u64 = 15 * 60;

/// how long a sign-up that has not followed through is kept
pub const HELD_TTL: u64 = 7 * 24 * 3600;

pub fn is_guest(name: &str) -> bool {
    name.starts_with(GUEST_PREFIX)
}

fn guest_expired(e: &identity::Entry) -> bool {
    is_guest(&e.name) && e.updated + GUEST_TTL < identity::now()
}

fn invite_file_name(public_key: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.as_bytes())
}

impl Directory {
    pub fn release(&self) -> Option<&ed25519_dalek::VerifyingKey> {
        self.release.as_ref()
    }

    /// An invite the owner signed, kept until it expires. Anyone may put
    /// one; only one the release key signed is kept.
    pub async fn put_invite_anywhere(
        &self,
        inv: &identity::SignedInvite,
    ) -> std::result::Result<(), String> {
        if self.agreed.is_some() {
            let Some(release) = &self.release else {
                return Err("this box holds no release key to check invites with".into());
            };
            identity::verify_invite(inv, release).map_err(|e| format!("refused: {e}"))?;
            return self
                .through_the_log(Item::Invite(inv.clone()))
                .await
                .map_err(|(_, e)| e);
        }
        self.put_invite(inv)
    }

    pub fn put_invite(&self, inv: &identity::SignedInvite) -> std::result::Result<(), String> {
        let Some(release) = &self.release else {
            return Err("this box holds no release key to check invites with".into());
        };
        identity::verify_invite(inv, release).map_err(|e| format!("refused: {e}"))?;
        // a window that never opened is refused; one about to close is held,
        // and `invite` drops it the moment it has: a short ttl must not lose
        // the race between minting and this put
        if inv.invite.expires <= inv.invite.issued {
            return Err("that invite has no window".into());
        }
        let p = self
            .invites
            .join(format!("{}.json", invite_file_name(&inv.invite.public_key)));
        let tmp = self
            .invites
            .join(format!(".{}.tmp", invite_file_name(&inv.invite.public_key)));
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(inv).map_err(|e| e.to_string())?,
        )
        .and_then(|_| std::fs::rename(&tmp, &p))
        .map_err(|e| e.to_string())
    }

    /// The invite for a code's public key, if held and not yet expired.
    /// Expired ones are removed as they are met.
    pub fn invite(&self, public_key: &str) -> Option<identity::SignedInvite> {
        let p = self
            .invites
            .join(format!("{}.json", invite_file_name(public_key)));
        let inv: identity::SignedInvite = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
        if inv.invite.expires <= identity::now() {
            let _ = std::fs::remove_file(&p);
            return None;
        }
        Some(inv)
    }

    /// One code, one person: is this invite already in another root's entry?
    pub fn grant_taken(&self, invite_public_key: &str, root: &str) -> bool {
        let Ok(listed) = self.list() else {
            return false;
        };
        listed.iter().any(|l| {
            self.entry(&l.name)
                .ok()
                .flatten()
                .and_then(|e| {
                    e.entry
                        .grant
                        .map(|g| (g.invite.invite.public_key, e.entry.root))
                })
                .is_some_and(|(k, r)| k == invite_public_key && r != root)
        })
    }

    /// A grant in an entry this box is about to hold: the owner's signature,
    /// the root's proof, and, if it is new here, still inside its window by
    /// this box's clock and not already used by someone else.
    fn check_grant(
        &self,
        existing: Option<&identity::SignedEntry>,
        new: &identity::Entry,
        at: u64,
    ) -> std::result::Result<(), String> {
        let Some(g) = &new.grant else {
            return Ok(());
        };
        let Some(release) = &self.release else {
            return Err("this box cannot check invites".into());
        };
        identity::verify_grant(new, release).map_err(|e| format!("grant: {e}"))?;
        let already = existing
            .and_then(|e| e.entry.grant.as_ref())
            .is_some_and(|old| old == g);
        if !already {
            if g.invite.invite.expires <= at {
                return Err("that invite has expired".into());
            }
            if self.grant_taken(&g.invite.invite.public_key, &new.root) {
                return Err("that code has already been used".into());
            }
        }
        Ok(())
    }

    /// The one way in: the accept rule, then the store (or the log). Used by the PUT from `dd` and by the browser join.
    pub(crate) async fn admit(
        &self,
        signed: identity::SignedEntry,
    ) -> std::result::Result<(), (StatusCode, String)> {
        let name = signed.entry.name.clone();
        let existing = self.entry(&name).map_err(|e| {
            eprintln!("directory: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, String::new())
        })?;
        if let Err(e) = identity::accept(existing.as_ref(), &signed) {
            eprintln!("directory: refused update for {name}: {e}");
            return Err((StatusCode::FORBIDDEN, format!("refused: {e}")));
        }
        if let Err(e) = no_passkey_keys(&signed) {
            eprintln!("directory: refused update for {name}: {e}");
            return Err((StatusCode::FORBIDDEN, e));
        }
        if let Err(e) = self.check_grant(existing.as_ref(), &signed.entry, identity::now()) {
            eprintln!("directory: refused update for {name}: {e}");
            return Err((StatusCode::CONFLICT, e));
        }
        if let Err(e) = self.claims_free(&signed) {
            eprintln!("directory: refused update for {name}: {e}");
            return Err((StatusCode::CONFLICT, e));
        }
        if self.agreed.is_some() {
            return self.through_the_log(Item::Entry(Box::new(signed))).await;
        }
        self.commit(&signed, |current| {
            self.check_grant(current, &signed.entry, identity::now())
        })
        .inspect_err(|(_, why)| eprintln!("directory: refused update for {name}: {why}"))?;
        eprintln!(
            "directory: {name} version {} ({} device(s), {} passkey(s){})",
            signed.entry.version,
            signed.entry.devices.len(),
            signed.entry.passkeys.len(),
            if signed.recovery_signature.is_some() {
                ", recovered"
            } else {
                ""
            }
        );
        Ok(())
    }
}

// ---- the directory as the control plane's agreed log
//
// Every write goes into the log through this box's dd-control (signed by
// its TPM); the validators order items and judge none. Every gate replays
// the log through the rules above, at each item's agreed time, so every
// box ends at the same directory. An update is the owner's own signature
// and needs nothing more. A name nobody holds yet is a claim: it is the
// claimant's only once the gate of every voting box has acknowledged that
// very claim in the log. A voter that lies can then stall a new name, and
// never give it to two people.

pub struct Agreed {
    control: String,
    submit: PathBuf,
    voters: Vec<String>,
    me: String,
    legacy: PathBuf,
    replay_file: PathBuf,
    replay: std::sync::Mutex<Replay>,
    changed: tokio::sync::Notify,
    /// Replay::migrated, read without its lock: every read of an entry
    /// asks, the replay among them while it holds the lock
    migrated: AtomicBool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Replay {
    /// the next log item to replay
    next: usize,
    /// claims to names nobody holds, waiting for every voter's ack, in log order
    pending: Vec<Pending>,
    /// claim hash -> the boxes whose gates acknowledged it
    acks: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// item hash -> what became of it, for a write waiting on it
    outcome: std::collections::BTreeMap<String, std::result::Result<(), String>>,
    /// this box's legacy entries are all in the log
    migrated: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct Pending {
    hash: String,
    entry: identity::SignedEntry,
    at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Item {
    Entry(Box<identity::SignedEntry>),
    Invite(identity::SignedInvite),
    Ack(String),
    /// leaving or coming back, signed by the root (leaving.rs)
    Leave(identity::SignedLeave),
}

#[derive(Deserialize)]
struct LogItem {
    time: i64,
    #[serde(rename = "box")]
    from: String,
    item: serde_json::Value,
}

#[derive(Deserialize)]
struct LogPage {
    items: Vec<LogItem>,
    next: usize,
}

/// an item's name in the log, for the replay's own claims, acks and
/// outcomes: its canonical json's sha256, the same whichever way the control
/// plane escaped it. Not the item hash the judge keys outcomes on
/// (identity::item_hash, sha256 of what was signed); acks name claims by
/// this until the replay is gone
fn item_hash(v: &serde_json::Value) -> String {
    use sha2::Digest as _;
    let b = serde_json::to_vec(v).unwrap_or_default();
    sha2::Sha256::digest(&b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

impl Directory {
    /// a write: this box's checks have passed; into the log, and the answer
    /// is what the replay made of it
    async fn through_the_log(&self, item: Item) -> std::result::Result<(), (StatusCode, String)> {
        let a = self.agreed.as_ref().expect("agreed mode");
        let v = serde_json::to_value(&item)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let hash = item_hash(&v);
        self.submit(&v).await.map_err(|e| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("the control plane: {e:#}"),
            )
        })?;
        // a new name waits for every voter's gate; updates land in seconds
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(r) = a.replay.lock().unwrap().outcome.get(&hash).cloned() {
                return r.map_err(|e| (StatusCode::CONFLICT, e));
            }
            let wait = a.changed.notified();
            if tokio::time::timeout_at(deadline, wait).await.is_err() {
                return Err((
                    StatusCode::ACCEPTED,
                    "sent; not agreed yet - it lands once every voting box has seen it".into(),
                ));
            }
        }
    }

    /// Leaving: through the log like any write, so every box holds the
    /// same date; without the control plane, this box's own. A coming back
    /// is not taken any more: leaving deletes everything at once.
    pub(crate) async fn say_leave(
        &self,
        s: identity::SignedLeave,
    ) -> std::result::Result<(), (StatusCode, String)> {
        if s.leave.back {
            return Err((StatusCode::CONFLICT, crate::leaving::NO_COMING_BACK.into()));
        }
        if self.leaving.has_left(&s.leave.name) {
            return Err((StatusCode::CONFLICT, "you have left already".into()));
        }
        let on_file = self.entry(&s.leave.name).ok().flatten();
        let Some(e) = on_file else {
            // a sign-up still waiting is this box's alone, and so is its word
            if let Some(h) = self.held(&s.leave.name) {
                return self
                    .leaving
                    .record(s, Some(&h.entry), identity::now())
                    .map_err(|e| (StatusCode::CONFLICT, e));
            }
            return Err((StatusCode::NOT_FOUND, "no entry by that name".into()));
        };
        identity::verify_leave(&s, &e.entry)
            .map_err(|e| (StatusCode::FORBIDDEN, format!("refused: {e}")))?;
        if self.agreed.is_some() {
            return self.through_the_log(Item::Leave(s)).await;
        }
        self.leaving
            .record(s, Some(&e.entry), identity::now())
            .map_err(|e| (StatusCode::CONFLICT, e))
    }

    /// one item to this box's dd-control, over its socket
    async fn submit(&self, v: &serde_json::Value) -> Result<()> {
        self.submit_raw(serde_json::to_vec(v)?).await
    }

    /// Whether this box takes a v2 envelope (docs/dev/identity-state.md
    /// §1): only where the control plane judges, which is where the gate
    /// reads its judgement - shadow or agreed, on the agreed log
    pub fn takes_v2(&self) -> bool {
        self.agreed.is_some() && self.ident_from != crate::ident::From::Replay
    }

    /// A v2 envelope, put here: checked to be one, of the kind the path
    /// says and for the name the path says, and handed to the control plane
    /// as the bytes it came in. Judging it is the control plane's; nothing
    /// here re-encodes it, and nothing here keeps it.
    pub async fn submit_envelope(
        &self,
        kind: &str,
        name: &str,
        bytes: &[u8],
    ) -> std::result::Result<String, (StatusCode, String)> {
        if !self.takes_v2() {
            return Err((
                StatusCode::BAD_REQUEST,
                "this box takes v1 items only for now (/_dd/config has no envelope)".into(),
            ));
        }
        let env = crate::ident::Envelope::parse(bytes).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        if env.kind != kind {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("a {} envelope, put where a {kind} goes", env.kind),
            ));
        }
        if env.name() != Some(name) || !identity::valid_name(name) {
            return Err((
                StatusCode::BAD_REQUEST,
                "name in path and body differ".into(),
            ));
        }
        let hash = env.signed_hash();
        self.not_seen(&hash, bytes)?;
        // signed by the key that speaks for the name: checked, not judged
        let signer = self
            .envelope_signed(kind, name, bytes)
            .map_err(|e| (StatusCode::FORBIDDEN, format!("refused: {e}")))?;
        self.send_verified(name, &signer, hash, bytes).await
    }

    /// An envelope PUT whose kind and whom it speaks for are settled
    /// elsewhere (member_boxes.rs): refused if already sent or in the log,
    /// the signature checked by `signed` (the key that signed), then sent
    pub async fn submit_verified(
        &self,
        name: &str,
        env: &crate::ident::Envelope,
        bytes: &[u8],
        signed: impl FnOnce() -> std::result::Result<String, String>,
    ) -> std::result::Result<String, (StatusCode, String)> {
        let hash = env.signed_hash();
        self.not_seen(&hash, bytes)?;
        let signer = signed().map_err(|e| (StatusCode::FORBIDDEN, format!("refused: {e}")))?;
        self.send_verified(name, &signer, hash, bytes).await
    }

    /// A remote computer's item (servers/gate/src/computers.rs): of a kind
    /// put there, the signed-in person's own, signed by their root - or,
    /// to start or stop one, by a device of theirs - before it is sent on.
    /// The owner of a start or stop comes from the agreed /computers, as
    /// last read: not read yet, nothing goes through.
    pub async fn submit_computer(
        &self,
        kind: &str,
        headers: &HeaderMap,
        bytes: &[u8],
    ) -> std::result::Result<String, (StatusCode, String)> {
        if !self.takes_v2() {
            return Err((
                StatusCode::BAD_REQUEST,
                "this box takes v1 items only for now (/_dd/config has no envelope)".into(),
            ));
        }
        let Some(&(_, owned)) = crate::computers::KINDS.iter().find(|(k, _)| *k == kind) else {
            return Err((StatusCode::NOT_FOUND, format!("{kind} is not put here")));
        };
        // who anyone is comes from the agreed state: not read yet, nothing
        // can be checked, and nothing goes through
        if !self.agreed_known() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "the agreed state is not read yet; try again".into(),
            ));
        }
        let env = crate::ident::Envelope::parse(bytes).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        if env.kind != kind {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("a {} envelope, put where a {kind} goes", env.kind),
            ));
        }
        let me = self
            .identify(headers)
            .ok_or((StatusCode::UNAUTHORIZED, "sign in first".to_string()))?;
        // where it goes, read before the signature is: only to know whose
        // key to check it with, never believed
        let owner = if owned {
            env.field("owner")
                .ok_or((StatusCode::BAD_REQUEST, "no owner named".to_string()))?
                .to_string()
        } else {
            let id = env
                .field("id")
                .ok_or((StatusCode::BAD_REQUEST, "no computer named".to_string()))?;
            let owners = self.computers.as_ref().ok_or((
                StatusCode::SERVICE_UNAVAILABLE,
                "this box does not read the computers yet".to_string(),
            ))?;
            owners
                .owner_of(id)
                .map_err(|()| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "the computers are not read yet; try again".to_string(),
                    )
                })?
                .ok_or((StatusCode::NOT_FOUND, "no such remote computer".to_string()))?
        };
        if owner != me || !identity::valid_name(&owner) {
            return Err((StatusCode::FORBIDDEN, "not your remote computer".into()));
        }
        let hash = env.signed_hash();
        self.not_seen(&hash, bytes)?;
        let signer = self
            .computer_signed(&owner, !owned, bytes)
            .map_err(|e| (StatusCode::FORBIDDEN, format!("refused: {e}")))?;
        self.send_verified(&owner, &signer, hash, bytes).await
    }

    /// the owner's root as the agreed state has it signed it; or, where a
    /// device may (a start or stop), one of their devices on it now
    fn computer_signed(
        &self,
        owner: &str,
        device_may: bool,
        bytes: &[u8],
    ) -> std::result::Result<String, String> {
        use identity::envelope::Envelope;
        let env: Envelope =
            serde_json::from_slice(bytes).map_err(|e| format!("not an envelope: {e}"))?;
        // no paper key speaks for a computer; a signature that means nothing
        // would still make the item another, past the list of items sent
        if env.recovery_sig.is_some() {
            return Err("a recovery signature has no place here".into());
        }
        let cur = self
            .agreed_member(owner)
            .and_then(|m| m.entry())
            .ok_or("nobody by that name")?;
        let root = &cur.entry.root;
        let by_root = identity::envelope::verify_root(&env, root, &cur.entry.passkeys);
        if by_root.is_ok() {
            return Ok(root.clone());
        }
        if device_may {
            let msg = env.message().map_err(|e| e.to_string())?;
            for d in &cur.entry.devices {
                if let Ok(k) = identity::decode_public(&d.public_key)
                    && identity::envelope::verify_ed25519(&env.sig, &msg, &k).is_ok()
                {
                    return Ok(d.public_key.clone());
                }
            }
            return Err("not signed by them or a device of theirs".into());
        }
        by_root.map(|()| root.clone()).map_err(|e| e.to_string())
    }

    /// Not sent already, by the shared item hash: the same kind and body
    /// signed is the same item, however its outer JSON is written. A list
    /// that cannot be read lets nothing through. Nor an item the agreed state
    /// says is in the log already, however long ago (from the state as last
    /// read, never a wait for it).
    pub(crate) fn not_seen(
        &self,
        hash: &str,
        bytes: &[u8],
    ) -> std::result::Result<(), (StatusCode, String)> {
        let unavailable = |e: String| (StatusCode::SERVICE_UNAVAILABLE, e);
        if self
            .guard
            .lock()
            .unwrap()
            .sent
            .has(hash, identity::now())
            .map_err(unavailable)?
        {
            return Err((StatusCode::CONFLICT, "that item was sent already".into()));
        }
        if self.in_agreed_log(bytes) {
            return Err((
                StatusCode::CONFLICT,
                "that item is in the log already".into(),
            ));
        }
        Ok(())
    }

    /// An item whose signature passed, to the control plane: the name's and
    /// the key's allowance first (only items that verified count: a forgery
    /// in a member's name spends nothing of theirs), then sent once
    pub(crate) async fn send_verified(
        &self,
        name: &str,
        signer: &str,
        hash: String,
        bytes: &[u8],
    ) -> std::result::Result<String, (StatusCode, String)> {
        let unavailable = |e: String| (StatusCode::SERVICE_UNAVAILABLE, e);
        {
            let mut g = self.guard.lock().unwrap();
            let now = std::time::Instant::now();
            if !g.by_name.take(name, ENVELOPES_PER_NAME, now)
                || !g.by_key.take(signer, ENVELOPES_PER_KEY, now)
            {
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many for this name; wait a minute".into(),
                ));
            }
            if g.sent.has(&hash, identity::now()).map_err(unavailable)? {
                return Err((StatusCode::CONFLICT, "that item was sent already".into()));
            }
            g.sent
                .add(hash.clone(), identity::now())
                .map_err(unavailable)?;
        }
        if let Err(e) = self.submit_raw(bytes.to_vec()).await {
            // not sent: the same item may be tried again
            let _ = self.guard.lock().unwrap().sent.remove(&hash);
            return Err((StatusCode::BAD_GATEWAY, format!("the control plane: {e:#}")));
        }
        Ok(hash)
    }

    /// Whether the agreed state, as last read, records this item (by the
    /// log's item hash) as judged already. Not read yet: no answer, and the
    /// sent list stands alone
    fn in_agreed_log(&self, bytes: &[u8]) -> bool {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return false;
        };
        let hash = identity::item_hash(&v);
        // a remote computer's item is judged beside identity, and its outcome
        // kept there (state/computers)
        if self.computers.as_ref().is_some_and(|c| c.judged(&hash)) {
            return true;
        }
        let Some(state) = self.ident.as_ref().and_then(|a| a.state()) else {
            return false;
        };
        state.outcomes.contains_key(&hash)
    }

    /// Room for one more PUT from this visitor: the one limiter in front of
    /// the directory's and leaving's PUTs, v1 and v2, asked before a byte of
    /// the body is read
    pub fn put_allowed(&self, headers: &HeaderMap) -> bool {
        self.guard.lock().unwrap().by_source.take(
            &source_of(headers),
            PUTS_PER_SOURCE,
            std::time::Instant::now(),
        )
    }

    /// An envelope's signature, by the key that speaks for `name` now: for
    /// an entry, the root on file (a changed root: the new root and the
    /// recovery key on file both), or for a name nobody holds, the root the
    /// entry names, with the terms agreed - a sign-up, as v1 takes one; for
    /// a leaving, the root on file
    fn envelope_signed(
        &self,
        kind: &str,
        name: &str,
        bytes: &[u8],
    ) -> std::result::Result<String, String> {
        use identity::envelope::Envelope;
        let env: Envelope =
            serde_json::from_slice(bytes).map_err(|e| format!("not an envelope: {e}"))?;
        let body = env.body_bytes().map_err(|e| e.to_string())?;
        let on_file = self.entry(name).map_err(|e| e.to_string())?;
        let check = |root: &str, sig: &str, trusted: &[identity::Passkey]| {
            identity::envelope::verify_root_sig(&env, sig, root, trusted).map_err(|e| e.to_string())
        };
        match kind {
            "entry" => {
                let new: identity::Entry =
                    serde_json::from_slice(&body).map_err(|e| format!("not an entry: {e}"))?;
                match on_file {
                    None => {
                        identity::agreed_terms(&new).map_err(|e| e.to_string())?;
                        check(&new.root, &env.sig, &new.passkeys)?;
                        Ok(new.root)
                    }
                    Some(cur) if cur.entry.root == new.root => {
                        check(&cur.entry.root, &env.sig, &cur.entry.passkeys)?;
                        Ok(new.root)
                    }
                    Some(cur) => {
                        // a new root: its own signature, and the recovery
                        // key on file says so too
                        check(&new.root, &env.sig, &new.passkeys)?;
                        let rs = env
                            .recovery_sig
                            .as_deref()
                            .ok_or("a new root needs the recovery key's signature")?;
                        check(&cur.entry.recovery, rs, &[])?;
                        Ok(new.root)
                    }
                }
            }
            "leave" => {
                let cur = on_file.ok_or("nobody by that name")?;
                check(&cur.entry.root, &env.sig, &cur.entry.passkeys)?;
                Ok(cur.entry.root)
            }
            _ => Err(format!("{kind} is not put here")),
        }
    }

    /// bytes to this box's dd-control, over its socket, as they are
    async fn submit_raw(&self, body: Vec<u8>) -> Result<()> {
        let a = self.agreed.as_ref().expect("agreed mode");
        let sock = a.submit.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            use std::io::{Read as _, Write as _};
            let mut c = std::os::unix::net::UnixStream::connect(&sock)
                .with_context(|| format!("{}", sock.display()))?;
            c.set_read_timeout(Some(Duration::from_secs(30)))?;
            write!(
                c,
                "POST /submit HTTP/1.1\r\nHost: control\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )?;
            c.write_all(&body)?;
            let mut answer = String::new();
            c.read_to_string(&mut answer)?;
            let status = answer.split_whitespace().nth(1).unwrap_or("");
            anyhow::ensure!(status == "202", "dd-control answered {}", answer.lines().next().unwrap_or(""));
            Ok(())
        })
        .await?
    }

    /// Forever: replay what the log has added, acknowledge the claims this
    /// box finds good, and put this box's pre-log entries into the log.
    pub async fn follow(self: Arc<Self>) {
        let Some(a) = self.agreed.as_ref() else {
            return;
        };
        let mut sent_legacy = std::collections::BTreeSet::new();
        loop {
            match self.catch_up().await {
                Ok(acks) => {
                    for hash in acks {
                        if let Err(e) = self
                            .submit(
                                &serde_json::to_value(Item::Ack(hash.clone())).unwrap_or_default(),
                            )
                            .await
                        {
                            eprintln!("directory: acknowledging {hash}: {e:#}");
                        }
                    }
                    if let Err(e) = self.migrate(&mut sent_legacy).await {
                        eprintln!("directory: putting the old entries in the log: {e:#}");
                    }
                }
                Err(e) => eprintln!("directory: reading the log: {e:#}"),
            }
            let _ = a;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Replay every new item; returns the claims this box should ack
    async fn catch_up(&self) -> Result<Vec<String>> {
        let a = self.agreed.as_ref().expect("agreed mode");
        let next = a.replay.lock().unwrap().next;
        let page: LogPage = self
            .http
            .get(format!(
                "{}/directory?since={next}",
                a.control.trim_end_matches('/')
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut to_ack = Vec::new();
        if page.items.is_empty() {
            return Ok(to_ack);
        }
        {
            let mut r = a.replay.lock().unwrap();
            for it in page.items {
                let at = (it.time / 1000).max(0) as u64;
                if let Some(h) = self.replay_one(&mut r, &it.from, it.item, at) {
                    to_ack.push(h);
                }
                r.next += 1;
            }
            debug_assert!(r.next <= page.next);
            let b = serde_json::to_vec(&*r)?;
            let tmp = a.replay_file.with_extension("tmp");
            std::fs::write(&tmp, b)?;
            std::fs::rename(&tmp, &a.replay_file)?;
        }
        a.changed.notify_waiters();
        Ok(to_ack)
    }

    /// One item, at its agreed time. Returns a claim's hash when this box's
    /// gate should acknowledge it.
    fn replay_one(
        &self,
        r: &mut Replay,
        from: &str,
        v: serde_json::Value,
        at: u64,
    ) -> Option<String> {
        let a = self.agreed.as_ref().expect("agreed mode");
        let hash = item_hash(&v);
        let item: Item = match serde_json::from_value(v) {
            Ok(i) => i,
            Err(e) => {
                r.outcome.insert(hash, Err(format!("not an item: {e}")));
                return None;
            }
        };
        let mut ack = None;
        match item {
            Item::Leave(s) => {
                let on_file = self.agreed_entry(&s.leave.name, at);
                let res = self
                    .leaving
                    .record(s, on_file.as_ref().map(|e| &e.entry), at);
                r.outcome.insert(hash, res);
            }
            Item::Ack(claim) => {
                if a.voters.iter().any(|v| v == from) {
                    r.acks.entry(claim).or_default().insert(from.to_string());
                }
            }
            Item::Invite(inv) => {
                let ok = match &self.release {
                    Some(k) => identity::verify_invite(&inv, k)
                        .map_err(|e| format!("refused: {e}"))
                        .and_then(|()| {
                            if inv.invite.expires <= at {
                                Err("that invite had expired".into())
                            } else {
                                self.put_invite(&inv)
                            }
                        }),
                    None => Err("no release key here".into()),
                };
                r.outcome.insert(hash, ok);
            }
            Item::Entry(signed) => {
                let signed = *signed;
                let name = signed.entry.name.clone();
                let existing = self.agreed_entry(&name, at);
                if existing.is_some() {
                    // an update: the owner's signature is all it needs
                    let res = self
                        .commit(&signed, |cur| self.check_grant(cur, &signed.entry, at))
                        .map_err(|(_, e)| e);
                    r.outcome.insert(hash, res);
                } else {
                    // a claim: good now, and then every voter's gate says so
                    let good = identity::accept(None, &signed)
                        .map_err(|e| format!("refused: {e}"))
                        .and_then(|()| self.check_grant(None, &signed.entry, at))
                        .and_then(|()| self.claims_free(&signed));
                    match good {
                        Ok(()) => {
                            if !r.pending.iter().any(|p| p.hash == hash) {
                                r.pending.push(Pending {
                                    hash: hash.clone(),
                                    entry: signed,
                                    at,
                                });
                            }
                            if a.voters.iter().any(|v| v == &a.me) {
                                ack = Some(hash);
                            }
                        }
                        Err(e) => {
                            r.outcome.insert(hash, Err(e));
                        }
                    }
                }
            }
        }
        self.settle(r);
        ack
    }

    /// Claims every voter acknowledged become entries, the first in log
    /// order winning its name; the rest for that name are refused.
    fn settle(&self, r: &mut Replay) {
        let a = self.agreed.as_ref().expect("agreed mode");
        loop {
            let ready = r.pending.iter().position(|p| {
                r.acks
                    .get(&p.hash)
                    .is_some_and(|got| a.voters.iter().all(|v| got.contains(v)))
            });
            let Some(i) = ready else { break };
            let p = r.pending.remove(i);
            let name = p.entry.entry.name.clone();
            let res = if self.agreed_entry(&name, p.at).is_some() {
                Err(format!("{name} is already taken"))
            } else {
                self.commit(&p.entry, |cur| self.check_grant(cur, &p.entry.entry, p.at))
                    .map_err(|(_, e)| e)
            };
            if res.is_ok() {
                eprintln!("directory: {name} is now in the agreed log");
                // the rest for this name: an update by the same owner (sent
                // while the name was still being agreed) lands now as one;
                // anyone else's claim lost
                let others: Vec<Pending> = r
                    .pending
                    .iter()
                    .filter(|q| q.entry.entry.name == name)
                    .cloned()
                    .collect();
                r.pending.retain(|q| q.entry.entry.name != name);
                let mut others = others;
                others.sort_by_key(|q| q.entry.entry.version);
                for q in others {
                    let res = self
                        .commit(&q.entry, |cur| self.check_grant(cur, &q.entry.entry, q.at))
                        .map_err(|_| format!("{name} went to an earlier claim"));
                    r.outcome.insert(q.hash, res);
                }
            }
            r.outcome.insert(p.hash, res);
        }
    }

    /// the entry the log has given this name, never the pre-log copy
    fn agreed_entry(&self, name: &str, at: u64) -> Option<identity::SignedEntry> {
        if !identity::valid_name(name) {
            return None;
        }
        let b = std::fs::read(self.dir.join(format!("{name}.json"))).ok()?;
        let e: identity::SignedEntry = serde_json::from_slice(&b).ok()?;
        (!(is_guest(&e.entry.name) && e.entry.updated + GUEST_TTL < at)).then_some(e)
    }

    /// This box's entries and invites from before the log, into it, once
    /// each; when the log has every one, the old copy is no longer read.
    async fn migrate(&self, sent: &mut std::collections::BTreeSet<String>) -> Result<()> {
        let a = self.agreed.as_ref().expect("agreed mode");
        if a.migrated.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut missing = 0;
        if let Ok(rd) = std::fs::read_dir(&a.legacy) {
            for f in rd.flatten() {
                let Some(name) = f
                    .file_name()
                    .to_str()
                    .and_then(|n| n.strip_suffix(".json"))
                    .map(str::to_string)
                else {
                    continue;
                };
                if !identity::valid_name(&name)
                    || self.agreed_entry(&name, identity::now()).is_some()
                {
                    continue;
                }
                let Ok(b) = std::fs::read(f.path()) else {
                    continue;
                };
                let Ok(e) = serde_json::from_slice::<identity::SignedEntry>(&b) else {
                    continue;
                };
                if guest_expired(&e.entry) {
                    continue;
                }
                missing += 1;
                if sent.insert(name.clone()) {
                    self.submit(&serde_json::to_value(Item::Entry(Box::new(e)))?)
                        .await?;
                    eprintln!("directory: {name} sent to the log");
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(&self.invites) {
            for f in rd.flatten() {
                let key = f.path().display().to_string();
                if sent.contains(&key) {
                    continue;
                }
                let Ok(b) = std::fs::read(f.path()) else {
                    continue;
                };
                let Ok(inv) = serde_json::from_slice::<identity::SignedInvite>(&b) else {
                    continue;
                };
                if inv.invite.expires > identity::now() {
                    self.submit(&serde_json::to_value(Item::Invite(inv))?)
                        .await?;
                }
                sent.insert(key);
            }
        }
        if missing == 0 {
            a.replay.lock().unwrap().migrated = true;
            a.migrated.store(true, Ordering::Relaxed);
            eprintln!("directory: every entry from before the log is in it");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use identity::{Device, Entry, Library, encode_public, fingerprint, generate, sign};

    fn scratch(what: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dd-dir-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn entry(name: &str, key: &ed25519_dalek::SigningKey, libs: &[&str], version: u64) -> Entry {
        Entry {
            name: name.into(),
            root: encode_public(&key.verifying_key()),
            // one paper key per person, the same in every version: a new one
            // each time would be a recovery-key swap, which only the old
            // paper key may sign
            recovery: {
                use sha2::Digest as _;
                let seed: [u8; 32] = sha2::Sha256::digest(key.to_bytes()).into();
                encode_public(&ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key())
            },
            recovered_from: vec![],
            devices: vec![{
                let p = encode_public(&generate().verifying_key());
                Device {
                    fingerprint: fingerprint(&p),
                    public_key: p,
                    added: 1,
                }
            }],
            passkeys: vec![],
            grant: None,
            libraries: libs
                .iter()
                .map(|id| Library {
                    id: (*id).into(),
                    keys: vec![],
                    readers: vec![],
                    created: 1,
                })
                .collect(),
            terms: Some(identity::TERMS.into()),
            removed_boxes: vec![],
            version,
            updated: identity::now(),
        }
    }

    #[test]
    fn an_entry_changed_on_disk_is_read_again() {
        let dir = scratch("cache");
        let d = Directory::open(dir.clone(), None).unwrap();
        let k = generate();
        let write = |v| {
            let e = sign(entry("ann", &k, &[], v), &k).unwrap();
            std::fs::write(dir.join("ann.json"), serde_json::to_vec(&e).unwrap()).unwrap();
        };
        write(1);
        assert_eq!(d.entry("ann").unwrap().unwrap().entry.version, 1);
        assert_eq!(d.entry("ann").unwrap().unwrap().entry.version, 1);
        write(22);
        assert_eq!(d.entry("ann").unwrap().unwrap().entry.version, 22);
        std::fs::remove_file(dir.join("ann.json")).unwrap();
        assert!(d.entry("ann").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_a_device_or_passkey_is_noticed_and_adding_one_is_not() {
        let k = generate();
        let old = entry("ann", &k, &[], 1);
        let mut added = old.clone();
        let p = encode_public(&generate().verifying_key());
        added.devices.push(Device {
            fingerprint: fingerprint(&p),
            public_key: p,
            added: 2,
        });
        assert!(!lost_a_key(&old, &added));
        assert!(lost_a_key(&added, &old));
        let mut fewer = added.clone();
        fewer.devices.remove(0);
        assert!(lost_a_key(&added, &fewer));
    }

    /// Recovering with the paper key keeps a member a member, and an
    /// owner's removal of any root they have had keeps them out: only the
    /// recovery key the entry on file named moves membership on.
    #[tokio::test]
    async fn a_recovery_keeps_membership_and_never_undoes_a_removal() {
        use sha2::Digest as _;
        let old = generate();
        let old_id = identity::member_id(&encode_public(&old.verifying_key()));
        let paper =
            ed25519_dalek::SigningKey::from_bytes(&sha2::Sha256::digest(old.to_bytes()).into());
        let next = generate();
        let mut recovered = entry("sarah", &next, &[], 2);
        recovered.recovery = encode_public(&generate().verifying_key());
        recovered.recovered_from = vec![encode_public(&old.verifying_key())];
        let open = |m: crate::Members| {
            Directory::open(scratch("recover").join("keys"), None)
                .unwrap()
                .with_members(Some(m))
        };

        let d = open(crate::Members::list(vec![old_id.clone()]));
        d.admit(sign(entry("sarah", &old, &[], 1), &old).unwrap())
            .await
            .unwrap();
        assert!(d.member("sarah"));
        // a recovery the paper key on file did not sign moves nothing
        let forged = identity::sign_recovery(recovered.clone(), &next, &generate()).unwrap();
        assert!(d.admit(forged).await.is_err());
        let mut stolen = recovered.clone();
        stolen.recovered_from = vec![];
        assert!(
            d.admit(identity::sign_recovery(stolen, &next, &paper).unwrap())
                .await
                .is_err(),
            "the root it replaces must join the earlier ones"
        );
        let good = identity::sign_recovery(recovered.clone(), &next, &paper).unwrap();
        d.admit(good.clone()).await.unwrap();
        assert!(
            d.member("sarah"),
            "still a member, by the root recovered from"
        );
        // kept, the refused ones not: what a client that knows only the old
        // root follows the name here with
        let h = d.handovers("sarah");
        assert_eq!(h.len(), 1);
        identity::follows(&good, &h, &old_id).unwrap();

        // removed by the owner, then recovered: still out
        let d = open(crate::Members {
            members: vec![old_id.clone()],
            revoked: vec![old_id.clone()],
            ..Default::default()
        });
        d.admit(sign(entry("sarah", &old, &[], 1), &old).unwrap())
            .await
            .unwrap();
        assert!(!d.member("sarah"));
        d.admit(good).await.unwrap();
        assert!(!d.member("sarah"), "a recovery escapes no removal");
    }

    /// A sign-up still waiting leaves by its own word, on this box alone:
    /// out and due at once, with no coming back, and then gone, statement and
    /// all, so the name's next holder starts clean.
    #[tokio::test]
    async fn a_waiting_sign_up_leaves_here_and_its_name_comes_free() {
        // its own parent: what is held sits beside the directory
        let dir = scratch("held-leaves").join("keys");
        let d = Directory::open(dir.clone(), None).unwrap();
        let eve = generate();
        d.hold(&sign(entry("eve", &eve, &[], 1), &eve).unwrap())
            .unwrap();
        let say = |back: bool| {
            identity::sign_leave(
                identity::Leave {
                    name: "eve".into(),
                    root: encode_public(&eve.verifying_key()),
                    at: identity::now(),
                    back,
                },
                &eve,
            )
            .unwrap()
        };
        d.say_leave(say(false)).await.unwrap();
        assert!(d.leaving.has_left("eve"));
        assert!(d.entry("eve").unwrap().is_none(), "still not published");
        assert_eq!(crate::leaving::held_leavers(&d).len(), 1, "due at once");
        assert!(
            d.say_leave(say(true)).await.is_err(),
            "nothing to come back to"
        );
        // someone else's word for her is nothing
        let mallory = generate();
        let forged = identity::sign_leave(
            identity::Leave {
                name: "eve".into(),
                root: encode_public(&mallory.verifying_key()),
                at: identity::now(),
                back: true,
            },
            &mallory,
        )
        .unwrap();
        assert!(d.say_leave(forged).await.is_err());
        assert!(d.leaving.has_left("eve"));

        // the hold gone (lapsed, or deleted): still due, and the next person
        // to take the name has not left
        d.unhold("eve");
        assert_eq!(crate::leaving::held_leavers(&d).len(), 1);
        let ann = generate();
        d.hold(&sign(entry("eve", &ann, &[], 1), &ann).unwrap())
            .unwrap();
        assert!(!d.leaving.has_left("eve"));
        assert!(crate::leaving::held_leavers(&d).is_empty());
    }

    /// A sign-up without a code stays here: not served, not listed, not a
    /// claim on the name anywhere else - until it follows through.
    #[tokio::test]
    async fn a_sign_up_without_a_code_is_held_and_not_published() {
        let dir = scratch("held");
        let d = Directory::open(dir.clone(), None).unwrap();
        let eve = generate();
        let held = sign(entry("eve", &eve, &[], 1), &eve).unwrap();

        d.hold(&held).unwrap();
        assert!(d.entry("eve").unwrap().is_none(), "not in the directory");
        assert!(d.held("eve").is_some(), "but held here");
        assert!(
            d.list().unwrap().iter().all(|l| l.name != "eve"),
            "and not listed"
        );
        assert_eq!(d.held_names(), vec!["eve".to_string()]);

        // someone else cannot take the name out from under the hold here
        let mallory = generate();
        let (status, _) = d
            .hold(&sign(entry("eve", &mallory, &[], 1), &mallory).unwrap())
            .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);

        // following through: published, and the hold goes
        d.admit(held).await.unwrap();
        d.unhold("eve");
        assert!(d.entry("eve").unwrap().is_some());
        assert!(d.held("eve").is_none());

        // and a name already in the directory cannot be held at all
        let (status, _) = d
            .hold(&sign(entry("eve", &mallory, &[], 1), &mallory).unwrap())
            .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);

        // one that never follows through is gone after its time
        let late = generate();
        let mut old = entry("zed", &late, &[], 1);
        old.updated = identity::now() - HELD_TTL - 1;
        d.hold(&sign(old, &late).unwrap()).unwrap();
        assert!(d.held("zed").is_none(), "past its time");
        assert!(
            d.held_names().is_empty(),
            "eve followed through and zed expired"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_library_belongs_to_whoever_claimed_it_first() {
        let dir = scratch("claims");
        let d = Directory::open(dir.clone(), None).unwrap();
        let hers = "a996a28ca51c9cf1d3f8e2038c8339c8";
        let his = "b0071e4bd2c34aa19d5e6f7081c2d3e4";

        let sarah = generate();
        d.admit(sign(entry("sarah", &sarah, &[hers], 1), &sarah).unwrap())
            .await
            .unwrap();

        // the whole of C1: tom's entry is validly signed, by tom, about tom,
        // and it names sarah's library. The gate would read it and agree.
        let tom = generate();
        let (status, why) = d
            .admit(sign(entry("tom", &tom, &[hers], 1), &tom).unwrap())
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT, "{why}");
        assert!(why.contains("already sarah's"), "{why}");
        assert!(d.entry("tom").unwrap().is_none(), "nothing of tom's stored");

        // his own id is his, and claiming it does not disturb hers
        d.admit(sign(entry("tom", &tom, &[his], 1), &tom).unwrap())
            .await
            .unwrap();
        // and sarah keeps publishing her own without tripping over herself
        d.admit(sign(entry("sarah", &sarah, &[hers], 2), &sarah).unwrap())
            .await
            .unwrap();
        assert_eq!(d.entry("sarah").unwrap().unwrap().entry.version, 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An entry from before passkeys held no keys (a library sealed to one)
    /// is on file and still read; its next version may not keep that seal,
    /// and the one without it is taken.
    #[tokio::test]
    async fn an_update_that_keeps_a_passkey_seal_is_refused() {
        let dir = scratch("passkey-seal");
        let d = Directory::open(dir.clone(), None).unwrap();
        let sarah = generate();
        let lib = "a996a28ca51c9cf1d3f8e2038c8339c8";
        let sealed_to_passkey = |v| {
            let mut e = entry("sarah", &sarah, &[lib], v);
            e.libraries[0].keys = vec![
                identity::SealedKey {
                    to: "device:f".into(),
                    sealed: "a".into(),
                },
                identity::SealedKey {
                    to: "passkey:p1".into(),
                    sealed: "b".into(),
                },
            ];
            e
        };
        // what an older dd published, as it lies on disk
        let old = sign(sealed_to_passkey(1), &sarah).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sarah.json"), serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(d.entry("sarah").unwrap().unwrap().entry.version, 1);

        let (status, why) = d
            .admit(sign(sealed_to_passkey(2), &sarah).unwrap())
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN, "{why}");
        assert!(why.contains("passkey"), "{why}");
        let mut keyed = sealed_to_passkey(2);
        keyed.libraries[0].keys.clear();
        keyed.passkeys.push(identity::Passkey {
            id: "p1".into(),
            cred: serde_json::json!({}),
            added: 1,
            rp_id: None,
            library_key: Some("a key a page made".into()),
        });
        assert!(d.admit(sign(keyed, &sarah).unwrap()).await.is_err());

        let mut moved = sealed_to_passkey(2);
        identity::drop_passkey_keys(&mut moved);
        d.admit(sign(moved, &sarah).unwrap()).await.unwrap();
        let now = d.entry("sarah").unwrap().unwrap();
        assert_eq!(now.entry.version, 2);
        assert!(!identity::holds_passkey_keys(&now.entry));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pull checks a peer's copy against what it read, then awaits the
    /// network; a newer one landing meanwhile is what the write answers to.
    #[tokio::test]
    async fn a_write_follows_what_landed_while_it_waited() {
        let dir = scratch("race");
        let d = Directory::open(dir.clone(), None).unwrap();
        let sarah = generate();
        d.admit(sign(entry("sarah", &sarah, &[], 1), &sarah).unwrap())
            .await
            .unwrap();

        let read = d.entry("sarah").unwrap();
        let from_peer = sign(entry("sarah", &sarah, &[], 6), &sarah).unwrap();
        identity::accept(read.as_ref(), &from_peer).unwrap();

        // she removes a device while the pull is out on the network
        d.admit(sign(entry("sarah", &sarah, &[], 7), &sarah).unwrap())
            .await
            .unwrap();

        let (status, _) = d.commit(&from_peer, |_| Ok(())).unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(d.entry("sarah").unwrap().unwrap().entry.version, 7);

        let _ = std::fs::remove_dir_all(&dir);
    }
    // ---- the agreed log

    /// a gate in agreed mode, its log fed by hand
    fn agreed_box(what: &str, me: &str) -> Directory {
        let dir = scratch(what);
        Directory::open(dir.join("directory"), None)
            .unwrap()
            .agreed(
                "http://127.0.0.1:1".into(),
                dir.join("submit.sock"),
                vec!["node1".into(), "node2".into()],
                me.into(),
            )
            .unwrap()
    }

    fn feed(d: &Directory, from: &str, item: Item, at: u64) -> (String, Option<String>) {
        let v = serde_json::to_value(&item).unwrap();
        let hash = item_hash(&v);
        let a = d.agreed.as_ref().unwrap();
        let mut r = a.replay.lock().unwrap();
        let ack = d.replay_one(&mut r, from, v, at);
        r.next += 1;
        (hash, ack)
    }

    /// The shadow check (docs/dev/identity-state.md): a log fetched from a
    /// box (`GET /directory?since=0`), replayed through these rules, and
    /// what they decided written out for the control plane's judge to be
    /// held to. SHADOW_LOG, SHADOW_OUT, SHADOW_RELEASE (base64 key),
    /// SHADOW_VOTERS (comma separated). Without SHADOW_LOG it does nothing.
    #[test]
    fn shadow_dump() {
        let Ok(log) = std::env::var("SHADOW_LOG") else {
            return;
        };
        let page: serde_json::Value = serde_json::from_slice(&std::fs::read(log).unwrap()).unwrap();
        let release = std::env::var("SHADOW_RELEASE")
            .ok()
            .map(|k| identity::decode_public(&k).unwrap());
        let voters: Vec<String> = std::env::var("SHADOW_VOTERS")
            .unwrap_or_else(|_| "node1,node2".into())
            .split(',')
            .map(str::to_string)
            .collect();
        let dir = scratch("shadow");
        let d = Directory::open(dir.join("directory"), release)
            .unwrap()
            .agreed(
                "http://127.0.0.1:1".into(),
                dir.join("submit.sock"),
                voters,
                "shadow".into(),
            )
            .unwrap();
        let mut hashes = vec![];
        {
            let a = d.agreed.as_ref().unwrap();
            let mut r = a.replay.lock().unwrap();
            for it in page["items"].as_array().unwrap() {
                let at = (it["time"].as_i64().unwrap() / 1000).max(0) as u64;
                let v = it["item"].clone();
                hashes.push(item_hash(&v));
                d.replay_one(&mut r, it["box"].as_str().unwrap(), v, at);
                r.next += 1;
            }
        }
        let a = d.agreed.as_ref().unwrap();
        let r = a.replay.lock().unwrap();
        let mut entries = serde_json::Map::new();
        for l in d.list().unwrap() {
            let e = d.entry(&l.name).unwrap().unwrap().entry;
            entries.insert(
                l.name.clone(),
                serde_json::json!({
                    "root": e.root, "version": e.version, "recovered_from": e.recovered_from,
                    "devices": e.devices.iter().map(|x| &x.public_key).collect::<Vec<_>>(),
                    "passkeys": e.passkeys.iter().map(|x| &x.id).collect::<Vec<_>>(),
                    "libraries": e.libraries.iter().map(|x| &x.id).collect::<Vec<_>>(),
                    "grant": e.grant.is_some(),
                    "handovers": d.handovers(&l.name).len(),
                }),
            );
        }
        let said: serde_json::Map<String, serde_json::Value> = d
            .leaving
            .said_names()
            .into_iter()
            .filter_map(|n| {
                let s = d.leaving.said(&n)?;
                Some((n, serde_json::json!({"at": s.statement.leave.at, "back": s.statement.leave.back, "agreed": s.agreed})))
            })
            .collect();
        let outcomes: serde_json::Map<String, serde_json::Value> = hashes
            .iter()
            .filter_map(|h| Some((h.clone(), serde_json::json!(r.outcome.get(h)?.is_ok()))))
            .collect();
        let out = serde_json::json!({
            "entries": entries,
            "pending": r.pending.iter().map(|p| p.hash.clone()).collect::<Vec<_>>(),
            "said": said,
            "outcomes": outcomes,
        });
        std::fs::write(
            std::env::var("SHADOW_OUT").unwrap(),
            serde_json::to_string_pretty(&out).unwrap() + "\n",
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn outcome(d: &Directory, hash: &str) -> Option<std::result::Result<(), String>> {
        d.agreed
            .as_ref()
            .unwrap()
            .replay
            .lock()
            .unwrap()
            .outcome
            .get(hash)
            .cloned()
    }

    #[test]
    fn a_new_name_is_its_claimants_only_when_every_voting_box_says_so() {
        let d = agreed_box("agreed-claim", "node1");
        let ann = generate();
        let claim = sign(entry("ann", &ann, &[], 1), &ann).unwrap();
        let (hash, ack) = feed(&d, "node1", Item::Entry(Box::new(claim)), 10);
        assert_eq!(
            ack.as_deref(),
            Some(hash.as_str()),
            "a voter's gate acks a good claim"
        );
        assert!(
            d.agreed_entry("ann", 10).is_none(),
            "not hers on a claim alone"
        );
        feed(&d, "node1", Item::Ack(hash.clone()), 11);
        // the witness, or any box that does not vote, counts for nothing
        feed(&d, "node3", Item::Ack(hash.clone()), 12);
        assert!(
            d.agreed_entry("ann", 12).is_none(),
            "one voter is not every voter"
        );
        feed(&d, "node2", Item::Ack(hash.clone()), 13);
        assert!(
            d.agreed_entry("ann", 13).is_some(),
            "every voter acked: hers"
        );
        assert_eq!(outcome(&d, &hash), Some(Ok(())));
    }

    #[test]
    fn two_claims_to_one_name_go_to_the_first_agreed_and_the_other_is_refused() {
        let d = agreed_box("agreed-race", "node2");
        let (a, b) = (generate(), generate());
        let (first, _) = feed(
            &d,
            "node1",
            Item::Entry(Box::new(sign(entry("sam", &a, &[], 1), &a).unwrap())),
            10,
        );
        let (second, _) = feed(
            &d,
            "node2",
            Item::Entry(Box::new(sign(entry("sam", &b, &[], 1), &b).unwrap())),
            11,
        );
        for v in ["node1", "node2"] {
            feed(&d, v, Item::Ack(first.clone()), 12);
        }
        assert_eq!(outcome(&d, &first), Some(Ok(())));
        assert!(
            outcome(&d, &second).unwrap().is_err(),
            "the later claim lost"
        );
        let held = d.agreed_entry("sam", 13).unwrap();
        assert_eq!(held.entry.root, encode_public(&a.verifying_key()));
        // acks arriving for the loser change nothing
        for v in ["node1", "node2"] {
            feed(&d, v, Item::Ack(second.clone()), 14);
        }
        assert_eq!(
            d.agreed_entry("sam", 15).unwrap().entry.root,
            held.entry.root
        );
    }

    #[test]
    fn an_update_by_its_owner_lands_at_once_and_a_forgery_never() {
        let d = agreed_box("agreed-update", "node1");
        let ann = generate();
        let (hash, _) = feed(
            &d,
            "node1",
            Item::Entry(Box::new(sign(entry("ann", &ann, &[], 1), &ann).unwrap())),
            10,
        );
        for v in ["node1", "node2"] {
            feed(&d, v, Item::Ack(hash.clone()), 11);
        }
        let (up, ack) = feed(
            &d,
            "node2",
            Item::Entry(Box::new(sign(entry("ann", &ann, &[], 2), &ann).unwrap())),
            12,
        );
        assert!(ack.is_none(), "an update is no claim");
        assert_eq!(outcome(&d, &up), Some(Ok(())));
        assert_eq!(d.agreed_entry("ann", 12).unwrap().entry.version, 2);
        let eve = generate();
        let (forged, _) = feed(
            &d,
            "node1",
            Item::Entry(Box::new(sign(entry("ann", &eve, &[], 3), &eve).unwrap())),
            13,
        );
        assert!(outcome(&d, &forged).unwrap().is_err());
        assert_eq!(d.agreed_entry("ann", 13).unwrap().entry.version, 2);
    }

    #[test]
    fn every_box_replaying_the_same_log_has_the_same_directory() {
        let (ann, bob) = (generate(), generate());
        let claim_a = sign(
            entry("ann", &ann, &["a996a28ca51c9cf1d3f8e2038c8339c8"], 1),
            &ann,
        )
        .unwrap();
        let claim_b = sign(
            entry("bob", &bob, &["a996a28ca51c9cf1d3f8e2038c8339c8"], 1),
            &bob,
        )
        .unwrap();
        let ha = item_hash(&serde_json::to_value(Item::Entry(Box::new(claim_a.clone()))).unwrap());
        let hb = item_hash(&serde_json::to_value(Item::Entry(Box::new(claim_b.clone()))).unwrap());
        let log = [
            ("node1", Item::Entry(Box::new(claim_a))),
            ("node2", Item::Entry(Box::new(claim_b))),
            ("node2", Item::Ack(hb.clone())),
            ("node1", Item::Ack(hb)),
            ("node1", Item::Ack(ha.clone())),
            ("node2", Item::Ack(ha)),
        ];
        let boxes = [
            agreed_box("agreed-same-1", "node1"),
            agreed_box("agreed-same-2", "node2"),
        ];
        for d in &boxes {
            for (i, (from, item)) in log.iter().enumerate() {
                let v = serde_json::to_value(item).unwrap();
                feed(d, from, serde_json::from_value(v).unwrap(), 10 + i as u64);
            }
        }
        let names = |d: &Directory| {
            let mut l: Vec<_> = d
                .list()
                .unwrap()
                .into_iter()
                .map(|l| (l.name, l.version))
                .collect();
            l.sort();
            l
        };
        assert_eq!(names(&boxes[0]), names(&boxes[1]));
        // bob's claim was acked first, so the shared library id is his and
        // ann's claim, naming it too, is refused on both
        assert!(boxes[0].agreed_entry("bob", 20).is_some());
        assert!(boxes[0].agreed_entry("ann", 20).is_none());
        assert!(boxes[1].agreed_entry("ann", 20).is_none());
    }
    #[test]
    fn an_entry_from_before_the_log_is_taken_into_it_as_it_was() {
        let d = agreed_box("agreed-migrate", "node1");
        let ann = generate();
        let before = sign(entry("ann", &ann, &[], 7), &ann).unwrap();
        // what this box held before the log, served until the log has it
        let a = d.agreed.as_ref().unwrap();
        std::fs::write(
            a.legacy.join("ann.json"),
            serde_json::to_vec(&before).unwrap(),
        )
        .unwrap();
        assert_eq!(
            d.entry("ann").unwrap().unwrap().entry.version,
            7,
            "served from before the log"
        );
        let (hash, _) = feed(&d, "node1", Item::Entry(Box::new(before)), 10);
        for v in ["node1", "node2"] {
            feed(&d, v, Item::Ack(hash.clone()), 11);
        }
        assert_eq!(
            outcome(&d, &hash),
            Some(Ok(())),
            "the same entry, version 7, taken as it was"
        );
        assert_eq!(d.agreed_entry("ann", 12).unwrap().entry.version, 7);
    }

    // --- who-is-who from the agreed state (ident.rs) ---

    fn agreed_with(
        dir: PathBuf,
        from: crate::ident::From,
        members: serde_json::Value,
    ) -> Directory {
        let d = Directory::open(dir, None)
            .unwrap()
            .identity_from(from, Some("http://127.0.0.1:1"));
        let s: crate::ident::State =
            serde_json::from_value(serde_json::json!({"height": 7, "members": members})).unwrap();
        d.ident.as_ref().unwrap().set(s);
        d
    }

    #[test]
    fn agreed_reads_the_agreed_state_and_nothing_else() {
        let dir = scratch("agreed");
        let k = generate();
        let e = sign(entry("ann", &k, &[], 3), &k).unwrap();
        // a replay file that says otherwise is not read in agreed
        std::fs::create_dir_all(&dir).unwrap();
        let old = sign(entry("ann", &k, &[], 1), &k).unwrap();
        std::fs::write(dir.join("ann.json"), serde_json::to_vec(&old).unwrap()).unwrap();
        let d = agreed_with(
            dir,
            crate::ident::From::Agreed,
            serde_json::json!({
                "ann": {"id": "a", "ids": ["a"], "status": "member", "item": {"entry": e}},
                "eve": {"id": "e", "ids": ["e"], "status": "left", "item": {"entry": e}},
            }),
        );
        assert_eq!(d.entry("ann").unwrap().unwrap().entry.version, 3);
        assert!(d.entry("bob").unwrap().is_none());
        assert!(!d.has_left("ann"));
        assert!(d.has_left("eve"));
        assert!(d.agreed_member("ann").unwrap().is_member());
    }

    #[test]
    fn agreed_knows_nothing_until_it_has_read() {
        let d = Directory::open(scratch("unread"), None)
            .unwrap()
            .identity_from(crate::ident::From::Agreed, Some("http://127.0.0.1:1"));
        assert!(d.entry("ann").unwrap().is_none());
        assert!(!d.has_left("ann"));
    }

    #[test]
    fn a_v2_envelope_reads_as_the_entry_it_carries() {
        use base64::Engine;
        let k = generate();
        let e = entry("ann", &k, &["lib1"], 5);
        let body = serde_json::to_vec(&e).unwrap();
        let m: crate::ident::Member = serde_json::from_value(serde_json::json!({
            "status": "member",
            "item": {
                "v": 2, "kind": "entry",
                "body": base64::engine::general_purpose::STANDARD.encode(&body),
                "sig": "c2ln"
            }
        }))
        .unwrap();
        let got = m.entry().unwrap();
        assert_eq!(got.entry, e);
        assert_eq!(got.signature, "c2ln");
    }

    #[test]
    fn shadow_serves_the_replay_and_names_each_difference() {
        let dir = scratch("shadow");
        std::fs::create_dir_all(&dir).unwrap();
        let k = generate();
        let mine = sign(entry("ann", &k, &[], 1), &k).unwrap();
        std::fs::write(dir.join("ann.json"), serde_json::to_vec(&mine).unwrap()).unwrap();
        let theirs = sign(entry("ann", &k, &[], 2), &k).unwrap();
        let d = agreed_with(
            dir,
            crate::ident::From::Shadow,
            serde_json::json!({"ann": {"status": "member", "item": {"entry": theirs}}}),
        );
        // the replay's answer is what is served
        assert_eq!(d.entry("ann").unwrap().unwrap().entry.version, 1);
        let diff = crate::ident::entry_differences(Some(&mine), Some(&theirs));
        assert!(diff.iter().any(|x| x.starts_with("version")), "{diff:?}");
        assert!(diff.iter().any(|x| x.starts_with("devices")), "{diff:?}");
        // and the same entry both ways differs in nothing
        assert!(crate::ident::entry_differences(Some(&mine), Some(&mine)).is_empty());
    }

    /// A member by a box of theirs (docs/dev/joining.md, "Access"): on no
    /// list, so the replay never counts them, and the agreed state's access
    /// does in agreed - for the directory's reads (may_read), a house's
    /// Wi-Fi (DirHouse) and a client's standing; shadow serves the replay
    /// and names the difference
    #[test]
    fn a_member_by_a_box_counts_wherever_the_agreed_state_is_read() {
        use crate::home::Vouch as _;
        let ann = generate();
        let cat = generate();
        let list = crate::Members::list(vec![identity::member_id(&encode_public(
            &ann.verifying_key(),
        ))]);
        let cat_e = sign(entry("cat", &cat, &[], 1), &cat).unwrap();
        let cat_id = identity::member_id(&cat_e.entry.root);
        let state = |access: &str, status: &str| {
            serde_json::json!({"cat": {"id": cat_id, "ids": [cat_id], "status": status, "access": access,
                "enlisted": [{"box": "j0000000001", "counts": access == "box", "why": "seen"}],
                "item": {"entry": cat_e}}})
        };
        let open = |from, members: serde_json::Value| {
            let dir = scratch(&format!("boxmember-{from:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("cat.json"), serde_json::to_vec(&cat_e).unwrap()).unwrap();
            let d = if from == crate::ident::From::Replay {
                Directory::open(dir, None).unwrap()
            } else {
                agreed_with(dir, from, members)
            };
            Arc::new(d.with_members(Some(list.clone())))
        };
        let house = |d: &Arc<Directory>| crate::home::DirHouse {
            home: crate::home::Home::new(None, &[]),
            directory: d.clone(),
            members: Some(list.clone()),
        };
        // agreed: her box counts
        let d = open(crate::ident::From::Agreed, state("box", "member"));
        assert!(d.member("cat"));
        assert!(house(&d).is_member("cat"));
        let st = d.standing("cat").unwrap();
        assert_eq!(st, (cat_e.entry.root.clone(), true, "box".to_string()));
        // her box gone: signed up, no access
        let d = open(crate::ident::From::Agreed, state("none", "none"));
        assert!(!d.member("cat"));
        assert!(!house(&d).is_member("cat"));
        assert!(!d.standing("cat").unwrap().1);
        // the agreed state not read yet: nobody, not everybody
        let dir = scratch("boxmember-unread");
        let unread = Directory::open(dir, None)
            .unwrap()
            .identity_from(crate::ident::From::Agreed, Some("http://127.0.0.1:1"))
            .with_members(Some(list.clone()));
        assert!(!unread.member("cat"));
        // replay: the list alone, as before
        let d = open(crate::ident::From::Replay, state("box", "member"));
        assert!(!d.member("cat"));
        assert!(!house(&d).is_member("cat"));
        assert!(d.standing("cat").is_none(), "a replaying box says nothing");
        // shadow: the replay's answer, and the difference said once
        let d = open(crate::ident::From::Shadow, state("box", "member"));
        assert!(!d.member("cat"));
        assert!(!house(&d).is_member("cat"));
        assert!(d.standing("cat").is_none(), "shadow goes by the replay");
    }

    /// The shadow log's line says who, which check, both answers and what
    /// the agreed state has of them; the same line is not written again
    /// within SHADOW_AGAIN
    #[test]
    fn the_shadow_log_names_who_the_check_and_both_answers_once() {
        let m: crate::ident::Member = serde_json::from_value(serde_json::json!({
            "status": "member", "access": "box",
            "enlisted": [{"box": "j0000000001", "why": "seen"}, {"box": "j0000000002", "why": "not qualifying"}]
        }))
        .unwrap();
        let said = crate::ident::agreed_said(Some(&m));
        assert_eq!(
            said,
            "agreed status member, access box, boxes: j0000000001 seen; j0000000002 not qualifying"
        );
        assert_eq!(
            crate::ident::agreed_said(None),
            "agreed state: no such name"
        );
        let who = format!("shadowtest{}", std::process::id());
        assert!(crate::ident::shadow_differs(&who, "member", &false, &true, Some(&m)));
        assert!(!crate::ident::shadow_differs(&who, "member", &false, &true, Some(&m)));
        // another check, or another answer, is another line
        assert!(crate::ident::shadow_differs(&who, "registered", &false, &true, Some(&m)));
        assert!(crate::ident::shadow_differs(&who, "member", &true, &false, None));
    }

    #[tokio::test]
    async fn the_agreed_state_is_read_once_then_only_when_it_changed() {
        use axum::{extract::Query, http::StatusCode, routing::get};
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        let app = axum::Router::new().route(
            "/identity",
            get(
                move |Query(q): Query<std::collections::HashMap<String, String>>| {
                    let h = h.clone();
                    async move {
                        h.fetch_add(1, Ordering::Relaxed);
                        if q.get("since").map(String::as_str) == Some("9") {
                            return StatusCode::NOT_MODIFIED.into_response();
                        }
                        axum::Json(serde_json::json!({
                            "height": 9,
                            "members": {"ann": {"status": "member", "item": {}}}
                        }))
                        .into_response()
                    }
                },
            ),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
        let a = crate::ident::Agreed::new(&format!("http://{addr}"));
        assert!(a.state().is_none());
        assert!(a.refresh().await.unwrap());
        assert_eq!(a.state().unwrap().height, 9);
        assert!(a.member("ann").unwrap().is_member());
        // nothing new: 304, the state kept
        assert!(!a.refresh().await.unwrap());
        assert_eq!(a.state().unwrap().height, 9);
        assert_eq!(hits.load(Ordering::Relaxed), 2);
    }

    // --- v2 envelopes, passed through to the control plane ---

    /// a dd-control stand-in on a socket: answers 202, keeps what it got
    fn fake_submit(path: &std::path::Path) -> std::sync::Arc<std::sync::Mutex<Vec<u8>>> {
        use std::io::{BufRead as _, Read as _, Write as _};
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let g = got.clone();
        let _ = std::fs::remove_file(path);
        let l = std::os::unix::net::UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let mut c = c.unwrap();
                let mut r = std::io::BufReader::new(c.try_clone().unwrap());
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                *g.lock().unwrap() = body;
                c.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                    .unwrap();
            }
        });
        got
    }

    fn agreed_dir(
        what: &str,
        from: crate::ident::From,
    ) -> (Directory, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let base = scratch(what);
        std::fs::create_dir_all(&base).unwrap();
        let sock = base.join("submit.sock");
        let got = fake_submit(&sock);
        let d = Directory::open(base.join("entries"), None)
            .unwrap()
            .agreed(
                "http://127.0.0.1:1".into(),
                sock,
                vec!["box".into()],
                "box".into(),
            )
            .unwrap()
            .identity_from(from, Some("http://127.0.0.1:1"));
        (d, got)
    }

    async fn put_v2(d: &Arc<Directory>, name: &str, source: &str, bytes: Vec<u8>) -> StatusCode {
        let mut h = HeaderMap::new();
        h.insert("x-dd-peer", source.parse().unwrap());
        put_entry(
            State(d.clone()),
            Path(name.into()),
            h,
            axum::body::Body::from(bytes),
        )
        .await
        .status()
    }

    fn sealed(e: &Entry, k: &ed25519_dalek::SigningKey) -> Vec<u8> {
        serde_json::to_vec(&identity::envelope::seal_json("entry", e, k).unwrap()).unwrap()
    }

    fn leave_by(name: &str, k: &ed25519_dalek::SigningKey) -> Vec<u8> {
        serde_json::to_vec(
            &identity::envelope::seal_json("leave", &serde_json::json!({"name": name, "at": 1}), k)
                .unwrap(),
        )
        .unwrap()
    }

    type Got = std::sync::Arc<std::sync::Mutex<Vec<u8>>>;

    /// a box reading the agreed state, where `ann` is on file with key `k`
    fn with_ann(what: &str) -> (Arc<Directory>, Got, ed25519_dalek::SigningKey) {
        let (d, got) = agreed_dir(what, crate::ident::From::Agreed);
        let k = generate();
        let e = sign(entry("ann", &k, &[], 1), &k).unwrap();
        let s: crate::ident::State = serde_json::from_value(serde_json::json!({
            "height": 1,
            "members": {"ann": {"status": "member", "item": {"entry": e}}}
        }))
        .unwrap();
        d.ident.as_ref().unwrap().set(s);
        (Arc::new(d), got, k)
    }

    // --- 1. one limiter, by visitor, for every PUT, before anything else

    #[tokio::test]
    async fn one_limiter_counts_every_put_from_a_visitor_v1_and_v2() {
        let (d, _got, k) = with_ann("one-limiter");
        let mut last = StatusCode::OK;
        for n in 0..=PUTS_PER_SOURCE {
            // v1 garbage and v2 envelopes alike spend the same allowance
            let b = if n % 2 == 0 {
                b"not json".to_vec()
            } else {
                sealed(&entry("ann", &k, &[], n as u64 + 2), &k)
            };
            last = put_v2(&d, "ann", "198.51.100.1", b).await;
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
        // another visitor is not touched by it
        let k2 = generate();
        let b = sealed(&entry("bea", &k2, &[], 1), &k2);
        assert_eq!(
            put_v2(&d, "bea", "198.51.100.2", b).await,
            StatusCode::ACCEPTED
        );
        // a header the client writes is not who they are
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", "203.0.113.1".parse().unwrap());
        h.insert("x-forwarded-for", "203.0.113.2".parse().unwrap());
        assert_eq!(source_of(&h), "local");
    }

    #[tokio::test]
    async fn over_the_limit_the_body_is_never_read_and_a_big_one_never_whole() {
        let (d, got, _k) = with_ann("before-reading");
        for _ in 0..PUTS_PER_SOURCE {
            put_v2(&d, "ann", "198.51.100.3", b"{}".to_vec()).await;
        }
        // a body that would fail to stream: refused at the limiter, unread
        let mut h = HeaderMap::new();
        h.insert("x-dd-peer", "198.51.100.3".parse().unwrap());
        let poisoned = axum::body::Body::from_stream(futures_util::stream::once(async {
            Err::<axum::body::Bytes, std::io::Error>(std::io::Error::other("read"))
        }));
        let res = put_entry(State(d.clone()), Path("ann".into()), h, poisoned).await;
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        // within the allowance, a body past the most is cut off, not kept
        let big = vec![b' '; ENVELOPE_MAX + 1];
        assert_eq!(
            put_v2(&d, "ann", "198.51.100.4", big).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert!(got.lock().unwrap().is_empty());
    }

    // --- 2. per name, only after a verified signature

    #[tokio::test]
    async fn forgeries_in_a_members_name_spend_nothing_of_theirs() {
        let (d, got, k) = with_ann("name-after-verify");
        let eve = generate();
        for n in 0..(ENVELOPES_PER_NAME * 2) {
            let forged = sealed(&entry("ann", &eve, &[], 50 + n as u64), &eve);
            assert_eq!(
                put_v2(&d, "ann", &format!("198.51.100.{n}"), forged).await,
                StatusCode::FORBIDDEN
            );
        }
        assert!(got.lock().unwrap().is_empty());
        assert_eq!(
            put_v2(
                &d,
                "ann",
                "203.0.113.9",
                sealed(&entry("ann", &k, &[], 2), &k)
            )
            .await,
            StatusCode::ACCEPTED
        );
        // and her own allowance is hers: past it, she waits
        let mut last = StatusCode::ACCEPTED;
        for v in 3..(3 + ENVELOPES_PER_NAME as u64) {
            last = put_v2(
                &d,
                "ann",
                &format!("203.0.113.{v}"),
                sealed(&entry("ann", &k, &[], v), &k),
            )
            .await;
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn unsigned_forged_or_someone_elses_never_reach_the_log() {
        let (d, got, k) = with_ann("signed-only");
        let mut env =
            identity::envelope::seal_json("entry", &entry("ann", &k, &[], 2), &k).unwrap();
        env.sig = "c2ln".into();
        assert_eq!(
            put_v2(&d, "ann", "a", serde_json::to_vec(&env).unwrap()).await,
            StatusCode::FORBIDDEN
        );
        env.sig = String::new();
        assert_eq!(
            put_v2(&d, "ann", "a", serde_json::to_vec(&env).unwrap()).await,
            StatusCode::BAD_REQUEST
        );
        let eve = generate();
        assert_eq!(
            put_v2(&d, "ann", "a", sealed(&entry("ann", &eve, &[], 2), &eve)).await,
            StatusCode::FORBIDDEN
        );
        let ctx = Arc::new(crate::leaving::Ctx {
            d: d.clone(),
            m: None,
            demo: None,
        });
        let res = crate::leaving::put_for_test(ctx, "ann", leave_by("ann", &eve)).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert!(got.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn signed_items_go_to_the_control_plane_as_they_came() {
        let (d, got, k) = with_ann("as-it-came");
        let e = sealed(&entry("ann", &k, &[], 2), &k);
        assert_eq!(
            put_v2(&d, "ann", "a", e.clone()).await,
            StatusCode::ACCEPTED
        );
        assert_eq!(*got.lock().unwrap(), e);
        let k2 = generate();
        let s = sealed(&entry("bea", &k2, &[], 1), &k2);
        assert_eq!(
            put_v2(&d, "bea", "b", s.clone()).await,
            StatusCode::ACCEPTED
        );
        assert_eq!(*got.lock().unwrap(), s);
        let l = leave_by("ann", &k);
        let ctx = Arc::new(crate::leaving::Ctx {
            d: d.clone(),
            m: None,
            demo: None,
        });
        assert_eq!(
            crate::leaving::put_for_test(ctx, "ann", l.clone())
                .await
                .status(),
            StatusCode::ACCEPTED
        );
        assert_eq!(*got.lock().unwrap(), l);
    }

    // --- 3. the sent list never fails open, and never drifts from its file

    #[tokio::test]
    async fn the_same_item_reworded_or_after_a_restart_is_sent_once() {
        let (d, _got, k) = with_ann("sent-once");
        let env = identity::envelope::seal_json("entry", &entry("ann", &k, &[], 2), &k).unwrap();
        assert_eq!(
            put_v2(&d, "ann", "a", serde_json::to_vec(&env).unwrap()).await,
            StatusCode::ACCEPTED
        );
        let mut v = serde_json::to_value(&env).unwrap();
        v["extra"] = serde_json::json!("x");
        let reworded = format!("  {}", serde_json::to_string(&v).unwrap()).into_bytes();
        assert_eq!(put_v2(&d, "ann", "b", reworded).await, StatusCode::CONFLICT);
        let base = std::env::temp_dir().join(format!("dd-dir-sent-once-{}", std::process::id()));
        let file = base.join("envelopes-sent.json");
        let on_disk: Vec<(String, u64)> =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(on_disk.len(), 1);
        let again = Sent::load(file);
        assert!(again.has(&on_disk[0].0, identity::now()).unwrap());
    }

    #[tokio::test]
    async fn an_unreadable_sent_list_refuses_every_v2_put() {
        let base = scratch("sent-corrupt");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("envelopes-sent.json"), b"{not a list").unwrap();
        let sock = base.join("submit.sock");
        let got = fake_submit(&sock);
        let d = Directory::open(base.join("entries"), None)
            .unwrap()
            .agreed(
                "http://127.0.0.1:1".into(),
                sock,
                vec!["box".into()],
                "box".into(),
            )
            .unwrap()
            .identity_from(crate::ident::From::Shadow, Some("http://127.0.0.1:1"));
        let d = Arc::new(d);
        let k = generate();
        assert_eq!(
            put_v2(&d, "bea", "a", sealed(&entry("bea", &k, &[], 1), &k)).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(got.lock().unwrap().is_empty());
    }

    #[test]
    fn a_sent_list_that_cannot_be_written_keeps_memory_as_the_file_is() {
        let base = scratch("sent-unwritable");
        std::fs::create_dir_all(&base).unwrap();
        let mut s = Sent::load(base.join("missing-dir").join("sent.json"));
        assert!(s.add("one".into(), 10).is_err());
        assert!(s.items.is_empty());
        // and from then on it says so, rather than letting anything through
        assert!(s.has("one", 10).is_err());
    }

    #[test]
    fn a_sent_item_is_kept_a_day_whatever_comes_after() {
        let mut s = Sent::default();
        let t = 1_000_000;
        s.add("real".into(), t).unwrap();
        for n in 0..10_000 {
            s.add(format!("junk{n}"), t + 60).unwrap();
        }
        assert!(s.has("real", t + 3600).unwrap());
        assert!(!s.has("real", t + SENT_SECS + 1).unwrap());
    }

    #[test]
    fn a_window_stays_bounded_and_never_shuts_a_newcomer_out() {
        let now = std::time::Instant::now();
        let mut w = Window::default();
        for n in 0..WINDOW_MAX {
            let at = now - Duration::from_secs(30) + Duration::from_millis(n as u64);
            w.0.insert(format!("k{n}"), std::collections::VecDeque::from([at]));
        }
        assert!(w.take("new", 1, now));
        assert_eq!(w.0.len(), WINDOW_MAX);
        assert!(!w.0.contains_key("k0"));
    }

    // --- remote computers' items (computers.rs): the same road in

    /// a box reading the agreed state, `ann` on file with root `k` and
    /// device `dev`, signed in as whoever x-test-user says
    fn computers_for_ann(
        what: &str,
    ) -> (
        Arc<Directory>,
        Got,
        ed25519_dalek::SigningKey,
        ed25519_dalek::SigningKey,
    ) {
        let (d, got) = agreed_dir(what, crate::ident::From::Agreed);
        let k = generate();
        let dev = generate();
        let mut e = entry("ann", &k, &[], 1);
        let p = encode_public(&dev.verifying_key());
        e.devices = vec![Device {
            fingerprint: fingerprint(&p),
            public_key: p,
            added: 1,
        }];
        let e = sign(e, &k).unwrap();
        let s: crate::ident::State = serde_json::from_value(serde_json::json!({
            "height": 1,
            "members": {"ann": {"status": "member", "item": {"entry": e}}}
        }))
        .unwrap();
        d.ident.as_ref().unwrap().set(s);
        d.set_caller(Box::new(|h: &HeaderMap| {
            h.get("x-test-user")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        }));
        (Arc::new(d), got, k, dev)
    }

    fn computer_item(
        kind: &str,
        body: serde_json::Value,
        k: &ed25519_dalek::SigningKey,
    ) -> Vec<u8> {
        serde_json::to_vec(&identity::envelope::seal_json(kind, &body, k).unwrap()).unwrap()
    }

    async fn put_computer(
        d: &Arc<Directory>,
        kind: &str,
        user: Option<&str>,
        source: &str,
        bytes: Vec<u8>,
    ) -> StatusCode {
        let mut h = HeaderMap::new();
        h.insert("x-dd-peer", source.parse().unwrap());
        if let Some(u) = user {
            h.insert("x-test-user", u.parse().unwrap());
        }
        crate::computers::put(
            State(d.clone()),
            Path(kind.into()),
            h,
            axum::body::Body::from(bytes),
        )
        .await
        .status()
    }

    fn a_computer(owner: &str, version: u64) -> serde_json::Value {
        serde_json::json!({"owner": owner, "id": "00000000000000a1", "name": "dev", "config": "default",
            "memory": 1024, "threads": 1, "disk": 8, "nix": "", "ports": [], "players": [],
            "consent": "any", "anywhere": true, "version": version})
    }

    #[tokio::test]
    async fn a_computer_goes_in_signed_by_its_owner_signed_in() {
        let (d, got, k, _) = computers_for_ann("computers-owner");
        let item = computer_item("computer", a_computer("ann", 1), &k);
        // nobody signed in
        assert_eq!(
            put_computer(&d, "computer", None, "a", item.clone()).await,
            StatusCode::UNAUTHORIZED
        );
        // someone else signed in
        assert_eq!(
            put_computer(&d, "computer", Some("bob"), "a", item.clone()).await,
            StatusCode::FORBIDDEN
        );
        assert!(got.lock().unwrap().is_empty());
        // the owner: through, as it came
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "a", item.clone()).await,
            StatusCode::ACCEPTED
        );
        assert_eq!(*got.lock().unwrap(), item);
        // the same again: refused by the shared list
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "b", item).await,
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn a_forgery_never_reaches_the_log() {
        let (d, got, _, dev) = computers_for_ann("computers-forged");
        // a device may not make a computer, only start or stop one
        let by_device = computer_item("computer", a_computer("ann", 1), &dev);
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "a", by_device).await,
            StatusCode::FORBIDDEN
        );
        let stranger = computer_item("computer", a_computer("ann", 2), &generate());
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "a", stranger).await,
            StatusCode::FORBIDDEN
        );
        assert!(got.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_start_or_stop_is_checked_against_the_owners_devices_now() {
        let (d, got, _, dev) = computers_for_ann("computers-power");
        let power = |k: &ed25519_dalek::SigningKey, at: u64| {
            computer_item(
                "computer-power",
                serde_json::json!({"id": "00000000000000a1", "up": false, "at": at}),
                k,
            )
        };
        // the computers not read yet: nothing goes through
        assert_eq!(
            put_computer(&d, "computer-power", Some("ann"), "a", power(&dev, 1)).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        d.computers.as_ref().unwrap().set(
            std::collections::HashMap::from([("00000000000000a1".to_string(), "ann".to_string())]),
            &[],
        );
        // a key that is no device of hers
        assert_eq!(
            put_computer(
                &d,
                "computer-power",
                Some("ann"),
                "a",
                power(&generate(), 2)
            )
            .await,
            StatusCode::FORBIDDEN
        );
        // someone else signed in, with her device's signature
        assert_eq!(
            put_computer(&d, "computer-power", Some("bob"), "a", power(&dev, 3)).await,
            StatusCode::FORBIDDEN
        );
        assert!(got.lock().unwrap().is_empty());
        // her device: through
        assert_eq!(
            put_computer(&d, "computer-power", Some("ann"), "a", power(&dev, 4)).await,
            StatusCode::ACCEPTED
        );
        // a computer nobody has
        let other = computer_item(
            "computer-power",
            serde_json::json!({"id": "00000000000000ff", "up": true, "at": 5}),
            &dev,
        );
        assert_eq!(
            put_computer(&d, "computer-power", Some("ann"), "a", other).await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn only_these_kinds_and_each_where_it_goes() {
        let (d, got, k, _) = computers_for_ann("computers-kinds");
        let item = computer_item("computer", a_computer("ann", 1), &k);
        assert_eq!(
            put_computer(&d, "entry", Some("ann"), "a", item.clone()).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            put_computer(&d, "computer-delete", Some("ann"), "a", item).await,
            StatusCode::BAD_REQUEST
        );
        assert!(got.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_body_is_bounded_and_the_name_allowance_counts() {
        let (d, _got, k, _) = computers_for_ann("computers-limits");
        let big = vec![b'x'; ENVELOPE_MAX + 1];
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "a", big).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let mut last = StatusCode::OK;
        for v in 1..=(ENVELOPES_PER_NAME as u64 + 1) {
            // each from its own visitor: the name's allowance, not the visitor's
            let item = computer_item("computer", a_computer("ann", v), &k);
            last = put_computer(&d, "computer", Some("ann"), &format!("v{v}"), item).await;
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn a_config_and_a_delete_go_in_the_same_way() {
        let (d, got, k, dev) = computers_for_ann("computers-config-delete");
        let config = serde_json::json!({"owner": "ann", "name": "big", "memory": 1024, "threads": 1,
            "disk": 8, "nix": "", "ports": [], "players": [], "consent": "any", "version": 1});
        let item = computer_item("computer-config", config.clone(), &k);
        assert_eq!(
            put_computer(&d, "computer-config", Some("bob"), "a", item.clone()).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            put_computer(&d, "computer-config", Some("ann"), "a", item.clone()).await,
            StatusCode::ACCEPTED
        );
        assert_eq!(*got.lock().unwrap(), item);
        let del = serde_json::json!({"owner": "ann", "id": "00000000000000a1", "at": 1});
        // a delete takes the root: a device of hers may not
        assert_eq!(
            put_computer(
                &d,
                "computer-delete",
                Some("ann"),
                "a",
                computer_item("computer-delete", del.clone(), &dev)
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            put_computer(
                &d,
                "computer-delete",
                Some("ann"),
                "a",
                computer_item("computer-delete", del, &k)
            )
            .await,
            StatusCode::ACCEPTED
        );
    }

    #[tokio::test]
    async fn nothing_goes_in_before_the_agreed_state_is_read() {
        let (d, got) = agreed_dir("computers-unread", crate::ident::From::Agreed);
        d.set_caller(Box::new(|h: &HeaderMap| {
            h.get("x-test-user")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        }));
        let d = Arc::new(d);
        let item = computer_item("computer", a_computer("ann", 1), &generate());
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "a", item).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(got.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_item_judged_long_ago_is_not_sent_again() {
        let (d, got, k, _) = computers_for_ann("computers-judged");
        let item = computer_item("computer", a_computer("ann", 1), &k);
        // the sent list forgot it (a day on); the judge's outcomes have not
        let hash = identity::item_hash(&serde_json::from_slice(&item).unwrap());
        d.computers
            .as_ref()
            .unwrap()
            .set(Default::default(), &[hash.as_str()]);
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "a", item).await,
            StatusCode::CONFLICT
        );
        assert!(got.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_recovery_signature_is_refused() {
        let (d, got, k, _) = computers_for_ann("computers-recovery-sig");
        let mut env = identity::envelope::seal_json("computer", &a_computer("ann", 1), &k).unwrap();
        env.recovery_sig = Some(env.sig.clone());
        let item = serde_json::to_vec(&env).unwrap();
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "a", item).await,
            StatusCode::FORBIDDEN
        );
        assert!(got.lock().unwrap().is_empty());
    }

    /// the largest item the judge takes (a 44 KiB Nix file) fits under the
    /// gate's 64 KiB, its body as base64
    #[tokio::test]
    async fn the_largest_computer_fits_the_gate() {
        let (d, _got, k, _) = computers_for_ann("computers-largest");
        let mut c = a_computer("ann", 1);
        c["nix"] = serde_json::Value::String("x".repeat(44 * 1024));
        let item = computer_item("computer", c, &k);
        assert!(item.len() <= ENVELOPE_MAX, "{}", item.len());
        assert_eq!(
            put_computer(&d, "computer", Some("ann"), "a", item).await,
            StatusCode::ACCEPTED
        );
    }

    #[tokio::test]
    async fn a_box_that_replays_takes_no_v2() {
        let (d, got) = agreed_dir("v2-replay", crate::ident::From::Replay);
        assert!(!d.takes_v2());
        let d = Arc::new(d);
        let k = generate();
        assert_eq!(
            put_v2(&d, "ann", "a", sealed(&entry("ann", &k, &[], 1), &k)).await,
            StatusCode::BAD_REQUEST
        );
        assert!(got.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_item_the_agreed_log_judged_is_not_sent_again() {
        let (d, got, k) = with_ann("in-agreed-log");
        let b = sealed(&entry("ann", &k, &[], 2), &k);
        let hash = identity::item_hash(&serde_json::from_slice(&b).unwrap());
        let s: crate::ident::State = serde_json::from_value(serde_json::json!({
            "height": 2,
            "members": {"ann": {"status": "member", "item": {"entry": sign(entry("ann", &k, &[], 1), &k).unwrap()}}},
            "outcomes": {hash: "ok"}
        }))
        .unwrap();
        d.ident.as_ref().unwrap().set(s);
        assert_eq!(put_v2(&d, "ann", "a", b).await, StatusCode::CONFLICT);
        assert!(got.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn not_yet_read_the_agreed_log_blocks_nothing() {
        let (d, got) = agreed_dir("agreed-unread", crate::ident::From::Shadow);
        let d = Arc::new(d);
        let k = generate();
        let b = sealed(&entry("bea", &k, &[], 1), &k);
        assert_eq!(
            put_v2(&d, "bea", "a", b.clone()).await,
            StatusCode::ACCEPTED
        );
        assert_eq!(*got.lock().unwrap(), b);
    }

    #[tokio::test]
    async fn one_key_signing_for_many_names_has_one_allowance() {
        // one key may be the root of several names: its allowance spans them
        let (d, _got, _k) = with_ann("per-key");
        let k = generate();
        let mut last = StatusCode::ACCEPTED;
        for n in 0..=ENVELOPES_PER_KEY {
            let name = format!("same{n}");
            last = put_v2(
                &d,
                &name,
                &format!("198.51.100.{n}"),
                sealed(&entry(&name, &k, &[], 1), &k),
            )
            .await;
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    }
}
