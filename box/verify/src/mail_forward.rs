//! A member's own email address, kept at Cloudflare and never on this box:
//! the member gives it here once, the gate hands it to the forwarding
//! service (modules/gate/mail-forward.py) on a socket only the gate may open,
//! and from then on mail to <name>@<domain> reaches them. Asked again, the
//! box can say whether forwarding is set, never where it goes.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Redirect, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

use crate::App;
use crate::pages;

async fn ask(socket: &str, req: Value) -> Option<Value> {
    let mut s = tokio::net::UnixStream::connect(socket).await.ok()?;
    s.write_all(format!("{req}\n").as_bytes()).await.ok()?;
    let mut line = String::new();
    tokio::io::BufReader::new(s)
        .read_line(&mut line)
        .await
        .ok()?;
    serde_json::from_str(&line).ok()
}

#[allow(clippy::result_large_err)]
fn member(app: &App, headers: &HeaderMap) -> Result<String, Response> {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
        Some(u) if app.member(&u) && u != pages::DEMO_USER && !app.guest(&u) => Ok(u),
        Some(_) => Err(Redirect::to("/_dd/home").into_response()),
        None => Err(Redirect::to("/_dd/login?rd=/_dd/email").into_response()),
    }
}

/// GET /_dd/email: the page
pub(crate) async fn page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match member(&app, &headers) {
        Ok(_) => crate::page("email"),
        Err(r) => r,
    }
}

/// GET /_dd/email/state: the member's box address, and whether mail to it
/// goes anywhere yet
pub(crate) async fn state(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Ok(user) = member(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(socket) = app.mail_forward.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let said = ask(socket, json!({ "op": "state", "name": user })).await;
    Json(json!({
        "address": format!("{user}@{}", app.domain),
        "forwarding": said.as_ref().and_then(|v| v["forwarding"].as_bool()),
        // the address's owner has pressed Cloudflare's link: only then is
        // anything forwarded
        "confirmed": said.as_ref().and_then(|v| v["confirmed"].as_bool()),
    }))
    .into_response()
}

/// POST /_dd/email/resend: Cloudflare's confirmation, sent again
pub(crate) async fn resend(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let json_body = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("application/json"));
    if !json_body {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(user) = member(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(socket) = app.mail_forward.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match ask(socket, json!({ "op": "resend", "name": user })).await {
        Some(v) if v["ok"].as_bool() == Some(true) => {
            Json(json!({ "confirmed": v["confirmed"].as_bool().unwrap_or(false) })).into_response()
        }
        Some(v) => (
            StatusCode::BAD_GATEWAY,
            v["why"].as_str().unwrap_or("that did not work").to_string(),
        )
            .into_response(),
        None => (
            StatusCode::BAD_GATEWAY,
            "the forwarding service did not answer",
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct Give {
    email: String,
}

/// POST /_dd/email: where the member's mail should go. Handed on and not
/// kept: it is in this process only for the length of the request.
pub(crate) async fn set(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(g): Json<Give>,
) -> Response {
    // a form on another site cannot send json without the browser asking
    // this one first, which it refuses
    let json_body = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("application/json"));
    if !json_body {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(user) = member(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(socket) = app.mail_forward.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let email = g.email.trim();
    if email.len() > 254 || !email.contains('@') || email.contains(char::is_whitespace) {
        return (StatusCode::BAD_REQUEST, "that is not an email address").into_response();
    }
    if email
        .to_ascii_lowercase()
        .ends_with(&format!("@{}", app.domain))
    {
        return (
            StatusCode::BAD_REQUEST,
            "that is an address on this network; give the one you read mail at",
        )
            .into_response();
    }
    match ask(socket, json!({ "op": "set", "name": user, "email": email })).await {
        Some(v) if v["ok"].as_bool() == Some(true) => {
            Json(json!({ "confirm": v["confirm"].as_bool().unwrap_or(false) })).into_response()
        }
        Some(v) => (
            StatusCode::BAD_GATEWAY,
            v["why"].as_str().unwrap_or("that did not work").to_string(),
        )
            .into_response(),
        None => (
            StatusCode::BAD_GATEWAY,
            "the forwarding service did not answer",
        )
            .into_response(),
    }
}
