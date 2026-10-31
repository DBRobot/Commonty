//! Setting up a box, from a desktop: a window of its own (labelled
//! "boxsetup", its page app/web/boxsetup.html) that writes the box
//! installer to a USB drive and puts on it what the installer reads in
//! commonty/: a join ticket for this member, the password of the backup
//! it makes of the machine's old disk, this computer's Wi-Fi if the member
//! says so, and the version (docs/dev/joining.md, As a member's box). Once
//! the box has joined, the backup goes into the member's images.
//!
//! The image, its check and the writer are the home controller's
//! (installer.rs, writer.rs); only what goes on the drive, and after, is here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use auth::KeyStore as _;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Manager as _, Runtime};
use zeroize::Zeroizing;

use crate::installer::{self, Progress};
use crate::writer;

pub const LABEL: &str = "boxsetup";
/// the installer's name in app.json: commonty-box-installer-<version>.img.zst
const IMAGE: &str = "commonty-box-installer-";
/// what the pending backup's password is sealed to the devices as
const SEALED_AS: &str = "join backup";

static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);
/// the ticket on the drive last written here, by id: what to look for
static TICKET: Mutex<Option<String>> = Mutex::new(None);
/// one settling at a time: a copy is long, and two would race
static SETTLING: Mutex<bool> = Mutex::new(false);

fn set(f: impl FnOnce(&mut Progress)) {
    if let Ok(mut p) = PROGRESS.lock() {
        f(p.get_or_insert_with(Progress::default));
    }
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

/// The window's routes, from site.rs: `path` under /_dd/app/boxsetup
pub async fn route<R: Runtime>(
    app: &AppHandle<R>,
    path: &str,
    method: &str,
    body: &[u8],
) -> Result<Response<Vec<u8>>, String> {
    match (method, path) {
        ("POST", "/open") => open_window(app),
        ("GET", "/disks") => match writer::list(app).await {
            Ok(d) => json(&d),
            Err(e) => Ok(refuse(StatusCode::BAD_GATEWAY, &e)),
        },
        // the network this computer is on; its password stays here
        ("GET", "/wifi") => {
            let n = crate::wifi::current().await;
            json(&serde_json::json!({
                "ssid": n.as_ref().map(|n| n.ssid.clone()),
                "known": n.as_ref().is_some_and(|n| n.psk.is_some()),
            }))
        }
        ("POST", "/write") => {
            let w: Write = serde_json::from_slice(body).map_err(|e| e.to_string())?;
            begin_write(app, w)
        }
        ("GET", "/progress") => json(
            &PROGRESS
                .lock()
                .map_err(|e| e.to_string())?
                .clone()
                .unwrap_or_default(),
        ),
        // the box the drive's ticket joined, once it has, and its backup
        ("GET", "/joined") => {
            let id = TICKET.lock().map_err(|e| e.to_string())?.clone();
            let Some(id) = id else {
                return Ok(refuse(StatusCode::NOT_FOUND, "no drive was written here"));
            };
            // its box, joined, made the member's first: until then it is
            // nobody's, and the backup stays where it is
            if let Err(e) = enlist(app, std::slice::from_ref(&id)).await {
                eprintln!("boxsetup: enlisting the box: {e}");
            }
            match pending(app).await {
                Ok(all) => json(&all.into_iter().find(|p| p.id == id)),
                Err(e) => Ok(refuse(StatusCode::BAD_GATEWAY, &e)),
            }
        }
        // Settings > Backups: every backup a box made on its way in
        ("GET", "/pending") => match pending(app).await {
            Ok(all) => json(&all),
            Err(e) => Ok(refuse(StatusCode::BAD_GATEWAY, &e)),
        },
        _ => Ok(refuse(StatusCode::NOT_FOUND, "no such step")),
    }
}

fn open_window<R: Runtime>(app: &AppHandle<R>) -> Result<Response<Vec<u8>>, String> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.set_focus();
        return json(&serde_json::json!({ "ok": true }));
    }
    let at = tauri::Url::parse(&format!("{}/_dd/app/boxsetup", crate::site::origin()))
        .map_err(|e| e.to_string())?;
    let url = if at.scheme() == "http" {
        tauri::WebviewUrl::External(at)
    } else {
        tauri::WebviewUrl::CustomProtocol(at)
    };
    tauri::WebviewWindowBuilder::new(app, LABEL, url)
        .title("Set up a box")
        .inner_size(760.0, 560.0)
        .min_inner_size(560.0, 480.0)
        .build()
        .map_err(|e| e.to_string())?;
    json(&serde_json::json!({ "ok": true }))
}

#[derive(Deserialize)]
struct Write {
    device: String,
    wifi: WifiChoice,
}

/// this computer's network, one typed in, or none: wired
#[derive(Deserialize)]
#[serde(tag = "use", rename_all = "lowercase")]
enum WifiChoice {
    This,
    Other {
        ssid: String,
        psk: Zeroizing<String>,
    },
    None,
}

fn begin_write<R: Runtime>(app: &AppHandle<R>, w: Write) -> Result<Response<Vec<u8>>, String> {
    let busy = PROGRESS
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .is_some_and(|p| matches!(p.stage.as_str(), "manifest" | "download" | "write" | "key"));
    if busy {
        return Ok(refuse(
            StatusCode::CONFLICT,
            "a drive is being written already",
        ));
    }
    if let WifiChoice::Other { ssid, psk } = &w.wifi
        && let Err(e) = valid_wifi(ssid, psk)
    {
        return Ok(refuse(StatusCode::BAD_REQUEST, e));
    }
    *PROGRESS.lock().map_err(|e| e.to_string())? = Some(Progress {
        stage: "manifest".into(),
        ..Default::default()
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = write_drive(&app, w).await {
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

/// WPA's own bounds: a name of 1 to 32 bytes, a password of 8 to 63
fn valid_wifi(ssid: &str, psk: &str) -> Result<(), &'static str> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("a Wi-Fi network's name is 1 to 32 characters");
    }
    if psk.len() < 8 || psk.len() > 63 {
        return Err("a Wi-Fi password is 8 to 63 characters");
    }
    Ok(())
}

async fn write_drive<R: Runtime>(app: &AppHandle<R>, w: Write) -> Result<(), String> {
    let disk = writer::list(app)
        .await?
        .into_iter()
        .find(|d| d.id == w.device)
        .ok_or("that drive is not in this computer any more")?;
    // the Wi-Fi first: a password the system will not give is said before
    // a gigabyte is written, not after
    let wifi = match w.wifi {
        WifiChoice::This => {
            let n = crate::wifi::current()
                .await
                .ok_or("this computer is not on Wi-Fi now")?;
            let psk = n
                .psk
                .ok_or("this computer would not give its Wi-Fi password; type it in instead")?;
            Some((n.ssid, psk))
        }
        WifiChoice::Other { ssid, psk } => Some((ssid, psk)),
        WifiChoice::None => None,
    };
    let (file, version) = installer::manifest(IMAGE).await?;
    set(|p| {
        p.stage = "download".into();
        p.version = version.clone();
    });
    let image = installer::download_to(&file, cache(), &|total, done, bps| {
        set(|p| {
            p.total = total;
            p.done = done;
            p.bytes_per_sec = bps;
            p.eta = writer::eta_secs(&writer::Line {
                phase: "download".into(),
                done,
                total,
                bytes_per_sec: bps,
            });
        })
    })
    .await?;
    set(|p| {
        p.stage = "write".into();
        p.done = 0;
        p.total = 0;
        p.bytes_per_sec = 0;
        p.eta = None;
    });
    writer::write(app, &image, &w.device, &file.sha256, |l| {
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
    let drive = made(app, &version, wifi).await?;
    let id = drive.ticket_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        installer::card::write_files(&disk, &drive.files())
            .map_err(|e| format!("the drive's files could not be written: {e}"))
    })
    .await
    .map_err(|e| e.to_string())??;
    *TICKET.lock().map_err(|e| e.to_string())? = Some(id);
    set(|p| p.stage = "written".into());
    Ok(())
}

fn cache() -> std::path::PathBuf {
    crate::paths::data().join("cache").join("box-installer")
}

/// What goes in commonty/ on the drive
struct Drive {
    ticket: Zeroizing<String>,
    ticket_id: String,
    /// the pending backup's restic password
    backup_key: Zeroizing<String>,
    wifi: Option<Zeroizing<String>>,
    version: String,
}

impl Drive {
    fn files(&self) -> Vec<(&str, &[u8])> {
        let mut f: Vec<(&str, &[u8])> = vec![
            ("ticket", self.ticket.as_bytes()),
            ("backup.key", self.backup_key.as_bytes()),
            ("version", self.version.as_bytes()),
        ];
        if let Some(w) = &self.wifi {
            f.push(("wifi.json", w.as_bytes()));
        }
        f
    }
}

/// The ticket, the backup's password - sealed to this member's devices
/// beside where the backup will be, so any of them can open it - and the
/// Wi-Fi, as the drive takes them.
async fn made<R: Runtime>(
    app: &AppHandle<R>,
    version: &str,
    wifi: Option<(String, Zeroizing<String>)>,
) -> Result<Drive, String> {
    let keys = app.state::<crate::account::Keys>();
    let user = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    let kp = auth::device::load(&keys.0)
        .map_err(|e| e.to_string())?
        .ok_or("this device has no key")?;
    let (ticket, ticket_id) = drive_ticket(&kp, &user)?;
    let mut raw = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut raw[..]).map_err(|e| e.to_string())?;
    let backup_key =
        Zeroizing::new(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw[..]));
    let entry = account::own_entry(&keys.0, &crate::account::dirs(), &user)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let (opener, _, token) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let sealed = account::seal_for_keys(&entry.entry, opener.device().1, SEALED_AS, &raw[..])
        .map_err(|e| format!("{e:#}"))?;
    directory::http()
        .map_err(|e| e.to_string())?
        .put(format!("{}dd-devices.json", pending_url(&ticket_id)?))
        .bearer_auth(token)
        .json(&sealed)
        .send()
        .await
        .map_err(|e| format!("the box did not answer: {e}"))?
        .error_for_status()
        .map_err(|e| format!("the box would not keep the backup's key: {e}"))?;
    let wifi = wifi.map(|(ssid, psk)| {
        Zeroizing::new(serde_json::json!({ "ssid": ssid, "psk": psk.as_str() }).to_string())
    });
    Ok(Drive {
        ticket,
        ticket_id,
        backup_key,
        wifi,
        version: version.to_string(),
    })
}

/// A join ticket for the drive: this device's, for the member, good for a
/// week and for joining alone, and its id.
fn drive_ticket(
    kp: &auth::device::KeyPair,
    user: &str,
) -> Result<(Zeroizing<String>, String), String> {
    let (t, id) = auth::device::mint_join_ticket(kp, user).map_err(|e| e.to_string())?;
    Ok((Zeroizing::new(t), id))
}

fn images_url() -> Result<String, String> {
    Ok(format!(
        "{}/_dd/images/",
        media::gate::files_base(&crate::account::dirs()).map_err(|e| e.to_string())?
    ))
}

fn pending_url(id: &str) -> Result<String, String> {
    Ok(format!("{}pending/{id}/", images_url()?))
}

/// A backup a box made of its machine's old disk on its way in, as Backups
/// shows it.
#[derive(Serialize, Clone)]
pub struct Pending {
    pub id: String,
    /// the box that joined with its ticket, once one has
    #[serde(rename = "box")]
    pub joined: Option<String>,
    /// waiting (no box yet), adding, added, failed
    pub state: String,
    pub error: Option<String>,
}

/// Every pending backup of this member's, and each one whose box has
/// joined copied into the member's images and taken away. Run on any of the
/// member's devices: the backups are where the box put them, not here.
pub async fn pending<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<Pending>, String> {
    let token = crate::site::token(app)?;
    let http = directory::http().map_err(|e| e.to_string())?;
    let r = http
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").map_err(|e| e.to_string())?,
            format!("{}pending/", images_url()?),
        )
        .header("depth", "1")
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("the box did not answer: {e}"))?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(vec![]);
    }
    let listing = r
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let ids = pending_ids(&listing);
    if ids.is_empty() {
        return Ok(vec![]);
    }
    if let Err(e) = enlist(app, &ids).await {
        eprintln!("boxsetup: enlisting boxes: {e}");
    }
    let joined = joined(app).await.unwrap_or_default();
    let mut out = Vec::new();
    for id in ids {
        let Some(name) = joined.get(&id).cloned() else {
            out.push(Pending {
                id,
                joined: None,
                state: "waiting".into(),
                error: None,
            });
            continue;
        };
        let (state, error) = match settle(app, &id, &name).await {
            Ok(true) => ("added", None),
            Ok(false) => ("adding", None),
            Err(e) => ("failed", Some(e)),
        };
        out.push(Pending {
            id,
            joined: Some(name),
            state: state.into(),
            error,
        });
    }
    Ok(out)
}

/// The ids of pending/'s directories, out of a PROPFIND answer
fn pending_ids(listing: &str) -> Vec<String> {
    let mut ids: Vec<String> = listing
        .split("href>")
        .filter_map(|s| s.split('<').next())
        .filter_map(|h| {
            let h = h.trim().trim_end_matches('/');
            let (dir, id) = h.rsplit_once('/')?;
            (dir.ends_with("/pending")
                && id.len() == 32
                && id.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| id.to_ascii_lowercase())
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// What the gate says of this member's boxes (`GET <door>/_dd/network/joined`,
/// member_boxes.rs): each that joined with a ticket of theirs, and whether
/// it is theirs yet
#[derive(Deserialize)]
struct Joined {
    name: String,
    ticket: String,
    #[serde(default)]
    enlisted: bool,
}

async fn joined_rows<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<Joined>, String> {
    let base = media::gate::door_base(&crate::account::dirs()).map_err(|e| e.to_string())?;
    directory::http()
        .map_err(|e| e.to_string())?
        .get(format!("{base}/_dd/network/joined"))
        .bearer_auth(crate::site::token(app)?)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

/// ticket id -> box name, for this member's boxes: the ones theirs, which
/// their pending backups move in for
async fn joined<R: Runtime>(app: &AppHandle<R>) -> Result<HashMap<String, String>, String> {
    Ok(joined_rows(app)
        .await?
        .into_iter()
        .filter(|j| j.enlisted)
        .map(|j| (j.ticket, j.name))
        .collect())
}

/// The boxes that joined with one of `tickets` (the drive this window wrote,
/// the member's pending backups) and are not theirs yet, enlisted with the
/// account's main key, where it is on this device. Elsewhere Settings >
/// Devices adds them, with the passkey.
async fn enlist<R: Runtime>(app: &AppHandle<R>, tickets: &[String]) -> Result<(), String> {
    let keys = app.state::<crate::account::Keys>();
    let Some(root) = account::load_root(&keys.0).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    for j in joined_rows(app).await? {
        if !j.enlisted && tickets.contains(&j.ticket) {
            crate::site::send_box_item(app, identity::envelope::ENLIST, &root, &j.name, &j.ticket)
                .await?;
        }
    }
    Ok(())
}

/// The pending backup into the member's images, named "from <box>", then
/// the pending one deleted. Ok(false): another settling has it now.
async fn settle<R: Runtime>(app: &AppHandle<R>, id: &str, name: &str) -> Result<bool, String> {
    {
        let mut s = SETTLING.lock().map_err(|e| e.to_string())?;
        if *s {
            return Ok(false);
        }
        *s = true;
    }
    let done = settle_one(app, id, name).await;
    if let Ok(mut s) = SETTLING.lock() {
        *s = false;
    }
    done.map(|()| true)
}

async fn settle_one<R: Runtime>(app: &AppHandle<R>, id: &str, name: &str) -> Result<(), String> {
    let mine = crate::site::archive_value(app)
        .await?
        .and_then(|v| v["password"].as_str().map(str::to_string))
        .map(Zeroizing::new)
        .ok_or("your disk images do not open on this device yet")?;
    let theirs = backup_password(app, id).await?;
    let held = Held::of(app)?;
    let from = pending_url(id)?;
    let to = images_url()?;
    let label = format!("from {name}");
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let tokens = Arc::new(archive::TokenProvider::new(held));
        let open = |url: &str, pw: Zeroizing<String>| {
            archive::Archive::new(
                url::Url::parse(url).map_err(|e| e.to_string())?,
                tokens.clone(),
                pw,
                rustic_core::NoProgressBars,
            )
            .map_err(|e| format!("{e:#}"))
        };
        let from = open(&from, theirs)?;
        // a box that joined with nothing to back up made no repository
        if from.exists().map_err(|e| format!("{e:#}"))? {
            from.copy_into(&open(&to, mine)?, Some(&label))
                .map_err(|e| format!("{e:#}"))?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    // copied, so the pending one goes (into the trash, its 30 days)
    directory::http()
        .map_err(|e| e.to_string())?
        .delete(pending_url(id)?)
        .bearer_auth(crate::site::token(app)?)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// the pending backup's password, opened from its sealed copy
async fn backup_password<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
) -> Result<Zeroizing<String>, String> {
    let keys = app.state::<crate::account::Keys>();
    let (opener, _, token) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let v: serde_json::Value = directory::http()
        .map_err(|e| e.to_string())?
        .get(format!("{}dd-devices.json", pending_url(id)?))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let user = keys
        .0
        .get(crate::account::USER)
        .map_err(|e| e.to_string())?
        .ok_or("no name on this device")?;
    let entry = account::own_entry(&keys.0, &crate::account::dirs(), &user)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let k = account::open_for_device(
        &v,
        &entry.entry,
        SEALED_AS,
        opener.device(),
        opener.root.as_ref(),
    )
    .map_err(|_| "that backup was made before this device was yours, so this device cannot open it; open Backups on the computer that set the box up")?;
    Ok(Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&k[..]),
    ))
}

/// The two keys the archive's token needs, the name and this device's key,
/// held for the copy's thread, which outlives any borrow of the app's store.
struct Held(Mutex<HashMap<&'static str, Zeroizing<String>>>);

impl Held {
    fn of<R: Runtime>(app: &AppHandle<R>) -> Result<Self, String> {
        let keys = app.state::<crate::account::Keys>();
        let mut m = HashMap::new();
        for k in [crate::account::USER, auth::device::ACCOUNT] {
            let v = keys
                .0
                .get(k)
                .map_err(|e| e.to_string())?
                .ok_or("this device has no key")?;
            m.insert(k, v);
        }
        Ok(Self(Mutex::new(m)))
    }
}

impl auth::KeyStore for Held {
    fn get(&self, account: &str) -> auth::Result<Option<Zeroizing<String>>> {
        Ok(self.0.lock().ok().and_then(|m| m.get(account).cloned()))
    }
    fn set(&self, _: &str, _: &str) -> auth::Result<()> {
        Ok(())
    }
    fn clear(&self, _: &str) -> auth::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_installer_out_of_a_checked_manifest() {
        let p = serde_json::json!({
            "kind": "app",
            "files": {
                "commonty-home-controller-0.1.30.img.zst": {"url": "https://x/c.img.zst", "sha256": "b".repeat(64)},
                "commonty-box-installer-0.1.30.img.zst": {"url": "https://x/b.img.zst", "sha256": "c".repeat(64)},
            }
        });
        let (f, v) = installer::pick(&p, IMAGE).unwrap();
        assert_eq!(v, "0.1.30");
        assert_eq!(f.url, "https://x/b.img.zst");
        let none = serde_json::json!({"files": {"commonty-home-controller-1.img.zst": {"url": "https://x", "sha256": "a".repeat(64)}}});
        assert!(installer::pick(&none, IMAGE).is_err());
    }

    // the box's own reader (dd-box-setup --ticket-id, which backup.sh runs)
    // on the ticket `made` writes: with no id here the backup has no folder
    // and dd-control has no claim to join with
    #[test]
    fn the_drives_ticket_names_its_id_as_the_box_reads_it() {
        let kp = auth::device::generate();
        let (t, id) = drive_ticket(&kp, "ann").unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(release::setup::ticket_id(&t).as_deref(), Some(&*id));
        let (_, other) = drive_ticket(&kp, "ann").unwrap();
        assert_ne!(other, id);
    }

    #[test]
    fn what_goes_on_the_drive() {
        let d = Drive {
            ticket: Zeroizing::new("TICKET".into()),
            ticket_id: "ab".repeat(16),
            backup_key: Zeroizing::new("KEY".into()),
            wifi: None,
            version: "0.1.30".into(),
        };
        let names: Vec<_> = d.files().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["ticket", "backup.key", "version"]);
        let d = Drive {
            wifi: Some(Zeroizing::new(r#"{"ssid":"a","psk":"b"}"#.into())),
            ..d
        };
        let f = d.files();
        assert_eq!(f.len(), 4);
        assert_eq!(f[3], ("wifi.json", &br#"{"ssid":"a","psk":"b"}"#[..]));
        assert_eq!(f[0].1, b"TICKET");
    }

    #[test]
    fn the_wifi_choice_as_the_page_sends_it() {
        let w: Write = serde_json::from_str(r#"{"device":"sdb","wifi":{"use":"this"}}"#).unwrap();
        assert!(matches!(w.wifi, WifiChoice::This));
        let w: Write = serde_json::from_str(
            r#"{"device":"sdb","wifi":{"use":"other","ssid":"Home","psk":"hunter222"}}"#,
        )
        .unwrap();
        assert!(matches!(&w.wifi, WifiChoice::Other { ssid, .. } if ssid == "Home"));
        let w: Write = serde_json::from_str(r#"{"device":"sdb","wifi":{"use":"none"}}"#).unwrap();
        assert!(matches!(w.wifi, WifiChoice::None));
        assert!(valid_wifi("Home", "hunter222").is_ok());
        assert!(valid_wifi("", "hunter222").is_err());
        assert!(valid_wifi("Home", "short").is_err());
        assert!(valid_wifi(&"x".repeat(33), "hunter222").is_err());
    }

    #[test]
    fn pending_backups_out_of_a_listing() {
        let a = "0123456789abcdef0123456789abcdef";
        let b = "fedcba9876543210fedcba9876543210";
        let listing = format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:">
<D:response><D:href>/_dd/images/pending/</D:href></D:response>
<D:response><D:href>/_dd/images/pending/{a}/</D:href></D:response>
<d:response><d:href>/_dd/images/pending/{b}</d:href></d:response>
<D:response><D:href>/_dd/images/pending/notanid/</D:href></D:response>
<D:response><D:href>/_dd/images/{a}/</D:href></D:response>
</D:multistatus>"#
        );
        assert_eq!(pending_ids(&listing), [a, b]);
        assert!(pending_ids("").is_empty());
    }
}
