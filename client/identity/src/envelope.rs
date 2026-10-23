//! The signed envelope (v2, docs/dev/identity-state.md): the bytes that were
//! signed travel as they were signed, and a checker verifies over them before
//! it reads anything in them. Nothing is ever rebuilt from a parsed value, so
//! any language checks an envelope the same way.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The kinds a person signs
pub const ENTRY: &str = "entry";
pub const LEAVE: &str = "leave";
pub const INVITE: &str = "invite";
pub const ENLIST: &str = "enlist";
pub const UNENLIST: &str = "unenlist";
pub const EXTEND: &str = "extend";
pub const SIGNOUT: &str = "signout";

/// A member's root naming a box as theirs: the box joined with `ticket`
/// (the id of a join ticket of theirs), `at` its own date (seconds). The
/// judge (state/ident enlist) holds the rest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Enlist {
    /// member_id of the root signing it
    pub member: String,
    #[serde(rename = "box")]
    pub box_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ticket: String,
    pub at: u64,
}

impl Enlist {
    /// The body for `kind` (ENLIST or UNENLIST: an unenlist names no ticket)
    pub fn new(kind: &str, root: &str, box_name: &str, ticket: &str, at: u64) -> Self {
        Self {
            member: crate::member_id(root),
            box_name: box_name.to_string(),
            ticket: if kind == ENLIST {
                ticket.to_string()
            } else {
                String::new()
            },
            at,
        }
    }
}

/// The fleet's admin keeping `name`'s access until `until` (seconds)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Extend {
    pub name: String,
    pub until: u64,
    pub at: u64,
}

/// A member's root signing them out everywhere: every sign-in begun before
/// `at` (seconds) is over, on every box, but `keep` - the session id of the
/// browser or app that asked, if it keeps its own. The judge (state/ident
/// signout.go) holds the rest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignOut {
    /// member_id of the root signing it
    pub member: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub keep: String,
}

impl SignOut {
    pub fn new(root: &str, at: u64, keep: &str) -> Self {
        Self {
            member: crate::member_id(root),
            at,
            keep: keep.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope {
    pub v: u8,
    pub kind: String,
    /// base64 (standard, padded) of the body's bytes, exactly as signed
    pub body: String,
    /// base64 ed25519 signature over `message`, or base64 of a passkey
    /// assertion whose challenge is sha256(`message`)
    pub sig: String,
    /// an entry changing its root: the same message signed by the recovery
    /// key on file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_sig: Option<String>,
}

/// What is signed: the kind and version, then the body's bytes
pub fn message(kind: &str, body: &[u8]) -> Vec<u8> {
    let mut m = format!("commonty {kind} v2\0").into_bytes();
    m.extend_from_slice(body);
    m
}

impl Envelope {
    /// The body's bytes, as signed. Says nothing about the signature.
    pub fn body_bytes(&self) -> Result<Vec<u8>> {
        B64.decode(&self.body)
            .map_err(|_| Error::Rejected("the body is not base64".into()))
    }

    pub fn message(&self) -> Result<Vec<u8>> {
        Ok(message(&self.kind, &self.body_bytes()?))
    }

    /// The body, read as JSON. Call only after the signature passed.
    pub fn read<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.body_bytes()?)?)
    }

    /// Verified with an ed25519 key: `sig` over `message`
    pub fn verify_ed25519(&self, key: &VerifyingKey) -> Result<()> {
        verify_ed25519(&self.sig, &self.message()?, key)
    }
}

/// `body` signed as `kind` by an ed25519 key
pub fn seal(kind: &str, body: Vec<u8>, key: &SigningKey) -> Envelope {
    let sig: Signature = key.sign(&message(kind, &body));
    Envelope {
        v: 2,
        kind: kind.to_string(),
        body: B64.encode(&body),
        sig: B64.encode(sig.to_bytes()),
        recovery_sig: None,
    }
}

/// `value` written as JSON, signed as `kind`
pub fn seal_json<T: Serialize>(kind: &str, value: &T, key: &SigningKey) -> Result<Envelope> {
    Ok(seal(kind, serde_json::to_vec(value)?, key))
}

pub fn verify_ed25519(sig_b64: &str, msg: &[u8], key: &VerifyingKey) -> Result<()> {
    let sig = B64.decode(sig_b64).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    key.verify(msg, &sig).map_err(|_| Error::Signature)
}

/// What a passkey signs for an envelope: sha256 of the message, as the
/// WebAuthn challenge
pub fn passkey_challenge(kind: &str, body: &[u8]) -> Vec<u8> {
    use sha2::Digest as _;
    sha2::Sha256::digest(message(kind, body)).to_vec()
}

/// Signed by `root`, as an entry's root is written: a passkey root by an
/// assertion over the envelope (checked against `passkeys`, the entry's),
/// an ed25519 root by its own signature. The one check every kind signed
/// by a root uses, here and in the judge (state/ident VerifyRoot).
pub fn verify_root(env: &Envelope, root: &str, passkeys: &[crate::Passkey]) -> Result<()> {
    verify_root_sig(env, &env.sig, root, passkeys)
}

/// verify_root over another signature the envelope carries (an entry's
/// recovery_sig, by the recovery key on file)
pub fn verify_root_sig(
    env: &Envelope,
    sig: &str,
    root: &str,
    passkeys: &[crate::Passkey],
) -> Result<()> {
    if root.starts_with(crate::WEBAUTHN_ROOT) {
        crate::check_envelope_assertion(
            root,
            passkeys,
            sig,
            &passkey_challenge(&env.kind, &env.body_bytes()?),
        )
    } else {
        let key = crate::decode_public(root)?;
        verify_ed25519(sig, &env.message()?, &key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_over_one_kind_is_no_signature_over_another() {
        let k = SigningKey::from_bytes(&[7; 32]);
        let e = seal(ENTRY, b"{\"name\":\"ada\"}".to_vec(), &k);
        assert!(e.verify_ed25519(&k.verifying_key()).is_ok());
        let mut other = e.clone();
        other.kind = LEAVE.into();
        assert!(other.verify_ed25519(&k.verifying_key()).is_err());
        let mut changed = e.clone();
        changed.body = B64.encode(b"{\"name\":\"bob\"}");
        assert!(changed.verify_ed25519(&k.verifying_key()).is_err());
    }
}
