//! The phone's VPN (gen/android/.../VpnPlugin.kt, CommontyVpn.kt): asked
//! for once, and its interface handed to the engine, so the browser and
//! every other app on the phone reach the fleet's names. A desktop does
//! this with proxy rules instead (browser.rs).

#[cfg(target_os = "android")]
use tauri::Manager as _;
use tauri::Runtime;
use tauri::plugin::{Builder, TauriPlugin};

#[cfg(target_os = "android")]
pub struct Vpn<R: Runtime>(tauri::plugin::PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("vpn")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let h = api.register_android_plugin("org.commonty.app", "VpnPlugin")?;
                app.manage(Vpn(h));
            }
            let _ = (app, api);
            Ok(())
        })
        .build()
}

/// the VPN up for this node's address; its interface's descriptor
#[cfg(target_os = "android")]
pub fn up<R: Runtime>(app: &tauri::AppHandle<R>, ip: &str) -> Result<i32, String> {
    #[derive(serde::Deserialize)]
    struct Fd {
        fd: i32,
    }
    let v = app.try_state::<Vpn<R>>().ok_or("no vpn plugin")?;
    v.0.run_mobile_plugin::<Fd>("start", serde_json::json!({ "ip": ip }))
        .map(|f| f.fd)
        .map_err(|e| e.to_string())
}
