//! One sign-in for every box (identity::session). This box sends a browser
//! with no session to the id page with a challenge of its own; the page
//! comes back with the passkey's delegation and the browser key's
//! signature over this box's name and that challenge. Checked here against
//! this box's own copy of the directory, it becomes a session cookie for
//! this name alone, signed with this box's own secret: nothing another box
//! is handed opens anything here, and nothing handed here opens another.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Form;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use identity::session::{Delegation, check_delegation, check_handoff};

use crate::{App, session, urlencode, valid_user};

/// how long a box's challenge waits for the id page to answer it
const WAIT: Duration = Duration::from_secs(300);

/// A challenge this box gave out: for which of its names, and where the
/// browser was going.
pub(crate) struct Waiting {
    at: Instant,
    host: String,
    rd: String,
}

/// Where a browser may be sent after: a path on this name. Anything that
/// is not plainly one - another host, a protocol-relative "//", a "/\" a
/// browser reads as one, a line break - is the front page instead.
pub(crate) fn local_path(rd: &str) -> String {
    let ok = rd.starts_with('/')
        && !rd.starts_with("//")
        && !rd.starts_with("/\\")
        && !rd.bytes().any(|b| b.is_ascii_control() || b == b'\\');
    if ok { rd.to_string() } else { "/".to_string() }
}

#[derive(serde::Deserialize)]
pub(crate) struct Start {
    #[serde(default)]
    rd: String,
}

/// GET /_dd/sso/start?rd=: to the id page, with a challenge for this name.
pub(crate) async fn start(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<Start>,
) -> Response {
    let Some(id) = app.id_origin.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let host = app.directory.here(&headers);
    if host.is_empty() {
        return (StatusCode::BAD_REQUEST, "not one of this box's names").into_response();
    }
    let nonce = session::random_id();
    {
        let mut w = app.sso.lock().unwrap();
        w.retain(|_, v: &mut Waiting| v.at.elapsed() < WAIT);
        w.insert(
            nonce.clone(),
            Waiting {
                at: Instant::now(),
                host: host.clone(),
                rd: local_path(&q.rd),
            },
        );
    }
    Redirect::to(&format!(
        "{id}/go?host={}&nonce={}",
        urlencode(&host),
        urlencode(&nonce)
    ))
    .into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct Creds {
    user: String,
}

/// GET /_dd/sso/creds?user=: the credential ids the id page offers the
/// browser for this name, as this box's directory has them - what a
/// sign-in here has always told anyone who typed the name.
pub(crate) async fn creds(State(app): State<Arc<App>>, Query(q): Query<Creds>) -> Response {
    let Some(id) = app.id_origin.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let user = q.user.trim().to_lowercase();
    let ids: Vec<String> = if valid_user(&user) {
        app.entry(&user)
            .ok()
            .flatten()
            .map(|e| e.entry.passkeys.into_iter().map(|p| p.id).collect())
            .unwrap_or_default()
    } else {
        vec![]
    };
    let mut r = axum::Json(serde_json::json!({ "ids": ids })).into_response();
    let h = r.headers_mut();
    h.insert(
        "access-control-allow-origin",
        HeaderValue::from_str(id).unwrap(),
    );
    h.insert("vary", HeaderValue::from_static("origin"));
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    r
}

#[derive(serde::Deserialize)]
pub(crate) struct Back {
    /// the delegation, json, base64url
    d: String,
    /// the browser key's signature over this name and the challenge
    sig: String,
    nonce: String,
}

/// POST /_dd/sso: the id page's answer. A session for this name, or nothing.
pub(crate) async fn finish(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Form(b): Form<Back>,
) -> Response {
    let Some(id) = app.id_origin.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // posted by the id page and nothing else
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(id) {
        return (StatusCode::FORBIDDEN, "not from the sign-in page").into_response();
    }
    let host = app.directory.here(&headers);
    let Some(w) = app.sso.lock().unwrap().remove(&b.nonce) else {
        return start_over("/");
    };
    if w.at.elapsed() > WAIT || w.host != host || host.is_empty() {
        return start_over(&w.rd);
    }
    let Some(d) = B64_URL
        .decode(b.d.as_bytes())
        .ok()
        .and_then(|j| serde_json::from_slice::<Delegation>(&j).ok())
    else {
        return (StatusCode::BAD_REQUEST, "not a sign-in").into_response();
    };
    let now = session::now();
    let checked = valid_user(&d.user)
        .then(|| app.entry(&d.user).ok().flatten())
        .flatten()
        .ok_or_else(|| "no such name here".to_string())
        .and_then(|e| check_delegation(&e.entry, &d, id, now).map_err(|e| e.to_string()))
        .and_then(|()| check_handoff(&d, &host, &b.nonce, &b.sig).map_err(|e| e.to_string()))
        .and_then(|()| {
            (!app.signins.is_ended(&session_id(&d.user, &d.sid)))
                .then_some(())
                .ok_or_else(|| "signed out".into())
        });
    if let Err(why) = checked {
        eprintln!("sso: {}: refused: {why}", d.user);
        return (
            StatusCode::UNAUTHORIZED,
            "That sign-in was not accepted here. Sign in again.",
        )
            .into_response();
    }
    let agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    app.signins
        .used(&d.user, &format!("passkey:{}", d.passkey), agent);
    let exp = d.exp.min(now + session::TTL);
    // the browser chose the sign-in's id; this box's own is made from it and
    // the person, so nobody's sign-in can take the id of anyone else's here
    let id = session_id(&d.user, &d.sid);
    let set = app.sessions.issue_as(&d.user, &id, exp);
    app.signins.saw(&d.user, &id, exp, agent, None);
    // the passkey was shown just now, not hours ago from a kept sign-in
    if now.saturating_sub(d.iat) < 300 {
        app.saw_passkey(&id);
    }
    let mut r = Redirect::to(&w.rd).into_response();
    r.headers_mut()
        .insert("set-cookie", HeaderValue::from_str(&set).unwrap());
    r
}

/// This box's id for a person's fleet sign-in: the same on every box for
/// the same sign-in (so ending it can reach them all), and nobody else's.
pub(crate) fn session_id(user: &str, sid: &str) -> String {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(format!("commonty session id v1\0{user}\0{sid}"));
    B64_URL.encode(&h[..18])
}

/// A challenge gone stale or used: ask again, from where the browser was going.
fn start_over(rd: &str) -> Response {
    Redirect::to(&format!("/_dd/sso/start?rd={}", urlencode(rd))).into_response()
}

#[cfg(test)]
mod tests {
    use super::local_path;

    #[test]
    fn a_sign_in_id_is_the_persons_own() {
        use super::session_id;
        assert_eq!(session_id("tom", "abc"), session_id("tom", "abc"));
        assert_ne!(
            session_id("tom", "abc"),
            session_id("ann", "abc"),
            "another's sign-in, another id"
        );
        assert_ne!(session_id("tom", "abc"), session_id("tom", "abd"));
    }

    #[test]
    fn only_a_path_on_this_name() {
        for (rd, want) in [
            ("/_dd/files?x=1", "/_dd/files?x=1"),
            ("/", "/"),
            ("", "/"),
            ("//evil.test", "/"),
            ("/\\evil.test", "/"),
            ("https://evil.test", "/"),
            ("/a\r\nSet-Cookie: x", "/"),
            ("/a\\b", "/"),
        ] {
            assert_eq!(local_path(rd), want, "{rd:?}");
        }
    }
}
