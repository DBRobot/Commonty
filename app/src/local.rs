//! What holds a member's keys in a page, served by this app to its own
//! windows and to the member's own browser: Photos (Ente's client),
//! Passwords (the Bitwarden web vault), and the site's own pages that open
//! the library or other keys (Files, Movies & TV, Chat, Settings). Their
//! code comes in the app's signed release, never from a box: whoever has
//! root on a box could otherwise hand the page that holds a person's keys a
//! copy that sends them away. The boxes keep the data and answer the
//! pages' calls; the code, and the keys, are this device's.
//!
//! Each runs at its own port on this computer (its own origin, so one
//! cannot read another's storage), a real web server and not the app's
//! scheme: Android's webview hands a scheme an empty body for every POST,
//! and a server streams where the scheme holds a whole file in memory.
//! Anything else on this computer could knock on these ports, so nothing is
//! answered without a cookie given at a door: the window's on 127.0.0.1
//! (`/_dd/app/enter?k=<this launch's secret>`), a browser's on its own
//! session's name, once the member has let that browser in from the app's
//! window (handoff.rs). The site's pages are the window's on its own scheme
//! (site.rs); here they are for a browser alone.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt as _;
use tauri::{AppHandle, Manager as _, Runtime};

use crate::handoff::{self, Admit, Finish, Names, Once, Pairing, Sessions, Via, View, Which};

/// Whether this app holds each port. A window or a browser is only ever
/// sent to a port this app is serving: if something else had one first, it
/// would be what the window or the browser asks its way in, so neither is
/// sent at all.
static PHOTOS_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static VAULT_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SITE_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Photos' pairing, Passwords' and the site's
static PAIRS: LazyLock<Mutex<[Pairing; 3]>> = LazyLock::new(Default::default);
static SESSIONS: LazyLock<Mutex<Sessions>> = LazyLock::new(Default::default);
/// what the window's next Photos page is handed, once
static WINDOW_KEY: Mutex<Once> = Mutex::new(Once::new());
/// when a commonty:// link last opened the browser
static LAST_LINK: Mutex<Option<Instant>> = Mutex::new(None);

const SERVICES: [Which; 3] = [Which::Photos, Which::Vault, Which::Site];

fn up(w: Which) -> &'static std::sync::atomic::AtomicBool {
    match w {
        Which::Photos => &PHOTOS_UP,
        Which::Vault => &VAULT_UP,
        Which::Site => &SITE_UP,
    }
}

fn holds(w: Which) -> bool {
    up(w).load(std::sync::atomic::Ordering::SeqCst)
}

fn slot(w: Which) -> usize {
    match w {
        Which::Photos => 0,
        Which::Vault => 1,
        Which::Site => 2,
    }
}

/// this install's pairing names, kept beside its other state
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

/// what the window's Photos page asks for once (photos.rs), so it is in no
/// address
pub fn set_window_key(hand: &str) {
    if let Ok(mut k) = WINDOW_KEY.lock() {
        k.set(hand.to_string(), Instant::now());
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
/// install's pairing page for the service, and the app's window at the page
/// where the member lets that browser in. A link from anywhere only does
/// that; nothing goes back to what sent it.
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
        Ok(p) => serde_json::Value::Object(
            SERVICES
                .iter()
                .map(|w| (w.name().to_string(), view_json(p[slot(*w)].view(now))))
                .collect(),
        ),
        Err(_) => serde_json::json!({}),
    }
}

/// the member's click in the app's window: let in the browser showing
/// `code`, or refuse it
pub fn pair_answer(service: &str, code: Option<&str>) -> Result<(), String> {
    let w = Which::of(service).ok_or("no such service")?;
    let mut p = PAIRS.lock().map_err(|e| e.to_string())?;
    match code {
        Some(c) => p[slot(w)].allow(c, Instant::now()).map_err(Into::into),
        None => {
            p[slot(w)].refuse();
            Ok(())
        }
    }
}

/// The servers, for the life of the app. A port already taken (a second
/// copy of the app) leaves that one unserved, said in the log.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    for which in SERVICES {
        let port = which.port();
        let var = match which {
            Which::Photos => Some("COMMONTY_PHOTOS_WEB"),
            Which::Vault => Some("COMMONTY_VAULT_WEB"),
            Which::Site => None,
        };
        let site = Arc::new(Site {
            app: app.clone(),
            which,
            root: var.and_then(std::env::var_os).map(PathBuf::from),
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
    let cookie_ok = match &via {
        Via::Window => cookie(h, handoff::WINDOW_COOKIE) == Some(crate::site::launch_secret()),
        Via::Browser(label) => SESSIONS
            .lock()
            .is_ok_and(|s| s.valid(site.which, label, cookie(h, handoff::BROWSER_COOKIE))),
        Via::Pair => false,
    };
    match handoff::admit(
        site.which,
        &via,
        cookie_ok,
        &method,
        &path,
        header_str(h, "origin"),
        header_str(h, "sec-fetch-site"),
        &own,
    ) {
        Admit::Refuse(code, why) => {
            return plain(
                StatusCode::from_u16(code).unwrap_or(StatusCode::FORBIDDEN),
                why,
            );
        }
        Admit::Pairing => return pair(&site, req, &path).await,
        Admit::Door => {
            return match &via {
                Via::Browser(label) => enter_browser(&req, site.which, label),
                _ => enter_window(&req),
            };
        }
        Admit::Serve => {}
    }
    // a browser's or the window's own asks, the same on every server
    match (method.as_str(), path.as_str()) {
        ("POST", "/_dd/app/door") => return door_for(&site, &via, req).await,
        // a link on the app's own pages to another of its services: the
        // same, followed at once (a page from anywhere else brings no cookie)
        ("GET", "/_dd/app/go") => {
            let q = req.uri().query().unwrap_or("");
            let service = handoff::query(q, "service").map(handoff::dec);
            let to = handoff::query(q, "path").map(handoff::dec);
            return match go_for(&site, &via, service.as_deref(), to.as_deref()).await {
                Ok(go) => Response::builder()
                    .status(StatusCode::SEE_OTHER)
                    .header(header::LOCATION, go)
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Body::empty())
                    .unwrap_or_default(),
                Err((status, why)) => plain(status, &why),
            };
        }
        ("POST", "/_dd/app/leave") => {
            if let (Via::Browser(label), Ok(mut s)) = (&via, SESSIONS.lock()) {
                s.end(label);
            }
            return json(StatusCode::OK, serde_json::json!({}));
        }
        _ => {}
    }
    match site.which {
        Which::Photos => photos(&site, req, &path, &via).await,
        Which::Vault => vault(&site, req, &path).await,
        Which::Site => site_pages(&site, req, &path).await,
    }
}

/// Another service, for the browser or window already in: a session of its
/// own there and its door, without the member's click again (they let this
/// browser in already). The page goes on to the address it is given.
async fn door_for<R: Runtime>(site: &Site<R>, via: &Via, req: Request) -> Response {
    let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else {
        return plain(StatusCode::BAD_REQUEST, "too much");
    };
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    match go_for(site, via, v["service"].as_str(), v["path"].as_str()).await {
        Ok(go) => json(StatusCode::OK, serde_json::json!({ "go": go })),
        Err((status, why)) => plain(status, &why),
    }
}

async fn go_for<R: Runtime>(
    site: &Site<R>,
    via: &Via,
    service: Option<&str>,
    path: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    let w = service
        .and_then(Which::of)
        .ok_or((StatusCode::BAD_REQUEST, "no such service".to_string()))?;
    let path = path.and_then(handoff::safe_then).map(str::to_string);
    let then = handoff::then_for(&handoff::Link { which: w, path });
    handoff::door_if_up(holds(w), w.port()).map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    let hand = if w == Which::Photos {
        Some(
            crate::photos::hand(&site.app)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e))?,
        )
    } else {
        None
    };
    Ok(match via {
        // the window has the site's pages on its own scheme
        Via::Window if w == Which::Site => format!("{}{then}", crate::site::origin()),
        Via::Window => {
            if let Some(h) = &hand {
                set_window_key(h);
            }
            door(w == Which::Vault, &then).map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?
        }
        Via::Browser(_) => {
            let (label, k) = SESSIONS
                .lock()
                .map(|mut s| s.create(w, &then, hand, Instant::now()))
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "no session".to_string()))?;
            handoff::session_door(w, &label, &k)
        }
        Via::Pair => return Err((StatusCode::FORBIDDEN, "not let in".to_string())),
    })
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
    let then = {
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
        match p.finish(v["verifier"].as_str().unwrap_or(""), now) {
            Finish::Wait => return json(StatusCode::ACCEPTED, serde_json::json!({})),
            Finish::No => return json(StatusCode::FORBIDDEN, serde_json::json!({})),
            Finish::In(then) => then,
        }
    };
    // what the Photos page is handed once it is in (photos.rs)
    let hand = if site.which == Which::Photos {
        match crate::photos::hand(&site.app).await {
            Ok(h) => Some(h),
            Err(e) => return plain(StatusCode::BAD_GATEWAY, &e),
        }
    } else {
        None
    };
    let Ok((label, k)) = SESSIONS
        .lock()
        .map(|mut s| s.create(site.which, &then, hand, now))
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

/// What the page that opens Photos is handed, once, to the window or the
/// browser session the app let in with it (photos.rs, `hand`).
fn photos_key(req: &Request, via: &Via) -> Response {
    if req.method() != "POST" {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "POST");
    }
    let now = Instant::now();
    let hand = match via {
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
    match hand.and_then(|h| serde_json::from_str::<serde_json::Value>(&h).ok()) {
        Some(v) => json(StatusCode::OK, v),
        None => plain(StatusCode::NOT_FOUND, "open Photos from the Commonty app"),
    }
}

async fn photos<R: Runtime>(site: &Site<R>, req: Request, path: &str, via: &Via) -> Response {
    // the page that opens the account, and the app's own scripts and wasm
    if path == "/_dd/photos" || path == "/_dd/photos/" {
        return app_asset(&site.app, "photos-open.html");
    }
    if path == "/_dd/app/photos-open.js" {
        return app_asset(&site.app, "photos-open.js");
    }
    // Ente's "confirm it's you" asks this module for the password (the menu
    // patch, servers/photos): the app's, never a passkey's
    if path == "/_dd/static/photos-passkey.js" {
        return app_asset(&site.app, "photos-confirm.js");
    }
    match (req.method().as_str(), path) {
        (_, "/_dd/app/photos-key") => return photos_key(&req, via),
        ("POST", "/_dd/app/photos-keyed") => {
            let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else {
                return plain(StatusCode::BAD_REQUEST, "too much");
            };
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            return match crate::photos::keyed(&site.app, v["pw"].as_str().unwrap_or("")).await {
                Ok(()) => json(StatusCode::OK, serde_json::json!({})),
                Err(e) => plain(StatusCode::CONFLICT, &e),
            };
        }
        ("POST", "/_dd/app/photos-confirm") => {
            return match crate::photos::current(&site.app) {
                Some(pw) => json(StatusCode::OK, serde_json::json!({ "pw": pw })),
                None => plain(StatusCode::NOT_FOUND, "Photos has no key on this device"),
            };
        }
        ("POST", "/_dd/app/photos-move") => {
            return match crate::photos::start_move(&site.app).await {
                Ok(()) => json(StatusCode::OK, crate::photos::moving(&site.app)),
                Err(e) => plain(StatusCode::BAD_GATEWAY, &e),
            };
        }
        ("GET", "/_dd/app/photos-move") => {
            return json(StatusCode::OK, crate::photos::moving(&site.app));
        }
        _ => {}
    }
    for (prefix, dir) in [("/_dd/static/", "_dd/static"), ("/_dd/web/", "_dd/web")] {
        if let Some(f) = path.strip_prefix(prefix) {
            if f.contains('/') || f.starts_with('.') {
                return plain(StatusCode::NOT_FOUND, "no such file");
            }
            return app_asset(&site.app, &format!("{dir}/{f}"));
        }
    }
    let token = match crate::site::token(&site.app) {
        Ok(t) => t,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &e),
    };
    // museum on the photos site: making the account sends the two calls
    // that need the fleet's code, which the gate there fills in for this
    // device's member (servers/gate/src/photos.rs, museum_verify)
    if path.starts_with("/_dd/museum/") {
        let base = format!("https://photos.{}", crate::account::domain());
        return forward(&base, Some(&token), req, false).await;
    }
    // the gate's: Photos' config, the bar's /_dd/me
    if path.starts_with("/_dd/") {
        let base = match media::gate::files_base(&crate::account::dirs()) {
            Ok(b) => b,
            Err(e) => return plain(StatusCode::BAD_GATEWAY, &e.to_string()),
        };
        return forward(&base, Some(&token), req, false).await;
    }
    // Ente's own files: a page by its name, its name with .html, or the app
    // shell (as nginx served them, try_files $uri $uri.html /index.html)
    client(site, "photos", path, true).await
}

/// The site's pages that hold a key, for a browser the member let in: the
/// copies the app carries (web/_dd, the site's own files), the library's
/// key and the archive password from this device's keys, and every other
/// call sent on to the boxes with this device's token. What a box answers
/// is data: `forward` refuses anything a page would run.
async fn site_pages<R: Runtime>(site: &Site<R>, req: Request, path: &str) -> Response {
    if let Some(p) = crate::site::page_for(path) {
        return match site.app.asset_resolver().get(format!("_dd/pages/{p}.html")) {
            Some(a) => Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(String::from_utf8_lossy(&a.bytes).replacen(
                    "</head>",
                    &format!("{}</head>", crate::site::domain_meta()),
                    1,
                )))
                .unwrap_or_default(),
            None => plain(StatusCode::NOT_FOUND, "not in the app"),
        };
    }
    for (prefix, dir) in [("/_dd/static/", "_dd/static"), ("/_dd/web/", "_dd/web")] {
        if let Some(f) = path.strip_prefix(prefix) {
            if f.contains('/') || f.starts_with('.') {
                return plain(StatusCode::NOT_FOUND, "no such file");
            }
            return app_asset(&site.app, &format!("{dir}/{f}"));
        }
    }
    let answer = |r: Result<Option<serde_json::Value>, String>, none: &str| match r {
        Ok(Some(v)) => json(StatusCode::OK, v),
        Ok(None) => plain(StatusCode::NOT_FOUND, none),
        Err(e) => plain(StatusCode::BAD_GATEWAY, &e),
    };
    match path {
        "/_dd/app/library" => {
            return answer(
                crate::site::library_value(&site.app).await,
                "no library opens here",
            );
        }
        "/_dd/app/archive" => {
            return answer(
                crate::site::archive_value(&site.app).await,
                "your disk images do not open on this device yet",
            );
        }
        // leaving is undone within its window, so a browser let in may ask
        // it too, with the key this device holds
        "/_dd/app/leaving" if req.method() == "POST" => {
            let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else {
                return plain(StatusCode::BAD_REQUEST, "too much");
            };
            return match crate::site::leave(&site.app, &body).await {
                Ok(v) => json(StatusCode::OK, v),
                Err((st, why)) => plain(st, &why),
            };
        }
        // a device is added or removed with the account's main key, from the
        // app's own window only (site.rs)
        p if p.starts_with("/_dd/app/") => {
            return plain(StatusCode::NOT_FOUND, "in the app's window");
        }
        _ => {}
    }
    let token = match crate::site::token(&site.app) {
        Ok(t) => t,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &e),
    };
    // Chat's model and search, on the model box (servers/chat)
    if path.starts_with("/v1/") || path == "/running" || path == "/search" {
        let base = format!("https://llm.{}", crate::account::domain());
        return forward(&base, Some(&token), req, false).await;
    }
    if path.starts_with("/_dd/") {
        let base = match media::gate::files_base(&crate::account::dirs()) {
            Ok(b) => b,
            Err(e) => return plain(StatusCode::BAD_GATEWAY, &e.to_string()),
        };
        return forward(&base, Some(&token), req, false).await;
    }
    plain(StatusCode::NOT_FOUND, "not in the app")
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
/// so its redirects to itself come back here, and its sign-in's binding
/// cookie passes both ways.
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
    // the vault's sign-in binding, and no other cookie (handoff.rs)
    if same_site
        && let Some(c) = handoff::binding_cookie_for(
            req.uri().path(),
            req.headers()
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok()),
        )
    {
        out = out.header(header::COOKIE.as_str(), c);
    }
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
        if k == header::SET_COOKIE && same_site {
            if let Some(c) = v.to_str().ok().and_then(handoff::binding_set_cookie)
                && let Ok(h) = HeaderValue::from_str(&c)
            {
                back = back.header(k, h);
            }
            continue;
        }
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
