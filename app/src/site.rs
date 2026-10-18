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

/// the pages the app carries, by the path the site serves them at; Chat
/// too, for a browser (local.rs), whose calls go to the model box
pub(crate) fn page_for(path: &str) -> Option<&'static str> {
    Some(match path {
        "/" | "/_dd/home" => "home",
        "/_dd/files" => "files",
        "/_dd/media" => "media",
        "/_dd/calendar" => "calendar",
        "/_dd/contacts" => "contacts",
        "/_dd/chat" => "chat",
        "/_dd/mail" => "mail",
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
        let path = req.uri().path().to_string();
        let r = serve(&app, req).await.unwrap_or_else(|e| {
            // said in the log too: a page shows only "the box said 502"
            eprintln!("site: {path}: {e}");
            plain(StatusCode::BAD_GATEWAY, &e)
        });
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
pub(crate) fn launch_secret() -> &'static str {
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

/// What a box may answer a page's call with: data, never code. A folder's
/// listing is XML and still data (WebDAV's 207, to a PROPFIND); refused,
/// Files and Movies & TV listed nothing and said 502. It goes with the
/// sandbox every answer gets, so nothing in it runs.
pub(crate) fn data_from_box(method: &str, status: u16, kind: &str) -> bool {
    let base = kind.split(';').next().unwrap_or("").trim();
    !runnable(kind)
        || (method == "PROPFIND" && status == 207 && matches!(base, "application/xml" | "text/xml"))
}

/// Signing in is the app's, not a box's: a page the app serves that is
/// told 401 lands here, where the box's own sign-in page would be refused
/// as code (502). The device's sign-in is what the boxes did not take.
pub(crate) const SIGNED_OUT: &str = "The boxes did not take this device's sign-in. Open the Commonty app on this computer and check it is signed in, then open the page again.";

/// The fleet's domain as this app knows it, for a page it serves: what the
/// page's links to the fleet are held to (shell.js), never a box's word
pub(crate) fn domain_meta() -> String {
    let d: String = crate::account::domain()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        .collect();
    format!("<meta name=\"dd-domain\" content=\"{d}\">")
}

/// The file the app carries at exactly `path`, or none. Tauri's own lookup
/// falls back to `<path>.html`, `<path>/index.html` and then the app's own
/// first screen, so a file the app does not have came back as that screen,
/// 200 and HTML: a missing script or stylesheet on a page in a browser was
/// the app's sign-in screen instead of a 404.
pub(crate) fn carried<R: Runtime>(app: &AppHandle<R>, path: &str) -> Option<tauri::Asset> {
    static KEYS: std::sync::OnceLock<std::collections::HashSet<String>> =
        std::sync::OnceLock::new();
    let keys = KEYS.get_or_init(|| {
        app.asset_resolver()
            .iter()
            .map(|(k, _)| k.trim_start_matches('/').to_string())
            .collect()
    });
    if !keys.contains(path.trim_start_matches('/')) {
        return None;
    }
    app.asset_resolver().get(path.to_string())
}

/// One of the app's own pages, with the launch secret in it
fn page_asset<R: Runtime>(app: &AppHandle<R>, path: &str) -> Response<Vec<u8>> {
    match carried(app, path) {
        Some(a) => {
            let html = String::from_utf8_lossy(&a.bytes).replacen(
                "</head>",
                &format!(
                    "<meta name=\"dd-app\" content=\"{}\">{}</head>",
                    launch_secret(),
                    domain_meta()
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
    match carried(app, path) {
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
            "/_dd/app/photos"
                | "/_dd/app/photos.js"
                | "/_dd/app/open"
                | "/_dd/app/vault"
                | "/_dd/app/vault.js"
                | "/_dd/app/allow"
                | "/_dd/app/allow.js"
                | "/_dd/app/installer"
                | "/_dd/app/installer.js"
                | "/_dd/app/installer.css"
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
    // Mail (mail.rs): the member's mail key, the Worker asked as this
    // device, mail signed for another member, and mail sent from here
    if path.starts_with("/_dd/app/mail/") {
        let post = req.method() == "POST";
        let r = match (path.as_str(), post) {
            ("/_dd/app/mail/key", false) => crate::mail::key(app, false).await,
            ("/_dd/app/mail/key", true) => crate::mail::key(app, true).await,
            ("/_dd/app/mail/worker", true) => crate::mail::ask(app, req.body()).await,
            ("/_dd/app/mail/vouch", true) => crate::mail::vouch(app, req.body()).await,
            ("/_dd/app/mail/send", true) => crate::mail::send(app, req.body()).await,
            _ => Err((404, "not in the app".into())),
        };
        return Ok(match r {
            Ok(v) => no_store(&v),
            Err((s, why)) => plain(
                StatusCode::from_u16(s).unwrap_or(StatusCode::BAD_GATEWAY),
                &why,
            ),
        });
    }
    // where the other programs on this computer find the calendars, and
    // their password: the app's window only (dav.rs)
    if path == "/_dd/app/dav" {
        return Ok(no_store(&crate::dav::details()));
    }
    if path == "/_dd/app/libraries" {
        return Ok(match libraries_value(app).await? {
            Some(v) => no_store(&v),
            None => plain(StatusCode::NOT_FOUND, "no library opens here"),
        });
    }
    if path == "/_dd/app/archive" {
        return Ok(match archive_value(app).await? {
            Some(v) => no_store(&v),
            None => plain(
                StatusCode::NOT_FOUND,
                "your disk images do not open on this device yet",
            ),
        });
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
    // a calendar shared, or who it is shared with changed: signed with the
    // main key here, like a device added
    if path == "/_dd/app/share" && req.method() == "POST" {
        return Ok(match share(app, req.body()).await {
            Ok(v) => json_response(&v),
            Err((st, why)) => plain(st, &why),
        });
    }
    if path == "/_dd/app/remove" && req.method() == "POST" {
        return remove(app, req.body()).await;
    }
    // the Leave tab's downloads, written to the Downloads folder here
    // (export.rs): the window's scheme cannot stream a download
    if path.starts_with("/_dd/app/export") {
        return Ok(export(app, &req, &path).await);
    }
    if path == "/_dd/app/leaving" && req.method() == "POST" {
        return Ok(match leave(app, req.body()).await {
            Ok(v) => json_response(&v),
            Err((st, why)) => plain(st, &why),
        });
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
    // Passwords in this window (local.rs): the page, its script, and what
    // it asks
    if path == "/_dd/app/vault" {
        return Ok(page_asset(app, "vault.html"));
    }
    if path == "/_dd/app/vault.js" {
        return Ok(asset(app, "vault.js"));
    }
    // the home controller's installer, a window of its own (installer.rs)
    if path == "/_dd/app/installer" {
        return Ok(page_asset(app, "installer.html"));
    }
    if let Some(f) = path
        .strip_prefix("/_dd/app/")
        .filter(|f| matches!(*f, "installer.js" | "installer.css"))
    {
        return Ok(asset(app, f));
    }
    if let Some(step) = path.strip_prefix("/_dd/app/installer/") {
        #[cfg(desktop)]
        return crate::installer::route(
            app,
            &format!("/{step}"),
            req.method().as_str(),
            req.body(),
        )
        .await;
        #[cfg(not(desktop))]
        {
            let _ = step;
            return Ok(plain(StatusCode::NOT_FOUND, "set one up from a computer"));
        }
    }
    // the household's home controllers, as this account keeps them
    // (controllers.rs), and Smart home in this window (smarthome.rs)
    if path == "/_dd/app/controllers" {
        return match crate::controllers::route(app).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(plain(StatusCode::BAD_GATEWAY, &e)),
        };
    }
    if path == "/_dd/app/smarthome/open" && req.method() == "POST" {
        return match crate::smarthome::open(app, req.body()).await {
            Ok(url) => Ok(json_response(&serde_json::json!({ "url": url }))),
            Err(e) => Ok(plain(StatusCode::BAD_GATEWAY, &e)),
        };
    }
    if path == "/_dd/app/smarthome/updates" && req.method() == "POST" {
        let name = req
            .uri()
            .query()
            .and_then(|q| q.strip_prefix("name="))
            .map(percent_decode)
            .unwrap_or_default();
        let hold = serde_json::from_slice::<serde_json::Value>(req.body())
            .ok()
            .and_then(|v| v["hold"].as_bool());
        let Some(hold) = hold else {
            return Ok(plain(StatusCode::BAD_REQUEST, "hold: true or false"));
        };
        return match crate::smarthome::set_updates(app, &name, hold).await {
            Ok(v) => Ok(json_response(&v)),
            Err(e) => Ok(plain(StatusCode::BAD_GATEWAY, &e)),
        };
    }
    if path == "/_dd/app/smarthome/status" {
        let name = req
            .uri()
            .query()
            .and_then(|q| q.strip_prefix("name="))
            .map(percent_decode)
            .unwrap_or_default();
        return Ok(json_response(&crate::smarthome::status(app, &name).await));
    }
    if path == "/_dd/app/vault/state" {
        let to = req
            .uri()
            .query()
            .and_then(|q| crate::handoff::query(q, "to"))
            .map(crate::handoff::dec);
        return crate::local::vault_state(app, to.as_deref());
    }
    // a browser asking to open Photos or Passwords (local.rs): the page
    // where the member lets it in by the code both show
    if path == "/_dd/app/allow" {
        return Ok(page_asset(app, "allow.html"));
    }
    if path == "/_dd/app/allow.js" {
        return Ok(asset(app, "allow.js"));
    }
    if path == "/_dd/app/pair/state" {
        return Ok(json_response(&crate::local::pair_state()));
    }
    if path == "/_dd/app/pair/answer" && req.method() == "POST" {
        let v: serde_json::Value = serde_json::from_slice(req.body()).unwrap_or_default();
        return Ok(
            match crate::local::pair_answer(v["service"].as_str().unwrap_or(""), v["code"].as_str())
            {
                Ok(()) => json_response(&serde_json::json!({})),
                Err(e) => plain(StatusCode::CONFLICT, &e),
            },
        );
    }
    if path == "/_dd/app/photos/state" {
        return crate::photos::state(app).await;
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
    if matches!(path.as_str(), "/_dd/login" | "/_dd/logout") {
        return Ok(plain(StatusCode::UNAUTHORIZED, SIGNED_OUT));
    }
    // the keys a page asks for in one go, and Photos for one that holds its
    // password (keys.rs)
    if path == "/_dd/app/keys" && req.method() == "POST" {
        return Ok(match crate::keys::keys(app, req.body()).await {
            Ok(v) => no_store(&v),
            Err((s, why)) => plain(
                StatusCode::from_u16(s).unwrap_or(StatusCode::BAD_GATEWAY),
                &why,
            ),
        });
    }
    if path.starts_with("/_dd/ente/") || path.starts_with("/_dd/ente-store/") {
        let (parts, body) = req.into_parts();
        return Ok(
            match crate::keys::ente_send(
                parts.method.as_str(),
                parts.uri.path(),
                parts.uri.query(),
                &parts.headers,
                body,
            )
            .await
            {
                Ok(a) => {
                    let mut back = Response::builder()
                        .status(a.status)
                        .header("x-content-type-options", "nosniff")
                        .header("content-security-policy", "sandbox; default-src 'none'")
                        .header("cache-control", "no-store")
                        .header(
                            "content-type",
                            a.kind.as_deref().unwrap_or("application/octet-stream"),
                        );
                    if let Some(l) = &a.location {
                        back = back.header("location", l);
                    }
                    back.body(a.body).unwrap_or_default()
                }
                Err((s, why)) => plain(
                    StatusCode::from_u16(s).unwrap_or(StatusCode::BAD_GATEWAY),
                    &why,
                ),
            },
        );
    }
    if path.starts_with("/_dd/") {
        return proxy(app, req).await;
    }
    Ok(plain(StatusCode::NOT_FOUND, "not in the app"))
}

async fn export<R: Runtime>(
    app: &AppHandle<R>,
    req: &Request<Vec<u8>>,
    path: &str,
) -> Response<Vec<u8>> {
    use crate::export::{self as x, Owner};
    let id = req
        .uri()
        .query()
        .and_then(|q| crate::handoff::query(q, "id"))
        .map(crate::handoff::dec)
        .unwrap_or_default();
    let refused = |(code, why): x::Refused| {
        plain(
            StatusCode::from_u16(code).unwrap_or(StatusCode::FORBIDDEN),
            &why,
        )
    };
    let done = |r: Result<serde_json::Value, x::Refused>| match r {
        Ok(v) => no_store(&v),
        Err(e) => refused(e),
    };
    let me = Owner::Window;
    match (req.method().as_str(), path) {
        ("POST", "/_dd/app/export") => {
            let v: serde_json::Value = serde_json::from_slice(req.body()).unwrap_or_default();
            match v["kind"].as_str().and_then(x::Kind::of) {
                Some(kind) => done(x::start(app, me, kind).await),
                None => plain(StatusCode::BAD_REQUEST, "photos or files"),
            }
        }
        ("GET", "/_dd/app/export/state") => done(x::state(&me, &id)),
        ("POST", "/_dd/app/export/chunk") if req.body().len() <= x::MOST => done(
            x::chunk(&me, &id, req.body().clone())
                .await
                .map(|()| serde_json::json!({})),
        ),
        ("POST", "/_dd/app/export/end") => done(x::end(&me, &id).await),
        ("POST", "/_dd/app/export/cancel") => {
            done(x::cancel(&me, &id).await.map(|()| serde_json::json!({})))
        }
        _ => plain(StatusCode::NOT_FOUND, "no such download"),
    }
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
    crate::local::network_ready().await;
    let target = match req.uri().query() {
        Some(q) => format!("{base}{}?{q}", req.uri().path()),
        None => format!("{base}{}", req.uri().path()),
    };
    let method =
        reqwest::Method::from_bytes(req.method().as_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut out = crate::local::http()?
        .request(method.clone(), &target)
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
        // a save made from a version (a calendar event, a contact)
        "if-match",
        "if-none-match",
    ] {
        if let Some(v) = req.headers().get(h).and_then(|v| v.to_str().ok()) {
            out = out.header(h, v);
        }
    }
    let body = req.into_body();
    if !body.is_empty() {
        out = out.body(body);
    }
    let r = crate::local::send(out)
        .await
        .map_err(|e| crate::local::why(&e))?;
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
    if !data_from_box(method.as_str(), r.status().as_u16(), &kind) {
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
        // who the box says sent another member's locked mail (mail.rs)
        "x-dd-from",
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

/// Who may open one of the member's libraries, `{ id?, readers: [{ name,
/// writes }] }`, signed into their entry with the main key on this device;
/// with no id, a new library for it first (a shared calendar). Answers
/// `{ id }`. In the app's window only, as a device added is.
async fn share<R: Runtime>(
    app: &AppHandle<R>,
    body: &[u8],
) -> Result<serde_json::Value, (StatusCode, String)> {
    let bad = |e: String| (StatusCode::BAD_GATEWAY, e);
    let refuse = |e: &str| (StatusCode::BAD_REQUEST, e.to_string());
    let keys = app.state::<crate::account::Keys>();
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| refuse(&e.to_string()))?;
    let mut readers = Vec::new();
    for r in v["readers"]
        .as_array()
        .ok_or_else(|| refuse("no readers"))?
    {
        let name = r["name"].as_str().unwrap_or("").trim().to_lowercase();
        if name.is_empty() {
            return Err(refuse("a member's name is needed"));
        }
        let writes = r["writes"].as_bool().unwrap_or(false);
        // a name given twice is one member
        readers.retain(|(n, _): &(String, bool)| *n != name);
        readers.push((name, writes));
    }
    let Some(root) = account::load_root(&keys.0).map_err(|e| bad(e.to_string()))? else {
        return Err((
            StatusCode::FORBIDDEN,
            "your account's main key is not on this device: share from the one that holds it"
                .into(),
        ));
    };
    let name = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| bad(e.to_string()))?
        .ok_or_else(|| bad("no name on this device".into()))?;
    let dirs = crate::account::dirs();
    let list = account::released(&keys.0, &dirs)
        .await
        .map_err(|e| bad(format!("{e:#}")))?;
    let id = match v["id"].as_str() {
        None => {
            account::new_shared_library(&dirs, &name, &root, &readers, &list)
                .await
                .map_err(|e| bad(format!("{e:#}")))?
                .0
        }
        Some(id) => {
            let (opener, user, _) =
                media::gate::Opener::load(&keys.0).map_err(|e| bad(e.to_string()))?;
            let libs = media::gate::openable(&dirs, &user, &opener, None)
                .await
                .map_err(|e| bad(format!("{e:#}")))?;
            let (_, _, key) = libs
                .iter()
                .find(|(o, l, _)| *o == user && l.id == id)
                .ok_or_else(|| refuse("not one of your libraries"))?;
            account::set_readers(&dirs, &name, &root, id, key, &readers, &list)
                .await
                .map_err(|e| bad(format!("{e:#}")))?;
            id.to_string()
        }
    };
    Ok(serde_json::json!({ "id": id }))
}

/// Leaving, or coming back, signed with the main key on this device: what
/// Settings' Leave tab asks, in this window or a browser let in from it.
/// An account whose root is a passkey signs with that, on the gate's page.
pub(crate) async fn leave<R: Runtime>(
    app: &AppHandle<R>,
    body: &[u8],
) -> Result<serde_json::Value, (StatusCode, String)> {
    let keys = app.state::<crate::account::Keys>();
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    if v["back"].as_bool() == Some(true) {
        return Err((
            StatusCode::CONFLICT,
            "leaving deleted everything at once: there is no coming back".into(),
        ));
    }
    let bad = |e: String| (StatusCode::BAD_GATEWAY, e);
    let Some(root) = account::load_root(&keys.0).map_err(|e| bad(e.to_string()))? else {
        return Err((
            StatusCode::FORBIDDEN,
            "your account's main key is not on this device: leave from the one that holds it"
                .into(),
        ));
    };
    let name = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| bad(e.to_string()))?
        .ok_or_else(|| bad("no name on this device".into()))?;
    account::leave(&crate::account::dirs(), &name, &root)
        .await
        .map_err(|e| bad(format!("{e:#}")))?;
    Ok(serde_json::json!({ "back": false }))
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
    Ok(match library_value(app).await? {
        Some(v) => no_store(&v),
        None => plain(StatusCode::NOT_FOUND, "no library opens here"),
    })
}

fn no_store(v: &serde_json::Value) -> Response<Vec<u8>> {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(v.to_string().into_bytes())
        .unwrap_or_default()
}

/// Every library a page may open here, the member's own first, then those
/// other members of the release share with them: `[{ id, key, owner,
/// reader, writer }]`. Calendar and Contacts read them all, since a
/// calendar two people keep is one member's library the other may write.
/// For the app's own pages only, as library_value.
pub(crate) async fn libraries_value<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<serde_json::Value>, String> {
    use base64::Engine as _;
    crate::local::network_ready().await;
    let keys = app.state::<crate::account::Keys>();
    let (mut opener, user, _) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let dirs = crate::account::dirs();
    if opener.anchor().is_none() {
        account::own_entry(&keys.0, &dirs, &user)
            .await
            .map_err(|e| format!("{e:#}"))?;
        (opener, _, _) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    }
    // another member's library counts only when the release says they
    // belong: without the list, the member's own alone
    let list = account::released(&keys.0, &dirs).await.ok();
    let libs = media::gate::openable(&dirs, &user, &opener, list.as_ref())
        .await
        .map_err(|e| format!("{e:#}"))?;
    if libs.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::Value::Array(
        libs.iter()
            .map(|(owner, lib, key)| {
                let mine = *owner == user;
                let writer = !mine && lib.readers.iter().any(|r| r.name == user && r.writes);
                serde_json::json!({
                    "id": lib.id,
                    "key": base64::engine::general_purpose::STANDARD.encode(&key[..]),
                    "owner": owner,
                    "reader": !mine && !writer,
                    "writer": writer,
                    // who the member shares their own with
                    "readers": if mine {
                        lib.readers
                            .iter()
                            .map(|r| serde_json::json!({ "name": r.name, "writes": r.writes }))
                            .collect::<Vec<_>>()
                    } else {
                        vec![]
                    },
                })
            })
            .collect(),
    )))
}

/// The library a page opens, `{ id, key, reader }`: this device's own keys
/// open the member's library out of the entry this device vouches for, as
/// the mount and the player do. The member's own alone: one shared with
/// them is another member's, never their Files. For the app's own pages
/// only: its window, and a browser it let in (local.rs).
pub(crate) async fn library_value<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<serde_json::Value>, String> {
    use base64::Engine as _;
    crate::local::network_ready().await;
    let keys = app.state::<crate::account::Keys>();
    let (mut opener, user, _) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let dirs = crate::account::dirs();
    if opener.anchor().is_none() {
        // a first look is held to the release (own_entry), which pins it
        account::own_entry(&keys.0, &dirs, &user)
            .await
            .map_err(|e| format!("{e:#}"))?;
        (opener, _, _) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    }
    let libs = media::gate::openable(&dirs, &user, &opener, None)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let Some((_, lib, key)) = libs.iter().find(|(o, _, _)| *o == user) else {
        return Ok(None);
    };
    Ok(Some(serde_json::json!({
        "id": lib.id,
        "key": base64::engine::general_purpose::STANDARD.encode(&key[..]),
        "reader": false,
    })))
}

/// The archive password for Backups, `{ password }`: `dd image` seals it
/// to this device's key beside the archives (dd-devices.json), and this
/// device opens it. For the app's own pages only, as the library is.
pub(crate) async fn archive_value<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<serde_json::Value>, String> {
    use base64::Engine as _;
    crate::local::network_ready().await;
    let keys = app.state::<crate::account::Keys>();
    let (opener, _, token) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let base = media::gate::files_base(&crate::account::dirs()).map_err(|e| e.to_string())?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .get(format!("{base}/_dd/images/dd-devices.json"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let v: serde_json::Value = r
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    // signed by a device of the member's own entry, under the root this
    // device vouches for: a box could seal a password of its own to it
    let user = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    let entry = account::own_entry(&keys.0, &crate::account::dirs(), &user)
        .await
        .map_err(|e| format!("{e:#}"))?;
    match account::open_for_device(
        &v,
        &entry.entry,
        "archive",
        opener.device(),
        opener.root.as_ref(),
    ) {
        Ok(k) => Ok(Some(serde_json::json!({
            "password": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&k[..]),
        }))),
        Err(e) => {
            eprintln!("archive: the copy beside the disk images is not taken: {e:#}");
            Ok(None)
        }
    }
}

/// What the app does with its keys at each launch, once it is signed in:
/// the libraries sealed to its devices, root and paper key alone (where the
/// root is here: only it signs the entry), and Photos' password shared with
/// the member's other devices. Best effort, said in the log; the next
/// launch tries again.
pub fn at_launch<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // the network comes up a moment after the window
        tokio::time::sleep(Duration::from_secs(5)).await;
        let keys = app.state::<crate::account::Keys>();
        let Ok(Some(user)) = keys.0.get(crate::account::USER) else {
            return;
        };
        let dirs = crate::account::dirs();
        // pins the root this device vouches for, the first time
        if let Err(e) = account::own_entry(&keys.0, &dirs, &user).await {
            eprintln!("keys: {e:#}");
            return;
        }
        if let Ok(Some(root)) = account::load_root(&keys.0) {
            match account::reseal(&dirs, &user, &root).await {
                Ok(Some(s)) => eprintln!(
                    "keys: libraries sealed to devices, root and paper key alone (version {})",
                    s.entry.version
                ),
                Ok(None) => {}
                Err(e) => eprintln!("keys: reseal: {e:#}"),
            }
        }
        if let Err(e) = crate::photos::settle(&app).await {
            eprintln!("keys: photos: {e}");
        }
    });
}

/// a query value as encodeURIComponent wrote it
pub(crate) fn percent_decode(s: &str) -> String {
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
        // a folder's listing, and only as the answer to a PROPFIND
        assert!(super::data_from_box(
            "PROPFIND",
            207,
            "application/xml; charset=utf-8"
        ));
        assert!(super::data_from_box("PROPFIND", 207, "text/xml"));
        assert!(!super::data_from_box("GET", 207, "application/xml"));
        assert!(!super::data_from_box("PROPFIND", 200, "application/xml"));
        assert!(!super::data_from_box(
            "PROPFIND",
            207,
            "application/xhtml+xml"
        ));
        assert!(!super::data_from_box("PROPFIND", 207, "image/svg+xml"));
        assert!(!super::data_from_box("PROPFIND", 207, "text/html"));
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
