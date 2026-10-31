//! The app keeps running with its window closed: the pages that hold keys
//! (local.rs) are served from here, and a browser sent to them while the
//! app is not running finds nothing. So closing the window hides it, the
//! tray's Quit is what ends it, and it starts with the session, hidden.
//!
//! It comes back by itself, too: killed or crashed, it is started again.
//! Linux: a systemd user unit (Restart=always, but not after Quit), with
//! an XDG autostart entry for a session without systemd. Windows: the
//! per-user Run key, starting a small keeper (`--keep`) that starts the
//! app again when it did not end by Quit. macOS: a LaunchAgent with
//! KeepAlive. Android: a foreground service, started with the app and at
//! boot (gen/android/.../KeepRunning.kt). One setting, a file in the data
//! folder the Android side reads too, turns starting by itself off.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Runtime};

/// what the login entry starts the app with: no window, the tray alone
#[cfg(desktop)]
pub const FLAG: &str = "--background";

/// Windows' login entry: the keeper, which starts the app with FLAG and
/// starts it again until it ends by Quit
#[cfg(windows)]
pub const KEEP: &str = "--keep";

/// The keeper (Windows, `--keep`): no window and no Tauri of its own, only
/// the app as a child, started again when it ended any way but Quit (exit
/// 0) - a crash, or Task Manager's End task. Five ends in a minute is a
/// fault the next start will not mend, so it gives up until the next login.
#[cfg(windows)]
pub fn keep() -> bool {
    if !std::env::args().skip(1).any(|a| a == KEEP) {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return true;
    };
    let mut ends: Vec<std::time::Instant> = vec![];
    loop {
        let status = std::process::Command::new(&exe).arg(FLAG).status();
        if status.as_ref().is_ok_and(|s| s.success()) {
            return true;
        }
        let now = std::time::Instant::now();
        ends.retain(|t| now.duration_since(*t) < std::time::Duration::from_secs(60));
        ends.push(now);
        if ends.len() >= 5 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

/// Started any other way than by its unit (from the menu, a link, an
/// autostart entry the session read itself) while the unit is there for
/// this very program: the unit is started first, and it is that copy that
/// runs - the one systemd starts again after a crash. This copy then hands
/// its arguments to it (single-instance: a link, or "show the window") and
/// ends. The unit is Type=dbus on single-instance's own name, so `start`
/// returns once that copy can take them.
#[cfg(target_os = "linux")]
pub fn into_unit() {
    if cfg!(debug_assertions) || std::env::var_os(unit::MARK).is_some() || !wanted() {
        return;
    }
    let Some(exe) = xdg::exe(std::env::var_os("APPIMAGE").as_deref()) else {
        return;
    };
    if !unit::running() || !unit::names(&exe) {
        return;
    }
    unit::start();
}

const SETTING: &str = "start-at-login";

/// started at login, or by a commonty:// link (which shows the window
/// itself when it needs the member's click)
#[cfg(desktop)]
pub fn hidden_start() -> bool {
    std::env::args()
        .skip(1)
        .any(|a| a == FLAG || a.starts_with("commonty://"))
}

/// starting by itself is on unless the member turned it off
pub fn wanted() -> bool {
    std::fs::read_to_string(crate::paths::data().join(SETTING)).map_or(true, |s| s.trim() != "off")
}

/// the login entry as the setting says, at every start: the path it names
/// moves with an update or an AppImage moved
pub fn at_launch<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = apply(app, wanted()) {
        eprintln!("background: {e}");
    }
}

#[tauri::command]
pub fn background_status() -> serde_json::Value {
    serde_json::json!({ "on": wanted(), "can": cfg!(any(target_os = "linux", windows, target_os = "macos", target_os = "android")) })
}

#[tauri::command]
pub fn background_set<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<(), String> {
    std::fs::write(
        crate::paths::data().join(SETTING),
        if on { "on" } else { "off" },
    )
    .map_err(|e| e.to_string())?;
    apply(&app, on)
}

fn apply<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use tauri::Manager as _;
        let at = xdg::path().ok_or("no home folder")?;
        let exe = xdg::exe(app.env().appimage.as_deref());
        // the unit, where there is a systemd for this account: it is what
        // starts the app at login and again after a crash, and the
        // autostart entry steps aside for it
        let unit = on && unit::running() && exe.is_some();
        if unit {
            unit::install(exe.as_deref().unwrap_or_default())?;
        } else {
            unit::remove();
        }
        if on {
            let exe = exe.ok_or("where this app is")?;
            if let Some(dir) = at.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&at, xdg::entry(&exe, unit)).map_err(|e| e.to_string())?;
        } else if at.exists() {
            std::fs::remove_file(&at).map_err(|e| e.to_string())?;
        }
    }
    #[cfg(windows)]
    {
        let _ = app;
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            run_key::set(&format!("\"{}\" {KEEP}", exe.display()))?;
        } else {
            run_key::clear();
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let at = agent::path().ok_or("no home folder")?;
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            if let Some(dir) = at.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&at, agent::plist(&exe.to_string_lossy())).map_err(|e| e.to_string())?;
        } else if at.exists() {
            std::fs::remove_file(&at).map_err(|e| e.to_string())?;
        }
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager as _;
        let k = app.try_state::<Keep<R>>().ok_or("no background plugin")?;
        k.0.run_mobile_plugin::<serde_json::Value>("set", serde_json::json!({ "on": on }))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(any(
        target_os = "linux",
        windows,
        target_os = "macos",
        target_os = "android"
    )))]
    let _ = (app, on);
    Ok(())
}

#[cfg(target_os = "android")]
pub struct Keep<R: Runtime>(tauri::plugin::PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("background")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager as _;
                let h = api.register_android_plugin("org.commonty.app", "BackgroundPlugin")?;
                app.manage(Keep(h));
            }
            let _ = (app, api);
            Ok(())
        })
        .build()
}

/// the tray's icon and its menu. Where the desktop has no tray library
/// (libayatana-appindicator on Linux) there is none, and opening the app
/// again is what brings its window back.
#[cfg(desktop)]
pub fn tray<R: Runtime>(app: &AppHandle<R>) {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let build = || -> tauri::Result<()> {
        let open = MenuItem::with_id(app, "open", "Open Commonty", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let mut t = TrayIconBuilder::with_id("main")
            .tooltip("Commonty")
            .menu(&Menu::with_items(app, &[&open, &quit])?)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, e| match e.id().as_ref() {
                "open" => show(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|t, e| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = e
                {
                    show(t.app_handle());
                }
            });
        if let Some(i) = app.default_window_icon() {
            t = t.icon(i.clone());
        }
        t.build(app)?;
        Ok(())
    };
    // the Linux tray library is looked for when it is first used, and its
    // absence is a panic, not an error
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("tray: {e}"),
        Err(_) => eprintln!("tray: no tray on this desktop"),
    }
}

#[cfg(desktop)]
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    use tauri::Manager as _;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(target_os = "linux")]
mod xdg {
    use std::path::PathBuf;

    pub fn path() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(config.join("autostart").join("commonty.desktop"))
    }

    /// The AppImage's own file, not where it is mounted this time; else the
    /// path this was started by, when it is one (a Nix wrapper's, which
    /// sets what the webview needs); else the program itself
    pub fn exe(appimage: Option<&std::ffi::OsStr>) -> Option<String> {
        if let Some(a) = appimage {
            return Some(a.to_string_lossy().into_owned());
        }
        let argv0 = std::env::args_os().next().map(PathBuf::from);
        if let Some(p) = argv0.filter(|p| p.is_absolute() && p.is_file()) {
            return Some(p.to_string_lossy().into_owned());
        }
        std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// With the unit in place, a session that turns autostart entries into
    /// units of its own (systemd's generator) leaves this one to it; one
    /// that reads the entry itself starts the app, which hands over to the
    /// unit (super::into_unit)
    pub fn entry(exe: &str, unit: bool) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName=Commonty\nComment=Keeps your Commonty pages ready for your browser\nExec={} {}\nIcon=commonty\nTerminal=false\nX-GNOME-Autostart-enabled=true\n{}",
            quote(exe),
            super::FLAG,
            if unit { "X-systemd-skip=true\n" } else { "" }
        )
    }

    /// an Exec argument, quoted as the desktop entry spec says, then
    /// escaped once more as the file's string values are
    pub(super) fn quote(arg: &str) -> String {
        let mut q = String::from("\"");
        for c in arg.chars() {
            match c {
                '"' | '`' | '$' | '\\' => {
                    q.push('\\');
                    q.push(c);
                }
                '%' => q.push_str("%%"),
                _ => q.push(c),
            }
        }
        q.push('"');
        q.replace('\\', "\\\\")
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_login_entry_starts_this_program_hidden() {
            let e = super::entry("/usr/bin/commonty", false);
            assert!(e.contains("\nExec=\"/usr/bin/commonty\" --background\n"));
            assert!(e.starts_with("[Desktop Entry]\n"));
            assert!(!e.contains("X-systemd-skip"));
            let e = super::entry("/usr/bin/commonty", true);
            assert!(
                e.ends_with("\nX-systemd-skip=true\n"),
                "the unit starts it: {e}"
            );
        }

        #[test]
        fn a_path_with_odd_characters_stays_one_argument() {
            assert_eq!(
                super::quote("/home/a b/Commonty 100%.AppImage"),
                "\"/home/a b/Commonty 100%%.AppImage\""
            );
            assert_eq!(
                super::quote("/x/\"$y`\\z"),
                "\"/x/\\\\\"\\\\$y\\\\`\\\\\\\\z\""
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod unit {
    use std::path::PathBuf;
    use std::process::Command;

    pub const NAME: &str = "commonty.service";
    /// set in the unit's environment: this copy is the unit's own
    pub const MARK: &str = "COMMONTY_UNIT";
    /// single-instance's name on the session bus (tauri.conf.json's identifier)
    const BUS: &str = "org.commonty.app.SingleInstance";
    /// what the session hands the app, for a unit started from inside it
    const SESSION: [&str; 5] = [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
    ];

    fn dir() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(config.join("systemd").join("user"))
    }

    fn wants() -> Option<PathBuf> {
        Some(dir()?.join("graphical-session.target.wants").join(NAME))
    }

    /// a systemd for this account, running now
    pub fn running() -> bool {
        std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|d| PathBuf::from(d).join("systemd").join("private").exists())
    }

    fn systemctl(args: &[&str]) -> bool {
        Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Restart=always, but not after an exit of 0 (Quit, or a second copy
    /// handing over); a stop - logging out, turning the setting off - is
    /// never restarted either. Five starts in a minute is a fault another
    /// start will not mend.
    pub fn text(exe: &str) -> String {
        format!(
            "[Unit]\n\
Description=Commonty, keeping your pages ready for your browser\n\
PartOf=graphical-session.target\n\
After=graphical-session.target\n\
StartLimitIntervalSec=60\n\
StartLimitBurst=5\n\
\n\
[Service]\n\
Type=dbus\n\
BusName={BUS}\n\
ExecStart={} {}\n\
Environment={MARK}=1\n\
Restart=always\n\
RestartPreventExitStatus=0\n\
RestartSec=2\n\
TimeoutStartSec=30\n\
\n\
[Install]\n\
WantedBy=graphical-session.target\n",
            quote(exe),
            super::FLAG
        )
    }

    /// whether the unit on disk starts this very program
    pub fn names(exe: &str) -> bool {
        dir()
            .and_then(|d| std::fs::read_to_string(d.join(NAME)).ok())
            .is_some_and(|t| t == text(exe))
    }

    /// written and enabled as `systemctl enable` would, by hand: no call
    /// and no wait at every start when nothing changed
    pub fn install(exe: &str) -> Result<(), String> {
        let d = dir().ok_or("no home folder")?;
        let w = wants().ok_or("no home folder")?;
        let changed = !names(exe);
        if changed {
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            std::fs::write(d.join(NAME), text(exe)).map_err(|e| e.to_string())?;
        }
        if std::fs::read_link(&w).ok() != Some(d.join(NAME)) {
            if let Some(p) = w.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let _ = std::fs::remove_file(&w);
            std::os::unix::fs::symlink(d.join(NAME), &w).map_err(|e| e.to_string())?;
        }
        if changed {
            systemctl(&["daemon-reload"]);
        }
        Ok(())
    }

    pub fn remove() {
        let (Some(d), Some(w)) = (dir(), wants()) else {
            return;
        };
        let had = d.join(NAME).exists();
        let _ = std::fs::remove_file(w);
        let _ = std::fs::remove_file(d.join(NAME));
        if had {
            systemctl(&["daemon-reload"]);
        }
    }

    /// The session's display, handed to the user's systemd (as GNOME does
    /// at login), then the unit started
    pub fn start() {
        let set: Vec<&str> = SESSION
            .into_iter()
            .filter(|v| std::env::var_os(v).is_some())
            .collect();
        if !set.is_empty() {
            let mut a = vec!["import-environment"];
            a.extend(set);
            systemctl(&a);
        }
        if !systemctl(&["start", NAME]) {
            eprintln!("background: the unit did not start, running here");
        }
    }

    /// an ExecStart argument: quoted, with systemd's specifiers and
    /// variables taken as themselves
    fn quote(arg: &str) -> String {
        let mut q = String::from("\"");
        for c in arg.chars() {
            match c {
                '"' | '\\' => {
                    q.push('\\');
                    q.push(c);
                }
                '%' => q.push_str("%%"),
                '$' => q.push_str("$$"),
                _ => q.push(c),
            }
        }
        q.push('"');
        q
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_unit_comes_back_after_a_crash_but_not_after_quit() {
            let t = super::text("/usr/bin/commonty");
            for want in [
                "\nExecStart=\"/usr/bin/commonty\" --background\n",
                "\nRestart=always\n",
                "\nRestartPreventExitStatus=0\n",
                "\nType=dbus\n",
                "\nBusName=org.commonty.app.SingleInstance\n",
                "\nEnvironment=COMMONTY_UNIT=1\n",
                "\nWantedBy=graphical-session.target\n",
                "\nPartOf=graphical-session.target\n",
            ] {
                assert!(t.contains(want), "{want:?} in {t}");
            }
        }

        #[test]
        fn a_path_with_odd_characters_stays_one_program() {
            assert_eq!(
                super::quote("/home/a b/Commonty 100%$x\"\\.AppImage"),
                "\"/home/a b/Commonty 100%%$$x\\\"\\\\.AppImage\""
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod agent {
    use std::path::PathBuf;

    pub fn path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(home.join("Library/LaunchAgents/org.commonty.app.plist"))
    }

    fn xml(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// started at login, hidden, and again when it ends by anything but
    /// Quit (exit 0)
    pub fn plist(exe: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\"><dict>\n\
<key>Label</key><string>org.commonty.app</string>\n\
<key>ProgramArguments</key><array><string>{}</string><string>{}</string></array>\n\
<key>RunAtLoad</key><true/>\n\
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n\
<key>ProcessType</key><string>Interactive</string>\n\
</dict></plist>\n",
            xml(exe),
            super::FLAG
        )
    }
}

#[cfg(windows)]
mod run_key {
    use std::os::windows::process::CommandExt as _;
    use std::process::Command;

    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    // no console window flashing up
    const NO_WINDOW: u32 = 0x0800_0000;

    fn reg(args: &[&str]) -> Option<std::process::Output> {
        Command::new("reg")
            .args(args)
            .creation_flags(NO_WINDOW)
            .output()
            .ok()
    }

    pub fn set(command: &str) -> Result<(), String> {
        reg(&[
            "add", KEY, "/v", "Commonty", "/t", "REG_SZ", "/d", command, "/f",
        ])
        .filter(|o| o.status.success())
        .map(|_| ())
        .ok_or_else(|| "the login entry could not be written".to_string())
    }

    pub fn clear() {
        let _ = reg(&["delete", KEY, "/v", "Commonty", "/f"]);
    }
}
