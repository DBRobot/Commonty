//! The Commonty app: the shell around the same crates `dd` is made of. This
//! process holds one device key in the OS keystore, reads the member's
//! entry from the directories, and shows its pages from `web/`. The pages
//! call the commands below; nothing else reaches them.

mod account;
mod browser;
mod net;
mod paths;
mod photos;
mod site;
mod vpn;

/// the keystore this app keeps its device key in: its own, so an app beside
/// `dd` on one machine is a device of its own
const SERVICE: &str = "commonty-app";

#[cfg(target_os = "android")]
use tauri::Manager as _;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // what the Devices tab calls this device's requests
    directory::set_agent(format!(
        "Commonty-app/{} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    ));
    let builder = tauri::Builder::default();
    #[cfg(not(target_os = "android"))]
    let builder = builder.manage(account::Keys(auth::open(SERVICE)));
    builder
        .plugin(vpn::init())
        .plugin(tauri_plugin_opener::init())
        // the site's own pages, carried in the app and served to itself
        .register_asynchronous_uri_scheme_protocol(site::SCHEME, site::handle)
        .invoke_handler(tauri::generate_handler![
            account::status,
            account::set_name,
            account::sign_up,
            account::admit_device,
            account::passkey_add,
            account::remove_device,
            account::recover,
            account::forget,
            net::net_status,
            net::net_join
        ])
        .setup(|app| {
            paths::init(app.handle());
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
            // on the network from the start, if this device has joined before
            net::resume(app.handle(), &crate::account::control_url());
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
