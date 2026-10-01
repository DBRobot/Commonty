//! A member's disk archives in the browser: restic's repository format, as
//! `dd image` writes it (box/archive). The archive password comes sealed to
//! the passkey (dd-passkeys.json, written by `dd image passkey`); opened
//! here it unlocks restic's key file, and the master key opens everything
//! else - the list of archives, the index, the pieces of a disk. Nothing is
//! readable to the server holding them, and nothing leaves this tab.

use aes::cipher::{BlockEncrypt, KeyInit, KeyIvInit, StreamCipher};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

type R<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn js<T>(r: R<T>) -> Result<T, JsValue> {
    r.map_err(|e| JsValue::from_str(&e))
}

/// restic's keys: AES-256 for the content, Poly1305-AES for its integrity
#[derive(Clone)]
struct Keys {
    encrypt: [u8; 32],
    mac_k: [u8; 16],
    mac_r: [u8; 16],
}

impl Drop for Keys {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.encrypt.zeroize();
        self.mac_k.zeroize();
        self.mac_r.zeroize();
    }
}

impl Keys {
    fn mac(&self, iv: &[u8], data: &[u8]) -> [u8; 16] {
        // Poly1305-AES: r as given, s the nonce under AES-128 with k
        let aes = aes::Aes128::new(&self.mac_k.into());
        let mut s = aes::Block::clone_from_slice(iv);
        aes.encrypt_block(&mut s);
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(&self.mac_r);
        key[16..].copy_from_slice(&s);
        poly1305_unpadded(&key, data)
    }

    /// IV || ciphertext || MAC, as every file and every blob is
    fn open(&self, sealed: &[u8]) -> R<Vec<u8>> {
        if sealed.len() < 32 {
            return Err("too short to be sealed".into());
        }
        let (iv, rest) = sealed.split_at(16);
        let (ct, tag) = rest.split_at(rest.len() - 16);
        if self.mac(iv, ct)[..] != tag[..] {
            return Err("this does not open with this key".into());
        }
        let mut out = ct.to_vec();
        let mut c = ctr::Ctr128BE::<aes::Aes256>::new(&self.encrypt.into(), iv.into());
        c.apply_keystream(&mut out);
        Ok(out)
    }

    fn seal(&self, plain: &[u8]) -> R<Vec<u8>> {
        let mut iv = [0u8; 16];
        getrandom::fill(&mut iv).map_err(err)?;
        let mut ct = plain.to_vec();
        let mut c = ctr::Ctr128BE::<aes::Aes256>::new(&self.encrypt.into(), (&iv).into());
        c.apply_keystream(&mut ct);
        let tag = self.mac(&iv, &ct);
        let mut out = iv.to_vec();
        out.extend_from_slice(&ct);
        out.extend_from_slice(&tag);
        Ok(out)
    }
}

/// Poly1305 over a message whose last block may be short, as the one-time
/// mac is defined (the crate's padded update would pad it with zeros)
fn poly1305_unpadded(key: &[u8; 32], data: &[u8]) -> [u8; 16] {
    poly1305::Poly1305::new(key.into())
        .compute_unpadded(data)
        .into()
}

#[derive(Deserialize)]
struct KeyFile {
    #[serde(rename = "N")]
    n: u64,
    r: u32,
    p: u32,
    salt: String,
    data: String,
}

#[derive(Deserialize)]
struct MasterKey {
    mac: MacKey,
    encrypt: String,
}

#[derive(Deserialize)]
struct MacKey {
    k: String,
    r: String,
}

fn arr<const N: usize>(b64: &str) -> R<[u8; N]> {
    B64.decode(b64)
        .map_err(err)?
        .try_into()
        .map_err(|_| format!("a key of the wrong length (want {N} bytes)"))
}

/// restic files are compressed with a leading 2, or are plain json
fn unpack(plain: Vec<u8>) -> R<Vec<u8>> {
    match plain.first() {
        Some(2) => zstd(&plain[1..]),
        _ => Ok(plain),
    }
}

fn zstd(data: &[u8]) -> R<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::new();
    ruzstd::decoding::StreamingDecoder::new(data)
        .map_err(err)?
        .read_to_end(&mut out)
        .map_err(err)?;
    Ok(out)
}

/// A member's archive, opened: what the page holds while it is open.
#[wasm_bindgen]
pub struct Archive {
    keys: Keys,
}

#[wasm_bindgen]
impl Archive {
    /// The archive password sealed to this passkey (dd-passkeys.json, by
    /// the passkey's credential id) and restic's key file: opens both.
    /// Slow on purpose: restic's key derivation is what guards the password.
    #[wasm_bindgen(constructor)]
    pub fn new(
        sealed_passwords_json: &str,
        credential_id: &str,
        prf_secret_b64: &str,
        key_file_json: &str,
    ) -> Result<Archive, JsValue> {
        js(open_archive(
            sealed_passwords_json,
            credential_id,
            prf_secret_b64,
            key_file_json,
        ))
    }

    /// a file of the repository (an index, a snapshot, the config), opened
    /// and decompressed
    pub fn file(&self, sealed: &[u8]) -> Result<Vec<u8>, JsValue> {
        js(self.keys.open(sealed).and_then(unpack))
    }

    /// One blob from a pack: opened, and decompressed when the index gave
    /// it an uncompressed length.
    pub fn blob(&self, sealed: &[u8], compressed: bool) -> Result<Vec<u8>, JsValue> {
        js(self
            .keys
            .open(sealed)
            .and_then(|p| if compressed { zstd(&p) } else { Ok(p) }))
    }

    /// A file of the repository's own, sealed for writing back: a new index
    /// after a delete. Json, uncompressed, which restic reads as it is.
    pub fn seal_file(&self, plain: &[u8]) -> Result<Vec<u8>, JsValue> {
        js(self.keys.seal(plain))
    }

    /// The name a repository file goes by: the sha256 of what is stored.
    pub fn id_of(stored: &[u8]) -> String {
        use sha2::Digest as _;
        sha2::Sha256::digest(stored)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

fn open_archive(
    sealed_passwords_json: &str,
    credential_id: &str,
    prf_secret_b64: &str,
    key_file_json: &str,
) -> R<Archive> {
    let sealed: std::collections::HashMap<String, String> =
        serde_json::from_str(sealed_passwords_json).map_err(err)?;
    let sealed = sealed
        .get(credential_id)
        .ok_or("these archives were not opened to this passkey yet: run `dd image passkey` on the computer that made them")?;
    let sk = crate::library::keypair_of(prf_secret_b64)?;
    let raw = library::open_with(&sk, sealed).map_err(err)?;
    let password = Zeroizing::new(URL_SAFE_NO_PAD.encode(&raw[..]));
    with_password(&password, key_file_json)
}

/// restic's key file, opened with the archive password itself
pub fn with_password(password: &str, key_file_json: &str) -> R<Archive> {
    let kf: KeyFile = serde_json::from_str(key_file_json).map_err(err)?;
    let log_n = (63 - kf.n.leading_zeros()) as u8;
    let params = scrypt::Params::new(log_n, kf.r, kf.p, 64).map_err(err)?;
    let mut derived = Zeroizing::new([0u8; 64]);
    scrypt::scrypt(
        password.as_bytes(),
        &B64.decode(&kf.salt).map_err(err)?,
        &params,
        &mut derived[..],
    )
    .map_err(err)?;
    let user = Keys {
        encrypt: derived[..32].try_into().unwrap(),
        mac_k: derived[32..48].try_into().unwrap(),
        mac_r: derived[48..64].try_into().unwrap(),
    };
    let master = Zeroizing::new(
        user.open(&B64.decode(&kf.data).map_err(err)?)
            .map_err(|_| "the archive password did not open the archive".to_string())?,
    );
    let m: MasterKey = serde_json::from_slice(&master).map_err(err)?;
    Ok(Archive {
        keys: Keys {
            encrypt: arr(&m.encrypt)?,
            mac_k: arr(&m.mac.k)?,
            mac_r: arr(&m.mac.r)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_sealed_opens_and_a_change_is_caught() {
        let k = Keys {
            encrypt: [7; 32],
            mac_k: [3; 16],
            mac_r: [9; 16],
        };
        let sealed = k.seal(b"{\"packs\":[]}").unwrap();
        assert_eq!(k.open(&sealed).unwrap(), b"{\"packs\":[]}");
        let mut bent = sealed.clone();
        bent[20] ^= 1;
        assert!(k.open(&bent).is_err());
    }
}
