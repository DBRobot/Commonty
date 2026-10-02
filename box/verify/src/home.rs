//! The Network tab's: how each box in the house is connected, and a Wi-Fi
//! change a member makes there. The change is signed by the member's passkey
//! in the browser, over the exact bytes of what changes; every box checks
//! that signature against the member's own entry before it acts, so the box
//! that relays it to the others is believed by none of them. A root unit on
//! each box tries the new details and falls back to the old ones
//! (nix/modules/box/wifi-apply.sh); this only hands it over and reads back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::Digest as _;

use crate::session::now;
use crate::{App, pages};

/// what a change signs: this label, then the payload's bytes as sent
const LABEL: &[u8] = b"commonty wifi v1\0";
/// how old a signed change may be when it arrives
const FRESH: u64 = 600;

pub struct Home {
    /// /run/dd-wifi, where the root unit reads requests and writes status
    dir: Option<PathBuf>,
    /// the pages a change may be signed on: Settings' (VERIFY_HOUSE_ORIGINS)
    origins: Vec<String>,
    /// the other boxes' gates, to relay to and ask after
    peers: Vec<String>,
    /// changes already acted on, by nonce, so a signed one is used once
    seen: Mutex<HashMap<String, u64>>,
}

impl Home {
    fn seen_file(dir: &Option<PathBuf>) -> Option<PathBuf> {
        dir.as_ref().map(|d| d.join("seen.json"))
    }

    fn keep_seen(&self, seen: &HashMap<String, u64>) {
        if let Some(p) = Self::seen_file(&self.dir)
            && let Ok(b) = serde_json::to_vec(seen)
        {
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    pub fn new(dir: Option<PathBuf>, directory_peers: &[String]) -> Home {
        let seen = Self::seen_file(&dir)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Home {
            dir,
            origins: std::env::var("VERIFY_HOUSE_ORIGINS")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            peers: directory_peers
                .iter()
                .map(|p| {
                    p.trim_end_matches('/')
                        .trim_end_matches("/_dd/directory")
                        .to_string()
                })
                .collect(),
            seen: Mutex::new(seen),
        }
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn read_json(p: PathBuf) -> serde_json::Value {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Who may change the house's Wi-Fi, as a box knows it: the full gate
/// (App), or a box that runs only the directory (DirHouse).
pub(crate) trait Vouch: Send + Sync {
    fn entry_of(&self, user: &str) -> Option<identity::SignedEntry>;
    /// a member: not a guest, not the demo
    fn is_member(&self, user: &str) -> bool;
    fn house(&self) -> &Home;
}

impl Vouch for App {
    fn entry_of(&self, user: &str) -> Option<identity::SignedEntry> {
        self.entry(user).ok().flatten()
    }
    fn is_member(&self, user: &str) -> bool {
        self.member(user) && user != pages::DEMO_USER && !self.guest(user)
    }
    fn house(&self) -> &Home {
        &self.home_net
    }
}

/// A box that runs the directory and nothing else of the gate: it still has
/// a Wi-Fi to change, the entries to check a change against, and the member
/// list its release came with.
pub struct DirHouse {
    pub home: Home,
    pub directory: Arc<crate::directory::Directory>,
    pub members: Option<crate::Members>,
}

impl Vouch for DirHouse {
    fn entry_of(&self, user: &str) -> Option<identity::SignedEntry> {
        self.directory.entry(user).ok().flatten()
    }
    fn is_member(&self, user: &str) -> bool {
        // no list, no members: this box does not guess
        let (Some(m), Some(e)) = (&self.members, self.entry_of(user)) else {
            return false;
        };
        let id = identity::member_id(&e.entry.root);
        if m.revoked.contains(&id) {
            return false;
        }
        m.members.contains(&id)
            || (e.entry.grant.is_some()
                && self
                    .directory
                    .release()
                    .is_some_and(|r| identity::verify_grant(&e.entry, r).is_ok()))
    }
    fn house(&self) -> &Home {
        &self.home
    }
}

/// The routes a directory-only box serves for the house
pub fn dir_router(h: Arc<DirHouse>) -> axum::Router {
    axum::Router::new()
        .route(
            "/_dd/house/here",
            axum::routing::get(|State(h): State<Arc<DirHouse>>| async move {
                Json(here(&h.home, false)).into_response()
            }),
        )
        .route(
            "/_dd/house/wifi/relay",
            axum::routing::post(
                |State(h): State<Arc<DirHouse>>, Json(s): Json<Signed>| async move {
                    relay_with(&*h, &s)
                },
            ),
        )
        .with_state(h)
}

/// This box: how it is connected, and how the last change went. The house's
/// network name only for a member asking this box (`named`): what any box
/// or stranger can ask leaves it out, as it would point to the house.
fn here(home: &Home, named: bool) -> serde_json::Value {
    let (mut status, last) = match &home.dir {
        Some(d) => (
            read_json(d.join("status.json")),
            read_json(d.join("result.json")),
        ),
        None => (serde_json::Value::Null, serde_json::Value::Null),
    };
    if !named && let Some(w) = status.get_mut("wifi").and_then(|w| w.as_object_mut()) {
        w.remove("ssid");
    }
    serde_json::json!({ "box": hostname(), "status": status, "last": last })
}

/// GET /_dd/house/here - for the other boxes' gates; nothing secret in it
pub(crate) async fn here_route(State(app): State<Arc<App>>) -> Response {
    Json(here(&app.home_net, false)).into_response()
}

fn member(app: &App, headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let user = app.signed_in(cookie)?;
    (app.member(&user) && user != pages::DEMO_USER && !app.guest(&user)).then_some(user)
}

/// GET /_dd/house: every box in the house, this one and its peers
pub(crate) async fn list(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if member(&app, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut boxes = vec![here(&app.home_net, true)];
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap_or_default();
    let asks = app.home_net.peers.iter().map(|p| {
        let http = http.clone();
        let url = format!("{p}/_dd/house/here");
        async move {
            match http.get(&url).send().await {
                Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
                _ => None,
            }
        }
    });
    for (p, got) in app
        .home_net
        .peers
        .iter()
        .zip(futures_util::future::join_all(asks).await)
    {
        boxes.push(got.unwrap_or_else(|| serde_json::json!({ "box": p, "unreachable": true })));
    }
    Json(serde_json::json!({ "boxes": boxes })).into_response()
}

#[derive(Deserialize, serde::Serialize, Clone)]
pub(crate) struct Signed {
    /// the change, as the bytes the passkey signed: {user, ssid, psk, at, nonce}
    payload: String,
    /// the passkey's id, and its answer (base64url parts)
    id: String,
    assertion: identity::Assertion,
}

#[derive(Deserialize)]
struct Change {
    user: String,
    ssid: String,
    psk: String,
    at: u64,
    nonce: String,
}

/// The member's own passkey said this, just now, and nobody has used it yet.
fn check(app: &dyn Vouch, s: &Signed) -> Result<Change, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::FORBIDDEN, m.to_string());
    let c: Change = serde_json::from_str(&s.payload)
        .map_err(|_| (StatusCode::BAD_REQUEST, "not a change".to_string()))?;
    // nothing NetworkManager's keyfile would read as more than a name
    if c.ssid.is_empty()
        || c.ssid.len() > 32
        || c.ssid.chars().any(|ch| ch.is_control() || ch == '\\')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "that network name will not do".into(),
        ));
    }
    if !(8..=63).contains(&c.psk.len()) || c.psk.chars().any(|ch| ch.is_control() || ch == '\\') {
        return Err((
            StatusCode::BAD_REQUEST,
            "a Wi-Fi password is 8 to 63 characters".into(),
        ));
    }
    // made in the last ten minutes, and not dated ahead to last longer
    if c.at > now() + 60 || now().saturating_sub(c.at) > FRESH {
        return Err(bad("that change is too old; make it again"));
    }
    // one answer whatever went wrong: who is a member is not told here
    if !app.is_member(&c.user) {
        return Err(bad("your passkey did not sign that"));
    }
    let entry = app
        .entry_of(&c.user)
        .ok_or_else(|| bad("your passkey did not sign that"))?;
    let mut h = sha2::Sha256::new();
    h.update(LABEL);
    h.update(s.payload.as_bytes());
    identity::check_login(
        &entry.entry,
        &s.id,
        &s.assertion,
        &h.finalize(),
        &app.house().origins,
    )
    .map_err(|_| bad("your passkey did not sign that"))?;
    let mut seen = app.house().seen.lock().unwrap();
    seen.retain(|_, t| now() < *t + FRESH * 2);
    if seen.insert(c.nonce.clone(), now()).is_some() {
        return Err(bad("that change was already made"));
    }
    // and kept where the gate's next start reads it, so a restart does not
    // make a change usable again
    app.house().keep_seen(&seen);
    Ok(c)
}

/// Into the folder the root unit watches. The password is there for as long
/// as the unit takes to read it.
fn hand_over(home: &Home, c: &Change) -> Result<(), (StatusCode, String)> {
    let Some(dir) = &home.dir else {
        return Err((StatusCode::NOT_FOUND, "this box has no Wi-Fi".into()));
    };
    let body = serde_json::json!({ "ssid": c.ssid, "psk": c.psk, "nonce": c.nonce });
    let tmp = dir.join("request.tmp");
    std::fs::write(&tmp, body.to_string())
        .and_then(|_| std::fs::rename(&tmp, dir.join("request")))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// POST /_dd/house/wifi: from the Network tab. Tried here, and passed on to
/// the other boxes, each of which checks the signature for itself.
pub(crate) async fn change(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(s): Json<Signed>,
) -> Response {
    if member(&app, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let c = match check(&*app, &s) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = hand_over(&app.home_net, &c) {
        // a box without Wi-Fi still passes it on
        if e.0 != StatusCode::NOT_FOUND {
            return e.into_response();
        }
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    let mut passed = 0;
    for p in &app.home_net.peers {
        if let Ok(r) = http
            .post(format!("{p}/_dd/house/wifi/relay"))
            .json(&s)
            .send()
            .await
            && r.status().is_success()
        {
            passed += 1;
        }
    }
    Json(serde_json::json!({ "nonce": c.nonce, "passed": passed })).into_response()
}

/// POST /_dd/house/wifi/relay: another box passing a member's change on.
/// Believed for the signature, not for who sent it.
pub(crate) async fn relay(State(app): State<Arc<App>>, Json(s): Json<Signed>) -> Response {
    relay_with(&*app, &s)
}

fn relay_with(v: &dyn Vouch, s: &Signed) -> Response {
    match check(v, s).and_then(|c| hand_over(v.house(), &c).map(|_| c)) {
        Ok(c) => Json(serde_json::json!({ "nonce": c.nonce })).into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn peers_are_their_gates() {
        let h = super::Home::new(
            None,
            &[
                "http://100.95.10.10:4181/_dd/directory".into(),
                "https://files.example/_dd/directory/".into(),
            ],
        );
        assert_eq!(
            h.peers,
            ["http://100.95.10.10:4181", "https://files.example"]
        );
    }
}
