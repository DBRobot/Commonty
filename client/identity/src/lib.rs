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

pub mod envelope;
pub mod fleet;
pub mod session;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, Verifier};
pub use ed25519_dalek::{SigningKey, VerifyingKey};
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
    /// The roots this person had before, oldest first: one more each time
    /// the recovery key on file installs a new root (`accept` holds it to
    /// exactly that, and a first sight carries none), so membership follows
    /// the person across a recovery. Absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovered_from: Vec<String>,
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
    /// The version of the Terms of use (and the Privacy policy with them)
    /// this person agreed to at sign-up (`TERMS`), signed like the rest.
    /// Absent on entries made before it was asked, which stay valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms: Option<String>,
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
    /// Retired: the public half of a key a browser once made from this
    /// passkey's PRF secret. Any page on the domain can ask a passkey for
    /// that secret, so a passkey holds no keys now (docs/dev/libraries.md).
    /// Kept so entries signed with it still read and verify; every update
    /// drops it (`drop_passkey_keys`).
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
    /// they may write too (a calendar two people keep): saves, moves and
    /// deletes into the trash; deleting for good stays the owner's
    #[serde(default, skip_serializing_if = "is_false")]
    pub writes: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// An entry and its root's signature, in either form it is written in:
///
/// - v1: `{entry, signature, recovery_signature}`, the signature over
///   `canonical(entry)` - serde's bytes, so only this crate can check it;
/// - v2: an envelope (docs/dev/identity-state.md, `envelope.rs`) of kind
///   `entry`, the signature over the body's bytes exactly as signed, which
///   any language checks with a hash and a signature check.
///
/// Both read into this one type, so every reader (gate, Workers, the app,
/// dd, the door) checks either with the same `verify` and `accept`. A v2 one
/// keeps its body: `entry` is that body, read, and `signature` holds
/// `v2:<body base64>:<signature>`; it is written back out as the envelope.
#[derive(Debug, Clone)]
pub struct SignedEntry {
    pub entry: Entry,
    /// base64 ed25519 signature over the canonical json of `entry`, by the
    /// root named IN the entry. Always present, so an entry at any version
    /// stands on its own: a box that has never seen the name can still tell
    /// the entry is internally honest.
    pub signature: String,
    /// When the root changed: the same bytes signed by the recovery key that
    /// was on file before. The box that holds the previous entry checks it.
    pub recovery_signature: Option<String>,
}

/// The Terms of use a sign-up agrees to: the date docs/policies/terms.md
/// last changed, which that page states (checks.terms-version holds the two
/// to each other). Change both when the terms change.
pub const TERMS: &str = "2026-10-11";
/// Sign-ups from this time on are asked (2026-10-11 UTC): an entry first
/// seen without `terms` and made since is refused (`agreed_terms`), and one
/// made before stays as valid as it always was.
pub const TERMS_ASKED_FROM: u64 = 1_791_676_800;

/// What a box asks of a new sign-up: the terms it agreed to, a date. An
/// entry from before sign-ups were asked needs none.
pub fn agreed_terms(entry: &Entry) -> Result<()> {
    match &entry.terms {
        Some(t) if is_date(t) => Ok(()),
        Some(_) => Err(Error::Rejected("the terms agreed to are not a date".into())),
        None if entry.updated < TERMS_ASKED_FROM => Ok(()),
        None => Err(Error::Rejected(
            "a new account agrees to the Terms of use and the Privacy policy".into(),
        )),
    }
}

/// YYYY-MM-DD
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

pub fn generate() -> SigningKey {
    let mut b = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *b).expect("randomness");
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

/// Every member id this person has had: the root's, then those of the
/// roots recovered from, newest first
pub fn member_ids(entry: &Entry) -> Vec<String> {
    std::iter::once(&entry.root)
        .chain(entry.recovered_from.iter().rev())
        .map(|r| member_id(r))
        .collect()
}

/// Let in by the release: some root this person has had is on `members`
/// and none is on `revoked` (a recovery escapes neither), or else an invite
/// the release key signed. Only as sound as the entry's `recovered_from`:
/// the directory's, which replays every recovery, or one held to an entry
/// pinned earlier.
pub fn is_member(
    entry: &Entry,
    members: &[String],
    revoked: &[String],
    release: Option<&VerifyingKey>,
) -> bool {
    let ids = member_ids(entry);
    if ids.iter().any(|id| revoked.contains(id)) {
        return false;
    }
    ids.iter().any(|id| members.contains(id))
        || (entry.grant.is_some() && release.is_some_and(|r| verify_grant(entry, r).is_ok()))
}

/// A recovery as the box that took it saw it: the entry on file, and the
/// one the recovery key on file put in its place. A directory keeps every
/// one it takes, so a client that knows an earlier root can follow the name
/// to its current one without taking the directory's word for
/// `recovered_from`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handover {
    pub before: SignedEntry,
    pub after: SignedEntry,
}

/// The one id the list names as `name`'s current root, if exactly one:
/// `dd member add` keeps it so, and a list that names two is no answer.
pub fn listed_as<'a>(
    name: &str,
    members: &'a [String],
    names: &std::collections::BTreeMap<String, String>,
) -> Option<&'a String> {
    let mut it = members
        .iter()
        .filter(|id| names.get(*id) == Some(&name_tag(id, name)));
    let one = it.next()?;
    it.next().is_none().then_some(one)
}

/// Is `signed` let in by the release, as a client can check on its own?
/// Its root is the one the signed list names as this entry's name's
/// current root, and none of its roots is revoked. A root it was recovered
/// from does not count: once superseded it is a key that may be in a
/// thief's hands, and an old paper key can fork from any entry it once
/// signed for, so neither may vouch for anyone. A member recovered, like
/// one let in by an invite, anchors once a release lists the new root.
pub fn anchored(
    signed: &SignedEntry,
    members: &[String],
    revoked: &[String],
    names: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    verify(signed)?;
    // claimed earlier roots only ever shut a person out
    if member_ids(&signed.entry)
        .iter()
        .any(|id| revoked.contains(id))
    {
        return Err(Error::Rejected(format!("{} is revoked", signed.entry.name)));
    }
    let id = member_id(&signed.entry.root);
    if listed_as(&signed.entry.name, members, names) != Some(&id) {
        return Err(Error::Rejected(format!(
            "the signed release does not list that root as {}'s",
            signed.entry.name
        )));
    }
    Ok(())
}

/// The recovering person's own entry, followed from a root they know is
/// theirs (`from`, a member id: their listed root, or one their device
/// pinned) through recoveries each proven by a handover that `accept`
/// takes. For that person's own recovery alone, which their paper key then
/// has to match: it vouches for nobody to anyone else.
pub fn follows(signed: &SignedEntry, handovers: &[Handover], from: &str) -> Result<()> {
    verify(signed)?;
    let mut cur = &signed.entry;
    // each step is a recovery: a new root, and `accept` holds the one
    // before to exactly the earlier roots less the last, so the walk ends
    // within as many steps as there are earlier roots, and any proof that
    // passes lands where any other would
    while member_id(&cur.root) != from {
        let step = handovers.iter().find(|h| {
            h.after.entry.name == cur.name
                && h.after.entry.root == cur.root
                && h.after.entry.recovered_from == cur.recovered_from
                && h.before.entry.root != h.after.entry.root
                && verify(&h.before).is_ok()
                && accept(Some(&h.before), &h.after).is_ok()
        });
        let Some(h) = step else {
            return Err(Error::Rejected(format!(
                "no directory showed how {} came from the root this person knows",
                signed.entry.name
            )));
        };
        cur = &h.before.entry;
    }
    Ok(())
}

/// What the member list says a root's name is: a hash of the id and the
/// name, so the list in the open repository names nobody outright, and a
/// tag lifted onto another id says nothing there.
pub fn name_tag(id: &str, name: &str) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"commonty member name v1\0");
    h.update(id.as_bytes());
    h.update(b"\0");
    h.update(name.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
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

/// In `SignedEntry::signature`: an entry that came as a v2 envelope
const V2_SIG: &str = "v2:";

/// v1 as written
#[derive(Serialize, Deserialize)]
struct V1Entry {
    entry: Entry,
    signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_signature: Option<String>,
}

impl SignedEntry {
    /// An entry's envelope, read: the body becomes `entry`, kept as signed
    pub fn from_envelope(e: &envelope::Envelope) -> Result<SignedEntry> {
        if e.v != 2 || e.kind != envelope::ENTRY {
            return Err(Error::Rejected("not an entry's envelope".into()));
        }
        let entry: Entry = e.read()?;
        Ok(SignedEntry {
            entry,
            signature: format!("{V2_SIG}{}:{}", e.body, e.sig),
            recovery_signature: e.recovery_sig.clone(),
        })
    }

    /// Its envelope, if it came as one
    pub fn envelope(&self) -> Option<envelope::Envelope> {
        let (body, sig) = self.signature.strip_prefix(V2_SIG)?.split_once(':')?;
        Some(envelope::Envelope {
            v: 2,
            kind: envelope::ENTRY.into(),
            body: body.into(),
            sig: sig.into(),
            recovery_sig: self.recovery_signature.clone(),
        })
    }

    /// The bytes its signatures are over, and the root's signature itself
    fn signed(&self) -> Result<(Vec<u8>, &str)> {
        match self.signature.strip_prefix(V2_SIG) {
            Some(rest) => {
                let (body, sig) = rest.split_once(':').ok_or(Error::Signature)?;
                let body = B64
                    .decode(body)
                    .map_err(|_| Error::Rejected("the body is not base64".into()))?;
                Ok((envelope::message(envelope::ENTRY, &body), sig))
            }
            None => Ok((canonical(&self.entry)?, &self.signature)),
        }
    }

    /// What a passkey root answered: v1 the labelled hash of the canonical
    /// entry, v2 the hash of the envelope's message
    fn passkey_challenge(&self) -> Result<Vec<u8>> {
        use sha2::Digest as _;
        match self.signature.strip_prefix(V2_SIG) {
            Some(_) => Ok(sha2::Sha256::digest(self.signed()?.0).to_vec()),
            None => challenge(&self.entry),
        }
    }
}

impl Serialize for SignedEntry {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self.envelope() {
            Some(e) => e.serialize(s),
            None => V1Entry {
                entry: self.entry.clone(),
                signature: self.signature.clone(),
                recovery_signature: self.recovery_signature.clone(),
            }
            .serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for SignedEntry {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;
        let v = serde_json::Value::deserialize(d)?;
        if v.get("v").is_some() {
            let e: envelope::Envelope = serde_json::from_value(v).map_err(D::Error::custom)?;
            return SignedEntry::from_envelope(&e).map_err(D::Error::custom);
        }
        let e: V1Entry = serde_json::from_value(v).map_err(D::Error::custom)?;
        // a v1 signature that looks like a v2 one would be read as one
        if e.signature.starts_with(V2_SIG) {
            return Err(D::Error::custom("a v1 signature is base64"));
        }
        Ok(SignedEntry {
            entry: e.entry,
            signature: e.signature,
            recovery_signature: e.recovery_signature,
        })
    }
}

fn check(sig_b64: &str, msg: &[u8], key: &VerifyingKey) -> Result<()> {
    let sig = B64.decode(sig_b64).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    key.verify(msg, &sig).map_err(|_| Error::Signature)
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
    let mut b = [0u8; 10];
    getrandom::fill(&mut b).expect("randomness");
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

/// The fleet's member list as a release carries it to a client
/// (fleet/members.json at the commit released): the ids let in and the ids
/// shut out, and the release's counter, so a client that has seen a newer
/// one is not handed an older one back.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberList {
    pub counter: u64,
    pub members: Vec<String>,
    pub revoked: Vec<String>,
    /// id -> `name_tag(id, name)`: whose name each listed root is
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub names: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedMemberList {
    pub list: MemberList,
    /// the release key, base64
    pub signer: String,
    pub signature: String,
}

/// what the release key signs for a member list: the list under a label of
/// its own, so no invite, release or app manifest signature counts as one
fn member_list_bytes(l: &MemberList) -> Result<Vec<u8>> {
    let mut b = b"commonty members v1\0".to_vec();
    b.extend(serde_json::to_vec(l)?);
    Ok(b)
}

pub fn sign_member_list(list: MemberList, release: &SigningKey) -> Result<SignedMemberList> {
    let sig: Signature = release.sign(&member_list_bytes(&list)?);
    Ok(SignedMemberList {
        list,
        signer: encode_public(&release.verifying_key()),
        signature: B64.encode(sig.to_bytes()),
    })
}

/// Signed by the release key named, and by no other.
pub fn verify_member_list(s: &SignedMemberList, release: &VerifyingKey) -> Result<()> {
    if s.signer != encode_public(release) {
        return Err(Error::Rejected(
            "member list is not signed by the release key".into(),
        ));
    }
    let sig = B64.decode(&s.signature).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    release
        .verify(&member_list_bytes(&s.list)?, &sig)
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
    // proved by the root that redeemed it, which a recovery since may have
    // replaced
    std::iter::once(&entry.root)
        .chain(&entry.recovered_from)
        .any(|root| {
            key.verify(&proof_bytes(&inv.public_key, root, g.redeemed), &sig)
                .is_ok()
        })
        .then_some(())
        .ok_or(Error::Signature)
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

/// A passkey's signature over a v2 envelope (envelope.rs): `root` is the
/// `webauthn:` root that signs, `trusted` the passkeys already on file (for a
/// first claim, the entry's own, as `accept` takes them), and `challenge`
/// `envelope::passkey_challenge` of the envelope. The same checks as an
/// entry's v1 signature: the root names a key on file, and the assertion is
/// over this challenge.
pub fn check_envelope_assertion(
    root: &str,
    trusted: &[Passkey],
    sig_b64: &str,
    challenge: &[u8],
) -> Result<()> {
    let (id, digest) = root_parts(root)
        .ok_or_else(|| Error::Key("a passkey root must commit to its key".into()))?;
    let passkey = trusted
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::Key("the root names a passkey the entry does not carry".into()))?;
    if cose_digest(&passkey.cred) != digest {
        return Err(Error::Key(
            "the root does not name this passkey's key".into(),
        ));
    }
    let a: Assertion = serde_json::from_slice(&B64.decode(sig_b64).map_err(|_| Error::Signature)?)
        .map_err(|_| Error::Signature)?;
    assertion_over(passkey, &a, challenge)
}

fn check_assertion(signed: &SignedEntry) -> Result<()> {
    check_assertion_with(signed, &signed.entry.passkeys)
}

/// The assertion over `entry`, checked with the key found in `trusted`.
///
/// Which list that is decides everything. A `webauthn:` root is a credential
/// id, and a credential id is public - it travels in the entry anyone may
/// read. So the key it names has to come from what is already on file; taken
/// from the entry under check it would only say that whoever wrote the entry
/// also chose the key, which is no statement at all.
fn check_assertion_with(signed: &SignedEntry, trusted: &[Passkey]) -> Result<()> {
    // one passkey check for every signed thing: an entry's v1 or v2 signature
    // goes through the same one an envelope of any kind does
    let (_, sig_b64) = signed.signed()?;
    check_envelope_assertion(
        &signed.entry.root,
        trusted,
        sig_b64,
        &signed.passkey_challenge()?,
    )
}

/// A sign-in: `a` is the passkey `id` of `entry` answering `challenge`, made
/// by a person on one of the pages in `origins` (exactly: "https://host").
/// For something other than a box that holds the person's passkeys - it
/// reads them from the signed entry. Any page of the relying party would be
/// too many: a box serving one could ask for an answer to another's
/// challenge and hand it on.
pub fn check_login(
    entry: &Entry,
    id: &str,
    a: &Assertion,
    challenge: &[u8],
    origins: &[String],
) -> Result<()> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
    let passkey = entry
        .passkeys
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::Key("that passkey is not in the entry".into()))?;
    let client = B64_URL
        .decode(&a.client_data_json)
        .map_err(|_| Error::Signature)?;
    let c: serde_json::Value = serde_json::from_slice(&client).map_err(|_| Error::Signature)?;
    let origin = c["origin"].as_str().unwrap_or_default();
    if !origins.iter().any(|o| o == origin) {
        return Err(Error::Signature);
    }
    assertion_over(passkey, a, challenge)
}

fn assertion_over(passkey: &Passkey, a: &Assertion, expected: &[u8]) -> Result<()> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
    use sha2::Digest as _;
    // the credential as webauthn-rs serialises it: Passkey { cred: Credential { cred: COSEKey } }
    let key: webauthn_rs_core::proto::COSEKey =
        serde_json::from_value(passkey.cred["cred"]["cred"].clone())
            .map_err(|e| Error::Key(format!("passkey public key: {e}")))?;
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
    if c["challenge"].as_str() != Some(B64_URL.encode(expected).as_str()) {
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

/// What a member's root signs to give a home controller its name and key
/// (docs/dev/home-controller.md): checked by the door, the gate and the DNS
/// Worker.
pub fn controller_name_message(name: &str, controller: &str) -> Vec<u8> {
    format!("commonty controller name v1\0{name}\0{controller}").into_bytes()
}

/// A controller's label, made from its key: random to anyone without it, so
/// it says nothing of the house, and no one names a label as theirs who does
/// not hold the controller's key (the gate keeps a household's backup by it).
pub fn controller_label(controller: &str) -> Result<String> {
    use sha2::Digest as _;
    let key = decode_public(controller)?;
    let mut h = sha2::Sha256::new();
    h.update(b"commonty controller label v1\0");
    h.update(key.as_bytes());
    let bits = u128::from_be_bytes(h.finalize()[..16].try_into().expect("16 bytes"));
    const ABC: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    Ok((0..16)
        .map(|i| ABC[((bits >> (123 - 5 * i)) & 31) as usize] as char)
        .collect())
}

/// The root of `entry` gave `controller` the name `name`
/// (controller_name_message): an ed25519 signature (base64) by a device-key
/// root, or, for a passkey root, the root passkey's assertion (base64 of its
/// JSON, as an entry's own signature is) over sha256 of that message, made
/// on one of `origins` exactly. Only this statement: the message is made
/// here, never handed in, so no other signature of the root's can pass for
/// it. The entry is the caller's to have checked.
pub fn controller_name_signed(
    entry: &Entry,
    name: &str,
    controller: &str,
    sig: &str,
    origins: &[String],
) -> Result<()> {
    use sha2::Digest as _;
    // the label is the controller's own: <label>.home.<domain>
    let label = controller_label(controller)?;
    if name
        .split_once('.')
        .is_none_or(|(l, rest)| l != label || !rest.starts_with("home."))
    {
        return Err(Error::Rejected("that name is not this controller's".into()));
    }
    let message = controller_name_message(name, controller);
    if !entry.root.starts_with(WEBAUTHN_ROOT) {
        let key = decode_public(&entry.root)?;
        let sig = B64.decode(sig.trim()).map_err(|_| Error::Signature)?;
        let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
        return key.verify(&message, &sig).map_err(|_| Error::Signature);
    }
    let (id, digest) = root_parts(&entry.root)
        .ok_or_else(|| Error::Key("a passkey root must commit to its key".into()))?;
    let passkey = entry
        .passkeys
        .iter()
        .find(|p| p.id == id && cose_digest(&p.cred) == digest)
        .ok_or_else(|| Error::Key("the root's passkey is not in the entry".into()))?;
    let a: Assertion =
        serde_json::from_slice(&B64.decode(sig.trim()).map_err(|_| Error::Signature)?)
            .map_err(|_| Error::Signature)?;
    assertion_over(passkey, &a, &sha2::Sha256::digest(&message))?;
    let client = {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
        B64_URL
            .decode(&a.client_data_json)
            .map_err(|_| Error::Signature)?
    };
    let c: serde_json::Value = serde_json::from_slice(&client).map_err(|_| Error::Signature)?;
    if !origins
        .iter()
        .any(|o| c["origin"].as_str() == Some(o.as_str()))
    {
        return Err(Error::Signature);
    }
    Ok(())
}

/// Is the entry signed by the root it names? True of every valid entry.
pub fn verify(signed: &SignedEntry) -> Result<()> {
    if signed.entry.root.starts_with(WEBAUTHN_ROOT) {
        return check_assertion(signed);
    }
    let (msg, sig) = signed.signed()?;
    check(sig, &msg, &decode_public(&signed.entry.root)?)
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
        // is why a box takes one only under the agreement rule. Earlier
        // roots are added one recovery at a time, never claimed.
        if !new.entry.recovered_from.is_empty() {
            return Err(Error::Rejected("a first entry has no earlier roots".into()));
        }
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
        check_assertion_with(new, &old.entry.passkeys)?;
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
    // the earlier roots are the ones on file, and the root on file joins
    // them when, and only when, the recovery key below replaces it
    let mut earlier = old.entry.recovered_from.clone();
    if new.entry.root != old.entry.root {
        earlier.push(old.entry.root.clone());
    }
    if new.entry.recovered_from != earlier {
        return Err(Error::Rejected(
            "the earlier roots are not the ones on file".into(),
        ));
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
    // over the same bytes the root signed: v1 the canonical entry, v2 the
    // envelope's message
    check(rs, &new.signed()?.0, &decode_public(&old.entry.recovery)?)
}

/// A person leaving the fleet, in their own root's words
/// (docs/dev/leaving.md). Not part of the entry: the entry stays, so the
/// name stays theirs. A coming back is no longer taken; one already in the
/// fleet's log still reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Leave {
    pub name: String,
    /// the root that signs it, as the entry on file names it
    pub root: String,
    /// when they said it; a box takes it only near the time it is agreed
    pub at: u64,
    /// true: coming back, which undoes an earlier leaving
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub back: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedLeave {
    pub leave: Leave,
    /// over `leave_challenge`: an ed25519 signature by a device root, or a
    /// passkey root's assertion (base64 json, as in `SignedEntry`)
    pub signature: String,
}

/// What a root signs to leave: the statement under a label of
/// its own, so no signature over an entry or a sign-in counts as one.
/// An item's hash: sha256 of what was signed and the signatures over it,
/// so one signed item has one hash however its outer JSON is written
/// (docs/dev/identity-state.md, "Item hash"). The control plane's judge keys
/// what became of each item on it, and a gate refuses an item already sent
/// or already judged by it. The signatures are in it so a forgery over the
/// same body never shares a hash with the real item, and its refusal never
/// stands for it.
///
/// - a v2 envelope: its message, `commonty <kind> v2` NUL body; then NUL and
///   `sig`, and NUL and `recovery_sig` when there is one
/// - a v1 entry: its canonical entry, NUL, `signature` (and NUL and
///   `recovery_signature`); a v1 invite: the invite, NUL, `signature`; a v1
///   leave: the leave, NUL, `signature` - each as serde writes it
/// - anything else (a box's ack, or what reads as none of these): the
///   item's own JSON, keys sorted
pub fn item_hash(v: &serde_json::Value) -> String {
    use sha2::Digest as _;
    let signed: Option<Vec<u8>> = (|| {
        let with = |mut b: Vec<u8>, sigs: &[Option<&str>]| {
            for s in sigs.iter().flatten() {
                b.push(0);
                b.extend_from_slice(s.as_bytes());
            }
            b
        };
        if v.get("v").and_then(|x| x.as_u64()) == Some(2) {
            let e: envelope::Envelope = serde_json::from_value(v.clone()).ok()?;
            let m = e.message().ok()?;
            return Some(with(m, &[Some(&e.sig), e.recovery_sig.as_deref()]));
        }
        let o = v.as_object().filter(|o| o.len() == 1)?;
        let (k, x) = o.iter().next()?;
        match k.as_str() {
            "entry" => {
                let s: SignedEntry = serde_json::from_value(x.clone()).ok()?;
                let c = canonical(&s.entry).ok()?;
                Some(with(
                    c,
                    &[Some(&s.signature), s.recovery_signature.as_deref()],
                ))
            }
            "invite" => {
                let s: SignedInvite = serde_json::from_value(x.clone()).ok()?;
                Some(with(
                    serde_json::to_vec(&s.invite).ok()?,
                    &[Some(&s.signature)],
                ))
            }
            "leave" => {
                let s: SignedLeave = serde_json::from_value(x.clone()).ok()?;
                Some(with(
                    serde_json::to_vec(&s.leave).ok()?,
                    &[Some(&s.signature)],
                ))
            }
            _ => None,
        }
    })();
    let bytes = signed.unwrap_or_else(|| serde_json::to_vec(v).unwrap_or_default());
    sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn leave_challenge(l: &Leave) -> Result<Vec<u8>> {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"commonty leave v1\0");
    h.update(serde_json::to_vec(l)?);
    Ok(h.finalize().to_vec())
}

pub fn sign_leave(leave: Leave, root: &SigningKey) -> Result<SignedLeave> {
    if leave.root != encode_public(&root.verifying_key()) {
        return Err(Error::Key(
            "that is not the root the statement names".into(),
        ));
    }
    let sig: Signature = root.sign(&leave_challenge(&leave)?);
    Ok(SignedLeave {
        leave,
        signature: B64.encode(sig.to_bytes()),
    })
}

/// Signed by the root of `on_file`, the entry the fleet holds for the name;
/// a passkey root's key is taken from that entry, never from the statement.
pub fn verify_leave(s: &SignedLeave, on_file: &Entry) -> Result<()> {
    if s.leave.name != on_file.name {
        return Err(Error::Rejected("that statement is someone else's".into()));
    }
    if s.leave.root != on_file.root {
        return Err(Error::Rejected("not signed by the root on file".into()));
    }
    let expected = leave_challenge(&s.leave)?;
    if on_file.root.starts_with(WEBAUTHN_ROOT) {
        let (id, digest) = root_parts(&on_file.root)
            .ok_or_else(|| Error::Key("a passkey root must commit to its key".into()))?;
        let passkey = on_file
            .passkeys
            .iter()
            .find(|p| p.id == id && cose_digest(&p.cred) == digest)
            .ok_or_else(|| Error::Key("the entry on file has no root passkey".into()))?;
        let a: Assertion =
            serde_json::from_slice(&B64.decode(&s.signature).map_err(|_| Error::Signature)?)
                .map_err(|_| Error::Signature)?;
        return assertion_over(passkey, &a, &expected);
    }
    let sig = B64.decode(&s.signature).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    decode_public(&on_file.root)?
        .verify(&expected, &sig)
        .map_err(|_| Error::Signature)
}

/// A library id: 32 hex characters, and nothing else. It becomes a folder
/// name, a bucket prefix and a command-line argument on every machine that
/// opens the library, so it can be nothing that means anything there - no
/// separator, no dot, no leading dash.
pub fn valid_library_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Key material an entry still keeps for a passkey: a key a browser made
/// from one, or a library key sealed to it. A passkey answers its PRF to
/// any page of the domain, so none of it may be published again.
pub fn holds_passkey_keys(entry: &Entry) -> bool {
    entry.passkeys.iter().any(|p| p.library_key.is_some())
        || entry
            .libraries
            .iter()
            .flat_map(|l| l.keys.iter().chain(l.readers.iter().flat_map(|r| &r.keys)))
            .any(|k| k.to.starts_with("passkey:"))
}

/// The same entry with all of that gone; true if anything was. What else
/// opens each library (devices, root, paper) is left as it was.
pub fn drop_passkey_keys(entry: &mut Entry) -> bool {
    let had = holds_passkey_keys(entry);
    for p in &mut entry.passkeys {
        p.library_key = None;
    }
    for l in &mut entry.libraries {
        l.keys.retain(|k| !k.to.starts_with("passkey:"));
        for r in &mut l.readers {
            r.keys.retain(|k| !k.to.starts_with("passkey:"));
        }
    }
    had
}

pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !s.starts_with('.')
}

/// Adding a device by code: what the new device commits to before it sees
/// the approving device's number - its key and its own number, hashed.
pub fn pairing_commit(public_key: &str, mine: &str) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"commonty pairing commit v1\0");
    h.update(public_key.as_bytes());
    h.update(b"\0");
    h.update(mine.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The six digits both screens show: the key and both devices' numbers,
/// worked out on each device. Neither the box between them nor either
/// device alone chooses them.
pub fn pairing_digits(public_key: &str, new_device: &str, approver: &str) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"commonty pairing v1\0");
    h.update(public_key.as_bytes());
    h.update(b"\0");
    h.update(new_device.as_bytes());
    h.update(b"\0");
    h.update(approver.as_bytes());
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// the purpose of a device token minted for directory reads alone: no other
/// route takes it, and it names the one host it is for
pub const DIRECTORY_READ: &str = "directory-read";

/// the header a directory read carries a key's proof in (docs/dev/gate.md)
pub const READ_PROOF_HEADER: &str = "x-dd-key-proof";
/// how far a proof's time may be from the box's clock, either way
pub const READ_PROOF_SKEW: u64 = 300;

fn read_proof_bytes(name: &str, at: u64) -> Vec<u8> {
    format!("dd-directory-read\n{name}\n{at}").into_bytes()
}

/// `<public key>.<at>.<signature>`: this key asks for `name`'s entry at `at`
pub fn read_proof(key: &SigningKey, name: &str, at: u64) -> String {
    let sig: Signature = key.sign(&read_proof_bytes(name, at));
    format!(
        "{}.{at}.{}",
        encode_public(&key.verifying_key()),
        B64.encode(sig.to_bytes())
    )
}

/// The public key that made `proof` for `name`, if it did and `now` is
/// within the skew of when it says it did.
pub fn check_read_proof(proof: &str, name: &str, now: u64) -> Option<String> {
    let mut parts = proof.trim().split('.');
    let (public, at, sig) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let at: u64 = at.parse().ok()?;
    if at.abs_diff(now) > READ_PROOF_SKEW {
        return None;
    }
    let key = decode_public(public).ok()?;
    let sig = Signature::from_slice(&B64.decode(sig).ok()?).ok()?;
    key.verify(&read_proof_bytes(name, at), &sig).ok()?;
    Some(public.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_reader_who_writes_says_so_and_one_who_does_not_reads_as_before() {
        // an entry signed before writers existed keeps its bytes, so its
        // signature still holds
        let before = r#"{"name":"bo","keys":[{"to":"device:g","sealed":"x"}]}"#;
        let r: super::Reader = serde_json::from_str(before).unwrap();
        assert!(!r.writes);
        assert_eq!(serde_json::to_string(&r).unwrap(), before);
        let w = super::Reader { writes: true, ..r };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.ends_with(r#","writes":true}"#), "{json}");
        let back: super::Reader = serde_json::from_str(&json).unwrap();
        assert!(back.writes);
    }

    #[test]
    fn a_read_proof_is_for_one_name_and_a_short_while() {
        let k = super::generate();
        let me = super::encode_public(&k.verifying_key());
        let p = super::read_proof(&k, "sarah", 1000);
        assert_eq!(super::check_read_proof(&p, "sarah", 1100), Some(me));
        assert_eq!(super::check_read_proof(&p, "tom", 1100), None);
        assert_eq!(super::check_read_proof(&p, "sarah", 1000 + 301), None);
        let forged = p.replace(".1000.", ".1001.");
        assert_eq!(super::check_read_proof(&forged, "sarah", 1000), None);
        assert_eq!(super::check_read_proof("x.y", "sarah", 1000), None);
    }

    #[test]
    fn pairing_sums_are_the_browsers() {
        // the same sums as servers/fleet/web/devices.js makes with SubtleCrypto
        assert_eq!(
            super::pairing_commit("PK", "aa"),
            "d82c73c3d371a64897284f4167a9b89cee2999c7fcef74de148ce8ecae0649f8"
        );
        assert_eq!(super::pairing_digits("PK", "aa", "bb"), "039979");
    }

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
            recovered_from: vec![],
            devices: vec![],
            passkeys: vec![],
            libraries: vec![],
            grant: Some(Grant {
                invite: inv.clone(),
                redeemed: 200,
                proof: prove(&ck, &inv.invite.public_key, root, 200),
            }),
            terms: None,
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

    #[test]
    fn a_new_sign_up_carries_the_terms_it_agreed_to_and_an_old_entry_needs_none() {
        let root = generate();
        let mut e = Entry {
            name: "tom".into(),
            root: encode_public(&root.verifying_key()),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![Device {
                fingerprint: "f".into(),
                public_key: encode_public(&generate().verifying_key()),
                added: 1,
            }],
            passkeys: vec![],
            libraries: vec![],
            grant: None,
            terms: None,
            version: 1,
            updated: TERMS_ASKED_FROM - 1,
        };
        // from before: as it was, byte for byte, and still good
        let old = sign(e.clone(), &root).unwrap();
        assert!(
            !String::from_utf8(canonical(&e).unwrap())
                .unwrap()
                .contains("terms")
        );
        verify(&old).unwrap();
        agreed_terms(&e).unwrap();
        // made since: refused without the terms, taken with them, signed in
        e.updated = TERMS_ASKED_FROM;
        assert!(agreed_terms(&e).is_err());
        e.terms = Some(TERMS.into());
        agreed_terms(&e).unwrap();
        let new = sign(e.clone(), &root).unwrap();
        verify(&new).unwrap();
        let mut changed = new.clone();
        changed.entry.terms = Some("2020-01-01".into());
        assert!(
            verify(&changed).is_err(),
            "the terms are signed with the rest"
        );
        e.terms = Some("yes".into());
        assert!(agreed_terms(&e).is_err());
    }

    #[test]
    fn leaving_is_said_by_the_root_on_file_and_nobody_else() {
        let root = generate();
        let entry = Entry {
            name: "tom".into(),
            root: encode_public(&root.verifying_key()),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![],
            passkeys: vec![],
            libraries: vec![],
            grant: None,
            terms: None,
            version: 1,
            updated: 1,
        };
        let leave = Leave {
            name: "tom".into(),
            root: entry.root.clone(),
            at: 100,
            back: false,
        };
        let s = sign_leave(leave.clone(), &root).unwrap();
        verify_leave(&s, &entry).unwrap();
        assert!(!serde_json::to_string(&s).unwrap().contains("back"));
        let mut changed = s.clone();
        changed.leave.back = true;
        assert!(
            verify_leave(&changed, &entry).is_err(),
            "a leave is not a coming back"
        );
        let other = generate();
        assert!(sign_leave(leave.clone(), &other).is_err());
        let forged = SignedLeave {
            leave: leave.clone(),
            signature: sign_leave(
                Leave {
                    root: encode_public(&other.verifying_key()),
                    ..leave.clone()
                },
                &other,
            )
            .unwrap()
            .signature,
        };
        assert!(verify_leave(&forged, &entry).is_err());
        let mut someone_else = entry.clone();
        someone_else.name = "ann".into();
        assert!(verify_leave(&s, &someone_else).is_err());
        // an entry signature is never a statement
        let signed = sign(entry.clone(), &root).unwrap();
        let lifted = SignedLeave {
            leave,
            signature: signed.signature,
        };
        assert!(verify_leave(&lifted, &entry).is_err());
    }

    #[test]
    fn a_device_root_says_a_controllers_name_and_nobody_else_does() {
        let root = generate();
        let entry = Entry {
            name: "tom".into(),
            root: encode_public(&root.verifying_key()),
            recovery: String::new(),
            recovered_from: vec![],
            terms: None,
            devices: vec![],
            passkeys: vec![],
            libraries: vec![],
            grant: None,
            version: 1,
            updated: 1,
        };
        let ctl = encode_public(&generate().verifying_key());
        let label = controller_label(&ctl).unwrap();
        let name = format!("{label}.home.commonty.org");
        let msg = controller_name_message(&name, &ctl);
        assert!(msg.starts_with(format!("commonty controller name v1\0{name}\0").as_bytes()));
        let sig = B64.encode(root.sign(&msg).to_bytes());
        let o = ["https://home.commonty.org".to_string()];
        controller_name_signed(&entry, &name, &ctl, &sig, &o).unwrap();
        let other = B64.encode(generate().sign(&msg).to_bytes());
        assert!(controller_name_signed(&entry, &name, &ctl, &other, &o).is_err());
        assert!(controller_name_signed(&entry, "xyz.home.commonty.org", &ctl, &sig, &o).is_err());
        // the root's signature on its own entry is no name
        let entry_sig = sign(entry.clone(), &root).unwrap().signature;
        assert!(controller_name_signed(&entry, &name, &ctl, &entry_sig, &o).is_err());
        // another controller's label is never this one's, even on the root's
        // word: a label is held by whoever holds its key
        let theirs = encode_public(&generate().verifying_key());
        let their_name = format!("{}.home.commonty.org", controller_label(&theirs).unwrap());
        let squat = B64.encode(
            root.sign(&controller_name_message(&their_name, &ctl))
                .to_bytes(),
        );
        let e = controller_name_signed(&entry, &their_name, &ctl, &squat, &o).unwrap_err();
        assert!(format!("{e}").contains("not this controller's"), "{e}");
        assert_eq!(controller_label(&ctl).unwrap(), label);
        assert!(
            label.len() == 16
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        );
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
                recovered_from: vec![],
                devices: vec![],
                passkeys: vec![pk],
                grant: None,
                libraries: vec![],
                terms: None,
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
                recovered_from: vec![],
                devices: vec![dev(&generate())],
                terms: None,
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
            recovered_from: vec![],
            devices: vec![dev(&d1)],
            terms: None,
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
        e3.recovered_from = vec![e2.root.clone()];
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
        // - but for one that claims earlier roots, which it cannot check
        accept(None, &s2).unwrap();
        assert!(accept(None, &s3).is_err());
        // but never an entry whose signature is not by the root it names
        let mut e0 = e1.clone();
        e0.root = encode_public(&stranger.verifying_key());
        let mut bad = sign(e0, &stranger).unwrap();
        bad.signature = s1.signature.clone();
        assert!(accept(None, &bad).is_err());
    }

    #[test]
    fn membership_and_its_revocation_follow_a_recovery_and_nothing_else() {
        let (root, recovery, next) = (generate(), generate(), generate());
        let release = generate();
        let ck = code_key("abcde-fghij");
        let inv = sign_invite(
            Invite {
                public_key: encode_public(&ck.verifying_key()),
                issued: 100,
                expires: 400,
            },
            &release,
        )
        .unwrap();
        let old = encode_public(&root.verifying_key());
        let e1 = Entry {
            name: "sarah".into(),
            root: old.clone(),
            recovery: encode_public(&recovery.verifying_key()),
            recovered_from: vec![],
            devices: vec![dev(&generate())],
            passkeys: vec![],
            grant: Some(Grant {
                invite: inv.clone(),
                redeemed: 200,
                proof: prove(&ck, &inv.invite.public_key, &old, 200),
            }),
            libraries: vec![],
            terms: None,
            version: 1,
            updated: 1,
        };
        let s1 = sign(e1.clone(), &root).unwrap();
        let mut e2 = e1.clone();
        e2.version = 2;
        e2.root = encode_public(&next.verifying_key());
        e2.recovery = encode_public(&generate().verifying_key());
        // the root on file must join the earlier ones, exactly
        e2.recovered_from = vec![];
        assert!(
            accept(
                Some(&s1),
                &sign_recovery(e2.clone(), &next, &recovery).unwrap()
            )
            .is_err()
        );
        e2.recovered_from = vec![old.clone(), "someone else's root".into()];
        assert!(
            accept(
                Some(&s1),
                &sign_recovery(e2.clone(), &next, &recovery).unwrap()
            )
            .is_err()
        );
        e2.recovered_from = vec![old.clone()];
        let s2 = sign_recovery(e2.clone(), &next, &recovery).unwrap();
        accept(Some(&s1), &s2).unwrap();
        // a later update keeps them, and cannot add one without a recovery
        let mut e3 = e2.clone();
        e3.version = 3;
        assert!(accept(Some(&s2), &sign(e3.clone(), &next).unwrap()).is_ok());
        e3.recovered_from
            .push(encode_public(&generate().verifying_key()));
        assert!(accept(Some(&s2), &sign(e3, &next).unwrap()).is_err());
        // a first sight may not claim another's root to borrow its membership
        let thief = generate();
        let mut t = e1.clone();
        t.name = "mallory".into();
        t.root = encode_public(&thief.verifying_key());
        t.recovery = String::new();
        t.recovered_from = vec![old.clone()];
        assert!(accept(None, &sign(t, &thief).unwrap()).is_err());

        let (old_id, new_id) = (member_id(&old), member_id(&e2.root));
        assert_eq!(member_ids(&e2), vec![new_id.clone(), old_id.clone()]);
        let r = release.verifying_key();
        // on the list by the old root: still a member once recovered
        assert!(is_member(&e2, std::slice::from_ref(&old_id), &[], None));
        // the owner's removal of the old root holds across the recovery
        assert!(!is_member(
            &e2,
            std::slice::from_ref(&old_id),
            std::slice::from_ref(&old_id),
            Some(&r)
        ));
        assert!(!is_member(
            &e2,
            std::slice::from_ref(&new_id),
            std::slice::from_ref(&old_id),
            Some(&r)
        ));
        assert!(!is_member(
            &e2,
            std::slice::from_ref(&old_id),
            std::slice::from_ref(&new_id),
            Some(&r)
        ));
        // the invite the old root redeemed comes along, and a revocation
        // still beats it
        verify_grant(&e2, &r).unwrap();
        assert!(is_member(&e2, &[], &[], Some(&r)));
        assert!(!is_member(&e2, &[], &[old_id], Some(&r)));
        let mut lifted = e2.clone();
        lifted.recovered_from = vec![];
        assert!(verify_grant(&lifted, &r).is_err());
    }

    /// What a client holds another member's entry to: the root the signed
    /// list names as that name's current one, and nothing else. What a
    /// person follows to their own entry on recovery: proven handovers from
    /// a root they know. A box can sign an entry under a root of its own and
    /// write any earlier roots into it; it cannot sign a handover.
    #[test]
    fn an_entry_is_anchored_by_its_listed_root_and_a_recovery_by_proofs_alone() {
        let (r0, p0, r1, p1, r2) = (generate(), generate(), generate(), generate(), generate());
        let e0 = Entry {
            name: "sarah".into(),
            root: encode_public(&r0.verifying_key()),
            recovery: encode_public(&p0.verifying_key()),
            recovered_from: vec![],
            devices: vec![dev(&generate())],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            terms: None,
            version: 1,
            updated: 1,
        };
        let s0 = sign(e0.clone(), &r0).unwrap();
        let id0 = member_id(&e0.root);
        let named = |ids: &[(&String, &str)]| -> std::collections::BTreeMap<String, String> {
            ids.iter()
                .map(|(id, n)| ((*id).clone(), name_tag(id, n)))
                .collect()
        };
        let listed = [id0.clone()];
        let hers = named(&[(&id0, "sarah")]);
        anchored(&s0, &listed, &[], &hers).unwrap();
        assert!(anchored(&s0, &[], &[], &hers).is_err(), "not on the list");
        assert!(anchored(&s0, &listed, &listed, &hers).is_err(), "revoked");
        assert!(anchored(&s0, &listed, &[], &named(&[(&id0, "tom")])).is_err());
        assert!(anchored(&s0, &listed, &[], &Default::default()).is_err());

        // two recoveries, then an ordinary update under the last root
        let mut e1 = e0.clone();
        e1.root = encode_public(&r1.verifying_key());
        e1.recovery = encode_public(&p1.verifying_key());
        e1.recovered_from = vec![e0.root.clone()];
        e1.version = 2;
        let s1 = sign_recovery(e1.clone(), &r1, &p0).unwrap();
        let mut e2 = e1.clone();
        e2.root = encode_public(&r2.verifying_key());
        e2.recovery = encode_public(&generate().verifying_key());
        e2.recovered_from = vec![e0.root.clone(), e1.root.clone()];
        e2.version = 3;
        let s2 = sign_recovery(e2.clone(), &r2, &p1).unwrap();
        let mut e3 = e2.clone();
        e3.version = 4;
        let s3 = sign(e3, &r2).unwrap();
        let chain = [
            Handover {
                before: s0.clone(),
                after: s1.clone(),
            },
            Handover {
                before: s1.clone(),
                after: s2.clone(),
            },
        ];
        // to anyone else, the recovered entry anchors only once a release
        // lists its new root
        assert!(anchored(&s3, &listed, &[], &hers).is_err());
        let id2 = member_id(&e2.root);
        anchored(
            &s3,
            std::slice::from_ref(&id2),
            &[],
            &named(&[(&id2, "sarah")]),
        )
        .unwrap();
        // and a superseded root vouches for nothing: a thief holding r0
        // signs sarah's name with it, and the release names r2 as hers
        let mut stolen = e0.clone();
        stolen.devices = vec![dev(&generate())];
        stolen.version = 9;
        let stolen = sign(stolen, &r0).unwrap();
        let both = [id0.clone(), id2.clone()];
        let after = named(&[(&id2, "sarah")]);
        assert!(anchored(&stolen, &both, &[], &after).is_err());
        // a list naming two current roots for one name names none
        assert!(anchored(&s3, &both, &[], &named(&[(&id0, "sarah"), (&id2, "sarah")])).is_err());

        // the person's own recovery follows the proofs from a root they know
        follows(&s3, &chain, &id0).unwrap();
        follows(&s3, &chain, &member_id(&e1.root)).unwrap();
        follows(&s0, &[], &id0).unwrap();
        assert!(follows(&s3, &chain[1..], &id0).is_err());
        assert!(follows(&s3, &[], &id0).is_err());
        // a bad handover served beside the good one does not hide it
        let mut decoy = chain.to_vec();
        let mut bent = s1.clone();
        bent.recovery_signature = Some(s0.signature.clone());
        decoy.insert(
            0,
            Handover {
                before: s0.clone(),
                after: bent,
            },
        );
        follows(&s3, &decoy, &id0).unwrap();
        // two real versions under one root are no recovery: refused, and
        // nothing goes round in circles over them
        let mut e3b = e2.clone();
        e3b.version = 5;
        let s3b = sign(e3b, &r2).unwrap();
        let same = [
            Handover {
                before: s3.clone(),
                after: s3b.clone(),
            },
            Handover {
                before: s3b.clone(),
                after: s3.clone(),
            },
        ];
        assert!(follows(&s3b, &same, &id0).is_err());
        let mut looping = same.to_vec();
        looping.extend(chain.iter().cloned());
        follows(&s3b, &looping, &id0).unwrap();
        // nor can a recovery change the name it is under
        let mut renamed = e1.clone();
        renamed.name = "tom".into();
        let renamed = sign_recovery(renamed, &r1, &p0).unwrap();
        let h = [Handover {
            before: s0.clone(),
            after: renamed.clone(),
        }];
        assert!(follows(&renamed, &h, &id0).is_err());

        // tom, a member, signs an entry under sarah's name with his own root
        let tom = generate();
        let mut as_her = e0.clone();
        as_her.root = encode_public(&tom.verifying_key());
        let as_her = sign(as_her, &tom).unwrap();
        let tom_id = member_id(&as_her.entry.root);
        let both = [id0.clone(), tom_id.clone()];
        let names = named(&[(&id0, "sarah"), (&tom_id, "tom")]);
        assert!(anchored(&as_her, &both, &[], &names).is_err());

        // a box's own root, naming sarah's real one as the root it replaced
        let boxed = generate();
        let mut f = e0.clone();
        f.root = encode_public(&boxed.verifying_key());
        f.recovery = encode_public(&boxed.verifying_key());
        f.recovered_from = vec![e0.root.clone()];
        f.version = 2;
        // co-signed by a paper key of its own, with a "before" it signed
        // too, or the real one: neither is a handover accept takes
        let forged = sign_recovery(f.clone(), &boxed, &boxed).unwrap();
        let mut fake_before = e0.clone();
        fake_before.recovery = encode_public(&boxed.verifying_key());
        let unsigned = SignedEntry {
            entry: fake_before.clone(),
            signature: s0.signature.clone(),
            recovery_signature: None,
        };
        for before in [s0.clone(), unsigned] {
            let h = [Handover {
                before,
                after: forged.clone(),
            }];
            assert!(follows(&forged, &h, &id0).is_err());
            assert!(anchored(&forged, &listed, &[], &hers).is_err());
        }
    }

    #[test]
    fn a_member_list_is_the_release_keys_and_nobody_elses() {
        let release = generate();
        let list = MemberList {
            counter: 7,
            members: vec![member_id("a")],
            names: [(member_id("a"), name_tag(&member_id("a"), "amy"))].into(),
            ..Default::default()
        };
        let s = sign_member_list(list.clone(), &release).unwrap();
        verify_member_list(&s, &release.verifying_key()).unwrap();
        assert!(verify_member_list(&s, &generate().verifying_key()).is_err());
        let mut grown = s.clone();
        grown.list.members.push(member_id("b"));
        assert!(verify_member_list(&grown, &release.verifying_key()).is_err());
        let mut renamed = s.clone();
        renamed
            .list
            .names
            .insert(member_id("a"), name_tag(&member_id("a"), "eve"));
        assert!(verify_member_list(&renamed, &release.verifying_key()).is_err());
        // the same key's signature over the bare json, as it signs an
        // invite, is no member list
        let bare: Signature = release.sign(&serde_json::to_vec(&list).unwrap());
        let mut swapped = s.clone();
        swapped.signature = B64.encode(bare.to_bytes());
        assert!(verify_member_list(&swapped, &release.verifying_key()).is_err());
    }

    #[test]
    fn passkey_keys_are_found_and_dropped_and_nothing_else_is() {
        let sealed = |to: &str| SealedKey {
            to: to.into(),
            sealed: "x".into(),
        };
        let mut e = Entry {
            name: "amy".into(),
            root: "r".into(),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![],
            passkeys: vec![Passkey {
                id: "p1".into(),
                cred: serde_json::json!({}),
                added: 1,
                rp_id: None,
                library_key: None,
            }],
            grant: None,
            libraries: vec![Library {
                id: "0".repeat(32),
                keys: vec![sealed("device:f"), sealed("root"), sealed("recovery")],
                readers: vec![Reader {
                    name: "bo".into(),
                    keys: vec![sealed("device:g")],
                    writes: false,
                }],
                created: 1,
            }],
            terms: None,
            version: 1,
            updated: 1,
        };
        assert!(!holds_passkey_keys(&e));
        let clean = e.clone();
        assert!(!drop_passkey_keys(&mut e));
        assert_eq!(e, clean);

        e.libraries[0].keys.push(sealed("passkey:p1"));
        assert!(holds_passkey_keys(&e));
        assert!(drop_passkey_keys(&mut e));
        assert_eq!(e, clean, "only the passkey's seal went");

        e.libraries[0].readers[0].keys.push(sealed("passkey:q"));
        e.passkeys[0].library_key = Some("k".into());
        assert!(holds_passkey_keys(&e));
        assert!(drop_passkey_keys(&mut e));
        assert_eq!(e, clean, "the passkey stays, without its key");
    }
}
