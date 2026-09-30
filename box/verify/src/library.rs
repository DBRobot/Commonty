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
            "this would go past your storage allowance: make room first (the Storage page shows what uses it)",
        )
            .into_response();
    }
    let r = library_gate::dav::serve(g, lib, raw, role, req).await;
    if upload && r.status().is_success() {
        app.storage.landed(&user, size);
    }
    r
}
