//! Ad blocking at home (modules/adblock): Pi-hole on the house's main box,
//! for the house network. Its own api listens on this box alone and wants
//! a password only the gate holds; the gate is its only door, and only the
//! household, signed in, may use it. Pi-hole keeps counts and nothing about who looked up what, so all
//! there is to show is totals, the lists, and the sites let through.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::App;

/// Pi-hole's api on this box, and who may switch it
#[derive(Clone, Debug)]
pub struct Adblock {
    pub api: String,
    pub household: Vec<String>,
    /// the box's address on the house network: what the router is given
    pub lan: String,
    /// where the household's on/off is kept: Pi-hole's own settings are
    /// written afresh by every release, which would switch it back on
    pub remember: Option<std::path::PathBuf>,
    /// Pi-hole's api password (sops, handed to the gate alone)
    pub password: Option<String>,
}

/// set once the household has used the switch since the gate started: the
/// start-up restore must not undo what they just chose
static SWITCHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// the gate's session with Pi-hole, while it lasts
static SID: std::sync::Mutex<Option<(String, std::time::Instant)>> = std::sync::Mutex::new(None);

async fn sign_in(ab: &Adblock) -> Option<String> {
    let pw = ab.password.as_ref()?;
    let r = match crate::http()
        .post(format!("{}/api/auth", ab.api))
        .timeout(Duration::from_secs(10))
        .json(&json!({ "password": pw }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("adblock: Pi-hole's api did not answer: {e}");
            return None;
        }
    };
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    let Some(sid) = v["session"]["sid"].as_str().map(str::to_string) else {
        eprintln!(
            "adblock: Pi-hole refused the gate's password ({status}): {}",
            v["session"]["message"]
        );
        return None;
    };
    // a minute short of what Pi-hole allows, so it never lapses mid-call
    let secs = v["session"]["validity"]
        .as_u64()
        .unwrap_or(300)
        .saturating_sub(60)
        .max(30);
    let until = std::time::Instant::now() + Duration::from_secs(secs);
    if let Ok(mut g) = SID.lock() {
        *g = Some((sid.clone(), until));
    }
    Some(sid)
}

async fn sid(ab: &Adblock, fresh: bool) -> Option<String> {
    if !fresh
        && let Ok(g) = SID.lock()
        && let Some((s, until)) = g.as_ref()
        && std::time::Instant::now() < *until
    {
        return Some(s.clone());
    }
    sign_in(ab).await
}

impl Adblock {
    pub fn allows(&self, user: &str) -> bool {
        self.household.iter().any(|h| h == user)
    }
}

/// Pi-hole writes its lists without waiting for a lock, so a write that lands
/// in its once-a-second check of the file is 400 "database is locked":
/// nothing written, safe to ask again
fn busy(status: reqwest::StatusCode, body: &Value) -> bool {
    status == reqwest::StatusCode::BAD_REQUEST
        && body["error"]["key"] == "database_error"
        && body["error"]["hint"]
            .as_str()
            .is_some_and(|h| h.contains("database is locked"))
}

async fn ftl(
    ab: &Adblock,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, StatusCode> {
    let mut fresh = false;
    // that read takes a few milliseconds; give it up to about five seconds
    let mut waits = [20u64, 50, 100, 200, 400, 800, 1600, 1600].into_iter();
    loop {
        let mut r = crate::http()
            .request(method.clone(), format!("{}/api{path}", ab.api))
            .timeout(Duration::from_secs(10));
        if ab.password.is_some() {
            let Some(s) = sid(ab, fresh).await else {
                return Err(StatusCode::BAD_GATEWAY);
            };
            r = r.header("X-FTL-SID", s);
        }
        if let Some(b) = &body {
            r = r.json(b);
        }
        let r = r.send().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
        let status = r.status();
        // the session lapsed or Pi-hole restarted: sign in again, once
        if status == reqwest::StatusCode::UNAUTHORIZED && !fresh && ab.password.is_some() {
            fresh = true;
            continue;
        }
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        let text = r.text().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status.is_success() {
            return if v.is_null() {
                Err(StatusCode::BAD_GATEWAY)
            } else {
                Ok(v)
            };
        }
        if busy(status, &v)
            && let Some(ms) = waits.next()
        {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            continue;
        }
        eprintln!("adblock: Pi-hole said {status} to {method} {path}: {text}");
        return Err(StatusCode::BAD_GATEWAY);
    }
}

/// the household member asking, or why not
fn who(app: &App, headers: &HeaderMap) -> Result<(String, Adblock), StatusCode> {
    let Some(ab) = app.adblock.clone() else {
        return Err(StatusCode::NOT_FOUND);
    };
    // a device's token too: the bar's switch on a page the app serves
    match app.identify(headers, "access") {
        Some(u) if app.member(&u) && ab.allows(&u) => Ok((u, ab)),
        Some(_) => Err(StatusCode::FORBIDDEN),
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

/// A change asked for by this site's own page: a form another site posts
/// cannot set this type without the browser asking first, which it refuses.
fn from_our_page(headers: &HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("application/json"))
}

/// GET /_dd/adblock: the page
pub(crate) async fn page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match who(&app, &headers) {
        Ok(_) => crate::page("adblock"),
        Err(StatusCode::UNAUTHORIZED) => Redirect::to("/_dd/login?rd=/_dd/adblock").into_response(),
        Err(_) => Redirect::to("/_dd/home").into_response(),
    }
}

/// GET /_dd/adblock/state: on or off (and until when), today's totals, the
/// lists, the sites let through. `?brief` is the menu's: the switch alone.
pub(crate) async fn state(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let (_, ab) = match who(&app, &headers) {
        Ok(x) => x,
        Err(s) => return s.into_response(),
    };
    let blocking = match ftl(&ab, reqwest::Method::GET, "/dns/blocking", None).await {
        Ok(v) => v,
        Err(s) => return s.into_response(),
    };
    let on = blocking["blocking"].as_str() == Some("enabled");
    // seconds until a pause ends by itself; none when switched off outright
    let timer = blocking["timer"].as_f64();
    let mut out = json!({ "on": on, "resumesIn": timer, "lan": ab.lan });
    if q.contains_key("brief") {
        return Json(out).into_response();
    }
    let (summary, history, lists, allowed) = tokio::join!(
        ftl(&ab, reqwest::Method::GET, "/stats/summary", None),
        ftl(&ab, reqwest::Method::GET, "/history", None),
        ftl(&ab, reqwest::Method::GET, "/lists?type=block", None),
        ftl(&ab, reqwest::Method::GET, "/domains/allow/exact", None),
    );
    let summary = summary.unwrap_or(Value::Null);
    out["today"] = json!({
        "total": summary["queries"]["total"],
        "blocked": summary["queries"]["blocked"],
        "percent": summary["queries"]["percent_blocked"],
    });
    out["sites"] = summary["gravity"]["domains_being_blocked"].clone();
    out["listsUpdated"] = summary["gravity"]["last_update"].clone();
    out["history"] = history
        .map(|h| {
            Value::Array(
                h["history"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|b| json!({ "t": b["timestamp"], "total": b["total"], "blocked": b["blocked"] }))
                    .collect(),
            )
        })
        .unwrap_or(Value::Array(vec![]));
    out["lists"] = lists
        .map(|l| {
            Value::Array(
                l["lists"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|x| json!({ "name": x["comment"], "address": x["address"], "count": x["number"], "updated": x["date_updated"] }))
                    .collect(),
            )
        })
        .unwrap_or(Value::Array(vec![]));
    out["allowed"] = allowed
        .map(|a| {
            Value::Array(
                a["domains"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|x| json!({ "domain": x["domain"], "by": x["comment"], "added": x["date_added"] }))
                    .collect(),
            )
        })
        .unwrap_or(Value::Array(vec![]));
    Json(out).into_response()
}

#[derive(Deserialize)]
pub(crate) struct Switch {
    /// on, or off until switched back
    on: Option<bool>,
    /// off for this long, then on again by itself
    pause_minutes: Option<u32>,
}

/// POST /_dd/adblock/switch: the menu's switch and the page's pause
pub(crate) async fn switch(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(s): Json<Switch>,
) -> Response {
    if !from_our_page(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (_, ab) = match who(&app, &headers) {
        Ok(x) => x,
        Err(s) => return s.into_response(),
    };
    SWITCHED.store(true, std::sync::atomic::Ordering::SeqCst);
    let body = match (s.on, s.pause_minutes) {
        (_, Some(m)) if (1..=24 * 60).contains(&m) => json!({ "blocking": false, "timer": m * 60 }),
        (Some(on), None) => json!({ "blocking": on, "timer": null }),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    match ftl(&ab, reqwest::Method::POST, "/dns/blocking", Some(body)).await {
        Ok(v) if s.pause_minutes.is_none() => {
            if let (Some(p), Some(on)) = (&ab.remember, s.on) {
                let _ = std::fs::write(p, if on { "on\n" } else { "off\n" });
            }
            Json(
                json!({ "on": v["blocking"].as_str() == Some("enabled"), "resumesIn": v["timer"] }),
            )
            .into_response()
        }
        Ok(v) => Json(
            json!({ "on": v["blocking"].as_str() == Some("enabled"), "resumesIn": v["timer"] }),
        )
        .into_response(),
        Err(s) => s.into_response(),
    }
}

/// At start: what the household last chose, put back. Pi-hole comes up
/// blocking after every release; a house that switched it off stays off.
pub async fn restore(ab: Adblock) {
    let Some(p) = &ab.remember else { return };
    let Ok(said) = std::fs::read_to_string(p) else {
        return;
    };
    if said.trim() != "off" {
        return;
    }
    // Pi-hole may still be starting: ask for a minute, unless the household
    // switches it themselves first
    for _ in 0..30 {
        if SWITCHED.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if ftl(
            &ab,
            reqwest::Method::POST,
            "/dns/blocking",
            Some(json!({ "blocking": false, "timer": null })),
        )
        .await
        .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// a name as a person types it, made into the one domain it means
fn domain(input: &str) -> Option<String> {
    let d = input
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let d = d
        .split(['/', '?', '#', ':'])
        .next()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let ok = !d.is_empty()
        && d.len() <= 253
        && d.contains('.')
        && d.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    ok.then_some(d)
}

#[derive(Deserialize)]
pub(crate) struct Allow {
    domain: String,
}

/// POST /_dd/adblock/allow: a site blocking broke, let through for the house
pub(crate) async fn allow(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(a): Json<Allow>,
) -> Response {
    if !from_our_page(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (user, ab) = match who(&app, &headers) {
        Ok(x) => x,
        Err(s) => return s.into_response(),
    };
    let Some(d) = domain(&a.domain) else {
        return (StatusCode::BAD_REQUEST, "that is not a site's name").into_response();
    };
    match ftl(
        &ab,
        reqwest::Method::POST,
        "/domains/allow/exact",
        Some(json!({ "domain": d, "comment": user })),
    )
    .await
    {
        Ok(_) => Json(json!({ "domain": d })).into_response(),
        Err(s) => s.into_response(),
    }
}

/// DELETE /_dd/adblock/allow/{domain}: blocked again
pub(crate) async fn unallow(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(d): Path<String>,
) -> Response {
    let (_, ab) = match who(&app, &headers) {
        Ok(x) => x,
        Err(s) => return s.into_response(),
    };
    let Some(d) = domain(&d) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match ftl(
        &ab,
        reqwest::Method::DELETE,
        &format!("/domains/allow/exact/{d}"),
        None,
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(s) => s.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::{busy, domain};
    use reqwest::StatusCode;
    use serde_json::json;

    #[test]
    fn a_site_as_people_type_it() {
        assert_eq!(
            domain("https://Shop.Example.com/cart?x=1").as_deref(),
            Some("shop.example.com")
        );
        assert_eq!(domain("s.shopify.com.").as_deref(), Some("s.shopify.com"));
        assert_eq!(domain("localhost"), None);
        assert_eq!(domain("a b.com"), None);
        assert_eq!(domain("-x.com"), None);
        assert_eq!(domain(""), None);
    }

    #[test]
    fn only_a_locked_file_is_asked_again() {
        let locked = json!({ "error": { "key": "database_error", "message": "Could not add to gravity database", "hint": "database is locked" } });
        assert!(busy(StatusCode::BAD_REQUEST, &locked));
        let dup = json!({ "error": { "key": "database_error", "message": "Could not add to gravity database", "hint": "UNIQUE constraint failed: domainlist.domain, domainlist.type" } });
        assert!(!busy(StatusCode::BAD_REQUEST, &dup));
        let bad =
            json!({ "error": { "key": "bad_request", "message": "Invalid domain", "hint": "x" } });
        assert!(!busy(StatusCode::BAD_REQUEST, &bad));
        assert!(!busy(StatusCode::INTERNAL_SERVER_ERROR, &locked));
    }
}
