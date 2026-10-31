//! The fleet's own network from this device: the engine is Go (app/net,
//! Tailscale's tsnet) behind four C calls, and what it gives back is a
//! loopback proxy into the network. Everything the app asks a box is then
//! asked through that proxy, by the box's name, straight to the box.

use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

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
    gone: bool,
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
    /// the control server no longer knows this device: it joins again
    #[serde(default)]
    pub gone: bool,
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
    rules: String,
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
        // Linux: the rules from a file beside the app's state, not from
        // the app. A browser holding the setting always finds them, and
        // they send it straight out when the app is not there, however
        // the app stopped. Elsewhere the engine's own address.
        let pac = match rules_file(&door.rules) {
            Some(f) => f,
            None => door.pac,
        };
        let by_hand = crate::browser::register(&pac);
        if let Some(m) = &by_hand {
            eprintln!("network: {m}");
        }
        if let Ok(mut b) = BROWSER.lock() {
            *b = (pac, by_hand);
        }
    }
}

/// the rules written where the system can read them without the app
fn rules_file(rules: &str) -> Option<String> {
    if !cfg!(target_os = "linux") || rules.is_empty() {
        return None;
    }
    let dir = state_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let at = dir.join("proxy.pac");
    let tmp = dir.join("proxy.pac.new");
    std::fs::write(&tmp, rules).ok()?;
    std::fs::rename(&tmp, &at).ok()?;
    Some(format!("file://{}", at.display()))
}

/// signing out on this device, or the app closing: the browser goes
/// straight out again
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
        c(&crate::paths::device_name())?,
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
    if st.gone {
        GONE.store(true, Ordering::SeqCst);
    }
    if !st.error.is_empty() {
        return Err(st.error);
    }
    GONE.store(false, Ordering::SeqCst);
    WANT.store(false, Ordering::SeqCst);
    eprintln!("network: on as {}, proxy at {}", st.ip, st.proxy);
    // joined for real: the next start resumes from here without a key
    let _ = std::fs::write(&marker, st.ip.as_bytes());
    if std::env::var_os("COMMONTY_NET_DEBUG").is_some() {
        eprintln!("network: proxy credential {}", st.credential);
    }
    let p = directory::Proxy {
        addr: st.proxy.clone(),
        user: "tsnet".to_string(),
        password: st.credential.clone(),
    };
    if let Ok(mut l) = PROXY.lock() {
        *l = Some(p.clone());
    }
    directory::via(Some(p));
    open_browser();
    Ok(st)
}

/// the control server said this device is not on the network any more
static GONE: AtomicBool = AtomicBool::new(false);

/// signed in, and not joined: the app was started with no state on disk
static WANT: AtomicBool = AtomicBool::new(false);

/// whether this device belongs on the network, so its calls to the boxes
/// go nowhere else: some services answer only there (Passwords' sign-in
/// refresh, outside it, is turned away and signs the person out)
pub fn wanted() -> bool {
    joined() || GONE.load(Ordering::SeqCst)
}

pub const NOT_ON_NETWORK: &str =
    "this device is not on your boxes' network right now; it is joining again";

/// the engine's proxy while it is up, to point clients at again after a
/// call found the engine down and went straight out
static PROXY: std::sync::Mutex<Option<directory::Proxy>> = std::sync::Mutex::new(None);

fn status_now() -> NetStatus {
    let out = take(unsafe { commonty_net_status() });
    let mut st: NetStatus = serde_json::from_str(&out).unwrap_or_default();
    if GONE.load(Ordering::SeqCst) && !st.running {
        st.gone = true;
    }
    if st.gone && st.error.is_empty() {
        st.error = NOT_ON_NETWORK.to_string();
    }
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
    // a device the network forgot: its state is worth nothing now
    if status_now().gone {
        forget_node(&app);
    }
    join(&app, &keys).await?;
    Ok(status_now())
}

/// one join at a time: Reconnect and the watcher's rejoin would each make a node
static JOINING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn join(app: &tauri::AppHandle, keys: &Keys) -> Result<(), String> {
    let _one = JOINING.lock().await;
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
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let app = handle;
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
    watch(app);
    Ok(())
}

/// a node the network forgot: off it, calls straight out at once, and its
/// state and marker gone so the next start is a join and not a resume
fn forget_node(app: &tauri::AppHandle) {
    stop();
    #[cfg(target_os = "android")]
    crate::vpn::down(app);
    let _ = app;
    GONE.store(true, Ordering::SeqCst);
    if let Ok(d) = state_dir() {
        clear_state(&d);
    }
}

fn clear_state(dir: &Path) {
    for f in ["tailscaled.state", "joined"] {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

/// the gate's door makes a node per join: after a failure, a minute, then
/// longer each time, to half an hour
const FIRST_WAIT: Duration = Duration::from_secs(60);
const LONGEST_WAIT: Duration = Duration::from_secs(30 * 60);
const LOOK_EVERY: Duration = Duration::from_secs(15);

/// One look at the engine. A forgotten node: off the network and joined
/// again; a device that wants on and is not: joined. The wait before the
/// next look, longer after a failed join.
fn step(
    st: &NetStatus,
    want: bool,
    wait: Duration,
    forget: impl FnOnce(),
    rejoin: impl FnOnce() -> Result<(), String>,
) -> Duration {
    if st.gone {
        forget();
    } else if !want || st.running {
        return FIRST_WAIT;
    }
    match rejoin() {
        Ok(()) => {
            eprintln!("network: joined again");
            FIRST_WAIT
        }
        Err(e) => {
            eprintln!("network: joining again: {e}; next in {}s", wait.as_secs());
            std::thread::sleep(wait);
            (wait * 2).min(LONGEST_WAIT)
        }
    }
}

/// the engine looked at while the app runs: a node the control server
/// forgot is joined again, and the proxy is put back once the engine is
/// running again after a call went around it (local::send)
fn watch(app: &tauri::AppHandle) {
    static ON: AtomicBool = AtomicBool::new(false);
    if ON.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let mut wait = FIRST_WAIT;
        loop {
            std::thread::sleep(LOOK_EVERY);
            let st = status_now();
            if st.running
                && directory::proxy().is_none()
                && let Some(p) = PROXY.lock().ok().and_then(|p| p.clone())
            {
                directory::via(Some(p));
            }
            wait = step(
                &st,
                WANT.load(Ordering::SeqCst),
                wait,
                || forget_node(&app),
                || {
                    let keys = app
                        .try_state::<Keys>()
                        .ok_or_else(|| "no keys".to_string())?;
                    tauri::async_runtime::block_on(join(&app, &keys))
                },
            );
        }
    });
}

/// a call found no way through the proxy: if the engine is not running,
/// the proxy refuses everything, so it is dropped and calls say the device
/// is off the network (local::http) until the engine is back (the watcher
/// puts the proxy back) or joined again
pub fn drop_if_down() -> bool {
    if directory::proxy().is_none() || status_now().running {
        return false;
    }
    eprintln!("network: the engine is not running; calls wait for it");
    directory::via(None);
    true
}

/// resume from the state on disk, at app start; nothing if never joined
pub fn resume(app: &tauri::AppHandle, control: &str) {
    let Ok(d) = state_dir() else { return };
    // the marker holds this node's address, which the phone's VPN needs
    let Ok(ip) = std::fs::read_to_string(d.join("joined")) else {
        // no state, but signed in: a device whose state was cleared (or
        // set aside by hand) joins again rather than staying off
        if app
            .try_state::<Keys>()
            .is_some_and(|k| media::gate::Opener::load(&k.0).is_ok())
        {
            WANT.store(true, Ordering::SeqCst);
            watch(app);
        }
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
        // and from here on, a node the control server forgets joins again
        watch(&app);
    });
}

/// whether this device has joined the network, so comes up on it at start
pub fn joined() -> bool {
    state_dir().is_ok_and(|d| d.join("joined").exists())
}

pub fn stop() {
    unsafe { commonty_net_stop() };
    if let Ok(mut p) = PROXY.lock() {
        *p = None;
    }
    directory::via(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn gone() -> NetStatus {
        NetStatus {
            gone: true,
            ..Default::default()
        }
    }

    // a node the control server forgot comes off the network before it
    // joins again, and a failed join waits longer each time, to a cap
    #[test]
    fn forgotten_node_comes_off_then_joins_again() {
        let calls = RefCell::new(Vec::new());
        let ok = NetStatus {
            running: true,
            ..Default::default()
        };
        let w = step(
            &ok,
            true,
            FIRST_WAIT,
            || calls.borrow_mut().push("off"),
            || Ok(()),
        );
        assert!(calls.borrow().is_empty(), "a running node is left alone");
        assert_eq!(w, FIRST_WAIT);

        let mut wait = Duration::from_millis(1);
        for _ in 0..2 {
            wait = step(
                &gone(),
                false,
                wait,
                || calls.borrow_mut().push("off"),
                || {
                    calls.borrow_mut().push("join");
                    Err("the gate is away".to_string())
                },
            );
        }
        assert_eq!(*calls.borrow(), ["off", "join", "off", "join"]);
        assert_eq!(wait, Duration::from_millis(4));
        let w = step(&gone(), false, Duration::from_millis(1), || {}, || Ok(()));
        assert_eq!(
            w, FIRST_WAIT,
            "joined: the next failure waits the least again"
        );
        assert_eq!((LONGEST_WAIT * 2).min(LONGEST_WAIT), LONGEST_WAIT);
    }

    // signed in with no state on disk (cleared, or set aside by hand): it
    // joins, and nothing of an old node is taken off first
    #[test]
    fn a_device_off_the_network_joins() {
        let calls = RefCell::new(Vec::new());
        let off = NetStatus::default();
        step(
            &off,
            true,
            Duration::from_millis(1),
            || calls.borrow_mut().push("off"),
            || {
                calls.borrow_mut().push("join");
                Ok(())
            },
        );
        assert_eq!(*calls.borrow(), ["join"]);
        calls.borrow_mut().clear();
        step(
            &off,
            false,
            Duration::from_millis(1),
            || calls.borrow_mut().push("off"),
            || {
                calls.borrow_mut().push("join");
                Ok(())
            },
        );
        assert!(calls.borrow().is_empty(), "a device never joined stays off");
    }

    #[test]
    fn clearing_takes_the_state_and_marker_only() {
        let dir = std::env::temp_dir().join(format!("commonty-net-clear-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["tailscaled.state", "joined", "proxy.pac"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        clear_state(&dir);
        assert!(!dir.join("tailscaled.state").exists());
        assert!(!dir.join("joined").exists());
        assert!(dir.join("proxy.pac").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
