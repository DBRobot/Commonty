//! The member's calendars and address books for the other programs on this
//! computer (Thunderbird, GNOME's and Apple's calendars and contacts): the
//! bridge (client/davbridge) on 127.0.0.1, over the libraries this device
//! opens, with a password of this computer's own. The Calendar and Contacts
//! pages in the app's window show where it is and the password.

use std::sync::Arc;

use tauri::{AppHandle, Manager as _, Runtime};

/// the bridge's address on this computer
pub const PORT: u16 = 41664;

/// the password programs sign in with: made once, kept beside the app's
/// other state, readable by this account alone
pub fn password() -> std::io::Result<String> {
    let at = crate::paths::data().join("calendar-password");
    if let Ok(p) = std::fs::read_to_string(&at)
        && p.trim().len() >= 20
    {
        return Ok(p.trim().to_string());
    }
    let mut b = [0u8; 18];
    getrandom::fill(&mut b).map_err(std::io::Error::other)?;
    use base64::Engine as _;
    let p = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    if let Some(d) = at.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(0o600);
    }
    std::io::Write::write_all(&mut o.open(&at)?, p.as_bytes())?;
    Ok(p)
}

/// what the Calendar and Contacts pages show for it, in the app's window
pub fn details() -> serde_json::Value {
    match password() {
        Ok(p) => serde_json::json!({
            "calendars": format!("http://127.0.0.1:{PORT}/dav/calendars/"),
            "contacts": format!("http://127.0.0.1:{PORT}/dav/contacts/"),
            "server": format!("http://127.0.0.1:{PORT}/"),
            "user": davbridge::USER,
            "password": p,
        }),
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    }
}

/// the calendars from a link added on this computer, beside the app's
/// other state: [{ lib, folder, url }], this account alone
fn links_file() -> std::path::PathBuf {
    crate::paths::data().join("calendar-links.json")
}

fn read_links() -> Vec<(String, String, String)> {
    let v: serde_json::Value = std::fs::read(links_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| {
            Some((
                l["lib"].as_str()?.to_string(),
                l["folder"].as_str()?.to_string(),
                l["url"].as_str()?.to_string(),
            ))
        })
        .collect()
}

static REFRESH: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// A calendar from a link, added in the app's window: `{ lib, folder, url }`,
/// kept in this computer's list and fetched at once.
pub fn add_link(body: &[u8]) -> Result<serde_json::Value, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let lib = v["lib"].as_str().unwrap_or("");
    let folder = v["folder"].as_str().unwrap_or("");
    let url =
        davbridge::subscribe::link(v["url"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
    if !identity::valid_library_id(lib)
        || !folder.starts_with("Calendars/")
        || folder.contains("..")
        || folder.matches('/').count() != 1
    {
        return Err("not a calendar".into());
    }
    let mut all: Vec<serde_json::Value> = read_links()
        .into_iter()
        .filter(|(l, f, _)| !(l == lib && f == folder))
        .map(|(l, f, u)| serde_json::json!({ "lib": l, "folder": f, "url": u }))
        .collect();
    all.push(serde_json::json!({ "lib": lib, "folder": folder, "url": url }));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(0o600);
    }
    std::io::Write::write_all(
        &mut o.open(links_file()).map_err(|e| e.to_string())?,
        serde_json::Value::Array(all).to_string().as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    REFRESH.notify_one();
    Ok(serde_json::json!({ "ok": true }))
}

/// the libraries and a token, from this device's keys
struct Device<R: Runtime> {
    app: AppHandle<R>,
}

#[async_trait::async_trait]
impl<R: Runtime> davbridge::gate::Source for Device<R> {
    async fn libraries(&self) -> anyhow::Result<Vec<davbridge::gate::Lib>> {
        crate::local::network_ready().await;
        let keys = self.app.state::<crate::account::Keys>();
        let (opener, user, _) = media::gate::Opener::load(&keys.0)?;
        let dirs = crate::account::dirs();
        let list = account::released(&keys.0, &dirs).await.ok();
        let libs = media::gate::openable(&dirs, &user, &opener, list.as_ref()).await?;
        let mut main = true;
        Ok(libs
            .into_iter()
            .map(|(owner, lib, key)| {
                let mine = owner == user;
                let writable = mine || lib.readers.iter().any(|r| r.name == user && r.writes);
                let l = davbridge::gate::Lib {
                    id: lib.id,
                    key,
                    main: mine && main,
                    owned: mine,
                    writable,
                };
                if mine {
                    main = false;
                }
                l
            })
            .collect())
    }

    fn links(&self) -> Vec<(String, String, String)> {
        read_links()
    }

    async fn token(&self) -> anyhow::Result<(String, String)> {
        crate::local::network_ready().await;
        let token = crate::site::token(&self.app).map_err(|e| anyhow::anyhow!(e))?;
        let base = media::gate::files_base(&crate::account::dirs())?;
        Ok((token, base))
    }
}

pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let pw = match password() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("calendars for other programs: no password: {e}");
                return;
            }
        };
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", PORT)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("calendars for other programs: 127.0.0.1:{PORT}: {e}");
                return;
            }
        };
        let store = Arc::new(davbridge::gate::GateStore::new(Device { app }));
        // calendars from a link: fetched now and every half hour after,
        // while the app runs; on one computer would do, on several each
        // writes the same files
        let links = store.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                for (cal, r) in davbridge::subscribe::refresh_all(&links).await {
                    if let Err(e) = r {
                        eprintln!("calendar from a link, {cal}: {e:#}");
                    }
                }
                // half an hour, or at once when one is added
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(30 * 60),
                    REFRESH.notified(),
                )
                .await;
            }
        });
        if let Err(e) = axum::serve(listener, davbridge::router(store, pw, PORT)).await {
            eprintln!("calendars for other programs: 127.0.0.1:{PORT}: {e}");
        }
    });
}
