//! A library as WebDAV: what rclone, a file manager and the app all speak.
//! One prefix in the bucket per library, the device token as bearer, an
//! owner may write and a reader may only read, and nothing is deleted at
//! once: DELETE moves into `trash/` under the library, and the trash lets
//! go of it after 90 days. The bytes and
//! names through here are rclone's crypt format, made and read on the
//! device; this box moves ciphertext.
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
/// how long it stays there before it is gone for good
pub const TRASH_SECS: u64 = 90 * 24 * 3600;

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
            if !identity::valid_library_id(id) {
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
    let writes = matches!(
        method.as_str(),
        "PUT" | "POST" | "MKCOL" | "MOVE" | "COPY" | "DELETE" | "PROPPATCH"
    );
    if writes && role != Role::Owner {
        return (StatusCode::FORBIDDEN, "a reader does not write").into_response();
    }
    if let Some(upload) = headers.get("x-dd-upload").and_then(|v| v.to_str().ok()) {
        let out = in_pieces(g, &lib, &path, upload.to_string(), req).await;
        return match out {
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
    let r = r.send().await?;
    if !r.status().is_success() {
        return Err(anyhow!("s3 put {}", r.status()));
    }
    Ok(StatusCode::CREATED.into_response())
}

async fn mkcol(g: &Gate, lib: &str, path: &str) -> Result<Response> {
    if path.is_empty() {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
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
            g.call_with(
                reqwest::Method::POST,
                &at,
                &id_q,
                &[],
                xml_body.into_bytes(),
            )
            .await?;
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
    if path.is_empty() || path == TRASH || path.starts_with("trash/") {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let from = key(lib, path);
    let mut any = false;
    if g.stat(&from).await?.is_some() {
        g.rename(&from, &format!("{lib}/{TRASH}/{stamp}/{path}"))
            .await?;
        any = true;
    }
    for e in g.list_all(&format!("{from}/")).await? {
        let rel = e.key.trim_start_matches(&format!("{lib}/"));
        g.rename(&e.key, &format!("{lib}/{TRASH}/{stamp}/{rel}"))
            .await?;
        any = true;
    }
    Ok(if any {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    })
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
    // out of the trash is a restore, and only that: a thing put there
    // (trash/<stamp>/...) moved back into the library, never copied, and
    // nothing moved into the trash but by DELETE
    let mut under = path.split('/');
    let restoring = mv
        && under.next() == Some(TRASH)
        && under
            .next()
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        && under.next().is_some();
    if path.is_empty()
        || to.is_empty()
        || to == TRASH
        || to.starts_with("trash/")
        || (path.starts_with("trash") && !restoring)
    {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    // Overwrite: F, as WebDAV has it - nothing already there is replaced.
    // A rename onto a taken name would otherwise drop what was there, and
    // not into the trash either.
    let keep = headers
        .get("overwrite")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("F"));
    if keep
        && (g.stat(&key(lib, &to)).await?.is_some()
            || !g.list_all(&format!("{}/", key(lib, &to))).await?.is_empty())
    {
        return Ok((
            StatusCode::PRECONDITION_FAILED,
            "something is already there",
        )
            .into_response());
    }
    let from = key(lib, path);
    let mut any = false;
    if g.stat(&from).await?.is_some() {
        if mv {
            g.rename(&from, &key(lib, &to)).await?;
        } else {
            g.copy(&from, &key(lib, &to)).await?;
        }
        any = true;
    }
    for e in g.list_all(&format!("{from}/")).await? {
        let rel = e.key.trim_start_matches(&format!("{from}/"));
        let target = format!("{}/{rel}", key(lib, &to));
        if mv {
            g.rename(&e.key, &target).await?;
        } else {
            g.copy(&e.key, &target).await?;
        }
        any = true;
    }
    Ok(if any {
        StatusCode::CREATED.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    })
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
}
