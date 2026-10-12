//! Signing in once for every box, with no box vouching for another.
//!
//! The sign-in page is not any box's: it is the id Worker's
//! (servers/system/id), at id.<domain>. There the browser makes a key of its
//! own (`key`, P-256, which the page cannot read out) and the person's
//! passkey signs a delegation to it: this key speaks for this person, until
//! then. A box that has no session for someone sends them to that page with
//! a one-off challenge of its own (`nonce`); the page signs the box's name
//! and that challenge with the key, and the box checks both signatures
//! against the passkey in its own copy of the directory. Then it sets a
//! cookie for its own name alone. Nothing a box receives works on another:
//! the hand-off names its host, and its cookie is signed by its own secret.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::{Assertion, Entry, Error, Result};

/// The longest a delegation may run, and a box's sessions with it.
pub const LIFE: u64 = 8 * 3600;
/// how far ahead of a box's clock a browser's may be
const SKEW: u64 = 120;

/// What the passkey signed, and what it signed it over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    pub user: String,
    /// the credential id of the passkey that signed it
    pub passkey: String,
    /// the browser's key: P-256, SPKI DER, base64url
    pub key: String,
    /// the sign-in's id: every box's session from it carries it, so ending
    /// it ends them all
    pub sid: String,
    pub iat: u64,
    pub exp: u64,
    pub assertion: Assertion,
}

/// What the passkey signs: the delegation's fields under a label of their
/// own, so no other signature of the same passkey can pass for one.
pub fn delegation_challenge(user: &str, key: &str, sid: &str, iat: u64, exp: u64) -> Vec<u8> {
    let mut h = sha2::Sha256::new();
    h.update(format!(
        "commonty session v1\0{user}\0{key}\0{sid}\0{iat}\0{exp}"
    ));
    h.finalize().to_vec()
}

/// What the browser's key signs for a box: its name and its challenge.
pub fn handoff_message(host: &str, nonce: &str) -> Vec<u8> {
    format!("commonty handoff v1\0{host}\0{nonce}").into_bytes()
}

/// Whether `d` is this person's passkey speaking, made on the sign-in page
/// (`origin`, exactly) and good `now`. `entry` is the box's own copy.
pub fn check_delegation(entry: &Entry, d: &Delegation, origin: &str, now: u64) -> Result<()> {
    if d.user != entry.name {
        return Err(Error::Rejected("not this entry's".into()));
    }
    if d.sid.len() < 16
        || d.sid.len() > 64
        || !d
            .sid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::Rejected("not a sign-in id".into()));
    }
    if d.iat > now + SKEW {
        return Err(Error::Rejected("signed in the future".into()));
    }
    if d.exp <= now {
        return Err(Error::Rejected("run out".into()));
    }
    if d.exp < d.iat || d.exp - d.iat > LIFE + SKEW {
        return Err(Error::Rejected("longer than a sign-in lasts".into()));
    }
    let challenge = delegation_challenge(&d.user, &d.key, &d.sid, d.iat, d.exp);
    crate::check_login(
        entry,
        &d.passkey,
        &d.assertion,
        &challenge,
        &[origin.to_string()],
    )
}

/// Whether the delegated key signed this box's challenge under this name.
/// `sig` is base64url: r||s as WebCrypto gives it, or DER.
pub fn check_handoff(d: &Delegation, host: &str, nonce: &str, sig: &str) -> Result<()> {
    use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier as _};
    use p256::pkcs8::DecodePublicKey as _;
    let der = B64_URL
        .decode(&d.key)
        .map_err(|_| Error::Key("not base64url".into()))?;
    let key = VerifyingKey::from_public_key_der(&der)
        .map_err(|_| Error::Key("not a P-256 key".into()))?;
    let raw = B64_URL.decode(sig).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&raw)
        .or_else(|_| Signature::from_der(&raw))
        .map_err(|_| Error::Signature)?;
    key.verify(&handoff_message(host, nonce), &sig)
        .map_err(|_| Error::Signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Passkey, generate};
    use p256::ecdsa::{SigningKey, signature::Signer as _};
    use p256::pkcs8::EncodePublicKey as _;

    const ORIGIN: &str = "https://id.commonty.org";
    const NOW: u64 = 1_800_000_000;

    /// A passkey as an authenticator would be: a P-256 key whose public half
    /// is on file the way webauthn-rs serialises it, and that signs
    /// assertions the way a browser hands them over.
    struct Authenticator {
        key: SigningKey,
        id: String,
    }

    impl Authenticator {
        fn new(id: &str) -> Self {
            Self {
                key: SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng),
                id: id.into(),
            }
        }
        fn on_file(&self) -> Passkey {
            use webauthn_rs_core::proto::{
                COSEAlgorithm, COSEEC2Key, COSEKey, COSEKeyType, ECDSACurve,
            };
            let point = self.key.verifying_key().to_encoded_point(false);
            let cose = COSEKey {
                type_: COSEAlgorithm::ES256,
                key: COSEKeyType::EC_EC2(COSEEC2Key {
                    curve: ECDSACurve::SECP256R1,
                    x: point.x().unwrap().to_vec(),
                    y: point.y().unwrap().to_vec(),
                }),
            };
            Passkey {
                id: self.id.clone(),
                cred: serde_json::json!({ "cred": { "cred": cose } }),
                added: 0,
                rp_id: Some("commonty.org".into()),
                library_key: None,
            }
        }
        fn assert(&self, challenge: &[u8], origin: &str, rp: &str) -> Assertion {
            let client = serde_json::json!({
                "type": "webauthn.get",
                "challenge": B64_URL.encode(challenge),
                "origin": origin,
            })
            .to_string();
            let mut auth = sha2::Sha256::digest(rp.as_bytes()).to_vec();
            auth.extend_from_slice(&[0b0000_0101, 0, 0, 0, 1]);
            let mut signed = auth.clone();
            signed.extend_from_slice(&sha2::Sha256::digest(client.as_bytes()));
            let sig: p256::ecdsa::Signature = self.key.sign(&signed);
            Assertion {
                authenticator_data: B64_URL.encode(&auth),
                client_data_json: B64_URL.encode(client.as_bytes()),
                signature: B64_URL.encode(sig.to_der().as_bytes()),
            }
        }
    }

    fn entry(name: &str, passkeys: Vec<Passkey>) -> Entry {
        Entry {
            name: name.into(),
            root: crate::encode_public(&generate().verifying_key()),
            recovery: String::new(),
            devices: vec![],
            passkeys,
            grant: None,
            libraries: vec![],
            recovered_from: vec![],
            terms: None,
            removed_boxes: vec![],
            version: 1,
            updated: 0,
        }
    }

    /// A browser's sign-in: its own key, delegated to by the passkey.
    fn sign_in(pk: &Authenticator, user: &str, origin: &str) -> (Delegation, SigningKey) {
        let browser = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let key = B64_URL.encode(
            browser
                .verifying_key()
                .to_public_key_der()
                .unwrap()
                .as_bytes(),
        );
        let (sid, iat, exp) = (
            "aaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            NOW - 60,
            NOW - 60 + LIFE,
        );
        let assertion = pk.assert(
            &delegation_challenge(user, &key, &sid, iat, exp),
            origin,
            "commonty.org",
        );
        (
            Delegation {
                user: user.into(),
                passkey: pk.id.clone(),
                key,
                sid,
                iat,
                exp,
                assertion,
            },
            browser,
        )
    }

    fn hand(browser: &SigningKey, host: &str, nonce: &str) -> String {
        let s: p256::ecdsa::Signature = browser.sign(&handoff_message(host, nonce));
        B64_URL.encode(s.to_bytes())
    }

    #[test]
    fn a_sign_in_on_the_id_page_hands_off_to_the_box_it_names() {
        let pk = Authenticator::new("cred-1");
        let e = entry("david", vec![pk.on_file()]);
        let (d, browser) = sign_in(&pk, "david", ORIGIN);
        check_delegation(&e, &d, ORIGIN, NOW).expect("the passkey delegated");
        let sig = hand(&browser, "metrics.node2.commonty.org", "n1");
        check_handoff(&d, "metrics.node2.commonty.org", "n1", &sig).expect("for this box");
        // not for another box, nor another of its challenges
        assert!(check_handoff(&d, "files.commonty.org", "n1", &sig).is_err());
        assert!(check_handoff(&d, "metrics.node2.commonty.org", "n2", &sig).is_err());
    }

    #[test]
    fn what_a_box_must_refuse() {
        let pk = Authenticator::new("cred-1");
        let other = Authenticator::new("cred-2");
        let e = entry("david", vec![pk.on_file()]);
        let (d, browser) = sign_in(&pk, "david", ORIGIN);

        // made on a box's own page, not the id page: a box could ask for one
        let (on_a_box, _) = sign_in(&pk, "david", "https://home.commonty.org");
        assert!(check_delegation(&e, &on_a_box, ORIGIN, NOW).is_err());
        // another passkey, not in this entry
        let (stranger, _) = sign_in(&other, "david", ORIGIN);
        assert!(check_delegation(&e, &stranger, ORIGIN, NOW).is_err());
        // someone else's entry
        assert!(check_delegation(&entry("tom", vec![pk.on_file()]), &d, ORIGIN, NOW).is_err());
        // run out, or from the future
        assert!(check_delegation(&e, &d, ORIGIN, d.exp).is_err());
        assert!(check_delegation(&e, &d, ORIGIN, d.iat - SKEW - 1).is_err());
        // any field changed after the passkey signed
        for tamper in [
            |d: &mut Delegation| d.exp += 1,
            |d: &mut Delegation| d.sid = "bbbbbbbbbbbbbbbbbbbbbbbb".into(),
            |d: &mut Delegation| d.iat -= 1,
        ] {
            let mut t = d.clone();
            tamper(&mut t);
            assert!(check_delegation(&e, &t, ORIGIN, NOW).is_err());
        }
        // another key put in place of the browser's: its hand-offs do not count
        let thief = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let mut swapped = d.clone();
        swapped.key = B64_URL.encode(
            thief
                .verifying_key()
                .to_public_key_der()
                .unwrap()
                .as_bytes(),
        );
        assert!(check_delegation(&e, &swapped, ORIGIN, NOW).is_err());
        assert!(check_handoff(&d, "h", "n", &hand(&thief, "h", "n")).is_err());
        // longer than a sign-in lasts, though the passkey signed it so
        let mut long = d.clone();
        long.exp = long.iat + LIFE + SKEW + 1;
        long.assertion = pk.assert(
            &delegation_challenge(&long.user, &long.key, &long.sid, long.iat, long.exp),
            ORIGIN,
            "commonty.org",
        );
        assert!(check_delegation(&e, &long, ORIGIN, NOW).is_err());
        // a sign-in id that could be anything
        let mut odd = d.clone();
        odd.sid = "a/b".into();
        assert!(check_delegation(&e, &odd, ORIGIN, NOW).is_err());
        let _ = browser;
    }
}
