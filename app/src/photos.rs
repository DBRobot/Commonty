//! Photos in the app's own window. The photo account's password is made by
//! the person's passkey, which the app's window cannot use, so the first
//! time on a device the app asks the browser for it: it opens a slot at the
//! gate, sends the browser to the Photos page with the slot and a one-time
//! key, and the browser - with the passkey - leaves the password there
//! sealed to that key (box/photos/web/photos.js). The app opens it, keeps it
//! in this device's keyring, and from then on signs its window in and
//! hands the password to the Photos page after the `#` of its address,
//! the part no server is ever sent.

use std::sync::Mutex;
use std::time::Duration;

use auth::KeyStore as _;
use base64::Engine as _;
use tauri::http::{Response, StatusCode, header};
use tauri::{AppHandle, Manager as _, Runtime};

/// the keyring account holding the photo password, on this device only
pub const PASSWORD: &str = "photos-password";

/// the slot this device is waiting on, and the key only it can open with
static PENDING: Mutex<Option<(String, library::Key)>> = Mutex::new(None);

fn photos_host() -> String {
    format!("photos.{}", crate::account::domain())
}

fn json(v: serde_json::Value) -> Result<Response<Vec<u8>>, String> {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(v.to_string().into_bytes())
        .map_err(|e| e.to_string())
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Where the page stands: signed in and ready (with what it posts and
/// where), waiting on the browser, or not begun.
pub async fn state<R: Runtime>(app: &AppHandle<R>) -> Result<Response<Vec<u8>>, String> {
    let keys = app.state::<crate::account::Keys>();
    if let Some(pw) = keys.0.get(PASSWORD).map_err(|e| e.to_string())? {
        return ready(app, &pw);
    }
    let pending = PENDING
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .map(|(id, _)| id.clone());
    let Some(id) = pending else {
        return json(serde_json::json!({ "needs": true }));
    };
    let base = media::gate::files_base(&crate::account::dirs()).map_err(|e| e.to_string())?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .get(format!("{base}/_dd/photos/handoff/{id}"))
        .bearer_auth(crate::site::token(app)?)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match r.status().as_u16() {
        204 => json(serde_json::json!({ "waiting": true })),
        200 => {
            let sealed = r.text().await.map_err(|e| e.to_string())?;
            let secret = PENDING
                .lock()
                .map_err(|e| e.to_string())?
                .take()
                .map(|(_, k)| k)
                .ok_or("no key to open it with")?;
            let raw = library::open_x25519(&secret, sealed.trim()).map_err(|e| e.to_string())?;
            // the password as the Photos page makes it from the passkey
            let pw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw[..]);
            keys.0.set(PASSWORD, &pw).map_err(|e| e.to_string())?;
            ready(app, &pw)
        }
        _ => {
            *PENDING.lock().map_err(|e| e.to_string())? = None;
            json(serde_json::json!({ "expired": true }))
        }
    }
}

/// what the app's page posts to sign its window in, and the address after
fn ready<R: Runtime>(app: &AppHandle<R>, pw: &str) -> Result<Response<Vec<u8>>, String> {
    let keys = app.state::<crate::account::Keys>();
    let kp = auth::device::load(&keys.0)
        .map_err(|e| e.to_string())?
        .ok_or("no device key here")?;
    let user = keys
        .0
        .get("user")
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    let host = photos_host();
    // five minutes, the Photos site alone: it passes through a form
    let token = auth::device::mint_at(
        &kp,
        &user,
        Duration::from_secs(300),
        Some("access"),
        Some(&host),
    )
    .map_err(|e| e.to_string())?;
    json(serde_json::json!({
        "ready": true,
        "action": format!("https://{host}/_dd/app/signin"),
        "token": token,
        "hash": format!("app={}&pw={}", enc(crate::site::origin()), enc(pw)),
    }))
}

/// The first time on this device: a slot at the gate, and the browser sent
/// to fill it. The one-time key's secret half stays here.
pub async fn start<R: Runtime>(app: &AppHandle<R>) -> Result<Response<Vec<u8>>, String> {
    use tauri_plugin_opener::OpenerExt as _;
    let (public, secret) = library::ephemeral();
    let base = media::gate::files_base(&crate::account::dirs()).map_err(|e| e.to_string())?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .post(format!("{base}/_dd/photos/handoff"))
        .bearer_auth(crate::site::token(app)?)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("the box said {}", r.status()));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
    let id = v["id"].as_str().ok_or("no slot")?.to_string();
    let url = format!(
        "https://{}/_dd/photos?handoff={id}&to={}",
        photos_host(),
        enc(&public)
    );
    *PENDING.lock().map_err(|e| e.to_string())? = Some((id, secret));
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())?;
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Vec::new())
        .map_err(|e| e.to_string())
}
