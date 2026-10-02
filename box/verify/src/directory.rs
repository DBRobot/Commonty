//! The directory: a person's signed entry, stored and served by any box.
//!
//! No credential admits an update. What admits it is a signature by the key
//! already on file, so a box running only this has nothing to lose but the
//! copies, and a box that wants to lie can only withhold or replay.
//!
//! Boxes share it. Each pulls its peers on a timer and runs what arrives
//! through the same rule a client's PUT gets, so a peer cannot push a
//! forgery either. The one moment the rule has nothing to check against is a
//! name this box has never seen, and that is where the peers come in: a
//! first sight from the network is taken only once this box has pulled every
//! peer at least once, and only if no peer knows the name under another root.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
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
    /// the release key: what signs an invite. None: grants cannot be
    /// checked here and are refused
    release: Option<ed25519_dalek::VerifyingKey>,
    /// the other boxes' directory urls, e.g. https://files.example/_dd/directory
    peers: Vec<String>,
    http: reqwest::Client,
    /// every peer answered a full pull at least once; until then no first
    /// sight from the network is taken
    synced: AtomicBool,
    /// held by every write of an entry, from its last check to its rename
    writing: std::sync::Mutex<()>,
    /// where a member's name is dropped when their sessions in the other
    /// services on this box are to end (VERIFY_ENDED): a device or passkey
    /// removed, or "sign out everywhere else". Passwords reads it
    /// (modules/vault/vaultwarden.nix). None: nothing else to tell.
    ended: Option<PathBuf>,
    /// entries as last read, by name, with the file's time and size then:
    /// every request asks whose it is, and the file rarely changes
    read: std::sync::Mutex<
        std::collections::HashMap<String, (std::time::SystemTime, u64, identity::SignedEntry)>,
    >,
}

#[derive(Serialize, Deserialize)]
pub struct Listed {
    pub name: String,
    pub version: u64,
}

impl Directory {
    pub fn open(
        dir: PathBuf,
        peers: Vec<String>,
        release: Option<ed25519_dalek::VerifyingKey>,
    ) -> Result<Self> {
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
        let synced = AtomicBool::new(peers.is_empty());
        Ok(Self {
            ended: std::env::var_os("VERIFY_ENDED").map(PathBuf::from),
            read: Default::default(),
            dir,
            invites,
            held,
            release,
            peers,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            synced,
            writing: std::sync::Mutex::new(()),
        })
    }

    /// The person's own signed entry, as last accepted by this box.
    pub fn entry(&self, name: &str) -> Result<Option<identity::SignedEntry>> {
        // the name is a path component here, and this is reached from a PUT
        // before the accept rule has had a look at it
        if !identity::valid_name(name) {
            return Ok(None);
        }
        let p = self.dir.join(format!("{name}.json"));
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
        let current = self.entry(&signed.entry.name).map_err(|e| {
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
    /// It becomes an entry when it earns one: a code redeemed, or the member
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
        let taken = || (StatusCode::CONFLICT, "that name is taken".to_string());
        if self.entry(name).ok().flatten().is_some() {
            return Err(taken());
        }
        if self
            .held(name)
            .is_some_and(|h| h.entry.root != signed.entry.root)
        {
            return Err(taken());
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

    /// A first sight from the network: may this box take it? Every peer has
    /// to answer, and none may hold the name under a different root. A peer
    /// that holds it under the same root is fine - the client publishes to
    /// all of them in turn, so that is the normal case.
    async fn first_sight_allowed(
        &self,
        new: &identity::SignedEntry,
    ) -> std::result::Result<(), (StatusCode, String)> {
        if !self.synced.load(Ordering::Relaxed) {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "this box has not yet pulled every peer; new names wait".into(),
            ));
        }
        for p in &self.peers {
            let url = format!("{}/{}", p.trim_end_matches('/'), new.entry.name);
            let r = self.http.get(&url).send().await;
            match r {
                Ok(r) if r.status() == StatusCode::NOT_FOUND => {}
                Ok(r) if r.status().is_success() => {
                    let theirs: identity::SignedEntry = r
                        .json()
                        .await
                        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("peer {p}: {e}")))?;
                    if theirs.entry.root != new.entry.root {
                        return Err((
                            StatusCode::CONFLICT,
                            format!("{} is already taken (known to {p})", new.entry.name),
                        ));
                    }
                }
                Ok(r) => {
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!("peer {p} answered {}", r.status()),
                    ));
                }
                Err(e) => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        format!("cannot confirm the name is free: peer {p} unreachable ({e})"),
                    ));
                }
            }
        }
        Ok(())
    }

    /// One pull of one peer: everything it has that is newer than ours,
    /// through the accept rule.
    async fn pull(&self, peer: &str) -> Result<usize> {
        let base = peer.trim_end_matches('/');
        let listed: Vec<Listed> = self
            .http
            .get(base)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("listing")?;
        let mut taken = 0;
        for l in listed {
            if !identity::valid_name(&l.name) {
                continue;
            }
            let ours = self.entry(&l.name)?;
            if ours.as_ref().is_some_and(|o| o.entry.version >= l.version) {
                continue;
            }
            let theirs: identity::SignedEntry = self
                .http
                .get(format!("{base}/{}", l.name))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if theirs.entry.name != l.name || guest_expired(&theirs.entry) {
                continue;
            }
            if let Some(g) = &theirs.entry.grant {
                let ok = self
                    .release
                    .as_ref()
                    .is_some_and(|r| identity::verify_grant(&theirs.entry, r).is_ok())
                    && !self.grant_taken(&g.invite.invite.public_key, &theirs.entry.root);
                if !ok {
                    eprintln!("directory: refused {} from {peer}: grant", l.name);
                    continue;
                }
            }
            if let Err(e) = self.claims_free(&theirs) {
                eprintln!("directory: refused {} from {peer}: {e}", l.name);
                continue;
            }
            // A name this box has never seen, on one peer's word: the same
            // agreement a sign-up gets - every peer answers, and none holds
            // the name under another root. Without it a peer that lies can
            // seed a new box with `david` under its own root before an
            // honest peer is asked, and the accept rule then refuses the
            // real one for good, because by then it is a different root.
            if ours.is_none()
                && let Err((_, why)) = self.first_sight_allowed(&theirs).await
            {
                eprintln!("directory: not yet taking {} from {peer}: {why}", l.name);
                continue;
            }
            match identity::accept(ours.as_ref(), &theirs)
                .map_err(|e| e.to_string())
                .and_then(|()| self.commit(&theirs, |_| Ok(())).map_err(|(_, why)| why))
            {
                Ok(()) => {
                    taken += 1;
                    eprintln!(
                        "directory: {} version {} from {peer}",
                        theirs.entry.name, theirs.entry.version
                    );
                }
                Err(e) => eprintln!("directory: refused {} from {peer}: {e}", l.name),
            }
        }
        Ok(taken)
    }

    /// Forever: pull every peer, every `every` seconds. The first time every
    /// peer has answered, the box starts taking new names from the network.
    pub async fn sync_forever(self: Arc<Self>, every: u64) {
        if self.peers.is_empty() {
            return;
        }
        let mut ok = vec![false; self.peers.len()];
        loop {
            for (i, p) in self.peers.iter().enumerate() {
                match self.pull(p).await {
                    Ok(_) => ok[i] = true,
                    Err(e) => eprintln!("directory: pull from {p} failed: {e:#}"),
                }
            }
            if ok.iter().all(|b| *b) && !self.synced.swap(true, Ordering::Relaxed) {
                eprintln!("directory: every peer pulled once; new names accepted from now");
                // the names this round had to leave, now that they can be
                // checked: at once, not a whole interval later
                continue;
            }
            tokio::time::sleep(Duration::from_secs(every)).await;
        }
    }
}

pub fn router(d: Arc<Directory>) -> Router {
    Router::new()
        .route("/_dd/directory", get(list))
        .route("/_dd/directory/{name}", get(get_entry).put(put_entry))
        .route("/_dd/invite/{key}", get(get_invite).put(put_invite))
        .with_state(d)
}

async fn list(State(d): State<Arc<Directory>>) -> Response {
    match d.list() {
        Ok(l) => Json(l).into_response(),
        Err(e) => {
            eprintln!("directory: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Public: an entry holds only public keys.
async fn get_entry(State(d): State<Arc<Directory>>, Path(name): Path<String>) -> Response {
    if !identity::valid_name(&name) {
        return StatusCode::BAD_REQUEST.into_response();
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
    match d.put_invite(&inv) {
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
    match d.admit(signed).await {
        Ok(()) => Json(serde_json::json!({ "accepted": true, "version": version })).into_response(),
        Err((status, why)) => (status, why).into_response(),
    }
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
            if g.invite.invite.expires <= identity::now() {
                return Err("that invite has expired".into());
            }
            if self.grant_taken(&g.invite.invite.public_key, &new.root) {
                return Err("that code has already been used".into());
            }
        }
        Ok(())
    }

    /// The one way in: the accept rule, then the peers for a new name,
    /// then the store. Used by the PUT from `dd` and by the browser join.
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
        if let Err(e) = self.check_grant(existing.as_ref(), &signed.entry) {
            eprintln!("directory: refused update for {name}: {e}");
            return Err((StatusCode::CONFLICT, e));
        }
        if let Err(e) = self.claims_free(&signed) {
            eprintln!("directory: refused update for {name}: {e}");
            return Err((StatusCode::CONFLICT, e));
        }
        if existing.is_none()
            && let Err((status, why)) = self.first_sight_allowed(&signed).await
        {
            eprintln!("directory: refused new name {name}: {why}");
            return Err((status, why));
        }
        self.commit(&signed, |current| self.check_grant(current, &signed.entry))
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
            version,
            updated: identity::now(),
        }
    }

    #[test]
    fn an_entry_changed_on_disk_is_read_again() {
        let dir = scratch("cache");
        let d = Directory::open(dir.clone(), vec![], None).unwrap();
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

    /// A sign-up without a code stays here: not served, not listed, not a
    /// claim on the name anywhere else - until it follows through.
    #[tokio::test]
    async fn a_sign_up_without_a_code_is_held_and_not_published() {
        let dir = scratch("held");
        let d = Directory::open(dir.clone(), vec![], None).unwrap();
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
        let d = Directory::open(dir.clone(), vec![], None).unwrap();
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

    /// A pull checks a peer's copy against what it read, then awaits the
    /// network; a newer one landing meanwhile is what the write answers to.
    #[tokio::test]
    async fn a_write_follows_what_landed_while_it_waited() {
        let dir = scratch("race");
        let d = Directory::open(dir.clone(), vec![], None).unwrap();
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
}
