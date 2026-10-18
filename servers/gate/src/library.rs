//! The library gate: who may reach which library. The storage and WebDAV
//! behind it are servers/library (crate library-gate); this decides, from the
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
        // suspended: their own libraries, to read and take, nothing more
        if app.suspension(&user).is_some()
            && let Ok(Some(e)) = app.directory.entry(&user)
            && e.entry.libraries.iter().any(|l| l.id == lib)
        {
            return Ok((user, Role::Reader));
        }
        return refused(StatusCode::FORBIDDEN, "not your library");
    }
    // the owner: the library is in their entry
    if let Ok(Some(e)) = app.directory.entry(&user)
        && e.entry.libraries.iter().any(|l| l.id == lib)
    {
        return Ok((user, Role::Owner));
    }
    // a reader: some owner's entry names them for it
    let names = |owner: &str| {
        app.directory.entry(owner).ok().flatten().is_some_and(|e| {
            e.entry
                .libraries
                .iter()
                .any(|x| x.id == lib && x.readers.iter().any(|r| r.name == user))
        })
    };
    let known = app.library_owners.lock().unwrap().get(lib).cloned();
    let everyone = || {
        app.directory
            .list()
            .map(|l| l.into_iter().map(|l| l.name).collect())
            .unwrap_or_default()
    };
    if let Some(owner) = owner_naming(known, names, everyone) {
        app.library_owners
            .lock()
            .unwrap()
            .insert(lib.to_string(), owner);
        return Ok((user, Role::Reader));
    }
    refused(StatusCode::FORBIDDEN, "not your library")
}

/// Whose entry names the reader: the owner it was last time first, as
/// their entry says now, and every entry only when that misses. Each
/// WebDAV request of a reader asks, so a film is not a read of the whole
/// directory per segment; what was last time is a guess, never an answer.
fn owner_naming(
    known: Option<String>,
    names: impl Fn(&str) -> bool,
    everyone: impl FnOnce() -> Vec<String>,
) -> Option<String> {
    if let Some(o) = known
        && names(&o)
    {
        return Some(o);
    }
    everyone().into_iter().find(|o| names(o))
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
    let piece = library_gate::dav::piece(req.headers()).is_some();
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
    // deleted for good from the trash: room again at once, for what the
    // delete itself took out, so two of one thing count it once
    if library_gate::dav::deletes_for_good(req.method(), &raw) && role == Role::Owner {
        return match library_gate::dav::delete_for_good(g, &lib, &raw).await {
            Ok((r, freed)) => {
                app.storage.freed(user, freed);
                r
            }
            Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        };
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
    // a member's join ticket, from the drive a box was set up with: its own
    // pending backup (pending/<its id>/) and nothing else, never a delete
    if let Some((user, ticket)) = crate::member_boxes::pending_ticket(&app, headers) {
        if app.role(&user) != Some(crate::pages::Role::Member)
            || !library_gate::dav::ticket_allows(
                req.method(),
                headers,
                &raw,
                &format!("pending/{ticket}"),
            )
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        return metered(&app, g, &user, images_prefix(&user), raw, Role::Owner, req).await;
    }
    let Some((user, by_device)) = app.identify_how(headers, "access") else {
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
    // method into a write (servers/library, in_pieces)
    let reads = matches!(
        req.method().as_str(),
        "GET" | "HEAD" | "OPTIONS" | "PROPFIND"
    ) && library_gate::dav::piece(headers).is_none();
    // only a verified device token skips the passkey: any other bearer is
    // no token, and the cookie beside it is a browser's
    if !reads && !by_device {
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

/// What Movies & TV keeps about a library that is not a file in it: where
/// each viewer stopped, and the shelf (years, a line about each title, its
/// art's names). Sealed in the browser with the library's key; the gate
/// keeps the bytes, replaced outright, and cannot read them. As files, a
/// place saved every half minute filled the library's trash.
pub(crate) mod notes {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use axum::body::Bytes;
    use axum::extract::{Path as UrlPath, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};

    use super::{Role, allowed};
    use crate::App;

    #[derive(PartialEq, Eq, Clone, Copy, Debug)]
    pub(crate) enum Kind {
        /// each viewer's own, owner or reader
        Progress,
        /// the library's: its owner writes it, its readers read it too
        Shelf,
    }

    impl Kind {
        pub(crate) fn parse(s: &str) -> Option<Kind> {
            match s {
                "progress" => Some(Kind::Progress),
                "shelf" => Some(Kind::Shelf),
                _ => None,
            }
        }

        // a place is ~200 bytes, so thousands fit; a shelf holds a line and
        // every episode's name per show, and the public door takes 1m a body
        fn cap(self) -> usize {
            match self {
                Kind::Progress => 512 * 1024,
                Kind::Shelf => 1024 * 1024,
            }
        }
    }

    fn valid_lib(lib: &str) -> bool {
        lib.len() == 32 && lib.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// notes/<lib>/shelf.bin and notes/<lib>/progress/<user>.bin
    pub(crate) fn file(dir: &Path, lib: &str, kind: Kind, user: &str) -> Option<PathBuf> {
        if !valid_lib(lib) {
            return None;
        }
        let at = dir.join("notes").join(lib.to_ascii_lowercase());
        match kind {
            Kind::Shelf => Some(at.join("shelf.bin")),
            Kind::Progress => {
                crate::valid_user(user).then(|| at.join("progress").join(format!("{user}.bin")))
            }
        }
    }

    pub(crate) fn read(
        dir: &Path,
        lib: &str,
        kind: Kind,
        user: &str,
    ) -> Result<Vec<u8>, StatusCode> {
        let p = file(dir, lib, kind, user).ok_or(StatusCode::BAD_REQUEST)?;
        std::fs::read(p).map_err(|_| StatusCode::NOT_FOUND)
    }

    pub(crate) fn write(
        dir: &Path,
        lib: &str,
        kind: Kind,
        user: &str,
        role: Role,
        body: &[u8],
    ) -> StatusCode {
        if kind == Kind::Shelf && role != Role::Owner {
            return StatusCode::FORBIDDEN;
        }
        if body.len() > kind.cap() {
            return StatusCode::PAYLOAD_TOO_LARGE;
        }
        let Some(p) = file(dir, lib, kind, user) else {
            return StatusCode::BAD_REQUEST;
        };
        // two tabs saving at once each write their own; the last rename wins whole
        let ok = p
            .parent()
            .is_some_and(|d| std::fs::create_dir_all(d).is_ok())
            && crate::private::write(&p, body).is_ok();
        if ok {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }

    /// a member who owns or reads the library; the demo keeps nothing here
    #[allow(clippy::result_large_err)]
    fn who(
        app: &App,
        headers: &HeaderMap,
        lib: &str,
        kind: &str,
    ) -> Result<(String, Role, Kind), Response> {
        let Some(kind) = Kind::parse(kind) else {
            return Err((StatusCode::BAD_REQUEST, "no such note").into_response());
        };
        let (user, role) = allowed(app, headers, lib)?;
        if user == crate::pages::DEMO_USER {
            return Err((StatusCode::FORBIDDEN, "the demo keeps nothing").into_response());
        }
        Ok((user, role, kind))
    }

    pub(crate) async fn get(
        State(app): State<Arc<App>>,
        UrlPath((lib, kind)): UrlPath<(String, String)>,
        headers: HeaderMap,
    ) -> Response {
        let (user, _, kind) = match who(&app, &headers, &lib, &kind) {
            Ok(w) => w,
            Err(r) => return r,
        };
        match read(&app.state_dir, &lib, kind, &user) {
            Ok(b) => ([("cache-control", "no-store")], b).into_response(),
            Err(c) => c.into_response(),
        }
    }

    pub(crate) async fn put(
        State(app): State<Arc<App>>,
        UrlPath((lib, kind)): UrlPath<(String, String)>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let (user, role, kind) = match who(&app, &headers, &lib, &kind) {
            Ok(w) => w,
            Err(r) => return r,
        };
        write(&app.state_dir, &lib, kind, &user, role, &body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::owner_naming;
    use std::cell::Cell;

    #[test]
    fn a_reader_is_found_by_their_owner_without_reading_everyone() {
        let shared_by = Cell::new("david");
        let names = |o: &str| o == shared_by.get();
        let scans = Cell::new(0);
        let everyone = || {
            scans.set(scans.get() + 1);
            vec!["sarah".to_string(), "david".to_string(), "tom".to_string()]
        };
        // the first time reads everyone, after that the owner alone
        assert_eq!(
            owner_naming(None, names, everyone).as_deref(),
            Some("david")
        );
        assert_eq!(
            owner_naming(Some("david".into()), names, everyone).as_deref(),
            Some("david")
        );
        assert_eq!(scans.get(), 1);
        // the owner took it back: the guess is checked, and nobody names them
        shared_by.set("nobody");
        assert_eq!(owner_naming(Some("david".into()), names, everyone), None);
        assert_eq!(scans.get(), 2);
    }

    mod notes {
        use super::super::Role;
        use super::super::notes::{Kind, file, read, write};
        use axum::http::StatusCode;

        const LIB: &str = "0123456789abcdef0123456789abcdef";

        fn dir(what: &str) -> std::path::PathBuf {
            let d = std::env::temp_dir().join(format!("dd-notes-{what}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            d
        }

        #[test]
        fn each_viewer_reads_back_their_own_place() {
            let d = dir("progress");
            let r = |u| read(&d, LIB, Kind::Progress, u);
            assert_eq!(r("david"), Err(StatusCode::NOT_FOUND));
            let w = |u, role, b: &[u8]| write(&d, LIB, Kind::Progress, u, role, b);
            assert_eq!(w("david", Role::Owner, b"d"), StatusCode::NO_CONTENT);
            assert_eq!(w("sarah", Role::Reader, b"s"), StatusCode::NO_CONTENT);
            assert_eq!(r("david").unwrap(), b"d");
            assert_eq!(r("sarah").unwrap(), b"s");
            // anyone else asking gets their own, which is nothing
            assert_eq!(r("tom"), Err(StatusCode::NOT_FOUND));
            // replaced outright, nothing left beside it
            assert_eq!(w("david", Role::Owner, b"again"), StatusCode::NO_CONTENT);
            assert_eq!(r("david").unwrap(), b"again");
            let kept = d.join("notes").join(LIB).join("progress");
            assert_eq!(std::fs::read_dir(kept).unwrap().count(), 2);
            let _ = std::fs::remove_dir_all(&d);
        }

        #[test]
        fn the_shelf_is_the_owners_and_readers_read_it() {
            let d = dir("shelf");
            let w = |u, role| write(&d, LIB, Kind::Shelf, u, role, b"shelf");
            assert_eq!(w("sarah", Role::Reader), StatusCode::FORBIDDEN);
            assert_eq!(
                read(&d, LIB, Kind::Shelf, "sarah"),
                Err(StatusCode::NOT_FOUND)
            );
            assert_eq!(w("david", Role::Owner), StatusCode::NO_CONTENT);
            assert_eq!(read(&d, LIB, Kind::Shelf, "sarah").unwrap(), b"shelf");
            let _ = std::fs::remove_dir_all(&d);
        }

        #[test]
        fn a_bad_library_name_or_kind_goes_nowhere() {
            let d = dir("bad");
            for lib in [
                "",
                "../../../../../../etc/passwd",
                "0123456789abcdef0123456789abcdeg",
                "0123456789abcdef/123456789abcdef",
                "0123456789abcdef0123456789abcdef0",
            ] {
                assert!(file(&d, lib, Kind::Shelf, "david").is_none(), "{lib}");
                let w = write(&d, lib, Kind::Progress, "david", Role::Owner, b"x");
                assert_eq!(w, StatusCode::BAD_REQUEST, "{lib}");
            }
            for user in ["", "../x", ".hidden", "a/b", "Big"] {
                assert!(file(&d, LIB, Kind::Progress, user).is_none(), "{user}");
            }
            for kind in ["", "Progress", "../shelf", "shelf.bin", "names"] {
                assert!(Kind::parse(kind).is_none(), "{kind}");
            }
            assert!(!d.exists());
        }

        #[test]
        fn too_big_is_refused() {
            let d = dir("big");
            let w = |kind, n| write(&d, LIB, kind, "david", Role::Owner, &vec![0u8; n]);
            let big = 512 * 1024 + 1;
            assert_eq!(w(Kind::Progress, big), StatusCode::PAYLOAD_TOO_LARGE);
            assert_eq!(w(Kind::Shelf, big), StatusCode::NO_CONTENT);
            let bigger = 1024 * 1024 + 1;
            assert_eq!(w(Kind::Shelf, bigger), StatusCode::PAYLOAD_TOO_LARGE);
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}
