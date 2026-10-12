//! The household's home controllers, as this account remembers them: a
//! list locked with the member's own library key, as the Devices tab locks
//! device names, kept at the gate as bytes (/_dd/controllers). No box can
//! tell from it whether there is a controller at all. The installer adds to
//! it after pairing; Settings and the home page read it through the app.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager as _, Runtime};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Controller {
    /// the name's first label
    pub label: String,
    /// <label>.home.<domain>
    pub name: String,
    /// the controller's ed25519 key from pairing, base64: what the app
    /// holds the door to from then on
    pub controller: String,
}

/// the member's own library: its id and key
async fn own_library<R: Runtime>(app: &AppHandle<R>) -> Result<(String, library::Key), String> {
    let keys = app.state::<crate::account::Keys>();
    let (opener, user, _) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let libs = media::gate::openable(&crate::account::dirs(), &user, &opener, None)
        .await
        .map_err(|e| format!("{e:#}"))?;
    libs.into_iter()
        .find(|(o, _, _)| *o == user)
        .map(|(_, l, k)| (l.id, k))
        .ok_or_else(|| {
            "your home controllers are kept locked with your library, and this account has none yet"
                .into()
        })
}

fn seal(key: &library::Key, id: &str, plain: &[u8]) -> Vec<u8> {
    let c = library::crypt::Cipher::for_library(key, id);
    let e = c.encrypter();
    let mut out = e.header();
    for (n, block) in plain.chunks(library::crypt::BLOCK).enumerate() {
        out.extend(e.block(n as u64, block));
    }
    out
}

fn open(key: &library::Key, id: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
    if sealed.len() < library::crypt::HEADER {
        return Err("shorter than a header".into());
    }
    let c = library::crypt::Cipher::for_library(key, id);
    let d = c
        .decrypter(&sealed[..library::crypt::HEADER])
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (n, block) in sealed[library::crypt::HEADER..]
        .chunks(library::crypt::SEALED_BLOCK)
        .enumerate()
    {
        out.extend(d.block(n as u64, block).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn url() -> Result<String, String> {
    let base = media::gate::files_base(&crate::account::dirs()).map_err(|e| e.to_string())?;
    Ok(format!("{base}/_dd/controllers"))
}

/// the list as it is kept; none yet is an empty list, one that will not
/// open is an error (or the next save would write over it)
pub async fn read<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<Controller>, String> {
    let (id, key) = own_library(app).await?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .get(url()?)
        .bearer_auth(crate::site::token(app)?)
        .send()
        .await
        .map_err(|e| format!("the box did not answer: {e}"))?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    if !r.status().is_success() {
        return Err(format!("the box said {}", r.status()));
    }
    let sealed = r.bytes().await.map_err(|e| e.to_string())?;
    // the key is stretched first (scrypt): off the async threads
    let plain = tauri::async_runtime::spawn_blocking(move || open(&key, &id, &sealed))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|_| "your home controllers could not be opened".to_string())?;
    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}

/// one more, or the same one again with what pairing said now; `old`, a
/// controller it was restored from, goes
pub async fn replace<R: Runtime>(
    app: &AppHandle<R>,
    old: Option<&str>,
    c: Controller,
) -> Result<(), String> {
    let mut all = read(app).await?;
    swap(&mut all, old, c);
    let (id, key) = own_library(app).await?;
    let plain = serde_json::to_vec(&all).map_err(|e| e.to_string())?;
    let sealed = tauri::async_runtime::spawn_blocking(move || seal(&key, &id, &plain))
        .await
        .map_err(|e| e.to_string())?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .put(url()?)
        .bearer_auth(crate::site::token(app)?)
        .body(sealed)
        .send()
        .await
        .map_err(|e| format!("the box did not answer: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("the box said {}", r.status()));
    }
    Ok(())
}

fn swap(all: &mut Vec<Controller>, old: Option<&str>, c: Controller) {
    all.retain(|x| x.label != c.label && Some(x.label.as_str()) != old);
    all.push(c);
}

/// GET /_dd/app/controllers: the list for the app's own pages
pub async fn route<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<tauri::http::Response<Vec<u8>>, String> {
    let all = read(app).await?;
    tauri::http::Response::builder()
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(serde_json::to_vec(&all).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_opens_with_the_key_it_was_locked_with_alone() {
        let key = library::random_key();
        let id = library::random_id();
        let list = vec![Controller {
            label: "abcdefghijklmnop".into(),
            name: "abcdefghijklmnop.home.commonty.org".into(),
            controller: "AAAA".into(),
        }];
        let plain = serde_json::to_vec(&list).unwrap();
        let sealed = seal(&key, &id, &plain);
        assert!(!sealed.windows(16).any(|w| w == b"abcdefghijklmnop"));
        let back: Vec<Controller> =
            serde_json::from_slice(&open(&key, &id, &sealed).unwrap()).unwrap();
        assert_eq!(back, list);
        assert!(open(&library::random_key(), &id, &sealed).is_err());
    }

    #[test]
    fn a_restored_controller_takes_the_old_ones_place() {
        let c = |l: &str| Controller {
            label: l.into(),
            name: format!("{l}.home.x"),
            controller: l.into(),
        };
        let mut all = vec![c("old"), c("other")];
        swap(&mut all, Some("old"), c("new"));
        assert_eq!(all, vec![c("other"), c("new")]);
        swap(&mut all, None, c("new"));
        assert_eq!(all, vec![c("other"), c("new")]);
    }
}
