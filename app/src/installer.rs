//! Setting up a home controller, from a desktop: a window of its own
//! (labelled "installer", its page app/web/installer.html) that writes the
//! card, finds the Pi on the house network and pairs it with this account
//! (docs/dev/home-controller.md, Setting one up). Its page asks the routes
//! below, behind the launch secret like every /_dd/app/ route.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use auth::KeyStore as _;
use base64::Engine as _;
use hmac::Mac as _;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Manager as _, Runtime};

use crate::writer;

pub const LABEL: &str = "installer";
/// the Pi, on the house network, until it is paired
const PAIRING: &str = "http://home-controller.local:8080";
const PAIR_LABEL: &[u8] = b"commonty controller pair v1\0";

/// Where the writing stands, as the page draws it.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Progress {
    /// idle, manifest, download, write, key, written, failed
    pub stage: String,
    /// the writer's: hash, write, verify
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub bytes_per_sec: u64,
    pub eta: Option<u64>,
    pub version: String,
    pub error: Option<String>,
}

static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);
/// The card's pairing key, kept here alone until pairing has used it
static PAIR_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);
/// the disk last written, for writing the key again by itself
static WRITTEN: Mutex<Option<writer::Disk>> = Mutex::new(None);
/// the controller whose backup the new one starts from, by label
static RESTORE: Mutex<Option<String>> = Mutex::new(None);
/// where pairing stands: pair, passkey, download, upload, finish, done, failed
static PAIRING_AT: Mutex<Option<Progress>> = Mutex::new(None);

fn set(f: impl FnOnce(&mut Progress)) {
    if let Ok(mut p) = PROGRESS.lock() {
        f(p.get_or_insert_with(Progress::default));
    }
}

fn set_pairing(f: impl FnOnce(&mut Progress)) {
    if let Ok(mut p) = PAIRING_AT.lock() {
        f(p.get_or_insert_with(Progress::default));
    }
}

fn step(stage: &str) {
    set_pairing(|p| {
        *p = Progress {
            stage: stage.into(),
            ..Default::default()
        }
    });
}

fn json(v: &impl Serialize) -> Result<Response<Vec<u8>>, String> {
    Response::builder()
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(serde_json::to_vec(v).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn refuse(status: StatusCode, why: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(why.as_bytes().to_vec())
        .unwrap_or_default()
}

/// The installer's own routes, from site.rs: `path` under /_dd/app/installer
pub async fn route<R: Runtime>(
    app: &AppHandle<R>,
    path: &str,
    method: &str,
    body: &[u8],
) -> Result<Response<Vec<u8>>, String> {
    match (method, path) {
        ("POST", "/open") => open_window(app),
        ("GET", "/start") => start_state(app).await,
        ("GET", "/disks") => match writer::list(app).await {
            Ok(d) => json(&d),
            Err(e) => Ok(refuse(StatusCode::BAD_GATEWAY, &e)),
        },
        ("POST", "/write") => {
            #[derive(Deserialize)]
            struct W {
                device: String,
            }
            let w: W = serde_json::from_slice(body).map_err(|e| e.to_string())?;
            begin_write(app, w.device)
        }
        ("GET", "/progress") => json(
            &PROGRESS
                .lock()
                .map_err(|e| e.to_string())?
                .clone()
                .unwrap_or_default(),
        ),
        ("POST", "/key-only") => {
            #[derive(Deserialize)]
            struct W {
                device: String,
            }
            let w: W = serde_json::from_slice(body).map_err(|e| e.to_string())?;
            match key_only(app, &w.device).await {
                Ok(()) => json(&serde_json::json!({ "ok": true })),
                Err(e) => Ok(refuse(StatusCode::BAD_GATEWAY, &e)),
            }
        }
        ("POST", "/key") => match write_key().await {
            Ok(()) => json(&serde_json::json!({ "ok": true })),
            Err(e) => Ok(refuse(StatusCode::BAD_GATEWAY, &e)),
        },
        ("GET", "/find") => json(&find().await),
        ("POST", "/pair") => {
            *PAIRING_AT.lock().map_err(|e| e.to_string())? = Some(Progress {
                stage: "pair".into(),
                ..Default::default()
            });
            match pair(app).await {
                Ok(c) => {
                    step("done");
                    json(&c)
                }
                Err(e) => {
                    set_pairing(|p| {
                        p.stage = "failed".into();
                        p.error = Some(e.clone());
                    });
                    Ok(refuse(StatusCode::BAD_GATEWAY, &e))
                }
            }
        }
        ("GET", "/pair/progress") => json(
            &PAIRING_AT
                .lock()
                .map_err(|e| e.to_string())?
                .clone()
                .unwrap_or_default(),
        ),
        ("GET", "/restore") => match restorable(app).await {
            Ok(l) => json(&l),
            Err(e) => Ok(refuse(StatusCode::BAD_GATEWAY, &e)),
        },
        ("POST", "/restore") => {
            #[derive(Deserialize)]
            struct R {
                label: Option<String>,
            }
            let r: R = serde_json::from_slice(body).map_err(|e| e.to_string())?;
            *RESTORE.lock().map_err(|e| e.to_string())? = r.label;
            json(&serde_json::json!({ "ok": true }))
        }
        ("POST", "/done") => done(app, body).await,
        _ => Ok(refuse(StatusCode::NOT_FOUND, "no such step")),
    }
}

/// The installer's window: its own, and only the installer in it.
fn open_window<R: Runtime>(app: &AppHandle<R>) -> Result<Response<Vec<u8>>, String> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.set_focus();
        return json(&serde_json::json!({ "ok": true }));
    }
    let at = tauri::Url::parse(&format!("{}/_dd/app/installer", crate::site::origin()))
        .map_err(|e| e.to_string())?;
    let url = if at.scheme() == "http" {
        tauri::WebviewUrl::External(at)
    } else {
        tauri::WebviewUrl::CustomProtocol(at)
    };
    tauri::WebviewWindowBuilder::new(app, LABEL, url)
        .title("Set up a home controller")
        .inner_size(760.0, 560.0)
        .min_inner_size(560.0, 480.0)
        .build()
        .map_err(|e| e.to_string())?;
    json(&serde_json::json!({ "ok": true }))
}

/// Before anything is written: where the account's root is (here, a
/// passkey, or a key on another device, which cannot sign from here),
/// what the domain is, and whether this app holds a pairing key.
async fn start_state<R: Runtime>(app: &AppHandle<R>) -> Result<Response<Vec<u8>>, String> {
    let keys = app.state::<crate::account::Keys>();
    let here = account::load_root(&keys.0)
        .map_err(|e| e.to_string())?
        .is_some();
    let root = if here {
        Some("here")
    } else {
        match own_entry(app).await {
            Ok(e) if e.entry.root.starts_with(identity::WEBAUTHN_ROOT) => Some("passkey"),
            Ok(_) => Some("elsewhere"),
            // not known yet; pairing says so if it comes to that
            Err(_) => None,
        }
    };
    json(&serde_json::json!({
        "root": root,
        "domain": crate::account::domain(),
        "written": WRITTEN.lock().map_err(|e| e.to_string())?.is_some(),
        "keyed": PAIR_KEY.lock().map_err(|e| e.to_string())?.is_some(),
    }))
}

async fn own_entry<R: Runtime>(app: &AppHandle<R>) -> Result<identity::SignedEntry, String> {
    let keys = app.state::<crate::account::Keys>();
    let user = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    directory::newest(&directory::fetch(&crate::account::dirs(), &user).await)
        .ok_or_else(|| "your entry could not be read".to_string())
}

fn begin_write<R: Runtime>(
    app: &AppHandle<R>,
    device: String,
) -> Result<Response<Vec<u8>>, String> {
    let busy = PROGRESS
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .is_some_and(|p| matches!(p.stage.as_str(), "manifest" | "download" | "write" | "key"));
    if busy {
        return Ok(refuse(
            StatusCode::CONFLICT,
            "a card is being written already",
        ));
    }
    *PROGRESS.lock().map_err(|e| e.to_string())? = Some(Progress {
        stage: "manifest".into(),
        ..Default::default()
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = write_card(&app, &device).await {
            set(|p| {
                p.stage = "failed".into();
                p.error = Some(e);
            });
        }
    });
    Ok(Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Vec::new())
        .unwrap_or_default())
}

/// The card: the signed manifest, the image it names, the writer, the key.
async fn write_card<R: Runtime>(app: &AppHandle<R>, device: &str) -> Result<(), String> {
    let disk = writer::list(app)
        .await?
        .into_iter()
        .find(|d| d.id == device)
        .ok_or("that card is not in this computer any more")?;
    let (file, version) = manifest().await?;
    set(|p| {
        p.stage = "download".into();
        p.version = version.clone();
        p.total = 0;
        p.done = 0;
    });
    let image = download(&file).await?;
    set(|p| {
        p.stage = "write".into();
        p.done = 0;
        p.total = 0;
        p.bytes_per_sec = 0;
        p.eta = None;
    });
    writer::write(app, &image, device, &file.sha256, |l| {
        set(|p| {
            p.eta = writer::eta_secs(&l);
            p.phase = l.phase;
            p.done = l.done;
            p.total = l.total;
            p.bytes_per_sec = l.bytes_per_sec;
        })
    })
    .await?;
    set(|p| {
        p.stage = "key".into();
        p.phase = "key".into();
    });
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| e.to_string())?;
    *PAIR_KEY.lock().map_err(|e| e.to_string())? = Some(key);
    *WRITTEN.lock().map_err(|e| e.to_string())? = Some(disk);
    write_key().await?;
    set(|p| p.stage = "written".into());
    Ok(())
}

#[derive(Debug, Clone)]
struct File {
    name: String,
    url: String,
    sha256: String,
}

/// app.json from the releases branch, checked with the release key this
/// app carries; the card is the one download named for the controller
async fn manifest() -> Result<(File, String), String> {
    let key = option_env!("COMMONTY_RELEASE_KEY")
        .ok_or("this copy of the app carries no release key, so it cannot check a card")?;
    let key = identity::decode_public(key).map_err(|e| e.to_string())?;
    let repo = option_env!("COMMONTY_REPO")
        .ok_or("this copy of the app does not know where releases are")?;
    let url = format!(
        "https://git.{}/{repo}/raw/branch/releases/app.json",
        crate::account::domain()
    );
    let raw = directory::http()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("the release list did not come: {e}"))?
        .error_for_status()
        .map_err(|e| format!("the release list did not come: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let p = release::verify_doc(&raw, "app", &key)
        .map_err(|e| format!("the release list is not signed by the release key: {e}"))?;
    pick(&p)
}

/// the controller's card out of a checked manifest
fn pick(p: &serde_json::Value) -> Result<(File, String), String> {
    let files = p["files"]
        .as_object()
        .ok_or("the release lists no downloads")?;
    let (name, f) = files
        .iter()
        .find(|(n, _)| n.starts_with("commonty-home-controller-") && n.ends_with(".img.zst"))
        .ok_or("this release has no home controller card yet")?;
    let sha256 = f["sha256"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("the card's checksum is missing")?
        .to_ascii_lowercase();
    let url = f["url"]
        .as_str()
        .filter(|u| u.starts_with("https://"))
        .ok_or("the card's address is missing")?
        .to_string();
    let version = name
        .trim_start_matches("commonty-home-controller-")
        .trim_end_matches(".img.zst")
        .to_string();
    Ok((
        File {
            name: name.clone(),
            url,
            sha256,
        },
        version,
    ))
}

fn cache() -> PathBuf {
    crate::paths::data().join("cache").join("controller")
}

/// The image, in this app's cache: a copy from before when it is still the
/// one the manifest names, else downloaded again and checked as it comes.
async fn download(f: &File) -> Result<PathBuf, String> {
    use futures_util::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;
    let dir = cache();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let at = dir.join(&f.name);
    if hash_file(&at).await.as_deref() == Some(f.sha256.as_str()) {
        return Ok(at);
    }
    // older cards go: one is a gigabyte or so
    if let Ok(mut d) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(e)) = d.next_entry().await {
            let _ = tokio::fs::remove_file(e.path()).await;
        }
    }
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .get(&f.url)
        .send()
        .await
        .map_err(|e| format!("the card did not download: {e}"))?
        .error_for_status()
        .map_err(|e| format!("the card did not download: {e}"))?;
    let total = r.content_length().unwrap_or(0);
    set(|p| p.total = total);
    let part = dir.join(format!("{}.part", f.name));
    let mut out = tokio::fs::File::create(&part)
        .await
        .map_err(|e| e.to_string())?;
    let mut h = sha2::Sha256::new();
    let mut done = 0u64;
    let started = std::time::Instant::now();
    let mut stream = r.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("the card did not download: {e}"))?;
        h.update(&chunk);
        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
        let bps = (done as f64 / started.elapsed().as_secs_f64().max(0.001)) as u64;
        set(|p| {
            p.done = done;
            p.bytes_per_sec = bps;
            p.eta = writer::eta_secs(&writer::Line {
                phase: "download".into(),
                done,
                total,
                bytes_per_sec: bps,
            });
        });
    }
    out.flush().await.map_err(|e| e.to_string())?;
    drop(out);
    let got = hex(&h.finalize());
    if got != f.sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(
            "the card that downloaded is not the one the release signed; nothing was written"
                .into(),
        );
    }
    tokio::fs::rename(&part, &at)
        .await
        .map_err(|e| e.to_string())?;
    Ok(at)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn hash_file(p: &std::path::Path) -> Option<String> {
    let p = p.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let mut f = std::fs::File::open(p).ok()?;
        let mut h = sha2::Sha256::new();
        std::io::copy(&mut f, &mut h).ok()?;
        Some(hex(&h.finalize()))
    })
    .await
    .ok()
    .flatten()
}

/// A card written another way (Without a pairing key): no image, no admin
/// rights, only a new pairing key on its boot partition, as after a write.
async fn key_only<R: Runtime>(app: &AppHandle<R>, device: &str) -> Result<(), String> {
    let disk = writer::list(app)
        .await?
        .into_iter()
        .find(|d| d.id == device)
        .ok_or("that card is not in this computer any more")?;
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| e.to_string())?;
    *PAIR_KEY.lock().map_err(|e| e.to_string())? = Some(key);
    *WRITTEN.lock().map_err(|e| e.to_string())? = Some(disk);
    write_key().await
}

/// The key to the card's boot partition, as /boot/commonty/pair on the Pi
async fn write_key() -> Result<(), String> {
    let key = PAIR_KEY
        .lock()
        .map_err(|e| e.to_string())?
        .ok_or("no card was written here")?;
    let disk = WRITTEN
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("no card was written here")?;
    tauri::async_runtime::spawn_blocking(move || card::write_pair_key(&disk, &key))
        .await
        .map_err(|e| e.to_string())?
}

/// Whether the Pi answers on the house network yet, and how far along it is
async fn find() -> serde_json::Value {
    let asked = async {
        let r = lan()?
            .get(format!("{PAIRING}/pair/status"))
            .timeout(Duration::from_secs(4))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        r.json::<serde_json::Value>()
            .await
            .map_err(|e| e.to_string())
    };
    match asked.await {
        Ok(s) => serde_json::json!({ "found": true, "status": s }),
        Err(e) => serde_json::json!({ "found": false, "error": e }),
    }
}

/// the house network straight, never the fleet's: the Pi is not on it yet
fn lan() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

// What the door's pair.rs takes and gives. The mac is over the exact bytes
// of each body, sent and answered, in the x-commonty-mac header.
#[derive(Serialize)]
struct Pair<'a> {
    entry: &'a identity::SignedEntry,
    at: u64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Paired {
    controller: String,
    label: String,
}

#[derive(Serialize)]
struct Finish<'a> {
    network_key: &'a str,
    name_sig: &'a str,
    domain: &'a str,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Finished {
    name: String,
}

const MAC_HEADER: &str = "x-commonty-mac";

fn hmac(key: &[u8], body: &[u8]) -> Result<hmac::Hmac<sha2::Sha256>, String> {
    let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).map_err(|e| e.to_string())?;
    m.update(PAIR_LABEL);
    m.update(body);
    Ok(m)
}

fn mac(key: &[u8], body: &[u8]) -> Result<String, String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(hmac(key, body)?.finalize().into_bytes()))
}

fn check_mac(key: &[u8], body: &[u8], mac: &str) -> bool {
    let Ok(m) = base64::engine::general_purpose::STANDARD.decode(mac.trim()) else {
        return false;
    };
    hmac(key, body).is_ok_and(|h| h.verify_slice(&m).is_ok())
}

/// One pairing message to the door, and its answer, both under the mac:
/// an answer the card's key did not make is never read.
async fn sealed<T: serde::de::DeserializeOwned>(
    lan: &reqwest::Client,
    key: &[u8],
    path: &str,
    msg: &impl Serialize,
) -> Result<T, String> {
    let body = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    let r = lan
        .post(format!("{PAIRING}{path}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(MAC_HEADER, mac(key, &body)?)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("the home controller did not answer: {e}"))?;
    let status = r.status();
    let theirs = r
        .headers()
        .get(MAC_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = r.bytes().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "the home controller refused ({status}): {}",
            String::from_utf8_lossy(&bytes)
        ));
    }
    if !check_mac(key, &bytes, &theirs) {
        return Err("that answer is not from the card this app wrote".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn valid_label(label: &str) -> bool {
    label.len() == 16
        && label
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7'))
}

#[derive(Serialize)]
pub struct Done {
    pub label: String,
    pub name: String,
}

/// Pairing (docs/dev/home-controller.md, Pairing): the account's entry to
/// the door, its key and label back, the name signed by the account's root
/// (here, or by its passkey in the browser), a network key from the gate,
/// a backup put back if one was chosen, and the finish; then the controller
/// goes into this account's locked list, in place of the one restored.
async fn pair<R: Runtime>(app: &AppHandle<R>) -> Result<Done, String> {
    use ed25519_dalek::Signer as _;
    let keys = app.state::<crate::account::Keys>();
    let root = account::load_root(&keys.0).map_err(|e| e.to_string())?;
    let key = PAIR_KEY.lock().map_err(|e| e.to_string())?.ok_or(
        "this app holds no pairing key for that controller; add one to its card from here",
    )?;
    let entry = own_entry(app).await?;
    if root.is_none() && !entry.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        return Err(ROOT_ELSEWHERE.into());
    }
    // the backup first: nothing is paired if it cannot be had
    let from = RESTORE.lock().map_err(|e| e.to_string())?.clone();
    let backup = match from {
        Some(label) => Some(fetch_backup(app, &label).await?),
        None => None,
    };
    step("pair");
    let lan = lan()?;
    let at = identity::now();
    let ask = Pair { entry: &entry, at };
    let back: Paired = sealed(&lan, &key, "/pair", &ask).await?;
    let Paired { controller, label } = back;
    if !valid_label(&label) || identity::decode_public(&controller).is_err() {
        return Err("the home controller's answer is not well formed".into());
    }
    let domain = crate::account::domain();
    let name = format!("{label}.home.{domain}");
    let message = identity::controller_name_message(&name, &controller);
    let name_sig = match &root {
        Some(root) => {
            base64::engine::general_purpose::STANDARD.encode(root.sign(&message).to_bytes())
        }
        None => {
            step("passkey");
            passkey_sig(app, &entry, &label, &name, &controller).await?
        }
    };
    step("finish");
    // the network key: the gate's, for the member pairing it
    let base = media::gate::door_base(&crate::account::dirs()).map_err(|e| e.to_string())?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .post(format!("{base}/_dd/network/controller"))
        .bearer_auth(crate::site::token(app)?)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        let s = r.status();
        return Err(format!(
            "the box did not give the controller a way onto the network ({s}): {}",
            r.text().await.unwrap_or_default()
        ));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
    // sealed to the controller's key, which the mac'd /pair answer gave: the
    // house network carries it, and the mac alone would not keep it secret
    let network_key = library::seal_to(
        &controller,
        v["key"].as_str().ok_or("no network key")?.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    if let Some(b) = &backup {
        upload_backup(&lan, &key, &controller, b).await?;
        step("finish");
    }
    let fin = Finish {
        network_key: &network_key,
        name_sig: &name_sig,
        domain: &domain,
    };
    let fin: Finished = sealed(&lan, &key, "/pair/finish", &fin).await?;
    if fin.name != name {
        return Err(format!(
            "the home controller took the name {}, not {name}",
            fin.name
        ));
    }
    crate::controllers::replace(
        app,
        backup.as_ref().map(|b| b.label.as_str()),
        crate::controllers::Controller {
            label: label.clone(),
            name: name.clone(),
            controller,
        },
    )
    .await?;
    *PAIR_KEY.lock().map_err(|e| e.to_string())? = None;
    *WRITTEN.lock().map_err(|e| e.to_string())? = None;
    *RESTORE.lock().map_err(|e| e.to_string())? = None;
    Ok(Done { label, name })
}

pub const ROOT_ELSEWHERE: &str = "Your account's main key is on another device, so this app cannot sign your home controller's name. Set it up from the device that holds the main key.";

/// A passkey root signs the name in the system browser, at the gate's
/// page; the answer is collected from the gate and checked before it is
/// used (The root's word, either kind of root).
async fn passkey_sig<R: Runtime>(
    app: &AppHandle<R>,
    entry: &identity::SignedEntry,
    label: &str,
    name: &str,
    controller: &str,
) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt as _;
    let base = media::gate::door_base(&crate::account::dirs()).map_err(|e| e.to_string())?;
    let http = directory::http().map_err(|e| e.to_string())?;
    // the page signs only what this device asked for, with its own token
    let asked = http
        .put(format!("{base}/_dd/controller-name/{label}"))
        .bearer_auth(crate::site::token(app)?)
        .json(&serde_json::json!({ "controller": controller }))
        .send()
        .await
        .map_err(|e| format!("could not reach the box: {e}"))?;
    if !asked.status().is_success() {
        return Err(format!(
            "the box would not take the ask for a name ({})",
            asked.status()
        ));
    }
    app.opener()
        .open_url(format!("{base}/_dd/controller-name#{label}"), None::<&str>)
        .map_err(|e| format!("could not open the browser: {e}"))?;
    let origins = [base.clone()];
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    loop {
        if std::time::Instant::now() > deadline {
            return Err(
                "no answer from your passkey within ten minutes; pair again to ask once more"
                    .into(),
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let r = match http
            .get(format!("{base}/_dd/controller-name/{label}"))
            .bearer_auth(crate::site::token(app)?)
            .send()
            .await
        {
            Ok(r) => r,
            // the network comes and goes; ask again
            Err(_) => continue,
        };
        if matches!(
            r.status(),
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            return Err(format!(
                "the box would not give the passkey's answer ({})",
                r.status()
            ));
        }
        if !r.status().is_success() {
            continue;
        }
        let Ok(v) = r.json::<serde_json::Value>().await else {
            continue;
        };
        let Some(sig) = v["sig"].as_str() else {
            continue;
        };
        // a box could hand back anything: only the root's own word goes on
        identity::controller_name_signed(&entry.entry, name, controller, sig, &origins)
            .map_err(|_| "the answer the box gave is not your passkey's signature")?;
        return Ok(sig.to_string());
    }
}

#[derive(Serialize)]
struct Restorable {
    label: String,
    name: String,
    /// whether the gate holds a backup of it
    backup: bool,
}

fn files_base() -> Result<String, String> {
    media::gate::files_base(&crate::account::dirs()).map_err(|e| e.to_string())
}

/// where the gate says a controller's backup can be read: None, no backup
async fn backup_url<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<Option<String>, String> {
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .get(format!("{}/_dd/controller/backup/{label}", files_base()?))
        .bearer_auth(crate::site::token(app)?)
        .send()
        .await
        .map_err(|e| format!("the box did not answer: {e}"))?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !r.status().is_success() {
        return Err(format!("the box said {}", r.status()));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
    Ok(v["get"].as_str().map(str::to_string))
}

/// GET /restore: the member's controllers, and which have a backup
async fn restorable<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<Restorable>, String> {
    let all = crate::controllers::read(app).await?;
    let asked = futures_util::future::join_all(all.iter().map(|c| backup_url(app, &c.label))).await;
    Ok(all
        .into_iter()
        .zip(asked)
        .map(|(c, u)| Restorable {
            label: c.label,
            name: c.name,
            backup: matches!(u, Ok(Some(_))),
        })
        .collect())
}

/// a backup, downloaded, and the key it is locked with, opened here
struct Backup {
    label: String,
    file: Vec<u8>,
    key: library::Key,
}

async fn fetch_backup<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<Backup, String> {
    use futures_util::StreamExt as _;
    step("download");
    let get = backup_url(app, label)
        .await?
        .ok_or("the box holds no backup of that home controller")?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .get(&get)
        .send()
        .await
        .map_err(|e| format!("the backup did not download: {e}"))?
        .error_for_status()
        .map_err(|e| format!("the backup did not download: {e}"))?;
    let total = r.content_length().unwrap_or(0);
    let mut file = Vec::with_capacity(total as usize);
    let started = std::time::Instant::now();
    let mut stream = r.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("the backup did not download: {e}"))?;
        file.extend_from_slice(&chunk);
        moved("download", file.len() as u64, total, started);
    }
    let keys = app.state::<crate::account::Keys>();
    let kp = auth::device::load(&keys.0)
        .map_err(|e| e.to_string())?
        .ok_or("this device has no key")?;
    let public = auth::device::public_b64(&kp);
    let seed: [u8; 32] = kp.private().to_bytes()[..]
        .try_into()
        .map_err(|_| "the device key is not 32 bytes")?;
    let device = ed25519_dalek::SigningKey::from_bytes(&seed);
    let key = backup::open(&file, &public, &device)?;
    Ok(Backup {
        label: label.to_string(),
        file,
        key,
    })
}

fn moved(stage: &str, done: u64, total: u64, started: std::time::Instant) {
    let bps = (done as f64 / started.elapsed().as_secs_f64().max(0.001)) as u64;
    set_pairing(|p| {
        p.stage = stage.into();
        p.done = done;
        p.total = total;
        p.bytes_per_sec = bps;
        p.eta = writer::eta_secs(&writer::Line {
            phase: stage.into(),
            done,
            total,
            bytes_per_sec: bps,
        });
    });
}

/// POST :8080/pair/restore: the file under the pairing mac, and its key
/// sealed to the new controller's
async fn upload_backup(
    lan: &reqwest::Client,
    key: &[u8],
    controller: &str,
    b: &Backup,
) -> Result<(), String> {
    use futures_util::StreamExt as _;
    const CHUNK: usize = 256 * 1024;
    let total = b.file.len() as u64;
    let sealed_key = library::seal_to(controller, &b.key[..]).map_err(|e| e.to_string())?;
    let chunks: Vec<Vec<u8>> = b.file.chunks(CHUNK).map(<[u8]>::to_vec).collect();
    let started = std::time::Instant::now();
    moved("upload", 0, total, started);
    let mut done = 0u64;
    let body = futures_util::stream::iter(chunks).map(move |c| {
        done += c.len() as u64;
        moved("upload", done, total, started);
        Ok::<_, std::io::Error>(c)
    });
    let r = lan
        .post(format!("{PAIRING}/pair/restore"))
        .timeout(Duration::from_secs(30 * 60))
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::CONTENT_LENGTH, total)
        .header(MAC_HEADER, mac(key, &b.file)?)
        .header("x-commonty-backup-key", sealed_key)
        .body(reqwest::Body::wrap_stream(body))
        .send()
        .await
        .map_err(|e| format!("the home controller did not take the backup: {e}"))?;
    let status = r.status();
    let theirs = r
        .headers()
        .get(MAC_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = r.bytes().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "the home controller did not take the backup ({status}): {}",
            String::from_utf8_lossy(&bytes)
        ));
    }
    if !check_mac(key, &bytes, &theirs) {
        return Err("that answer is not from the controller being paired".into());
    }
    Ok(())
}

/// The backup file (Backups): "CBK1" | u32 BE header length | header JSON |
/// the sealed tar. Only the header is read here.
mod backup {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Header {
        #[allow(dead_code)]
        pub nonce: String,
        pub sealed: Vec<Sealed>,
    }

    #[derive(Deserialize)]
    pub struct Sealed {
        pub to: String,
        pub key: String,
    }

    pub fn header(file: &[u8]) -> Result<Header, String> {
        let bad = || "that is not a home controller backup".to_string();
        let rest = file.strip_prefix(b"CBK1").ok_or_else(bad)?;
        let len = u32::from_be_bytes(
            rest.get(..4)
                .ok_or_else(bad)?
                .try_into()
                .map_err(|_| bad())?,
        ) as usize;
        let h = rest.get(4..4 + len).ok_or_else(bad)?;
        serde_json::from_slice(h).map_err(|_| bad())
    }

    /// K, opened with this device's key, from the entry sealed to it
    pub fn open(
        file: &[u8],
        public: &str,
        device: &ed25519_dalek::SigningKey,
    ) -> Result<library::Key, String> {
        let h = header(file)?;
        let s = h
            .sealed
            .iter()
            .find(|s| s.to == public)
            .ok_or("that backup was made before this device was yours, so this device cannot open it; restore from a device that was")?;
        library::open_with(device, &s.key)
            .map_err(|_| "this device's key does not open that backup".to_string())
    }
}

/// The last step: Smart home in the app's main window, and this one closed.
async fn done<R: Runtime>(app: &AppHandle<R>, body: &[u8]) -> Result<Response<Vec<u8>>, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let name = v["name"].as_str().ok_or("which home controller?")?;
    let url = match crate::smarthome::url_for(app, name).await {
        Ok(u) => u,
        Err(e) => return Ok(refuse(StatusCode::BAD_GATEWAY, &e)),
    };
    if let Some(main) = app.get_webview_window("main") {
        let to = tauri::Url::parse(&url).map_err(|e| e.to_string())?;
        main.navigate(to).map_err(|e| e.to_string())?;
        let _ = main.set_focus();
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.close();
    }
    json(&serde_json::json!({ "ok": true }))
}

/// The card's boot partition, found and mounted the platform's way once the
/// writer has had the new table read.
mod card {
    use super::writer::Disk;

    pub fn write_pair_key(disk: &Disk, key: &[u8; 32]) -> Result<(), String> {
        let (mount, ours) = mount(disk)?;
        let dir = mount.join("commonty");
        let wrote = (|| {
            std::fs::create_dir_all(&dir)?;
            let f = dir.join("pair");
            std::fs::write(&f, key)?;
            std::fs::File::open(&f)?.sync_all()
        })()
        .map_err(|e| format!("the pairing key could not be written to the card: {e}"));
        unmount(disk, &mount, ours);
        wrote
    }

    #[cfg(target_os = "linux")]
    fn part_of(dev: &str) -> String {
        // mmcblk0, nvme0n1, loop0: p1; sdb: 1
        if dev.ends_with(|c: char| c.is_ascii_digit()) {
            format!("{dev}p1")
        } else {
            format!("{dev}1")
        }
    }

    /// where the kernel has a device mounted, from /proc/self/mountinfo
    #[cfg(target_os = "linux")]
    pub(super) fn mounted_at(mountinfo: &str, dev: &str) -> Option<std::path::PathBuf> {
        mountinfo.lines().find_map(|l| {
            let (left, right) = l.split_once(" - ")?;
            let source = right.split(' ').nth(1)?;
            (source == dev).then(|| {
                let at = left.split(' ').nth(4).unwrap_or_default();
                std::path::PathBuf::from(at.replace("\\040", " ").replace("\\011", "\t"))
            })
        })
    }

    #[cfg(target_os = "linux")]
    fn mount(disk: &Disk) -> Result<(std::path::PathBuf, bool), String> {
        let part = part_of(&disk.name);
        // the writer had the table read again; the node follows from udev
        for _ in 0..30 {
            if std::path::Path::new(&part).exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        // a desktop may mount it by itself, a moment later
        for _ in 0..6 {
            let info = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
            if let Some(at) = mounted_at(&info, &part) {
                return Ok((at, false));
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let out = std::process::Command::new("udisksctl")
            .args(["mount", "--no-user-interaction", "-b", &part])
            .output()
            .map_err(|e| {
                format!("the card's boot partition could not be opened (udisksctl): {e}")
            })?;
        let said = String::from_utf8_lossy(&out.stdout);
        // "Mounted /dev/sdb1 at /media/me/FIRMWARE"
        let at = said
            .split(" at ")
            .nth(1)
            .map(|s| s.trim().trim_end_matches('.'))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!(
                    "the card's boot partition could not be opened: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )
            })?;
        Ok((std::path::PathBuf::from(at), true))
    }

    #[cfg(target_os = "linux")]
    fn unmount(disk: &Disk, _at: &std::path::Path, _ours: bool) {
        // safe to take out: unmounted whoever mounted it
        let _ = std::process::Command::new("udisksctl")
            .args([
                "unmount",
                "--no-user-interaction",
                "-b",
                &part_of(&disk.name),
            ])
            .output();
    }

    #[cfg(target_os = "macos")]
    fn mount(disk: &Disk) -> Result<(std::path::PathBuf, bool), String> {
        let part = format!("/dev/{}s1", disk.id);
        let _ = std::process::Command::new("diskutil")
            .args(["mount", &part])
            .output();
        let out = std::process::Command::new("diskutil")
            .args(["info", &part])
            .output()
            .map_err(|e| e.to_string())?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.trim().strip_prefix("Mount Point:"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| (std::path::PathBuf::from(s), true))
            .ok_or_else(|| "the card's boot partition could not be opened".into())
    }

    #[cfg(target_os = "macos")]
    fn unmount(disk: &Disk, _at: &std::path::Path, _ours: bool) {
        let _ = std::process::Command::new("diskutil")
            .args(["unmountDisk", &format!("/dev/{}", disk.id)])
            .output();
    }

    #[cfg(windows)]
    fn mount(disk: &Disk) -> Result<(std::path::PathBuf, bool), String> {
        use std::os::windows::process::CommandExt as _;
        let n = disk
            .id
            .strip_prefix("PhysicalDrive")
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
            .ok_or("not a disk number")?;
        // Windows gives the new boot partition a letter by itself
        for _ in 0..40 {
            let out = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &format!("Get-Partition -DiskNumber {n} | Where-Object DriveLetter | Select-Object -First 1 -ExpandProperty DriveLetter"),
                ])
                .creation_flags(0x0800_0000)
                .output()
                .map_err(|e| e.to_string())?;
            let l = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if l.len() == 1 && l.bytes().all(|b| b.is_ascii_alphabetic()) {
                return Ok((std::path::PathBuf::from(format!("{l}:\\")), false));
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Err("Windows did not open the card's boot partition. Take the card out, put it back in, and press Try again.".into())
    }

    #[cfg(windows)]
    fn unmount(_disk: &Disk, _at: &std::path::Path, _ours: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_out_of_a_checked_manifest() {
        let p = serde_json::json!({
            "kind": "app",
            "files": {
                "commonty_0.1.30_amd64.deb": {"url": "https://x/a.deb", "sha256": "a".repeat(64)},
                "commonty-home-controller-0.1.30.img.zst": {"url": "https://x/c.img.zst", "sha256": "B".repeat(64)},
            }
        });
        let (f, v) = pick(&p).unwrap();
        assert_eq!(v, "0.1.30");
        assert_eq!(f.url, "https://x/c.img.zst");
        assert_eq!(f.sha256, "b".repeat(64));
        let none = serde_json::json!({"files": {"commonty-setup.exe": {"url": "https://x", "sha256": "a".repeat(64)}}});
        assert!(pick(&none).is_err());
        let plain = serde_json::json!({"files": {"commonty-home-controller-1.img.zst": {"url": "http://x", "sha256": "a".repeat(64)}}});
        assert!(pick(&plain).is_err());
    }

    #[test]
    fn the_mac_is_over_the_bytes_sent() {
        // the door's own (servers/smarthome/door/src/pair.rs): the label,
        // then the body exactly as it went, base64, in x-commonty-mac
        let key = [9u8; 32];
        let body = br#"{"label":"abcdefghijklmnop","controller":"Q09OVFJPTExFUg=="}"#;
        let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(&key).unwrap();
        m.update(b"commonty controller pair v1\0");
        m.update(body);
        let want = base64::engine::general_purpose::STANDARD.encode(m.finalize().into_bytes());
        assert_eq!(mac(&key, body).unwrap(), want);
        assert!(check_mac(&key, body, &want));
        assert!(!check_mac(&[8u8; 32], body, &want));
        // the same message in another key order is other bytes
        let other = br#"{"controller":"Q09OVFJPTExFUg==","label":"abcdefghijklmnop"}"#;
        assert!(!check_mac(&key, other, &want));
        assert!(!check_mac(&key, body, "not base64"));
        let p: Paired = serde_json::from_slice(body).unwrap();
        assert_eq!(p.label, "abcdefghijklmnop");
    }

    #[test]
    fn what_the_root_signs_for_the_name() {
        assert_eq!(
            identity::controller_name_message("abcdefghijklmnop.home.commonty.org", "KEY"),
            b"commonty controller name v1\0abcdefghijklmnop.home.commonty.org\0KEY".to_vec()
        );
        assert!(valid_label("abcdefghijklmn27"));
        assert!(!valid_label("abcdefghijklmn18"));
        assert!(!valid_label("short"));
    }

    fn backup_file(header: &serde_json::Value, rest: &[u8]) -> Vec<u8> {
        let h = serde_json::to_vec(header).unwrap();
        let mut f = b"CBK1".to_vec();
        f.extend((h.len() as u32).to_be_bytes());
        f.extend(h);
        f.extend(rest);
        f
    }

    #[test]
    fn a_backups_key_opens_with_this_devices_key_alone() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mine = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let theirs = ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]);
        let mine_pub = b64.encode(mine.verifying_key().to_bytes());
        let theirs_pub = b64.encode(theirs.verifying_key().to_bytes());
        let k = [42u8; 32];
        let header = serde_json::json!({
            "nonce": b64.encode([0u8; 24]),
            "sealed": [
                {"to": theirs_pub, "key": library::seal_to(&theirs_pub, &k).unwrap()},
                {"to": mine_pub, "key": library::seal_to(&mine_pub, &k).unwrap()},
            ],
            "made": 1760000000,
        });
        let file = backup_file(&header, b"sealed tar");
        assert_eq!(*backup::open(&file, &mine_pub, &mine).unwrap(), k);
        // a device it was not sealed to
        let other = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let other_pub = b64.encode(other.verifying_key().to_bytes());
        assert!(backup::open(&file, &other_pub, &other).is_err());
        // named as this device, sealed to another
        assert!(backup::open(&file, &theirs_pub, &mine).is_err());
        // not a backup, or cut short
        assert!(backup::header(b"CBK2\0\0\0\x02{}").is_err());
        assert!(backup::header(&file[..10]).is_err());
        assert!(backup::header(b"CBK1\xff\xff\xff\xff{}").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_boot_partition_where_the_desktop_put_it() {
        let info = "36 35 98:0 / /mnt1 rw,noatime master:1 - ext3 /dev/root rw\n\
                    90 29 8:17 / /media/me/FIRM\\040WARE rw,nosuid shared:50 - vfat /dev/sdb1 rw,uid=1000\n";
        assert_eq!(
            card::mounted_at(info, "/dev/sdb1"),
            Some(std::path::PathBuf::from("/media/me/FIRM WARE"))
        );
        assert_eq!(card::mounted_at(info, "/dev/sdc1"), None);
    }
}
