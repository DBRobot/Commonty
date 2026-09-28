//! The site's own pages, in the app. The app carries the signed-in pages
//! and what they load (web/_dd, the box's own files, kept identical by a
//! check in the flake) and serves them to itself under its own address,
//! so they open without a round trip. What a page asks the box for goes
//! to the box over the fleet's network, with this device's token instead
//! of a browser's session; the library's key comes from this device's own
//! keys instead of a passkey.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager as _, Runtime, UriSchemeContext, UriSchemeResponder};

pub const SCHEME: &str = "commonty";

/// where the scheme lives for this platform's webview
pub fn origin() -> &'static str {
    if cfg!(any(windows, target_os = "android")) {
        "http://commonty.localhost"
    } else {
        "commonty://localhost"
    }
}

/// the pages the app carries, by the path the site serves them at
fn page_for(path: &str) -> Option<&'static str> {
    Some(match path {
        "/" | "/_dd/home" => "home",
        "/_dd/files" => "files",
        "/_dd/media" => "media",
        "/_dd/boxes" => "boxes",
        "/_dd/backups" => "backups",
        "/_dd/devices" => "devices",
        "/_dd/network" => "network",
        _ => return None,
    })
}

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    req: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let r = serve(&app, req)
            .await
            .unwrap_or_else(|e| plain(StatusCode::BAD_GATEWAY, &e));
        responder.respond(r);
    });
}

fn plain(status: StatusCode, text: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(text.as_bytes().to_vec())
        .unwrap_or_default()
}

fn asset<R: Runtime>(app: &AppHandle<R>, path: &str) -> Response<Vec<u8>> {
    match app.asset_resolver().get(path.to_string()) {
        Some(a) => Response::builder()
            .header(header::CONTENT_TYPE, a.mime_type)
            .header(header::CACHE_CONTROL, "no-cache")
            .body(a.bytes)
            .unwrap_or_default(),
        None => plain(StatusCode::NOT_FOUND, "not in the app"),
    }
}

async fn serve<R: Runtime>(
    app: &AppHandle<R>,
    req: Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>, String> {
    let path = req.uri().path().to_string();
    // Only the app's own pages speak for this device. Anything else in a
    // webview - a page it was sent to, a link it followed - is refused
    // before the device's token is anywhere near it. A navigation carries
    // no Origin; a fetch, a form or an XHR from elsewhere does.
    if let Some(o) = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        && o != origin()
    {
        return Ok(plain(StatusCode::FORBIDDEN, "not one of the app's pages"));
    }
    if let Some(p) = page_for(&path) {
        return Ok(asset(app, &format!("_dd/pages/{p}.html")));
    }
    if let Some(f) = path.strip_prefix("/_dd/static/") {
        if f.contains('/') || f.starts_with('.') {
            return Ok(plain(StatusCode::NOT_FOUND, "no such file"));
        }
        return Ok(asset(app, &format!("_dd/static/{f}")));
    }
    if path == "/_dd/app/library" {
        return library(app).await;
    }
    // Photos in this window (photos.rs): the page that signs the window in,
    // and what it asks
    if path == "/_dd/app/photos" {
        return Ok(asset(app, "photos.html"));
    }
    if path == "/_dd/app/photos/state" {
        return crate::photos::state(app).await;
    }
    if path == "/_dd/app/photos/start" {
        return crate::photos::start(app).await;
    }
    // a service that is not one of these pages (photos, games, code):
    // the device's own browser, which reaches it through the app too
    if path == "/_dd/app/open" {
        use tauri_plugin_opener::OpenerExt as _;
        let url = req
            .uri()
            .query()
            .and_then(|q| q.strip_prefix("url="))
            .map(percent_decode)
            .unwrap_or_default();
        if !url.starts_with("https://") {
            return Ok(plain(StatusCode::BAD_REQUEST, "only a web address"));
        }
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|e| e.to_string())?;
        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .unwrap_or_default());
    }
    if path.starts_with("/_dd/") {
        return proxy(app, req).await;
    }
    Ok(plain(StatusCode::NOT_FOUND, "not in the app"))
}

/// This device's token for the services, made here and kept for most of
/// its hour: every request a page makes carries one.
static TOKEN: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub(crate) fn token<R: Runtime>(app: &AppHandle<R>) -> Result<String, String> {
    let mut t = TOKEN.lock().map_err(|e| e.to_string())?;
    if let Some((tok, made)) = t.as_ref()
        && made.elapsed() < Duration::from_secs(50 * 60)
    {
        return Ok(tok.clone());
    }
    let keys = app.state::<crate::account::Keys>();
    let (_, _, tok) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    *t = Some((tok.clone(), Instant::now()));
    Ok(tok)
}

/// The page's request, sent on to the box's gate over the network: the
/// same path, the headers a page's request means something by, this
/// device's token in place of a session.
async fn proxy<R: Runtime>(
    app: &AppHandle<R>,
    req: Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>, String> {
    let base = media::gate::files_base(&crate::account::dirs()).map_err(|e| e.to_string())?;
    let target = match req.uri().query() {
        Some(q) => format!("{base}{}?{q}", req.uri().path()),
        None => format!("{base}{}", req.uri().path()),
    };
    let method =
        reqwest::Method::from_bytes(req.method().as_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut out = directory::http()
        .map_err(|e| e.to_string())?
        .request(method, &target)
        .bearer_auth(token(app)?);
    for h in [
        "content-type",
        "range",
        "depth",
        "destination",
        "overwrite",
        "x-dd-upload",
        "x-dd-part",
        "x-dd-presign",
        "accept",
    ] {
        if let Some(v) = req.headers().get(h).and_then(|v| v.to_str().ok()) {
            out = out.header(h, v);
        }
    }
    let body = req.into_body();
    if !body.is_empty() {
        out = out.body(body);
    }
    let r = out.send().await.map_err(|e| e.to_string())?;
    let mut back = Response::builder().status(r.status().as_u16());
    for h in [
        "content-type",
        "content-range",
        "accept-ranges",
        "etag",
        "last-modified",
        "cache-control",
    ] {
        if let Some(v) = r.headers().get(h).and_then(|v| v.to_str().ok()) {
            back = back.header(h, v);
        }
    }
    // a box that sends the page elsewhere on its own host sends it to the
    // app's copy of the same path
    if let Some(l) = r.headers().get("location").and_then(|v| v.to_str().ok()) {
        back = back.header("location", l.strip_prefix(&base).unwrap_or(l));
    }
    let bytes = r.bytes().await.map_err(|e| e.to_string())?;
    back.body(bytes.to_vec()).map_err(|e| e.to_string())
}

/// The library a page opens: this device's own keys open the member's
/// library, as the mount and the player do, and the key goes to the page
/// that asked, which is one of the app's own.
async fn library<R: Runtime>(app: &AppHandle<R>) -> Result<Response<Vec<u8>>, String> {
    use base64::Engine as _;
    let keys = app.state::<crate::account::Keys>();
    let (opener, user, _) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let libs = media::gate::openable(&crate::account::dirs(), &user, &opener)
        .await
        .map_err(|e| format!("{e:#}"))?;
    // the member's own first; one shared with them otherwise, to read
    let Some((owner, lib, key)) = libs
        .iter()
        .find(|(o, _, _)| *o == user)
        .or_else(|| libs.first())
    else {
        return Ok(plain(StatusCode::NOT_FOUND, "no library opens here"));
    };
    let v = serde_json::json!({
        "id": lib.id,
        "key": base64::engine::general_purpose::STANDARD.encode(&key[..]),
        "reader": *owner != user,
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(v.to_string().into_bytes())
        .map_err(|e| e.to_string())
}

/// a query value as encodeURIComponent wrote it
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
