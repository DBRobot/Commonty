//! What the controller runs, and the household's hold on updates; and the
//! controller key's word on the certificate it serves, which the app holds
//! the door to (an ACME certificate cannot carry an ed25519 key).

use crate::{App, refuse, same_origin};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ed25519_dalek::Signer as _;
use serde_json::json;
use sha2::Digest as _;
use std::sync::Arc;

/// `commonty door tls v1\0<host>\0<hex sha256 of the leaf certificate, DER>`
pub fn tls_message(host: &str, cert_der: &[u8]) -> Vec<u8> {
    let h: String = sha2::Sha256::digest(cert_der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("commonty door tls v1\0{host}\0{h}").into_bytes()
}

/// GET /_door/tls: {sig} over the certificate this door serves. Anyone may
/// ask; it says only what the certificate already says, signed.
pub async fn tls(State(app): State<Arc<App>>) -> Response {
    use base64::Engine as _;
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject as _;
    let (Some(name), Some((cert, _))) = (app.name(), app.cfg.tls.clone()) else {
        return refuse(StatusCode::NOT_FOUND, "this door has no certificate");
    };
    let Some(leaf) = CertificateDer::pem_file_iter(&cert)
        .ok()
        .and_then(|mut i| i.next())
        .and_then(Result::ok)
    else {
        return refuse(StatusCode::NOT_FOUND, "this door has no certificate yet");
    };
    let sig = app.key.sign(&tls_message(&name, leaf.as_ref()));
    Json(json!({ "sig": base64::engine::general_purpose::STANDARD.encode(sig.to_bytes()) }))
        .into_response()
}

fn release(app: &App) -> Option<u64> {
    std::fs::read_to_string(app.cfg.agent_state.join("counter"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn hold_file(app: &App) -> std::path::PathBuf {
    app.cfg.agent_state.join("hold")
}

/// GET /_door/status, for someone in: the release it runs, whether its
/// updates are held, and how the last backup went ({at, ok, error?}, at in
/// unix seconds)
pub async fn status(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match app.who(&headers) {
        Some((_, true)) => {}
        Some(_) => return refuse(StatusCode::FORBIDDEN, "you are not in yet"),
        None => return refuse(StatusCode::UNAUTHORIZED, "sign in first"),
    }
    let release = release(&app).map(|n| format!("release {n}"));
    let updates = if hold_file(&app).exists() {
        "hold"
    } else {
        "auto"
    };
    let backup = app.with_store(|s| s.state.backup.clone());
    Json(json!({ "release": release, "updates": updates, "backup": backup })).into_response()
}

#[derive(serde::Deserialize)]
pub struct Updates {
    hold: bool,
}

/// POST /_door/updates {hold}: someone in holds the controller's updates, or
/// lets them go. The release agent takes nothing while the hold is there.
pub async fn set(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(u): Json<Updates>,
) -> Response {
    if !same_origin(&app, &headers) {
        return refuse(StatusCode::FORBIDDEN, "not from this door's page");
    }
    match app.who(&headers) {
        Some((_, true)) => {}
        Some(_) => return refuse(StatusCode::FORBIDDEN, "you are not in yet"),
        None => return refuse(StatusCode::UNAUTHORIZED, "sign in first"),
    }
    let f = hold_file(&app);
    let done = if u.hold {
        std::fs::create_dir_all(&app.cfg.agent_state)
            .and_then(|_| std::fs::write(&f, b"held at the door\n"))
    } else {
        match std::fs::remove_file(&f) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    };
    if let Err(e) = done {
        eprintln!("updates: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let release = release(&app).map(|n| format!("release {n}"));
    Json(json!({ "release": release, "updates": if u.hold { "hold" } else { "auto" } }))
        .into_response()
}

#[cfg(test)]
mod tests {
    use crate::tests::*;

    #[tokio::test]
    async fn someone_in_holds_the_updates_and_lets_them_go() {
        let r = rig("updates");
        let url = serve(crate::router(r.app.clone())).await;
        std::fs::create_dir_all(&r.app.cfg.agent_state).unwrap();
        std::fs::write(r.app.cfg.agent_state.join("counter"), "41\n").unwrap();
        let c = client();
        // nobody signed in: nothing
        assert_eq!(
            c.get(format!("{url}/_door/status"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let (_, _, cookie) = person(&r.app, "ann");
        let origin = r.app.origin().unwrap();
        let st: serde_json::Value = c
            .get(format!("{url}/_door/status"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(st["release"], "release 41");
        assert_eq!(st["updates"], "auto");
        let set = |hold: bool| {
            c.post(format!("{url}/_door/updates"))
                .header("cookie", &cookie)
                .header("origin", &origin)
                .json(&serde_json::json!({ "hold": hold }))
                .send()
        };
        assert_eq!(set(true).await.unwrap().status(), 200);
        assert!(r.app.cfg.agent_state.join("hold").exists());
        assert_eq!(set(false).await.unwrap().status(), 200);
        assert!(!r.app.cfg.agent_state.join("hold").exists());
        // from another page: refused
        let other = c
            .post(format!("{url}/_door/updates"))
            .header("cookie", &cookie)
            .header("origin", "https://evil.example")
            .json(&serde_json::json!({ "hold": true }))
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), 403);
        assert!(!r.app.cfg.agent_state.join("hold").exists());
    }

    #[test]
    fn the_tls_words_are_the_apps() {
        let m = super::tls_message("x.home.commonty.org", b"der");
        assert!(m.starts_with(b"commonty door tls v1\0x.home.commonty.org\0"));
        assert_eq!(
            m.len(),
            "commonty door tls v1\0x.home.commonty.org\0".len() + 64
        );
    }
}
