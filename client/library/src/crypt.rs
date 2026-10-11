//! rclone's crypt format, so a library is an rclone remote: `rclone mount`
//! opens it on any desktop, and nothing here is a format of ours. Keys
//! come from scrypt over a password and a salt (the library key, base64,
//! and the library id); names are AES-EME with PKCS#7 padding in lowercase
//! base32hex, one segment at a time; a file is an 8-byte magic, a 24-byte
//! nonce, then 64 KiB blocks each sealed with NaCl secretbox under the
//! nonce incremented by the block number. Every byte is checked against
//! rclone itself in the tests.
//!
//! A box that works on one file gets that file's block keys, not the data
//! key (`Cipher::block_keys`, `BlockOpener`): XSalsa20 is HSalsa20 of the
//! key over the nonce's first 16 bytes, then Salsa20 under that subkey
//! and the last 8. rclone counts from the nonce's first byte, so those 16
//! bytes differ for every block and so does the subkey. HSalsa20 is a
//! PRF: a block key gives back neither the data key nor any other block's.

use aes::Aes256;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{KeyIvInit as _, StreamCipher as _};
use base64::Engine as _;
use crypto_secretbox::aead::AeadInPlace as _;
use crypto_secretbox::{KeyInit as _, XSalsa20Poly1305};
use eme_mode::DynamicEme;
use subtle::ConstantTimeEq as _;
use zeroize::{Zeroize as _, Zeroizing};

use crate::{Error, Key, Result};

pub const MAGIC: &[u8; 8] = b"RCLONE\x00\x00";
pub const NONCE: usize = 24;
pub const HEADER: usize = 8 + NONCE;
/// the plain bytes in a block
pub const BLOCK: usize = 64 * 1024;
/// the tag before each block's bytes
pub const TAG: usize = 16;
pub const SEALED_BLOCK: usize = TAG + BLOCK;
/// a block's own key (Cipher::block_keys)
pub const BLOCK_KEY: usize = 32;
/// the most block keys a box is handed for one file: 128 GiB of film, and
/// 64 MiB of keys
pub const MAX_BLOCK_KEYS: u64 = 2 << 20;
const NAME_BLOCK: usize = 16;
/// a name longer than this is not a name rclone would write
const MAX_NAME: usize = 2048;

/// the keys of one remote
pub struct Cipher {
    data: Zeroizing<[u8; 32]>,
    name: Zeroizing<[u8; 32]>,
    tweak: Zeroizing<[u8; 16]>,
}

impl Cipher {
    /// what rclone derives from `password` and `password2` (the salt),
    /// both as the person typed them, not obscured
    pub fn new(password: &str, salt: &str) -> Cipher {
        // the len here is a hint the crate caps at 64; the output is 80 bytes
        let params = scrypt::Params::new(14, 8, 1, 64).expect("scrypt params");
        let mut out = Zeroizing::new([0u8; 80]);
        scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut out[..])
            .expect("scrypt");
        let mut data = [0u8; 32];
        let mut name = [0u8; 32];
        let mut tweak = [0u8; 16];
        data.copy_from_slice(&out[..32]);
        name.copy_from_slice(&out[32..64]);
        tweak.copy_from_slice(&out[64..80]);
        Cipher {
            data: Zeroizing::new(data),
            name: Zeroizing::new(name),
            tweak: Zeroizing::new(tweak),
        }
    }

    /// a library's remote: its key is the password, its id the salt. What
    /// a member types into `rclone config` to open it by hand.
    pub fn for_library(key: &Key, id: &str) -> Cipher {
        Cipher::new(&password_of(key), id)
    }

    /// The keys of blocks 0..`blocks` of the file with this header, end to
    /// end: what a box doing one job on one file gets instead of the data
    /// key. They open that file's blocks and nothing else.
    pub fn block_keys(&self, header: &[u8], blocks: u64) -> Result<Zeroizing<Vec<u8>>> {
        let nonce = nonce_of(header)?;
        if blocks > MAX_BLOCK_KEYS {
            return Err(Error::Format("too large a file to hand to a box".into()));
        }
        let mut out = Zeroizing::new(Vec::with_capacity(blocks as usize * BLOCK_KEY));
        for n in 0..blocks {
            out.extend_from_slice(&block_key(&self.data, &nonce_for(&nonce, n))[..]);
        }
        Ok(out)
    }

    fn eme(&self) -> DynamicEme<Aes256> {
        DynamicEme::<Aes256>::new(self.name.as_ref().into(), self.tweak.as_ref().into())
    }

    /// one path segment
    pub fn encrypt_segment(&self, plain: &str) -> String {
        if plain.is_empty() {
            return String::new();
        }
        let padded = self.eme().encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
        base32hex(&padded)
    }

    pub fn decrypt_segment(&self, enc: &str) -> Result<String> {
        if enc.is_empty() {
            return Ok(String::new());
        }
        let raw = unbase32hex(enc)?;
        if raw.is_empty() || raw.len() % NAME_BLOCK != 0 || raw.len() > MAX_NAME {
            return Err(Error::Format("not an encrypted name".into()));
        }
        let plain = self
            .eme()
            .decrypt_padded_vec_mut::<Pkcs7>(&raw)
            .map_err(|_| Error::Format("name padding".into()))?;
        String::from_utf8(plain).map_err(|_| Error::Format("name is not utf-8".into()))
    }

    /// a path, segment by segment; the separators stay
    pub fn encrypt_path(&self, plain: &str) -> String {
        plain
            .split('/')
            .map(|s| self.encrypt_segment(s))
            .collect::<Vec<_>>()
            .join("/")
    }

    pub fn decrypt_path(&self, enc: &str) -> Result<String> {
        enc.split('/')
            .map(|s| self.decrypt_segment(s))
            .collect::<Result<Vec<_>>>()
            .map(|v| v.join("/"))
    }

    /// the writer of one file: a fresh nonce, blocks in order
    pub fn encrypter(&self) -> Encrypter {
        let mut nonce = [0u8; NONCE];
        getrandom::getrandom(&mut nonce).expect("randomness");
        Encrypter {
            key: self.data.clone(),
            nonce,
        }
    }

    /// the reader of one file, from its header
    pub fn decrypter(&self, header: &[u8]) -> Result<Decrypter> {
        Ok(Decrypter {
            key: self.data.clone(),
            nonce: nonce_of(header)?,
        })
    }
}

/// the file's nonce, from its header
fn nonce_of(header: &[u8]) -> Result<[u8; NONCE]> {
    if header.len() < HEADER || &header[..8] != MAGIC {
        return Err(Error::Format("not an encrypted file".into()));
    }
    let mut nonce = [0u8; NONCE];
    nonce.copy_from_slice(&header[8..HEADER]);
    Ok(nonce)
}

/// the key of the block with this nonce: secretbox's own first step
pub fn block_key(data: &[u8; 32], nonce: &[u8; NONCE]) -> Key {
    let k = salsa20::hsalsa::<salsa20::cipher::consts::U10>(
        data.into(),
        salsa20::cipher::generic_array::GenericArray::from_slice(&nonce[..16]),
    );
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&k);
    out
}

/// A reader of one file from its block keys alone, for a box doing a job
/// on that file: it never holds the data key.
pub struct BlockOpener {
    nonce: [u8; NONCE],
    keys: Zeroizing<Vec<u8>>,
}

impl BlockOpener {
    pub fn new(header: &[u8], keys: Zeroizing<Vec<u8>>) -> Result<BlockOpener> {
        if !keys.len().is_multiple_of(BLOCK_KEY) {
            return Err(Error::Format("not a run of block keys".into()));
        }
        Ok(BlockOpener {
            nonce: nonce_of(header)?,
            keys,
        })
    }

    /// how many blocks it has keys for
    pub fn blocks(&self) -> u64 {
        (self.keys.len() / BLOCK_KEY) as u64
    }

    pub fn block(&self, n: u64, sealed: &[u8]) -> Result<Vec<u8>> {
        let at = usize::try_from(n)
            .ok()
            .filter(|_| n < self.blocks())
            .ok_or_else(|| Error::Format("no key for that block".into()))?
            * BLOCK_KEY;
        open_with_block_key(
            &self.keys[at..at + BLOCK_KEY],
            &nonce_for(&self.nonce, n),
            sealed,
        )
    }
}

/// secretbox's open after its HSalsa20: Salsa20 under the block key and the
/// nonce's last 8 bytes, the first 32 bytes of its stream the Poly1305 key
fn open_with_block_key(key: &[u8], nonce: &[u8; NONCE], sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < TAG {
        return Err(Error::Format("short block".into()));
    }
    let mut stream = salsa20::Salsa20::new(key.into(), nonce[16..].into());
    let mut mac_key = Zeroizing::new([0u8; 32]);
    stream.apply_keystream(&mut mac_key[..]);
    let tag = poly1305::Poly1305::new(mac_key.as_ref().into()).compute_unpadded(&sealed[TAG..]);
    if !bool::from(tag.as_slice().ct_eq(&sealed[..TAG])) {
        return Err(Error::Format("block does not open".into()));
    }
    let mut buf = sealed[TAG..].to_vec();
    stream.apply_keystream(&mut buf);
    Ok(buf)
}

/// the password rclone would need for a library key: its base64
pub fn password_of(key: &Key) -> String {
    base64::engine::general_purpose::STANDARD.encode(&key[..])
}

pub struct Encrypter {
    key: Zeroizing<[u8; 32]>,
    nonce: [u8; NONCE],
}

impl Encrypter {
    pub fn header(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(HEADER);
        h.extend_from_slice(MAGIC);
        h.extend_from_slice(&self.nonce);
        h
    }
    /// block `n` (at most BLOCK bytes): tag then bytes
    pub fn block(&self, n: u64, plain: &[u8]) -> Vec<u8> {
        seal(&self.key, &nonce_for(&self.nonce, n), plain)
    }
}

pub struct Decrypter {
    key: Zeroizing<[u8; 32]>,
    nonce: [u8; NONCE],
}

impl Decrypter {
    pub fn block(&self, n: u64, sealed: &[u8]) -> Result<Vec<u8>> {
        open(&self.key, &nonce_for(&self.nonce, n), sealed)
    }
}

/// the nonce of block `n`: the file's nonce plus n, little-endian with carry
pub fn nonce_for(initial: &[u8; NONCE], n: u64) -> [u8; NONCE] {
    let mut nonce = *initial;
    if n == 0 {
        return nonce;
    }
    let mut x = n;
    let mut carry: u16 = 0;
    for digit in nonce.iter_mut().take(8) {
        let add = (x & 0xff) as u16;
        x >>= 8;
        carry += *digit as u16 + add;
        *digit = carry as u8;
        carry >>= 8;
    }
    if carry != 0 {
        for digit in nonce.iter_mut().skip(8) {
            carry += *digit as u16;
            *digit = carry as u8;
            carry >>= 8;
            if carry == 0 {
                break;
            }
        }
    }
    nonce
}

fn seal(key: &[u8; 32], nonce: &[u8; NONCE], plain: &[u8]) -> Vec<u8> {
    let sb = XSalsa20Poly1305::new(key.into());
    let mut buf = Vec::with_capacity(TAG + plain.len());
    buf.extend_from_slice(&[0u8; TAG]);
    buf.extend_from_slice(plain);
    let tag = sb
        .encrypt_in_place_detached(nonce.into(), b"", &mut buf[TAG..])
        .expect("secretbox");
    buf[..TAG].copy_from_slice(&tag);
    buf
}

fn open(key: &[u8; 32], nonce: &[u8; NONCE], sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < TAG {
        return Err(Error::Format("short block".into()));
    }
    let sb = XSalsa20Poly1305::new(key.into());
    let mut buf = sealed[TAG..].to_vec();
    sb.decrypt_in_place_detached(nonce.into(), b"", &mut buf, sealed[..TAG].into())
        .map_err(|_| {
            buf.zeroize();
            Error::Format("block does not open".into())
        })?;
    Ok(buf)
}

/// the size of a file's ciphertext for `plain` bytes, and back
pub fn sealed_size(plain: u64) -> u64 {
    if plain == 0 {
        return HEADER as u64;
    }
    let blocks = plain.div_ceil(BLOCK as u64);
    HEADER as u64 + plain + blocks * TAG as u64
}

pub fn plain_size(sealed: u64) -> Result<u64> {
    if sealed < HEADER as u64 {
        return Err(Error::Format("shorter than a header".into()));
    }
    let body = sealed - HEADER as u64;
    if body == 0 {
        return Ok(0);
    }
    let full = body / SEALED_BLOCK as u64;
    let rest = body % SEALED_BLOCK as u64;
    if rest != 0 && rest <= TAG as u64 {
        return Err(Error::Format("a partial block with no bytes".into()));
    }
    Ok(full * BLOCK as u64 + if rest == 0 { 0 } else { rest - TAG as u64 })
}

const B32: base32::Alphabet = base32::Alphabet::Rfc4648Hex { padding: false };

fn base32hex(b: &[u8]) -> String {
    base32::encode(B32, b).to_lowercase()
}

fn unbase32hex(s: &str) -> Result<Vec<u8>> {
    base32::decode(B32, &s.to_uppercase()).ok_or_else(|| Error::Format("not base32".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let c = Cipher::new("pw", "salt");
        for n in ["a", "Movies/x.mkv", "a name with spaces.txt", "é"] {
            let e = c.encrypt_path(n);
            assert!(!e.contains(' '));
            assert_eq!(c.decrypt_path(&e).unwrap(), n);
        }
        assert_eq!(c.encrypt_path(""), "");
    }

    #[test]
    fn blocks_round_trip_and_sizes() {
        let c = Cipher::new("pw", "salt");
        let e = c.encrypter();
        let plain: Vec<u8> = (0..(BLOCK * 2 + 17)).map(|i| i as u8).collect();
        let mut out = e.header();
        for (n, chunk) in plain.chunks(BLOCK).enumerate() {
            out.extend(e.block(n as u64, chunk));
        }
        assert_eq!(out.len() as u64, sealed_size(plain.len() as u64));
        assert_eq!(plain_size(out.len() as u64).unwrap(), plain.len() as u64);
        let d = c.decrypter(&out[..HEADER]).unwrap();
        let mut back = Vec::new();
        for (n, chunk) in out[HEADER..].chunks(SEALED_BLOCK).enumerate() {
            back.extend(d.block(n as u64, chunk).unwrap());
        }
        assert_eq!(back, plain);
        // a block out of order does not open
        assert!(d.block(1, &out[HEADER..HEADER + SEALED_BLOCK]).is_err());
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// a file of `blocks` blocks (the last one short) under `nonce`
    fn sealed_file(c: &Cipher, nonce: [u8; NONCE], blocks: usize) -> (Vec<u8>, Vec<u8>) {
        let e = Encrypter {
            key: c.data.clone(),
            nonce,
        };
        let plain: Vec<u8> = (0..BLOCK * (blocks - 1) + 99)
            .map(|i| (i * 7) as u8)
            .collect();
        let mut out = e.header();
        for (n, chunk) in plain.chunks(BLOCK).enumerate() {
            out.extend(e.block(n as u64, chunk));
        }
        (out, plain)
    }

    #[test]
    fn a_block_key_is_hsalsa20_as_nacl_has_it() {
        // NaCl's own vector (tests/core4): firstkey, nonceprefix -> secondkey
        let key: [u8; 32] = hex("1b27556473e985d462cd51197a9a46c76009549eac6474f206c4ee0844f68389")
            .try_into()
            .unwrap();
        let mut nonce = [0u8; NONCE];
        nonce[..16].copy_from_slice(&hex("69696ee955b62b73cd62bda875fc73d6"));
        assert_eq!(
            block_key(&key, &nonce)[..],
            hex("dc908dda0b9344a953629b733820778880f3ceb421bb61b91cbd4c3e66256ce4")[..]
        );
        // and it is what secretbox itself derives
        use crypto_secretbox::Kdf as _;
        assert_eq!(
            block_key(&key, &nonce)[..],
            XSalsa20Poly1305::kdf((&key).into(), nonce[..16].into())[..]
        );
    }

    #[test]
    fn block_keys_open_what_the_data_key_opens() {
        let c = Cipher::new("pw", "salt");
        // the low byte about to carry: block keys follow the carry
        let mut nonce = [9u8; NONCE];
        nonce[0] = 0xfe;
        nonce[1] = 0xff;
        let (file, plain) = sealed_file(&c, nonce, 4);
        let keys = c.block_keys(&file[..HEADER], 4).unwrap();
        assert_eq!(keys.len(), 4 * BLOCK_KEY);
        let distinct: std::collections::HashSet<&[u8]> = keys.chunks(BLOCK_KEY).collect();
        assert_eq!(distinct.len(), 4, "every block its own key");
        let o = BlockOpener::new(&file[..HEADER], keys).unwrap();
        let d = c.decrypter(&file[..HEADER]).unwrap();
        let mut back = Vec::new();
        for (n, b) in file[HEADER..].chunks(SEALED_BLOCK).enumerate() {
            let opened = o.block(n as u64, b).unwrap();
            assert_eq!(opened, d.block(n as u64, b).unwrap());
            back.extend(opened);
        }
        assert_eq!(back, plain);
        // past the keys it was given, nothing
        assert!(o.block(4, &file[HEADER..HEADER + SEALED_BLOCK]).is_err());
        // one block's key does not open another
        assert!(o.block(1, &file[HEADER..HEADER + SEALED_BLOCK]).is_err());
        // a changed byte does not open
        let mut bad = file[HEADER..HEADER + SEALED_BLOCK].to_vec();
        bad[TAG + 5] ^= 1;
        assert!(o.block(0, &bad).is_err());
    }

    #[test]
    fn block_keys_open_one_file_only() {
        let c = Cipher::new("pw", "salt");
        let (a, _) = sealed_file(&c, [1u8; NONCE], 2);
        let (b, _) = sealed_file(&c, [2u8; NONCE], 2);
        let keys_a = c.block_keys(&a[..HEADER], 2).unwrap();
        // file A's keys, even told B's header, open none of B
        let o = BlockOpener::new(&b[..HEADER], keys_a.clone()).unwrap();
        for (n, blk) in b[HEADER..].chunks(SEALED_BLOCK).enumerate() {
            assert!(o.block(n as u64, blk).is_err());
        }
        let o = BlockOpener::new(&a[..HEADER], keys_a).unwrap();
        for (n, blk) in b[HEADER..].chunks(SEALED_BLOCK).enumerate() {
            assert!(o.block(n as u64, blk).is_err());
        }
        // and another library's keys do not open A
        let other = Cipher::new("pw2", "salt");
        let wrong = other.block_keys(&a[..HEADER], 2).unwrap();
        let o = BlockOpener::new(&a[..HEADER], wrong).unwrap();
        assert!(o.block(0, &a[HEADER..HEADER + SEALED_BLOCK]).is_err());
        // a run that is not whole keys is refused
        assert!(BlockOpener::new(&a[..HEADER], Zeroizing::new(vec![0u8; 33])).is_err());
        assert!(c.block_keys(&a[..HEADER], MAX_BLOCK_KEYS + 1).is_err());
    }

    #[test]
    fn nonce_carries() {
        let z = [0u8; NONCE];
        let mut one = z;
        one[0] = 1;
        assert_eq!(nonce_for(&z, 1), one);
        let mut ff = z;
        ff[0] = 0xff;
        let mut expect = z;
        expect[1] = 1;
        assert_eq!(nonce_for(&ff, 1), expect);
        let mut top = [0xffu8; 8];
        let mut n = z;
        n[..8].copy_from_slice(&top);
        top = [0u8; 8];
        let mut expect = z;
        expect[..8].copy_from_slice(&top);
        expect[8] = 1;
        assert_eq!(nonce_for(&n, 1), expect);
    }
}
