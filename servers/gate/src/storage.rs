//! Storage: one allowance per member across every service. The storage
//! ledger (`dd storage-ledger`, a timer on this box) adds up what each
//! member keeps - Photos, their libraries, Code, Passwords - and writes it
//! down; it also holds Photos to what is left. This reads what it wrote, for
//! the Storage tab of Settings, and holds the libraries to it as uploads arrive: what
//! arrived since the ledger last counted is added as it lands, so the hour
//! between its counts is no way past the allowance.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Redirect, Response};
use serde::Deserialize;

use crate::App;
use crate::pages;

/// what the ledger wrote: when, the allowance, and each member's bytes
#[derive(Clone, Default, Deserialize)]
pub struct Counted {
    pub updated: u64,
    pub budget: u64,
    #[serde(default)]
    pub members: HashMap<String, Usage>,
}

#[derive(Clone, Default, Deserialize, serde::Serialize)]
pub struct Usage {
    #[serde(default)]
    pub photos: u64,
    #[serde(default)]
    pub libraries: u64,
    #[serde(default)]
    pub code: u64,
    #[serde(default)]
    pub passwords: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.photos + self.libraries + self.code + self.passwords
    }
}

#[derive(Default)]
pub struct Ledger {
    path: Option<PathBuf>,
    /// the file as last read, and its modification time
    read: Mutex<Option<(std::time::SystemTime, Arc<Counted>)>>,
    /// bytes uploaded to a member's libraries since the ledger's count, and
    /// bytes deleted for good from their trash: (count, uploaded, freed)
    since: Mutex<HashMap<String, (u64, u64, u64)>>,
    /// room promised to uploads still under way, per member, so two at once
    /// cannot each be told the same room is free
    held: Mutex<HashMap<String, u64>>,
}

/// Room set aside for one upload: at most `limit` bytes (None: no allowance
/// on this box). Given back with `Ledger::done`.
pub struct Held {
    user: String,
    pub limit: Option<u64>,
}

impl Ledger {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            ..Default::default()
        }
    }

    /// the ledger's latest, read again only when the file changed
    pub fn counted(&self) -> Option<Arc<Counted>> {
        let path = self.path.as_ref()?;
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        let mut read = self.read.lock().unwrap();
        if let Some((at, c)) = read.as_ref()
            && *at == modified
        {
            return Some(c.clone());
        }
        let c: Counted = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let c = Arc::new(c);
        *read = Some((modified, c.clone()));
        Some(c)
    }

    /// uploaded and freed since the ledger counted, for this member
    fn since(&self, user: &str, counted_at: u64) -> (u64, u64) {
        match self.since.lock().unwrap().get(user) {
            Some((at, up, freed)) if *at == counted_at => (*up, *freed),
            _ => (0, 0),
        }
    }

    /// what a member keeps now: the ledger's count, and what changed since
    fn used(&self, c: &Counted, user: &str) -> u64 {
        let (up, freed) = self.since(user, c.updated);
        (c.members.get(user).map(Usage::total).unwrap_or(0) + up).saturating_sub(freed)
    }

    /// their libraries' part of it, for Settings
    pub fn libraries(&self, c: &Counted, user: &str) -> u64 {
        let (up, freed) = self.since(user, c.updated);
        (c.members.get(user).map(|u| u.libraries).unwrap_or(0) + up).saturating_sub(freed)
    }

    /// Room for `more` bytes in this member's libraries? With no ledger (a
    /// box without one, or before its first count) there is no allowance to
    /// hold to, and uploads go ahead. (Uploads ask `hold`, which sets the room
    /// aside; this only asks.)
    #[cfg(test)]
    pub fn room(&self, user: &str, more: u64) -> bool {
        let Some(c) = self.counted() else {
            return true;
        };
        if c.budget == 0 {
            return true;
        }
        let used = self.used(&c, user) + self.held.lock().unwrap().get(user).copied().unwrap_or(0);
        used.saturating_add(more) <= c.budget
    }

    /// Set aside room for an upload: what it says it is (`want`), or, when
    /// it does not say, everything that is left. None: no room for it.
    pub fn hold(&self, user: &str, want: Option<u64>) -> Option<Held> {
        let Some(c) = self.counted().filter(|c| c.budget > 0) else {
            return Some(Held {
                user: user.to_string(),
                limit: None,
            });
        };
        let mut held = self.held.lock().unwrap();
        let busy = held.get(user).copied().unwrap_or(0);
        let used = self.used(&c, user) + busy;
        let left = c.budget.saturating_sub(used);
        let take = match want {
            Some(w) if w <= left => w,
            Some(_) => return None,
            None if left > 0 => left,
            None => return None,
        };
        held.insert(user.to_string(), busy + take);
        Some(Held {
            user: user.to_string(),
            limit: Some(take),
        })
    }

    /// The upload is over: its room is given back, and what landed counted.
    pub fn done(&self, h: Held, landed: u64) {
        if let Some(n) = h.limit {
            let mut held = self.held.lock().unwrap();
            if let Some(b) = held.get_mut(&h.user) {
                *b = b.saturating_sub(n);
            }
        }
        if landed > 0 {
            self.landed(&h.user, landed);
        }
    }

    /// an upload landed: count it until the ledger next does
    pub fn landed(&self, user: &str, bytes: u64) {
        self.change(user, |e| e.1 += bytes);
    }

    /// deleted for good from the trash: room again at once, not at the
    /// ledger's next count
    pub fn freed(&self, user: &str, bytes: u64) {
        self.change(user, |e| e.2 += bytes);
    }

    fn change(&self, user: &str, f: impl FnOnce(&mut (u64, u64, u64))) {
        let Some(c) = self.counted() else { return };
        let mut since = self.since.lock().unwrap();
        let e = since.entry(user.to_string()).or_insert((c.updated, 0, 0));
        if e.0 != c.updated {
            *e = (c.updated, 0, 0);
        }
        f(e);
    }
}

/// GET /internal/storage/members: for the ledger, on this box alone (nginx
/// passes /_dd/ and nothing else): every member, and what their libraries
/// and disk images hold in the bucket, trash included until it is emptied
pub(crate) async fn members(State(app): State<Arc<App>>) -> Response {
    let Ok(listed) = app.directory.list() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let mut out = serde_json::Map::new();
    for l in listed {
        if l.name == pages::DEMO_USER || !app.member(&l.name) || app.guest(&l.name) {
            continue;
        }
        let libs = app
            .directory
            .entry(&l.name)
            .ok()
            .flatten()
            .map(|e| {
                e.entry
                    .libraries
                    .iter()
                    .map(|x| x.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut bytes = 0u64;
        if let Some(g) = app.library.as_ref() {
            // their libraries, and their disk images beside them
            let prefixes = libs
                .into_iter()
                .chain([crate::library::images_prefix(&l.name)]);
            for id in prefixes {
                match g.list_all(&format!("{id}/")).await {
                    Ok(objects) => bytes += objects.iter().map(|o| o.size).sum::<u64>(),
                    Err(e) => {
                        eprintln!("storage: listing library {id}: {e:#}");
                        return StatusCode::BAD_GATEWAY.into_response();
                    }
                }
            }
        }
        out.insert(l.name, serde_json::json!({ "libraries": bytes }));
    }
    Json(serde_json::Value::Object(out)).into_response()
}

#[allow(clippy::result_large_err)]
fn member(app: &App, headers: &HeaderMap) -> Result<String, Response> {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        Some(u) if app.member(&u) && u != pages::DEMO_USER && !app.guest(&u) => Ok(u),
        Some(_) => Err(Redirect::to("/_dd/home").into_response()),
        None => Err(Redirect::to("/_dd/login?rd=/_dd/settings").into_response()),
    }
}

/// GET /_dd/storage/mine: this member's allowance and what fills it
pub(crate) async fn mine(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    // signed in and not a member is a 403, as everywhere else: a 401 would
    // send them to sign in again
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let user = match member(&app, &headers) {
        Ok(u) => u,
        Err(_) if app.signed_in(cookie).is_some() => {
            return StatusCode::FORBIDDEN.into_response();
        }
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let Some(c) = app.storage.counted() else {
        return Json(serde_json::json!({ "counted": false })).into_response();
    };
    let mut u = c.members.get(&user).cloned().unwrap_or_default();
    u.libraries = app.storage.libraries(&c, &user);
    Json(serde_json::json!({
        "counted": true,
        "updated": c.updated,
        "budget": c.budget,
        "used": u.total(),
        "parts": u,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger(json: &str) -> (Ledger, PathBuf) {
        let dir = std::env::temp_dir().join(format!("dd-storage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // one file a test: tests run at once, and each removes its own
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = dir.join(format!("{n}.json"));
        std::fs::write(&p, json).unwrap();
        (Ledger::new(Some(p.clone())), p)
    }

    #[test]
    fn an_upload_fits_the_allowance_or_is_refused() {
        let (l, p) = ledger(
            r#"{"updated": 100, "budget": 1000,
                "members": {"tom": {"photos": 600, "libraries": 200, "code": 50, "passwords": 0}}}"#,
        );
        // 850 used of 1000
        assert!(l.room("tom", 150));
        assert!(!l.room("tom", 151));
        // what lands before the next count is counted meanwhile
        l.landed("tom", 100);
        assert!(l.room("tom", 50));
        assert!(!l.room("tom", 51));
        // what is deleted for good is room again at once
        l.freed("tom", 30);
        assert!(l.room("tom", 80));
        assert!(!l.room("tom", 81));
        l.freed("tom", 1 << 40);
        assert!(l.room("tom", 1000), "never below nothing");
        // someone the ledger has not seen yet starts from nothing
        assert!(l.room("sarah", 1000));
        assert!(!l.room("sarah", 1001));
        std::fs::remove_file(p).unwrap();
    }

    /// Room is set aside while an upload streams: two at once cannot both
    /// have the same room, one that says no size gets what is left and no
    /// more, and what comes back is what landed.
    #[test]
    fn an_upload_holds_its_room_until_it_is_done() {
        let (l, p) = ledger(
            r#"{"updated": 100, "budget": 1000,
                "members": {"tom": {"photos": 600, "libraries": 200, "code": 50, "passwords": 0}}}"#,
        );
        let a = l.hold("tom", Some(100)).expect("100 of the 150 left");
        assert_eq!(a.limit, Some(100));
        assert!(l.hold("tom", Some(51)).is_none(), "the 100 is spoken for");
        let b = l.hold("tom", None).expect("no size: the 50 that is left");
        assert_eq!(b.limit, Some(50));
        assert!(l.hold("tom", None).is_none(), "nothing left at all");
        // b was cut off before it landed anything; a landed 80 of its 100
        l.done(b, 0);
        l.done(a, 80);
        // 850 + 80 used: 70 left
        assert!(l.room("tom", 70));
        assert!(!l.room("tom", 71));
        assert_eq!(l.hold("tom", None).unwrap().limit, Some(70));
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn no_ledger_no_allowance() {
        assert!(Ledger::new(None).room("tom", u64::MAX));
        assert_eq!(Ledger::new(None).hold("tom", None).unwrap().limit, None);
        let (l, p) = ledger(r#"{"updated": 1, "budget": 0, "members": {}}"#);
        assert!(l.room("tom", 1 << 40));
        std::fs::remove_file(p).unwrap();
    }
}
