//! `dd disk recover`: a box's disk key from the copy in its /boot/dd,
//! opened with the owner's paper key - for a box whose TPM half will not
//! open (a firmware update, a reset, a dead chip). The copy is age,
//! encrypted to the paper key as an ssh-ed25519 recipient
//! (nix/modules/box/disk-unlock-enrol.sh); the paper key is that key's seed.
use std::io::Read as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use zeroize::Zeroizing;

/// The age identity the paper key is: an ed25519 key from its seed, in the
/// OpenSSH form age reads.
fn identity(paper: &str) -> Result<age::ssh::Identity> {
    let seed = identity::decode_secret(paper).context("that is not a recovery key")?;
    let pair = ssh_key::private::Ed25519Keypair::from_seed(&seed.to_bytes());
    let key = ssh_key::PrivateKey::from(pair);
    let pem = key.to_openssh(ssh_key::LineEnding::LF)?;
    age::ssh::Identity::from_buffer(std::io::Cursor::new(pem.as_bytes()), None)
        .context("reading the paper key as an age identity")
}

pub fn recover(paper: &str, file: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let id = identity(paper)?;
    let sealed = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let decryptor = age::Decryptor::new(&sealed[..]).context("not an age file")?;
    let mut r = decryptor
        .decrypt(std::iter::once(&id as &dyn age::Identity))
        .context("this paper key does not open that file")?;
    let mut key = Zeroizing::new(Vec::new());
    r.read_to_end(&mut key)?;
    if key.len() != 32 {
        bail!("that file does not hold a disk key");
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn the_paper_key_opens_what_was_sealed_to_its_public_half() {
        let seed = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let paper = identity::encode_secret(&seed);
        // the recipient line as nix/roles/core.nix carries it
        let public = ssh_key::PublicKey::from(ssh_key::public::Ed25519PublicKey(
            seed.verifying_key().to_bytes(),
        ));
        let recipient: age::ssh::Recipient = public.to_openssh().unwrap().parse().unwrap();
        let mut sealed = Vec::new();
        let enc =
            age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .unwrap();
        let mut w = enc.wrap_output(&mut sealed).unwrap();
        w.write_all(&[9; 32]).unwrap();
        w.finish().unwrap();
        let dir = std::env::temp_dir().join(format!("dd-disk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("node9-disk.age");
        std::fs::write(&f, &sealed).unwrap();
        assert_eq!(&recover(&paper, &f).unwrap()[..], &[9; 32]);
        let other = identity::encode_secret(&ed25519_dalek::SigningKey::from_bytes(&[6; 32]));
        assert!(recover(&other, &f).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
