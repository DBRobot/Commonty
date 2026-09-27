//! dd-transcode: a box doing compute on one file for one viewer.
//!
//! The rule of the fleet is ciphertext at rest, plaintext only in memory
//! while working. A member's device asks this box to play something its
//! own player cannot: it sends a url for the sealed file (which it got
//! from the gate with its own token, good for minutes; this box holds no
//! key to the bucket) and the library's data key sealed to this box's
//! ephemeral key. The file is rclone's crypt format: a header and 64 KiB
//! blocks. The box fetches and decrypts blocks in memory as ffmpeg asks
//! for them (containers keep their index at the end, so ffmpeg must seek:
//! the plain bytes are served to it over a second listener on localhost,
//! with ranges, that nothing else reaches), and serves the HLS it makes
//! from a runtime directory that is wiped when the session ends. The
//! names' key is never here; the data key lives as long as the session
//! and is zeroed with it.
//!
//!   GET  /key                    the box's current public key to seal to
//!   POST /session                { url, key: sealed data key, size: sealed bytes,
//!                                  from: seconds in to start at (resuming) }
//!                                -> { id, playlist, from, duration: seconds or null }
//!   GET  /session/{id}/{file}    the playlist and its segments
//!   DELETE /session/{id}         over, wiped
//!
//!   GET  /plain/{id}             (the ffmpeg listener only) the file

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use tokio::sync::Mutex;

/// a session nobody has fetched from in this long is over
const IDLE: Duration = Duration::from_secs(600);
/// decrypted pieces kept per session, for ffmpeg's seeks back
const KEEP: usize = 64;
/// blocks fetched from the bucket in one request: 64 of 64 KiB, a range
const PIECE: u64 = 64;
/// ffmpegs at once: each takes cores for as long as its film plays, and
/// anyone who can reach start could otherwise ask for a hundred
const MAX_SESSIONS: usize = 4;

use library::crypt::{BLOCK, HEADER, SEALED_BLOCK};

struct Session {
    dir: PathBuf,
    last: Mutex<Instant>,
    child: Mutex<Option<tokio::process::Child>>,
    url: String,
    dec: library::crypt::Decrypter,
    /// plain bytes
    size: u64,
    cache: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
}

impl Session {
    /// piece p (PIECE blocks from block p*PIECE), plain, from the cache or
    /// the bucket by range
    async fn piece(&self, http: &reqwest::Client, p: u64) -> Result<Arc<Vec<u8>>> {
        if let Some(c) = self.cache.lock().await.get(&p) {
            return Ok(c.clone());
        }
        let first = p * PIECE;
        let from = HEADER as u64 + first * SEALED_BLOCK as u64;
        let to = from + PIECE * SEALED_BLOCK as u64 - 1;
        let sealed = http
            .get(&self.url)
            .header("range", format!("bytes={from}-{to}"))
            .send()
            .await?
            .error_for_status()?;
        let sealed = capped(sealed, (PIECE * SEALED_BLOCK as u64) as usize).await?;
        let mut plain = Vec::with_capacity(sealed.len());
        for (i, b) in sealed.chunks(SEALED_BLOCK).enumerate() {
            plain.extend(
                self.dec
                    .block(first + i as u64, b)
                    .context("block does not open")?,
            );
        }
        let plain = Arc::new(plain);
        let mut cache = self.cache.lock().await;
        if cache.len() >= KEEP {
            // the furthest from this one goes; ffmpeg mostly moves forward
            let far = cache.keys().copied().max_by_key(|k| k.abs_diff(p));
            if let Some(k) = far {
                cache.remove(&k);
            }
        }
        cache.insert(p, plain.clone());
        Ok(plain)
    }
}

struct App {
    public: String,
    secret: zeroize::Zeroizing<[u8; 32]>,
    root: PathBuf,
    ffmpeg: PathBuf,
    plain: std::net::SocketAddr,
    http: reqwest::Client,
    /// the only url prefix a session may fetch from
    source: String,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

#[derive(Deserialize)]
struct Start {
    /// where the sealed file is, for as long as a film plays, ranges allowed
    url: String,
    /// the library's data key, sealed to this box's public key
    key: String,
    /// the sealed file's size in bytes
    size: u64,
    /// where in the film to begin, in seconds: a viewer picking up where
    /// they stopped. The playlist starts there; the page adds it back.
    #[serde(default)]
    from: f64,
}

/// the containers ffmpeg and ffprobe may open, and nothing else
const FORMATS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,avi,mpegts,mpeg,flv,asf,ogg";

async fn key(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "key": app.public }))
}

async fn start(State(app): State<Arc<App>>, Json(s): Json<Start>) -> Response {
    if app.sessions.lock().await.len() >= MAX_SESSIONS {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "this box is transcoding all it can; try again when a film ends",
        )
            .into_response();
    }
    let data_key = match library::open_x25519(&app.secret, &s.key) {
        Ok(k) => k,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "the key is not sealed to this box").into_response();
        }
    };
    // The member sends this url. Anything but the libraries bucket is this
    // box fetching wherever it is pointed, from inside the network - so
    // it is the bucket's own prefix or nothing, and redirects are not
    // followed (the client is built without them).
    if app.source.is_empty() || !s.url.starts_with(&app.source) {
        return (StatusCode::BAD_REQUEST, "not a library file").into_response();
    }
    let plain_size = match library::crypt::plain_size(s.size) {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "not a sealed file's size").into_response(),
    };
    // the header first: the file's nonce, from which every block's follows
    let header = match app
        .http
        .get(&s.url)
        .header("range", format!("bytes=0-{}", HEADER - 1))
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => match capped(r, HEADER).await {
            Ok(b) => b,
            Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
        },
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    let dec = match library::crypt::Cipher::decrypter_with(&data_key, &header) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "not an encrypted file").into_response(),
    };
    let id = library::random_id();
    let dir = app.root.join(&id);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let session = Arc::new(Session {
        dir: dir.clone(),
        last: Mutex::new(Instant::now()),
        child: Mutex::new(None),
        url: s.url,
        dec,
        size: plain_size,
        cache: Mutex::new(HashMap::new()),
    });
    app.sessions
        .lock()
        .await
        .insert(id.clone(), session.clone());
    let input = format!("http://{}/plain/{id}", app.plain);
    // how long the film is, for the page's "left" and its progress: read
    // from the container, which is quick; a file that will not say plays
    // all the same
    let duration = probe(&app.ffmpeg.with_file_name("ffprobe"), &input).await;
    // resuming past the end would make nothing; start again instead
    let from = if s.from.is_finite() && s.from > 0.0 && duration.is_none_or(|d| s.from < d - 5.0) {
        s.from
    } else {
        0.0
    };
    // ffmpeg reads the plain file from this process, seeking as it likes,
    // and writes HLS into the session directory; the first segments appear
    // within seconds, the playlist grows as it goes
    let child = match tokio::process::Command::new(&app.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            // ffmpeg will happily open whatever a file asks it to. The
            // input is a member's own media, decrypted here, and it goes
            // through every demuxer ffmpeg has: the only things it may
            // reach are the containers we expect, over the one protocol
            // this process serves them on.
            "-protocol_whitelist",
            "http,tcp",
            "-format_whitelist",
            FORMATS,
        ])
        .args(if from > 0.0 {
            vec!["-ss".to_string(), format!("{from:.3}")]
        } else {
            vec![]
        })
        .args([
            "-i",
            &input,
            // the picture and the sound: a film's subtitle tracks made a
            // file of their own for every segment, and nothing reads them
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-sn",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-f",
            "hls",
            "-hls_time",
            "4",
            "-hls_playlist_type",
            "event",
            "-hls_segment_filename",
        ])
        .arg(dir.join("%05d.ts"))
        .arg(dir.join("index.m3u8"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            if let Some(s) = app.sessions.lock().await.remove(&id) {
                wipe(s).await;
            }
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ffmpeg: {e}")).into_response();
        }
    };
    // Answered once there is something to play: a page that asked for the
    // playlist the moment the session began found none yet, and its player
    // gave up on the 404. ffmpeg ending first is its error, not a session.
    let mut child = child;
    let playlist = dir.join("index.m3u8");
    let mut ready = false;
    for _ in 0..150 {
        if std::fs::read_to_string(&playlist).is_ok_and(|p| p.contains(".ts")) {
            ready = true;
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            if let Some(s) = app.sessions.lock().await.remove(&id) {
                wipe(s).await;
            }
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "ffmpeg stopped before the first segment ({status}): not a film it can read"
                ),
            )
                .into_response();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    *session.child.lock().await = Some(child);
    if !ready {
        if let Some(s) = app.sessions.lock().await.remove(&id) {
            wipe(s).await;
        }
        return (
            StatusCode::GATEWAY_TIMEOUT,
            "no first segment in thirty seconds: the box is too busy, or the file is too slow to read",
        )
            .into_response();
    }
    Json(serde_json::json!({
        "id": id,
        "playlist": format!("/session/{id}/index.m3u8"),
        "from": from,
        "duration": duration,
    }))
    .into_response()
}

/// the film's length in seconds, if its container says within a few seconds
async fn probe(ffprobe: &std::path::Path, input: &str) -> Option<f64> {
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "http,tcp",
                "-format_whitelist",
                FORMATS,
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
                input,
            ])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let d: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    (d.is_finite() && d > 0.0).then_some(d)
}

/// the plain bytes of a session's file for ffmpeg: whole, or a range
async fn plain(
    State(app): State<Arc<App>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !same_user(peer) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(sess) = app.sessions.lock().await.get(&id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let size = sess.size;
    let range = headers
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            let start: u64 = a.parse().ok()?;
            let end: u64 = if b.is_empty() {
                size.saturating_sub(1)
            } else {
                b.parse().ok()?
            };
            Some((start, end.min(size.saturating_sub(1))))
        });
    let (start, end) = match range {
        Some((s, e)) if s <= e && s < size => (s, e),
        Some(_) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [("content-range", format!("bytes */{size}"))],
            )
                .into_response();
        }
        None if size == 0 => {
            return (StatusCode::OK, [("accept-ranges", "bytes")], Vec::new()).into_response();
        }
        None => (0, size - 1),
    };
    let http = app.http.clone();
    let stream = futures::stream::try_unfold((sess, start), move |(sess, at)| {
        let http = http.clone();
        async move {
            if at > end {
                return Ok::<_, anyhow::Error>(None);
            }
            let piece_bytes = PIECE * BLOCK as u64;
            let p = at / piece_bytes;
            let plain = sess.piece(&http, p).await?;
            let from = (at - p * piece_bytes) as usize;
            let to = ((end + 1 - p * piece_bytes) as usize).min(plain.len());
            if from >= to {
                return Ok(None);
            }
            let piece = bytes::Bytes::copy_from_slice(&plain[from..to]);
            let next = at + (to - from) as u64;
            Ok(Some((piece, (sess, next))))
        }
    });
    let len = end + 1 - start;
    let status = if range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut r = Response::builder()
        .status(status)
        .header("accept-ranges", "bytes")
        .header("content-length", len)
        .header("content-type", "application/octet-stream");
    if range.is_some() {
        r = r.header("content-range", format!("bytes {start}-{end}/{size}"));
    }
    r.body(Body::from_stream(stream))
        .unwrap_or_else(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
}

async fn serve(State(app): State<Arc<App>>, Path((id, file)): Path<(String, String)>) -> Response {
    if !file.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(sess) = app.sessions.lock().await.get(&id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    *sess.last.lock().await = Instant::now();
    let dir = sess.dir.clone();
    let ty = if file.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "video/mp2t"
    };
    match tokio::fs::read(dir.join(&file)).await {
        Ok(b) => ([("content-type", ty), ("cache-control", "no-store")], b).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn end(State(app): State<Arc<App>>, Path(id): Path<String>) -> StatusCode {
    match app.sessions.lock().await.remove(&id) {
        Some(s) => {
            wipe(s).await;
            StatusCode::NO_CONTENT
        }
        None => StatusCode::NOT_FOUND,
    }
}

async fn wipe(s: Arc<Session>) {
    if let Some(mut c) = s.child.lock().await.take() {
        let _ = c.kill().await;
    }
    let _ = tokio::fs::remove_dir_all(&s.dir).await;
}

/// A response, read to at most `cap` bytes: what was asked for and no more.
async fn capped(mut r: reqwest::Response, cap: usize) -> Result<Vec<u8>> {
    if r.content_length().is_some_and(|n| n > cap as u64) {
        anyhow::bail!("the bucket answered with more than was asked for");
    }
    let mut out = Vec::with_capacity(cap.min(1 << 20));
    while let Some(c) = r.chunk().await? {
        if out.len() + c.len() > cap {
            anyhow::bail!("the bucket answered with more than was asked for");
        }
        out.extend_from_slice(&c);
    }
    Ok(out)
}

/// Is the other end of this loopback connection a process of our own user?
///
/// The session id is the only thing guarding a session's plaintext, and it
/// is on ffmpeg's command line, which every process on the box can read
/// through /proc. So the listener that serves plaintext asks the kernel who
/// opened the connection: /proc/net/tcp lists each socket with its owner,
/// and ffmpeg's is ours - it runs as this service's user. Anyone else with
/// the id in hand is still someone else.
fn same_user(peer: std::net::SocketAddr) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let std::net::SocketAddr::V4(p) = peer else {
        return false;
    };
    let Ok(me) = std::fs::metadata("/proc/self").map(|m| m.uid()) else {
        return false;
    };
    let want = format!(
        "{:08X}:{:04X}",
        u32::from_le_bytes(p.ip().octets()),
        p.port()
    );
    let Ok(table) = std::fs::read_to_string("/proc/net/tcp") else {
        return false;
    };
    table.lines().skip(1).any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.get(1) == Some(&want.as_str()) && f.get(7).and_then(|u| u.parse::<u32>().ok()) == Some(me)
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let bind: std::net::SocketAddr = std::env::var("TRANSCODE_BIND")
        .unwrap_or_else(|_| "127.0.0.1:4190".into())
        .parse()
        .context("TRANSCODE_BIND")?;
    let root = PathBuf::from(
        std::env::var("TRANSCODE_DIR").unwrap_or_else(|_| "/run/dd-transcode".into()),
    );
    let ffmpeg =
        PathBuf::from(std::env::var("TRANSCODE_FFMPEG").unwrap_or_else(|_| "ffmpeg".into()));
    // the listener ffmpeg reads plaintext from: localhost, never proxied
    let plain_bind: std::net::SocketAddr = std::env::var("TRANSCODE_PLAIN_BIND")
        .unwrap_or_else(|_| "127.0.0.1:4191".into())
        .parse()
        .context("TRANSCODE_PLAIN_BIND")?;
    std::fs::create_dir_all(&root)?;
    // a fresh key every start: a session outlives nothing
    let (public, secret) = library::ephemeral();
    let app = Arc::new(App {
        public,
        secret,
        root,
        ffmpeg,
        plain: plain_bind,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(60))
            .build()?,
        source: std::env::var("TRANSCODE_SOURCE").unwrap_or_default(),
        sessions: Mutex::new(HashMap::new()),
    });
    let sweeper = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let mut s = sweeper.sessions.lock().await;
            let mut gone = Vec::new();
            for (k, v) in s.iter() {
                if v.last.lock().await.elapsed() > IDLE {
                    gone.push(k.clone());
                }
            }
            for id in gone {
                if let Some(sess) = s.remove(&id) {
                    wipe(sess).await;
                }
            }
        }
    });
    let router = axum::Router::new()
        .route("/key", get(key))
        .route("/session", post(start))
        .route("/session/{id}/{file}", get(serve))
        .route("/session/{id}", axum::routing::delete(end))
        .with_state(app.clone());
    let inner = axum::Router::new()
        .route("/plain/{id}", get(plain))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let plain_listener = tokio::net::TcpListener::bind(plain_bind).await?;
    eprintln!("dd-transcode on {bind}, plain for ffmpeg on {plain_bind}");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(
            plain_listener,
            inner.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        {
            eprintln!("plain listener: {e}");
        }
    });
    axum::serve(listener, router).await?;
    Ok(())
}
