//! Storage: one allowance per member across every service. The storage
//! ledger (`dd storage-ledger`, a timer on this box) adds up what each
//! member keeps - Photos, their libraries, Code, Passwords - and writes it
//! down; it also holds Photos to what is left. This reads what it wrote, for
//! the Storage page, and holds the libraries to it as uploads arrive: what
//! arrived since the ledger last counted is added as it lands, so the ten
//! minutes between counts is no way past the allowance.

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
    /// bytes uploaded to a member's libraries since the ledger's count
    since: Mutex<HashMap<String, (u64, u64)>>,
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

    /// uploaded since the ledger counted, for this member
    fn uploaded(&self, user: &str, counted_at: u64) -> u64 {
        match self.since.lock().unwrap().get(user) {
            Some((at, n)) if *at == counted_at => *n,
            _ => 0,
        }
    }

    /// Room for `more` bytes in this member's libraries? With no ledger (a
    /// box without one, or before its first count) there is no allowance to
    /// hold to, and uploads go ahead.
    pub fn room(&self, user: &str, more: u64) -> bool {
        let Some(c) = self.counted() else {
            return true;
        };
        if c.budget == 0 {
            return true;
        }
        let used =
            c.members.get(user).map(Usage::total).unwrap_or(0) + self.uploaded(user, c.updated);
        used.saturating_add(more) <= c.budget
    }

    /// an upload landed: count it until the ledger next does
    pub fn landed(&self, user: &str, bytes: u64) {
        let Some(c) = self.counted() else { return };
        let mut since = self.since.lock().unwrap();
        let e = since.entry(user.to_string()).or_insert((c.updated, 0));
        if e.0 != c.updated {
            *e = (c.updated, 0);
        }
        e.1 += bytes;
    }
}

/// GET /internal/storage/members: for the ledger, on this box alone (nginx
/// passes /_dd/ and nothing else): every member, and what their libraries
/// hold in the bucket, trash included until it is emptied
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
            for id in libs {
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
        None => Err(Redirect::to("/_dd/login?rd=/_dd/storage").into_response()),
    }
}

/// GET /_dd/storage: the page
pub(crate) async fn page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match member(&app, &headers) {
        Ok(_) => crate::page("storage"),
        Err(r) => r,
    }
}

/// GET /_dd/storage/mine: this member's allowance and what fills it
pub(crate) async fn mine(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let user = match member(&app, &headers) {
        Ok(u) => u,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let Some(c) = app.storage.counted() else {
        return Json(serde_json::json!({ "counted": false })).into_response();
    };
    let mut u = c.members.get(&user).cloned().unwrap_or_default();
    u.libraries += app.storage.uploaded(&user, c.updated);
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
        let p = dir.join(format!("{}.json", json.len()));
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
        // someone the ledger has not seen yet starts from nothing
        assert!(l.room("sarah", 1000));
        assert!(!l.room("sarah", 1001));
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn no_ledger_no_allowance() {
        assert!(Ledger::new(None).room("tom", u64::MAX));
        let (l, p) = ledger(r#"{"updated": 1, "budget": 0, "members": {}}"#);
        assert!(l.room("tom", 1 << 40));
        std::fs::remove_file(p).unwrap();
    }
}
