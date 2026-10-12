//! The app keeps running with its window closed: the pages that hold keys
//! (local.rs) are served from here, and a browser sent to them while the
//! app is not running finds nothing. So closing the window hides it, the
//! tray's Quit is what ends it, and it starts with the session, hidden.
//!
//! Linux: an XDG autostart entry. Windows: the per-user Run key. Android:
//! a foreground service, started with the app and at boot
//! (gen/android/.../KeepRunning.kt). One setting, a file in the data
//! folder the Android side reads too, turns starting by itself off.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Runtime};

/// what the login entry starts the app with: no window, the tray alone
#[cfg(desktop)]
pub const FLAG: &str = "--background";

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
    serde_json::json!({ "on": wanted(), "can": cfg!(any(target_os = "linux", windows, target_os = "android")) })
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
        let at = xdg::path().ok_or("no home folder")?;
        if on {
            let exe = xdg::exe(app).ok_or("where this app is")?;
            if let Some(dir) = at.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&at, xdg::entry(&exe)).map_err(|e| e.to_string())?;
        } else if at.exists() {
            std::fs::remove_file(&at).map_err(|e| e.to_string())?;
        }
    }
    #[cfg(windows)]
    {
        let _ = app;
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            run_key::set(&format!("\"{}\" {FLAG}", exe.display()))?;
        } else {
            run_key::clear();
        }
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager as _;
        let k = app.try_state::<Keep<R>>().ok_or("no background plugin")?;
        k.0.run_mobile_plugin::<serde_json::Value>("set", serde_json::json!({ "on": on }))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "android")))]
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
    pub fn exe<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<String> {
        use tauri::Manager as _;
        if let Some(a) = app.env().appimage {
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

    pub fn entry(exe: &str) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName=Commonty\nComment=Keeps your Commonty pages ready for your browser\nExec={} {}\nIcon=commonty\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            quote(exe),
            super::FLAG
        )
    }

    /// an Exec argument, quoted as the desktop entry spec says, then
    /// escaped once more as the file's string values are
    fn quote(arg: &str) -> String {
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
            let e = super::entry("/usr/bin/commonty");
            assert!(e.contains("\nExec=\"/usr/bin/commonty\" --background\n"));
            assert!(e.starts_with("[Desktop Entry]\n"));
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
