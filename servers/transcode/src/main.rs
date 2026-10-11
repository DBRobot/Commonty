//! dd-transcode: a box doing compute on one file for one viewer.
//!
//! The rule of the fleet is ciphertext at rest, plaintext only in memory
//! while working. A member's device asks this box to play something its
//! own player cannot: it sends a url for the sealed file (which it got
//! from the gate with its own token, good for minutes; this box holds no
//! key to the bucket) and that file's block keys sealed to this box's
//! ephemeral key: one 32-byte key per block, which open that file and
//! nothing else (client/library/src/crypt.rs, BlockOpener). The library's
//! data key never comes here. The file is rclone's crypt format: a header
//! and 64 KiB blocks. The box fetches and decrypts blocks in memory as
//! ffmpeg asks for them (containers keep their index at the end, so ffmpeg
//! must seek: the plain bytes are served to it over a second listener on
//! localhost, with ranges, that nothing else reaches), and serves the HLS
//! it makes from a runtime directory that is wiped when the session ends.
//! The block keys live as long as the session and are zeroed with it.
//!
//!   GET  /key                    the box's current public key to seal to
//!   POST /session                { url, keys: sealed block keys, size: sealed bytes,
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
use axum::extract::{DefaultBodyLimit, Path, State};
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
/// Segments (4 s each) kept behind the furthest one the player asked for,
/// and made ahead of it before ffmpeg is paused: six minutes each way. The
/// segments are plaintext, so they live in memory only, and a whole film
/// would not fit; a jump outside the window starts a new session there.
const BEHIND: u64 = 90;
const AHEAD: u64 = 90;
/// And at most this many bytes of them per session, whatever the minutes:
/// a copied picture is the file's own bitrate, and six minutes each way of
/// a 4K remux is gigabytes. Half of it ahead of the player, the rest
/// behind. With KEEP's 256 MiB, four sessions hold 2.5 GiB, which leaves
/// the unit's 4G MemoryMax room for four ffmpegs (transcode.nix).
const WINDOW_BYTES: u64 = 384 << 20;
/// A start's body: the block keys of a file up to MAX_BLOCK_KEYS (64 MiB,
/// 85 MiB in base64) and room for the rest. A 2 GB film's are 1 MiB.
const START_BODY: usize = 96 << 20;
/// the segment length asked of ffmpeg, in seconds
const SEGMENT_SECS: f64 = 4.0;
/// A copied picture can only be cut at the film's own keyframes, so its
/// segments are as long as they are far apart: Sintel's, 10.4 s apart,
/// made 11 MB segments, and Chrome on a small Android phone stopped on the
/// third. Copied only when they are this close; otherwise encoded, with a
/// keyframe every SEGMENT_SECS. That costs encoding (the GPU, or the cores)
/// for every film with sparse keyframes, which is most x264 encodes.
const COPY_KEYS_SECS: f64 = 5.0;
/// and only when a segment of it stays this small: a phone's player holds
/// about 20 MB (media.js), and a high-bitrate remux in 4 s pieces is more
const COPY_SEGMENT_BYTES: f64 = 6.0 * 1024.0 * 1024.0;
/// how much of the film is read for its keyframes: long enough to see
/// x264's usual 10.4 s between them, short enough to read in a moment
const KEYS_LOOK: f64 = 16.0;
/// ffmpeg's lines logged per session: a broken file can repeat one forever
const FFMPEG_LINES: usize = 40;

use library::crypt::{BLOCK, BLOCK_KEY, HEADER, SEALED_BLOCK};

struct Session {
    /// the start of its id, for the log: the whole id opens its segments
    short: String,
    /// the member who started it, as the gate named them to nginx
    owner: String,
    /// its place among MAX_SESSIONS, given back when it is wiped
    slot: std::sync::Mutex<Option<tokio::sync::OwnedSemaphorePermit>>,
    dir: PathBuf,
    last: Mutex<Instant>,
    child: Mutex<Option<tokio::process::Child>>,
    url: String,
    dec: library::crypt::BlockOpener,
    /// plain bytes
    size: u64,
    cache: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
    /// the furthest segment the player has asked for
    asked: std::sync::atomic::AtomicU64,
    /// ffmpeg stopped (SIGSTOP) because it is AHEAD of the player
    paused: std::sync::atomic::AtomicBool,
    /// the biggest finished segment seen, in bytes, for the log
    largest: std::sync::atomic::AtomicU64,
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
    /// the GPU's render node, when the box has one ffmpeg can encode on
    vaapi: Option<PathBuf>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// MAX_SESSIONS places, one taken before a start waits on anything, so
    /// starts at the same moment cannot all see room
    slots: Arc<tokio::sync::Semaphore>,
}

#[derive(Deserialize)]
struct Start {
    /// where the sealed file is, for as long as a film plays, ranges allowed
    url: String,
    /// the file's block keys, end to end, sealed to this box's public key
    keys: String,
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

async fn start(
    State(app): State<Arc<App>>,
    headers: axum::http::HeaderMap,
    Json(s): Json<Start>,
) -> Response {
    // One film at a time per member: starting another ends the one they
    // had going, so a person switching films never waits and nobody can
    // hold every slot on the box.
    let owner = headers
        .get("x-dd-user")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // Everyone watching the demo is the demo: each is one visitor of many,
    // so theirs are left to the box's own limit on films at once.
    if !owner.is_empty() && owner != "demo" {
        let theirs: Vec<Arc<Session>> = {
            let mut all = app.sessions.lock().await;
            let ids: Vec<String> = all
                .iter()
                .filter(|(_, x)| x.owner == owner)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| all.remove(id)).collect()
        };
        for old in theirs {
            wipe(old, "another film started").await;
        }
    }
    let Ok(slot) = app.slots.clone().try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "this box is transcoding all it can; try again when a film ends",
        )
            .into_response();
    };
    let keys = match library::open_x25519_bytes(&app.secret, &s.keys) {
        Ok(k) => k,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "the keys are not sealed to this box",
            )
                .into_response();
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
    // a key for every block, so ffmpeg never reads into one that will not open
    if keys.len() as u64 != plain_size.div_ceil(BLOCK as u64) * BLOCK_KEY as u64 {
        return (StatusCode::BAD_REQUEST, "not a key for every block").into_response();
    }
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
    let dec = match library::crypt::BlockOpener::new(&header, keys) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "not an encrypted file").into_response(),
    };
    let id = library::random_id();
    let dir = app.root.join(&id);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let short: String = id.chars().take(6).collect();
    let began = Instant::now();
    let session = Arc::new(Session {
        short: short.clone(),
        owner,
        slot: std::sync::Mutex::new(Some(slot)),
        dir: dir.clone(),
        last: Mutex::new(Instant::now()),
        asked: std::sync::atomic::AtomicU64::new(0),
        paused: std::sync::atomic::AtomicBool::new(false),
        largest: std::sync::atomic::AtomicU64::new(0),
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
    // and how far apart its keyframes are, at once: the window read is
    // counted from the container's start, which is near enough to 0
    let asked = if s.from.is_finite() && s.from > 0.0 {
        s.from
    } else {
        0.0
    };
    let ffprobe = app.ffmpeg.with_file_name("ffprobe");
    let (probed, keys) = tokio::join!(probe(&ffprobe, &input), read_keys(&ffprobe, &input, asked));
    let duration = probed.duration;
    let choice = choose(&probed, keys.as_ref());
    // resuming past the end would make nothing; start again instead
    let mut from = if asked > 0.0 && duration.is_none_or(|d| asked < d - 5.0) {
        asked
    } else {
        0.0
    };
    // A copied picture can only start on a keyframe, while the sound is cut
    // at the second asked for: every jump played seconds of picture with no
    // sound, and the page's clock ran that far ahead. Both start at the
    // keyframe instead, and the page is told that is where it began.
    if from > 0.0
        && choice.copy
        && let Some(k) = keyframe(&ffprobe, &input, probed.start, from).await
    {
        from = k;
    }
    // ffmpeg reads the plain file from this process, seeking as it likes,
    // and writes HLS into the session directory; the first segments appear
    // within seconds, the playlist grows as it goes
    // the chip only when there is encoding to do; a copy needs none
    let mut hw = app.vaapi.as_deref().filter(|_| !choice.copy);
    let run = |hw: Option<&std::path::Path>| {
        ffmpeg(&app.ffmpeg, &input, from, &probed, choice.copy, &dir, hw).map(|mut c| {
            log_ffmpeg(&mut c, &id, &short);
            c
        })
    };
    let child = match run(hw) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("transcode: {short}: ffmpeg would not start: {e}");
            if let Some(s) = app.sessions.lock().await.remove(&id) {
                wipe(s, "ffmpeg would not start").await;
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
            // a picture the chip would not take: the cores, as before
            if hw.take().is_some() {
                eprintln!("transcode: {short}: the GPU gave up ({status}), encoding on the cores");
                let _ = std::fs::remove_file(&playlist);
                if let Ok(c) = run(None) {
                    child = c;
                    continue;
                }
            }
            eprintln!("transcode: {short}: ffmpeg stopped before the first segment ({status})");
            if let Some(s) = app.sessions.lock().await.remove(&id) {
                wipe(s, "no first segment").await;
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
        eprintln!("transcode: {short}: no first segment in thirty seconds");
        if let Some(s) = app.sessions.lock().await.remove(&id) {
            wipe(s, "too slow to start").await;
        }
        return (
            StatusCode::GATEWAY_TIMEOUT,
            "no first segment in thirty seconds: the box is too busy, or the file is too slow to read",
        )
            .into_response();
    }
    // what it chose and why, for the next time a film will not play: no
    // name, no key, nothing of the file but its kind
    let mb = |b: f64| b / 1e6;
    eprintln!(
        "transcode: {short}: started at {from:.0}s in {:.1}s: picture {} ({}; {} {} {}x{}, keyframes {}, a copied segment ~{}), sound {} ({} {}ch)",
        began.elapsed().as_secs_f64(),
        match (choice.copy, hw.is_some()) {
            (true, _) => "copied",
            (false, true) => "encoded on the GPU",
            (false, false) => "encoded on the cores",
        },
        choice.why,
        probed.video,
        probed.pixels,
        probed.width,
        probed.height,
        keys.as_ref()
            .map_or("unread".into(), |k| format!("{:.1}s apart", k.apart)),
        choice
            .segment
            .map_or("unknown".into(), |b| format!("{:.1} MB", mb(b))),
        if probed.sound_as_is() {
            "copied"
        } else {
            "made stereo AAC"
        },
        probed.audio,
        probed.channels,
    );
    Json(serde_json::json!({
        "id": id,
        "playlist": format!("/session/{id}/index.m3u8"),
        "from": from,
        "duration": duration,
    }))
    .into_response()
}

/// ffmpeg for one session: the plain file in, HLS into `dir`. With `hw`,
/// what needs encoding is encoded on the box's GPU (VAAPI) instead of its
/// cores.
fn ffmpeg(
    bin: &std::path::Path,
    input: &str,
    from: f64,
    probed: &Probe,
    copy: bool,
    dir: &std::path::Path,
    hw: Option<&std::path::Path>,
) -> std::io::Result<tokio::process::Child> {
    let every = format!("expr:gte(t,n_forced*{SEGMENT_SECS})");
    tokio::process::Command::new(bin)
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
        // the Quick Sync path decodes on the chip where it can, and where
        // it cannot the frames go up to it; 10-bit comes down to 8 on the
        // way, since browsers play 8-bit H.264
        .args(match hw {
            Some(dev) => vec![
                "-hwaccel".to_string(),
                "vaapi".into(),
                "-hwaccel_output_format".into(),
                "vaapi".into(),
                "-vaapi_device".into(),
                dev.to_string_lossy().into_owned(),
            ],
            None => vec![],
        })
        .args(if from > 0.0 {
            vec!["-ss".to_string(), format!("{from:.3}")]
        } else {
            vec![]
        })
        .args([
            "-i", input,
            // the picture and the sound: a film's subtitle tracks made a
            // file of their own for every segment, and nothing reads them
            "-map", "0:v:0", "-map", "0:a:0?", "-sn",
        ])
        // Encoding a film as it plays took every core the box had, and on a
        // box busy with anything else fell behind the viewer: what browsers
        // play already is only repacked. What is encoded gets a keyframe
        // every four seconds, so the playlist's pieces are the size asked
        // (`choose`: a copy too, only when its own keyframes allow it).
        .args(if copy {
            vec!["-c:v", "copy"]
        } else if hw.is_some() {
            vec![
                "-vf",
                "format=nv12|vaapi,hwupload,scale_vaapi=format=nv12",
                "-c:v",
                "h264_vaapi",
                "-qp",
                "23",
                "-force_key_frames",
                &every,
            ]
        } else {
            vec![
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "23",
                "-force_key_frames",
                &every,
            ]
        })
        .args(if probed.sound_as_is() {
            vec!["-c:a", "copy"]
        } else {
            vec!["-c:a", "aac", "-ac", "2"]
        })
        .args(["-f", "hls", "-hls_time"])
        .arg(SEGMENT_SECS.to_string())
        .args(["-hls_playlist_type", "event", "-hls_segment_filename"])
        .arg(dir.join("%05d.ts"))
        .arg(dir.join("index.m3u8"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        // read by log_ffmpeg, into the journal with the session's name
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

/// ffmpeg's errors into the journal, each line marked with the session and
/// the session's whole id cut to its start (the plain url carries it)
fn log_ffmpeg(child: &mut tokio::process::Child, id: &str, short: &str) {
    use tokio::io::AsyncBufReadExt as _;
    let Some(err) = child.stderr.take() else {
        return;
    };
    let (id, short) = (id.to_string(), short.to_string());
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(err).lines();
        let mut n = 0;
        // read to the end even past the cap, so ffmpeg never blocks on a full pipe
        while let Ok(Some(l)) = lines.next_line().await {
            n += 1;
            if n <= FFMPEG_LINES {
                eprintln!("transcode: {short}: ffmpeg: {}", l.replace(&id, &short));
            } else if n == FFMPEG_LINES + 1 {
                eprintln!("transcode: {short}: ffmpeg: (more, not logged)");
            }
        }
    });
}

/// What the container says: how long the film is, and what its picture and
/// sound are, so the box does only the work a browser needs
#[derive(Default)]
struct Probe {
    /// the container's first timestamp, which a keyframe's is counted from
    start: f64,
    duration: Option<f64>,
    video: String,
    pixels: String,
    width: u64,
    height: u64,
    audio: String,
    channels: u64,
}

/// The picture's keyframes and bitrate over KEYS_LOOK seconds of it
struct Keys {
    /// the most seconds between two keyframes (or an edge of what was read)
    apart: f64,
    /// picture bytes a second
    rate: f64,
}

/// Whether the picture is copied, and why, with the bytes a copied segment
/// would be, when known
struct Choice {
    copy: bool,
    why: &'static str,
    segment: Option<f64>,
}

/// Copy only what browsers play, cut into segments small enough for a phone.
fn choose(p: &Probe, keys: Option<&Keys>) -> Choice {
    let encode = |why, segment| Choice {
        copy: false,
        why,
        segment,
    };
    if !p.picture_as_is() {
        return encode("not H.264 4:2:0", None);
    }
    // unread is treated as far apart: small segments are what matters
    let Some(k) = keys else {
        return encode("keyframes unread", None);
    };
    // hls cuts at the first keyframe past SEGMENT_SECS
    let secs = (SEGMENT_SECS / k.apart.max(0.04)).ceil().max(1.0) * k.apart;
    let segment = Some(k.rate * secs);
    if k.apart > COPY_KEYS_SECS {
        return encode("keyframes far apart", segment);
    }
    if k.rate * secs > COPY_SEGMENT_BYTES {
        return encode("segments too big", segment);
    }
    Choice {
        copy: true,
        why: "H.264 4:2:0, keyframes close",
        segment,
    }
}

/// Keys from ffprobe's packets, one "pts_time,dts_time,size,flags" a line
fn keys_from(csv: &str) -> Option<Keys> {
    let (mut first, mut last, mut bytes) = (f64::MAX, f64::MIN, 0u64);
    let mut keys = Vec::new();
    for l in csv.lines() {
        let f: Vec<&str> = l.trim().split(',').collect();
        let [pts, dts, size, flags, ..] = f[..] else {
            continue;
        };
        let Some(t) = pts
            .parse::<f64>()
            .ok()
            .or_else(|| dts.parse().ok())
            .filter(|t| t.is_finite())
        else {
            continue;
        };
        bytes += size.parse::<u64>().unwrap_or(0);
        first = first.min(t);
        last = last.max(t);
        if flags.contains('K') {
            keys.push(t);
        }
    }
    if keys.is_empty() || last - first < 1.0 {
        return None;
    }
    keys.sort_by(f64::total_cmp);
    // what was read past the last keyframe is at least that far apart
    let mut apart = (keys[0] - first).max(last - keys[keys.len() - 1]);
    for w in keys.windows(2) {
        apart = apart.max(w[1] - w[0]);
    }
    Some(Keys {
        apart,
        rate: bytes as f64 / (last - first),
    })
}

/// The picture's packets for KEYS_LOOK seconds from `from`: their flags and
/// sizes, from the container, nothing decoded.
async fn read_keys(ffprobe: &std::path::Path, input: &str, from: f64) -> Option<Keys> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "http,tcp",
                "-format_whitelist",
                FORMATS,
                "-read_intervals",
                &format!("{from:.3}%+{KEYS_LOOK}"),
                "-select_streams",
                "v:0",
                "-show_entries",
                "packet=pts_time,dts_time,size,flags",
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
    keys_from(&String::from_utf8_lossy(&out.stdout))
}

impl Probe {
    /// H.264 in 4:2:0 is what every browser plays: packed into the playlist
    /// as it is, no encoding - a second's work instead of all the cores
    fn picture_as_is(&self) -> bool {
        self.video == "h264" && matches!(self.pixels.as_str(), "yuv420p" | "yuvj420p")
    }
    /// stereo AAC plays everywhere; anything else (5.1, AC-3, DTS) is made so
    fn sound_as_is(&self) -> bool {
        self.audio == "aac" && self.channels <= 2
    }
}

/// The last keyframe at or before `from` seconds in, read from the index
/// around it: a twentieth of a second, not a scan of the film.
async fn keyframe(ffprobe: &std::path::Path, input: &str, start: f64, from: f64) -> Option<f64> {
    let at = start + from;
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "http,tcp",
                "-format_whitelist",
                FORMATS,
                "-read_intervals",
                &format!("{:.3}%{:.3}", (at - 20.0).max(start), at + 0.001),
                "-select_streams",
                "v:0",
                "-skip_frame",
                "nokey",
                "-show_entries",
                "frame=best_effort_timestamp_time",
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
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok())
        .filter(|k| *k <= at + 0.001)
        .reduce(f64::max)
        .map(|k| (k - start).max(0.0))
}

/// the film's length and streams, if its container says within a few seconds
async fn probe(ffprobe: &std::path::Path, input: &str) -> Probe {
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
                "format=duration,start_time:stream=codec_type,codec_name,pix_fmt,width,height,channels",
                "-of",
                "json",
                input,
            ])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(out)) = out else {
        return Probe::default();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Probe::default();
    };
    let first = |kind: &str| {
        v["streams"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["codec_type"] == kind))
            .cloned()
            .unwrap_or_default()
    };
    let (video, audio) = (first("video"), first("audio"));
    Probe {
        start: v["format"]["start_time"]
            .as_str()
            .and_then(|d| d.parse::<f64>().ok())
            .filter(|d| d.is_finite())
            .unwrap_or(0.0),
        duration: v["format"]["duration"]
            .as_str()
            .and_then(|d| d.parse::<f64>().ok())
            .filter(|d| d.is_finite() && *d > 0.0),
        video: video["codec_name"].as_str().unwrap_or_default().to_string(),
        pixels: video["pix_fmt"].as_str().unwrap_or_default().to_string(),
        width: video["width"].as_u64().unwrap_or(0),
        height: video["height"].as_u64().unwrap_or(0),
        audio: audio["codec_name"].as_str().unwrap_or_default().to_string(),
        channels: audio["channels"].as_u64().unwrap_or(0),
    }
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
    if let Some(n) = segment(&file) {
        sess.asked
            .fetch_max(n, std::sync::atomic::Ordering::Relaxed);
    }
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
            wipe(s, "closed").await;
            StatusCode::NO_CONTENT
        }
        None => StatusCode::NOT_FOUND,
    }
}

/// a segment's number from its name, "00042.ts"
fn segment(file: &str) -> Option<u64> {
    file.strip_suffix(".ts")?.parse().ok()
}

/// What to do about a session's window, from its segments (number,
/// bytes) and the furthest one the player asked for: the segments to
/// delete, and Some(true) to pause ffmpeg, Some(false) to let it go on.
/// Behind the player, more than BEHIND segments go, and the oldest go
/// while the window is past WINDOW_BYTES; ffmpeg waits while it is more
/// than AHEAD segments or half the bytes ahead, until it is within half
/// that again.
fn window(segments: &[(u64, u64)], asked: u64, paused: bool) -> (Vec<u64>, Option<bool>) {
    let mut segs = segments.to_vec();
    segs.sort();
    let made = segs.last().map(|s| s.0).unwrap_or(0);
    let ahead: u64 = segs.iter().filter(|s| s.0 >= asked).map(|s| s.1).sum();
    let mut total: u64 = segs.iter().map(|s| s.1).sum();
    let mut gone = Vec::new();
    for &(n, bytes) in segs.iter().filter(|s| s.0 < asked) {
        if n + BEHIND < asked || total > WINDOW_BYTES {
            gone.push(n);
            total -= bytes;
        }
    }
    let pause = made > asked + AHEAD || ahead > WINDOW_BYTES / 2;
    let resume = made <= asked + AHEAD / 2 && ahead <= WINDOW_BYTES / 4;
    let change = if pause && !paused {
        Some(true)
    } else if resume && paused {
        Some(false)
    } else {
        None
    };
    (gone, change)
}

/// a session's segments on disk: (number, bytes)
async fn segments(dir: &std::path::Path) -> Vec<(u64, u64)> {
    let mut segs = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name();
            let Some(n) = name.to_str().and_then(segment) else {
                continue;
            };
            let bytes = e.metadata().await.map(|m| m.len()).unwrap_or(0);
            segs.push((n, bytes));
        }
    }
    segs
}

/// how many segments are finished and the biggest of them: the newest is
/// still being written
fn finished(segs: &[(u64, u64)]) -> (usize, u64) {
    let newest = segs.iter().map(|s| s.0).max();
    let done = segs.iter().filter(|s| Some(s.0) != newest);
    (done.clone().count(), done.map(|s| s.1).max().unwrap_or(0))
}

/// a playlist's segments: how many, and the longest in seconds
fn extinf(playlist: &str) -> (usize, f64) {
    playlist
        .lines()
        .filter_map(|l| l.strip_prefix("#EXTINF:"))
        .filter_map(|l| l.split(',').next()?.trim().parse::<f64>().ok())
        .fold((0, 0.0), |(n, m), d| (n + 1, f64::max(m, d)))
}

/// Keep a session to its window (`window`).
async fn keep_window(s: &Session) {
    use std::sync::atomic::Ordering::Relaxed;
    let asked = s.asked.load(Relaxed);
    let segs = segments(&s.dir).await;
    s.largest.fetch_max(finished(&segs).1, Relaxed);
    let (gone, change) = window(&segs, asked, s.paused.load(Relaxed));
    for n in gone {
        let _ = tokio::fs::remove_file(s.dir.join(format!("{n:05}.ts"))).await;
    }
    let Some(pause) = change else { return };
    let Some(pid) = s.child.lock().await.as_ref().and_then(|c| c.id()) else {
        return;
    };
    let sig = if pause { "-STOP" } else { "-CONT" };
    if tokio::process::Command::new("kill")
        .args([sig, &pid.to_string()])
        .status()
        .await
        .is_ok_and(|st| st.success())
    {
        s.paused.store(pause, Relaxed);
    }
}

async fn wipe(s: Arc<Session>, why: &str) {
    if let Some(mut c) = s.child.lock().await.take() {
        let _ = c.kill().await;
    }
    // what the player was given, read before it goes
    let made = tokio::fs::read_to_string(s.dir.join("index.m3u8"))
        .await
        .unwrap_or_default();
    let (count, longest) = extinf(&made);
    let largest = s
        .largest
        .load(std::sync::atomic::Ordering::Relaxed)
        .max(finished(&segments(&s.dir).await).1);
    eprintln!(
        "transcode: {}: ended ({why}): {count} segments, longest {longest:.1}s, largest {:.1} MB",
        s.short,
        largest as f64 / 1e6
    );
    let _ = tokio::fs::remove_dir_all(&s.dir).await;
    // the place is free now, not when the last reader lets go of it
    s.slot.lock().unwrap().take();
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
        vaapi: std::env::var("TRANSCODE_VAAPI")
            .ok()
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
        sessions: Mutex::new(HashMap::new()),
        slots: Arc::new(tokio::sync::Semaphore::new(MAX_SESSIONS)),
    });
    let windows = app.clone();
    tokio::spawn(async move {
        loop {
            // often enough that a copy, which packs as fast as it reads,
            // goes little past the window between looks
            tokio::time::sleep(Duration::from_secs(1)).await;
            let all: Vec<Arc<Session>> = windows.sessions.lock().await.values().cloned().collect();
            for s in all {
                keep_window(&s).await;
            }
        }
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
                    wipe(sess, "idle").await;
                }
            }
        }
    });
    let router = axum::Router::new()
        .route("/key", get(key))
        .route(
            "/session",
            post(start).layer(DefaultBodyLimit::max(START_BODY)),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1 << 20;

    #[test]
    fn the_window_is_held_to_minutes() {
        // small segments: BEHIND and AHEAD decide
        let segs: Vec<(u64, u64)> = (0..=200).map(|n| (n, MB)).collect();
        let (gone, change) = window(&segs, 100, false);
        assert_eq!(gone, (0..10).collect::<Vec<_>>());
        assert_eq!(change, Some(true), "100 made ahead of the player");
    }

    #[test]
    fn the_window_is_held_to_bytes() {
        // a copied 4K picture: 30 MB a segment
        let segs: Vec<(u64, u64)> = (0..=40).map(|n| (n, 30 * MB)).collect();
        let (gone, change) = window(&segs, 30, false);
        // 41 segments are 1230 MB: the oldest behind go until it fits
        let kept: u64 = segs
            .iter()
            .filter(|s| !gone.contains(&s.0))
            .map(|s| s.1)
            .sum();
        assert!(kept <= WINDOW_BYTES, "{kept}");
        assert_eq!(gone.first(), Some(&0));
        assert!(
            gone.iter().all(|n| *n < 30),
            "nothing ahead of the player goes"
        );
        // 11 ahead (330 MB) is past half the window: ffmpeg waits
        assert_eq!(change, Some(true));
        // and goes on once the player has caught up
        let (_, change) = window(&segs[25..], 38, true);
        assert_eq!(change, Some(false));
    }

    fn h264() -> Probe {
        Probe {
            video: "h264".into(),
            pixels: "yuv420p".into(),
            ..Probe::default()
        }
    }

    /// packets as ffprobe lists them: a keyframe every `apart` seconds,
    /// `rate` bytes a second at 24 frames a second, for `secs`
    fn packets(apart: f64, rate: f64, secs: f64) -> String {
        let frames = (secs * 24.0) as u64;
        let every = (apart * 24.0).round() as u64;
        (0..frames)
            .map(|i| {
                let t = i as f64 / 24.0;
                let k = if i % every == 0 { "K__" } else { "___" };
                format!("{t:.6},{t:.6},{},{k}\n", (rate / 24.0) as u64)
            })
            .collect()
    }

    /// The demo's Sintel: keyframes 10.4 s apart at 8.6 Mbit/s made 11 MB
    /// segments, which a phone's player could not hold. It is encoded.
    #[test]
    fn sparse_keyframes_are_encoded() {
        let k = keys_from(&packets(10.417, 1.07e6, 16.0)).unwrap();
        assert!((k.apart - 10.417).abs() < 0.1, "{}", k.apart);
        let c = choose(&h264(), Some(&k));
        assert!(!c.copy);
        assert!(c.segment.unwrap() > 10e6);
    }

    #[test]
    fn close_keyframes_are_copied() {
        let k = keys_from(&packets(2.0, 0.6e6, 16.0)).unwrap();
        assert!((k.apart - 2.0).abs() < 0.1, "{}", k.apart);
        let c = choose(&h264(), Some(&k));
        assert!(c.copy, "{}", c.why);
        assert!(c.segment.unwrap() < COPY_SEGMENT_BYTES);
        // and 5 s apart is the most a copy takes
        assert!(choose(&h264(), keys_from(&packets(5.0, 0.6e6, 16.0)).as_ref()).copy);
        assert!(!choose(&h264(), keys_from(&packets(6.0, 0.6e6, 16.0)).as_ref()).copy);
    }

    /// a remux at 32 Mbit/s, keyframes close or not, is 16 MB a segment
    #[test]
    fn big_segments_are_encoded() {
        let k = keys_from(&packets(2.0, 4e6, 16.0)).unwrap();
        let c = choose(&h264(), Some(&k));
        assert!(!c.copy);
        assert_eq!(c.why, "segments too big");
    }

    #[test]
    fn unread_or_unplayable_is_encoded() {
        assert!(!choose(&h264(), None).copy);
        let k = keys_from(&packets(2.0, 0.6e6, 16.0)).unwrap();
        let hevc = Probe {
            video: "hevc".into(),
            ..h264()
        };
        assert!(!choose(&hevc, Some(&k)).copy);
        let ten_bit = Probe {
            pixels: "yuv420p10le".into(),
            ..h264()
        };
        assert!(!choose(&ten_bit, Some(&k)).copy);
    }

    #[test]
    fn keys_are_read_from_packets() {
        // one keyframe in all that was read: at least that long apart
        assert!(keys_from(&packets(100.0, 1e5, 16.0)).unwrap().apart > 15.0);
        // no keyframe, nothing read, or junk: unknown
        assert!(keys_from("0.0,0.0,100,___\n5.0,5.0,100,___\n").is_none());
        assert!(keys_from("").is_none());
        assert!(keys_from("N/A\nnonsense,,,\n").is_none());
        // a missing pts falls back on the dts
        let k = keys_from("N/A,0.0,1000,K__\nN/A,2.0,1000,K_\nN/A,4.0,1000,K_\n").unwrap();
        assert!((k.apart - 2.0).abs() < 1e-9);
        assert!((k.rate - 750.0).abs() < 1e-9);
    }

    #[test]
    fn the_log_reads_the_playlist() {
        let p = "#EXTM3U\n#EXT-X-TARGETDURATION:11\n#EXTINF:10.416667,\n00000.ts\n#EXTINF:4.000000,\n00001.ts\n";
        let (n, longest) = extinf(p);
        assert_eq!(n, 2);
        assert!((longest - 10.416667).abs() < 1e-6);
        // the one still being written is not counted as finished
        assert_eq!(finished(&[(0, 5), (1, 9), (2, 100)]), (2, 9));
    }

    #[test]
    fn ffmpeg_goes_on_only_when_well_within() {
        let segs: Vec<(u64, u64)> = (0..=60).map(|n| (n, MB)).collect();
        assert_eq!(window(&segs, 0, true).1, None);
        assert_eq!(window(&segs, 20, true).1, Some(false));
        assert_eq!(window(&segs, 20, false).1, None);
    }

    /// Starts at the same moment, each waiting on the bucket: only
    /// MAX_SESSIONS get a place, the rest are told at once.
    #[tokio::test]
    async fn starts_at_once_take_no_more_than_the_places() {
        let slow = axum::Router::new().fallback(|| async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            vec![0u8; HEADER]
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, slow).await });
        let (public, secret) = library::ephemeral();
        // one block's key: the size below is one block
        let keys = library::seal_to_x25519(&public, &[7u8; 32]).unwrap();
        let app = Arc::new(App {
            public,
            secret,
            root: std::env::temp_dir(),
            ffmpeg: PathBuf::from("/nonexistent/ffmpeg"),
            plain: "127.0.0.1:1".parse().unwrap(),
            http: reqwest::Client::new(),
            source: format!("http://{at}/"),
            vaapi: None,
            sessions: Mutex::new(HashMap::new()),
            slots: Arc::new(tokio::sync::Semaphore::new(MAX_SESSIONS)),
        });
        let starts: Vec<_> = (0..MAX_SESSIONS + 2)
            .map(|i| {
                let app = app.clone();
                let keys = keys.clone();
                tokio::spawn(async move {
                    let mut h = HeaderMap::new();
                    h.insert("x-dd-user", format!("m{i}").parse().unwrap());
                    let s = Start {
                        url: format!("http://{at}/f"),
                        keys,
                        size: library::crypt::sealed_size(1),
                        from: 0.0,
                    };
                    start(State(app), h, Json(s)).await.status()
                })
            })
            .collect();
        let mut busy = 0;
        for s in starts {
            if s.await.unwrap() == StatusCode::SERVICE_UNAVAILABLE {
                busy += 1;
            }
        }
        assert_eq!(busy, 2);
        // a header that does not open gave every place back
        assert_eq!(app.slots.available_permits(), MAX_SESSIONS);
    }

    /// Keys for fewer or more blocks than the file has are refused before
    /// anything is fetched.
    #[tokio::test]
    async fn a_start_needs_a_key_for_every_block() {
        let (public, secret) = library::ephemeral();
        let app = Arc::new(App {
            public: public.clone(),
            secret,
            root: std::env::temp_dir(),
            ffmpeg: PathBuf::from("/nonexistent/ffmpeg"),
            plain: "127.0.0.1:1".parse().unwrap(),
            http: reqwest::Client::new(),
            // nothing listens here: a fetch would be a 502, not a 400
            source: "http://127.0.0.1:1/".into(),
            vaapi: None,
            sessions: Mutex::new(HashMap::new()),
            slots: Arc::new(tokio::sync::Semaphore::new(MAX_SESSIONS)),
        });
        for (blocks, keys) in [(3u64, 2usize), (3, 4), (2, 0)] {
            let s = Start {
                url: "http://127.0.0.1:1/f".into(),
                keys: library::seal_to_x25519(&public, &vec![1u8; keys * BLOCK_KEY]).unwrap(),
                size: library::crypt::sealed_size(blocks * BLOCK as u64),
                from: 0.0,
            };
            let got = start(State(app.clone()), HeaderMap::new(), Json(s)).await;
            assert_eq!(got.status(), StatusCode::BAD_REQUEST, "{blocks} {keys}");
        }
        assert_eq!(app.slots.available_permits(), MAX_SESSIONS);
    }
}
