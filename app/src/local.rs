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
//! (`/_dd/app/enter?k=<this launch's secret>`), a browser's on this
//! device's own name for the browsers it lets in, once the member has let
//! that browser in from the app's window (handoff.rs). A browser let in is
//! remembered until the member forgets it, so it is asked once. The site's
//! pages are the window's on its own scheme (site.rs); here they are for a
//! browser alone.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt as _;
use tauri::{AppHandle, Manager as _, Runtime};

use crate::handoff::{self, Admit, Browsers, Finish, Names, Once, Pairing, Via, View, Which};

/// Whether this app holds each port. A window or a browser is only ever
/// sent to a port this app is serving: if something else had one first, it
/// would be what the window or the browser asks its way in, so neither is
/// sent at all.
static PHOTOS_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static VAULT_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SITE_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Photos' pairing, Passwords' and the site's
static PAIRS: LazyLock<Mutex<[Pairing; 3]>> = LazyLock::new(Default::default);
/// the browsers let in, kept in browsers.json for this device's key
static BROWSERS: LazyLock<Mutex<Option<Browsers>>> = LazyLock::new(Default::default);
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

/// This device's key's fingerprint and its own name for let-in browsers
/// (handoff::own_label), once it has a key; asked of the keystore once.
fn own<R: Runtime>(app: &AppHandle<R>) -> Option<(String, String)> {
    static OWN: Mutex<Option<(String, String)>> = Mutex::new(None);
    let mut o = OWN.lock().ok()?;
    if o.is_none() {
        let keys = app.state::<crate::account::Keys>();
        let kp = auth::device::load(&keys.0).ok().flatten()?;
        let fp = identity::fingerprint(&auth::device::public_b64(&kp));
        *o = Some((fp, handoff::own_label(&kp.private().to_bytes()[..])));
    }
    o.clone()
}

fn browsers_file() -> PathBuf {
    crate::paths::data().join("browsers.json")
}

/// The browsers let in, read from their file the first time, for this
/// device; `f` changes them, and what it changed is written back.
fn with_browsers<R: Runtime, T>(
    app: &AppHandle<R>,
    f: impl FnOnce(&mut Browsers) -> (T, bool),
) -> Option<T> {
    let (device, _) = own(app)?;
    let mut b = BROWSERS.lock().ok()?;
    if b.as_ref().is_none_or(|b| b.device() != device) {
        let kept = std::fs::read_to_string(browsers_file()).unwrap_or_default();
        *b = Some(Browsers::from_json(&device, &kept));
    }
    let b = b.as_mut()?;
    let (out, changed) = f(b);
    if changed && let Err(e) = keep(&browsers_file(), &b.to_json()) {
        eprintln!("local: browsers: {e}");
    }
    Some(out)
}

/// written whole, readable by this account alone
fn keep(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.new");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(0o600);
    }
    std::io::Write::write_all(&mut o.open(&tmp)?, text.as_bytes())?;
    std::fs::rename(&tmp, path)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The browsers this device let in, for the app's own screen: which, when
/// let in and last seen
#[tauri::command]
pub fn browsers(app: AppHandle) -> Vec<serde_json::Value> {
    with_browsers(&app, |b| {
        let l = b
            .list()
            .iter()
            .map(|x| serde_json::json!({ "id": x.id, "name": x.name, "made": x.made, "seen": x.seen }))
            .collect();
        (l, false)
    })
    .unwrap_or_default()
}

/// A browser forgotten from the app's own screen: it asks again next time
#[tauri::command]
pub fn browser_forget(app: AppHandle, id: String) -> bool {
    with_browsers(&app, |b| {
        let gone = b.forget(&id);
        (gone, gone)
    })
    .unwrap_or(false)
}

/// Signing out on this device: every browser it let in goes with it
pub fn forget_browsers() {
    if let Ok(mut b) = BROWSERS.lock() {
        *b = None;
    }
    let _ = std::fs::remove_file(browsers_file());
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
        if holds(link.which) && holds(Which::Site) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handoff::door_if_up(holds(link.which), link.which.port())?;
    // every service pairs on the site's page
    handoff::door_if_up(holds(Which::Site), Which::Site.port())?;
    let names = names().ok_or("no names for the browser")?;
    PAIRS.lock().map_err(|e| e.to_string())?[slot(link.which)]
        .arm(&handoff::then_for(link), Instant::now());
    // the window comes up when a browser asks to be let in (pair), not
    // here: a browser let in before goes straight to its page
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
    // what a page shows only as "the gate said 502", said in the log
    if status == StatusCode::BAD_GATEWAY {
        eprintln!("local: 502: {text}");
    }
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

/// the app's own window, by this launch's cookie (Smart home's server too)
pub(crate) fn window_cookie(h: &HeaderMap) -> bool {
    cookie(h, handoff::WINDOW_COOKIE) == Some(crate::site::launch_secret())
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

async fn handle<R: Runtime>(State(site): State<Arc<Site<R>>>, req: Request) -> Response {
    let Some(names) = names() else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "no names for this app");
    };
    let h = req.headers();
    let own_name = own(&site.app).map(|(_, o)| o);
    let Some(via) = handoff::via(
        header_str(h, "host"),
        site.which,
        names,
        own_name.as_deref(),
    ) else {
        return plain(StatusCode::MISDIRECTED_REQUEST, "not this app's address");
    };
    let own_origin = format!("http://{}", header_str(h, "host").unwrap_or_default());
    let path = req.uri().path().to_string();
    let method = req.method().as_str().to_string();
    // which let-in browser this is, by its cookie
    let who = match via {
        Via::Browser => with_browsers(&site.app, |b| {
            let id = b.check(site.which, cookie(h, handoff::BROWSER_COOKIE));
            let changed = id.as_deref().is_some_and(|i| b.seen(i, now_secs()));
            (id, changed)
        })
        .flatten(),
        _ => None,
    };
    let cookie_ok = match &via {
        Via::Window => cookie(h, handoff::WINDOW_COOKIE) == Some(crate::site::launch_secret()),
        Via::Browser => who.is_some(),
        Via::Pair => false,
    };
    let nav = header_str(h, "sec-fetch-mode") == Some("navigate")
        && header_str(h, "sec-fetch-dest") == Some("document");
    match handoff::admit(
        site.which,
        &via,
        cookie_ok,
        &method,
        &path,
        header_str(h, "origin"),
        header_str(h, "sec-fetch-site"),
        nav,
        &own_origin,
    ) {
        Admit::Refuse(code, why) => {
            return plain(
                StatusCode::from_u16(code).unwrap_or(StatusCode::FORBIDDEN),
                why,
            );
        }
        Admit::Pairing => return pair(&site, req, &path).await,
        Admit::GoOn => {
            let at = match req.uri().query() {
                Some(q) => format!("{path}?{q}"),
                None => path.clone(),
            };
            return go_on(handoff::safe_then(&at).unwrap_or("/"));
        }
        Admit::Door => {
            return match &via {
                Via::Browser => enter_browser(&site.app, &req, site.which),
                _ => enter_window(&req),
            };
        }
        Admit::Serve => {}
    }
    if via == Via::Browser && path == "/_dd/app/resume" {
        let q = req.uri().query().unwrap_or("").to_string();
        return resume(
            &site,
            names,
            &q,
            who.as_deref(),
            header_str(h, "sec-fetch-site"),
        )
        .await;
    }
    // a browser's or the window's own asks, the same on every server
    match (method.as_str(), path.as_str()) {
        ("POST", "/_dd/app/door") => return door_for(&site, &via, who.as_deref(), req).await,
        // a link on the app's own pages to another of its services: the
        // same, followed at once (a page from anywhere else brings no cookie)
        ("GET", "/_dd/app/go") => {
            let q = req.uri().query().unwrap_or("");
            let service = handoff::query(q, "service").map(handoff::dec);
            let to = handoff::query(q, "path").map(handoff::dec);
            return match go_for(
                &site,
                &via,
                who.as_deref(),
                service.as_deref(),
                to.as_deref(),
            )
            .await
            {
                Ok(go) => Response::builder()
                    .status(StatusCode::SEE_OTHER)
                    .header(header::LOCATION, go)
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Body::empty())
                    .unwrap_or_default(),
                Err((status, why)) => plain(status, &why),
            };
        }
        // signing out in a browser let in: the app forgets it, and it is
        // asked again next time
        ("POST", "/_dd/app/leave") => {
            if let Some(id) = &who {
                with_browsers(&site.app, |b| {
                    let gone = b.forget(id);
                    (gone, gone)
                });
            }
            return json(StatusCode::OK, serde_json::json!({}));
        }
        _ => {}
    }
    match site.which {
        Which::Photos => photos(&site, req, &path, &via, who.as_deref()).await,
        Which::Vault => vault(&site, req, &path).await,
        Which::Site => site_pages(&site, req, &path, who.as_deref()).await,
    }
}

/// A page that opens `then` on this origin by itself, so the request after
/// it comes from here and carries the Strict cookie
fn go_on(then: &str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header("referrer-policy", "no-referrer")
        .header(
            "content-security-policy",
            "default-src 'none'; frame-ancestors 'none'",
        )
        .body(Body::from(handoff::enter_page(then)))
        .unwrap_or_default()
}

/// A browser let in before, sent here by its pairing page (pair.js keeps
/// this device's name for it there), or just let in: on to the page the
/// link asked for, on whichever service it is, with no code and no click.
/// One the app does not know, or has forgotten, goes back to the pairing
/// page to ask again.
async fn resume<R: Runtime>(
    site: &Site<R>,
    names: &Names,
    query: &str,
    who: Option<&str>,
    fetch_site: Option<&str>,
) -> Response {
    let w = handoff::query(query, "service")
        .and_then(Which::of)
        .unwrap_or(site.which);
    let Some(id) = who else {
        // from the pairing page: another site, so without the cookie yet
        if fetch_site != Some("same-origin") {
            return go_on(&format!("/_dd/app/resume?service={}", w.name()));
        }
        return see_other(&format!(
            "{}/_dd/app/pair?again&service={}",
            names.origin(Which::Site),
            w.name()
        ));
    };
    let now = Instant::now();
    let then = with_browsers(&site.app, |b| (b.take_pending(id, w, now), false))
        .flatten()
        .or_else(|| {
            PAIRS
                .lock()
                .ok()
                .and_then(|mut p| p[slot(w)].take_then(now))
        })
        .unwrap_or_else(|| {
            handoff::then_for(&handoff::Link {
                which: w,
                path: None,
            })
        });
    if w == site.which {
        return see_other(handoff::safe_then(&then).unwrap_or("/"));
    }
    if let Err(e) = handoff::door_if_up(holds(w), w.port()) {
        return plain(StatusCode::SERVICE_UNAVAILABLE, &e);
    }
    let hand = if w == Which::Photos {
        match crate::photos::hand(&site.app).await {
            Ok(h) => Some(h),
            Err(e) => return plain(StatusCode::BAD_GATEWAY, &e),
        }
    } else {
        None
    };
    match browser_door(site, id, w, &then, hand) {
        Ok(go) => see_other(&go),
        Err((status, why)) => plain(status, &why),
    }
}

fn see_other(to: &str) -> Response {
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, to)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::empty())
        .unwrap_or_default()
}

/// Another service, for the browser or window already in: a door into it
/// for the same browser, without the member's click again (they let this
/// browser in already). The page goes on to the address it is given.
async fn door_for<R: Runtime>(
    site: &Site<R>,
    via: &Via,
    who: Option<&str>,
    req: Request,
) -> Response {
    let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else {
        return plain(StatusCode::BAD_REQUEST, "too much");
    };
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    match go_for(site, via, who, v["service"].as_str(), v["path"].as_str()).await {
        Ok(go) => json(StatusCode::OK, serde_json::json!({ "go": go })),
        Err((status, why)) => plain(status, &why),
    }
}

async fn go_for<R: Runtime>(
    site: &Site<R>,
    via: &Via,
    who: Option<&str>,
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
        Via::Browser => {
            let id = who.ok_or((StatusCode::FORBIDDEN, "not let in".to_string()))?;
            browser_door(site, id, w, &then, hand)?
        }
        Via::Pair => return Err((StatusCode::FORBIDDEN, "not let in".to_string())),
    })
}

/// a door into `w` on this device's own name, for browser `id`
fn browser_door<R: Runtime>(
    site: &Site<R>,
    id: &str,
    w: Which,
    then: &str,
    hand: Option<String>,
) -> Result<String, (StatusCode, String)> {
    let gone = || {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "no device key here".to_string(),
        )
    };
    let (_, label) = own(&site.app).ok_or_else(gone)?;
    let k = with_browsers(&site.app, |b| {
        (b.door(id, w, then, hand, Instant::now()), false)
    })
    .flatten()
    .ok_or((StatusCode::FORBIDDEN, "not let in".to_string()))?;
    Ok(handoff::session_door(w, &label, &k))
}

/// The window's door: the app's own window, holding this launch's secret,
/// is given the cookie every other request needs. Lax, not Strict: the
/// window arrives here from the app's own scheme, another site to a
/// browser, and a Strict cookie set on that way in is not sent on the
/// redirect that follows it.
pub(crate) fn enter_window(req: &Request) -> Response {
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
        ("POST", "/_dd/app/pair/ask" | "/_dd/app/pair/finish" | "/_dd/app/pair/known") => {}
        _ => return plain(StatusCode::NOT_FOUND, "not here"),
    }
    let agent = header_str(req.headers(), "user-agent")
        .map(handoff::browser_name)
        .unwrap_or_else(|| "A browser".into());
    let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else {
        return plain(StatusCode::BAD_REQUEST, "too much");
    };
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    // every service pairs on the site's page (handoff::browser_url), so a
    // browser keeps one name, for all of them
    let w = v["service"]
        .as_str()
        .and_then(Which::of)
        .unwrap_or(site.which);
    // whether the name a browser kept from being let in before is still
    // this device's: then it goes there itself, and nothing is asked
    if path == "/_dd/app/pair/known" {
        let mine = own(&site.app)
            .is_some_and(|(_, o)| v["host"].as_str() == Some(&handoff::host_port(Which::Site, &o)));
        return json(StatusCode::OK, serde_json::json!({ "known": mine }));
    }
    let now = Instant::now();
    let then = {
        let Ok(mut pairs) = PAIRS.lock() else {
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "no pairing");
        };
        let p = &mut pairs[slot(w)];
        if path == "/_dd/app/pair/ask" {
            return match p.ask(v["hash"].as_str().unwrap_or(""), now) {
                Ok(code) => {
                    // the member's click, in the app's window
                    show_window(&site.app, "/_dd/app/allow");
                    json(StatusCode::OK, serde_json::json!({ "code": code }))
                }
                Err(why) => json(StatusCode::FORBIDDEN, serde_json::json!({ "why": why })),
            };
        }
        match p.finish(v["verifier"].as_str().unwrap_or(""), now) {
            Finish::Wait => return json(StatusCode::ACCEPTED, serde_json::json!({})),
            Finish::No => return json(StatusCode::FORBIDDEN, serde_json::json!({})),
            Finish::In(then) => then,
        }
    };
    // Remembered from now on, on the site's own name first: from there it
    // comes straight in next time, to this service or any other
    let Some(id) = with_browsers(&site.app, |b| {
        let id = b.add(&agent, now_secs());
        b.pending(&id, w, &then, now);
        (id, true)
    }) else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "no device key here");
    };
    let onto = format!("/_dd/app/resume?service={}", w.name());
    match browser_door(site, &id, Which::Site, &onto, None) {
        Ok(go) => json(StatusCode::OK, serde_json::json!({ "go": go })),
        Err((status, why)) => plain(status, &why),
    }
}

/// A browser's door on this device's own name: the one-time secret the
/// pairing page was handed, spent here, for this browser's own cookie,
/// kept as long as a browser keeps one.
fn enter_browser<R: Runtime>(app: &AppHandle<R>, req: &Request, which: Which) -> Response {
    let q = req.uri().query().unwrap_or("");
    let k = handoff::query(q, "k").unwrap_or("");
    let got = with_browsers(app, |b| {
        let got = b.enter(which, k, Instant::now());
        let changed = got.is_some();
        (got, changed)
    })
    .flatten();
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
                "{}={c}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
                handoff::BROWSER_COOKIE,
                handoff::BROWSER_LIFE_SECS
            ),
        )
        .body(Body::from(handoff::enter_page(&then)))
        .unwrap_or_default()
}

/// What the page that opens Photos is handed, once, to the window or the
/// browser the app let in with it (photos.rs, `hand`).
fn photos_key<R: Runtime>(
    app: &AppHandle<R>,
    req: &Request,
    via: &Via,
    who: Option<&str>,
) -> Response {
    if req.method() != "POST" {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "POST");
    }
    let now = Instant::now();
    let hand = match via {
        Via::Window => WINDOW_KEY.lock().ok().and_then(|mut k| k.take(now)),
        Via::Browser => who.and_then(|id| {
            with_browsers(app, |b| (b.take_key(Which::Photos, id, now), false)).flatten()
        }),
        Via::Pair => None,
    };
    match hand.and_then(|h| serde_json::from_str::<serde_json::Value>(&h).ok()) {
        Some(v) => json(StatusCode::OK, v),
        None => plain(StatusCode::NOT_FOUND, "open Photos from the Commonty app"),
    }
}

async fn photos<R: Runtime>(
    site: &Site<R>,
    req: Request,
    path: &str,
    via: &Via,
    who: Option<&str>,
) -> Response {
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
        (_, "/_dd/app/photos-key") => return photos_key(&site.app, &req, via, who),
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

/// what a page the app hands a browser may load and run
const PAGE_POLICY: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data: https:; media-src 'self' blob:; connect-src 'self' https://api.themoviedb.org https://image.tmdb.org; frame-src 'self' blob:; worker-src 'self' blob:; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'self'";

/// The site's pages that hold a key, for a browser the member let in: the
/// copies the app carries (web/_dd, the site's own files), the library's
/// key and the archive password from this device's keys, and every other
/// call sent on to the boxes with this device's token. What a box answers
/// is data: `forward` refuses anything a page would run.
async fn site_pages<R: Runtime>(
    site: &Site<R>,
    req: Request,
    path: &str,
    who: Option<&str>,
) -> Response {
    if let Some(p) = crate::site::page_for(path) {
        return match crate::site::carried(&site.app, &format!("_dd/pages/{p}.html")) {
            Some(a) => Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                // the app window's own policy (tauri.conf.json): no script
                // but the app's files, so nothing in a message, a draft or
                // a name a box wrote ever runs where the keys are
                .header("content-security-policy", PAGE_POLICY)
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
        // Mail, as in the app's window (mail.rs): this browser was let in
        p if p.starts_with("/_dd/app/mail/") => {
            let post = req.method() == axum::http::Method::POST;
            let p = p.to_string();
            let Ok(body) = axum::body::to_bytes(req.into_body(), 40 << 20).await else {
                return plain(StatusCode::PAYLOAD_TOO_LARGE, "too much");
            };
            let r = match (p.as_str(), post) {
                ("/_dd/app/mail/key", false) => crate::mail::key(&site.app, false).await,
                ("/_dd/app/mail/key", true) => crate::mail::key(&site.app, true).await,
                ("/_dd/app/mail/worker", true) => crate::mail::ask(&site.app, &body).await,
                ("/_dd/app/mail/vouch", true) => crate::mail::vouch(&site.app, &body).await,
                ("/_dd/app/mail/send", true) => crate::mail::send(&site.app, &body).await,
                _ => Err((404, "not in the app".into())),
            };
            return match r {
                Ok(v) => json(StatusCode::OK, v),
                Err((s, why)) => plain(
                    StatusCode::from_u16(s).unwrap_or(StatusCode::BAD_GATEWAY),
                    &why,
                ),
            };
        }
        // taking everything out, from the Leave tab: this browser's own
        // downloads (export.rs)
        p if p.starts_with("/_dd/app/export") => return export_route(site, who, req, p).await,
        // a device is added or removed, and the account left (which deletes
        // everything at once), with the account's main key, from the app's
        // own window only (site.rs)
        p if p.starts_with("/_dd/app/") => {
            return plain(StatusCode::NOT_FOUND, "in the app's window");
        }
        _ => {}
    }
    if matches!(path, "/_dd/login" | "/_dd/logout") {
        return plain(StatusCode::UNAUTHORIZED, crate::site::SIGNED_OUT);
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

/// The Leave tab's downloads, for the browser session that started them
/// (export.rs): start one, open it, follow it, feed the library's zip in,
/// end or stop it
async fn export_route<R: Runtime>(
    site: &Site<R>,
    who: Option<&str>,
    req: Request,
    path: &str,
) -> Response {
    let Some(id) = who else {
        return plain(StatusCode::FORBIDDEN, "not let in");
    };
    let owner = crate::export::Owner::Browser(id.to_string());
    let q = req.uri().query().unwrap_or("").to_string();
    let id = handoff::query(&q, "id")
        .map(handoff::dec)
        .unwrap_or_default();
    let refused = |(code, why): crate::export::Refused| {
        plain(
            StatusCode::from_u16(code).unwrap_or(StatusCode::FORBIDDEN),
            &why,
        )
    };
    let method = req.method().as_str().to_string();
    match (method.as_str(), path) {
        ("POST", "/_dd/app/export") => {
            let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else {
                return plain(StatusCode::BAD_REQUEST, "too much");
            };
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let Some(kind) = v["kind"].as_str().and_then(crate::export::Kind::of) else {
                return plain(StatusCode::BAD_REQUEST, "photos or files");
            };
            match crate::export::start(&site.app, owner, kind).await {
                Ok(v) => json(StatusCode::OK, v),
                Err(e) => refused(e),
            }
        }
        ("GET", "/_dd/app/export/zip") => match crate::export::take_download(&owner, &id) {
            Ok((stream, name)) => Response::builder()
                .header(header::CONTENT_TYPE, "application/zip")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                )
                .header(header::CACHE_CONTROL, "no-store")
                .header("x-content-type-options", "nosniff")
                .body(Body::from_stream(stream))
                .unwrap_or_default(),
            Err(e) => refused(e),
        },
        ("GET", "/_dd/app/export/state") => match crate::export::state(&owner, &id) {
            Ok(v) => json(StatusCode::OK, v),
            Err(e) => refused(e),
        },
        ("POST", "/_dd/app/export/chunk") => {
            let Ok(body) = axum::body::to_bytes(req.into_body(), crate::export::MOST).await else {
                return plain(StatusCode::PAYLOAD_TOO_LARGE, "too much at once");
            };
            match crate::export::chunk(&owner, &id, body.to_vec()).await {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => refused(e),
            }
        }
        ("POST", "/_dd/app/export/end") => match crate::export::end(&owner, &id).await {
            Ok(v) => json(StatusCode::OK, v),
            Err(e) => refused(e),
        },
        ("POST", "/_dd/app/export/cancel") => match crate::export::cancel(&owner, &id).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => refused(e),
        },
        _ => plain(StatusCode::NOT_FOUND, "no such download"),
    }
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
    match crate::site::carried(app, path) {
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
        if let Some(a) = crate::site::carried(&site.app, &t) {
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

/// One client for every call the pages make, through the network's proxy
/// while it is up. A new one for each call was a new handshake through the
/// proxy for each of a page's dozens of requests. Made again when the proxy
/// changes (the network came up again on another port).
pub(crate) fn http() -> Result<reqwest::Client, String> {
    static C: Mutex<Option<(Option<String>, reqwest::Client)>> = Mutex::new(None);
    let via = directory::proxy().map(|p| format!("{}\0{}\0{}", p.addr, p.user, p.password));
    let mut c = C.lock().map_err(|e| e.to_string())?;
    if let Some((v, client)) = c.as_ref()
        && *v == via
    {
        return Ok(client.clone());
    }
    let client = directory::builder()
        .and_then(|b| {
            b.redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .build()
                .map_err(Into::into)
        })
        .map_err(|e| e.to_string())?;
    *c = Some((via, client.clone()));
    Ok(client)
}

/// The network comes up a few seconds after the window, and a page's first
/// calls went straight out before it and failed ("error sending request",
/// the bar's "the box said 502"). On a device that has joined, they wait
/// for it, a while.
pub(crate) async fn network_ready() {
    if !crate::net::joined() {
        return;
    }
    for _ in 0..60 {
        if directory::proxy().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// A call to a box, sent again when it found no way through yet (the
/// network still finding the box), if it has no body to send twice
pub(crate) async fn send(mut out: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    for _ in 0..3 {
        let Some(again) = out.try_clone() else { break };
        match out.send().await {
            Err(e) if e.is_connect() => {
                eprintln!(
                    "local: {}: {}, again",
                    e.url().map(|u| u.path()).unwrap_or(""),
                    why(&e)
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
                out = again;
            }
            r => return r,
        }
    }
    out.send().await
}

/// an error and what lies under it: reqwest's own says only that a request
/// was not sent
pub(crate) fn why(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut at = e.source();
    while let Some(c) = at {
        s.push_str(": ");
        s.push_str(&c.to_string());
        at = c.source();
    }
    s
}

/// whether a request carries a body: a length above nothing, or chunks
pub(crate) fn has_body(h: &HeaderMap) -> bool {
    h.contains_key(header::TRANSFER_ENCODING)
        || h.get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > 0)
}

/// The page's call, sent on to a box over the fleet's network and answered
/// back as it came, streamed both ways. `bearer`: this device's token for
/// the gate. `same_site`: the box is the page's own server (Vaultwarden),
/// so its redirects to itself come back here, and its sign-in's binding
/// cookie passes both ways.
pub(crate) async fn forward(
    base: &str,
    bearer: Option<&str>,
    req: Request,
    same_site: bool,
) -> Response {
    let target = match req.uri().query() {
        Some(q) => format!("{base}{}?{q}", req.uri().path()),
        None => format!("{base}{}", req.uri().path()),
    };
    network_ready().await;
    let client = match http() {
        Ok(c) => c,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &e),
    };
    let Ok(method) = reqwest::Method::from_bytes(req.method().as_str().as_bytes()) else {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "no such method");
    };
    let method_name = method.as_str().to_string();
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
        ) || (bearer.is_some() && k == header::AUTHORIZATION)
        {
            continue;
        }
        out = out.header(k.as_str(), v.as_bytes());
    }
    if let Some(t) = bearer {
        out = out.bearer_auth(t);
    }
    // a body only where the page sent one: a GET or a PROPFIND with an
    // empty stream went out chunked
    if has_body(req.headers()) {
        let body = req.into_body().into_data_stream();
        out = out.body(reqwest::Body::wrap_stream(body));
    }
    let r = match send(out).await {
        Ok(r) => r,
        Err(e) => return plain(StatusCode::BAD_GATEWAY, &why(&e)),
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
    if !crate::site::data_from_box(&method_name, r.status().as_u16(), &kind) {
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
/// the app's own copy of the vault, through its door, at `to` (a path of
/// the vault's own, like its Export screen) or its front.
pub fn vault_state<R: Runtime>(
    app: &AppHandle<R>,
    to: Option<&str>,
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
        "door": door(
            true,
            to.and_then(handoff::safe_then)
                .filter(|t| !t.starts_with("/_dd/"))
                .unwrap_or("/")
        )?,
    });
    tauri::http::Response::builder()
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(v.to_string().into_bytes())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(b: &[u8]) -> String {
        use sha2::Digest as _;
        sha2::Sha256::digest(b)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect()
    }

    /// A box on a loopback port, as the pages' calls reach one: a folder's
    /// listing, its own sign-in page, and what it was sent for anything else
    async fn a_box() -> String {
        let app = axum::Router::new().fallback(|req: Request| async move {
            let (parts, body) = req.into_parts();
            let body = axum::body::to_bytes(body, 16 << 20)
                .await
                .unwrap_or_default();
            let h = |n: &str| {
                parts
                    .headers
                    .get_all(n)
                    .iter()
                    .filter_map(|v| v.to_str().ok().map(str::to_string))
                    .collect::<Vec<_>>()
            };
            match (parts.method.as_str(), parts.uri.path()) {
                ("PROPFIND", _) => (
                    StatusCode::MULTI_STATUS,
                    [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                    "<d:multistatus xmlns:d=\"DAV:\"/>".to_string(),
                )
                    .into_response(),
                (_, "/_dd/login") => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html")],
                    "<script>sign in</script>".to_string(),
                )
                    .into_response(),
                (_, "/listing.xml") => (
                    StatusCode::MULTI_STATUS,
                    [(header::CONTENT_TYPE, "application/xml")],
                    "<x/>".to_string(),
                )
                    .into_response(),
                _ => json(
                    StatusCode::OK,
                    serde_json::json!({
                        "authorization": h("authorization"),
                        "cookie": h("cookie"),
                        "chunked": h("transfer-encoding"),
                        "length": h("content-length"),
                        "body": String::from_utf8_lossy(&body[..body.len().min(1024)]),
                        "size": body.len(),
                        "sha256": sha(&body),
                    }),
                ),
            }
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = l.local_addr().unwrap();
        tauri::async_runtime::spawn(async move { axum::serve(l, app).await });
        format!("http://{at}")
    }

    fn ask(method: &str, path: &str, headers: &[(&str, &str)], body: &'static str) -> Request {
        let mut r = axum::http::Request::builder().method(method).uri(path);
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        r.body(Body::from(body)).unwrap()
    }

    async fn text(r: Response) -> (StatusCode, String) {
        let st = r.status();
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (st, String::from_utf8_lossy(&b).into_owned())
    }

    /// Files and Movies & TV list a folder with a PROPFIND, which a box
    /// answers in XML: refused as code, every listing was a 502
    #[test]
    fn a_folders_listing_passes_and_a_page_from_the_box_does_not() {
        tauri::async_runtime::block_on(async {
            let base = a_box().await;
            let r = forward(
                &base,
                Some("t"),
                ask("PROPFIND", "/_dd/dav/l/", &[("depth", "1")], ""),
                false,
            )
            .await;
            assert_eq!(r.status(), StatusCode::MULTI_STATUS);
            assert_eq!(
                r.headers().get("content-security-policy").unwrap(),
                "sandbox; default-src 'none'"
            );
            assert!(text(r).await.1.contains("multistatus"));
            // XML to anything but a listing, and a page, are still code
            let (st, why) =
                text(forward(&base, Some("t"), ask("GET", "/listing.xml", &[], ""), false).await)
                    .await;
            assert_eq!(st, StatusCode::BAD_GATEWAY, "{why}");
            let (st, _) =
                text(forward(&base, Some("t"), ask("GET", "/_dd/login", &[], ""), false).await)
                    .await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
        });
    }

    /// The box gets this device's token alone: not the browser's cookie,
    /// not an Authorization of the page's own, and no body where the page
    /// sent none
    #[test]
    fn a_call_goes_on_with_the_devices_token_and_its_own_body_alone() {
        tauri::async_runtime::block_on(async {
            let base = a_box().await;
            let r = forward(
                &base,
                Some("device"),
                ask(
                    "GET",
                    "/_dd/me",
                    &[
                        ("authorization", "Bearer page"),
                        ("cookie", "dd_browser=secret"),
                    ],
                    "",
                ),
                false,
            )
            .await;
            let (st, seen) = text(r).await;
            assert_eq!(st, StatusCode::OK);
            let seen: serde_json::Value = serde_json::from_str(&seen).unwrap();
            assert_eq!(seen["authorization"], serde_json::json!(["Bearer device"]));
            assert_eq!(seen["cookie"], serde_json::json!([]));
            assert_eq!(
                seen["chunked"],
                serde_json::json!([]),
                "a GET goes without a body"
            );
            let r = forward(
                &base,
                Some("device"),
                ask("PUT", "/_dd/dav/l/x", &[("content-length", "3")], "abc"),
                false,
            )
            .await;
            let seen: serde_json::Value = serde_json::from_str(&text(r).await.1).unwrap();
            assert_eq!(seen["body"], "abc");
        });
    }

    /// A large file goes up a piece at a time (library.js put: a 6 MiB
    /// part, sealed, by PUT): each part reaches the box whole, whether the
    /// page said its length or sent it in chunks
    #[test]
    fn a_large_upload_reaches_the_box_whole() {
        tauri::async_runtime::block_on(async {
            let base = a_box().await;
            let part: Vec<u8> = (0..7 * 1024 * 1024 + 123)
                .map(|i| (i % 251) as u8)
                .collect();
            let want = sha(&part);
            for chunked in [false, true] {
                let mut r = axum::http::Request::builder()
                    .method("PUT")
                    .uri("/_dd/dav/l/big")
                    .header("x-dd-upload", "u")
                    .header("x-dd-part", "1");
                let body = if chunked {
                    r = r.header("transfer-encoding", "chunked");
                    let pieces: Vec<Result<Vec<u8>, std::io::Error>> =
                        part.chunks(64 * 1024).map(|c| Ok(c.to_vec())).collect();
                    Body::from_stream(futures_util::stream::iter(pieces))
                } else {
                    r = r.header("content-length", part.len().to_string());
                    Body::from(part.clone())
                };
                let resp = forward(&base, Some("t"), r.body(body).unwrap(), false).await;
                let (st, seen) = text(resp).await;
                assert_eq!(st, StatusCode::OK, "{seen}");
                let seen: serde_json::Value = serde_json::from_str(&seen).unwrap();
                assert_eq!(seen["size"], part.len(), "chunked: {chunked}");
                assert_eq!(seen["sha256"], want.as_str(), "chunked: {chunked}");
            }
        });
    }

    #[test]
    fn a_body_is_one_the_page_sent() {
        let h = |pairs: &[(&str, &str)]| {
            let mut m = HeaderMap::new();
            for (k, v) in pairs {
                m.insert(
                    axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    HeaderValue::from_str(v).unwrap(),
                );
            }
            m
        };
        assert!(!has_body(&h(&[])));
        assert!(!has_body(&h(&[("content-length", "0")])));
        assert!(has_body(&h(&[("content-length", "12")])));
        assert!(has_body(&h(&[("transfer-encoding", "chunked")])));
    }
}
