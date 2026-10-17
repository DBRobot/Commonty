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
