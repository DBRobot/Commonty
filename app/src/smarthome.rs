//! Smart home in the app's window: the household's home controller, through
//! a server of the app's own on this computer, as Photos and Passwords are
//! (local.rs). The window talks to 127.0.0.1; this talks to the controller's
//! door over the fleet's network (directory::builder), signed in with this
//! device's key, and holds the door to the key pairing gave
//! (docs/dev/home-controller.md, The name and its certificate).
//!
//! Unlike a box's answers, the controller's pages and scripts are let
//! through: the Pi is the household's own device, not a box, and Home
//! Assistant's pages are what this window is for. What makes that safe is
//! the pin: a door whose certificate the controller's key has not vouched
//! for is never sent a byte.
//!
//! The pin: the certificate is a public one (ACME) and cannot carry the
//! controller's ed25519 key, so the door vouches for it instead - GET
//! /_door/tls answers {sig}, the controller key's signature over
//! `commonty door tls v1\0<host>\0<hex sha256 of the leaf certificate>`.
//! The connection that asks it carries no secret; every other connection is
//! refused in the handshake unless its certificate is one so vouched for.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use futures_util::TryStreamExt as _;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::Digest as _;
use tauri::{AppHandle, Manager as _, Runtime};

/// Smart home's address on this computer: fixed, as Home Assistant keeps
/// its sign-in in the origin's storage
pub const PORT: u16 = 41663;

static UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// what the door's certificate is vouched for with
pub fn tls_message(host: &str, cert: &[u8]) -> Vec<u8> {
    let h: String = sha2::Sha256::digest(cert)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("commonty door tls v1\0{host}\0{h}").into_bytes()
}

/// what this device signs to sign in at a door as `name` (the door's signin.rs)
pub fn signin_message(host: &str, nonce: &str, name: &str) -> Vec<u8> {
    format!("commonty door v1\0{host}\0{nonce}\0{name}").into_bytes()
}

/// `sig` (base64) over `msg` by the controller key `controller` (base64)
fn vouched(controller: &str, msg: &[u8], sig: &str) -> bool {
    let Ok(k) = B64.decode(controller) else {
        return false;
    };
    let Ok(k) = <[u8; 32]>::try_from(k.as_slice()) else {
        return false;
    };
    let Ok(k) = ed25519_dalek::VerifyingKey::from_bytes(&k) else {
        return false;
    };
    let Ok(s) = B64.decode(sig.trim()) else {
        return false;
    };
    let Ok(s) = ed25519_dalek::Signature::from_slice(&s) else {
        return false;
    };
    k.verify_strict(msg, &s).is_ok()
}

/// by host: the certificates (their sha256) its controller vouched for
static PINS: Mutex<Option<HashMap<String, HashSet<[u8; 32]>>>> = Mutex::new(None);

fn pinned(host: &str, cert: &[u8]) -> bool {
    let h: [u8; 32] = sha2::Sha256::digest(cert).into();
    PINS.lock()
        .ok()
        .and_then(|p| p.as_ref().and_then(|m| m.get(host).map(|s| s.contains(&h))))
        .unwrap_or(false)
}

fn pin(host: &str, cert: &[u8]) {
    if let Ok(mut p) = PINS.lock() {
        p.get_or_insert_with(HashMap::new)
            .entry(host.to_string())
            .or_default()
            .insert(sha2::Sha256::digest(cert).into());
    }
}

/// The handshake's check: `strict`, a certificate vouched for already; not
/// strict, any (the one request that asks the door to vouch, which carries
/// nothing, and whose certificate is checked against the answer).
#[derive(Debug)]
struct Pinned {
    strict: bool,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let host = match server_name {
            ServerName::DnsName(d) => d.as_ref().to_string(),
            _ => return Err(rustls::Error::General("not a name".into())),
        };
        if !self.strict || pinned(&host, end_entity.as_ref()) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "a certificate the home controller has not vouched for".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn client(strict: bool) -> Result<reqwest::Client, String> {
    client_from(directory::builder().map_err(|e| e.to_string())?, strict)
}

fn client_from(b: reqwest::ClientBuilder, strict: bool) -> Result<reqwest::Client, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { strict, provider }))
        .with_no_client_auth();
    // HTTP/1.1 alone: Home Assistant's websocket is an upgrade
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    let b = b
        .use_preconfigured_tls(tls)
        .http1_only()
        .redirect(reqwest::redirect::Policy::none());
    let b = if strict {
        b
    } else {
        // a connection of its own each time, whose certificate is the one
        // the answer is checked against
        b.tls_info(true).pool_max_idle_per_host(0)
    };
    b.build().map_err(|e| e.to_string())
}

/// Asks the door to vouch for the certificate it shows, and pins it if the
/// controller's key did.
async fn vouch(host: &str, controller: &str) -> Result<(), String> {
    vouch_by(
        &client(false)?,
        &format!("https://{host}"),
        host,
        controller,
    )
    .await
}

async fn vouch_by(
    http: &reqwest::Client,
    base: &str,
    host: &str,
    controller: &str,
) -> Result<(), String> {
    let r = http
        .get(format!("{base}/_door/tls"))
        .send()
        .await
        .map_err(|e| format!("the home controller did not answer: {e}"))?;
    let cert = r
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|t| t.peer_certificate())
        .map(<[u8]>::to_vec)
        .ok_or("the home controller showed no certificate")?;
    if !r.status().is_success() {
        return Err(format!(
            "the home controller did not vouch for its certificate ({})",
            r.status()
        ));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
    let sig = v["sig"].as_str().unwrap_or_default();
    if !vouched(controller, &tls_message(host, &cert), sig) {
        return Err(
            "this is not your home controller: its certificate is not signed by the key it was paired with"
                .into(),
        );
    }
    pin(host, &cert);
    Ok(())
}

/// one controller, signed in: its door's cookie and the client held to it
struct Door {
    controller: String,
    http: reqwest::Client,
    cookie: Option<String>,
}

/// by name; and the one this computer's Smart home window shows
static DOORS: tokio::sync::Mutex<Option<HashMap<String, Door>>> =
    tokio::sync::Mutex::const_new(None);
static SHOWN: Mutex<Option<String>> = Mutex::new(None);

/// This device signs in at the door: a challenge, signed by the device key,
/// with the account's newest entry, which the door may take as newer than
/// the one it keeps.
async fn sign_in<R: Runtime>(
    app: &AppHandle<R>,
    host: &str,
    http: &reqwest::Client,
) -> Result<String, String> {
    use auth::KeyStore as _;
    use ed25519_dalek::Signer as _;
    let keys = app.state::<crate::account::Keys>();
    let kp = auth::device::load(&keys.0)
        .map_err(|e| e.to_string())?
        .ok_or("no device key here")?;
    let user = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    let seed: [u8; 32] = kp.private().to_bytes()[..]
        .try_into()
        .map_err(|_| "the device key is not 32 bytes")?;
    let device = ed25519_dalek::SigningKey::from_bytes(&seed);
    let entry = directory::newest(&directory::fetch(&crate::account::dirs(), &user).await)
        .ok_or("your entry could not be read")?;
    let c: serde_json::Value = http
        .get(format!("https://{host}/_door/challenge"))
        .send()
        .await
        .map_err(|e| format!("the home controller did not answer: {e}"))?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let nonce = c["nonce"].as_str().ok_or("the door gave no challenge")?;
    let sig = B64.encode(device.sign(&signin_message(host, nonce, &user)).to_bytes());
    let r = http
        .post(format!("https://{host}/_door/signin"))
        .json(&serde_json::json!({
            "name": user,
            "entry": entry,
            "device": auth::device::public_b64(&kp),
            "sig": sig,
            "nonce": nonce,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        let s = r.status();
        return Err(format!(
            "the home controller did not let this device in ({s}): {}",
            r.text().await.unwrap_or_default()
        ));
    }
    door_cookie(r.headers()).ok_or_else(|| "the door gave no session".into())
}

/// `door=<value>` out of a Set-Cookie, as the Cookie it goes back as
fn door_cookie(h: &reqwest::header::HeaderMap) -> Option<String> {
    h.get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .map(str::trim)
        .find(|c| c.starts_with("door=") && c.len() > "door=".len())
        .map(str::to_string)
}

/// the door for `host`, vouched for and signed in; `fresh` signs in again
async fn door<R: Runtime>(
    app: &AppHandle<R>,
    host: &str,
    fresh: bool,
) -> Result<(reqwest::Client, String), String> {
    let mut doors = DOORS.lock().await;
    let doors = doors.get_or_insert_with(HashMap::new);
    let controller = match doors.get(host) {
        Some(d) => d.controller.clone(),
        None => crate::controllers::read(app)
            .await?
            .into_iter()
            .find(|c| c.name == host)
            .map(|c| c.controller)
            .ok_or("that home controller is not one of yours")?,
    };
    if let Some(d) = doors.get(host)
        && !fresh
        && let Some(c) = &d.cookie
    {
        return Ok((d.http.clone(), c.clone()));
    }
    vouch(host, &controller).await?;
    let http = client(true)?;
    let cookie = sign_in(app, host, &http).await?;
    doors.insert(
        host.to_string(),
        Door {
            controller,
            http: http.clone(),
            cookie: Some(cookie.clone()),
        },
    );
    Ok((http, cookie))
}

/// POST /_dd/app/smarthome/open {name}: where the window goes to show it
pub async fn open<R: Runtime>(app: &AppHandle<R>, body: &[u8]) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let name = v["name"].as_str().ok_or("which home controller?")?;
    url_for(app, name).await
}

pub async fn url_for<R: Runtime>(app: &AppHandle<R>, name: &str) -> Result<String, String> {
    if !UP.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(format!(
            "something else on this computer holds port {PORT}, so this app will not open Smart home there"
        ));
    }
    door(app, name, false).await?;
    *SHOWN.lock().map_err(|e| e.to_string())? = Some(name.to_string());
    Ok(format!(
        "http://127.0.0.1:{PORT}/_dd/app/enter?k={}&to=/",
        crate::site::launch_secret()
    ))
}

/// GET /_dd/app/smarthome/status?name=: for Settings, from the door itself
/// (no box knows): whether it answers, who can open it, its release and
/// whether its updates are held
pub async fn status<R: Runtime>(app: &AppHandle<R>, name: &str) -> serde_json::Value {
    let asked = async {
        let (http, cookie) = door(app, name, false).await?;
        let get = |path: &str| {
            http.get(format!("https://{name}{path}"))
                .header(reqwest::header::COOKIE, &cookie)
                .send()
        };
        let (people, st) = futures_util::join!(get("/_door/people"), get("/_door/status"));
        let people = match people {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
            _ => None,
        };
        // the door's own status (release, updates held): not every door
        // answers it yet
        let st = match st {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
            _ => None,
        };
        Ok::<_, String>((people, st))
    };
    match asked.await {
        Ok((people, st)) => serde_json::json!({
            "reachable": true,
            "people": people,
            "status": st,
        }),
        Err(e) => serde_json::json!({ "reachable": false, "error": e }),
    }
}

/// POST /_dd/app/smarthome/updates?name= {hold}: the household's hold on
/// the controller's updates, said to the door as from its own page
pub async fn set_updates<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
    hold: bool,
) -> Result<serde_json::Value, String> {
    let (http, cookie) = door(app, name, false).await?;
    let r = http
        .post(format!("https://{name}/_door/updates"))
        .header(reqwest::header::COOKIE, &cookie)
        .header(reqwest::header::ORIGIN, format!("https://{name}"))
        .json(&serde_json::json!({ "hold": hold }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("the home controller said {}", r.status()));
    }
    r.json().await.map_err(|e| e.to_string())
}

/// The server, for the life of the app.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let app = Arc::new(app.clone());
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", PORT)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("smart home: 127.0.0.1:{PORT}: {e}");
                return;
            }
        };
        UP.store(true, std::sync::atomic::Ordering::SeqCst);
        let router = axum::Router::new().fallback(handle::<R>).with_state(app);
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("smart home: 127.0.0.1:{PORT}: {e}");
        }
    });
}

fn plain(status: StatusCode, text: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        text.to_string(),
    )
        .into_response()
}

async fn handle<R: Runtime>(State(app): State<Arc<AppHandle<R>>>, req: Request) -> Response {
    if req.uri().path() == "/_dd/app/enter" {
        return crate::local::enter_window(&req);
    }
    if !crate::local::window_cookie(req.headers()) || !from_own(req.headers(), req.method(), PORT) {
        return plain(StatusCode::FORBIDDEN, "open this from the Commonty app");
    }
    let Some(host) = SHOWN.lock().ok().and_then(|s| s.clone()) else {
        return plain(StatusCode::NOT_FOUND, "open Smart home from the app");
    };
    forward(&app, &host, req).await
}

/// Asked by this server's own page, and nothing else. The app's window
/// cookie is shared by every local server (Photos, Passwords, the site's
/// pages, Smart home), and every
/// port of 127.0.0.1 is one site to a browser, so a page one of them shows
/// (a home controller's, say) is sent the cookie when it asks another.
/// What a request says of where it came from decides: its Origin, or the
/// browser's Sec-Fetch-Site, must be this server's own. A top-level GET is
/// let in from anywhere: the app's window arrives that way, from its own
/// scheme, and a navigation reads nothing back across sites.
fn from_own(h: &header::HeaderMap, method: &axum::http::Method, port: u16) -> bool {
    let ours = format!("http://127.0.0.1:{port}");
    // a page of its own, top level: never a frame, which another page on
    // 127.0.0.1 could put it in and dress up
    let navigation = matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD)
        && h.get("sec-fetch-mode")
            .is_some_and(|m| m.as_bytes() == b"navigate")
        && h.get("sec-fetch-dest")
            .is_some_and(|d| d.as_bytes() == b"document")
        && !h.contains_key(header::UPGRADE);
    if navigation {
        return true;
    }
    let origin_ok = h
        .get(header::ORIGIN)
        .is_none_or(|o| o.as_bytes() == ours.as_bytes());
    let site_ok = h
        .get("sec-fetch-site")
        .is_none_or(|s| matches!(s.as_bytes(), b"same-origin" | b"none"));
    origin_ok && site_ok
}

/// The window's request, to the door. A request with no body that the door
/// turned away as signed out is signed in again and asked once more.
async fn forward<R: Runtime>(app: &AppHandle<R>, host: &str, mut req: Request) -> Response {
    let (http, cookie) = match door(app, host, false).await {
        Ok(d) => d,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &e),
    };
    let target = match req.uri().query() {
        Some(q) => format!("https://{host}{}?{q}", req.uri().path()),
        None => format!("https://{host}{}", req.uri().path()),
    };
    let upgrade = req.headers().contains_key(header::UPGRADE);
    let bodiless = matches!(req.method().as_str(), "GET" | "HEAD") && !upgrade;
    let Ok(method) = reqwest::Method::from_bytes(req.method().as_str().as_bytes()) else {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "no such method");
    };
    let mut headers = reqwest::header::HeaderMap::new();
    for (k, v) in req.headers() {
        // this hop's, and the window's own cookies: the door's session is
        // held here and never given to the window
        if matches!(
            k.as_str(),
            "host" | "cookie" | "referer" | "content-length" | "transfer-encoding"
        ) || (!upgrade && matches!(k.as_str(), "connection" | "upgrade"))
        {
            continue;
        }
        if let (Ok(k), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(k.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(v.as_bytes()),
        ) {
            headers.insert(k, v);
        }
    }
    // the door checks a request's origin is its own
    if headers.contains_key(reqwest::header::ORIGIN)
        && let Ok(o) = reqwest::header::HeaderValue::from_str(&format!("https://{host}"))
    {
        headers.insert(reqwest::header::ORIGIN, o);
    }
    let send = |http: &reqwest::Client, cookie: &str, body: reqwest::Body| {
        let mut h = headers.clone();
        if let Ok(c) = reqwest::header::HeaderValue::from_str(cookie) {
            h.insert(reqwest::header::COOKIE, c);
        }
        http.request(method.clone(), &target)
            .headers(h)
            .body(body)
            .send()
    };
    let on_upgrade = upgrade.then(|| hyper::upgrade::on(&mut req));
    let body = if bodiless || upgrade {
        reqwest::Body::from(Vec::new())
    } else {
        reqwest::Body::wrap_stream(req.into_body().into_data_stream())
    };
    let mut r = match send(&http, &cookie, body).await {
        Ok(r) => r,
        Err(e) => {
            // a certificate renewed since: vouched for again, next time
            DOORS.lock().await.as_mut().map(|d| d.remove(host));
            return plain(
                StatusCode::BAD_GATEWAY,
                &format!("the home controller did not answer: {e}"),
            );
        }
    };
    if r.status() == reqwest::StatusCode::UNAUTHORIZED && bodiless {
        match door(app, host, true).await {
            Ok((http, cookie)) => match send(&http, &cookie, reqwest::Body::from(Vec::new())).await
            {
                Ok(again) => r = again,
                Err(e) => return plain(StatusCode::BAD_GATEWAY, &e.to_string()),
            },
            Err(e) => return plain(StatusCode::BAD_GATEWAY, &e),
        }
    }
    let mut back = Response::builder().status(r.status().as_u16());
    for (k, v) in r.headers() {
        if matches!(
            k.as_str(),
            "transfer-encoding" | "set-cookie" | "strict-transport-security" | "alt-svc"
        ) || (!upgrade && k.as_str() == "connection")
        {
            continue;
        }
        if k == reqwest::header::LOCATION {
            // the door's own pages are this server's copy of them
            let l = v.to_str().unwrap_or("");
            let here = l.strip_prefix(&format!("https://{host}")).unwrap_or(l);
            if let Ok(h) = HeaderValue::from_str(here) {
                back = back.header(header::LOCATION, h);
            }
            continue;
        }
        back = back.header(k.as_str(), v.as_bytes());
    }
    if let (Some(down), reqwest::StatusCode::SWITCHING_PROTOCOLS) = (on_upgrade, r.status()) {
        tauri::async_runtime::spawn(async move {
            let (Ok(up), Ok(down)) = (r.upgrade().await, down.await) else {
                return;
            };
            let mut down = hyper_util::rt::TokioIo::new(down);
            let mut up = up;
            let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
        });
        return back.body(Body::empty()).unwrap_or_default();
    }
    let stream = r.bytes_stream().map_err(std::io::Error::other);
    back.body(Body::from_stream(stream)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn smart_homes_server_serves_its_own_page_alone() {
        use axum::http::Method;
        let port = 41663;
        let mut h = header::HeaderMap::new();
        assert!(
            from_own(&h, &Method::POST, port),
            "the app asking for itself"
        );
        h.insert(header::ORIGIN, "http://127.0.0.1:41663".parse().unwrap());
        h.insert("sec-fetch-site", "same-origin".parse().unwrap());
        assert!(from_own(&h, &Method::POST, port));
        assert!(
            !from_own(&h, &Method::POST, 41662),
            "Smart home's page asking Passwords"
        );
        let mut h = header::HeaderMap::new();
        h.insert("sec-fetch-site", "same-site".parse().unwrap());
        assert!(!from_own(&h, &Method::POST, port), "a page on another port");
        h.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(
            !from_own(&h, &Method::GET, port),
            "a fetch, not a navigation"
        );
        h.insert("sec-fetch-mode", "navigate".parse().unwrap());
        h.insert("sec-fetch-dest", "iframe".parse().unwrap());
        assert!(!from_own(&h, &Method::GET, port), "framed by another page");
        h.insert("sec-fetch-dest", "document".parse().unwrap());
        assert!(
            from_own(&h, &Method::GET, port),
            "the app's window, by its own scheme's redirect"
        );
        assert!(
            !from_own(&h, &Method::POST, port),
            "a form posted from elsewhere"
        );
        h.insert(header::UPGRADE, "websocket".parse().unwrap());
        assert!(
            !from_own(&h, &Method::GET, port),
            "a websocket from elsewhere"
        );
    }

    use super::*;
    use ed25519_dalek::Signer as _;

    #[test]
    fn a_certificate_counts_only_when_the_controller_vouched_for_it() {
        let k = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let controller = B64.encode(k.verifying_key().to_bytes());
        let host = "abcdefghijklmnop.home.commonty.org";
        let cert = b"the leaf certificate's bytes";
        let sig = B64.encode(k.sign(&tls_message(host, cert)).to_bytes());
        assert!(vouched(&controller, &tls_message(host, cert), &sig));
        // another certificate, another name, another key: no
        assert!(!vouched(&controller, &tls_message(host, b"another"), &sig));
        assert!(!vouched(
            &controller,
            &tls_message("x.home.commonty.org", cert),
            &sig
        ));
        let other = B64.encode(
            ed25519_dalek::SigningKey::from_bytes(&[8; 32])
                .verifying_key()
                .to_bytes(),
        );
        assert!(!vouched(&other, &tls_message(host, cert), &sig));
        assert!(!vouched(
            &controller,
            &tls_message(host, cert),
            "not base64"
        ));
        // and the handshake holds the door to what was pinned
        assert!(!pinned(host, cert));
        pin(host, cert);
        assert!(pinned(host, cert));
        assert!(!pinned(host, b"another"));
        assert!(!pinned("x.home.commonty.org", cert));
    }

    #[test]
    fn what_a_device_signs_at_the_door() {
        assert_eq!(
            signin_message("a.home.x.org", "n0nce", "ann"),
            b"commonty door v1\0a.home.x.org\0n0nce\0ann".to_vec()
        );
    }

    #[test]
    fn the_doors_session_out_of_its_answer() {
        let mut h = reqwest::header::HeaderMap::new();
        h.append(
            reqwest::header::SET_COOKIE,
            "other=1; Path=/".parse().unwrap(),
        );
        h.append(
            reqwest::header::SET_COOKIE,
            "door=abc123; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800"
                .parse()
                .unwrap(),
        );
        assert_eq!(door_cookie(&h).as_deref(), Some("door=abc123"));
        let mut gone = reqwest::header::HeaderMap::new();
        gone.append(
            reqwest::header::SET_COOKIE,
            "door=; Max-Age=0".parse().unwrap(),
        );
        assert_eq!(door_cookie(&gone), None);
    }

    /// A door with a real certificate: refused until its controller vouches
    /// for it, refused when the wrong key does, let through once the right
    /// one has.
    #[tokio::test]
    async fn the_handshake_holds_the_door_to_its_controller() {
        use rustls::pki_types::pem::PemObject as _;
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let host = "abcdefghijklmnop.home.door.test";
        let pem = include_bytes!("testdata/door.pem");
        let cert = CertificateDer::from_pem_slice(pem).unwrap();
        let key =
            rustls::pki_types::PrivateKeyDer::from_pem_slice(include_bytes!("testdata/door.key"))
                .unwrap();
        let k = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let controller = B64.encode(k.verifying_key().to_bytes());
        let vouch_sig = B64.encode(k.sign(&tls_message(host, cert.as_ref())).to_bytes());
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)
        .unwrap();
        let tls = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = l.accept().await else {
                    return;
                };
                let (tls, sig) = (tls.clone(), vouch_sig.clone());
                tokio::spawn(async move {
                    let Ok(mut s) = tls.accept(tcp).await else {
                        return;
                    };
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let body = if req.starts_with("GET /_door/tls ") {
                        format!("{{\"sig\":\"{sig}\"}}")
                    } else {
                        "{\"in\":true}".to_string()
                    };
                    let _ = s
                        .write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes())
                        .await;
                    let _ = s.shutdown().await;
                });
            }
        });
        let at = || reqwest::Client::builder().no_proxy().resolve(host, addr);
        let url = format!("https://{host}:{}/x", addr.port());
        let strict = client_from(at(), true).unwrap();
        assert!(
            strict.get(&url).send().await.is_err(),
            "nothing passes before the controller vouches"
        );
        let probe = client_from(at(), false).unwrap();
        let base = format!("https://{host}:{}", addr.port());
        let wrong = B64.encode(
            ed25519_dalek::SigningKey::from_bytes(&[4; 32])
                .verifying_key()
                .to_bytes(),
        );
        assert!(vouch_by(&probe, &base, host, &wrong).await.is_err());
        assert!(strict.get(&url).send().await.is_err());
        // the right key: pinned under the name the handshake sees
        vouch_by(&probe, &base, host, &controller).await.unwrap();
        let ok = strict.get(&url).send().await.unwrap();
        assert_eq!(ok.status(), 200);
    }
}
