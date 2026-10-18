//! A member's mail on this box (docs/dev/mail.md): only ever locked. The
//! mail Worker locks what arrives to the member's key and holds it; this
//! gate takes it from there, signed as this box (the hold socket, which
//! signs that and nothing else), and keeps it in the mail bucket under the
//! member's name. Their app reads and writes their own objects here, hands
//! mail for another member straight to that member's store, and sends mail
//! out through the pipe: a WebSocket joined to port 25 of the other side's
//! server, carrying the app's own TLS. Nothing here can open any of it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::App;

/// the kinds of thing a member keeps: what came, what they sent, drafts, the
/// changes their devices agree on (read, starred, moved), their search
/// index's pieces, their spam filter, and their key, sealed to their devices
pub const KINDS: &[&str] = &["in", "sent", "drafts", "state", "index", "filter", "key"];
/// one object at most this big: Email Routing takes 25 MB, locked a little more
pub const MAX: usize = 32 << 20;
/// mail handed to another member, per sender per hour
const DELIVER_PER_HOUR: u32 = 120;
/// pipes opened per member per hour
const PIPES_PER_HOUR: u32 = 120;
/// bytes of mail one member may hand to others' inboxes in an hour
const DELIVER_BYTES_PER_HOUR: u64 = 200 << 20;
/// requests a visitor may make a minute, reading and writing
const READS_PER_MINUTE: usize = 600;
const WRITES_PER_MINUTE: usize = 120;
/// how long a pipe may stay open
const PIPE_SECS: u64 = 600;

pub struct Mail {
    pub s3: library_gate::Gate,
    /// https://mail.<domain>
    pub worker: String,
    /// this box's name, as fleet.js BOXES knows it
    pub box_name: String,
    /// the hold socket: signs "commonty mail hold v1\0<box>\0..." for us
    pub sign: PathBuf,
    /// DNS-over-HTTPS, for the MX of a domain mail is sent to
    pub doh: String,
    counts: std::sync::Mutex<std::collections::HashMap<(String, &'static str, u64), u32>>,
    sent_bytes: std::sync::Mutex<std::collections::HashMap<(String, u64), u64>>,
    visitors: std::sync::Mutex<crate::directory::Window>,
    clock: std::sync::Mutex<u64>,
}

pub fn from_env() -> Result<Option<Mail>> {
    let Ok(worker) = std::env::var("VERIFY_MAIL_WORKER") else {
        return Ok(None);
    };
    Ok(Some(Mail {
        s3: library_gate::Gate {
            endpoint: std::env::var("VERIFY_MAIL_S3")
                .or_else(|_| std::env::var("VERIFY_LIBRARY_S3"))
                .context("VERIFY_MAIL_S3")?
                .trim_end_matches('/')
                .to_string(),
            bucket: std::env::var("VERIFY_MAIL_BUCKET").unwrap_or_else(|_| "mail".into()),
            // the libraries' key, which the mail bucket is given too
            key_id: std::env::var("MAIL_ID")
                .or_else(|_| std::env::var("LIBRARY_ID"))
                .context("MAIL_ID")?,
            key_secret: std::env::var("MAIL_SECRET")
                .or_else(|_| std::env::var("LIBRARY_SECRET"))
                .context("MAIL_SECRET")?,
            region: std::env::var("VERIFY_LIBRARY_REGION").unwrap_or_else(|_| "us-east-1".into()),
        },
        worker: worker.trim_end_matches('/').to_string(),
        box_name: std::env::var("VERIFY_MAIL_BOX").context("VERIFY_MAIL_BOX")?,
        sign: std::env::var("VERIFY_MAIL_SIGN")
            .unwrap_or_else(|_| "/run/dd-mail-hold/sign.sock".into())
            .into(),
        doh: std::env::var("VERIFY_MAIL_DOH")
            .unwrap_or_else(|_| "https://cloudflare-dns.com/dns-query".into()),
        counts: Default::default(),
        sent_bytes: Default::default(),
        visitors: Default::default(),
        clock: std::sync::Mutex::new(0),
    }))
}

pub fn valid_id(kind: &str, id: &str) -> bool {
    let ok = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    KINDS.contains(&kind) && !id.is_empty() && id.len() <= 64 && id.chars().all(ok)
}

fn name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
}

impl Mail {
    /// counted in the hour: false once `max` is reached
    fn spend(&self, who: &str, what: &'static str, max: u32) -> bool {
        let hour = now() / 3600;
        let mut c = self.counts.lock().unwrap();
        c.retain(|(_, _, h), _| *h + 1 >= hour);
        let n = c.entry((who.to_string(), what, hour)).or_default();
        if *n >= max {
            return false;
        }
        *n += 1;
        true
    }

    /// bytes counted in the hour: false once `max` would be passed
    fn spend_bytes(&self, who: &str, n: u64, max: u64) -> bool {
        let hour = now() / 3600;
        let mut c = self.sent_bytes.lock().unwrap();
        c.retain(|(_, h), _| *h + 1 >= hour);
        let used = c.entry((who.to_string(), hour)).or_default();
        if used.saturating_add(n) > max {
            return false;
        }
        *used += n;
        true
    }

    /// a signed time newer than the last, in milliseconds
    fn tick(&self) -> u64 {
        let mut c = self.clock.lock().unwrap();
        let t = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64)
            .max(*c + 1);
        *c = t;
        t
    }

    async fn signed(&self, method: &str, path: &str) -> Result<reqwest::RequestBuilder> {
        let at = self.tick();
        let digest = hex(&<sha2::Sha256 as sha2::Digest>::digest(b""));
        let msg = format!(
            "commonty mail hold v1\0{}\0{at}\0{method}\0{path}\0{digest}",
            self.box_name
        );
        let sig = sign_with(&self.sign, msg.as_bytes()).await?;
        let url = format!("{}{path}", self.worker);
        let c = reqwest::Client::new();
        let r = if method == "POST" {
            c.post(url).body(Vec::<u8>::new())
        } else {
            c.get(url)
        };
        Ok(r.header("x-dd-box", &self.box_name)
            .header("x-dd-at", at.to_string())
            .header("x-dd-sig", sig))
    }

    /// Take what the Worker holds: each one kept here, then let go there.
    /// Only for members of this fleet; anyone else's waits for a box that
    /// knows them.
    pub async fn take(&self, member: impl Fn(&str) -> bool) -> Result<usize> {
        #[derive(Deserialize)]
        struct Item {
            id: String,
            member: String,
            parts: u32,
        }
        #[derive(Deserialize)]
        struct Items {
            items: Vec<Item>,
        }
        // page by page: mail this box passes over (a member it does not know)
        // never keeps it from what comes after
        let mut kept = 0;
        let mut after = String::new();
        loop {
            let r = self
                .signed("GET", &format!("/api/hold/list?after={after}"))
                .await?
                .send()
                .await?;
            if !r.status().is_success() {
                bail!("the mail Worker said {}", r.status());
            }
            let list: Items = r.json().await?;
            let Some(last) = list.items.last().map(|i| i.id.clone()) else {
                return Ok(kept);
            };
            if !valid_id("in", &last) || last <= after {
                bail!("the mail Worker's list went backwards");
            }
            after = last;
            for it in list.items {
                if !name_ok(&it.member)
                    || !member(&it.member)
                    || !valid_id("in", &it.id)
                    || it.parts == 0
                    || it.parts > 40
                {
                    continue;
                }
                let mut blob = Vec::new();
                for n in 0..it.parts {
                    let r = self
                        .signed("GET", &format!("/api/hold/get?id={}&n={n}", it.id))
                        .await?
                        .send()
                        .await?;
                    if !r.status().is_success() {
                        bail!("the mail Worker said {} for a part", r.status());
                    }
                    blob.extend_from_slice(&r.bytes().await?);
                    if blob.len() > MAX {
                        bail!("held mail bigger than any");
                    }
                }
                let size = blob.len().to_string();
                self.s3
                    .put_bytes(
                        &format!("{}/in/{}", it.member, it.id),
                        blob,
                        &[("size", &size)],
                    )
                    .await?;
                let r = self
                    .signed("POST", &format!("/api/hold/ack?id={}", it.id))
                    .await?
                    .send()
                    .await?;
                if !r.status().is_success() {
                    bail!("the mail Worker said {} to the ack", r.status());
                }
                kept += 1;
            }
        }
    }
}

impl Mail {
    /// someone left: every locked object under their name, gone
    pub async fn forget(&self, name: &str) -> Result<usize> {
        if !name_ok(name) {
            bail!("not a name");
        }
        let prefix = format!("{name}/");
        let mut n = 0;
        loop {
            let items = self.s3.list(&prefix, "", 1000).await?;
            if items.is_empty() {
                return Ok(n);
            }
            for (k, _) in items {
                self.s3.delete_object(&k).await?;
                n += 1;
            }
        }
    }
}

/// every 15 seconds, and at once after anything was taken
pub(crate) async fn run(app: Arc<App>) {
    let Some(m) = app.mail.as_ref() else { return };
    loop {
        let again = match m.take(|n| app.member(n)).await {
            Ok(n) => n > 0,
            Err(e) => {
                eprintln!("mail: taking held mail: {e:#}");
                false
            }
        };
        if !again {
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    }
}

/// The hold socket signs one message: written whole, then read back as
/// base64url. dd-mail-hold-sign (root) refuses anything else.
async fn sign_with(sock: &std::path::Path, msg: &[u8]) -> Result<String> {
    let mut s = tokio::net::UnixStream::connect(sock)
        .await
        .with_context(|| format!("the hold socket {}", sock.display()))?;
    s.write_all(msg).await?;
    s.shutdown().await?;
    let mut out = String::new();
    s.read_to_string(&mut out).await?;
    let out = out.trim().to_string();
    if out.is_empty() {
        bail!("the hold socket signed nothing");
    }
    Ok(out)
}

#[allow(clippy::result_large_err)]
fn who<'a>(
    app: &'a App,
    headers: &HeaderMap,
    write: bool,
) -> std::result::Result<(String, &'a Mail), Response> {
    let Some(m) = app.mail.as_ref() else {
        return Err((StatusCode::NOT_FOUND, "no mail on this box").into_response());
    };
    // each visitor's share, before anything else is read or asked
    let most = if write {
        WRITES_PER_MINUTE
    } else {
        READS_PER_MINUTE
    };
    let source = crate::directory::source_of(headers);
    if !m.visitors.lock().unwrap().take(
        &format!("{source}\0{write}"),
        most,
        std::time::Instant::now(),
    ) {
        return Err((StatusCode::TOO_MANY_REQUESTS, "slow down").into_response());
    }
    let Some(user) = app.identify(headers, "access") else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "a device token or a signed-in browser is required",
        )
            .into_response());
    };
    if user == crate::pages::DEMO_USER {
        return Err((StatusCode::FORBIDDEN, "the demo has no mail").into_response());
    }
    // who is a member is not known yet: not "nobody", so not a refusal
    if matches!(app.directory.ident_from, crate::ident::From::Agreed)
        && !app.directory.agreed_known()
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "this box has not read who is a member yet",
        )
            .into_response());
    }
    if !app.member(&user) {
        // suspended: their own mail, to read and take, nothing more
        if !write && app.suspension(&user).is_some() {
            return Ok((user, m));
        }
        return Err((StatusCode::FORBIDDEN, "members only").into_response());
    }
    Ok((user, m))
}

#[derive(Deserialize)]
pub(crate) struct ListQ {
    #[serde(default)]
    after: String,
    #[serde(default)]
    max: Option<usize>,
}

/// GET /_dd/mail/list/{kind}?after=<id>: [{id, size}], in id order
pub(crate) async fn list(
    State(app): State<Arc<App>>,
    Path(kind): Path<String>,
    Query(q): Query<ListQ>,
    headers: HeaderMap,
) -> Response {
    let (user, m) = match who(&app, &headers, false) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if !KINDS.contains(&kind.as_str()) || !(q.after.is_empty() || valid_id(&kind, &q.after)) {
        return (StatusCode::BAD_REQUEST, "not a kind of mail").into_response();
    }
    let prefix = format!("{user}/{kind}/");
    let after = if q.after.is_empty() {
        String::new()
    } else {
        format!("{prefix}{}", q.after)
    };
    match m
        .s3
        .list(&prefix, &after, q.max.unwrap_or(1000).clamp(1, 1000))
        .await
    {
        Ok(items) => axum::Json(
            items
                .into_iter()
                .map(|(k, size)| serde_json::json!({ "id": k[prefix.len()..], "size": size }))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("storage: {e}")).into_response(),
    }
}

/// GET, PUT and DELETE /_dd/mail/o/{kind}/{id}: one locked object of theirs
pub(crate) async fn object(
    State(app): State<Arc<App>>,
    Path((kind, id)): Path<(String, String)>,
    method: axum::http::Method,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let write = method != axum::http::Method::GET && method != axum::http::Method::HEAD;
    let (user, m) = match who(&app, &headers, write) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if !valid_id(&kind, &id) {
        return (StatusCode::BAD_REQUEST, "not an id").into_response();
    }
    // the body is read only now, for someone it may be from, and no more than an object may be
    let body = if method == axum::http::Method::PUT {
        match axum::body::to_bytes(body, MAX).await {
            Ok(b) => b,
            Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        }
    } else {
        Bytes::new()
    };
    let key = format!("{}/{kind}/{id}", user.as_str());
    match method {
        // a part of it: the list reads each message's summary alone, which
        // is at the front (bytes=0-65535), never its attachments
        axum::http::Method::GET if headers.contains_key("range") => {
            let range = headers
                .get("range")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !range.starts_with("bytes=") || range.len() > 64 {
                return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            }
            let url = m.s3.presign_for("GET", &key, 60);
            match reqwest::Client::new()
                .get(&url)
                .header("range", range)
                .send()
                .await
            {
                Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {
                    StatusCode::NOT_FOUND.into_response()
                }
                Ok(r)
                    if r.status().is_success()
                        || r.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE =>
                {
                    let status = r.status().as_u16();
                    let cr = r
                        .headers()
                        .get("content-range")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    let b = r.bytes().await.unwrap_or_default();
                    let mut out = Response::builder()
                        .status(status)
                        .header("content-type", "application/octet-stream")
                        .header("cache-control", "private, no-store");
                    if let Some(cr) = cr {
                        out = out.header("content-range", cr);
                    }
                    out.body(Body::from(b)).unwrap()
                }
                Ok(r) => {
                    (StatusCode::BAD_GATEWAY, format!("storage: {}", r.status())).into_response()
                }
                Err(e) => (StatusCode::BAD_GATEWAY, format!("storage: {e}")).into_response(),
            }
        }
        axum::http::Method::GET => match m.s3.get_bytes(&key).await {
            Ok(Some((b, meta))) => {
                let mut r = Response::builder()
                    .header("content-type", "application/octet-stream")
                    // what came and what was sent never change; drafts do
                    .header(
                        "cache-control",
                        if kind == "in" || kind == "sent" {
                            "private, max-age=31536000, immutable"
                        } else {
                            "private, no-store"
                        },
                    );
                if let Some((_, f)) = meta.iter().find(|(k, _)| k == "from") {
                    r = r.header("x-dd-from", f);
                }
                r.body(Body::from(b)).unwrap()
            }
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => (StatusCode::BAD_GATEWAY, format!("storage: {e}")).into_response(),
        },
        axum::http::Method::PUT => {
            if body.len() > MAX {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            }
            let n = body.len() as u64;
            let Some(held) = app.storage.hold(&user, Some(n)) else {
                return (StatusCode::INSUFFICIENT_STORAGE, "your storage is full").into_response();
            };
            let r = m.s3.put_bytes(&key, body.to_vec(), &[]).await;
            app.storage.done(held, if r.is_ok() { n } else { 0 });
            match r {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => (StatusCode::BAD_GATEWAY, format!("storage: {e}")).into_response(),
            }
        }
        axum::http::Method::DELETE => match m.s3.delete_object(&key).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => (StatusCode::BAD_GATEWAY, format!("storage: {e}")).into_response(),
        },
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

/// POST /_dd/mail/deliver/{to}/{id}: mail one member's app locked to
/// another's key, put in that member's inbox as it is. The sender is
/// written beside it, as this gate knows them; their app signed what is
/// inside, and the reader's app checks that against the sender's devices.
pub(crate) async fn deliver(
    State(app): State<Arc<App>>,
    Path((to, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let (user, m) = match who(&app, &headers, true) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if !name_ok(&to) || !app.member(&to) {
        return (StatusCode::NOT_FOUND, "no such member").into_response();
    }
    if !valid_id("in", &id) {
        return (StatusCode::BAD_REQUEST, "not a locked message").into_response();
    }
    // they have Mail on: their key is kept here
    match m.s3.list(&format!("{to}/key/doc"), "", 1).await {
        Ok(k) if !k.is_empty() => {}
        Ok(_) => return (StatusCode::NOT_FOUND, "they have no Mail").into_response(),
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("storage: {e}")).into_response(),
    }
    let body = match axum::body::to_bytes(body, MAX).await {
        Ok(b) if b.len() >= 8 => b,
        Ok(_) => return (StatusCode::BAD_REQUEST, "not a locked message").into_response(),
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    if !m.spend_bytes(&user, body.len() as u64, DELIVER_BYTES_PER_HOUR) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "that is as much as you can send this hour",
        )
            .into_response();
    }
    if !m.spend(&user, "deliver", DELIVER_PER_HOUR) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "that is as much as you can send this hour",
        )
            .into_response();
    }
    let key = format!("{to}/in/{id}");
    // never over one that is there: an id is the sender's to make, not to reuse
    match m.s3.list(&key, "", 1).await {
        Ok(k) if k.iter().any(|(x, _)| *x == key) => {
            return (StatusCode::CONFLICT, "that id is taken").into_response();
        }
        Ok(_) => {}
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("storage: {e}")).into_response(),
    }
    // it is kept in their store, and counts toward their allowance
    let n = body.len() as u64;
    let Some(held) = app.storage.hold(&to, Some(n)) else {
        return (StatusCode::INSUFFICIENT_STORAGE, "their storage is full").into_response();
    };
    let size = body.len().to_string();
    let r =
        m.s3.put_bytes(&key, body.to_vec(), &[("from", &user), ("size", &size)])
            .await;
    app.storage.done(held, if r.is_ok() { n } else { 0 });
    match r {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("storage: {e}")).into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct PipeQ {
    domain: String,
    host: String,
}

/// GET /_dd/mail/pipe?domain=<d>&host=<mx>: a WebSocket joined to port 25 of
/// `host`, which must be a mail server `domain` names (its MX, asked of
/// DNS-over-HTTPS here, not taken from the app). Bytes only: the app runs
/// SMTP and its own TLS through it, and this gate sees ciphertext after the
/// first few lines.
pub(crate) async fn pipe(
    State(app): State<Arc<App>>,
    Query(q): Query<PipeQ>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // the member's app opens it, which sends no Origin: a page in a browser,
    // on any box, never does this with the member's cookie
    if headers.contains_key("origin") {
        return (StatusCode::FORBIDDEN, "the app opens this, not a page").into_response();
    }
    let (user, m) = match who(&app, &headers, true) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let domain = q.domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = q.host.trim().trim_end_matches('.').to_ascii_lowercase();
    if !hostname_ok(&domain) || !hostname_ok(&host) {
        return (StatusCode::BAD_REQUEST, "not a domain").into_response();
    }
    match mx(&m.doh, &domain).await {
        Ok(hosts) if hosts.contains(&host) => {}
        Ok(_) => {
            return (
                StatusCode::FORBIDDEN,
                "that is not one of the domain's mail servers",
            )
                .into_response();
        }
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("dns: {e}")).into_response(),
    }
    if !m.spend(&user, "pipe", PIPES_PER_HOUR) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "that is as much as you can send this hour",
        )
            .into_response();
    }
    // a name any domain can point anywhere: only an address on the public
    // internet, never this box, its house or the fleet's network
    let addrs: Vec<std::net::SocketAddr> = match tokio::net::lookup_host((host.as_str(), 25)).await
    {
        Ok(a) => a.filter(|a| public(a.ip())).collect(),
        Err(_) => Vec::new(),
    };
    let Some(addr) = addrs.first().copied() else {
        return (
            StatusCode::FORBIDDEN,
            "that mail server is not on the internet",
        )
            .into_response();
    };
    // who opened a way out to which server: what the box sees anyway, kept
    // in its journal, so mail sent around the Worker's signature is someone's
    eprintln!("mail: {user} opened a pipe to {host} ({domain})");
    let tcp = match tokio::time::timeout(
        Duration::from_secs(20),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        _ => return (StatusCode::BAD_GATEWAY, "their mail server did not answer").into_response(),
    };
    ws.max_message_size(1 << 20)
        .on_upgrade(move |socket| async move {
            let _ = tokio::time::timeout(Duration::from_secs(PIPE_SECS), join(socket, tcp)).await;
        })
}

/// an address out on the internet: not loopback, private, link-local,
/// shared (100.64/10, the fleet's own network), multicast or reserved
pub fn public(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_broadcast()
                || a.is_multicast()
                || a.is_unspecified()
                || a.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || o[0] >= 240
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                || (o[0] == 192 && o[1] == 88 && o[2] == 99))
        }
        std::net::IpAddr::V6(a) => {
            let s = a.segments();
            let o = a.octets();
            // an IPv4 address carried inside one: 6to4, NAT64, IPv4-mapped
            // or -compatible - only as public as it is
            let inner = |b: &[u8]| public(std::net::Ipv4Addr::new(b[0], b[1], b[2], b[3]).into());
            !(a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || (s[0] == 0x2001 && s[1] == 0)
                || (s[0] == 0x2002 && !inner(&o[2..6]))
                || (s[0..6] == [0x64, 0xff9b, 0, 0, 0, 0] && !inner(&o[12..16]))
                || (s[0..6] == [0, 0, 0, 0, 0, 0] && !inner(&o[12..16]))
                || a.to_ipv4_mapped().is_some_and(|v4| !public(v4.into())))
        }
    }
}

fn hostname_ok(h: &str) -> bool {
    h.len() <= 253
        && h.contains('.')
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !l.starts_with('-')
        })
}

/// the MX hosts of `domain`, or the domain itself when it has none (RFC 5321)
pub async fn mx(doh: &str, domain: &str) -> Result<Vec<String>> {
    let v: serde_json::Value = reqwest::Client::new()
        .get(doh)
        .query(&[("name", domain), ("type", "MX")])
        .header("accept", "application/dns-json")
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .json()
        .await?;
    if v["Status"].as_u64() != Some(0) && v["Status"].as_u64() != Some(3) {
        return Err(anyhow!("status {}", v["Status"]));
    }
    let mut hosts: Vec<(u64, String)> = v["Answer"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|r| r["type"].as_u64() == Some(15))
                .filter_map(|r| {
                    let d = r["data"].as_str()?;
                    let mut it = d.split_whitespace();
                    let pref = it.next()?.parse().ok()?;
                    Some((pref, it.next()?.trim_end_matches('.').to_ascii_lowercase()))
                })
                .collect()
        })
        .unwrap_or_default();
    hosts.sort();
    if hosts.is_empty() {
        return Ok(vec![domain.to_string()]);
    }
    Ok(hosts
        .into_iter()
        .map(|(_, h)| h)
        .filter(|h| !h.is_empty())
        .collect())
}

async fn join(mut socket: WebSocket, tcp: tokio::net::TcpStream) {
    let (mut rd, mut wr) = tcp.into_split();
    let mut buf = vec![0u8; 16 << 10];
    loop {
        tokio::select! {
            n = rd.read(&mut buf) => match n {
                Ok(0) | Err(_) => break,
                Ok(n) => if socket.send(Message::Binary(Bytes::copy_from_slice(&buf[..n]))).await.is_err() { break },
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(b))) => if wr.write_all(&b).await.is_err() { break },
                Some(Ok(Message::Text(t))) => if wr.write_all(t.as_bytes()).await.is_err() { break },
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                _ => break,
            },
        }
    }
    let _ = wr.shutdown().await;
    let _ = socket.send(Message::Close(None)).await;
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) type Store = std::sync::Arc<
        std::sync::Mutex<std::collections::BTreeMap<String, (Vec<u8>, Option<String>)>>,
    >;

    /// a bucket as the gate uses it: PUT, GET (with Range), list, DELETE
    pub(crate) async fn fake_s3() -> (String, Store) {
        use axum::extract::Request;
        let store: Store = Default::default();
        let held = store.clone();
        let app = axum::Router::new().fallback(move |req: Request| {
            let store = held.clone();
            async move {
                let path = req
                    .uri()
                    .path()
                    .trim_start_matches("/mail/")
                    .replace("%2F", "/")
                    .to_string();
                let query = req.uri().query().unwrap_or("").to_string();
                let from = req
                    .headers()
                    .get("x-amz-meta-from")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let range = req
                    .headers()
                    .get("range")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let method = req.method().clone();
                let body = axum::body::to_bytes(req.into_body(), 64 << 20)
                    .await
                    .unwrap_or_default();
                let mut b = store.lock().unwrap();
                let q = |k: &str| {
                    query
                        .split('&')
                        .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
                        .unwrap_or("")
                        .replace("%2F", "/")
                };
                match method.as_str() {
                    "GET" if query.contains("list-type") => {
                        let (prefix, after) = (q("prefix"), q("start-after"));
                        let mut out = String::from("<ListBucketResult>");
                        for (k, (v, _)) in b
                            .iter()
                            .filter(|(k, _)| k.starts_with(&prefix) && **k > after)
                        {
                            out.push_str(&format!(
                                "<Contents><Key>{k}</Key><Size>{}</Size></Contents>",
                                v.len()
                            ));
                        }
                        out.push_str("</ListBucketResult>");
                        out.into_response()
                    }
                    "GET" => match b.get(&path) {
                        Some((v, f)) => {
                            let mut r = Response::builder();
                            if let Some(f) = f {
                                r = r.header("x-amz-meta-from", f);
                            }
                            let v = match range.as_deref().and_then(|x| x.strip_prefix("bytes=")) {
                                Some(x) => {
                                    let (a, z) = x.split_once('-').unwrap();
                                    let a: usize = a.parse().unwrap();
                                    let z = z.parse::<usize>().unwrap().min(v.len() - 1);
                                    v[a..=z].to_vec()
                                }
                                None => v.clone(),
                            };
                            r.body(Body::from(v)).unwrap()
                        }
                        None => StatusCode::NOT_FOUND.into_response(),
                    },
                    "PUT" => {
                        b.insert(path, (body.to_vec(), from));
                        StatusCode::OK.into_response()
                    }
                    "DELETE" => {
                        b.remove(&path);
                        StatusCode::NO_CONTENT.into_response()
                    }
                    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
                }
            }
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
        (format!("http://{at}"), store)
    }

    pub(crate) fn mail_at(endpoint: &str) -> Mail {
        Mail {
            s3: library_gate::Gate {
                endpoint: endpoint.into(),
                bucket: "mail".into(),
                key_id: "k".into(),
                key_secret: "s".into(),
                region: "r".into(),
            },
            worker: "http://127.0.0.1:1".into(),
            box_name: "b".into(),
            sign: "/nonexistent".into(),
            doh: "http://127.0.0.1:1".into(),
            counts: Default::default(),
            sent_bytes: Default::default(),
            visitors: Default::default(),
            clock: std::sync::Mutex::new(0),
        }
    }

    #[test]
    fn ids_and_kinds() {
        assert!(valid_id("in", "0mvd7o78x0n2r1k185a2v"));
        assert!(valid_id("state", "dev1-00042"));
        assert!(!valid_id("in", "../x"));
        assert!(!valid_id("in", ""));
        assert!(!valid_id("secrets", "a"));
        assert!(!valid_id("in", &"a".repeat(65)));
    }

    #[test]
    fn only_the_internet() {
        for ok in [
            "142.251.167.27",
            "2607:f8b0:4004:c1b::1a",
            "2002:8efb:a71b::1",
            "64:ff9b::8efb:a71b",
        ] {
            assert!(public(ok.parse().unwrap()), "{ok}");
        }
        for no in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.20",
            "172.16.0.1",
            "169.254.1.1",
            "100.64.0.2",
            "0.0.0.0",
            "::1",
            "fd7a:115c:a1e0::4",
            "fe80::1",
            "::ffff:192.168.1.1",
            "224.0.0.1",
            "198.18.0.1",
            "2002:c0a8:0114::1",
            "64:ff9b::c0a8:114",
            "::192.168.1.20",
            "2001:db8::1",
            "2001:0:4136:e378::1",
            "fec0::1",
        ] {
            assert!(!public(no.parse().unwrap()), "{no}");
        }
    }

    #[test]
    fn hostnames() {
        assert!(hostname_ok("gmail-smtp-in.l.google.com"));
        assert!(!hostname_ok("localhost"));
        assert!(!hostname_ok("a..b"));
        assert!(!hostname_ok("-x.com"));
        assert!(!hostname_ok("x.com:25"));
        assert!(!hostname_ok("10.0.0.1/8"));
    }

    #[test]
    fn spending_stops_at_the_hour_s_share() {
        let m = Mail {
            s3: library_gate::Gate {
                endpoint: "http://x".into(),
                bucket: "mail".into(),
                key_id: "k".into(),
                key_secret: "s".into(),
                region: "r".into(),
            },
            worker: "http://w".into(),
            box_name: "b".into(),
            sign: "/nonexistent".into(),
            doh: "http://d".into(),
            counts: Default::default(),
            sent_bytes: Default::default(),
            visitors: Default::default(),
            clock: std::sync::Mutex::new(0),
        };
        assert!((0..3).all(|_| m.spend("a", "pipe", 3)));
        assert!(!m.spend("a", "pipe", 3));
        assert!(m.spend("b", "pipe", 3));
        let (t1, t2) = (m.tick(), m.tick());
        assert!(t2 > t1);
    }
}
