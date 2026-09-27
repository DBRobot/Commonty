//! Photos: the step between the site and Ente's own app. The person's
//! passkey makes the photo account's password in the page (the page's
//! wasm, web/photos.js), which opens or makes their account and hands Ente's
//! app a session. Signing out of the site clears what that app keeps in
//! the browser. The page itself lives with Photos (box/photos).

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
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
        None => Redirect::to("/_dd/login?rd=/_dd/photos").into_response(),
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
        "email": format!("{user}{}", p.email_suffix),
        "rpId": app.domain,
    });
    if user == pages::DEMO_USER {
        // the demo has an account already and never makes one, so it has
        // no use for the code. The code is one value for the whole fleet:
        // whoever holds it can verify an address at users.<domain> that is
        // not theirs, and the demo is the one session anybody may open.
        let Some(pw) = &p.demo_password else {
            return StatusCode::FORBIDDEN.into_response();
        };
        cfg["password"] = serde_json::Value::String(pw.clone());
    } else {
        cfg["code"] = serde_json::Value::String(p.code.clone());
    }
    Json(cfg).into_response()
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
            ("clear-site-data", "\"cache\", \"storage\""),
            ("cache-control", "no-store"),
            ("content-type", "text/html; charset=utf-8"),
        ],
        format!("<!doctype html><meta http-equiv=\"refresh\" content=\"0;url={then}\"><title>Signed out</title>"),
    )
        .into_response()
}
