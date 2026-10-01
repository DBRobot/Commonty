//! Adding a device by QR code: a signed-in device asks for a code and shows
//! it; the new device - the app, with a key of its own - offers that key
//! against the code; both show the same four digits; the account's main key
//! approves by signing the key into the entry, as any device is added. The
//! box only keeps the pending offer, ten minutes at most, and lets nobody in
//! by itself: a key is in once the entry says so.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::session::{now, random};
use crate::{App, pages};

const TTL: u64 = 600;
/// no 0/O, 1/I/L: read off one screen and typed on another
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

#[derive(Clone)]
struct Pending {
    user: String,
    made: u64,
    offer: Option<Offer>,
}

#[derive(Clone, serde::Serialize)]
struct Offer {
    public_key: String,
    fingerprint: String,
    kind: String,
    digits: String,
}

#[derive(Default)]
pub struct Adding(Mutex<HashMap<String, Pending>>);

fn code() -> String {
    let b = random(6).unwrap_or_default();
    let c: String = b
        .iter()
        .map(|x| ALPHABET[*x as usize % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &c[..3], &c[3..])
}

/// what someone types may have its dash, spaces or lower case
fn normal(raw: &str) -> String {
    let c: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if c.len() == 6 {
        format!("{}-{}", &c[..3], &c[3..])
    } else {
        c
    }
}

fn digits() -> String {
    let b = random(4).unwrap_or_default();
    let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) % 10_000;
    format!("{n:04}")
}

/// members and guests: anyone with devices of their own
fn person(app: &App, headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let user = app.signed_in(cookie)?;
    (user != pages::DEMO_USER && (app.member(&user) || app.guest(&user))).then_some(user)
}

impl Adding {
    fn tidy(m: &mut HashMap<String, Pending>) {
        let t = now();
        m.retain(|_, p| t < p.made + TTL);
    }
}

/// POST /_dd/add/start -> { code, url, expires }
pub(crate) async fn start(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = person(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut m = app.adding.0.lock().unwrap();
    Adding::tidy(&mut m);
    // one at a time for each person
    m.retain(|_, p| p.user != user);
    let c = code();
    m.insert(
        c.clone(),
        Pending {
            user,
            made: now(),
            offer: None,
        },
    );
    Json(serde_json::json!({
        "code": c,
        "url": format!("https://home.{}/add#{}", app.domain, c),
        "expires": now() + TTL,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct CodeQ {
    code: String,
}

/// GET /_dd/add/status?code= - for the device that showed the code
pub(crate) async fn status(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<CodeQ>,
) -> Response {
    let Some(user) = person(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut m = app.adding.0.lock().unwrap();
    Adding::tidy(&mut m);
    match m.get(&normal(&q.code)) {
        Some(p) if p.user == user => match &p.offer {
            Some(o) => Json(serde_json::json!({ "state": "offered", "offer": o })).into_response(),
            None => Json(serde_json::json!({ "state": "waiting" })).into_response(),
        },
        _ => Json(serde_json::json!({ "state": "gone" })).into_response(),
    }
}

/// POST /_dd/add/cancel {code}
pub(crate) async fn cancel(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(q): Json<CodeQ>,
) -> Response {
    let Some(user) = person(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut m = app.adding.0.lock().unwrap();
    m.retain(|c, p| !(p.user == user && *c == normal(&q.code)));
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(crate) struct OfferIn {
    code: String,
    public_key: String,
}

/// POST /_dd/add/offer {code, public_key} - from the new device, which has
/// no account yet: the code is all it has. One offer per code.
pub(crate) async fn offer(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(o): Json<OfferIn>,
) -> Response {
    if identity::decode_public(&o.public_key).is_err() {
        return (StatusCode::BAD_REQUEST, "that is not a device key").into_response();
    }
    let agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let mut m = app.adding.0.lock().unwrap();
    Adding::tidy(&mut m);
    let Some(p) = m.get_mut(&normal(&o.code)) else {
        return (
            StatusCode::NOT_FOUND,
            "that code is not one we know, or it ran out",
        )
            .into_response();
    };
    if p.offer.is_some() {
        return (
            StatusCode::CONFLICT,
            "that code was used already; make a new one",
        )
            .into_response();
    }
    let d = digits();
    p.offer = Some(Offer {
        fingerprint: identity::fingerprint(&o.public_key),
        public_key: o.public_key,
        kind: crate::signins::kind(agent),
        digits: d.clone(),
    });
    Json(serde_json::json!({ "user": p.user, "digits": d })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_easy_to_read_and_type() {
        for _ in 0..50 {
            let c = code();
            assert_eq!(c.len(), 7);
            assert!(
                c.chars()
                    .all(|ch| ch == '-' || ALPHABET.contains(&(ch as u8)))
            );
            assert_eq!(normal(&c.to_lowercase().replace('-', " ")), c);
        }
        assert_eq!(digits().len(), 4);
    }
}
