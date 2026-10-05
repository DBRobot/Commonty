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
    if !matches!(role, Role::Owner) {
        return library_gate::dav::serve(g, lib, raw, role, req).await;
    }
    metered(&app, g, &user, lib, raw, role, req).await
}

/// Past your allowance, as a person reads it.
fn no_room() -> Response {
    (
        StatusCode::INSUFFICIENT_STORAGE,
        "this would go past your storage allowance: make room first (Settings, Storage shows what uses it)",
    )
        .into_response()
}

/// A write, held to its owner's allowance across every service (storage.rs),
/// by what it carries rather than what it says. Room is set aside first: the
/// size it declares, or all that is left when it declares none. The body is
/// counted as it streams and cut off past that room, so the store never
/// takes the object; what did land is counted until the ledger next looks.
/// A COPY adds what it copies, measured in the bucket before it is made.
async fn metered(
    app: &App,
    g: &Gate,
    user: &str,
    lib: String,
    raw: String,
    role: Role,
    req: Request,
) -> Response {
    use futures_util::StreamExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    let method = req.method().as_str().to_string();
    let piece = req.headers().contains_key("x-dd-upload");
    // the upload header makes dav::serve take the body as a piece whatever
    // the method, so it is metered as one, never as the copy it may claim
    if method == "COPY" && !piece {
        let size = match library_gate::dav::bytes_at(g, &lib, &raw).await {
            Ok(Some(n)) => n,
            Ok(None) => return (StatusCode::BAD_REQUEST, "not a path").into_response(),
            Err(e) => return (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        };
        let Some(held) = app.storage.hold(user, Some(size)) else {
            return no_room();
        };
        let r = library_gate::dav::serve(g, lib, raw, role, req).await;
        let landed = if r.status().is_success() { size } else { 0 };
        app.storage.done(held, landed);
        return r;
    }
    if method != "PUT" && !piece {
        return library_gate::dav::serve(g, lib, raw, role, req).await;
    }
    let declared = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let Some(held) = app.storage.hold(user, declared) else {
        return no_room();
    };
    let limit = held.limit;
    let seen = Arc::new(AtomicU64::new(0));
    let counter = seen.clone();
    let (parts, body) = req.into_parts();
    let body = body.into_data_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        let n = counter.fetch_add(chunk.len() as u64, Ordering::Relaxed) + chunk.len() as u64;
        if limit.is_some_and(|l| n > l) {
            return Err(std::io::Error::other("past the storage allowance"));
        }
        Ok(chunk)
    });
    let req = Request::from_parts(parts, axum::body::Body::from_stream(body));
    let r = library_gate::dav::serve(g, lib, raw, role, req).await;
    let carried = seen.load(Ordering::Relaxed);
    let over = limit.is_some_and(|l| carried > l);
    app.storage.done(
        held,
        if r.status().is_success() && !over {
            carried
        } else {
            0
        },
    );
    if over { no_room() } else { r }
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
    // files (storage.rs counts the prefix too)
    metered(&app, g, &user, images_prefix(&user), raw, Role::Owner, req).await
}

/// where a member's disk images are in the bucket: a top-level prefix, as a
/// library's is, so the trash is let go of the same way; never 32 hex
/// characters, so never a library's
pub(crate) fn images_prefix(user: &str) -> String {
    format!("images-{user}")
}
