//! The library gate: who may reach which library. The storage and WebDAV
//! behind it are box/library (crate library-gate); this decides, from the
//! directory and the release's member list, whether a request is the
//! library's owner, a reader it was shared with, or nobody.

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};

pub use library_gate::{Gate, HANDOFF_SECS, Role, from_env};

use crate::App;

/// who is asking, and may they touch this library
#[allow(clippy::result_large_err)]
pub(crate) fn allowed(
    app: &App,
    headers: &HeaderMap,
    lib: &str,
) -> std::result::Result<(String, Role), Response> {
    let refused = |code: StatusCode, why: &str| Err((code, why.to_string()).into_response());
    if !lib.chars().all(|c| c.is_ascii_hexdigit()) || lib.len() != 32 {
        return refused(StatusCode::BAD_REQUEST, "not a library id");
    }
    // a device's token or this box's own session cookie: `dd` and rclone
    // bring the first, the Files and Movies pages in a browser the second
    let Some(user) = app.identify(headers, "access") else {
        return refused(
            StatusCode::UNAUTHORIZED,
            "a device token or a signed-in browser is required",
        );
    };
    // the demo reads the one library the box keeps for it, and nothing
    // else: it owns no entry and no key of its own
    if user == crate::pages::DEMO_USER {
        return match &app.demo_library {
            Some((id, _)) if id == lib => Ok((user, Role::Reader)),
            _ => refused(StatusCode::FORBIDDEN, "not your library"),
        };
    }
    // the demo's library belongs to the box, not to a person: its id sits
    // in this box's configuration where anyone can read it, and it is in
    // nobody's entry, so no entry may claim it either
    if app.demo_library.as_ref().is_some_and(|(id, _)| id == lib) {
        return refused(StatusCode::FORBIDDEN, "not your library");
    }
    // a signed-up stranger is not a member. Everything below reads an
    // entry, and an entry is a document you write about yourself, so an id
    // in it proves nothing on its own; the release's member list is the
    // only word on who belongs here.
    if !app.member(&user) {
        return refused(StatusCode::FORBIDDEN, "not your library");
    }
    // the owner: the library is in their entry
    if let Ok(Some(e)) = app.directory.entry(&user)
        && e.entry.libraries.iter().any(|l| l.id == lib)
    {
        return Ok((user, Role::Owner));
    }
    // a reader: some owner's entry names them for it
    if let Ok(listed) = app.directory.list() {
        for l in listed {
            if let Ok(Some(e)) = app.directory.entry(&l.name)
                && e.entry
                    .libraries
                    .iter()
                    .any(|x| x.id == lib && x.readers.iter().any(|r| r.name == user))
            {
                return Ok((user, Role::Reader));
            }
        }
    }
    refused(StatusCode::FORBIDDEN, "not your library")
}

#[allow(clippy::result_large_err)]
pub(crate) fn gate(app: &App) -> std::result::Result<&Gate, Response> {
    app.library
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "no libraries on this box").into_response())
}

/// every method under /_dd/dav/{lib}/..., one handler
pub(crate) async fn handle(
    State(app): State<Arc<App>>,
    Path((lib, path)): Path<(String, String)>,
    req: Request,
) -> Response {
    serve(app, lib, path, req).await
}

pub(crate) async fn handle_root(
    State(app): State<Arc<App>>,
    Path(lib): Path<String>,
    req: Request,
) -> Response {
    serve(app, lib, String::new(), req).await
}

async fn serve(app: Arc<App>, lib: String, raw: String, req: Request) -> Response {
    let g = match gate(&app) {
        Ok(g) => g,
        Err(r) => return r,
    };
    if req.method() == Method::OPTIONS {
        return library_gate::dav::options();
    }
    let (user, role) = match allowed(&app, req.headers(), &lib) {
        Ok(r) => r,
        Err(r) => return r,
    };
    // An upload counts against its owner's allowance, across every service
    // (storage.rs). A file whose size is not said up front is let in while
    // there is any room, and counted when it lands.
    let upload = req.method() == Method::PUT && matches!(role, Role::Owner);
    let size = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if upload && !app.storage.room(&user, size.max(1)) {
        return (
            StatusCode::INSUFFICIENT_STORAGE,
            "this would go past your storage allowance: make room first (Settings, Storage shows what uses it)",
        )
            .into_response();
    }
    let r = library_gate::dav::serve(g, lib, raw, role, req).await;
    if upload && r.status().is_success() {
        app.storage.landed(&user, size);
    }
    r
}

/// Every method under /images/..., a member's disk images: restic's layout,
/// ciphertext made on the device (`dd image`), in the bucket under
/// `images-<name>` beside the libraries, so the pool keeps the copies (two,
/// on two boxes) and no box disk holds a third. Only the member's own; a
/// browser changes nothing here without a fresh passkey (Backups, in
/// Settings), as the box always held it to.
pub(crate) async fn images_path(
    State(app): State<Arc<App>>,
    Path(raw): Path<String>,
    req: Request,
) -> Response {
    images(app, raw, req).await
}

pub(crate) async fn images_root(State(app): State<Arc<App>>, req: Request) -> Response {
    images(app, String::new(), req).await
}

async fn images(app: Arc<App>, raw: String, req: Request) -> Response {
    let g = match gate(&app) {
        Ok(g) => g,
        Err(r) => return r,
    };
    if req.method() == Method::OPTIONS {
        return library_gate::dav::options();
    }
    let headers = req.headers();
    let Some(user) = app.identify(headers, "access") else {
        return (
            StatusCode::UNAUTHORIZED,
            "a device token or a signed-in browser is required",
        )
            .into_response();
    };
    if app.role(&user) != Some(crate::pages::Role::Member) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // a name that is a folder name and nothing more
    if user.is_empty()
        || !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    // a read is a read method and nothing more: the upload header turns any
    // method into a write (box/library, in_pieces)
    let reads = matches!(
        req.method().as_str(),
        "GET" | "HEAD" | "OPTIONS" | "PROPFIND"
    ) && !headers.contains_key("x-dd-upload");
    if !reads && crate::bearer(headers).is_none() {
        let session = app
            .sessions
            .session(headers.get("cookie").and_then(|v| v.to_str().ok()))
            .map(|s| s.1)
            .unwrap_or_default();
        if !app.passkey_fresh(&session) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    // the images count against their owner's allowance like a library's
    // files (storage.rs counts the prefix too): checked against what the
    // request says, and the ledger's recount of the bucket settles what landed
    let upload =
        matches!(req.method().as_str(), "PUT" | "COPY") || headers.contains_key("x-dd-upload");
    let size = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if upload && !app.storage.room(&user, size.max(1)) {
        return (
            StatusCode::INSUFFICIENT_STORAGE,
            "this would go past your storage allowance: make room first (Settings, Storage shows what uses it)",
        )
            .into_response();
    }
    let r = library_gate::dav::serve(g, images_prefix(&user), raw, Role::Owner, req).await;
    if upload && r.status().is_success() {
        app.storage.landed(&user, size);
    }
    r
}

/// where a member's disk images are in the bucket: a top-level prefix, as a
/// library's is, so the trash is let go of the same way; never 32 hex
/// characters, so never a library's
pub(crate) fn images_prefix(user: &str) -> String {
    format!("images-{user}")
}
