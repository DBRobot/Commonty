//! Leaving Commonty: a member's own signed word or the owner's removal, the
//! same answer on every box, and what goes when (docs/dev/leaving.md).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use serde::{Deserialize, Serialize};
use webauthn_rs::prelude::PublicKeyCredential;

use crate::App;
use crate::directory::Directory;

pub const WINDOW: u64 = 90 * 24 * 3600;
/// how far a statement's own time may run ahead of, or behind, its agreeing
const AHEAD: u64 = 300;
const BEHIND: u64 = 24 * 3600;

/// VERIFY_LEAVE_WINDOW_SECS shortens it, for a test
pub fn window() -> u64 {
    std::env::var("VERIFY_LEAVE_WINDOW_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(WINDOW)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Member,
    /// out, everything of theirs kept until `until`
    Left {
        since: u64,
        until: u64,
    },
    /// out past the window: everything of theirs goes
    Due {
        since: u64,
    },
}

/// When someone left: by their own word (a leave, not a coming back), or by
/// the owner's removal, whichever came first.
pub fn left_at(said: Option<&Said>, removed: Option<u64>) -> Option<u64> {
    let own = said.filter(|s| !s.statement.leave.back).map(|s| s.agreed);
    match (own, removed) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

pub fn phase(left: Option<u64>, now: u64, window: u64) -> Phase {
    match left {
        None => Phase::Member,
        Some(since) if now >= since.saturating_add(window) => Phase::Due { since },
        Some(since) => Phase::Left {
            since,
            until: since.saturating_add(window),
        },
    }
}

/// A statement is taken only near when it is agreed: never dated ahead,
/// which would put off the deleting, nor kept back to be played later.
pub fn timely(l: &identity::Leave, at: u64) -> Result<(), String> {
    if l.at > at.saturating_add(AHEAD) {
        return Err("dated ahead of now".into());
    }
    if l.at.saturating_add(BEHIND) < at {
        return Err("too old to be taken".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Said {
    pub statement: identity::SignedLeave,
    /// when the fleet agreed it: the log's time, or this box's alone
    /// without the control plane
    pub agreed: u64,
}

/// What this box has done for one leaving, keyed by its time: leaving
/// again later starts over
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Work {
    pub left: u64,
    #[serde(default)]
    pub ended: bool,
    #[serde(default)]
    pub done: BTreeSet<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    said: BTreeMap<String, Said>,
    #[serde(default)]
    work: BTreeMap<String, Work>,
}

pub struct Store {
    path: Option<PathBuf>,
    file: Mutex<File>,
    ceremonies: Mutex<HashMap<String, (Instant, identity::Leave)>>,
}

impl Store {
    pub fn open(path: Option<PathBuf>) -> Store {
        let file = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Store {
            path,
            file: Mutex::new(file),
            ceremonies: Default::default(),
        }
    }

    fn save(&self, f: &File) {
        let Some(p) = &self.path else { return };
        let tmp = p.with_extension("json.tmp");
        let ok = serde_json::to_vec_pretty(f)
            .ok()
            .is_some_and(|b| std::fs::write(&tmp, b).is_ok())
            && std::fs::rename(&tmp, p).is_ok();
        if !ok {
            eprintln!("leaving: could not write {}", p.display());
        }
    }

    pub fn said(&self, name: &str) -> Option<Said> {
        self.file.lock().unwrap().said.get(name).cloned()
    }

    /// out by their own word, whatever the member list says
    pub fn has_left(&self, name: &str) -> bool {
        self.said(name).is_some_and(|s| !s.statement.leave.back)
    }

    /// A statement agreed at `at`, held to the entry the fleet has for the
    /// name then. The newest statement stands; the same one twice is the
    /// log replayed.
    pub fn record(
        &self,
        s: identity::SignedLeave,
        on_file: Option<&identity::Entry>,
        at: u64,
    ) -> Result<(), String> {
        let e = on_file.ok_or("no entry by that name")?;
        identity::verify_leave(&s, e).map_err(|e| format!("refused: {e}"))?;
        timely(&s.leave, at)?;
        let mut f = self.file.lock().unwrap();
        if let Some(old) = f.said.get(&s.leave.name) {
            if old.statement == s {
                return Ok(());
            }
            if s.leave.at <= old.statement.leave.at {
                return Err("older than the statement on record".into());
            }
        }
        f.said.insert(
            s.leave.name.clone(),
            Said {
                statement: s,
                agreed: at,
            },
        );
        self.save(&f);
        Ok(())
    }

    /// what is done for `name` leaving at `left`
    pub fn work(&self, name: &str, left: u64) -> Work {
        match self.file.lock().unwrap().work.get(name) {
            Some(w) if w.left == left => w.clone(),
            _ => Work {
                left,
                ..Default::default()
            },
        }
    }

    pub fn mark(&self, name: &str, left: u64, change: impl FnOnce(&mut Work)) {
        let mut f = self.file.lock().unwrap();
        let w = f.work.entry(name.to_string()).or_default();
        if w.left != left {
            *w = Work {
                left,
                ..Default::default()
            };
        }
        change(w);
        self.save(&f);
    }

    fn ceremony_put(&self, l: identity::Leave) -> String {
        let id = crate::session::random_id();
        let mut m = self.ceremonies.lock().unwrap();
        m.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(300));
        m.insert(id.clone(), (Instant::now(), l));
        id
    }

    fn ceremony_take(&self, id: &str) -> Option<identity::Leave> {
        self.ceremonies
            .lock()
            .unwrap()
            .remove(id)
            .filter(|(t, _)| t.elapsed() < Duration::from_secs(300))
            .map(|(_, l)| l)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Leaver {
    pub name: String,
    /// the start of the member id: what the logs say instead of the name
    pub id: String,
    pub left: u64,
    pub due: bool,
    /// their own libraries, whose prefixes go with them: only ids no one
    /// else's entry names (`deletable`)
    pub libraries: Vec<String>,
    /// ids their entry names that are kept, and why
    pub kept: Vec<(String, &'static str)>,
}

/// A library id exactly as the gate makes them: 32 lowercase hex. An entry
/// is the person's own document, so nothing in it is taken as a path.
fn well_formed(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Which of the ids `name`'s entry lists may be deleted for good: well
/// formed, not the demo's, and named by no other entry in the directory,
/// leaving or not. The directory already refuses a second claim to an id
/// (directory.rs, claims_free); a deletion does not lean on that alone.
pub fn deletable(
    name: &str,
    ids: &[String],
    holders: &HashMap<String, BTreeSet<String>>,
    demo: Option<&str>,
) -> (Vec<String>, Vec<(String, &'static str)>) {
    let mut ok = Vec::new();
    let mut kept = Vec::new();
    for id in ids {
        let why = if !well_formed(id) {
            Some("not a library id")
        } else if demo == Some(id.as_str()) {
            Some("the demo's library")
        } else if holders.get(id).is_some_and(|h| h.iter().any(|n| n != name)) {
            Some("another entry names it")
        } else {
            None
        };
        match why {
            Some(w) => kept.push((id.clone(), w)),
            None if !ok.contains(id) => ok.push(id.clone()),
            None => {}
        }
    }
    (ok, kept)
}

/// The removal date counts only for an id the list also shuts out:
/// `dd member add` takes both away together.
pub fn removed_at(m: &crate::Members, id: &str) -> Option<u64> {
    m.revoked
        .iter()
        .any(|r| r == id)
        .then(|| m.removed.get(id).copied())
        .flatten()
}

/// Everyone out of the fleet by their word or the owner's, as this box's
/// copy of the directory and its release have it.
pub fn leavers(
    d: &Directory,
    m: &crate::Members,
    demo: Option<&str>,
    now: u64,
    window: u64,
) -> Vec<Leaver> {
    let entries: Vec<(String, identity::SignedEntry)> = d
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|l| Some((l.name.clone(), d.entry(&l.name).ok().flatten()?)))
        .collect();
    let mut holders: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (name, e) in &entries {
        for l in &e.entry.libraries {
            holders
                .entry(l.id.clone())
                .or_default()
                .insert(name.clone());
        }
    }
    let mut out = Vec::new();
    for (name, e) in entries {
        if crate::directory::is_guest(&name) {
            continue;
        }
        let id = identity::member_id(&e.entry.root);
        let left = left_at(d.leaving.said(&name).as_ref(), removed_at(m, &id));
        let (left, due) = match phase(left, now, window) {
            Phase::Member => continue,
            Phase::Left { since, .. } => (since, false),
            Phase::Due { since } => (since, true),
        };
        let ids: Vec<String> = e.entry.libraries.iter().map(|l| l.id.clone()).collect();
        let (libraries, kept) = deletable(&name, &ids, &holders, demo);
        out.push(Leaver {
            name,
            id: id[..8].to_string(),
            left,
            due,
            libraries,
            kept,
        });
    }
    out
}

/// The gate's own share, every minute: at once, every way in ends; past
/// the window, what this box holds of theirs goes. The other services do
/// theirs on their own boxes and report back here (`done`).
pub(crate) async fn tick(app: &App) {
    let Some(m) = &app.members else { return };
    let mail = std::env::var("VERIFY_MAIL_FORGET").ok();
    let mut reap = false;
    let demo = app.demo_library.as_ref().map(|(id, _)| id.as_str());
    for l in leavers(&app.directory, m, demo, identity::now(), window()) {
        let w = app.directory.leaving.work(&l.name, l.left);
        if !w.ended {
            let n = app.signins.forget(&l.name);
            app.directory.end_elsewhere(&l.name);
            let links = app.friends.drop_links_of(&l.name).unwrap_or(0);
            app.directory
                .leaving
                .mark(&l.name, l.left, |w| w.ended = true);
            eprintln!(
                "leaving: member {} is out: {n} session(s) and {links} friend link(s) ended",
                l.id
            );
            reap = true;
        }
        if !l.due {
            continue;
        }
        if !w.done.contains("gate") {
            match forget_here(app, &l) {
                Ok(()) => {
                    app.directory
                        .leaving
                        .mark(&l.name, l.left, |w| _ = w.done.insert("gate".into()));
                    eprintln!(
                        "leaving: member {}: the gate's notes and names are gone",
                        l.id
                    );
                }
                Err(e) => eprintln!("leaving: member {}: {e:#}", l.id),
            }
        }
        if let Some(g) = &app.library
            && !w.done.contains("libraries")
        {
            for (id, why) in &l.kept {
                eprintln!("leaving: member {}: library {id} kept: {why}", l.id);
            }
            let mut n = 0;
            let mut ok = true;
            for prefix in l
                .libraries
                .iter()
                .cloned()
                .chain(identity::valid_name(&l.name).then(|| format!("images-{}", l.name)))
            {
                match g.forget(&prefix).await {
                    Ok(k) => n += k,
                    Err(e) => {
                        ok = false;
                        eprintln!("leaving: member {}: a library: {e:#}", l.id);
                    }
                }
            }
            if ok {
                app.directory
                    .leaving
                    .mark(&l.name, l.left, |w| _ = w.done.insert("libraries".into()));
                eprintln!(
                    "leaving: member {}: libraries and disk images gone ({n} object(s))",
                    l.id
                );
            }
        }
        if let Some(url) = &mail
            && !w.done.contains("mail")
        {
            match tell_mail(url, &l.name).await {
                Ok(()) => {
                    app.directory
                        .leaving
                        .mark(&l.name, l.left, |w| _ = w.done.insert("mail".into()));
                    eprintln!("leaving: member {}: their mail address is gone", l.id);
                }
                Err(e) => eprintln!("leaving: member {}: mail: {e:#}", l.id),
            }
        }
    }
    if reap
        && let Some(door) = &app.network
        && !m.members.is_empty()
        && let Err(e) = door
            .reap(|u| (app.member(u) || app.guest(u)) && u != crate::pages::DEMO_USER)
            .await
    {
        eprintln!("network: reaping: {e:#}");
    }
}

/// what this box's gate keeps of someone: the names they gave their
/// devices, their notes on films, their friends, their sign-ins
fn forget_here(app: &App, l: &Leaver) -> anyhow::Result<()> {
    let gone = |p: PathBuf| match std::fs::remove_file(&p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    };
    gone(app.state_dir.join("names").join(format!("{}.bin", l.name)))?;
    anyhow::ensure!(identity::valid_name(&l.name), "not a name");
    let notes = app.state_dir.join("notes");
    for id in l.libraries.iter().filter(|id| well_formed(id)) {
        match std::fs::remove_dir_all(notes.join(id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    if let Ok(libs) = std::fs::read_dir(&notes) {
        for lib in libs.flatten() {
            gone(lib.path().join("progress").join(format!("{}.bin", l.name)))?;
        }
    }
    app.friends.forget(&l.name)?;
    app.signins.forget(&l.name);
    Ok(())
}

/// The mail Worker deletes the address itself, once it has checked the
/// member's own statement or the release's removal (client/mail/worker/leaving.js)
async fn tell_mail(url: &str, name: &str) -> anyhow::Result<()> {
    let r = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .post(url)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await?;
    anyhow::ensure!(r.status().is_success(), "the Worker said {}", r.status());
    Ok(())
}

// ---- over http

/// what the routes below need, on a full box and on a directory-only one
pub(crate) struct Ctx {
    pub d: Arc<Directory>,
    pub m: Option<crate::Members>,
    /// the demo library's id, never deleted
    pub demo: Option<String>,
}

/// the statement, for anyone; `dd leave`'s PUT
pub(crate) fn router<S: Clone + Send + Sync + 'static>(c: Arc<Ctx>) -> axum::Router<S> {
    axum::Router::new()
        .route(
            "/_dd/leaving/{name}",
            axum::routing::get(statement).put(put),
        )
        .with_state(c)
}

/// for the processes on this box alone (lib.rs puts this_box_only on it)
pub(crate) fn internal_router<S: Clone + Send + Sync + 'static>(c: Arc<Ctx>) -> axum::Router<S> {
    axum::Router::new()
        .route("/internal/leaving", axum::routing::get(internal))
        .route("/internal/leaving/done", axum::routing::post(done))
        .with_state(c)
}

/// GET /internal/leaving: for the services on this box that delete their
/// own share (photos, passwords, forge, games)
async fn internal(State(c): State<Arc<Ctx>>) -> Response {
    let d = &c.d;
    let Some(m) = &c.m else {
        return Json(serde_json::json!({ "members": [] })).into_response();
    };
    let list: Vec<serde_json::Value> = leavers(d, m, c.demo.as_deref(), identity::now(), window())
        .into_iter()
        .map(|l| {
            let done = d.leaving.work(&l.name, l.left).done;
            serde_json::json!({
                "name": l.name, "id": l.id, "left": l.left, "due": l.due, "done": done,
            })
        })
        .collect();
    Json(serde_json::json!({ "window": window(), "members": list })).into_response()
}

#[derive(Deserialize)]
struct Done {
    name: String,
    service: String,
}

/// POST /internal/leaving/done: a service on this box deleted its share
async fn done(State(c): State<Arc<Ctx>>, Json(x): Json<Done>) -> Response {
    let (d, m) = (&c.d, c.m.as_ref());
    let ok_service = !x.service.is_empty()
        && x.service.len() <= 32
        && x.service
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-');
    let Some(m) = m.filter(|_| ok_service) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(l) = leavers(d, m, c.demo.as_deref(), identity::now(), window())
        .into_iter()
        .find(|l| l.name == x.name && l.due)
    else {
        return (
            StatusCode::CONFLICT,
            "not someone whose things are due to go",
        )
            .into_response();
    };
    d.leaving
        .mark(&l.name, l.left, |w| _ = w.done.insert(x.service.clone()));
    eprintln!("leaving: member {}: {} is done", l.id, x.service);
    StatusCode::NO_CONTENT.into_response()
}

/// GET /_dd/leaving/{name}: the newest statement, signed, for the mail
/// Worker to check for itself. It names a member and their root, so it is
/// read as their entry is: by members, the member, or the Worker's read key
async fn statement(
    State(c): State<Arc<Ctx>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !c.d.may_read(&headers, Some(&name)) {
        return crate::directory::refused();
    }
    match c.d.leaving.said(&name) {
        Some(s) => Json(serde_json::json!({ "statement": s.statement, "agreed": s.agreed }))
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// PUT /_dd/leaving/{name}: a statement `dd leave` signed with a device root
async fn put(
    State(c): State<Arc<Ctx>>,
    Path(name): Path<String>,
    Json(s): Json<identity::SignedLeave>,
) -> Response {
    if s.leave.name != name {
        return (StatusCode::BAD_REQUEST, "that statement is someone else's").into_response();
    }
    match c.d.say_leave(s).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err((st, why)) => (st, why).into_response(),
    }
}

fn signed_in(app: &App, headers: &HeaderMap) -> Option<String> {
    app.signed_in(headers.get("cookie").and_then(|v| v.to_str().ok()))
}

/// GET /_dd/leave: where the signed-in person stands
pub(crate) async fn mine(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = signed_in(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(e) = app.entry(&user).ok().flatten() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let removed = app
        .members
        .as_ref()
        .and_then(|m| removed_at(m, &identity::member_id(&e.entry.root)));
    let said = app.directory.leaving.said(&user);
    let (left, until) = match phase(left_at(said.as_ref(), removed), identity::now(), window()) {
        Phase::Member => (None, None),
        Phase::Left { since, until } => (Some(since), Some(until)),
        Phase::Due { since } => (Some(since), Some(since + window())),
    };
    Json(serde_json::json!({
        "name": user,
        "member": app.member(&user),
        "left": left,
        "until": until,
        "removed": removed.is_some(),
        "window": window(),
        "passkey": e.entry.root.starts_with(identity::WEBAUTHN_ROOT),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct Start {
    #[serde(default)]
    back: bool,
}

/// POST /_dd/leave/start {back}: the statement, for the root passkey to sign
pub(crate) async fn start(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(q): Json<Start>,
) -> Response {
    if !crate::from_our_site(&app.domain, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(user) = signed_in(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if user == crate::pages::DEMO_USER {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(e) = app.entry(&user).ok().flatten() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let out = app.directory.leaving.has_left(&user);
    if q.back != out {
        return (
            StatusCode::CONFLICT,
            if q.back {
                "you have not left"
            } else {
                "you have left already"
            },
        )
            .into_response();
    }
    if !q.back && !app.member(&user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !e.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        return (
            StatusCode::BAD_REQUEST,
            "this account's key is on a device: leave from there",
        )
            .into_response();
    }
    let leave = identity::Leave {
        name: user,
        root: e.entry.root.clone(),
        at: identity::now(),
        back: q.back,
    };
    let Ok(challenge) = identity::leave_challenge(&leave) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let allow: Vec<serde_json::Value> = e
        .entry
        .passkeys
        .iter()
        .filter(|p| {
            e.entry
                .root
                .starts_with(&format!("{}{}:", identity::WEBAUTHN_ROOT, p.id))
        })
        .map(|p| serde_json::json!({ "type": "public-key", "id": p.id }))
        .collect();
    let ceremony = app.directory.leaving.ceremony_put(leave);
    Json(serde_json::json!({
        "ceremony": ceremony,
        "publicKey": {
            "challenge": B64_URL.encode(challenge),
            "timeout": 60000,
            "rpId": app.domain,
            "allowCredentials": allow,
            "userVerification": "preferred",
        }
    }))
    .into_response()
}

/// POST /_dd/leave/sign: the passkey's answer; into the log, and at once
/// this box ends the session it came in
pub(crate) async fn sign(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(cred): Json<PublicKeyCredential>,
) -> Response {
    if !crate::from_our_site(&app.domain, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(user) = signed_in(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(leave) = headers
        .get("x-dd-ceremony")
        .and_then(|v| v.to_str().ok())
        .and_then(|id| app.directory.leaving.ceremony_take(id))
        .filter(|l| l.name == user)
    else {
        return (StatusCode::BAD_REQUEST, "no ceremony").into_response();
    };
    let assertion = identity::Assertion {
        authenticator_data: B64_URL.encode(&cred.response.authenticator_data),
        client_data_json: B64_URL.encode(&cred.response.client_data_json),
        signature: B64_URL.encode(&cred.response.signature),
    };
    let Ok(sig) = serde_json::to_vec(&assertion) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let back = leave.back;
    let s = identity::SignedLeave {
        leave,
        signature: B64.encode(sig),
    };
    if let Err((st, why)) = app.directory.say_leave(s).await {
        return (st, why).into_response();
    }
    if !back {
        app.signins.forget(&user);
        app.directory.end_elsewhere(&user);
    }
    Json(serde_json::json!({ "back": back })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 3600;

    fn said(at: u64, agreed: u64, back: bool) -> Said {
        Said {
            statement: identity::SignedLeave {
                leave: identity::Leave {
                    name: "tom".into(),
                    root: "r".into(),
                    at,
                    back,
                },
                signature: String::new(),
            },
            agreed,
        }
    }

    #[test]
    fn the_window_runs_from_the_first_word_that_they_are_out() {
        assert_eq!(left_at(None, None), None);
        assert_eq!(
            left_at(Some(&said(10, 12, false)), None),
            Some(12),
            "the agreed time, not the claimed one"
        );
        assert_eq!(left_at(None, Some(50)), Some(50));
        assert_eq!(left_at(Some(&said(10, 12, false)), Some(50)), Some(12));
        assert_eq!(left_at(Some(&said(60, 61, false)), Some(50)), Some(50));
        // coming back undoes their own leaving, never the owner's removal
        assert_eq!(left_at(Some(&said(70, 71, true)), None), None);
        assert_eq!(left_at(Some(&said(70, 71, true)), Some(50)), Some(50));
    }

    #[test]
    fn everything_is_kept_for_the_window_and_then_due() {
        let w = 90 * DAY;
        assert_eq!(phase(None, 1000, w), Phase::Member);
        assert_eq!(
            phase(Some(1000), 1000, w),
            Phase::Left {
                since: 1000,
                until: 1000 + w
            }
        );
        assert_eq!(
            phase(Some(1000), 1000 + w - 1, w),
            Phase::Left {
                since: 1000,
                until: 1000 + w
            }
        );
        assert_eq!(phase(Some(1000), 1000 + w, w), Phase::Due { since: 1000 });
        assert_eq!(WINDOW, 90 * DAY);
    }

    #[test]
    fn a_statement_is_taken_only_near_when_it_is_agreed() {
        let l = |at| identity::Leave {
            name: "tom".into(),
            root: "r".into(),
            at,
            back: false,
        };
        let now = 100 * DAY;
        assert!(timely(&l(now), now).is_ok());
        assert!(timely(&l(now + 299), now).is_ok());
        assert!(
            timely(&l(now + 301), now).is_err(),
            "dated ahead puts off the deleting"
        );
        assert!(timely(&l(now - DAY), now).is_ok());
        assert!(
            timely(&l(now - DAY - 1), now).is_err(),
            "an old one played back later"
        );
    }

    #[test]
    fn only_libraries_no_one_else_names_are_deleted() {
        let mine = "0123456789abcdef0123456789abcdef".to_string();
        let shared = "11111111111111111111111111111111".to_string();
        let demo = "22222222222222222222222222222222".to_string();
        let mut holders: HashMap<String, BTreeSet<String>> = HashMap::new();
        holders
            .entry(mine.clone())
            .or_default()
            .insert("tom".into());
        holders
            .entry(shared.clone())
            .or_default()
            .extend(["tom".into(), "ann".into()]);
        holders
            .entry(demo.clone())
            .or_default()
            .insert("tom".into());
        let ids = vec![
            mine.clone(),
            shared.clone(),
            demo.clone(),
            "../../etc".into(),
            "0123456789ABCDEF0123456789ABCDEF".into(),
            "0123".into(),
            mine.clone(),
        ];
        let (ok, kept) = deletable("tom", &ids, &holders, Some(&demo));
        assert_eq!(ok, vec![mine]);
        assert!(
            kept.contains(&(shared, "another entry names it")),
            "another member's survives"
        );
        assert!(kept.contains(&(demo, "the demo's library")));
        assert_eq!(
            kept.iter()
                .filter(|(_, w)| *w == "not a library id")
                .count(),
            3
        );
    }

    #[test]
    fn a_removal_date_counts_only_while_the_list_shuts_them_out() {
        let mut m = crate::Members::default();
        m.removed.insert("abc".into(), 7);
        assert_eq!(removed_at(&m, "abc"), None);
        m.revoked.push("abc".into());
        assert_eq!(removed_at(&m, "abc"), Some(7));
        m.revoked.push("def".into());
        assert_eq!(
            removed_at(&m, "def"),
            None,
            "revoked before dates were kept: not scheduled"
        );
    }

    #[test]
    fn the_newest_statement_stands_and_work_starts_over_on_a_new_leaving() {
        let dir = std::env::temp_dir().join(format!("leaving-{}", crate::session::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = identity::generate();
        let entry = identity::Entry {
            name: "tom".into(),
            root: identity::encode_public(&root.verifying_key()),
            recovery: String::new(),
            devices: vec![],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            version: 1,
            updated: 1,
        };
        let say = |at, back| {
            identity::sign_leave(
                identity::Leave {
                    name: "tom".into(),
                    root: entry.root.clone(),
                    at,
                    back,
                },
                &root,
            )
            .unwrap()
        };
        let s = Store::open(Some(dir.join("leaving.json")));
        assert!(
            s.record(say(1000, false), None, 1000).is_err(),
            "no entry, nothing to check"
        );
        s.record(say(1000, false), Some(&entry), 1001).unwrap();
        assert!(s.has_left("tom"));
        s.record(say(1000, false), Some(&entry), 1001).unwrap();
        assert!(
            s.record(say(900, true), Some(&entry), 1002).is_err(),
            "older than what stands"
        );
        s.mark("tom", 1001, |w| {
            w.ended = true;
            w.done.insert("gate".into());
        });
        assert!(s.work("tom", 1001).done.contains("gate"));
        // kept across a restart
        let s = Store::open(Some(dir.join("leaving.json")));
        assert!(s.has_left("tom"));
        assert!(s.work("tom", 1001).ended);
        s.record(say(2000, true), Some(&entry), 2000).unwrap();
        assert!(!s.has_left("tom"));
        s.record(say(3000, false), Some(&entry), 3000).unwrap();
        assert_eq!(
            s.work("tom", 3000),
            Work {
                left: 3000,
                ..Default::default()
            }
        );
        let mut forged = say(4000, true);
        forged.leave.name = "ann".into();
        assert!(s.record(forged, Some(&entry), 4000).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
