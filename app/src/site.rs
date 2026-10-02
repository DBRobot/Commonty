//! The site's own pages, in the app. The app carries the signed-in pages
//! and what they load (web/_dd, the box's own files, kept identical by a
//! check in the flake) and serves them to itself under its own address,
//! so they open without a round trip. What a page asks the box for goes
//! to the box over the fleet's network, with this device's token instead
//! of a browser's session; the library's key comes from this device's own
//! keys instead of a passkey.

use auth::KeyStore as _;
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
        "/_dd/settings" | "/_dd/backups" | "/_dd/devices" | "/_dd/network" | "/_dd/boxes"
        | "/_dd/storage" => "settings",
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

/// A secret made at each launch, handed to the app's own pages in a meta tag
/// and asked of every request to the app's own routes (/_dd/app/...): what
/// speaks for the device's keys is a page the app wrote, and nothing a box
/// or a link could put in front of it.
fn launch_secret() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let mut b = [0u8; 24];
        getrandom::fill(&mut b).expect("random");
        b.iter().map(|x| format!("{x:02x}")).collect()
    })
}

fn has_secret(req: &Request<Vec<u8>>) -> bool {
    req.headers()
        .get("x-dd-app")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == launch_secret())
}

/// A type the window would run, or show as a page of its own
fn runnable(kind: &str) -> bool {
    ["html", "xml", "svg", "javascript", "ecmascript", "css"]
        .iter()
        .any(|k| kind.contains(k))
}

/// One of the app's own pages, with the launch secret in it
fn page_asset<R: Runtime>(app: &AppHandle<R>, path: &str) -> Response<Vec<u8>> {
    match app.asset_resolver().get(path.to_string()) {
        Some(a) => {
            let html = String::from_utf8_lossy(&a.bytes).replacen(
                "</head>",
                &format!(
                    "<meta name=\"dd-app\" content=\"{}\"></head>",
                    launch_secret()
                ),
                1,
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(html.into_bytes())
                .unwrap_or_default()
        }
        None => plain(StatusCode::NOT_FOUND, "not in the app"),
    }
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
        return Ok(page_asset(app, &format!("_dd/pages/{p}.html")));
    }
    // the page code's wasm and players: carried, never fetched from a box
    if let Some(f) = path.strip_prefix("/_dd/web/") {
        if f.contains('/') || f.starts_with('.') {
            return Ok(plain(StatusCode::NOT_FOUND, "no such file"));
        }
        return Ok(asset(app, &format!("_dd/web/{f}")));
    }
    // the app's own routes answer the app's own pages alone
    if path.starts_with("/_dd/app/")
        && !matches!(
            path.as_str(),
            "/_dd/app/photos" | "/_dd/app/photos.js" | "/_dd/app/open"
        )
        && !has_secret(&req)
    {
        return Ok(plain(StatusCode::FORBIDDEN, "not one of the app's pages"));
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
    // Settings > Devices in this window: whether the account's main key is
    // here, and approving a device added by QR code with it
    if path == "/_dd/app/root" {
        let keys = app.state::<crate::account::Keys>();
        let here = account::load_root(&keys.0).ok().flatten().is_some();
        return Ok(json_response(&serde_json::json!({ "here": here })));
    }
    if path == "/_dd/app/admit" && req.method() == "POST" {
        return admit(app, req.body()).await;
    }
    if path == "/_dd/app/remove" && req.method() == "POST" {
        return remove(app, req.body()).await;
    }
    // Photos in this window (photos.rs): the page that signs the window in,
    // and what it asks
    if path == "/_dd/app/photos" {
        return Ok(page_asset(app, "photos.html"));
    }
    // its script: without this route the page never got past "Opening"
    if path == "/_dd/app/photos.js" {
        return Ok(asset(app, "photos.js"));
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
    // What a box answers is data for the app's pages - json, files, media -
    // and never something the window would run or show as a page of its
    // own: that would speak with the app's voice, keys and all. So no html,
    // xml, svg or script passes, nothing goes without its type, and the
    // browser is told not to guess one.
    let kind = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if runnable(&kind) {
        return Ok(plain(
            StatusCode::BAD_GATEWAY,
            "the box answered with a page; the app shows only its own",
        ));
    }
    let mut back = Response::builder()
        .status(r.status().as_u16())
        .header("x-content-type-options", "nosniff")
        .header("content-security-policy", "sandbox; default-src 'none'");
    if kind.is_empty() {
        back = back.header("content-type", "application/octet-stream");
    }
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
fn json_response(v: &serde_json::Value) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(v.to_string().into_bytes())
        .unwrap_or_default()
}

/// A device's key into the account, signed by the main key on this device:
/// what a page in this window asks once both screens showed the same digits.
async fn admit<R: Runtime>(app: &AppHandle<R>, body: &[u8]) -> Result<Response<Vec<u8>>, String> {
    let keys = app.state::<crate::account::Keys>();
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let pk = v["public_key"].as_str().ok_or("no key to add")?;
    let Some(root) = account::load_root(&keys.0).map_err(|e| e.to_string())? else {
        return Ok(plain(
            StatusCode::FORBIDDEN,
            "the account's main key is not on this device",
        ));
    };
    let name = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    account::admit(&crate::account::dirs(), &name, &root, pk)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(json_response(&serde_json::json!({ "ok": true })))
}

/// A device or a passkey out of the account, signed by the main key here.
async fn remove<R: Runtime>(app: &AppHandle<R>, body: &[u8]) -> Result<Response<Vec<u8>>, String> {
    let keys = app.state::<crate::account::Keys>();
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let Some(root) = account::load_root(&keys.0).map_err(|e| e.to_string())? else {
        return Ok(plain(
            StatusCode::FORBIDDEN,
            "the account's main key is not on this device",
        ));
    };
    let name = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    let dirs = crate::account::dirs();
    // this device goes by signing out, not by removing itself
    if let Some(fp) = v["fingerprint"].as_str()
        && let Ok((kp, _)) = auth::device::load_or_create(&keys.0)
        && identity::fingerprint(&auth::device::public_b64(&kp)) == fp
    {
        return Ok(plain(
            StatusCode::BAD_REQUEST,
            "not this device: sign out instead",
        ));
    }
    let done = match (v["fingerprint"].as_str(), v["passkey"].as_str()) {
        (Some(fp), _) => account::remove_device(&dirs, &name, &root, fp).await,
        (_, Some(id)) => account::remove_passkey(&dirs, &name, &root, id).await,
        _ => return Ok(plain(StatusCode::BAD_REQUEST, "remove what?")),
    };
    done.map_err(|e| format!("{e:#}"))?;
    Ok(json_response(&serde_json::json!({ "ok": true })))
}

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

#[cfg(test)]
mod tests {
    #[test]
    fn a_box_sends_data_and_never_a_page() {
        for k in [
            "text/html; charset=utf-8",
            "application/xhtml+xml",
            "image/svg+xml",
            "text/javascript",
            "application/javascript",
            "text/xml",
            "text/css",
        ] {
            assert!(super::runnable(k), "{k}");
        }
        for k in [
            "application/json",
            "video/mp4",
            "application/vnd.apple.mpegurl",
            "image/jpeg",
            "application/octet-stream",
            "text/plain",
        ] {
            assert!(!super::runnable(k), "{k}");
        }
    }
}
