//! Libraries in the browser. A passkey's PRF secret is a key only that
//! passkey can make; from it this derives the keypair the member's
//! libraries are sealed to, opens a library key out of their entry, and
//! does the name and block crypto of the format (rclone's crypt, the same
//! code the app and `dd` use). Nothing here leaves the tab: the page asks
//! the gate for ciphertext and turns it into names and bytes itself.

use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The keypair a passkey makes: its PRF secret, hashed with a label of our
/// own so the photos key and this one are different keys, as an ed25519
/// seed. The public half goes in the entry (`dd passkey link`), the secret
/// half never exists outside this tab.
fn keypair(prf_secret_b64: &str) -> R<ed25519_dalek::SigningKey> {
    use base64::Engine as _;
    use sha2::Digest as _;
    // the page sends base64url (webauthn.js); take either alphabet
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(
            prf_secret_b64
                .replace('+', "-")
                .replace('/', "_")
                .trim_end_matches('='),
        )
        .map_err(err)?;
    let mut h = sha2::Sha512::new();
    h.update(b"dd-library-device");
    h.update(&secret);
    let d = h.finalize();
    let mut seed = Zeroizing::new([0u8; 32]);
    seed.copy_from_slice(&d[..32]);
    Ok(ed25519_dalek::SigningKey::from_bytes(&seed))
}

/// the public half, for `dd passkey link`
fn device_key(prf_secret_b64: &str) -> R<String> {
    Ok(identity::encode_public(
        &keypair(prf_secret_b64)?.verifying_key(),
    ))
}

/// A library this passkey can open: the entry as json, the library's id,
/// and the passkey's PRF secret. Returns the library key, base64, which
/// the calls below take. The page holds it for as long as the tab lives.
fn open(entry_json: &str, id: &str, prf_secret_b64: &str) -> R<String> {
    let entry: identity::SignedEntry = serde_json::from_str(entry_json).map_err(err)?;
    identity::verify(&entry).map_err(err)?;
    let lib = entry
        .entry
        .libraries
        .iter()
        .find(|l| l.id == id)
        .ok_or_else(|| "no such library in that entry".to_string())?;
    let sk = keypair(prf_secret_b64)?;
    let public = identity::encode_public(&sk.verifying_key());
    // the key sealed to this passkey's derived key, whichever entry named it
    for k in &lib.keys {
        if k.to.starts_with("passkey:")
            && let Ok(key) = library::open_with(&sk, &k.sealed)
        {
            let _ = &public;
            return Ok(library::crypt::password_of(&key));
        }
    }
    Err("no key in that library opens with this passkey: `dd passkey link` it once".into())
}

fn cipher(library_key_b64: &str, id: &str) -> R<library::crypt::Cipher> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(library_key_b64)
        .map_err(err)?;
    if raw.len() != 32 {
        return Err("that is not a library key".into());
    }
    let mut k = Zeroizing::new([0u8; 32]);
    k.copy_from_slice(&raw);
    Ok(library::crypt::Cipher::for_library(&k, id))
}

/// a plain path to the encrypted one the gate knows
fn encrypt_path(library_key_b64: &str, id: &str, plain: &str) -> R<String> {
    Ok(cipher(library_key_b64, id)?.encrypt_path(plain))
}

/// and back; an error means the name is not ours to read
fn decrypt_path(library_key_b64: &str, id: &str, enc: &str) -> R<String> {
    cipher(library_key_b64, id)?.decrypt_path(enc).map_err(err)
}

/// the plain bytes of a file the gate served, whole
fn open_file(library_key_b64: &str, id: &str, sealed: &[u8]) -> R<Vec<u8>> {
    let c = cipher(library_key_b64, id)?;
    if sealed.len() < library::crypt::HEADER {
        return Err("shorter than a header".into());
    }
    let d = c
        .decrypter(&sealed[..library::crypt::HEADER])
        .map_err(err)?;
    let mut out = Vec::with_capacity(sealed.len());
    for (n, block) in sealed[library::crypt::HEADER..]
        .chunks(library::crypt::SEALED_BLOCK)
        .enumerate()
    {
        out.extend(d.block(n as u64, block).map_err(err)?);
    }
    Ok(out)
}

/// a file the page is uploading: header and blocks, ready to PUT
fn seal_file(library_key_b64: &str, id: &str, plain: &[u8]) -> R<Vec<u8>> {
    let c = cipher(library_key_b64, id)?;
    let e = c.encrypter();
    let mut out = e.header();
    for (n, block) in plain.chunks(library::crypt::BLOCK).enumerate() {
        out.extend(e.block(n as u64, block));
    }
    Ok(out)
}

/// A file sealed a piece at a time, so a large one never sits in memory
/// whole: the header, then runs of whole blocks from any block number.
/// Every block has its own nonce (the file's plus its number), so pieces
/// can be sealed in any order and the result is the same as seal_file.
#[wasm_bindgen]
pub struct Sealer(library::crypt::Encrypter);

#[wasm_bindgen]
impl Sealer {
    #[wasm_bindgen(constructor)]
    pub fn new(library_key_b64: &str, id: &str) -> Result<Sealer, JsValue> {
        js(cipher(library_key_b64, id).map(|c| Sealer(c.encrypter())))
    }
    pub fn header(&self) -> Vec<u8> {
        self.0.header()
    }
    /// plain bytes starting at block `first`; whole blocks except at the end
    pub fn seal(&self, first: f64, plain: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len() + plain.len() / 4096 + 16);
        for (n, block) in plain.chunks(library::crypt::BLOCK).enumerate() {
            out.extend(self.0.block(first as u64 + n as u64, block));
        }
        out
    }
}

/// The other way: the header once, then sealed runs of whole blocks.
#[wasm_bindgen]
pub struct Opener(library::crypt::Decrypter);

#[wasm_bindgen]
impl Opener {
    #[wasm_bindgen(constructor)]
    pub fn new(library_key_b64: &str, id: &str, header: &[u8]) -> Result<Opener, JsValue> {
        js(cipher(library_key_b64, id)
            .and_then(|c| c.decrypter(header).map_err(err))
            .map(Opener))
    }
    pub fn open(&self, first: f64, sealed: &[u8]) -> Result<Vec<u8>, JsValue> {
        let mut out = Vec::with_capacity(sealed.len());
        for (n, block) in sealed.chunks(library::crypt::SEALED_BLOCK).enumerate() {
            out.extend(js(self
                .0
                .block(first as u64 + n as u64, block)
                .map_err(err))?);
        }
        Ok(out)
    }
}

/// The library's data key, sealed to a box that is about to do one job
/// on one file. The box can read this library while the session lasts,
/// which is what transcoding is: plaintext in its memory for one film,
/// nothing kept. Nothing else about the library goes with it, and the
/// key it is sealed to is thrown away when the box restarts.
fn key_for_box(library_key_b64: &str, id: &str, box_public_b64: &str) -> R<String> {
    let c = cipher(library_key_b64, id)?;
    library::seal_to_x25519(box_public_b64, &c.data_key()[..]).map_err(err)
}

/// A passkey's secret sealed to an app's one-time key: how the Photos
/// password reaches the app (box/photos), which cannot use a passkey itself.
/// The secret comes base64url, as the page has it; only that key opens it.
fn secret_for_app(app_public_b64: &str, secret_b64u: &str) -> R<String> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(secret_b64u.trim_end_matches('='))
        .map_err(err)?;
    if raw.len() != 32 {
        return Err("not a passkey's secret".into());
    }
    library::seal_to_x25519(app_public_b64, &raw).map_err(err)
}

/// how many plain bytes a file of this sealed size holds
fn plain_of(sealed: f64) -> R<f64> {
    library::crypt::plain_size(sealed as u64)
        .map(|n| n as f64)
        .map_err(err)
}

// ---- what the page calls; the work is above, where a test can reach it

fn js<T>(r: R<T>) -> Result<T, JsValue> {
    r.map_err(|e| JsValue::from_str(&e))
}

#[wasm_bindgen]
pub fn library_device_key(prf_secret_b64: &str) -> Result<String, JsValue> {
    js(device_key(prf_secret_b64))
}

#[wasm_bindgen]
pub fn library_open(entry_json: &str, id: &str, prf_secret_b64: &str) -> Result<String, JsValue> {
    js(open(entry_json, id, prf_secret_b64))
}

#[wasm_bindgen]
pub fn path_encrypt(library_key_b64: &str, id: &str, plain: &str) -> Result<String, JsValue> {
    js(encrypt_path(library_key_b64, id, plain))
}

#[wasm_bindgen]
pub fn path_decrypt(library_key_b64: &str, id: &str, enc: &str) -> Result<String, JsValue> {
    js(decrypt_path(library_key_b64, id, enc))
}

#[wasm_bindgen]
pub fn file_open(library_key_b64: &str, id: &str, sealed: &[u8]) -> Result<Vec<u8>, JsValue> {
    js(open_file(library_key_b64, id, sealed))
}

#[wasm_bindgen]
pub fn file_seal(library_key_b64: &str, id: &str, plain: &[u8]) -> Result<Vec<u8>, JsValue> {
    js(seal_file(library_key_b64, id, plain))
}

#[wasm_bindgen]
pub fn library_key_for_box(
    library_key_b64: &str,
    id: &str,
    box_public_b64: &str,
) -> Result<String, JsValue> {
    js(key_for_box(library_key_b64, id, box_public_b64))
}

#[wasm_bindgen]
pub fn seal_for_app(app_public_b64: &str, secret_b64u: &str) -> Result<String, JsValue> {
    js(secret_for_app(app_public_b64, secret_b64u))
}

#[wasm_bindgen]
pub fn plain_size(sealed: f64) -> Result<f64, JsValue> {
    js(plain_of(sealed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the page hands the secret over as base64url; it is the same key
    #[test]
    fn a_secret_in_either_alphabet_is_the_same_key() {
        use base64::Engine as _;
        let raw = [0xfbu8; 32]; // encodes with '_' and '/' in it
        let url = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        let std = base64::engine::general_purpose::STANDARD.encode(raw);
        assert!(url.contains('_') && std.contains('/'));
        assert_eq!(
            keypair(&url).unwrap().verifying_key(),
            keypair(&std).unwrap().verifying_key()
        );
    }

    /// sealed in pieces is sealed whole, and opens in pieces
    #[test]
    fn pieces_are_the_whole_file() {
        use base64::Engine as _;
        let key = base64::engine::general_purpose::STANDARD.encode([3u8; 32]);
        let id = "a996a28ca51c9cf1d3f8e2038c8339c8";
        let plain: Vec<u8> = (0..(3 * library::crypt::BLOCK + 777))
            .map(|i| i as u8)
            .collect();
        let s = Sealer(cipher(&key, id).unwrap().encrypter());
        let mut sealed = s.header();
        sealed.extend(s.seal(0.0, &plain[..2 * library::crypt::BLOCK]));
        sealed.extend(s.seal(2.0, &plain[2 * library::crypt::BLOCK..]));
        assert_eq!(open_file(&key, id, &sealed).unwrap(), plain);

        let hdr = &sealed[..library::crypt::HEADER];
        let o = Opener(cipher(&key, id).unwrap().decrypter(hdr).unwrap());
        let body = &sealed[library::crypt::HEADER..];
        let mut back = o.open(0.0, &body[..library::crypt::SEALED_BLOCK]).unwrap();
        back.extend(o.open(1.0, &body[library::crypt::SEALED_BLOCK..]).unwrap());
        assert_eq!(back, plain);
    }

    /// the key a passkey makes is the same every time and depends on the
    /// label, so the photos key and this one can never be the same key
    #[test]
    fn derived_key_is_stable_and_labelled() {
        use base64::Engine as _;
        let secret = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let a = keypair(&secret).unwrap();
        let b = keypair(&secret).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
        let other = base64::engine::general_purpose::STANDARD.encode([8u8; 32]);
        assert_ne!(keypair(&other).unwrap().to_bytes(), a.to_bytes());
        // and the public half is what library_device_key prints
        assert_eq!(
            device_key(&secret).unwrap(),
            identity::encode_public(&a.verifying_key())
        );
    }

    /// what the page opens is what the fleet sealed
    #[test]
    fn a_passkey_opens_a_library_sealed_to_it() {
        use base64::Engine as _;
        let secret = base64::engine::general_purpose::STANDARD.encode([3u8; 32]);
        let device = device_key(&secret).unwrap();
        let root = identity::generate();
        let key = library::random_key();
        let id = library::random_id();
        let entry = identity::Entry {
            name: "sarah".into(),
            root: identity::encode_public(&root.verifying_key()),
            recovery: String::new(),
            devices: vec![],
            passkeys: vec![identity::Passkey {
                id: "pk1".into(),
                cred: serde_json::Value::Null,
                added: 0,
                library_key: Some(device),
                rp_id: None,
            }],
            grant: None,
            libraries: vec![],
            version: 1,
            updated: 0,
        };
        let lib = identity::Library {
            id: id.clone(),
            keys: library::seal_for_entry(&entry, &key).unwrap(),
            readers: vec![],
            created: 0,
        };
        let entry = identity::Entry {
            libraries: vec![lib],
            ..entry
        };
        let signed = identity::sign(entry, &root).unwrap();
        let json = serde_json::to_string(&signed).unwrap();
        let opened = open(&json, &id, &secret).unwrap();
        assert_eq!(opened, library::crypt::password_of(&key));
        // a name written with it reads back
        let enc = encrypt_path(&opened, &id, "Files/a note.txt").unwrap();
        assert_eq!(
            decrypt_path(&opened, &id, &enc).unwrap(),
            "Files/a note.txt"
        );
        // and another passkey's secret does not open it
        let stranger = base64::engine::general_purpose::STANDARD.encode([9u8; 32]);
        assert!(open(&json, &id, &stranger).is_err());
    }

    #[test]
    fn the_data_key_goes_to_a_box_sealed_and_to_no_one_else() {
        let (public, secret) = library::ephemeral();
        let key = library::random_key();
        let k = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &key[..]);
        let sealed = key_for_box(&k, "aa", &public).unwrap();
        let opened = library::open_x25519(&secret, &sealed).unwrap();
        let c = cipher(&k, "aa").unwrap();
        assert_eq!(&opened[..], &c.data_key()[..]);
        let (_, other) = library::ephemeral();
        assert!(library::open_x25519(&other, &sealed).is_err());
    }
}
