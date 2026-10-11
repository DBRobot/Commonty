//! Libraries in the browser: the name and block crypto of the format
//! (rclone's crypt, the same code the app and `dd` use), for a page that
//! holds a library key. The key comes from the app (its window, or a
//! browser it let in), or the demo's config; never from a passkey, which
//! answers its PRF to any page of the domain. Nothing here leaves the tab:
//! the page asks the gate for ciphertext and turns it into names and bytes
//! itself.

use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// scrypt runs once per library per page: a listing decrypts every name in
// it, and each fresh derivation cost the page a tenth of a second per name
type Kept = (
    Zeroizing<String>,
    String,
    std::rc::Rc<library::crypt::Cipher>,
);
thread_local! {
    static CIPHERS: std::cell::RefCell<Vec<Kept>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn cipher(library_key_b64: &str, id: &str) -> R<std::rc::Rc<library::crypt::Cipher>> {
    use base64::Engine as _;
    let hit = CIPHERS.with(|c| {
        c.borrow()
            .iter()
            .find(|(k, i, _)| k.as_str() == library_key_b64 && i == id)
            .map(|(_, _, c)| c.clone())
    });
    if let Some(c) = hit {
        return Ok(c);
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(library_key_b64)
        .map_err(err)?;
    if raw.len() != 32 {
        return Err("that is not a library key".into());
    }
    let mut k = Zeroizing::new([0u8; 32]);
    k.copy_from_slice(&raw);
    let c = std::rc::Rc::new(library::crypt::Cipher::for_library(&k, id));
    CIPHERS.with(|all| {
        let mut all = all.borrow_mut();
        if all.len() >= 4 {
            all.remove(0);
        }
        all.push((
            Zeroizing::new(library_key_b64.to_string()),
            id.to_string(),
            c.clone(),
        ));
    });
    Ok(c)
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

/// One file's block keys, sealed to a box that is about to do one job on
/// it: from the file's header and sealed size, every block of it. The box
/// can read that one file while the session lasts and nothing else of the
/// library; the data key never leaves this tab. 32 bytes a 64 KiB block:
/// 1 MiB for a 2 GB film. The key it is sealed to is thrown away when the
/// box restarts.
fn blocks_for_box(
    library_key_b64: &str,
    id: &str,
    header: &[u8],
    sealed_size: f64,
    box_public_b64: &str,
) -> R<String> {
    let c = cipher(library_key_b64, id)?;
    let plain = library::crypt::plain_size(sealed_size as u64).map_err(err)?;
    let keys = c
        .block_keys(header, plain.div_ceil(library::crypt::BLOCK as u64))
        .map_err(err)?;
    library::seal_to_x25519(box_public_b64, &keys).map_err(err)
}

/// A passkey's secret sealed to an app's one-time key: how the Photos
/// password reaches the app (servers/photos), which cannot use a passkey itself.
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
pub fn library_blocks_for_box(
    library_key_b64: &str,
    id: &str,
    header: &[u8],
    sealed_size: f64,
    box_public_b64: &str,
) -> Result<String, JsValue> {
    js(blocks_for_box(
        library_key_b64,
        id,
        header,
        sealed_size,
        box_public_b64,
    ))
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

    /// a box gets one file's block keys, sealed to it: they open that
    /// file there, and no other box opens them
    #[test]
    fn one_files_keys_go_to_a_box_sealed_and_to_no_one_else() {
        use base64::Engine as _;
        let (public, secret) = library::ephemeral();
        let k = base64::engine::general_purpose::STANDARD.encode(&library::random_key()[..]);
        let id = "aa";
        let plain: Vec<u8> = (0..(2 * library::crypt::BLOCK + 5))
            .map(|i| i as u8)
            .collect();
        let sealed = seal_file(&k, id, &plain).unwrap();
        let hdr = &sealed[..library::crypt::HEADER];
        let to_box = blocks_for_box(&k, id, hdr, sealed.len() as f64, &public).unwrap();
        let keys = library::open_x25519_bytes(&secret, &to_box).unwrap();
        assert_eq!(keys.len(), 3 * library::crypt::BLOCK_KEY);
        let o = library::crypt::BlockOpener::new(hdr, keys).unwrap();
        let mut back = Vec::new();
        for (n, b) in sealed[library::crypt::HEADER..]
            .chunks(library::crypt::SEALED_BLOCK)
            .enumerate()
        {
            back.extend(o.block(n as u64, b).unwrap());
        }
        assert_eq!(back, plain);
        // another file of the same library stays shut to them
        let other = seal_file(&k, id, &plain).unwrap();
        assert!(
            o.block(
                0,
                &other[library::crypt::HEADER..][..library::crypt::SEALED_BLOCK]
            )
            .is_err()
        );
        let (_, stranger) = library::ephemeral();
        assert!(library::open_x25519_bytes(&stranger, &to_box).is_err());
    }
}
