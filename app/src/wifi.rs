//! This computer's Wi-Fi, for a box's installer drive (boxsetup.rs): the
//! network it is on now and its password, read the platform's way. The
//! password is handed to the drive and to nothing else; it is never logged.

use serde::Serialize;
use zeroize::Zeroizing;

#[derive(Serialize)]
pub struct Network {
    pub ssid: String,
    /// absent when the system would not say it (no rights, or a network
    /// with no password); the member types it then
    #[serde(skip)]
    pub psk: Option<Zeroizing<String>>,
}

/// The network this computer is on, if any is Wi-Fi.
pub async fn current() -> Option<Network> {
    tauri::async_runtime::spawn_blocking(read)
        .await
        .ok()
        .flatten()
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(target_os = "linux")]
fn read() -> Option<Network> {
    let active = run(
        "nmcli",
        &["-t", "-f", "NAME,TYPE", "connection", "show", "--active"],
    )?;
    let name = nm_wifi_connection(&active)?;
    // the ssid and, with the secret agent's leave, the password
    let fields = run(
        "nmcli",
        &[
            "--show-secrets",
            "-g",
            "802-11-wireless.ssid,802-11-wireless-security.psk",
            "connection",
            "show",
            &name,
        ],
    )?;
    nm_fields(&fields)
}

/// the first active connection of the wifi type, out of `nmcli -t`
#[cfg(any(target_os = "linux", test))]
fn nm_wifi_connection(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let (name, kind) = split_terse(l)?;
        (kind == "802-11-wireless").then_some(name)
    })
}

/// NAME:TYPE, where a colon in the name is written \:
#[cfg(any(target_os = "linux", test))]
fn split_terse(l: &str) -> Option<(String, String)> {
    let mut name = String::new();
    let mut chars = l.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => name.push(chars.next()?),
            ':' => return Some((name, chars.collect())),
            c => name.push(c),
        }
    }
    None
}

/// `-g ssid,psk`: one line each, the psk's empty when not said
#[cfg(any(target_os = "linux", test))]
fn nm_fields(out: &str) -> Option<Network> {
    let mut lines = out.lines();
    let ssid = lines.next()?.trim().to_string();
    if ssid.is_empty() {
        return None;
    }
    let psk = lines
        .next()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| Zeroizing::new(p.to_string()));
    Some(Network { ssid, psk })
}

#[cfg(target_os = "macos")]
fn read() -> Option<Network> {
    let ports = run("networksetup", &["-listallhardwareports"])?;
    let dev = mac_wifi_device(&ports)?;
    let ssid = run("networksetup", &["-getairportnetwork", &dev])
        .and_then(|o| mac_ssid(&o))
        .or_else(|| run("ipconfig", &["getsummary", &dev]).and_then(|o| mac_summary_ssid(&o)))?;
    // the keychain asks the person first; a no leaves it to be typed
    let psk = run(
        "security",
        &[
            "find-generic-password",
            "-D",
            "AirPort network password",
            "-w",
            "-a",
            &ssid,
        ],
    )
    .map(|p| p.trim().to_string())
    .filter(|p| !p.is_empty())
    .map(Zeroizing::new);
    Some(Network { ssid, psk })
}

/// the Wi-Fi port's device (en0) out of -listallhardwareports
#[cfg(any(target_os = "macos", test))]
fn mac_wifi_device(out: &str) -> Option<String> {
    let mut lines = out.lines();
    while let Some(l) = lines.next() {
        let port = l.trim().strip_prefix("Hardware Port:").map(str::trim);
        if matches!(port, Some("Wi-Fi" | "AirPort")) {
            return lines
                .next()?
                .trim()
                .strip_prefix("Device:")
                .map(|d| d.trim().to_string());
        }
    }
    None
}

/// "Current Wi-Fi Network: <ssid>"; anything else says it is not on one
#[cfg(any(target_os = "macos", test))]
fn mac_ssid(out: &str) -> Option<String> {
    out.trim()
        .strip_prefix("Current Wi-Fi Network:")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// newer macOS hides the above; `ipconfig getsummary` has "  SSID : <ssid>"
#[cfg(any(target_os = "macos", test))]
fn mac_summary_ssid(out: &str) -> Option<String> {
    out.lines()
        .find_map(|l| l.trim().strip_prefix("SSID : "))
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "<redacted>")
        .map(str::to_string)
}

#[cfg(windows)]
fn read() -> Option<Network> {
    let ifs = run("netsh", &["wlan", "show", "interfaces"])?;
    let (ssid, profile) = win_interface(&ifs)?;
    let psk = run(
        "netsh",
        &[
            "wlan",
            "show",
            "profile",
            &format!("name={profile}"),
            "key=clear",
        ],
    )
    .and_then(|o| win_key(&o))
    .map(Zeroizing::new);
    Some(Network { ssid, psk })
}

/// the connected interface's SSID (not BSSID) and profile
#[cfg(any(windows, test))]
fn win_interface(out: &str) -> Option<(String, String)> {
    let field = |name: &str| {
        out.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
    };
    let ssid = field("SSID").filter(|s| !s.is_empty())?;
    let profile = field("Profile").unwrap_or_else(|| ssid.clone());
    Some((ssid, profile))
}

#[cfg(any(windows, test))]
fn win_key(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == "Key Content")
            .then(|| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn read() -> Option<Network> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn networkmanager() {
        let active = "Wired connection 1:802-3-ethernet\nhome\\:5G:802-11-wireless\nlo:loopback\n";
        assert_eq!(nm_wifi_connection(active).as_deref(), Some("home:5G"));
        assert_eq!(nm_wifi_connection("lo:loopback\n"), None);
        let n = nm_fields("home 5G\nhunter22\n").unwrap();
        assert_eq!(n.ssid, "home 5G");
        assert_eq!(n.psk.as_deref().map(String::as_str), Some("hunter22"));
        // the agent said no, or an open network
        assert!(nm_fields("home 5G\n\n").unwrap().psk.is_none());
        assert!(nm_fields("\n").is_none());
    }

    #[test]
    fn macos() {
        let ports = "\nHardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: N/A\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa:bb\n";
        assert_eq!(mac_wifi_device(ports).as_deref(), Some("en0"));
        assert_eq!(
            mac_ssid("Current Wi-Fi Network: Home Net\n").as_deref(),
            Some("Home Net")
        );
        assert_eq!(
            mac_ssid("You are not associated with an AirPort network.\n"),
            None
        );
        let summary = "<dictionary> {\n  BSSID : aa:bb\n  SSID : Home Net\n}";
        assert_eq!(mac_summary_ssid(summary).as_deref(), Some("Home Net"));
        assert_eq!(mac_summary_ssid("  SSID : <redacted>\n"), None);
    }

    #[test]
    fn windows() {
        let ifs = "There is 1 interface on the system:\n\n    Name                   : Wi-Fi\n    State                  : connected\n    SSID                   : Home Net\n    BSSID                  : aa:bb:cc:dd:ee:ff\n    Profile                : Home Net 2\n";
        assert_eq!(
            win_interface(ifs),
            Some(("Home Net".to_string(), "Home Net 2".to_string()))
        );
        assert_eq!(
            win_interface("    State                  : disconnected\n"),
            None
        );
        let prof = "Security settings\n-----------------\n    Authentication         : WPA2-Personal\n    Key Content            : hunter22\n";
        assert_eq!(win_key(prof).as_deref(), Some("hunter22"));
        assert_eq!(win_key("    Security key           : Absent\n"), None);
    }
}
