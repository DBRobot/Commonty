//! A member's encrypted library: the files a box holds but cannot read.
//!
//! One library is one prefix in the bucket, in rclone's crypt format
//! (crypt.rs): encrypted names, encrypted blocks, so `rclone mount` opens
//! it on any desktop and no format here is ours. The library key is the
//! remote's password, sealed to the member's device, root and recovery
//! keys and published in their directory entry: the keys live in the
//! identity, never on a box, never in a file on disk. Sharing a library is
//! sealing its key to one more reader; taking it back is a new key.
//!
//! What a box learns: how many objects exist, how big they are, who
//! fetches which when. Names and contents, never.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use zeroize::Zeroizing;

pub mod crypt;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("key: {0}")]
    Key(String),
    #[error("this key does not open it")]
    Sealed,
    #[error("ciphertext is damaged or not for this key")]
    Aead,
    #[error("record: {0}")]
    Record(String),
    #[error("format: {0}")]
    Format(String),
    #[error(transparent)]
    Identity(#[from] identity::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// a 32-byte symmetric key, wiped when dropped
pub type Key = Zeroizing<[u8; 32]>;

pub fn random_key() -> Key {
    let mut k = Zeroizing::new([0u8; 32]);
    fill(&mut k[..]);
    k
}

/// 32 hex characters: a library or file id, safe in a url, a bucket key
/// and a directory entry alike
pub fn random_id() -> String {
    let mut b = [0u8; 16];
    fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The system's randomness: the kernel's on a box or a laptop, the
/// browser's crypto.getRandomValues in the page's wasm. Reading
/// /dev/urandom as a file, as this did, has no file to read in a browser:
/// sealing a library key to a box's transcoder aborted the page's wasm.
fn fill(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("randomness");
}

// ---------------------------------------------------------------- sealing

/// The x25519 keys that go with an identity's ed25519 keys, so a library
/// key can be sealed to a device, root or recovery key already in the
/// directory. Standard birational map (the same one libsodium uses).
pub fn seal_to(public_ed25519_b64: &str, secret: &[u8]) -> Result<String> {
    let vk = identity::decode_public(public_ed25519_b64)?;
    let ed = curve25519_dalek::edwards::CompressedEdwardsY(vk.to_bytes())
        .decompress()
        .ok_or_else(|| Error::Key("not a valid ed25519 point".into()))?;
    let mont = ed.to_montgomery();
    let pk = crypto_box::PublicKey::from(mont.to_bytes());
    let sealed = pk.seal(&mut rand_core(), secret).map_err(|_| Error::Aead)?;
    Ok(B64.encode(sealed))
}

/// open what was sealed to this ed25519 signing key
pub fn open_with(secret_ed25519: &ed25519_dalek::SigningKey, sealed_b64: &str) -> Result<Key> {
    let h = sha2::Sha512::digest(secret_ed25519.to_bytes());
    let mut scalar = [0u8; 32];
    scalar.copy_from_slice(&h[..32]);
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;
    let sk = crypto_box::SecretKey::from(scalar);
    let sealed = B64
        .decode(sealed_b64)
        .map_err(|e| Error::Key(e.to_string()))?;
    let opened = sk.unseal(&sealed).map_err(|_| Error::Sealed)?;
    let arr: [u8; 32] = opened[..]
        .try_into()
        .map_err(|_| Error::Key("sealed key is not 32 bytes".into()))?;
    Ok(Zeroizing::new(arr))
}

use sha2::Digest as _;

/// A fresh x25519 pair for a box that will do one job on one file: the
/// client seals the file key to `public`, the box opens it with `secret`
/// and forgets both when the job ends. Nothing about the library is ever
/// sealed this way, only a file key, for a session.
pub fn ephemeral() -> (String, Zeroizing<[u8; 32]>) {
    let mut sk = Zeroizing::new([0u8; 32]);
    fill(&mut sk[..]);
    let secret = crypto_box::SecretKey::from(*sk);
    (B64.encode(secret.public_key().as_bytes()), sk)
}

pub fn seal_to_x25519(public_b64: &str, secret: &[u8]) -> Result<String> {
    let b = B64
        .decode(public_b64)
        .map_err(|e| Error::Key(e.to_string()))?;
    let arr: [u8; 32] = b[..]
        .try_into()
        .map_err(|_| Error::Key("x25519 key is not 32 bytes".into()))?;
    let pk = crypto_box::PublicKey::from(arr);
    let sealed = pk.seal(&mut rand_core(), secret).map_err(|_| Error::Aead)?;
    Ok(B64.encode(sealed))
}

pub fn open_x25519(secret: &[u8; 32], sealed_b64: &str) -> Result<Key> {
    let sk = crypto_box::SecretKey::from(*secret);
    let sealed = B64
        .decode(sealed_b64)
        .map_err(|e| Error::Key(e.to_string()))?;
    let opened = sk.unseal(&sealed).map_err(|_| Error::Sealed)?;
    let arr: [u8; 32] = opened[..]
        .try_into()
        .map_err(|_| Error::Key("sealed key is not 32 bytes".into()))?;
    Ok(Zeroizing::new(arr))
}

fn rand_core() -> Urandom {
    Urandom
}

/// the system's randomness (fill) as a RngCore, for the sealing api that wants one
struct Urandom;
impl crypto_box::aead::rand_core::RngCore for Urandom {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        fill(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        fill(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        fill(dest)
    }
    fn try_fill_bytes(
        &mut self,
        dest: &mut [u8],
    ) -> std::result::Result<(), crypto_box::aead::rand_core::Error> {
        fill(dest);
        Ok(())
    }
}
impl crypto_box::aead::rand_core::CryptoRng for Urandom {}

// ------------------------------------------------------------- directory

pub use identity::{Library, Reader, SealedKey};

/// the library key sealed to every key an entry names for its owner
pub fn seal_for_entry(entry: &identity::Entry, key: &Key) -> Result<Vec<SealedKey>> {
    let mut out = Vec::new();
    for d in &entry.devices {
        out.push(SealedKey {
            to: format!("device:{}", d.fingerprint),
            sealed: seal_to(&d.public_key, &key[..])?,
        });
    }
    if !entry.root.starts_with("webauthn:") {
        out.push(SealedKey {
            to: "root".into(),
            sealed: seal_to(&entry.root, &key[..])?,
        });
    }
    // a passkey that has published a library key of its own: any browser
    // holding that passkey can open this library
    for p in &entry.passkeys {
        if let Some(pk) = &p.library_key {
            out.push(SealedKey {
                to: format!("passkey:{}", p.id),
                sealed: seal_to(pk, &key[..])?,
            });
        }
    }
    if !entry.recovery.is_empty() {
        out.push(SealedKey {
            to: "recovery".into(),
            sealed: seal_to(&entry.recovery, &key[..])?,
        });
    }
    Ok(out)
}

/// open the library key with whichever of this machine's keys it was sealed to
pub fn open_library(
    lib: &Library,
    device: Option<(&str, &ed25519_dalek::SigningKey)>,
    root: Option<&ed25519_dalek::SigningKey>,
) -> Result<Key> {
    for k in &lib.keys {
        if let Some((fp, sk)) = device
            && k.to == format!("device:{fp}")
            && let Ok(key) = open_with(sk, &k.sealed)
        {
            return Ok(key);
        }
        if k.to == "root"
            && let Some(sk) = root
            && let Ok(key) = open_with(sk, &k.sealed)
        {
            return Ok(key);
        }
    }
    Err(Error::Sealed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        devices: &[&ed25519_dalek::SigningKey],
        root: &ed25519_dalek::SigningKey,
    ) -> identity::Entry {
        identity::Entry {
            name: "amy".into(),
            root: identity::encode_public(&root.verifying_key()),
            recovery: String::new(),
            devices: devices
                .iter()
                .map(|d| identity::Device {
                    fingerprint: identity::fingerprint(&identity::encode_public(
                        &d.verifying_key(),
                    )),
                    public_key: identity::encode_public(&d.verifying_key()),
                    added: 1,
                })
                .collect(),
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            version: 1,
            updated: 1,
        }
    }

    #[test]
    fn a_library_key_opens_with_a_device_or_the_root_and_nothing_else() {
        let root = identity::generate();
        let phone = identity::generate();
        let stranger = identity::generate();
        let e = entry(&[&phone], &root);
        let key = random_key();
        let lib = Library {
            id: random_id(),
            keys: seal_for_entry(&e, &key).unwrap(),
            readers: vec![],
            created: 1,
        };
        assert_eq!(lib.keys.len(), 2);
        let fp = identity::fingerprint(&identity::encode_public(&phone.verifying_key()));
        assert_eq!(
            &open_library(&lib, Some((&fp, &phone)), None).unwrap()[..],
            &key[..]
        );
        assert_eq!(
            &open_library(&lib, None, Some(&root)).unwrap()[..],
            &key[..]
        );
        assert!(matches!(
            open_library(&lib, None, Some(&stranger)),
            Err(Error::Sealed)
        ));
    }

    #[test]
    fn a_file_key_sealed_to_a_box_for_one_job() {
        let (public, secret) = ephemeral();
        let fk = random_key();
        let sealed = seal_to_x25519(&public, &fk[..]).unwrap();
        assert_eq!(&open_x25519(&secret, &sealed).unwrap()[..], &fk[..]);
        let (_, other) = ephemeral();
        assert!(open_x25519(&other, &sealed).is_err());
    }
}
