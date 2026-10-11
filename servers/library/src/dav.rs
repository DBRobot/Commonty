//! A library as WebDAV: what rclone, a file manager and the app all speak.
//! One prefix in the bucket per library, the device token as bearer, an
//! owner may write and a reader may only read, and nothing is deleted at
//! once: DELETE, and a PUT or MOVE over a name already taken, put what was
//! there into `trash/` under the library, and the trash lets go of it after
//! 30 days. The bytes and names through here are rclone's crypt format,
//! made and read on the device; this box moves ciphertext.
//!
//!   OPTIONS, PROPFIND (depth 0 or 1), HEAD, GET (ranges), PUT, MKCOL,
//!   MOVE, COPY, DELETE, under /_dd/dav/{lib}/...

use anyhow::{Context, Result, anyhow};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::{Gate, Role, uri_encode};

/// where a deleted thing goes, under the library
const TRASH: &str = "trash";
static TRASH_PLACE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// a library's id, or `images-<name>`: the bucket's top level, and nothing
/// that could reach past one person's folder
fn top_prefix(id: &str) -> bool {
    let images = id.strip_prefix("images-").is_some_and(|n| {
        !n.is_empty()
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    });
    identity::valid_library_id(id) || images
}

/// how long it stays there before it is gone for good
pub const TRASH_SECS: u64 = 30 * 24 * 3600;

/// one thing under a prefix, as the bucket lists it
pub struct Entry {
    pub key: String,
    pub size: u64,
    pub modified: String,
}

impl Gate {
    /// the objects directly under `prefix` and the prefixes below it, one
    /// level, all pages
    pub async fn list_level(&self, prefix: &str) -> Result<(Vec<Entry>, Vec<String>)> {
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut q = vec![
                ("delimiter", "/".to_string()),
                ("list-type", "2".to_string()),
                ("prefix", prefix.to_string()),
            ];
            if let Some(t) = &token {
                q.push(("continuation-token", t.clone()));
            }
            q.sort();
            let query = q
                .iter()
                .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
                .collect::<Vec<_>>()
                .join("&");
            let xml = self.call(reqwest::Method::GET, "/", &query, &[]).await?;
            for c in xml.split("<Contents>").skip(1) {
                let key = between(c, "<Key>", "</Key>").unwrap_or_default();
                let size = between(c, "<Size>", "</Size>")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let modified = between(c, "<LastModified>", "</LastModified>").unwrap_or_default();
                files.push(Entry {
                    key: unxml(&key),
                    size,
                    modified,
                });
            }
            for p in xml.split("<CommonPrefixes>").skip(1) {
                if let Some(pre) = between(p, "<Prefix>", "</Prefix>") {
                    dirs.push(unxml(&pre));
                }
            }
            if xml.contains("<IsTruncated>true</IsTruncated>") {
                token = between(&xml, "<NextContinuationToken>", "</NextContinuationToken>");
                if token.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok((files, dirs))
    }

    /// Everything trashed more than TRASH_SECS before `now`, deleted: the
    /// one delete that is not a move. Each library's trash is a folder per
    /// second things went into it, so the name says how old it is.
    pub async fn purge_trash(&self, now: u64) -> Result<usize> {
        let (_, libs) = self.list_level("").await?;
        let mut n = 0;
        for lib in libs {
            let id = lib.trim_end_matches('/');
            // a library, or a member's disk images (images-<name>, the gate's
            // /_dd/images): both keep a trash, and both let it go
            if !top_prefix(id) {
                continue;
            }
            let (_, stamps) = self.list_level(&format!("{id}/{TRASH}/")).await?;
            for s in stamps {
                let Some(stamp) = s
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .and_then(|t| t.parse::<u64>().ok())
                else {
                    continue;
                };
                if stamp + TRASH_SECS > now {
                    continue;
                }
                for e in self.list_all(&s).await? {
                    self.delete(&e.key).await?;
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    /// A library, or a member's disk images, gone for good, trash and all:
    /// someone who left, or was removed 30 days ago (the gate's leaving.rs).
    /// Safe to run again.
    pub async fn forget(&self, id: &str) -> Result<usize> {
        anyhow::ensure!(top_prefix(id), "not a library or a member's images");
        let mut n = 0;
        for e in self.list_all(&format!("{id}/")).await? {
            self.delete(&e.key).await?;
            n += 1;
        }
        Ok(n)
    }

    /// every object under a prefix, all levels
    pub async fn list_all(&self, prefix: &str) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut q = vec![
                ("list-type", "2".to_string()),
                ("prefix", prefix.to_string()),
            ];
            if let Some(t) = &token {
                q.push(("continuation-token", t.clone()));
            }
            q.sort();
            let query = q
                .iter()
                .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
                .collect::<Vec<_>>()
                .join("&");
            let xml = self.call(reqwest::Method::GET, "/", &query, &[]).await?;
            for c in xml.split("<Contents>").skip(1) {
                out.push(Entry {
                    key: unxml(&between(c, "<Key>", "</Key>").unwrap_or_default()),
                    size: between(c, "<Size>", "</Size>")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                    modified: between(c, "<LastModified>", "</LastModified>").unwrap_or_default(),
                });
            }
            if xml.contains("<IsTruncated>true</IsTruncated>") {
                token = between(&xml, "<NextContinuationToken>", "</NextContinuationToken>");
                if token.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(out)
    }

    /// one object's size and date, if it exists
    pub async fn stat(&self, object: &str) -> Result<Option<(u64, String)>> {
        let r = reqwest::Client::new()
            .head(self.presign("HEAD", object))
            .send()
            .await?;
        if r.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err(anyhow!("s3 head {}", r.status()));
        }
        let size = r
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let modified = r
            .headers()
            .get(header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        Ok(Some((size, modified)))
    }

    /// move: a copy and then the source gone
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.copy(from, to).await?;
        self.delete(from).await
    }

    /// the objects a path names: the file, and with `folder` everything
    /// under it as well
    async fn objects_at(&self, lib: &str, path: &str, folder: bool) -> Result<Vec<String>> {
        let at = key(lib, path);
        let mut out = Vec::new();
        if self.stat(&at).await?.is_some() {
            out.push(at.clone());
        }
        if folder {
            out.extend(
                self.list_all(&format!("{at}/"))
                    .await?
                    .into_iter()
                    .map(|e| e.key),
            );
        }
        Ok(out)
    }

    /// `objects` (under `path`) into trash/<stamp>/, the stamp a second on
    /// while one is taken, so nothing in the trash is ever overwritten. A
    /// copy leaves the original where it was, for a PUT about to replace it.
    async fn keep_in_trash(
        &self,
        lib: &str,
        path: &str,
        objects: &[String],
        copy: bool,
    ) -> Result<Vec<String>> {
        if objects.is_empty() {
            return Ok(Vec::new());
        }
        // one at a time picks a place and fills it: two writes of one name in
        // the same second would otherwise both find trash/<stamp>/ free
        let _held = TRASH_PLACE.lock().await;
        let mut stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut tries = 0;
        while !self
            .objects_at(lib, &format!("{TRASH}/{stamp}/{path}"), true)
            .await?
            .is_empty()
        {
            stamp += 1;
            tries += 1;
            if tries > 60 {
                return Err(anyhow!("no free place in the trash for {path}"));
            }
        }
        let mut made = Vec::new();
        for o in objects {
            let rel = o.strip_prefix(&format!("{lib}/")).unwrap_or(o);
            let to = format!("{lib}/{TRASH}/{stamp}/{rel}");
            if copy {
                self.copy(o, &to).await?;
            } else {
                self.rename(o, &to).await?;
            }
            made.push(to);
        }
        Ok(made)
    }

    /// what an object is now, to tell whether a write changed it: its etag,
    /// size and date as the store gives them, or None when there is none
    async fn version(&self, object: &str) -> Result<Option<String>> {
        let r = reqwest::Client::new()
            .head(self.presign("HEAD", object))
            .send()
            .await?;
        if r.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err(anyhow!("s3 head {}", r.status()));
        }
        let h = |n: header::HeaderName| {
            r.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        Ok(Some(format!(
            "{} {} {}",
            h(header::ETAG),
            h(header::CONTENT_LENGTH),
            h(header::LAST_MODIFIED)
        )))
    }

    /// A write that failed takes back the trash copy it made, so failed
    /// tries are no room the allowance never held - but only when the old
    /// file is plainly still in place. A write the store took whose answer
    /// was lost has replaced it, and then the trash copy is all that is
    /// left of it; when that cannot be told, the copy stays.
    async fn untrash(&self, object: &str, before: &Option<String>, made: &[String]) {
        if made.is_empty() || !matches!(self.version(object).await, Ok(now) if now == *before) {
            return;
        }
        for k in made {
            if let Err(e) = self.delete(k).await {
                eprintln!("dav: the trash copy of a failed write stays: {e:#}");
            }
        }
    }
}

/// the trash itself or anything in it: kept by DELETE and emptied by the
/// purge, never written by a client
fn in_trash(path: &str) -> bool {
    path == TRASH || path.starts_with("trash/")
}

/// a thing put in the trash (trash/<stamp>/...), which may move back out
fn restorable(path: &str) -> bool {
    let mut under = path.split('/');
    under.next() == Some(TRASH)
        && under
            .next()
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        && under.next().is_some()
}

/// why a MOVE or COPY from `path` to `to` is refused, if it is: out of the
/// trash only a restore (a move, never a copy), nothing into it, and never
/// onto itself or a folder of its own, since what is at the destination
/// goes to the trash first
fn refused(path: &str, to: &str, mv: bool) -> bool {
    let inside = |a: &str, b: &str| a.starts_with(&format!("{b}/"));
    path.is_empty()
        || to.is_empty()
        || in_trash(to)
        || (in_trash(path) && !(mv && restorable(path)))
        || path == to
        || inside(to, path)
        || inside(path, to)
}

fn between(s: &str, a: &str, b: &str) -> Option<String> {
    let start = s.find(a)? + a.len();
    let end = s[start..].find(b)? + start;
    Some(s[start..end].to_string())
}

fn unxml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// a path under the library as the client named it: decoded, no dots, no
/// empty segments, no way out
fn clean(path: &str) -> Option<String> {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Some(String::new());
    }
    let mut out = Vec::new();
    for seg in path.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains('\\') {
            return None;
        }
        out.push(seg);
    }
    Some(out.join("/"))
}

/// the library's prefix in the bucket plus a path
fn key(lib: &str, path: &str) -> String {
    if path.is_empty() {
        format!("{lib}/")
    } else {
        format!("{lib}/{path}")
    }
}

/// an S3 date (RFC 3339) as WebDAV wants it (RFC 1123); a HEAD already
/// gives RFC 1123
fn dav_date(s: &str) -> String {
    if s.contains(',') || s.is_empty() {
        return s.to_string();
    }
    // 2026-09-24T01:02:03.000Z -> Wed, 24 Sep 2026 01:02:03 GMT
    let (date, time) = s.split_once('T').unwrap_or((s, "00:00:00"));
    let mut d = date.split('-').filter_map(|p| p.parse::<u64>().ok());
    let (y, m, day) = (
        d.next().unwrap_or(1970),
        d.next().unwrap_or(1),
        d.next().unwrap_or(1),
    );
    let time = time.split(['.', 'Z']).next().unwrap_or("00:00:00");
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // day of week from the civil date (Zeller-ish, Sakamoto)
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let yy = if m < 3 { y - 1 } else { y };
    let dow = (yy + yy / 4 - yy / 100 + yy / 400 + t[(m as usize).clamp(1, 12) - 1] + day) % 7;
    let days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!(
        "{}, {:02} {} {:04} {} GMT",
        days[dow as usize],
        day,
        months[(m as usize).clamp(1, 12) - 1],
        y,
        time
    )
}

fn propfind_response(href: &str, name: &str, dir: bool, size: u64, modified: &str) -> String {
    let kind = if dir { "<D:collection/>" } else { "" };
    let len = if dir {
        String::new()
    } else {
        format!("<D:getcontentlength>{size}</D:getcontentlength>")
    };
    format!(
        "<D:response><D:href>{}</D:href><D:propstat><D:prop><D:displayname>{}</D:displayname><D:resourcetype>{kind}</D:resourcetype>{len}<D:getlastmodified>{}</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        xml(href),
        xml(name),
        xml(&dav_date(modified))
    )
}

fn multistatus(body: String) -> Response {
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">{body}</D:multistatus>"),
    )
        .into_response()
}

fn href(lib: &str, path: &str, dir: bool) -> String {
    let mut h = format!("/_dd/dav/{lib}/");
    if !path.is_empty() {
        h.push_str(
            &path
                .split('/')
                .map(|s| uri_encode(s, false))
                .collect::<Vec<_>>()
                .join("/"),
        );
        if dir {
            h.push('/');
        }
    }
    h
}

/// OPTIONS, which answers before anyone is asked who they are
pub fn options() -> Response {
    (
        StatusCode::OK,
        [
            ("DAV", "1"),
            (
                "Allow",
                "OPTIONS, PROPFIND, HEAD, GET, PUT, MKCOL, MOVE, COPY, DELETE",
            ),
        ],
    )
        .into_response()
}

/// A request the gate has let through, as `role` in library `lib`: every
/// method, one function, since axum has no PROPFIND of its own.
pub async fn serve(g: &Gate, lib: String, raw: String, role: Role, req: Request) -> Response {
    let method = req.method().clone();
    let headers = req.headers().clone();
    let Some(path) = clean(&raw) else {
        return (StatusCode::BAD_REQUEST, "not a path").into_response();
    };
    // the upload header makes any method a write (in_pieces takes the body
    // whatever the method), so it is one here too
    let writes = piece(&headers).is_some()
        || matches!(
            method.as_str(),
            "PUT" | "POST" | "MKCOL" | "MOVE" | "COPY" | "DELETE" | "PROPPATCH"
        );
    if writes && role != Role::Owner {
        return (StatusCode::FORBIDDEN, "a reader does not write").into_response();
    }
    if let Some(upload) = piece(&headers) {
        let out = in_pieces(g, &lib, &path, upload.to_string(), req).await;
        return match out {
            Ok(r) => r,
            Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        };
    }
    // a folder asked for with GET: its listing as json, the shape nginx's
    // autoindex gives, which the disk images' clients read (client/archive)
    if method == Method::GET && (path.is_empty() || raw.ends_with('/')) {
        return match listing(g, &lib, &path).await {
            Ok(r) => r,
            Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        };
    }
    let out = match method.as_str() {
        "PROPFIND" => propfind(g, &lib, &path, &headers).await,
        "HEAD" | "GET" => get(g, &lib, &path, &headers, method == Method::HEAD).await,
        "PUT" => put(g, &lib, &path, req).await,
        "MKCOL" => mkcol(g, &lib, &path).await,
        "DELETE" => delete(g, &lib, &path).await,
        "MOVE" | "COPY" => move_or_copy(g, &lib, &path, &headers, method.as_str() == "MOVE").await,
        "PROPPATCH" => Ok(multistatus(format!(
            "<D:response><D:href>{}</D:href><D:propstat><D:prop/><D:status>HTTP/1.1 403 Forbidden</D:status></D:propstat></D:response>",
            xml(&href(&lib, &path, false))
        ))),
        _ => Ok(StatusCode::METHOD_NOT_ALLOWED.into_response()),
    };
    match out {
        Ok(r) => r,
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

/// The upload header, as `serve` reads it: a request with one is a piece of
/// an upload whatever its method. Callers that meter or guard writes ask
/// this, so they and `serve` cannot read the header two ways.
pub fn piece(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-dd-upload").and_then(|v| v.to_str().ok())
}

/// What a COPY from `raw` would add to the bucket: the object's size, or
/// everything under the folder. None: not a path.
pub async fn bytes_at(g: &Gate, lib: &str, raw: &str) -> Result<Option<u64>> {
    let Some(path) = clean(raw) else {
        return Ok(None);
    };
    if !path.is_empty()
        && let Some((size, _)) = g.stat(&key(lib, &path)).await?
    {
        return Ok(Some(size));
    }
    let prefix = if path.is_empty() {
        key(lib, "")
    } else {
        format!("{}/", key(lib, &path))
    };
    Ok(Some(
        g.list_all(&prefix).await?.iter().map(|e| e.size).sum(),
    ))
}

/// one folder, one level: [{name, type, mtime, size}], as nginx's autoindex
/// json has it; nothing under a folder is no folder (404), and the trash is
/// not listed
async fn listing(g: &Gate, lib: &str, path: &str) -> Result<Response> {
    let prefix = if path.is_empty() {
        key(lib, "")
    } else {
        format!("{}/", key(lib, path))
    };
    let (files, dirs) = g.list_level(&prefix).await?;
    if !path.is_empty() && files.is_empty() && dirs.is_empty() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let mut out = Vec::new();
    for d in dirs {
        let rel = d.trim_start_matches(&prefix).trim_end_matches('/');
        if rel.is_empty() || (path.is_empty() && rel == TRASH) {
            continue;
        }
        out.push(serde_json::json!({ "name": rel, "type": "directory", "mtime": "" }));
    }
    for f in files {
        let rel = f.key.trim_start_matches(&prefix);
        if rel.is_empty() {
            continue;
        }
        out.push(serde_json::json!({
            "name": rel,
            "type": "file",
            "mtime": dav_date(&f.modified),
            "size": f.size,
        }));
    }
    Ok(axum::Json(out).into_response())
}

async fn propfind(g: &Gate, lib: &str, path: &str, headers: &HeaderMap) -> Result<Response> {
    let depth = headers
        .get("depth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("1");
    let name = path.rsplit('/').next().unwrap_or("").to_string();
    // a file?
    if !path.is_empty()
        && let Some((size, modified)) = g.stat(&key(lib, path)).await?
    {
        return Ok(multistatus(propfind_response(
            &href(lib, path, false),
            &name,
            false,
            size,
            &modified,
        )));
    }
    // a folder: the root always, another if anything is under it
    let prefix = if path.is_empty() {
        key(lib, "")
    } else {
        format!("{}/", key(lib, path))
    };
    let (files, dirs) = g.list_level(&prefix).await?;
    if !path.is_empty() && files.is_empty() && dirs.is_empty() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let mut body = propfind_response(&href(lib, path, true), &name, true, 0, "");
    if depth != "0" {
        for d in dirs {
            let rel = d.trim_start_matches(&prefix).trim_end_matches('/');
            if rel.is_empty() || (path.is_empty() && rel == TRASH) {
                continue;
            }
            let full = if path.is_empty() {
                rel.to_string()
            } else {
                format!("{path}/{rel}")
            };
            body.push_str(&propfind_response(
                &href(lib, &full, true),
                rel,
                true,
                0,
                "",
            ));
        }
        for f in files {
            let rel = f.key.trim_start_matches(&prefix);
            // a folder's marker is not a file
            if rel.is_empty() {
                continue;
            }
            let full = if path.is_empty() {
                rel.to_string()
            } else {
                format!("{path}/{rel}")
            };
            body.push_str(&propfind_response(
                &href(lib, &full, false),
                rel,
                false,
                f.size,
                &f.modified,
            ));
        }
    }
    Ok(multistatus(body))
}

async fn get(g: &Gate, lib: &str, path: &str, headers: &HeaderMap, head: bool) -> Result<Response> {
    if path.is_empty() {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    // a device handing a file to a box's compute (transcode) asks for a
    // url the box can fetch ranges from for as long as a film plays, with
    // no token
    if !head && headers.get("x-dd-presign").is_some() {
        if g.stat(&key(lib, path)).await?.is_none() {
            return Ok(StatusCode::NOT_FOUND.into_response());
        }
        return Ok(axum::Json(serde_json::json!({
            "url": g.presign_for("GET", &key(lib, path), crate::HANDOFF_SECS)
        }))
        .into_response());
    }
    let url = g.presign(if head { "HEAD" } else { "GET" }, &key(lib, path));
    let client = reqwest::Client::new();
    let mut r = if head {
        client.head(url)
    } else {
        client.get(url)
    };
    if let Some(range) = headers.get(header::RANGE) {
        r = r.header(header::RANGE, range);
    }
    let r = r.send().await?;
    let status = r.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let mut out = Response::builder().status(status.as_u16());
    for h in [
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::LAST_MODIFIED,
        header::ETAG,
        header::ACCEPT_RANGES,
    ] {
        if let Some(v) = r.headers().get(&h) {
            out = out.header(h, v);
        }
    }
    out = out.header(header::CONTENT_TYPE, "application/octet-stream");
    let body = if head {
        Body::empty()
    } else {
        Body::from_stream(r.bytes_stream())
    };
    out.body(body).context("response")
}

async fn put(g: &Gate, lib: &str, path: &str, req: Request) -> Result<Response> {
    if path.is_empty() {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    if in_trash(path) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    // what had this name is kept: a copy into the trash, and the PUT then
    // replaces it, so a failed upload leaves the old file where it was
    let before = g.version(&key(lib, path)).await?;
    let old = g.objects_at(lib, path, false).await?;
    let kept = g.keep_in_trash(lib, path, &old, true).await?;
    let len = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let url = g.presign("PUT", &key(lib, path));
    let stream = req.into_body().into_data_stream();
    let mut r = reqwest::Client::new()
        .put(url)
        .body(reqwest::Body::wrap_stream(stream));
    if let Some(l) = len {
        r = r.header(header::CONTENT_LENGTH, l);
    }
    match r.send().await {
        Ok(r) if r.status().is_success() => Ok(StatusCode::CREATED.into_response()),
        Ok(r) => {
            g.untrash(&key(lib, path), &before, &kept).await;
            Err(anyhow!("s3 put {}", r.status()))
        }
        Err(e) => {
            g.untrash(&key(lib, path), &before, &kept).await;
            Err(e.into())
        }
    }
}

async fn mkcol(g: &Gate, lib: &str, path: &str) -> Result<Response> {
    if path.is_empty() {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    if in_trash(path) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    // a folder in a bucket is its marker object
    let r = reqwest::Client::new()
        .put(g.presign("PUT", &format!("{}/", key(lib, path))))
        .header(header::CONTENT_LENGTH, 0)
        .send()
        .await?;
    if !r.status().is_success() {
        return Err(anyhow!("s3 put {}", r.status()));
    }
    Ok(StatusCode::CREATED.into_response())
}

/// A file too large to hold in a browser's memory, sent in pieces: the
/// page seals a piece at a time and the gate hands each on as one part of
/// an S3 multipart upload. `x-dd-upload: start` (POST) opens one; the id
/// it returns then goes with each part (PUT, `x-dd-part: n`, 1-based) and
/// with the end (POST, body {"parts": [etag, ...]}) or the abandoning of
/// it (DELETE, which drops the unfinished parts and nothing else).
async fn in_pieces(
    g: &Gate,
    lib: &str,
    path: &str,
    upload: String,
    req: Request,
) -> Result<Response> {
    if path.is_empty() {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    if in_trash(path) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let object = key(lib, path);
    let at = format!("/{}", uri_encode(&object, true));
    let method = req.method().clone();
    if method == Method::POST && upload == "start" {
        let xml = g.call(reqwest::Method::POST, &at, "uploads=", &[]).await?;
        let id = between(&xml, "<UploadId>", "</UploadId>").context("no upload id")?;
        return Ok(axum::Json(serde_json::json!({ "upload": unxml(&id) })).into_response());
    }
    let ok_id = !upload.is_empty()
        && upload.len() <= 200
        && upload
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if !ok_id {
        return Ok((StatusCode::BAD_REQUEST, "not an upload id").into_response());
    }
    let id_q = format!("uploadId={}", uri_encode(&upload, false));
    match method.as_str() {
        "PUT" => {
            let Some(n) = req
                .headers()
                .get("x-dd-part")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|n| (1..=10_000).contains(n))
            else {
                return Ok((StatusCode::BAD_REQUEST, "x-dd-part: 1 to 10000").into_response());
            };
            let len = req
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let n = n.to_string();
            let url = g.presign_with(
                "PUT",
                &object,
                600,
                &[("partNumber", &n), ("uploadId", &upload)],
            );
            let mut r = reqwest::Client::new()
                .put(url)
                .body(reqwest::Body::wrap_stream(
                    req.into_body().into_data_stream(),
                ));
            if let Some(l) = len {
                r = r.header(header::CONTENT_LENGTH, l);
            }
            let r = r.send().await?;
            if !r.status().is_success() {
                return Err(anyhow!("s3 part {}", r.status()));
            }
            let etag = r
                .headers()
                .get(header::ETAG)
                .and_then(|v| v.to_str().ok())
                .context("no etag for the part")?
                .to_string();
            Ok(axum::Json(serde_json::json!({ "etag": etag })).into_response())
        }
        "POST" => {
            let body = axum::body::to_bytes(req.into_body(), 1 << 20).await?;
            let v: serde_json::Value = serde_json::from_slice(&body).context("the parts")?;
            let parts = v["parts"].as_array().context("no parts")?;
            let mut xml_body = String::from("<CompleteMultipartUpload>");
            for (i, e) in parts.iter().enumerate() {
                let e = e.as_str().context("an etag is a string")?;
                xml_body.push_str(&format!(
                    "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>",
                    i + 1,
                    xml(e)
                ));
            }
            xml_body.push_str("</CompleteMultipartUpload>");
            // as a whole PUT: what had this name goes to the trash first
            let before = g.version(&key(lib, path)).await?;
            let old = g.objects_at(lib, path, false).await?;
            let kept = g.keep_in_trash(lib, path, &old, true).await?;
            if let Err(e) = g
                .call_with(
                    reqwest::Method::POST,
                    &at,
                    &id_q,
                    &[],
                    xml_body.into_bytes(),
                )
                .await
            {
                g.untrash(&key(lib, path), &before, &kept).await;
                return Err(e);
            }
            Ok(StatusCode::CREATED.into_response())
        }
        "DELETE" => {
            g.call(reqwest::Method::DELETE, &at, &id_q, &[]).await?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        _ => Ok(StatusCode::METHOD_NOT_ALLOWED.into_response()),
    }
}

/// gone from where it was, kept under trash with a stamp so nothing there
/// is ever overwritten
async fn delete(g: &Gate, lib: &str, path: &str) -> Result<Response> {
    if path.is_empty() || in_trash(path) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let objects = g.objects_at(lib, path, true).await?;
    if objects.is_empty() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    g.keep_in_trash(lib, path, &objects, false).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn move_or_copy(
    g: &Gate,
    lib: &str,
    path: &str,
    headers: &HeaderMap,
    mv: bool,
) -> Result<Response> {
    let Some(dest) = headers.get("destination").and_then(|v| v.to_str().ok()) else {
        return Ok((StatusCode::BAD_REQUEST, "no destination").into_response());
    };
    // the destination as a path under the same library, however the
    // client wrote the url around it
    let marker = format!("/_dd/dav/{lib}/");
    let Some(rest) = dest.find(&marker).map(|i| &dest[i + marker.len()..]) else {
        return Ok((StatusCode::FORBIDDEN, "not within this library").into_response());
    };
    let rest = rest.split('?').next().unwrap_or(rest);
    let decoded = percent_decode(rest);
    let Some(to) = clean(&decoded) else {
        return Ok((StatusCode::BAD_REQUEST, "not a path").into_response());
    };
    if refused(path, &to, mv) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let from = key(lib, path);
    let source = g.objects_at(lib, path, true).await?;
    if source.is_empty() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    // Overwrite: F, as WebDAV has it - nothing already there is replaced.
    // Without it what was there goes to the trash first, as a DELETE of it
    // would: a rename onto a taken name never drops a file outright.
    let keep = headers
        .get("overwrite")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("F"));
    let taken = g.objects_at(lib, &to, true).await?;
    if !taken.is_empty() {
        if keep {
            return Ok((
                StatusCode::PRECONDITION_FAILED,
                "something is already there",
            )
                .into_response());
        }
        g.keep_in_trash(lib, &to, &taken, false).await?;
    }
    for o in source {
        let target = if o == from {
            key(lib, &to)
        } else {
            let rel = o.strip_prefix(&format!("{from}/")).unwrap_or(&o);
            format!("{}/{rel}", key(lib, &to))
        };
        if mv {
            g.rename(&o, &target).await?;
        } else {
            g.copy(&o, &target).await?;
        }
    }
    Ok(StatusCode::CREATED.into_response())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // three bytes of room, checked by addition: `b.len() - 1` is an
        // underflow waiting for an empty string
        if b[i] == b'%'
            && i + 3 <= b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
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
    use super::*;

    #[test]
    fn only_one_library_or_one_members_images_can_be_forgotten() {
        assert!(top_prefix("0123456789abcdef0123456789abcdef"));
        assert!(top_prefix("images-tom"));
        for no in [
            "",
            "images-",
            "images-a/b",
            "0123",
            "../x",
            "images-tom/../x",
        ] {
            assert!(!top_prefix(no), "{no}");
        }
    }

    #[test]
    fn paths_are_kept_inside() {
        assert_eq!(clean("/a/b/"), Some("a/b".into()));
        assert_eq!(clean(""), Some(String::new()));
        assert_eq!(clean("a/../b"), None);
        assert_eq!(clean("a//b"), None);
        assert_eq!(key("l", ""), "l/");
        assert_eq!(key("l", "a"), "l/a");
    }

    #[test]
    fn dates() {
        assert_eq!(
            dav_date("2026-09-24T01:02:03.000Z"),
            "Thu, 24 Sep 2026 01:02:03 GMT"
        );
        assert_eq!(
            dav_date("Thu, 24 Sep 2026 01:02:03 GMT"),
            "Thu, 24 Sep 2026 01:02:03 GMT"
        );
    }

    #[test]
    fn decoding() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("bad%"), "bad%");
    }

    #[test]
    fn a_name_beginning_with_trash_is_not_the_trash() {
        // base32hex names may start with "trash"; only the folder is it
        assert!(!in_trash("trashv0k1"));
        assert!(!refused("trashv0k1", "a", true));
        assert!(!refused("a", "trashv0k1", true));
        assert!(in_trash("trash") && in_trash("trash/1/a"));
        assert!(refused("a", "trash/1/a", true));
        assert!(refused("trash", "a", true));
        assert!(refused("trash/1/a", "a", false), "a copy out is no restore");
        assert!(!refused("trash/1/a", "a", true));
        assert!(refused("a", "a", true));
        assert!(refused("a", "a/b", true));
        assert!(refused("a/b", "a", true));
    }

    // A bucket in memory, enough of S3 for the gate: objects by key, a
    // listing, HEAD, GET, PUT, copy and DELETE. Signatures are not checked.
    type Bucket = std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>;

    async fn fake_s3(bucket: Bucket, req: Request) -> Response {
        let path = percent_decode(req.uri().path());
        let object = path.trim_start_matches("/b/").to_string();
        let query: Vec<(String, String)> = req
            .uri()
            .query()
            .unwrap_or("")
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_string(), percent_decode(v)))
            .collect();
        let q = |k: &str| query.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
        let copy = req
            .headers()
            .get("x-amz-copy-source")
            .and_then(|v| v.to_str().ok())
            .map(|v| percent_decode(v).trim_start_matches("/b/").to_string());
        let method = req.method().clone();
        let body = axum::body::to_bytes(req.into_body(), 1 << 20)
            .await
            .unwrap();
        let mut b = bucket.lock().unwrap();
        match method.as_str() {
            "GET" if q("list-type").is_some() => {
                let prefix = q("prefix").unwrap_or_default();
                let delim = q("delimiter").is_some();
                let mut out = String::from("<ListBucketResult>");
                let mut dirs = std::collections::BTreeSet::new();
                for (k, v) in b.range(prefix.clone()..) {
                    if !k.starts_with(&prefix) {
                        break;
                    }
                    let rest = &k[prefix.len()..];
                    if delim && let Some(i) = rest.find('/') {
                        dirs.insert(format!("{prefix}{}", &rest[..=i]));
                        continue;
                    }
                    out.push_str(&format!(
                        "<Contents><Key>{k}</Key><Size>{}</Size><LastModified>2026-01-01T00:00:00.000Z</LastModified></Contents>",
                        v.len()
                    ));
                }
                for d in dirs {
                    out.push_str(&format!(
                        "<CommonPrefixes><Prefix>{d}</Prefix></CommonPrefixes>"
                    ));
                }
                out.push_str("<IsTruncated>false</IsTruncated></ListBucketResult>");
                out.into_response()
            }
            "HEAD" | "GET" => match b.get(&object) {
                Some(v) => (
                    [(header::CONTENT_LENGTH, v.len().to_string())],
                    if method == Method::HEAD {
                        Vec::new()
                    } else {
                        v.clone()
                    },
                )
                    .into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            },
            "PUT" => {
                let v = match copy {
                    Some(from) => match b.get(&from) {
                        Some(v) => v.clone(),
                        None => return StatusCode::NOT_FOUND.into_response(),
                    },
                    // a body the store refuses: what an upload cut off looks like to it
                    None if body.as_ref() == b"refused" => {
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                    None => body.to_vec(),
                };
                // kept, but the answer is lost on the way back
                let lost = v == b"lost";
                b.insert(object, v);
                if lost {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                StatusCode::OK.into_response()
            }
            "DELETE" => {
                b.remove(&object);
                StatusCode::NO_CONTENT.into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        }
    }

    async fn bucket() -> (Gate, Bucket) {
        let bucket: Bucket = Default::default();
        let b = bucket.clone();
        let app = axum::Router::new().fallback(move |req: Request| fake_s3(b.clone(), req));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
        let g = Gate {
            endpoint: format!("http://{at}"),
            bucket: "b".into(),
            key_id: "k".into(),
            key_secret: "s".into(),
            region: "garage".into(),
        };
        (g, bucket)
    }

    async fn dav(
        g: &Gate,
        method: &str,
        path: &str,
        hs: &[(&str, &str)],
        body: &str,
    ) -> StatusCode {
        let mut r = Request::builder()
            .method(method)
            .uri(format!("/_dd/dav/L/{path}"));
        for (k, v) in hs {
            r = r.header(*k, *v);
        }
        let req = r.body(Body::from(body.to_string())).unwrap();
        serve(g, "L".into(), path.into(), Role::Owner, req)
            .await
            .status()
    }

    fn trashed(b: &Bucket, name: &str) -> Vec<Vec<u8>> {
        b.lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with("L/trash/") && k.ends_with(&format!("/{name}")))
            .map(|(_, v)| v.clone())
            .collect()
    }

    #[tokio::test]
    async fn a_put_over_a_file_keeps_the_old_one_in_the_trash() {
        let (g, b) = bucket().await;
        assert_eq!(dav(&g, "PUT", "a", &[], "one").await, StatusCode::CREATED);
        assert!(trashed(&b, "a").is_empty(), "a new name trashes nothing");
        assert_eq!(dav(&g, "PUT", "a", &[], "two").await, StatusCode::CREATED);
        assert_eq!(b.lock().unwrap()["L/a"], b"two");
        assert_eq!(trashed(&b, "a"), vec![b"one".to_vec()]);
        // and again within the same second: the first kept copy stays
        assert_eq!(dav(&g, "PUT", "a", &[], "three").await, StatusCode::CREATED);
        let mut kept = trashed(&b, "a");
        kept.sort();
        assert_eq!(kept, vec![b"one".to_vec(), b"two".to_vec()]);
    }

    #[tokio::test]
    async fn a_reader_writes_nothing_dressed_as_a_read() {
        let (g, b) = bucket().await;
        for m in ["GET", "HEAD", "PROPFIND", "OPTIONS"] {
            let req = Request::builder()
                .method(m)
                .uri("/_dd/dav/L/a")
                .header("x-dd-upload", "start")
                .body(Body::from("x"))
                .unwrap();
            let r = serve(&g, "L".into(), "a".into(), Role::Reader, req).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{m}");
        }
        assert!(b.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_put_that_fails_leaves_no_copy_in_the_trash() {
        let (g, b) = bucket().await;
        dav(&g, "PUT", "a", &[], "one").await;
        // a write the store does not take, as when the allowance cuts one off
        for _ in 0..3 {
            assert!(!dav(&g, "PUT", "a", &[], "refused").await.is_success());
        }
        assert_eq!(b.lock().unwrap()["L/a"], b"one");
        assert!(trashed(&b, "a").is_empty(), "no copy per failed try");
    }

    #[tokio::test]
    async fn a_write_whose_answer_was_lost_keeps_the_old_one_in_the_trash() {
        let (g, b) = bucket().await;
        dav(&g, "PUT", "a", &[], "one").await;
        assert!(!dav(&g, "PUT", "a", &[], "lost").await.is_success());
        assert_eq!(b.lock().unwrap()["L/a"], b"lost");
        assert_eq!(trashed(&b, "a"), vec![b"one".to_vec()]);
    }

    #[tokio::test]
    async fn two_writes_of_one_name_at_once_keep_both_old_ones() {
        let (g, b) = bucket().await;
        dav(&g, "PUT", "a", &[], "one").await;
        dav(&g, "PUT", "a", &[], "two").await;
        let (x, y) = tokio::join!(
            dav(&g, "PUT", "a", &[], "three"),
            dav(&g, "PUT", "a", &[], "four")
        );
        assert!(x.is_success() && y.is_success());
        // one, two, and whichever of three and four landed first
        assert_eq!(trashed(&b, "a").len(), 3);
    }

    #[tokio::test]
    async fn a_move_onto_a_taken_name_trashes_what_was_there() {
        let (g, b) = bucket().await;
        dav(&g, "PUT", "a", &[], "from").await;
        dav(&g, "PUT", "b", &[], "there").await;
        let to = [("destination", "https://x/_dd/dav/L/b")];
        let keep = [("destination", "https://x/_dd/dav/L/b"), ("overwrite", "F")];
        assert_eq!(
            dav(&g, "MOVE", "a", &keep, "").await,
            StatusCode::PRECONDITION_FAILED
        );
        assert_eq!(dav(&g, "MOVE", "a", &to, "").await, StatusCode::CREATED);
        let all = b.lock().unwrap().clone();
        assert_eq!(all["L/b"], b"from");
        assert!(!all.contains_key("L/a"));
        assert_eq!(trashed(&b, "b"), vec![b"there".to_vec()]);
    }

    #[tokio::test]
    async fn nothing_is_written_into_the_trash_but_a_restore_comes_out() {
        let (g, b) = bucket().await;
        for (m, p) in [("PUT", "trash/1/x"), ("PUT", "trash"), ("MKCOL", "trash/1")] {
            assert_eq!(
                dav(&g, m, p, &[], "x").await,
                StatusCode::FORBIDDEN,
                "{m} {p}"
            );
        }
        dav(&g, "PUT", "a", &[], "kept").await;
        let into = [("destination", "https://x/_dd/dav/L/trash/1/a")];
        assert_eq!(dav(&g, "MOVE", "a", &into, "").await, StatusCode::FORBIDDEN);
        assert_eq!(
            dav(&g, "DELETE", "a", &[], "").await,
            StatusCode::NO_CONTENT
        );
        let at = b
            .lock()
            .unwrap()
            .keys()
            .find(|k| k.starts_with("L/trash/"))
            .unwrap()
            .trim_start_matches("L/")
            .to_string();
        assert!(b.lock().unwrap().keys().all(|k| k.starts_with("L/trash/")));
        let back = [("destination", "https://x/_dd/dav/L/a"), ("overwrite", "F")];
        assert_eq!(dav(&g, "COPY", &at, &back, "").await, StatusCode::FORBIDDEN);
        assert_eq!(dav(&g, "MOVE", &at, &back, "").await, StatusCode::CREATED);
        assert_eq!(b.lock().unwrap()["L/a"], b"kept");
    }

    #[tokio::test]
    async fn a_name_beginning_with_trash_moves_like_any_other() {
        let (g, b) = bucket().await;
        dav(&g, "PUT", "trashv0k1", &[], "x").await;
        let to = [("destination", "https://x/_dd/dav/L/c")];
        assert_eq!(
            dav(&g, "MOVE", "trashv0k1", &to, "").await,
            StatusCode::CREATED
        );
        assert_eq!(b.lock().unwrap()["L/c"], b"x");
    }

    #[tokio::test]
    async fn two_deletes_of_one_name_in_a_second_keep_both() {
        let (g, b) = bucket().await;
        dav(&g, "PUT", "a", &[], "one").await;
        dav(&g, "DELETE", "a", &[], "").await;
        dav(&g, "PUT", "a", &[], "two").await;
        dav(&g, "DELETE", "a", &[], "").await;
        let mut kept = trashed(&b, "a");
        kept.sort();
        assert_eq!(kept, vec![b"one".to_vec(), b"two".to_vec()]);
    }
}
