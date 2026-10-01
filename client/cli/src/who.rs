//! `dd`'s side of a member's identity: account does the work (client/account),
//! this prints what every directory said and the recovery key.

use anyhow::Result;
use identity::SignedEntry;

pub use account::{
    ROOT, add_library, admit, admit_passkey, create, load_root, ours, recover, remove_device,
    remove_passkey,
};
pub use directory::{fetch, http, newest};

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
