//! `dd media`: every openable library as folders on this machine, through
//! rclone, for any player (client/media does the work).

use std::path::PathBuf;

use anyhow::{Context, Result};

/// how long one token lives: rclone asks `dd media --token` for the next
/// when the gate refuses this one, so a mount outlives any of them
const TOKEN_SECS: u64 = 3600;

pub async fn run(
    keys: &auth::Store,
    dirs: &[String],
    at: Option<PathBuf>,
    token_only: bool,
) -> Result<()> {
    let (opener, user, _) = media::gate::Opener::load(keys)?;
    let kp = auth::device::load(keys)?.context("no device key here")?;
    let token = auth::device::mint(&kp, &user, std::time::Duration::from_secs(TOKEN_SECS))?;
    if token_only {
        println!("{token}");
        return Ok(());
    }
    let home = std::env::var("HOME").context("HOME")?;
    let at = at.unwrap_or_else(|| PathBuf::from(&home).join("Commonty"));
    let renew = media::mount::token_command(&std::env::current_exe()?, &["media", "--token"]);
    let base = media::gate::files_base(dirs)?;
    let mut mounts = Vec::new();
    let list = crate::librarycmd::released(keys, dirs).await;
    for (owner, lib, key) in media::gate::openable(dirs, &user, &opener, list.as_ref()).await? {
        let gate = media::gate::Gate::new(&base, &lib.id, &token, &key);
        let (dav, _) = gate.dav();
        let dir = at.join(&lib.id[..12]);
        let m = media::mount::mount(&lib.id, &owner, dav, &renew, &key, &dir)?;
        eprintln!(
            "dd media: {} ({}) at {}",
            &lib.id[..12],
            if owner == user {
                "yours".to_string()
            } else {
                format!("{owner}'s")
            },
            dir.display()
        );
        mounts.push(m);
    }
    if mounts.is_empty() {
        anyhow::bail!("no library to mount - `dd library new` makes one");
    }
    println!("mounted under {}", at.display());
    println!("point your player at it");
    println!("press ctrl-c to unmount");
    tokio::signal::ctrl_c().await?;
    drop(mounts);
    Ok(())
}
