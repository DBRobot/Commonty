//! Photos: the step between the site and Ente's own app. The person's
//! passkey makes the photo account's password in the page (the page's
//! wasm, web/photos.js), which opens or makes their account and hands Ente's
//! app a session. Signing out of the site clears what that app keeps in
//! the browser. The page itself lives with Photos (box/photos).

use std::sync::Arc;
use std::time::{Duration, Instant};

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Json, Redirect, Response};
use serde::Deserialize;

use crate::App;
use crate::pages::{self, Menu, Service, initial, render};

#[derive(Template)]
#[template(path = "photos.html")]
struct Photos<'a> {
    user: &'a str,
    initial: String,
    menu: Menu,
}

/// Photos: opened by the passkey, or by the demo's password.
fn sheet(user: &str, services: &[Service]) -> String {
    render(Photos {
        user,
        initial: initial(user),
        menu: Menu::of(user, services),
    })
}

/// Photos, opened with the passkey: the page runs our wasm against ente.
pub(crate) async fn page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
        // the demo has no passkey: the page gets its password from the config
        Some(user) if user == pages::DEMO_USER => match &app.photos {
            Some(p) if p.demo_password.is_some() => Html(sheet(&user, &app.home)).into_response(),
            _ => Redirect::to("/_dd/home").into_response(),
        },
        Some(user) if app.member(&user) && app.photos.is_some() => {
            Html(sheet(&user, &app.home)).into_response()
        }
        Some(_) => Redirect::to("/_dd/home").into_response(),
        // back to exactly this after signing in: a handoff link keeps its
        // slot and the app's key
        None => {
            let here = headers
                .get("x-original-uri")
                .and_then(|v| v.to_str().ok())
                .filter(|u| u.starts_with("/_dd/photos"))
                .unwrap_or("/_dd/photos");
            Redirect::to(&format!("/_dd/login?rd={}", crate::urlencode(here))).into_response()
        }
    }
}

/// What the photos page needs, for a member with a session: museum's
/// address, this person's address there, the passkey relying party, and the
/// verification code museum takes for addresses of ours.
pub(crate) async fn config(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let Some(user) = app.sessions.user(cookie) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !app.member(&user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(p) = &app.photos else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut cfg = serde_json::json!({
        "api": p.api,
        // where the page makes or links the account: museum, on this
        // site's own name, so the two calls that need the fleet's code
        // come here (museum_verify) and the code never reaches a browser
        "accountApi": "/_dd/museum",
        "email": format!("{user}{}", p.email_suffix),
        "rpId": app.domain,
    });
    if user == pages::DEMO_USER {
        // the demo has an account already and never makes one
        let Some(pw) = &p.demo_password else {
            return StatusCode::FORBIDDEN.into_response();
        };
        cfg["password"] = serde_json::Value::String(pw.clone());
    }
    Json(cfg).into_response()
}

/// POST /_dd/photos/museum/{verify-email,change-email}: the two museum calls
/// that carry the fleet's verification code, made for a member and only for
/// their own address. The page sends them without a code; this puts it in
/// and hands museum's answer back as it came. The code is one value for the
/// whole fleet - whoever held it could claim any address at users.<domain> -
/// so it stays on this box.
pub(crate) async fn museum_verify(
    State(app): State<Arc<App>>,
    Path(op): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<serde_json::Value>,
) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let Some(user) = app.sessions.user(cookie) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !app.member(&user) || user == pages::DEMO_USER {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(p) = &app.photos else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if op != "verify-email" && op != "change-email" {
        return StatusCode::NOT_FOUND.into_response();
    }
    if body["email"].as_str() != Some(format!("{user}{}", p.email_suffix).as_str()) {
        return (StatusCode::FORBIDDEN, "only your own address").into_response();
    }
    body["ott"] = serde_json::Value::String(p.code.clone());
    let mut r = reqwest::Client::new()
        .post(format!("{}/users/{op}", p.api.trim_end_matches('/')))
        .timeout(Duration::from_secs(20))
        .json(&body);
    // change-email acts on the account the person is signed in to
    for h in ["x-auth-token", "x-client-package", "x-client-version"] {
        if let Some(v) = headers.get(h) {
            r = r.header(h, v);
        }
    }
    match r.send().await {
        Ok(res) => {
            let status =
                StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = res.bytes().await.unwrap_or_default();
            (status, [("content-type", "application/json")], bytes).into_response()
        }
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

/// the photos host, from its tile: https://photos.<domain>
pub(crate) fn origin(home: &[pages::Service]) -> Option<String> {
    home.iter()
        .find_map(|s| s.url.strip_suffix("/_dd/photos").map(str::to_string))
}

#[derive(Deserialize)]
pub(crate) struct Forget {
    then: Option<String>,
}

/// The photo app's storage in this browser, cleared by the browser itself
/// (Clear-Site-Data), then on to the host the person signed out from.
/// Storage, not the http cache: what the photo app decrypted lives in its
/// storage, the cache holds the same public files for everyone and
/// ciphertext, and Chrome held sign-out for seconds emptying it.
pub(crate) async fn forget(State(app): State<Arc<App>>, Query(q): Query<Forget>) -> Response {
    let then = q
        .then
        .filter(|t| {
            t.strip_prefix("https://")
                .and_then(|r| r.strip_suffix('/'))
                .is_some_and(|h| crate::fleet_host(&app.domain, h))
        })
        .unwrap_or_else(|| "/".to_string());
    (
        [
            ("clear-site-data", "\"storage\""),
            ("cache-control", "no-store"),
            ("content-type", "text/html; charset=utf-8"),
        ],
        format!("<!doctype html><meta http-equiv=\"refresh\" content=\"0;url={then}\"><title>Signed out</title>"),
    )
        .into_response()
}

// ---- Photos in the app
//
// The photo account's password is made by the person's passkey, which an
// app's window cannot use. So the app asks the browser: it opens a slot
// here, signing as the device it is, and the browser - signed in, with the
// passkey - fills it with the password sealed to a key only the app holds.
// The app takes it once. The gate holds what it cannot read, for minutes.

/// how long a slot waits for the browser
const HANDOFF: Duration = Duration::from_secs(600);
/// slots at once, all people together: each is a few hundred bytes
const HANDOFFS: usize = 256;

pub(crate) struct Handoff {
    user: String,
    sealed: Option<String>,
    until: Instant,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// a slot for this device's person
pub(crate) async fn handoff_open(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !app.member(&user) || user == pages::DEMO_USER {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(raw) = crate::session::random(16) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let id = hex(&raw);
    let mut slots = app.handoffs.lock().unwrap();
    slots.retain(|_, h| h.until > Instant::now());
    if slots.len() >= HANDOFFS {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    slots.insert(
        id.clone(),
        Handoff {
            user,
            sealed: None,
            until: Instant::now() + HANDOFF,
        },
    );
    Json(serde_json::json!({ "id": id })).into_response()
}

/// the browser's part: the password, sealed to the app's key
pub(crate) async fn handoff_fill(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let Some(user) = app.sessions.user(cookie) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if body.is_empty() || body.len() > 1024 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut slots = app.handoffs.lock().unwrap();
    match slots.get_mut(&id) {
        // someone else's slot is no slot at all, as far as anyone can tell
        Some(h) if h.user == user && h.until > Instant::now() => {
            if h.sealed.is_some() {
                return StatusCode::CONFLICT.into_response();
            }
            h.sealed = Some(body);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// the app's part: what the browser left, once; nothing yet is 204
pub(crate) async fn handoff_take(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut slots = app.handoffs.lock().unwrap();
    let ready = match slots.get(&id) {
        Some(h) if h.user == user && h.until > Instant::now() => h.sealed.is_some(),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    if !ready {
        return StatusCode::NO_CONTENT.into_response();
    }
    let sealed = slots.remove(&id).and_then(|h| h.sealed).unwrap_or_default();
    ([("cache-control", "no-store")], sealed).into_response()
}

#[derive(Deserialize)]
pub(crate) struct AppSignin {
    token: String,
}

/// The app's window signed in as the person the app's device signs for: its
/// own page posts the device's token here, and the answer is the session a
/// browser gets from a passkey. Only from the app's own page: the app's
/// window runs no one else's form.
pub(crate) async fn app_signin(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Form(f): Form<AppSignin>,
) -> Response {
    let origin = headers.get("origin").and_then(|v| v.to_str().ok());
    if !matches!(
        origin,
        None | Some("null" | "commonty://localhost" | "http://commonty.localhost")
    ) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let user = match app.verify_biscuit_for(&f.token, "access", &crate::asked_host(&headers)) {
        Ok(u) => u,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if !app.member(&user) || user == pages::DEMO_USER {
        return StatusCode::FORBIDDEN.into_response();
    }
    // on to the Photos step; the browser keeps the fragment the app's page
    // put on this address, which is where the password rides, never here
    let mut r = Redirect::to("/_dd/photos").into_response();
    if let Ok(c) = HeaderValue::from_str(&app.sessions.issue(&user)) {
        r.headers_mut().insert("set-cookie", c);
    }
    r
}
