//! The Commonty app: the shell around the same crates `dd` is made of. This
//! process holds one device key in the OS keystore, reads the member's
//! entry from the directories, and shows its pages from `web/`. The pages
//! call the commands below; nothing else reaches them.

mod account;
mod background;
#[cfg(desktop)]
mod boxsetup;
mod browser;
mod controllers;
mod export;
mod handoff;
#[cfg(desktop)]
mod installer;
mod local;
mod net;
mod paths;
mod photos;
mod site;
mod smarthome;
mod vpn;
#[cfg(desktop)]
mod wifi;
#[cfg(desktop)]
mod writer;
mod zip;

/// the keystore this app keeps its device key in: its own, so an app beside
/// `dd` on one machine is a device of its own
const SERVICE: &str = "commonty-app";

/// The version people downloaded: the release tag's, which the release
/// build hands over (COMMONTY_VERSION); a build from a checkout says the
/// workspace's
pub fn version() -> &'static str {
    option_env!("COMMONTY_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[cfg(target_os = "android")]
use tauri::Manager as _;

/// commonty:// links: the site's pages that hold a key (Files, Movies & TV,
/// Chat, Settings), Photos and Passwords, in a browser, hand over to the
/// app, which opens that browser at its own copy (local.rs). An AppImage is
/// installed by no one, so it says it takes them itself.
fn links(app: &tauri::AppHandle) {
    #[cfg(target_os = "linux")]
    use tauri::Manager as _;
    use tauri_plugin_deep_link::DeepLinkExt as _;
    #[cfg(target_os = "linux")]
    if app.env().appimage.is_some()
        && let Err(e) = app.deep_link().register_all()
    {
        eprintln!("links: {e}");
    }
    let h = app.clone();
    app.deep_link().on_open_url(move |e| {
        for u in e.urls() {
            local::open_link(&h, u.as_str());
        }
    });
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        for u in urls {
            local::open_link(app, u.as_str());
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // what the Devices tab calls this device's requests
    directory::set_agent(format!(
        "Commonty-app/{} ({})",
        version(),
        std::env::consts::OS
    ));
    let builder = tauri::Builder::default();
    #[cfg(not(target_os = "android"))]
    let builder = builder.manage(account::Keys(auth::open(SERVICE)));
    // a commonty:// link opening a second copy hands its link to this one
    // (the deep-link feature), and a plain second launch shows this window
    #[cfg(any(target_os = "linux", windows))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _| {
        if !argv
            .iter()
            .any(|a| a.starts_with("commonty://") || a == background::FLAG)
        {
            background::show(app);
        }
    }));
    // closing the window hides it: the app goes on serving the browser
    // until the tray's Quit
    #[cfg(desktop)]
    let builder = builder.on_window_event(|w, e| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = e {
            api.prevent_close();
            let _ = w.hide();
        }
    });
    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(vpn::init())
        .plugin(background::init())
        .plugin(tauri_plugin_opener::init())
        // the site's own pages, carried in the app and served to itself
        .register_asynchronous_uri_scheme_protocol(site::SCHEME, site::handle)
        .invoke_handler(tauri::generate_handler![
            account::status,
            account::set_name,
            account::join_with_code,
            account::sign_up,
            account::age_refused,
            account::refuse_age,
            account::open_policy,
            account::passkey_add,
            account::recover,
            account::forget,
            net::net_status,
            net::net_join,
            local::browsers,
            local::browser_forget,
            background::background_status,
            background::background_set
        ])
        .setup(|app| {
            paths::init(app.handle());
            // the window is made hidden (tauri.conf.json): a start at login
            // stays in the tray, and a link shows it at the page it needs
            #[cfg(desktop)]
            {
                background::tray(app.handle());
                if !background::hidden_start() {
                    background::show(app.handle());
                }
            }
            background::at_launch(app.handle());
            // what holds keys in a page, served to the app's windows and the
            // member's browser from its own release (local.rs)
            local::start(app.handle());
            links(app.handle());
            // Smart home, the same way (smarthome.rs)
            smarthome::start(app.handle());
            // GTK draws the title bar itself on Wayland, with the buttons its
            // settings name. Built with Nix, the app brings GNOME's own
            // default - a close button alone - and never sees the one the
            // desktop was set up with, so minimize and maximize were missing.
            #[cfg(target_os = "linux")]
            {
                use gtk::prelude::*;
                if let Some(s) = gtk::Settings::default() {
                    s.set_gtk_decoration_layout(Some(":minimize,maximize,close"));
                }
            }
            // on Android the keys are a file in the app's private storage
            #[cfg(target_os = "android")]
            app.manage(account::Keys(auth::open_file(
                paths::data().join("keys.json"),
            )));
            // the directory is members' to read: every read carries this
            // device's token once it has a name and a key
            let h = app.handle().clone();
            directory::read_with(Some(std::sync::Arc::new(move |host: &str| {
                ::account::read_token(&tauri::Manager::try_state::<account::Keys>(&h)?.0, host)
            })));
            // on the network from the start, if this device has joined before
            net::resume(app.handle(), &crate::account::control_url());
            // the keys this device holds, put where its other devices find them
            site::at_launch(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("the app window")
        .run(|_, event| {
            if let tauri::RunEvent::Exit = event {
                // the browser goes straight out while the app is closed; the
                // next start points it here again
                net::forget_browser();
                net::stop();
            }
        });
}
