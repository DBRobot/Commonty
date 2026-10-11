//! Photos and Passwords, served by this app to its own windows and to the
//! member's own browser. Their code - Ente's photos client, the Bitwarden
//! web vault - comes in the app's signed release, never from a box:
//! whoever has root on a box could otherwise hand the page that holds a
//! person's keys a copy that sends them away. The boxes keep the data and
//! answer the clients' calls; the code is ours.
//!
//! Each runs at its own port on this computer (its own origin, so one
//! cannot read the other's storage), a real web server and not the app's
//! scheme: Android's webview hands a scheme an empty body for every POST,
//! and a server streams where the scheme holds a whole file in memory.
//! Anything else on this computer could knock on these ports, so nothing is
//! answered without a cookie given at a door: the window's on 127.0.0.1
//! (`/_dd/app/enter?k=<this launch's secret>`), a browser's on its own
//! session's name, once the member has let that browser in from the app's
//! window (handoff.rs).

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt as _;
use tauri::{AppHandle, Manager as _, Runtime};

use crate::handoff::{self, Finish, Names, Once, Pairing, Sessions, Via, View, Which};

/// Whether this app holds each port. A window or a browser is only ever
/// sent to a port this app is serving: if something else had one first, it
/// would be what the window or the browser asks its way in, so neither is
/// sent at all.
static PHOTOS_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static VAULT_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Photos' pairing and Passwords'
static PAIRS: LazyLock<Mutex<[Pairing; 2]>> = LazyLock::new(Default::default);
static SESSIONS: LazyLock<Mutex<Sessions>> = LazyLock::new(Default::default);
/// the Photos password for the window's next page, once
static WINDOW_KEY: Mutex<Once> = Mutex::new(Once::new());
/// when a commonty:// link last opened the browser
static LAST_LINK: Mutex<Option<Instant>> = Mutex::new(None);

fn up(w: Which) -> &'static std::sync::atomic::AtomicBool {
    if w == Which::Vault {
        &VAULT_UP
    } else {
        &PHOTOS_UP
    }
}

fn holds(w: Which) -> bool {
    up(w).load(std::sync::atomic::Ordering::SeqCst)
}

fn slot(w: Which) -> usize {
    if w == Which::Vault { 1 } else { 0 }
}

/// this install's two pairing names, kept beside its other state
fn names() -> Option<&'static Names> {
    static N: std::sync::OnceLock<Option<Names>> = std::sync::OnceLock::new();
    N.get_or_init(|| {
        Names::load(&crate::paths::data().join("local-names"))
            .map_err(|e| eprintln!("local: names: {e}"))
            .ok()
    })
    .as_ref()
}

struct Site<R: Runtime> {
    app: AppHandle<R>,
    which: Which,
    /// the client's files, from the app's release (COMMONTY_PHOTOS_WEB,
    /// COMMONTY_VAULT_WEB)
    root: Option<PathBuf>,
}

pub fn url(which_vault: bool, path: &str) -> String {
    let w = if which_vault {
        Which::Vault
    } else {
        Which::Photos
    };
    format!("http://127.0.0.1:{}{path}", w.port())
}

/// where a window starts: the door, with this launch's secret, and the page
/// after it; only while this app holds the port
pub fn door(which_vault: bool, then: &str) -> Result<String, String> {
    let w = if which_vault {
        Which::Vault
    } else {
        Which::Photos
    };
    handoff::door_if_up(holds(w), w.port())?;
    Ok(url(
        which_vault,
        &format!(
            "/_dd/app/enter?k={}&to={}",
            crate::site::launch_secret(),
            enc(then)
        ),
    ))
}

/// the Photos password the window's page asks for once (photos.rs), so it
/// is in no address
pub fn set_window_key(pw: &str) {
    if let Ok(mut k) = WINDOW_KEY.lock() {
        k.set(pw.to_string(), Instant::now());
    }
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

/// A commonty:// link (lib.rs): the member's default browser opened at this
/// install's pairing page for Photos or Passwords, and the app's window at
/// the page where the member lets that browser in. A link from anywhere
/// only does that; nothing goes back to what sent it.
pub fn open_link<R: Runtime>(app: &AppHandle<R>, url: &str) {
    let Some(link) = handoff::parse_link(url) else {
        eprintln!("local: not a link of ours");
        return;
    };
    {
        let Ok(mut last) = LAST_LINK.lock() else {
            return;
        };
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return;
        }
        *last = Some(now);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = to_browser(&app, &link).await {
            eprintln!("local: {e}");
        }
    });
}

fn photos_password<R: Runtime>(app: &AppHandle<R>) -> Result<Option<String>, String> {
    use auth::KeyStore as _;
    let keys = app.state::<crate::account::Keys>();
    Ok(keys
        .0
        .get(crate::photos::PASSWORD)
        .map_err(|e| e.to_string())?
        .map(|pw| pw.to_string()))
}

async fn to_browser<R: Runtime>(app: &AppHandle<R>, link: &handoff::Link) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;
    // a link can start the app: its servers come up a moment after
    for _ in 0..50 {
        if holds(link.which) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handoff::door_if_up(holds(link.which), link.which.port())?;
    let names = names().ok_or("no names for the browser")?;
    if link.which == Which::Photos && photos_password(app)?.is_none() {
        // the first time on this device: the window fetches the password
        // with the browser's passkey (photos.rs)
        show_window(app, "/_dd/app/photos");
        return Ok(());
    }
    PAIRS.lock().map_err(|e| e.to_string())?[slot(link.which)]
        .arm(&handoff::then_for(link), Instant::now());
    show_window(app, "/_dd/app/allow");
    app.opener()
        .open_url(handoff::browser_url(names, link.which), None::<&str>)
        .map_err(|e| e.to_string())
}

fn show_window<R: Runtime>(app: &AppHandle<R>, path: &str) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
        if let Ok(u) = format!("{}{path}", crate::site::origin()).parse() {
            let _ = w.navigate(u);
        }
    }
}

fn view_json(v: View) -> serde_json::Value {
    match v {
        View::Idle => serde_json::json!({ "state": "idle" }),
        View::Waiting => serde_json::json!({ "state": "waiting" }),
        View::Asking(code) => serde_json::json!({ "state": "asking", "code": code }),
        View::Crowded => serde_json::json!({ "state": "crowded" }),
    }
}

/// what the app's window shows on its allow page (site.rs), for each service
pub fn pair_state() -> serde_json::Value {
    let now = Instant::now();
    match PAIRS.lock() {
        Ok(p) => serde_json::json!({
            "photos": view_json(p[0].view(now)),
            "vault": view_json(p[1].view(now)),
        }),
        Err(_) => serde_json::json!({}),
    }
}

fn which_of(service: &str) -> Result<Which, String> {
    match service {
        "photos" => Ok(Which::Photos),
        "vault" => Ok(Which::Vault),
        _ => Err("no such service".into()),
    }
}

/// the member's click in the app's window: let in the browser showing
/// `code`, or refuse it
pub fn pair_answer(service: &str, code: Option<&str>) -> Result<(), String> {
    let w = which_of(service)?;
    let mut p = PAIRS.lock().map_err(|e| e.to_string())?;
    match code {
        Some(c) => p[slot(w)].allow(c, Instant::now()).map_err(Into::into),
        None => {
            p[slot(w)].refuse();
            Ok(())
        }
    }
}

/// Both servers, for the life of the app. A port already taken (a second
/// copy of the app) leaves that one unserved, said in the log.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    for (which, var) in [
        (Which::Photos, "COMMONTY_PHOTOS_WEB"),
        (Which::Vault, "COMMONTY_VAULT_WEB"),
    ] {
        let port = which.port();
        let site = Arc::new(Site {
            app: app.clone(),
            which,
            root: std::env::var_os(var).map(PathBuf::from),
        });
        tauri::async_runtime::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(l) => {
                    up(which).store(true, std::sync::atomic::Ordering::SeqCst);
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

fn json(status: StatusCode, v: serde_json::Value) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        v.to_string(),
    )
        .into_response()
}

fn cookie<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    handoff::cookie(
        h.get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok()),
        name,
    )
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

async fn handle<R: Runtime>(State(site): State<Arc<Site<R>>>, req: Request) -> Response {
    let Some(names) = names() else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "no names for this app");
    };
    let h = req.headers();
    let via = match SESSIONS.lock() {
        Ok(s) => handoff::via(header_str(h, "host"), site.which, names, &s),
        Err(_) => None,
    };
    let Some(via) = via else {
        return plain(StatusCode::MISDIRECTED_REQUEST, "not this app's address");
    };
    let own = format!("http://{}", header_str(h, "host").unwrap_or_default());
    let path = req.uri().path().to_string();
    let method = req.method().as_str().to_string();
    let way_in = handoff::way_in(site.which, &via, &method, &path);
    if !way_in
        && !handoff::same_origin(
            &method,
            header_str(h, "origin"),
            header_str(h, "sec-fetch-site"),
            &own,
            via == Via::Window,
        )
    {
        return plain(StatusCode::FORBIDDEN, "not this page's own request");
    }
    match &via {
        Via::Window => {
            if path == "/_dd/app/enter" {
                return enter_window(&req);
            }
            if cookie(h, handoff::WINDOW_COOKIE) != Some(crate::site::launch_secret()) {
                return plain(StatusCode::FORBIDDEN, "open this from the Commonty app");
            }
        }
        // the pairing page and what it asks; nothing else is here
        Via::Pair => return pair(&site, req, &path).await,
        Via::Browser(label) => {
            if path == "/_dd/app/enter" {
                return enter_browser(&req, site.which, label);
            }
            let ok = SESSIONS
                .lock()
                .is_ok_and(|s| s.valid(site.which, label, cookie(h, handoff::BROWSER_COOKIE)));
            if !ok && !way_in {
                return plain(
                    StatusCode::FORBIDDEN,
                    "open this from Photos' or Passwords' page, through the Commonty app",
                );
            }
        }
    }
    if site.which == Which::Photos && path == "/_dd/app/photos-key" {
        return photos_key(&req, &via);
    }
    match site.which {
        Which::Photos => photos(&site, req, &path).await,
        Which::Vault => vault(&site, req, &path).await,
    }
}

/// The window's door: the app's own window, holding this launch's secret,
/// is given the cookie every other request needs. Lax, not Strict: the
/// window arrives here from the app's own scheme, another site to a
/// browser, and a Strict cookie set on that way in is not sent on the
/// redirect that follows it.
fn enter_window(req: &Request) -> Response {
    let q = req.uri().query().unwrap_or("");
    if handoff::query(q, "k").map(handoff::dec).as_deref() != Some(crate::site::launch_secret()) {
        return plain(StatusCode::FORBIDDEN, "not the app's window");
    }
    let to = handoff::query(q, "to")
        .map(handoff::dec)
        .filter(|t| handoff::safe_then(t).is_some());
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(
            header::SET_COOKIE,
            format!(
                "{}={}; Path=/; HttpOnly; SameSite=Lax",
                handoff::WINDOW_COOKIE,
                crate::site::launch_secret()
            ),
        )
        .header(header::LOCATION, to.unwrap_or_else(|| "/".into()))
        .body(Body::empty())
        .unwrap_or_default()
}

/// The pairing page on the install's name: the page and its script, the
/// app's own files it styles itself with, and its two asks. A browser
/// shows the hash of a verifier only it holds, the member lets it in from
/// the app's window by the code both show, and the browser then shows the
/// verifier for its own session's door.
async fn pair<R: Runtime>(site: &Site<R>, req: Request, path: &str) -> Response {
    match (req.method().as_str(), path) {
        ("GET", "/_dd/app/pair") => return app_asset(&site.app, "pair.html"),
        ("GET", "/_dd/app/pair.js") => return app_asset(&site.app, "pair.js"),
        ("GET", p) if p.starts_with("/_dd/static/") => {
            let f = &p["/_dd/static/".len()..];
            if f.contains('/') || f.starts_with('.') {
                return plain(StatusCode::NOT_FOUND, "no such file");
            }
            return app_asset(&site.app, &format!("_dd/static/{f}"));
        }
        ("POST", "/_dd/app/pair/ask" | "/_dd/app/pair/finish") => {}
        _ => return plain(StatusCode::NOT_FOUND, "not here"),
    }
    let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else {
        return plain(StatusCode::BAD_REQUEST, "too much");
    };
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let now = Instant::now();
    let Ok(mut pairs) = PAIRS.lock() else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "no pairing");
    };
    let p = &mut pairs[slot(site.which)];
    if path == "/_dd/app/pair/ask" {
        return match p.ask(v["hash"].as_str().unwrap_or(""), now) {
            Ok(code) => json(StatusCode::OK, serde_json::json!({ "code": code })),
            Err(why) => json(StatusCode::FORBIDDEN, serde_json::json!({ "why": why })),
        };
    }
    let then = match p.finish(v["verifier"].as_str().unwrap_or(""), now) {
        Finish::Wait => return json(StatusCode::ACCEPTED, serde_json::json!({})),
        Finish::No => return json(StatusCode::FORBIDDEN, serde_json::json!({})),
        Finish::In(then) => then,
    };
    drop(pairs);
    let pw = if site.which == Which::Photos {
        match photos_password(&site.app) {
            Ok(Some(pw)) => Some(pw),
            _ => {
                return plain(
                    StatusCode::NOT_FOUND,
                    "Photos is not open on this device yet",
                );
            }
        }
    } else {
        None
    };
    let Ok((label, k)) = SESSIONS
        .lock()
        .map(|mut s| s.create(site.which, &then, pw, now))
    else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "no session");
    };
    json(
        StatusCode::OK,
        serde_json::json!({ "go": handoff::session_door(site.which, &label, &k) }),
    )
}

/// A browser's door on its session's own name: the one-time secret the
/// pairing page was handed, spent here, for this browser's own cookie.
fn enter_browser(req: &Request, which: Which, label: &str) -> Response {
    let q = req.uri().query().unwrap_or("");
    let k = handoff::query(q, "k").unwrap_or("");
    let got = SESSIONS
        .lock()
        .ok()
        .and_then(|mut s| s.enter(which, label, k, Instant::now()));
    let Some((c, then)) = got else {
        return plain(
            StatusCode::FORBIDDEN,
            "this link was used already or is too old: open it again from the page",
        );
    };
    let then = handoff::safe_then(&then).unwrap_or("/").to_string();
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header("referrer-policy", "no-referrer")
        .header(
            header::SET_COOKIE,
            format!(
                "{}={c}; Path=/; HttpOnly; SameSite=Strict",
                handoff::BROWSER_COOKIE
            ),
        )
        .body(Body::from(handoff::enter_page(&then)))
        .unwrap_or_default()
}

/// The Photos password for the page that opens the account, once, to the
/// window or the browser session the app let in with it.
fn photos_key(req: &Request, via: &Via) -> Response {
    if req.method() != "POST" {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "POST");
    }
    let now = Instant::now();
    let pw = match via {
        Via::Window => WINDOW_KEY.lock().ok().and_then(|mut k| k.take(now)),
        Via::Browser(label) => SESSIONS.lock().ok().and_then(|mut s| {
            s.take_key(
                Which::Photos,
                label,
                cookie(req.headers(), handoff::BROWSER_COOKIE),
                now,
            )
        }),
        Via::Pair => None,
    };
    match pw {
        Some(pw) => json(StatusCode::OK, serde_json::json!({ "pw": pw })),
        None => plain(StatusCode::NOT_FOUND, "open Photos from the Commonty app"),
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
    client(site, "photos", path, true).await
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
    client(site, "vault", path, false).await
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

/// A file of a client's: from the copy built into the app (web/_clients/<name>,
/// the same on every platform - Android has no nix store to read from), or
/// from a folder for a development run that names one (COMMONTY_PHOTOS_WEB,
/// COMMONTY_VAULT_WEB).
async fn client<R: Runtime>(site: &Site<R>, name: &str, path: &str, shell: bool) -> Response {
    if site.root.is_some() {
        return file(site.root.as_deref(), path, shell).await;
    }
    let rel = path.trim_start_matches('/');
    if rel
        .split('/')
        .any(|s| s == ".." || s.starts_with('.') || s.contains('\\'))
    {
        return plain(StatusCode::NOT_FOUND, "no such file");
    }
    let mut tries = vec![if rel.is_empty() || rel.ends_with('/') {
        format!("_clients/{name}/{rel}index.html")
    } else {
        format!("_clients/{name}/{rel}")
    }];
    if shell {
        tries.push(format!("_clients/{name}/{rel}.html"));
        tries.push(format!("_clients/{name}/index.html"));
    }
    for t in tries {
        if let Some(a) = site.app.asset_resolver().get(t.clone()) {
            // the resolver's own guess at a type is by the asset's name;
            // ours for the few it would get wrong (wasm, maps)
            let kind = match Path::new(&t).extension().and_then(|e| e.to_str()) {
                Some("wasm" | "map" | "mjs") => mime(Path::new(&t)).to_string(),
                _ => a.mime_type,
            };
            return Response::builder()
                .header(header::CONTENT_TYPE, kind)
                .header(header::CACHE_CONTROL, "no-cache")
                .header("x-content-type-options", "nosniff")
                .body(Body::from(a.bytes))
                .unwrap_or_default();
        }
    }
    plain(StatusCode::NOT_FOUND, "no such file")
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
