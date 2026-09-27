//! The fleet's own network from this device: the engine is Go (app/net,
//! Tailscale's tsnet) behind four C calls, and what it gives back is a
//! loopback proxy into the network. Everything the app asks a box is then
//! asked through that proxy, by the box's name, straight to the box.

use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::account::Keys;

unsafe extern "C" {
    fn commonty_net_start(
        dir: *const c_char,
        control: *const c_char,
        key: *const c_char,
        hostname: *const c_char,
        tunfd: i32,
    ) -> *mut c_char;
    fn commonty_net_status() -> *mut c_char;
    fn commonty_net_stop();
    fn commonty_net_browser(domain: *const c_char) -> *mut c_char;
    fn commonty_net_free(p: *mut c_char);
}

/// a string the engine returned, freed when read
fn take(p: *mut c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { commonty_net_free(p) };
    s
}

#[derive(Deserialize)]
struct Started {
    proxy: String,
    credential: String,
    ip: String,
    #[serde(default)]
    error: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Peer {
    pub name: String,
    pub ip: String,
    pub online: bool,
}

#[derive(Serialize, Deserialize, Default)]
pub struct NetStatus {
    pub running: bool,
    pub state: String,
    pub ip: String,
    pub name: String,
    #[serde(default)]
    pub peers: Vec<Peer>,
    #[serde(default)]
    pub error: String,
    /// whether this device has joined before (state on disk)
    #[serde(default)]
    pub joined: bool,
    /// what the browser here needs by hand, when it could not be set up
    #[serde(default)]
    pub browser: String,
}

/// the proxy rules' url while they are up, and what the person has to do
/// by hand if the system could not be told
static BROWSER: std::sync::Mutex<(String, Option<String>)> =
    std::sync::Mutex::new((String::new(), None));

#[derive(Deserialize)]
struct Door {
    #[serde(default)]
    pac: String,
    #[serde(default)]
    error: String,
}

/// the browser on this machine, sent through the network for the fleet's
/// names (browser.rs); a phone gets its VPN instead
fn open_browser() {
    #[cfg(not(target_os = "android"))]
    {
        let Ok(d) = CString::new(crate::account::domain()) else {
            return;
        };
        let out = take(unsafe { commonty_net_browser(d.as_ptr()) });
        let door: Door = match serde_json::from_str(&out) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("network: browser: {e}: {out}");
                return;
            }
        };
        if !door.error.is_empty() {
            eprintln!("network: browser: {}", door.error);
            return;
        }
        let by_hand = crate::browser::register(&door.pac);
        if let Some(m) = &by_hand {
            eprintln!("network: {m}");
        }
        if let Ok(mut b) = BROWSER.lock() {
            *b = (door.pac, by_hand);
        }
    }
}

/// signing out on this device: the browser goes straight out again
pub fn forget_browser() {
    if let Ok(mut b) = BROWSER.lock()
        && !b.0.is_empty()
    {
        crate::browser::unregister(&b.0);
        *b = (String::new(), None);
    }
}

fn state_dir() -> Result<PathBuf, String> {
    Ok(crate::paths::data().join("net"))
}

fn hostname() -> String {
    #[cfg(target_os = "android")]
    {
        "phone".to_string()
    }
    #[cfg(not(target_os = "android"))]
    {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "device".to_string())
    }
}

/// bring the engine up, from the state on disk or with a fresh key, and
/// point every client at its proxy
fn start(control: &str, key: &str, tunfd: i32) -> Result<Started, String> {
    let dir = state_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let marker = dir.join("joined");
    let c = |s: &str| CString::new(s).map_err(|e| e.to_string());
    let (dir, control, key, host) = (
        c(&dir.to_string_lossy())?,
        c(control)?,
        c(key)?,
        c(&hostname())?,
    );
    let out = take(unsafe {
        commonty_net_start(
            dir.as_ptr(),
            control.as_ptr(),
            key.as_ptr(),
            host.as_ptr(),
            tunfd,
        )
    });
    let st: Started = serde_json::from_str(&out).map_err(|e| format!("engine: {e}: {out}"))?;
    if !st.error.is_empty() {
        return Err(st.error);
    }
    eprintln!("network: on as {}, proxy at {}", st.ip, st.proxy);
    // joined for real: the next start resumes from here without a key
    let _ = std::fs::write(&marker, st.ip.as_bytes());
    if std::env::var_os("COMMONTY_NET_DEBUG").is_some() {
        eprintln!("network: proxy credential {}", st.credential);
    }
    directory::via(Some(directory::Proxy {
        addr: st.proxy.clone(),
        user: "tsnet".to_string(),
        password: st.credential.clone(),
    }));
    open_browser();
    Ok(st)
}

fn status_now() -> NetStatus {
    let out = take(unsafe { commonty_net_status() });
    let mut st: NetStatus = serde_json::from_str(&out).unwrap_or_default();
    st.joined = state_dir()
        .map(|d| d.join("joined").exists())
        .unwrap_or(false);
    if let Ok(b) = BROWSER.lock() {
        st.browser = b.1.clone().unwrap_or_default();
    }
    st
}

#[tauri::command]
pub async fn net_status() -> Result<NetStatus, String> {
    Ok(status_now())
}

/// join: the gate vouches for this device and hands it a key; the engine
/// takes it once, and from then on the state on disk is the membership
#[tauri::command]
pub async fn net_join(app: tauri::AppHandle, keys: State<'_, Keys>) -> Result<NetStatus, String> {
    let (_, user, token) = media::gate::Opener::load(&keys.0).map_err(|e| e.to_string())?;
    let dirs = crate::account::dirs();
    // the front door: this device is not on the network yet
    let base = media::gate::door_base(&dirs).map_err(|e| e.to_string())?;
    let r = directory::http()
        .map_err(|e| e.to_string())?
        .post(format!("{base}/_dd/network/join"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!(
            "the gate refused {user}'s device: {} {}",
            r.status(),
            r.text().await.unwrap_or_default()
        ));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
    let control = v["control_url"]
        .as_str()
        .ok_or("no control url")?
        .to_string();
    let key = v["key"].as_str().ok_or("no key")?.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let st = start(&control, &key, -1)?;
        // joined; a phone now takes the network for the whole device
        #[cfg(target_os = "android")]
        {
            stop();
            let fd = crate::vpn::up(&app, &st.ip)?;
            if let Err(e) = start(&control, "", fd) {
                crate::vpn::down(&app);
                return Err(e);
            }
        }
        let _ = (&app, st);
        Ok::<_, String>(())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(status_now())
}

/// resume from the state on disk, at app start; nothing if never joined
pub fn resume(app: &tauri::AppHandle, control: &str) {
    let Ok(d) = state_dir() else { return };
    // the marker holds this node's address, which the phone's VPN needs
    let Ok(ip) = std::fs::read_to_string(d.join("joined")) else {
        return;
    };
    let (app, control) = (app.clone(), control.to_string());
    std::thread::spawn(move || {
        #[cfg(target_os = "android")]
        let fd = match crate::vpn::up(&app, ip.trim()) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("network: {e}");
                return;
            }
        };
        #[cfg(not(target_os = "android"))]
        let fd = {
            let _ = (&app, &ip);
            -1
        };
        if let Err(e) = start(&control, "", fd) {
            eprintln!("network: {e}");
            #[cfg(target_os = "android")]
            crate::vpn::down(&app);
        }
    });
}

pub fn stop() {
    unsafe { commonty_net_stop() };
    directory::via(None);
}
