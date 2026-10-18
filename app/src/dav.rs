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
                    writable,
                };
                if mine {
                    main = false;
                }
                l
            })
            .collect())
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
        if let Err(e) = axum::serve(listener, davbridge::router(store, pw, PORT)).await {
            eprintln!("calendars for other programs: 127.0.0.1:{PORT}: {e}");
        }
    });
}
