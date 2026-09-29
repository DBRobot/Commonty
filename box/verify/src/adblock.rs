//! Ad blocking at home (modules/adblock): Pi-hole on the house's main box,
//! for the house network. Its own api listens on this box alone and asks
//! for nothing; the gate is its only door, and only the household may use
//! it. Pi-hole keeps counts and nothing about who looked up what, so all
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
}

impl Adblock {
    pub fn allows(&self, user: &str) -> bool {
        self.household.iter().any(|h| h == user)
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

async fn ftl(
    ab: &Adblock,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, StatusCode> {
    let mut r = client().request(method, format!("{}/api{path}", ab.api));
    if let Some(b) = body {
        r = r.json(&b);
    }
    let r = r.send().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    if !r.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    if r.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    r.json().await.map_err(|_| StatusCode::BAD_GATEWAY)
}

/// the household member asking, or why not
fn who(app: &App, headers: &HeaderMap) -> Result<(String, Adblock), StatusCode> {
    let Some(ab) = app.adblock.clone() else {
        return Err(StatusCode::NOT_FOUND);
    };
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
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
    // Pi-hole may still be starting: ask for a minute
    for _ in 0..30 {
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
    use super::domain;

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
}
