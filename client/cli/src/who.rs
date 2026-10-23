//! `dd`'s side of a member's identity: account does the work (client/account),
//! this prints what every directory said and the recovery key.

use anyhow::Result;
use identity::SignedEntry;

pub use account::{
    ROOT, add_library, admit, admit_passkey, create, load_root, ours, recover, remove_device,
    remove_passkey,
};
pub use directory::{fetch, http};

/// our own entry from every directory: this device's token, and the root's
/// proof where the root is here (a machine it was imported to, say)
pub async fn fetch_ours(
    keys: &impl auth::KeyStore,
    dirs: &[String],
    name: &str,
) -> Result<Vec<(String, Result<Option<SignedEntry>>)>> {
    let root = load_root(keys)?;
    Ok(directory::fetch_proven(dirs, name, &root.iter().collect::<Vec<_>>()).await)
}

/// publish, and say what each directory made of it
pub async fn publish(dirs: &[String], signed: &SignedEntry) -> Result<()> {
    let took = account::publish(dirs, signed).await?;
    for (d, t) in &took {
        if let directory::Took::Accepted = t {
            println!("  {d}: accepted version {}", signed.entry.version);
        }
    }
    Ok(())
}

pub fn print_recovery(recovery: &str) {
    println!("\n  RECOVERY KEY - write this down, on paper, now.");
    println!("  It is the only way back in if every device is lost or stolen.");
    println!("  It is not stored anywhere, and nobody can issue another.\n");
    println!("    {recovery}\n");
}

/// Sign out everywhere (state/ident signout.go): every sign-in before now
/// ends on every box. Signed by the root here and put at the door with this
/// device's token; sent already is sent.
pub async fn sign_out_everywhere(
    keys: &auth::Store,
    dirs: &[String],
    root: &ed25519_dalek::SigningKey,
) -> Result<()> {
    let (_, _, token) = media::gate::Opener::load(keys)?;
    let door = media::gate::door_base(dirs)?;
    let body = identity::envelope::SignOut::new(
        &identity::encode_public(&root.verifying_key()),
        identity::now(),
        "",
    );
    let env = identity::envelope::seal_json(identity::envelope::SIGNOUT, &body, root)?;
    let r = http()?
        .put(format!("{door}/_dd/boxes/{}", identity::envelope::SIGNOUT))
        .bearer_auth(&token)
        .json(&env)
        .send()
        .await?;
    let status = r.status();
    if status.is_success() || status == reqwest::StatusCode::CONFLICT {
        return Ok(());
    }
    anyhow::bail!(
        "the gate said {status}: {}",
        r.text().await.unwrap_or_default()
    )
}
