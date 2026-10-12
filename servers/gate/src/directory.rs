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

    /// The person's own signed entry, as last accepted by this box.
    pub fn entry(&self, name: &str) -> Result<Option<identity::SignedEntry>> {
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
        let mut all = self.handovers(&after.entry.name);
        all.push(identity::Handover {
            before: before.clone(),
            after: after.clone(),
        });
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
        if let Some((at, seen)) = &*self.member_list_seen.lock().unwrap() {
            let fresh = if seen.is_some() { 600 } else { 60 };
            if at.elapsed() < Duration::from_secs(fresh) {
                return seen.clone();
            }
        }
        let read = async {
            let raw = self
                .http
                .get(url)
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .text()
                .await
                .ok()?;
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

    /// On the release's list (or let in by a code the owner signed), and not
    /// revoked. With no list, anyone with an entry.
    pub fn member(&self, name: &str) -> bool {
        let Some(e) = self.entry(name).ok().flatten() else {
            return false;
        };
        let Some(m) = &self.members else {
            return true;
        };
        identity::is_member(&e.entry, &m.members, &m.revoked, self.release())
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
        .route("/_dd/directory/{name}/recoveries", get(get_recoveries))
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

async fn put_entry(
    State(d): State<Arc<Directory>>,
    Path(name): Path<String>,
    Json(signed): Json<identity::SignedEntry>,
) -> Response {
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

/// an item's name in the log: its canonical json's sha256, the same
/// whichever way the control plane escaped it
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
        let a = self.agreed.as_ref().expect("agreed mode");
        let sock = a.submit.clone();
        let body = serde_json::to_vec(v)?;
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
        identity::anchored(&good, &h, std::slice::from_ref(&old_id), &[], None).unwrap();

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
}
