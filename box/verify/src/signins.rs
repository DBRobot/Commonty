//! Who is signed in where, for the Devices tab: every browser or app session
//! this box gave out (by the random id in its cookie), what it is, when it
//! started and was last seen, and which device's key started it; and when
//! each device key and passkey was last used. A session can be ended here
//! before its cookie runs out, and one a removed device started ends by
//! itself. Kept in signins.json beside the session secret; the names people
//! give their devices are not here - those are locked in the browser.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::session::now;

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub user: String,
    /// what it is, in words: "Chrome on Linux", "Commonty app on Android"
    pub kind: String,
    pub first: u64,
    pub last: u64,
    /// when its cookie runs out anyway
    pub exp: u64,
    /// the device whose key signed it in (the app, `dd`), if one did
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeyUse {
    pub kind: String,
    pub last: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    sessions: BTreeMap<String, Session>,
    /// ended before their time, by id, until the cookie would have run out
    ended: BTreeMap<String, u64>,
    /// by user, then by device fingerprint or "passkey:<id>"
    keys: BTreeMap<String, BTreeMap<String, KeyUse>>,
}

pub struct SignIns {
    path: Option<PathBuf>,
    state: Mutex<(State, u64)>,
}

/// how stale "last seen" may get before it is written down
const WRITE_EVERY: u64 = 60;

impl SignIns {
    pub fn open(path: Option<PathBuf>) -> SignIns {
        let state = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        SignIns {
            path,
            state: Mutex::new((state, 0)),
        }
    }

    fn save(&self, s: &mut (State, u64), force: bool) {
        let t = now();
        if !force && t < s.1 + WRITE_EVERY {
            return;
        }
        s.0.sessions.retain(|_, v| v.exp > t);
        s.0.ended.retain(|_, exp| *exp > t);
        s.1 = t;
        if let Some(p) = &self.path
            && let Ok(b) = serde_json::to_vec(&s.0)
        {
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, p);
            }
        }
    }

    pub fn is_ended(&self, id: &str) -> bool {
        self.state.lock().unwrap().0.ended.contains_key(id)
    }

    /// A session was used: noted, and what it is, from its user agent.
    pub fn saw(&self, user: &str, id: &str, exp: u64, agent: &str, device: Option<&str>) {
        let mut s = self.state.lock().unwrap();
        let t = now();
        let fresh = !s.0.sessions.contains_key(id);
        let e =
            s.0.sessions
                .entry(id.to_string())
                .or_insert_with(|| Session {
                    user: user.to_string(),
                    kind: kind(agent),
                    first: t,
                    last: t,
                    exp,
                    device: device.map(str::to_string),
                });
        e.last = t;
        if device.is_some() && e.device.is_none() {
            e.device = device.map(str::to_string);
        }
        self.save(&mut s, fresh);
    }

    /// The device a session was signed in by, if one was.
    pub fn device_of(&self, id: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .0
            .sessions
            .get(id)?
            .device
            .clone()
    }

    pub fn end(&self, id: &str) {
        let mut s = self.state.lock().unwrap();
        let exp =
            s.0.sessions
                .remove(id)
                .map(|v| v.exp)
                .unwrap_or(now() + crate::session::TTL);
        s.0.ended.insert(id.to_string(), exp);
        self.save(&mut s, true);
    }

    pub fn sessions(&self, user: &str) -> Vec<(String, Session)> {
        let s = self.state.lock().unwrap();
        let t = now();
        let mut v: Vec<_> =
            s.0.sessions
                .iter()
                .filter(|(_, x)| x.user == user && x.exp > t)
                .map(|(k, x)| (k.clone(), x.clone()))
                .collect();
        v.sort_by_key(|(_, x)| std::cmp::Reverse(x.last));
        v
    }

    /// A device key or a passkey was used.
    pub fn used(&self, user: &str, key: &str, agent: &str) {
        let mut s = self.state.lock().unwrap();
        let t = now();
        let k = s.0.keys.entry(user.to_string()).or_default();
        let fresh = !k.contains_key(key);
        // a request that says nothing about itself keeps what an earlier one said
        let said = kind(agent);
        let kind = match k.get(key) {
            Some(was) if said == UNKNOWN => was.kind.clone(),
            _ => said,
        };
        k.insert(key.to_string(), KeyUse { kind, last: t });
        self.save(&mut s, fresh);
    }

    pub fn keys(&self, user: &str) -> BTreeMap<String, KeyUse> {
        self.state
            .lock()
            .unwrap()
            .0
            .keys
            .get(user)
            .cloned()
            .unwrap_or_default()
    }
}

const UNKNOWN: &str = "Something unrecognised";

/// What a user agent is, the way a person would say it.
pub fn kind(agent: &str) -> String {
    let a = agent.to_ascii_lowercase();
    let os = if a.contains("android") {
        "Android"
    } else if a.contains("iphone") || a.contains("ipad") {
        "iOS"
    } else if a.contains("mac os") || a.contains("macintosh") || a.contains("macos") {
        "macOS"
    } else if a.contains("windows") {
        "Windows"
    } else if a.contains("linux") {
        "Linux"
    } else {
        ""
    };
    // the command line names itself dd/…: to a person it is their computer
    if a.starts_with("dd/") {
        return if os.is_empty() {
            "A computer".into()
        } else {
            format!("{os} computer")
        };
    }
    let what = if a.contains("commonty") || a.contains("tauri") {
        "Commonty app"
    } else if a.contains("edg/") {
        "Edge"
    } else if a.contains("firefox/") {
        "Firefox"
    } else if a.contains("chrome/") || a.contains("chromium/") {
        "Chrome"
    } else if a.contains("safari/") {
        "Safari"
    } else {
        ""
    };
    match (what, os) {
        ("", "") => UNKNOWN.into(),
        ("", os) => format!("A browser on {os}"),
        (w, "") => w.into(),
        (w, os) => format!("{w} on {os}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agents_in_words() {
        let chrome = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
        assert_eq!(kind(chrome), "Chrome on Linux");
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36 Edg/141.0";
        assert_eq!(kind(edge), "Edge on Windows");
        let fx = "Mozilla/5.0 (Windows NT 10.0; rv:142.0) Gecko/20100101 Firefox/142.0";
        assert_eq!(kind(fx), "Firefox on Windows");
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
        assert_eq!(kind(safari), "Safari on iOS");
        assert_eq!(kind("dd/0.1.0 (linux)"), "Linux computer");
        assert_eq!(kind(""), "Something unrecognised");
    }

    #[test]
    fn an_ended_session_stays_ended_and_others_are_listed() {
        let s = SignIns::open(None);
        let exp = now() + 3600;
        s.saw("tom", "a", exp, "Firefox/1 (Linux)", None);
        s.saw("tom", "b", exp, "dd/0.1 (linux)", Some("fp1"));
        s.saw("sam", "c", exp, "", None);
        assert_eq!(s.sessions("tom").len(), 2);
        assert_eq!(s.device_of("b").as_deref(), Some("fp1"));
        s.end("a");
        assert!(s.is_ended("a"));
        assert!(!s.is_ended("b"));
        assert_eq!(s.sessions("tom").len(), 1);
        // using it again does not bring it back
        assert!(s.is_ended("a"));
    }
}

pub(crate) mod http {
    //! GET /_dd/signins, POST /_dd/signins/end, and the locked device names
    //! at /_dd/devices/names - the Devices tab's (box/fleet/web/devices.js).
    use std::sync::Arc;

    use axum::Json;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use serde::Deserialize;

    use crate::{App, pages};

    /// a member, signed in by session: who, and this session's id
    #[allow(clippy::result_large_err)]
    fn member(app: &App, headers: &HeaderMap) -> Result<(String, String), Response> {
        let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
        let user = app
            .signed_in(cookie)
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;
        // members and guests: anyone with devices of their own
        if user == pages::DEMO_USER || !(app.member(&user) || app.guest(&user)) {
            return Err(StatusCode::FORBIDDEN.into_response());
        }
        let id = app
            .sessions
            .session(cookie)
            .map(|s| s.1)
            .unwrap_or_default();
        Ok((user, id))
    }

    pub(crate) async fn list(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
        let (user, here) = match member(&app, &headers) {
            Ok(m) => m,
            Err(r) => return r,
        };
        let sessions: Vec<_> = app
            .signins
            .sessions(&user)
            .into_iter()
            .map(|(id, s)| {
                serde_json::json!({
                    "id": id,
                    "kind": s.kind,
                    "first": s.first,
                    "last": s.last,
                    "device": s.device,
                    "current": id == here,
                })
            })
            .collect();
        Json(serde_json::json!({ "sessions": sessions, "keys": app.signins.keys(&user) }))
            .into_response()
    }

    #[derive(Deserialize)]
    pub(crate) struct End {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        others: bool,
    }

    /// Ending a session somewhere else wants the passkey just now: a stolen
    /// cookie cannot sign its owner out of everything.
    pub(crate) async fn end(
        State(app): State<Arc<App>>,
        headers: HeaderMap,
        Json(e): Json<End>,
    ) -> Response {
        let (user, here) = match member(&app, &headers) {
            Ok(m) => m,
            Err(r) => return r,
        };
        if !app.passkey_fresh(&here) {
            return (
                StatusCode::FORBIDDEN,
                "confirm it is you with your passkey first",
            )
                .into_response();
        }
        let mine: Vec<String> = app
            .signins
            .sessions(&user)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let gone: Vec<&String> = match (&e.id, e.others) {
            (_, true) => mine.iter().filter(|id| **id != here).collect(),
            (Some(id), false) if mine.contains(id) => vec![id],
            _ => return (StatusCode::NOT_FOUND, "no such session of yours").into_response(),
        };
        for id in &gone {
            app.signins.end(id);
        }
        // and every other way in that this gate signed them into
        if e.others {
            app.directory.end_elsewhere(&user);
        }
        Json(serde_json::json!({ "ended": gone.len() })).into_response()
    }

    fn names_file(app: &App, user: &str) -> Option<std::path::PathBuf> {
        crate::valid_user(user).then(|| app.state_dir.join("names").join(format!("{user}.bin")))
    }

    /// The member's names for their devices, locked in their browser with
    /// their library key: this box keeps the bytes and cannot read them.
    pub(crate) async fn names(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
        let (user, _) = match member(&app, &headers) {
            Ok(m) => m,
            Err(r) => return r,
        };
        match names_file(&app, &user).and_then(|p| std::fs::read(p).ok()) {
            Some(b) => b.into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    pub(crate) async fn put_names(
        State(app): State<Arc<App>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let (user, _) = match member(&app, &headers) {
            Ok(m) => m,
            Err(r) => return r,
        };
        if body.len() > 64 * 1024 {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        let Some(p) = names_file(&app, &user) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let tmp = p.with_extension("bin.tmp");
        let ok = p
            .parent()
            .is_some_and(|d| std::fs::create_dir_all(d).is_ok())
            && std::fs::write(&tmp, &body).is_ok()
            && std::fs::rename(&tmp, &p).is_ok();
        if ok {
            StatusCode::NO_CONTENT.into_response()
        } else {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
