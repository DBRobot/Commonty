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
