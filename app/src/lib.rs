//! The Commonty app: the shell around the same crates `dd` is made of. This
//! process holds one device key in the OS keystore, reads the member's
//! entry from the directories, and shows its pages from `web/`. The pages
//! call the commands below; nothing else reaches them.

mod account;
mod browser;
#[cfg(not(target_os = "android"))]
mod media;
mod net;
mod paths;
mod site;
mod vpn;

/// the keystore this app keeps its device key in: its own, so an app beside
/// `dd` on one machine is a device of its own
const SERVICE: &str = "commonty-app";

#[cfg(target_os = "android")]
use tauri::Manager as _;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(not(target_os = "android"))]
    let builder = builder
        .manage(account::Keys(auth::open(SERVICE)))
        .manage(media::Media::default());
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
            media_status,
            media_open,
            media_close,
            net::net_status,
            net::net_join
        ])
        .setup(|app| {
            paths::init(app.handle());
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
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                #[cfg(not(target_os = "android"))]
                media::stop_all(app);
                // the browser goes straight out while the app is closed; the
                // next start points it here again
                net::forget_browser();
                net::stop();
            }
        });
}

#[cfg(not(target_os = "android"))]
use media::{media_close, media_open, media_status};

/// a phone has no mount and runs no jellyfin: the page hides the card
#[cfg(target_os = "android")]
#[tauri::command]
async fn media_status() -> Result<serde_json::Value, String> {
    Ok(
        serde_json::json!({ "running": false, "jellyfin": null, "libraries": 0, "files": 0, "unavailable": "not on a phone" }),
    )
}
#[cfg(target_os = "android")]
#[tauri::command]
async fn media_open() -> Result<serde_json::Value, String> {
    Err("Movies & TV plays on a desktop for now".to_string())
}
#[cfg(target_os = "android")]
#[tauri::command]
async fn media_close() -> Result<(), String> {
    Ok(())
}
