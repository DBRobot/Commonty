//! The browser on this machine, sent to the fleet for the fleet's names:
//! the engine serves proxy rules (app/net/browser.go) and this registers
//! them with the system, so a browser asking for files.<domain> goes
//! through this device's place on the network and everything else goes
//! straight out. A proxy the person set themselves is never replaced: then
//! the rules' address is shown for them to add by hand.
//!
//! Windows: the per-user Internet Settings every browser there reads.
//! Linux: GNOME's proxy setting, which Chrome and Firefox follow.
//! Android has no such setting; the phone gets a VPN instead.

/// Some(message) when the browser could not be pointed here by itself
pub fn register(pac: &str) -> Option<String> {
    let by_hand = || {
        Some(format!(
            "set your browser's automatic proxy configuration to {pac} to open the fleet's pages there"
        ))
    };
    #[cfg(target_os = "windows")]
    {
        match windows::current() {
            Some(url) if url != pac => by_hand(),
            _ => windows::set(pac).err().and_then(|_| by_hand()),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match gnome::current() {
            Some((mode, url)) if mode == "none" || (mode == "auto" && url == pac) => {
                gnome::set(pac).err().and_then(|_| by_hand())
            }
            _ => by_hand(),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = by_hand;
        None
    }
}

/// undo it, if it is still ours (signing out on this device, or the app
/// closing)
pub fn unregister(pac: &str) {
    #[cfg(target_os = "windows")]
    if windows::current().as_deref() == Some(pac) {
        windows::clear();
    }
    #[cfg(target_os = "linux")]
    if matches!(gnome::current(), Some((m, u)) if m == "auto" && u == pac) {
        gnome::clear();
    }
    let _ = pac;
}

#[cfg(target_os = "linux")]
mod gnome {
    use std::process::Command;

    const SCHEMA: &str = "org.gnome.system.proxy";

    fn get(key: &str) -> Option<String> {
        let o = Command::new("gsettings")
            .args(["get", SCHEMA, key])
            .output()
            .ok()?;
        o.status.success().then(|| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .trim_matches('\'')
                .to_string()
        })
    }

    fn put(key: &str, value: &str) -> Result<(), ()> {
        Command::new("gsettings")
            .args(["set", SCHEMA, key, value])
            .status()
            .ok()
            .filter(|s| s.success())
            .map(|_| ())
            .ok_or(())
    }

    /// (mode, autoconfig url), or None where there is no GNOME
    pub fn current() -> Option<(String, String)> {
        Some((get("mode")?, get("autoconfig-url")?))
    }

    pub fn set(pac: &str) -> Result<(), ()> {
        put("autoconfig-url", pac)?;
        put("mode", "auto")
    }

    pub fn clear() {
        let _ = put("mode", "none");
        let _ = put("autoconfig-url", "");
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::os::windows::process::CommandExt as _;
    use std::process::Command;

    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    // no console window flashing up
    const NO_WINDOW: u32 = 0x0800_0000;

    fn reg(args: &[&str]) -> Option<std::process::Output> {
        Command::new("reg")
            .args(args)
            .creation_flags(NO_WINDOW)
            .output()
            .ok()
    }

    /// the rules url set now; None when there is none
    pub fn current() -> Option<String> {
        let o = reg(&["query", KEY, "/v", "AutoConfigURL"])?;
        if !o.status.success() {
            return None;
        }
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .find(|l| l.contains("AutoConfigURL"))
            .and_then(|l| l.split_whitespace().last())
            .map(str::to_string)
    }

    pub fn set(pac: &str) -> Result<(), ()> {
        reg(&[
            "add",
            KEY,
            "/v",
            "AutoConfigURL",
            "/t",
            "REG_SZ",
            "/d",
            pac,
            "/f",
        ])
        .filter(|o| o.status.success())
        .map(|_| ())
        .ok_or(())
    }

    pub fn clear() {
        let _ = reg(&["delete", KEY, "/v", "AutoConfigURL", "/f"]);
    }
}
