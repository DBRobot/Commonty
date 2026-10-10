//! Photos and Passwords, served by this app to its own windows. Their code -
//! Ente's photos client, the Bitwarden web vault - comes in the app's signed
//! release, never from a box: whoever has root on a box could otherwise hand
//! the page that holds a person's keys a copy that sends them away. The
//! boxes keep the data and answer the clients' calls; the code is ours.
//!
//! Each runs at its own address on this computer (its own origin, so one
//! cannot read the other's storage), a real web server and not the app's
//! scheme: Android's webview hands a scheme an empty body for every POST,
//! and a server streams where the scheme holds a whole file in memory.
//! Anything else on this computer could knock on these ports, so nothing is
//! answered without the cookie the app's own window was given at the door
//! (`/_dd/app/enter?k=<this launch's secret>`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt as _;
use tauri::{AppHandle, Runtime};

/// Photos' address on this computer: fixed, because a browser keeps an
/// origin's storage by its port and Ente keeps a session there
pub const PHOTOS_PORT: u16 = 41661;
/// Passwords': the vault keeps its settings and the sign-in's state there
pub const VAULT_PORT: u16 = 41662;

const COOKIE: &str = "dd_local";

/// Whether this app holds each port. A window is only ever sent to a port
/// this app is serving: if something else had one first, it would be
/// handed the door's secret and the password after the page's #, so the
/// window is not sent at all.
static PHOTOS_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static VAULT_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Photos,
    Vault,
}

struct Site<R: Runtime> {
    app: AppHandle<R>,
    which: Which,
    /// the client's files, from the app's release (COMMONTY_PHOTOS_WEB,
    /// COMMONTY_VAULT_WEB)
    root: Option<PathBuf>,
}

pub fn url(which_vault: bool, path: &str) -> String {
    let port = if which_vault { VAULT_PORT } else { PHOTOS_PORT };
    format!("http://127.0.0.1:{port}{path}")
}

/// where a window starts: the door, with this launch's secret, and the page
/// after it; only while this app holds the port
pub fn door(which_vault: bool, then: &str) -> Result<String, String> {
    let up = if which_vault { &VAULT_UP } else { &PHOTOS_UP };
    if !up.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(format!(
            "something else on this computer holds port {}, so this app will not open its page there",
            if which_vault { VAULT_PORT } else { PHOTOS_PORT }
        ));
    }
    Ok(url(
        which_vault,
        &format!(
            "/_dd/app/enter?k={}&to={}",
            crate::site::launch_secret(),
            enc(then)
        ),
    ))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Both servers, for the life of the app. A port already taken (a second
/// copy of the app) leaves that one unserved, said in the log.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    for (which, port, var) in [
        (Which::Photos, PHOTOS_PORT, "COMMONTY_PHOTOS_WEB"),
        (Which::Vault, VAULT_PORT, "COMMONTY_VAULT_WEB"),
    ] {
        let site = Arc::new(Site {
            app: app.clone(),
            which,
            root: std::env::var_os(var).map(PathBuf::from),
        });
        tauri::async_runtime::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(l) => {
                    let up = if which == Which::Vault {
                        &VAULT_UP
                    } else {
                        &PHOTOS_UP
                    };
                    up.store(true, std::sync::atomic::Ordering::SeqCst);
                    l
                }
                Err(e) => {
                    eprintln!("local: 127.0.0.1:{port}: {e}");
                    return;
                }
            };
            let router = axum::Router::new().fallback(handle::<R>).with_state(site);
            if let Err(e) = axum::serve(listener, router).await {
                eprintln!("local: 127.0.0.1:{port}: {e}");
            }
        });
    }
}

fn plain(status: StatusCode, text: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        text.to_string(),
    )
        .into_response()
}

fn has_cookie(h: &HeaderMap) -> bool {
    let want = format!("{COOKIE}={}", crate::site::launch_secret());
    h.get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .any(|c| c.trim() == want)
}

async fn handle<R: Runtime>(State(site): State<Arc<Site<R>>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    // the door: the app's own window, holding this launch's secret, is given
    // the cookie every other request needs. Lax, not Strict: the window
    // arrives here from the app's own scheme, another site to a browser,
    // and a Strict cookie set on that way in is not sent on the redirect
    // that follows it
    if path == "/_dd/app/enter" {
        let q = req.uri().query().unwrap_or("");
        let get = |k: &str| {
            q.split('&')
                .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
                .map(crate::site::percent_decode)
        };
        if get("k").as_deref() != Some(crate::site::launch_secret()) {
            return plain(StatusCode::FORBIDDEN, "not the app's window");
        }
        let to = get("to").filter(|t| t.starts_with('/') && !t.starts_with("//"));
        return Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(
                header::SET_COOKIE,
                format!(
                    "{COOKIE}={}; Path=/; HttpOnly; SameSite=Lax",
                    crate::site::launch_secret()
                ),
            )
            .header(header::LOCATION, to.unwrap_or_else(|| "/".into()))
            .body(Body::empty())
            .unwrap_or_default();
    }
    if !has_cookie(req.headers()) {
        return plain(StatusCode::FORBIDDEN, "open this from the Commonty app");
    }
    match site.which {
        Which::Photos => photos(&site, req, &path).await,
        Which::Vault => vault(&site, req, &path).await,
    }
}

async fn photos<R: Runtime>(site: &Site<R>, req: Request, path: &str) -> Response {
    // the page that opens the account, and the app's own scripts and wasm
    if path == "/_dd/photos" || path == "/_dd/photos/" {
        return app_asset(&site.app, "photos-open.html");
    }
    if path == "/_dd/app/photos-open.js" {
        return app_asset(&site.app, "photos-open.js");
    }
    for (prefix, dir) in [("/_dd/static/", "_dd/static"), ("/_dd/web/", "_dd/web")] {
        if let Some(f) = path.strip_prefix(prefix) {
            if f.contains('/') || f.starts_with('.') {
                return plain(StatusCode::NOT_FOUND, "no such file");
            }
            return app_asset(&site.app, &format!("{dir}/{f}"));
        }
    }
    // the gate's: Photos' config, the bar's /_dd/me, museum for an account
    if path.starts_with("/_dd/") {
        let base = match media::gate::files_base(&crate::account::dirs()) {
            Ok(b) => b,
            Err(e) => return plain(StatusCode::BAD_GATEWAY, &e.to_string()),
        };
        let token = match crate::site::token(&site.app) {
            Ok(t) => t,
            Err(e) => return plain(StatusCode::BAD_GATEWAY, &e),
        };
        return forward(&base, Some(&token), req, false).await;
    }
    // Ente's own files: a page by its name, its name with .html, or the app
    // shell (as nginx served them, try_files $uri $uri.html /index.html)
    file(site.root.as_deref(), path, true).await
}

async fn vault<R: Runtime>(site: &Site<R>, req: Request, path: &str) -> Response {
    // what Vaultwarden answers itself: its api, the sign-in, icons, its
    // stylesheet; the box, over the fleet's network, as the page's own origin
    let to_box = [
        "/api/",
        "/identity/",
        "/icons/",
        "/events/",
        "/notifications/",
        "/alive",
        "/pwned/",
    ]
    .iter()
    .any(|p| path.starts_with(p) || path == p.trim_end_matches('/'));
    if to_box {
        let base = format!("https://vault.{}", crate::account::domain());
        return forward(&base, None, req, true).await;
    }
    file(site.root.as_deref(), path, false).await
}

/// One of the app's own files (web/), served as the app carries it
fn app_asset<R: Runtime>(app: &AppHandle<R>, path: &str) -> Response {
    match app.asset_resolver().get(path.to_string()) {
        Some(a) => Response::builder()
            .header(header::CONTENT_TYPE, a.mime_type)
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(a.bytes))
            .unwrap_or_default(),
        None => plain(StatusCode::NOT_FOUND, "not in the app"),
    }
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

/// A file of the client's, never outside its root: no `..`, no hidden names.
/// With `shell`, a path that is no file is the client's own page for it
/// (Next's /gallery.html) or its front page.
async fn file(root: Option<&Path>, path: &str, shell: bool) -> Response {
    let Some(root) = root else {
        return plain(StatusCode::NOT_FOUND, "this app was built without it");
    };
    let rel = path.trim_start_matches('/');
    if rel
        .split('/')
        .any(|s| s == ".." || s.starts_with('.') || s.contains('\\'))
    {
        return plain(StatusCode::NOT_FOUND, "no such file");
    }
    let mut tries = vec![if rel.is_empty() || rel.ends_with('/') {
        root.join(rel).join("index.html")
    } else {
        root.join(rel)
    }];
    if shell {
        tries.push(root.join(format!("{rel}.html")));
        tries.push(root.join("index.html"));
    }
    for p in tries {
        if let Ok(meta) = tokio::fs::metadata(&p).await
            && meta.is_file()
            && let Ok(f) = tokio::fs::File::open(&p).await
        {
            let stream = tokio_util::io::ReaderStream::new(f);
            return Response::builder()
                .header(header::CONTENT_TYPE, mime(&p))
                .header(header::CONTENT_LENGTH, meta.len())
                .header(header::CACHE_CONTROL, "no-cache")
                .header("x-content-type-options", "nosniff")
                .body(Body::from_stream(stream))
                .unwrap_or_default();
        }
    }
    plain(StatusCode::NOT_FOUND, "no such file")
}

/// The page's call, sent on to a box over the fleet's network and answered
/// back as it came, streamed both ways. `bearer`: this device's token for
/// the gate. `same_site`: the box is the page's own server (Vaultwarden),
/// so its redirects to itself come back here.
async fn forward(base: &str, bearer: Option<&str>, req: Request, same_site: bool) -> Response {
    let target = match req.uri().query() {
        Some(q) => format!("{base}{}?{q}", req.uri().path()),
        None => format!("{base}{}", req.uri().path()),
    };
    let client = match directory::builder().and_then(|b| {
        b.redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(Into::into)
    }) {
        Ok(c) => c,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &e.to_string()),
    };
    let Ok(method) = reqwest::Method::from_bytes(req.method().as_str().as_bytes()) else {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "no such method");
    };
    let mut out = client.request(method, &target);
    for (k, v) in req.headers() {
        // what is this hop's, and what would speak for this computer
        if matches!(
            k.as_str(),
            "host"
                | "cookie"
                | "origin"
                | "referer"
                | "connection"
                | "content-length"
                | "transfer-encoding"
        ) {
            continue;
        }
        out = out.header(k.as_str(), v.as_bytes());
    }
    if let Some(t) = bearer {
        out = out.bearer_auth(t);
    }
    let body = req.into_body().into_data_stream();
    out = out.body(reqwest::Body::wrap_stream(body));
    let r = match out.send().await {
        Ok(r) => r,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &e.to_string()),
    };
    // What a box answers is data, never code: a page, a script or a
    // stylesheet from a box would run with the keys this page holds, which
    // is the whole thing this server exists to stop. Nothing without a type
    // either, and the browser is told not to guess one.
    let kind = r
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ["html", "xml", "svg", "javascript", "ecmascript", "css"]
        .iter()
        .any(|k| kind.contains(k))
    {
        return plain(
            StatusCode::BAD_GATEWAY,
            "the box answered with code; the app runs only its own",
        );
    }
    let mut back = Response::builder()
        .status(r.status().as_u16())
        .header("x-content-type-options", "nosniff")
        .header("content-security-policy", "sandbox; default-src 'none'");
    if kind.is_empty()
        && r.status() != reqwest::StatusCode::NO_CONTENT
        && !r.status().is_redirection()
    {
        back = back.header(header::CONTENT_TYPE, "application/octet-stream");
    }
    for (k, v) in r.headers() {
        if matches!(
            k.as_str(),
            "connection"
                | "transfer-encoding"
                | "set-cookie"
                | "strict-transport-security"
                | "alt-svc"
                | "content-security-policy"
                | "x-content-type-options"
        ) {
            continue;
        }
        if k == header::LOCATION && same_site {
            // the box's own pages are this server's copy of them
            let l = v.to_str().unwrap_or("");
            let here = l.strip_prefix(base).unwrap_or(l);
            if let Ok(h) = HeaderValue::from_str(here) {
                back = back.header(k, h);
            }
            continue;
        }
        back = back.header(k, v);
    }
    let stream = r.bytes_stream().map_err(std::io::Error::other);
    back.body(Body::from_stream(stream)).unwrap_or_default()
}

/// Passwords in the app's window: the gate's sign-in for the window (the
/// vault's own sign-in goes through the gate and wants its session), then
/// the app's own copy of the vault, through its door.
pub fn vault_state<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<tauri::http::Response<Vec<u8>>, String> {
    use auth::KeyStore as _;
    use tauri::Manager as _;
    let keys = app.state::<crate::account::Keys>();
    let kp = auth::device::load(&keys.0)
        .map_err(|e| e.to_string())?
        .ok_or("no device key here")?;
    let user = keys
        .0
        .get("user")
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    let host = format!("home.{}", crate::account::domain());
    // five minutes, the gate's own host alone: it passes through a form
    let token = auth::device::mint_at(
        &kp,
        &user,
        std::time::Duration::from_secs(300),
        Some("access"),
        Some(&host),
    )
    .map_err(|e| e.to_string())?;
    let v = serde_json::json!({
        "action": format!("https://{host}/_dd/app/signin"),
        "token": token,
        "door": door(true, "/")?,
    });
    tauri::http::Response::builder()
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(v.to_string().into_bytes())
        .map_err(|e| e.to_string())
}
