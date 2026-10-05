//! Where this app keeps its things: its data directory as the platform
//! gives it (a folder under the home on desktop, the app's private
//! storage on a phone). Set once at start, read anywhere.

use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(not(target_os = "linux"))]
use tauri::Manager as _;

static DATA: OnceLock<PathBuf> = OnceLock::new();

pub fn init(app: &tauri::AppHandle) {
    // Linux: the folder dd's things sit next to. Anywhere else, what the
    // platform gives the app (AppData on Windows, the app's private
    // storage on a phone): Windows has no HOME, and the app's state once
    // fell through to the temp folder there, which gets cleaned out.
    #[cfg(target_os = "linux")]
    let dir = {
        let _ = app;
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".local/share/commonty"))
            .unwrap_or_else(|_| std::env::temp_dir().join("commonty"))
    };
    #[cfg(not(target_os = "linux"))]
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("commonty"));
    let _ = std::fs::create_dir_all(&dir);
    // what an earlier version left in the temp folder comes along once
    #[cfg(target_os = "windows")]
    {
        let old = std::env::temp_dir().join("commonty").join("net");
        let new = dir.join("net");
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
        }
    }
    let _ = DATA.set(dir);
}

/// This device's name on the network: the machine's own where there is
/// one, and a generic one with a few letters of its own where there is
/// not (a phone), so two phones are not both "phone". Made once and kept.
pub fn device_name() -> String {
    let at = data().join("device-name");
    if let Some(n) = std::fs::read_to_string(&at)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return n;
    }
    let machine = if cfg!(target_os = "windows") {
        std::env::var("COMPUTERNAME").ok()
    } else if cfg!(target_os = "android") {
        None
    } else {
        std::fs::read_to_string("/etc/hostname").ok()
    };
    let name = match machine.as_deref().map(clean).filter(|s| !s.is_empty()) {
        Some(n) => n,
        None => {
            let mut b = [0u8; 2];
            let _ = getrandom::fill(&mut b);
            let kind = if cfg!(target_os = "android") {
                "phone"
            } else {
                "device"
            };
            format!("{kind}-{:02x}{:02x}", b[0], b[1])
        }
    };
    let _ = std::fs::write(&at, &name);
    name
}

/// a name the network takes: lowercase letters, digits and dashes
fn clean(s: &str) -> String {
    let lower: String = s
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    lower.trim_matches('-').chars().take(40).collect()
}

/// the app's data directory; before init, a temp folder
pub fn data() -> PathBuf {
    DATA.get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("commonty"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_machine_name_the_network_takes() {
        assert_eq!(super::clean("DESKTOP-ABC123\r\n"), "desktop-abc123");
        assert_eq!(super::clean("David's Laptop"), "david-s-laptop");
        assert_eq!(super::clean("  "), "");
    }
}
