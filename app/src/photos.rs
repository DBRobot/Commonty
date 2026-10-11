//! Photos' password: made here, at random, kept with this device's keys,
//! and shared with the member's other devices sealed to their keys. No
//! passkey makes it (any page of the domain can ask a passkey for its PRF)
//! and no box can open it.
//!
//! The app hands it to its own Photos page once per door (local.rs,
//! `/_dd/app/photos-key`), with what the page should do with it:
//!
//! ```text
//! {pw}               open the account
//! {pw, fresh: true}  open it, or make it when there is none
//! {pw, next}         an account a passkey made: open it with pw (the old
//!                    password), give it next, then say so (photos-keyed)
//! ```
//!
//! Beside the member's disk images, as `dd-photos.json`, the password is
//! sealed to every device in an entry the member's own root signs, the
//! root and the paper key, and signed by the device that sealed it
//! (account::seal_for_keys); a device that does not
//! hold it yet opens it from there. An account made before any of this,
//! whose password only a passkey can make and no device here holds, moves
//! once: the member's browser asks the passkey on the site's move page and
//! hands the old password to this app sealed to a key made for it.

use std::sync::Mutex;
use std::time::Duration;

use auth::KeyStore as _;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64U;
use tauri::http::{Response, header};
use tauri::{AppHandle, Manager as _, Runtime};

/// the password a passkey made, from before; gone once the account moved
pub const LEGACY: &str = "photos-password";
/// the password this app made
pub const KEY: &str = "photos-key";
const BLOB: &str = "dd-photos.json";
/// what the copy is of, signed with it (account::seal_for_keys)
const WHAT: &str = "photos";

/// passwords made here and handed to a page, not yet the account's
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// a move under way: the gate's slot and the key only this app opens with
static MOVING: Mutex<Option<(String, library::Key)>> = Mutex::new(None);

fn store<R: Runtime>(app: &AppHandle<R>) -> tauri::State<'_, crate::account::Keys> {
    app.state::<crate::account::Keys>()
}

fn fresh() -> String {
    B64U.encode(&library::random_key()[..])
}

fn base() -> Result<String, String> {
    media::gate::files_base(&crate::account::dirs()).map_err(|e| e.to_string())
}

fn blob_url() -> Result<String, String> {
    Ok(format!("{}/_dd/images/{BLOB}", base()?))
}

/// What the Photos page is handed, as json (the shapes above)
pub async fn hand<R: Runtime>(app: &AppHandle<R>) -> Result<String, String> {
    let keys = store(app);
    if let Some(k) = keys.0.get(KEY).map_err(|e| e.to_string())? {
        return Ok(serde_json::json!({ "pw": k.as_str() }).to_string());
    }
    match from_blob(app).await {
        Ok(Some(k)) => {
            keys.0.set(KEY, &k).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "pw": k }).to_string());
        }
        Ok(None) => {}
        // the box not answering is not "no password anywhere": a page that
        // made a new account now would leave the member two
        Err(e) => return Err(format!("the box did not say whether Photos has a key: {e}")),
    }
    let next = fresh();
    PENDING
        .lock()
        .map_err(|e| e.to_string())?
        .push(next.clone());
    Ok(match keys.0.get(LEGACY).map_err(|e| e.to_string())? {
        Some(old) => serde_json::json!({ "pw": old.as_str(), "next": next }),
        None => serde_json::json!({ "pw": next, "fresh": true }),
    }
    .to_string())
}

/// The page made the account with the password it was handed, or moved it
/// to it: it is the account's now. Only a password this app made counts.
pub async fn keyed<R: Runtime>(app: &AppHandle<R>, pw: &str) -> Result<(), String> {
    {
        let mut p = PENDING.lock().map_err(|e| e.to_string())?;
        if !p.iter().any(|k| k == pw) {
            return Err("not a password this app made".into());
        }
        p.retain(|k| k != pw);
    }
    let keys = store(app);
    keys.0.set(KEY, pw).map_err(|e| e.to_string())?;
    let _ = keys.0.clear(LEGACY);
    share(app, pw).await
}

/// The password, again, for Ente's own "confirm it's you"
pub fn current<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    store(app).0.get(KEY).ok().flatten().map(|k| k.to_string())
}
/// The member's own entry, as this device vouches for it (the root it holds
/// or pinned), with this device's keys
async fn own<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<(media::gate::Opener, identity::Entry, String), String> {
    let keys = store(app);
    let (opener, user, token) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let entry = account::own_entry(&keys.0, &crate::account::dirs(), &user)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok((opener, entry.entry, token))
}

/// sealed to every key of the member's own entry that a device holds, and
/// signed by this device, beside their disk images
async fn share<R: Runtime>(app: &AppHandle<R>, pw: &str) -> Result<(), String> {
    let raw = B64U.decode(pw).map_err(|e| e.to_string())?;
    let (opener, entry, token) = own(app).await?;
    let sealed = account::seal_for_keys(&entry, opener.device().1, WHAT, &raw)
        .map_err(|e| format!("{e:#}"))?;
    directory::http()
        .map_err(|e| e.to_string())?
        .put(blob_url()?)
        .bearer_auth(token)
        .json(&sealed)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn blob<R: Runtime>(app: &AppHandle<R>) -> Result<Option<serde_json::Value>, String> {
    let token = crate::site::token(app)?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .get(blob_url()?)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let r = r.error_for_status().map_err(|e| e.to_string())?;
    Ok(Some(r.json().await.map_err(|e| e.to_string())?))
}

/// The password out of the sealed copy, once a device of the member's own
/// entry is shown to have signed it, with this device's key or the root
/// here. A copy no key here opens, or that no device of theirs made (a box
/// can seal a password of its own to this device), is as good as none.
async fn from_blob<R: Runtime>(app: &AppHandle<R>) -> Result<Option<String>, String> {
    let Some(v) = blob(app).await? else {
        return Ok(None);
    };
    let (opener, entry, _) = own(app).await?;
    match account::open_for_device(&v, &entry, WHAT, opener.device(), opener.root.as_ref()) {
        Ok(k) => Ok(Some(B64U.encode(&k[..]))),
        Err(e) => {
            eprintln!("photos: the copy beside the disk images is not taken: {e:#}");
            Ok(None)
        }
    }
}

/// At launch: a device that holds the password makes sure every key it is
/// due to is in a copy the member's devices made (one admitted since, a copy
/// never made or not theirs). For an entry whose root is a passkey that is
/// this device alone (account::seal_for_keys says why).
pub async fn settle<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(pw) = current(app) else {
        return Ok(());
    };
    let (opener, entry, _) = own(app).await?;
    let me = opener.device().1.verifying_key();
    let have: Vec<String> = blob(app)
        .await?
        .and_then(|v| {
            account::vouched(&v, &entry, &me, WHAT)
                .ok()
                .and_then(|k| k.as_object().map(|o| o.keys().cloned().collect()))
        })
        .unwrap_or_default();
    let missing = account::due_for_keys(&entry, opener.device().0)
        .iter()
        .any(|k| !have.contains(k));
    if missing {
        share(app, &pw).await?;
    }
    Ok(())
}

/// After the paper key brought the account back on this device: the
/// password out of the copy sealed to the old paper key - made by a device
/// of the entry as it was before (`before`, which the paper key matched) -
/// kept here, and sealed again to the entry as it is now.
pub async fn recovered<R: Runtime>(
    app: &AppHandle<R>,
    paper: &ed25519_dalek::SigningKey,
    before: &identity::Entry,
) -> Result<(), String> {
    // the paper key is what vouches for that entry: it is the one it names
    if before.recovery != identity::encode_public(&paper.verifying_key()) {
        return Err("that paper key is not the one the account had".into());
    }
    let Some(v) = blob(app).await? else {
        return Ok(());
    };
    let Some(by) = v["by"].as_str() else {
        return Ok(());
    };
    // the old devices' keys are not this one's: the copy is checked with
    // the key of the device that signed it, as the old entry names it
    let signer = before
        .devices
        .iter()
        .find(|d| d.fingerprint == by)
        .and_then(|d| identity::decode_public(&d.public_key).ok())
        .ok_or("the copy was made by no device of the account")?;
    let keys = account::vouched(&v, before, &signer, WHAT).map_err(|e| format!("{e:#}"))?;
    let Some(sealed) = keys["recovery"].as_str() else {
        return Ok(());
    };
    let k = library::open_with(paper, sealed).map_err(|e| e.to_string())?;
    let pw = B64U.encode(&k[..]);
    store(app).0.set(KEY, &pw).map_err(|e| e.to_string())?;
    share(app, &pw).await
}

/// The move of an account a passkey made: a slot at the gate, the browser
/// sent to the site's move page to fill it, and this app waiting for it.
/// The page that asked follows `moving()`.
pub async fn start_move<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;
    let (public, secret) = library::ephemeral();
    let token = crate::site::token(app)?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .post(format!("{}/_dd/photos/handoff", base()?))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("the box said {}", r.status()));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
    let id = v["id"].as_str().ok_or("no slot")?.to_string();
    let url = format!(
        "https://home.{}/_dd/photos/move?slot={id}&to={}",
        crate::account::domain(),
        crate::handoff::enc(&public)
    );
    *MOVING.lock().map_err(|e| e.to_string())? = Some((id.clone(), secret));
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())?;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = wait_move(&app, &id).await {
            eprintln!("photos: the move: {e}");
        }
        if let Ok(mut m) = MOVING.lock() {
            *m = None;
        }
    });
    Ok(())
}

async fn wait_move<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), String> {
    let http = directory::http().map_err(|e| e.to_string())?;
    for _ in 0..300 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let r = http
            .get(format!("{}/_dd/photos/handoff/{id}", base()?))
            .bearer_auth(crate::site::token(app)?)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        match r.status().as_u16() {
            204 => continue,
            200 => {
                let sealed = r.text().await.map_err(|e| e.to_string())?;
                let secret = MOVING
                    .lock()
                    .map_err(|e| e.to_string())?
                    .as_ref()
                    .filter(|(slot, _)| slot == id)
                    .map(|(_, k)| k.clone())
                    .ok_or("no key to open it with")?;
                let raw =
                    library::open_x25519(&secret, sealed.trim()).map_err(|e| e.to_string())?;
                // the old password, as the passkey made it; the page moves
                // the account off it before anything else
                store(app)
                    .0
                    .set(LEGACY, &B64U.encode(&raw[..]))
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }
            s => return Err(format!("the box said {s}")),
        }
    }
    Err("nothing came within ten minutes".into())
}

/// whether a move is waiting on the browser, and whether there is an old
/// password here to move now
pub fn moving<R: Runtime>(app: &AppHandle<R>) -> serde_json::Value {
    let waiting = MOVING.lock().map(|m| m.is_some()).unwrap_or(false);
    let here = store(app).0.get(LEGACY).ok().flatten().is_some();
    serde_json::json!({ "waiting": waiting, "ready": here })
}

fn json(v: serde_json::Value) -> Result<Response<Vec<u8>>, String> {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(v.to_string().into_bytes())
        .map_err(|e| e.to_string())
}

/// The window's Photos page (web/photos.js): where it goes, the app's own
/// copy of Photos through its door, with what that page is handed waiting
/// for it (no address carries a password, so no history keeps one).
pub async fn state<R: Runtime>(app: &AppHandle<R>) -> Result<Response<Vec<u8>>, String> {
    let hand = hand(app).await?;
    let url = format!(
        "{}#app={}",
        crate::local::door(false, "/_dd/photos")?,
        crate::handoff::enc(crate::site::origin())
    );
    crate::local::set_window_key(&hand);
    json(serde_json::json!({ "ready": true, "url": url }))
}
