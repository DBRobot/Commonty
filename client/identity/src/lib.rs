//! An identity that no server issues.
//!
//! A person IS their root key. Their directory entry - name, root, recovery
//! key, the devices allowed to sign for them - is a document they sign
//! themselves. Boxes only store and relay it. A box can withhold an entry or
//! serve a stale one; it cannot produce a valid newer one, because it holds no
//! key that could sign it.
//!
//! Two keys, on purpose. The root key signs everything day to day and lives
//! on the person's devices. The recovery key lives on paper: it can sign
//! exactly one thing, an entry that installs a new root - so a lost or stolen
//! device is survivable without any server or person standing in for you.
//!
//! Or one key: an account made in a browser has a passkey for a root
//! (`webauthn:<credential id>`), and the entry is signed by a webauthn
//! assertion whose challenge is the entry's hash. No device key, no paper;
//! the passkey platform's sync is the recovery. Every box checks it the
//! same way and none can make one: the authenticator signs, the box only
//! asked.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
pub use ed25519_dalek::SigningKey;
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("bad key: {0}")]
    Key(String),
    #[error("bad signature")]
    Signature,
    #[error("{0}")]
    Rejected(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// What a person publishes about themselves. Field order is the canonical
/// form: serde keeps it, and the signature is over exactly these bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// base64 ed25519 public key that signs updates; or `webauthn:<id>`,
    /// a passkey in `passkeys` that signs them with an assertion
    pub root: String,
    /// base64 ed25519 public key that may install a new root; empty for a
    /// passkey root, which cannot be replaced
    pub recovery: String,
    pub devices: Vec<Device>,
    /// Browser passkeys, as the box that enrolled them serialised the
    /// credential. In the entry rather than on a box, so no box can add one
    /// for you and any box can check a login against them. Absent when
    /// empty, so entries signed before the field existed still verify.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passkeys: Vec<Passkey>,
    /// An invite redeemed: membership granted by a code the fleet's owner
    /// signed, proven by this root. Absent for everyone else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<Grant>,
    /// This person's encrypted libraries (crate library): each one's key
    /// sealed to their devices, root and recovery key, and to any reader's.
    /// In the entry so the keys are where the identity is and nowhere else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub libraries: Vec<Library>,
    /// strictly increasing; a box never accepts an older or equal one
    pub version: u64,
    pub updated: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Passkey {
    /// the credential id, base64url, as the browser presents it
    pub id: String,
    /// the whole credential as webauthn-rs serialises it; opaque here
    pub cred: serde_json::Value,
    pub added: u64,
    /// The relying party this passkey was made for: the domain the box
    /// serving the enrolment answers on. An assertion carries a hash of
    /// it, so recording it here is what lets any box tell a signature made
    /// for this fleet from one made somewhere else. Absent on passkeys
    /// enrolled before this, which are checked as they always were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rp_id: Option<String>,
    /// The public half of a key only this passkey can make: the browser
    /// derives it from the passkey's own PRF secret, which never leaves
    /// the tab. Library keys are sealed to it, so any browser holding this
    /// passkey opens the member's libraries without being admitted as a
    /// device of its own. Absent on a passkey enrolled before this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub fingerprint: String,
    /// base64 ed25519 public key that signs this device's tokens
    pub public_key: String,
    pub added: u64,
}

/// A library as its owner's entry carries it: an id, its key sealed to
/// every key that may open it. The bucket, the records and the chunks
/// live on boxes under that id (crate library); this is the only place
/// the key exists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Library {
    pub id: String,
    /// "device:<fingerprint>", "root" or "recovery": whose key opens `sealed`
    pub keys: Vec<SealedKey>,
    /// members this library is shared with, each with the key sealed to
    /// their devices; empty until sharing exists
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readers: Vec<Reader>,
    pub created: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SealedKey {
    pub to: String,
    pub sealed: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reader {
    pub name: String,
    pub keys: Vec<SealedKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedEntry {
    pub entry: Entry,
    /// base64 ed25519 signature over the canonical json of `entry`, by the
    /// root named IN the entry. Always present, so an entry at any version
    /// stands on its own: a box that has never seen the name can still tell
    /// the entry is internally honest.
    pub signature: String,
    /// When the root changed: the same bytes signed by the recovery key that
    /// was on file before. The box that holds the previous entry checks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_signature: Option<String>,
}

pub fn generate() -> SigningKey {
    use std::io::Read;
    let mut b = Zeroizing::new([0u8; 32]);
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut *b))
        .expect("/dev/urandom");
    SigningKey::from_bytes(&b)
}

pub fn encode_secret(k: &SigningKey) -> Zeroizing<String> {
    Zeroizing::new(B64.encode(k.to_bytes()))
}

pub fn decode_secret(s: &str) -> Result<SigningKey> {
    let b = Zeroizing::new(
        B64.decode(s.trim())
            .map_err(|e| Error::Key(e.to_string()))?,
    );
    let arr: [u8; 32] = b[..]
        .try_into()
        .map_err(|_| Error::Key("secret is not 32 bytes".into()))?;
    Ok(SigningKey::from_bytes(&arr))
}

pub fn encode_public(k: &VerifyingKey) -> String {
    B64.encode(k.to_bytes())
}

pub fn decode_public(s: &str) -> Result<VerifyingKey> {
    let b = B64
        .decode(s.trim())
        .map_err(|e| Error::Key(e.to_string()))?;
    let arr: [u8; 32] = b[..]
        .try_into()
        .map_err(|_| Error::Key("public key is not 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| Error::Key(e.to_string()))
}

/// Short stable name for a public key: first six bytes, hex.
pub fn fingerprint(public_b64: &str) -> String {
    B64.decode(public_b64)
        .unwrap_or_default()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What the fleet's member list names a person by: a hash of their root,
/// so the list carries no names and nothing a key can be recovered from,
/// and a name someone else claims first buys them nothing. Full width, so
/// grinding a key that matches an entry on the list is not a thing.
pub fn member_id(root: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(root.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn canonical(entry: &Entry) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(entry)?)
}

/// Sign with the root the entry names.
pub fn sign(entry: Entry, root: &SigningKey) -> Result<SignedEntry> {
    if entry.root != encode_public(&root.verifying_key()) {
        return Err(Error::Key("that is not the root the entry names".into()));
    }
    let sig: Signature = root.sign(&canonical(&entry)?);
    Ok(SignedEntry {
        entry,
        signature: B64.encode(sig.to_bytes()),
        recovery_signature: None,
    })
}

/// Recovery: a new root signs, and the old recovery key co-signs to hand over.
pub fn sign_recovery(
    entry: Entry,
    new_root: &SigningKey,
    recovery: &SigningKey,
) -> Result<SignedEntry> {
    let mut signed = sign(entry, new_root)?;
    let sig: Signature = recovery.sign(&canonical(&signed.entry)?);
    signed.recovery_signature = Some(B64.encode(sig.to_bytes()));
    Ok(signed)
}

fn check(sig_b64: &str, entry: &Entry, key: &VerifyingKey) -> Result<()> {
    let sig = B64.decode(sig_b64).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    key.verify(&canonical(entry)?, &sig)
        .map_err(|_| Error::Signature)
}

pub const WEBAUTHN_ROOT: &str = "webauthn:";

/// A passkey root, naming the credential and committing to its key:
/// `webauthn:<credential id>:<base64url sha256 of the COSE key>`.
///
/// The second half is the whole point. A credential id is a public label -
/// it rides in every entry anyone may read - so on its own it is something
/// to copy, not something to prove. And `member_id` hashes the root string,
/// so a copied root is a copied membership: a brand new name, self-signed
/// with the copier's own key, inheriting whatever the fleet grants the
/// member whose id it borrowed.
pub fn passkey_root(id: &str, cred: &serde_json::Value) -> String {
    format!("{WEBAUTHN_ROOT}{id}:{}", cose_digest(cred))
}

/// the key inside a serialised webauthn-rs credential, hashed
fn cose_digest(cred: &serde_json::Value) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
    use sha2::Digest as _;
    let cose = &cred["cred"]["cred"];
    B64_URL.encode(sha2::Sha256::digest(
        serde_json::to_vec(cose).unwrap_or_default(),
    ))
}

/// the credential id and the key digest a passkey root carries
fn root_parts(root: &str) -> Option<(&str, &str)> {
    root.strip_prefix(WEBAUTHN_ROOT)?.split_once(':')
}

/// An invitation: a code the owner hands to a person, good for minutes.
/// The code is a seed; only the public key it derives is written down,
/// signed by the release key, so a box can check a redemption and the code
/// itself is never seen by anyone but the two people.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Invite {
    /// base64 ed25519 public key derived from the code
    pub public_key: String,
    pub issued: u64,
    pub expires: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedInvite {
    pub invite: Invite,
    /// the release key, base64: the only key that grants membership
    pub signer: String,
    pub signature: String,
}

/// A redeemed invite, carried in the entry of the person it let in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Grant {
    pub invite: SignedInvite,
    pub redeemed: u64,
    /// base64 ed25519 signature by the code's key over `proof_bytes`: this
    /// root, this invite, this moment. Lifting it into another entry proves
    /// nothing, because the root is in it.
    pub proof: String,
}

const CODE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Ten characters, two groups, no ambiguous letters: 50 bits.
pub fn new_code() -> String {
    use std::io::Read;
    let mut b = [0u8; 10];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .expect("/dev/urandom");
    let s: String = b
        .iter()
        .map(|x| CODE_ALPHABET[(*x as usize) % CODE_ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &s[..5], &s[5..])
}

/// What a person typed, as the code: lowercase, letters and digits only.
pub fn normalize_code(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The key a code is: sha256("dd-invite:" + code) as an ed25519 seed. The
/// browser derives the same one.
pub fn code_key(code: &str) -> SigningKey {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(format!("dd-invite:{}", normalize_code(code)).as_bytes());
    SigningKey::from_bytes(&h.into())
}

pub fn sign_invite(invite: Invite, release: &SigningKey) -> Result<SignedInvite> {
    let sig: Signature = release.sign(&serde_json::to_vec(&invite)?);
    Ok(SignedInvite {
        invite,
        signer: encode_public(&release.verifying_key()),
        signature: B64.encode(sig.to_bytes()),
    })
}

/// Signed by the release key named, and by no other.
pub fn verify_invite(s: &SignedInvite, release: &VerifyingKey) -> Result<()> {
    if s.signer != encode_public(release) {
        return Err(Error::Rejected(
            "invite is not signed by the release key".into(),
        ));
    }
    let sig = B64.decode(&s.signature).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    release
        .verify(&serde_json::to_vec(&s.invite)?, &sig)
        .map_err(|_| Error::Signature)
}

/// The bytes the code's key signs. Compact json in this order; the browser
/// builds the same string.
/// What a grant's proof binds to.
///
/// For a passkey root, the credential id and not the key digest. The browser
/// makes this claim with the credential it has just created, before the box
/// has serialised it, so the digest does not exist yet on the side doing the
/// proving. Binding to the id is what can honestly be proved at that moment.
///
/// It is narrower than binding the whole root, and the narrowness is bounded:
/// membership comes from `member_id` over the *whole* root, digest included,
/// so a grant lifted onto another key does not bring the original's
/// membership with it - only the invite, which `grant_taken` already refuses
/// to let two roots redeem. Binding the whole root wants the box to hand the
/// browser its root before the entry is built, which is a round trip the
/// join flow does not have yet.
pub fn grant_subject(root: &str) -> &str {
    match root_parts(root) {
        Some((id, _)) => &root[..WEBAUTHN_ROOT.len() + id.len()],
        None => root,
    }
}

pub fn proof_bytes(invite_public_key: &str, root: &str, redeemed: u64) -> Vec<u8> {
    let root = grant_subject(root);
    format!(
        "{{\"invite\":{},\"root\":{},\"redeemed\":{redeemed}}}",
        serde_json::to_string(invite_public_key).unwrap_or_default(),
        serde_json::to_string(root).unwrap_or_default()
    )
    .into_bytes()
}

pub fn prove(code: &SigningKey, invite_public_key: &str, root: &str, redeemed: u64) -> String {
    let sig: Signature = code.sign(&proof_bytes(invite_public_key, root, redeemed));
    B64.encode(sig.to_bytes())
}

/// The grant in `entry` was made by the code the owner signed, for this
/// root, within the invite's window. Whether the window had passed by the
/// time a box saw it is the box's check, with its own clock.
pub fn verify_grant(entry: &Entry, release: &VerifyingKey) -> Result<()> {
    let g = entry
        .grant
        .as_ref()
        .ok_or_else(|| Error::Rejected("no grant".into()))?;
    verify_invite(&g.invite, release)?;
    let inv = &g.invite.invite;
    if g.redeemed < inv.issued || g.redeemed > inv.expires {
        return Err(Error::Rejected(
            "redeemed outside the invite's window".into(),
        ));
    }
    let key = decode_public(&inv.public_key)?;
    let sig = B64.decode(&g.proof).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    key.verify(&proof_bytes(&inv.public_key, &entry.root, g.redeemed), &sig)
        .map_err(|_| Error::Signature)
}

/// The signature a passkey root makes: the three parts of a webauthn
/// assertion, base64url as the browser hands them over. Stored base64 of
/// this json in `SignedEntry::signature`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assertion {
    #[serde(rename = "authenticatorData")]
    pub authenticator_data: String,
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    pub signature: String,
}

/// What the browser must sign for `entry`: sha256 of its canonical form,
/// as the webauthn challenge.
/// What a passkey signs to change an entry: the entry under a label that
/// says so. The same passkey signs sign-in challenges, and those are the
/// box's random bytes; the label keeps a signature made for one from ever
/// counting as the other, and leaves room to sign other things with the
/// same key later without them meaning this.
pub fn challenge(entry: &Entry) -> Result<Vec<u8>> {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"commonty entry v1\0");
    h.update(canonical(entry)?);
    Ok(h.finalize().to_vec())
}

fn check_assertion(sig_b64: &str, entry: &Entry) -> Result<()> {
    check_assertion_with(sig_b64, entry, &entry.passkeys)
}

/// The assertion over `entry`, checked with the key found in `trusted`.
///
/// Which list that is decides everything. A `webauthn:` root is a credential
/// id, and a credential id is public - it travels in the entry anyone may
/// read. So the key it names has to come from what is already on file; taken
/// from the entry under check it would only say that whoever wrote the entry
/// also chose the key, which is no statement at all.
fn check_assertion_with(sig_b64: &str, entry: &Entry, trusted: &[Passkey]) -> Result<()> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
    use sha2::Digest as _;
    let (id, digest) = root_parts(&entry.root)
        .ok_or_else(|| Error::Key("a passkey root must commit to its key".into()))?;
    let passkey = trusted
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::Key("the root names a passkey the entry does not carry".into()))?;
    // the root says which key, not just which credential: a credential id
    // is public and copying one would otherwise copy the membership that
    // member_id derives from the root string
    if cose_digest(&passkey.cred) != digest {
        return Err(Error::Key(
            "the root does not name this passkey's key".into(),
        ));
    }
    // the credential as webauthn-rs serialises it: Passkey { cred: Credential { cred: COSEKey } }
    let key: webauthn_rs_core::proto::COSEKey =
        serde_json::from_value(passkey.cred["cred"]["cred"].clone())
            .map_err(|e| Error::Key(format!("passkey public key: {e}")))?;
    let a: Assertion = serde_json::from_slice(&B64.decode(sig_b64).map_err(|_| Error::Signature)?)
        .map_err(|_| Error::Signature)?;
    let auth = B64_URL
        .decode(&a.authenticator_data)
        .map_err(|_| Error::Signature)?;
    let client = B64_URL
        .decode(&a.client_data_json)
        .map_err(|_| Error::Signature)?;
    let sig = B64_URL.decode(&a.signature).map_err(|_| Error::Signature)?;
    // what the authenticator signed: authData || sha256(clientDataJSON)
    let mut data = auth.clone();
    data.extend_from_slice(&sha2::Sha256::digest(&client));
    if !key.verify_signature(&sig, &data).unwrap_or(false) {
        return Err(Error::Signature);
    }
    // and what it was asked to sign: this entry, and nothing else
    let c: serde_json::Value = serde_json::from_slice(&client).map_err(|_| Error::Signature)?;
    if c["type"].as_str() != Some("webauthn.get") {
        return Err(Error::Signature);
    }
    if c["challenge"].as_str() != Some(B64_URL.encode(challenge(entry)?).as_str()) {
        return Err(Error::Signature);
    }
    let origin = c["origin"].as_str().unwrap_or_default();
    if !presence_and_place(&auth, origin, passkey.rp_id.as_deref()) {
        return Err(Error::Signature);
    }
    Ok(())
}

/// Whether an assertion says "a person, here": user-present is set, which
/// an authenticator does only after a touch or an unlock, and it was made on
/// https for the relying party on record - or, for a passkey from before that
/// was recorded, for a domain the origin sits in.
fn presence_and_place(auth: &[u8], origin: &str, rp_id: Option<&str>) -> bool {
    use sha2::Digest as _;
    if auth.len() < 37 || auth[32] & 0x01 == 0 {
        return false;
    }
    let Some(host) = origin
        .strip_prefix("https://")
        .map(|h| h.split(['/', ':']).next().unwrap_or_default())
    else {
        return false;
    };
    let within = |rp: &str| host == rp || host.ends_with(&format!(".{rp}"));
    let hashed = |rp: &str| sha2::Sha256::digest(rp.as_bytes())[..] == auth[..32];
    match rp_id {
        Some(rp) => within(rp) && hashed(rp),
        None => {
            let labels: Vec<&str> = host.split('.').collect();
            (0..labels.len().saturating_sub(1)).any(|i| hashed(&labels[i..].join(".")))
        }
    }
}

/// Is the entry signed by the root it names? True of every valid entry.
pub fn verify(signed: &SignedEntry) -> Result<()> {
    if signed.entry.root.starts_with(WEBAUTHN_ROOT) {
        return check_assertion(&signed.signature, &signed.entry);
    }
    check(
        &signed.signature,
        &signed.entry,
        &decode_public(&signed.entry.root)?,
    )
}

/// The rule a box applies before storing `new` in place of `existing`.
///
/// - always: signed by the root it names
/// - first sight: nothing more. Whether a first sight may be trusted at all
///   is the box's question (has any peer seen the name?), not this one's.
/// - otherwise: strictly newer version, and either the same root as on
///   file, or a different root co-signed by the recovery key on file - that
///   is what recovery is
///
/// Nothing here consults a server, an admin, or a name registry. A box that
/// wants to lie can only withhold or replay; it cannot make this pass.
pub fn accept(existing: Option<&SignedEntry>, new: &SignedEntry) -> Result<()> {
    if !valid_name(&new.entry.name) {
        return Err(Error::Rejected("bad name".into()));
    }
    // a device key or a passkey root: something has to be able to sign
    if new.entry.devices.is_empty() && !new.entry.root.starts_with(WEBAUTHN_ROOT) {
        return Err(Error::Rejected("an entry needs at least one device".into()));
    }
    for d in &new.entry.devices {
        decode_public(&d.public_key)?;
    }
    if let Some(l) = new
        .entry
        .libraries
        .iter()
        .find(|l| !valid_library_id(&l.id))
    {
        return Err(Error::Rejected(format!("{:?} is not a library id", l.id)));
    }
    let Some(old) = existing else {
        // a first sight: there is nothing on file to check against, which
        // is why a box takes one only under the agreement rule
        verify(new)?;
        return Ok(());
    };
    // An update to a passkey root proves itself with the key on file, and
    // must keep carrying that very credential. Without both halves the root
    // - a public credential id - is a name anyone can sign under, and the
    // owner has no recovery key to take it back with.
    if old.entry.root.starts_with(WEBAUTHN_ROOT) && new.entry.root == old.entry.root {
        let (id, _) = root_parts(&old.entry.root)
            .ok_or_else(|| Error::Key("a passkey root must commit to its key".into()))?;
        let was = old.entry.passkeys.iter().find(|p| p.id == id);
        let now = new.entry.passkeys.iter().find(|p| p.id == id);
        match (was, now) {
            (Some(w), Some(n)) if w.cred == n.cred => {}
            (Some(_), _) => {
                return Err(Error::Rejected(
                    "the root's passkey may not be changed or dropped".into(),
                ));
            }
            (None, _) => return Err(Error::Key("the entry on file has no root passkey".into())),
        }
        check_assertion_with(&new.signature, &new.entry, &old.entry.passkeys)?;
    } else {
        verify(new)?;
    }
    if new.entry.name != old.entry.name {
        return Err(Error::Rejected("name mismatch".into()));
    }
    if new.entry.version <= old.entry.version {
        return Err(Error::Rejected(format!(
            "version {} is not newer than {}",
            new.entry.version, old.entry.version
        )));
    }
    // the paper key is the one thing a stolen root cannot take: only the
    // recovery key on file may name a different one (a first one, where
    // there was none, is the root's to set)
    let recovery_changed =
        new.entry.recovery != old.entry.recovery && !old.entry.recovery.is_empty();
    if new.entry.root == old.entry.root && !recovery_changed {
        return Ok(());
    }
    let Some(rs) = &new.recovery_signature else {
        if new.entry.root == old.entry.root {
            return Err(Error::Rejected(
                "only the recovery key may change the recovery key".into(),
            ));
        }
        return Err(Error::Rejected(
            "only the recovery key may change the root".into(),
        ));
    };
    if old.entry.recovery.is_empty() {
        return Err(Error::Rejected("a passkey root has no recovery key".into()));
    }
    check(rs, &new.entry, &decode_public(&old.entry.recovery)?)
}

/// A library id: 32 hex characters, and nothing else. It becomes a folder
/// name, a bucket prefix and a command-line argument on every machine that
/// opens the library, so it can be nothing that means anything there - no
/// separator, no dot, no leading dash.
pub fn valid_library_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !s.starts_with('.')
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invite_is_a_code_the_owner_signed_and_a_root_redeemed() {
        let release = generate();
        let code = new_code();
        assert_eq!(code.len(), 11);
        let ck = code_key(&code);
        assert_eq!(
            encode_public(&ck.verifying_key()),
            encode_public(&code_key(&code.to_uppercase().replace('-', " ")).verifying_key()),
            "typing is forgiving"
        );
        let inv = sign_invite(
            Invite {
                public_key: encode_public(&ck.verifying_key()),
                issued: 100,
                expires: 400,
            },
            &release,
        )
        .unwrap();
        verify_invite(&inv, &release.verifying_key()).unwrap();
        assert!(verify_invite(&inv, &generate().verifying_key()).is_err());
        let root = "webauthn:abc";
        let mut e = Entry {
            name: "tom".into(),
            root: root.into(),
            recovery: String::new(),
            devices: vec![],
            passkeys: vec![],
            libraries: vec![],
            grant: Some(Grant {
                invite: inv.clone(),
                redeemed: 200,
                proof: prove(&ck, &inv.invite.public_key, root, 200),
            }),
            version: 1,
            updated: 200,
        };
        verify_grant(&e, &release.verifying_key()).unwrap();
        // the wrong code, another root, or a time outside the window
        e.grant.as_mut().unwrap().proof =
            prove(&code_key("nope"), &inv.invite.public_key, root, 200);
        assert!(verify_grant(&e, &release.verifying_key()).is_err());
        e.grant.as_mut().unwrap().proof = prove(&ck, &inv.invite.public_key, "webauthn:other", 200);
        assert!(verify_grant(&e, &release.verifying_key()).is_err());
        e.grant.as_mut().unwrap().proof = prove(&ck, &inv.invite.public_key, root, 500);
        e.grant.as_mut().unwrap().redeemed = 500;
        assert!(verify_grant(&e, &release.verifying_key()).is_err());
    }

    /// A passkey root is a credential id, and a credential id is public.
    /// The whole of it: if the key that checks an update can come from the
    /// update, anyone who can read an entry can publish the next one.
    #[test]
    fn a_passkey_root_cannot_have_its_key_swapped_under_it() {
        let passkey = |cred: serde_json::Value| Passkey {
            id: "AbC".into(),
            cred,
            added: 1,
            library_key: None,
            rp_id: None,
        };
        let hers_cred = serde_json::json!({ "cred": { "cred": "hers" } });
        let root = passkey_root("AbC", &hers_cred);
        let entry = |pk: Passkey, version: u64| SignedEntry {
            entry: Entry {
                name: "alice".into(),
                root: root.clone(),
                recovery: String::new(),
                devices: vec![],
                passkeys: vec![pk],
                grant: None,
                libraries: vec![],
                version,
                updated: version,
            },
            signature: "not reached".into(),
            recovery_signature: None,
        };
        let hers = entry(passkey(hers_cred.clone()), 7);

        // the attack: alice's root, alice's name, a newer version, and the
        // attacker's own key under the same credential id
        let theirs = entry(
            passkey(serde_json::json!({ "cred": { "cred": "theirs" } })),
            8,
        );
        let e = accept(Some(&hers), &theirs).unwrap_err();
        assert!(format!("{e}").contains("may not be changed"), "{e}");

        // and dropping it is the same move by another name
        let mut gone = theirs.clone();
        gone.entry.passkeys.clear();
        let e = accept(Some(&hers), &gone).unwrap_err();
        assert!(format!("{e}").contains("may not be changed"), "{e}");

        // a root that is simply replaced has no way back either: a passkey
        // root has no recovery key, so this is refused as it always was
        let mut other = theirs.clone();
        other.entry.root = passkey_root("XyZ", &hers_cred);
        assert!(accept(Some(&hers), &other).is_err());

        // F6: a NEW name copying her root. First sight has nothing on file
        // to check against, so this is the copier's own key self-verifying -
        // and member_id hashes the root string, so it would be her
        // membership under their name. The key digest in the root is what
        // refuses it.
        let mut mallory = entry(
            passkey(serde_json::json!({ "cred": { "cred": "theirs" } })),
            1,
        );
        mallory.entry.name = "mallory".into();
        let e = accept(None, &mallory).unwrap_err();
        assert!(
            format!("{e}").contains("does not name this passkey's key"),
            "{e}"
        );
    }

    /// What an assertion has to say beyond "this key signed something":
    /// somebody was there, and it was here.
    #[test]
    fn an_assertion_must_show_presence_and_the_right_relying_party() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
        use sha2::Digest as _;

        // authData is rpIdHash(32) || flags(1) || counter(4)
        let auth = |rp: &str, flags: u8| {
            let mut v = sha2::Sha256::digest(rp.as_bytes()).to_vec();
            v.push(flags);
            v.extend_from_slice(&[0, 0, 0, 1]);
            v
        };
        let with_presence = auth("commonty.org", 0b0000_0001);
        let without = auth("commonty.org", 0b0000_0000);
        let elsewhere = auth("evil.example", 0b0000_0001);

        assert_eq!(with_presence.len(), 37);
        assert_eq!(with_presence[32] & 1, 1, "user present");
        assert_eq!(without[32] & 1, 0, "nobody was there");
        assert_ne!(
            with_presence[..32],
            elsewhere[..32],
            "a different relying party hashes differently"
        );

        // the check itself: present, on https, for the relying party on record
        use super::presence_and_place as ok;
        assert!(ok(
            &with_presence,
            "https://home.commonty.org",
            Some("commonty.org")
        ));
        assert!(ok(
            &with_presence,
            "https://commonty.org",
            Some("commonty.org")
        ));
        assert!(!ok(
            &without,
            "https://home.commonty.org",
            Some("commonty.org")
        ));
        assert!(!ok(
            &elsewhere,
            "https://home.commonty.org",
            Some("commonty.org")
        ));
        assert!(!ok(
            &with_presence,
            "https://commonty.org.evil.example",
            Some("commonty.org")
        ));
        assert!(!ok(
            &with_presence,
            "http://home.commonty.org",
            Some("commonty.org")
        ));
        assert!(!ok(
            &with_presence[..20],
            "https://home.commonty.org",
            Some("commonty.org")
        ));
        // a passkey from before rp_id was recorded: some domain the origin is in
        assert!(ok(&with_presence, "https://home.commonty.org", None));
        assert!(!ok(&elsewhere, "https://home.commonty.org", None));
        let _ = B64_URL;
    }

    /// An id is about to be a folder and an argument everywhere the library
    /// opens: anything but 32 hex characters is refused at the door.
    #[test]
    fn a_library_id_that_could_mean_something_is_refused() {
        let root = generate();
        let with = |id: &str| {
            let e = Entry {
                passkeys: vec![],
                grant: None,
                libraries: vec![Library {
                    id: id.into(),
                    keys: vec![],
                    readers: vec![],
                    created: 1,
                }],
                name: "sarah".into(),
                root: encode_public(&root.verifying_key()),
                recovery: encode_public(&generate().verifying_key()),
                devices: vec![dev(&generate())],
                version: 1,
                updated: 1,
            };
            accept(None, &sign(e, &root).unwrap())
        };
        assert!(with(&"ab".repeat(16)).is_ok());
        for bad in [
            "../../etc",
            "-rf",
            "a".repeat(31).as_str(),
            &"g".repeat(32),
            "",
            &"ab/".repeat(11),
        ] {
            assert!(with(bad).is_err(), "{bad:?} was taken as a library id");
        }
    }

    fn dev(k: &SigningKey) -> Device {
        let p = encode_public(&k.verifying_key());
        Device {
            fingerprint: fingerprint(&p),
            public_key: p,
            added: 1,
        }
    }

    #[test]
    fn birth_update_rotation_and_the_things_a_box_must_refuse() {
        let root = generate();
        let recovery = generate();
        let d1 = generate();
        let e1 = Entry {
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            name: "sarah".into(),
            root: encode_public(&root.verifying_key()),
            recovery: encode_public(&recovery.verifying_key()),
            devices: vec![dev(&d1)],
            version: 1,
            updated: 1,
        };
        let s1 = sign(e1.clone(), &root).unwrap();
        accept(None, &s1).unwrap();

        // a stranger's key cannot produce version 2: sign() refuses a key the
        // entry does not name, and a forged signature fails accept
        let stranger = generate();
        let mut e2 = e1.clone();
        e2.version = 2;
        e2.devices.push(dev(&generate()));
        assert!(sign(e2.clone(), &stranger).is_err());
        let mut forged = sign(e2.clone(), &root).unwrap();
        forged.signature = B64.encode([0u8; 64]);
        assert!(accept(Some(&s1), &forged).is_err());
        // the root can
        let s2 = sign(e2.clone(), &root).unwrap();
        accept(Some(&s1), &s2).unwrap();
        // replaying the old one is refused
        assert!(accept(Some(&s2), &s1).is_err());
        // the root alone may not name a new paper key: a stolen root would
        // otherwise lock the owner out of recovery for good
        let mut swap = e2.clone();
        swap.version = 3;
        swap.recovery = encode_public(&generate().verifying_key());
        assert!(accept(Some(&s2), &sign(swap.clone(), &root).unwrap()).is_err());
        // the paper key on file may, and the root still signs as before
        accept(
            Some(&s2),
            &sign_recovery(swap.clone(), &root, &recovery).unwrap(),
        )
        .unwrap();
        // and a wrong paper key may not
        assert!(accept(Some(&s2), &sign_recovery(swap, &root, &generate()).unwrap()).is_err());
        // a new root on its own is refused, however well it signs itself
        let mut e3 = e2.clone();
        e3.version = 3;
        e3.root = encode_public(&stranger.verifying_key());
        assert!(accept(Some(&s2), &sign(e3.clone(), &stranger).unwrap()).is_err());
        // co-signed by the recovery key on file it is recovery
        let s3 = sign_recovery(e3.clone(), &stranger, &recovery).unwrap();
        accept(Some(&s2), &s3).unwrap();
        // co-signed by the wrong recovery key it is not
        assert!(
            accept(
                Some(&s2),
                &sign_recovery(e3.clone(), &stranger, &generate()).unwrap()
            )
            .is_err()
        );
        // afterwards the OLD root is out and the new one is in
        let mut e4 = e3.clone();
        e4.version = 4;
        assert!(sign(e4.clone(), &root).is_err());
        assert!(accept(Some(&s3), &sign(e4, &stranger).unwrap()).is_ok());
        // a box seeing the name for the first time takes any version that
        // stands on its own - the peer check is the box's job, not this one's
        accept(None, &s3).unwrap();
        // but never an entry whose signature is not by the root it names
        let mut e0 = e1.clone();
        e0.root = encode_public(&stranger.verifying_key());
        let mut bad = sign(e0, &stranger).unwrap();
        bad.signature = s1.signature.clone();
        assert!(accept(None, &bad).is_err());
    }
}
