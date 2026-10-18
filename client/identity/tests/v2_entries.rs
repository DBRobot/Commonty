//! An entry written as a v2 envelope (docs/dev/identity-state.md) reads into
//! the same `SignedEntry` as a v1 one, and every reader's `verify` and
//! `accept` check it over the bytes that were signed.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use identity::envelope;
use identity::*;
use serde_json::json;

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn entry(name: &str, root: &SigningKey, version: u64) -> Entry {
    let device = encode_public(&key(40 + version as u8).verifying_key());
    Entry {
        name: name.into(),
        root: encode_public(&root.verifying_key()),
        recovery: String::new(),
        recovered_from: vec![],
        devices: vec![Device {
            fingerprint: fingerprint(&device),
            public_key: device,
            added: 1,
        }],
        passkeys: vec![],
        libraries: vec![],
        grant: None,
        terms: None,
        removed_boxes: vec![],
        version,
        updated: version,
    }
}

/// `e` sealed as a v2 envelope by `root`, and read as a reader reads it:
/// off the wire, as JSON
fn v2(e: &Entry, root: &SigningKey) -> SignedEntry {
    let env = envelope::seal_json(envelope::ENTRY, e, root).unwrap();
    serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap()
}

#[test]
fn a_v2_entry_verifies_over_its_bytes_and_nothing_else() {
    let root = key(2);
    let s = v2(&entry("ada", &root, 1), &root);
    verify(&s).unwrap();
    accept(None, &s).unwrap();
    assert!(s.envelope().is_some());

    // written back out it is the envelope it came as, byte for byte
    let wire = serde_json::to_value(&s).unwrap();
    assert_eq!(wire["v"], 2);
    assert_eq!(wire["kind"], "entry");
    let again: SignedEntry = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&again).unwrap(), wire);

    // another body under the same signature
    let mut env = s.envelope().unwrap();
    env.body = B64.encode(serde_json::to_vec(&entry("bob", &root, 1)).unwrap());
    assert!(verify(&SignedEntry::from_envelope(&env).unwrap()).is_err());

    // the same body and signature as another kind
    let mut env = s.envelope().unwrap();
    env.kind = envelope::LEAVE.into();
    assert!(SignedEntry::from_envelope(&env).is_err());

    // signed by a key that is not the root the body names
    let other = v2(&entry("ada", &root, 1), &key(9));
    assert!(verify(&other).is_err());
}

#[test]
fn v1_still_reads_and_cannot_pass_for_v2() {
    let root = key(2);
    let s = sign(entry("ada", &root, 1), &root).unwrap();
    let wire = serde_json::to_value(&s).unwrap();
    assert!(wire.get("v").is_none());
    let back: SignedEntry = serde_json::from_value(wire.clone()).unwrap();
    verify(&back).unwrap();
    assert!(back.envelope().is_none());

    let mut forged = wire;
    forged["signature"] = json!("v2:e30=:AAAA");
    assert!(serde_json::from_value::<SignedEntry>(forged).is_err());
}

#[test]
fn an_update_may_move_from_v1_to_v2_under_the_same_rules() {
    let root = key(2);
    let old = sign(entry("ada", &root, 1), &root).unwrap();
    let new = v2(&entry("ada", &root, 2), &root);
    accept(Some(&old), &new).unwrap();
    accept(Some(&new), &v2(&entry("ada", &root, 3), &root)).unwrap();

    // not newer
    assert!(accept(Some(&new), &v2(&entry("ada", &root, 2), &root)).is_err());
    // another root without the recovery key
    let thief = key(7);
    let mut stolen = entry("ada", &thief, 3);
    stolen.recovered_from = vec![old.entry.root.clone()];
    assert!(accept(Some(&new), &v2(&stolen, &thief)).is_err());
}

#[test]
fn a_v2_recovery_is_cosigned_over_the_same_message() {
    let root = key(2);
    let paper = key(3);
    let new_root = key(5);
    let mut first = entry("ada", &root, 1);
    first.recovery = encode_public(&paper.verifying_key());
    let old = v2(&first, &root);

    let mut next = entry("ada", &new_root, 2);
    next.recovery = first.recovery.clone();
    next.recovered_from = vec![first.root.clone()];
    let mut env = envelope::seal_json(envelope::ENTRY, &next, &new_root).unwrap();
    let cosign = |k: &SigningKey, env: &envelope::Envelope| {
        use ed25519_dalek::Signer as _;
        B64.encode(k.sign(&env.message().unwrap()).to_bytes())
    };
    env.recovery_sig = Some(cosign(&paper, &env));
    accept(Some(&old), &SignedEntry::from_envelope(&env).unwrap()).unwrap();

    env.recovery_sig = Some(cosign(&key(8), &env));
    assert!(accept(Some(&old), &SignedEntry::from_envelope(&env).unwrap()).is_err());
}

/// a P-256 passkey as webauthn-rs serialises one, answering as a browser
struct Pk {
    sk: p256::ecdsa::SigningKey,
    id: String,
    cred: serde_json::Value,
}

impl Pk {
    fn new(seed: u8) -> Pk {
        let sk = p256::ecdsa::SigningKey::from_bytes(&[seed; 32].into()).unwrap();
        let point = sk.verifying_key().to_encoded_point(false);
        let id = B64URL.encode([seed; 16]);
        let cred = json!({"cred": {
            "attestation": {"data": "None", "metadata": "None"},
            "attestation_format": "none",
            "backup_eligible": true,
            "backup_state": true,
            "counter": 0,
            "cred": {"key": {"EC_EC2": {"curve": "SECP256R1",
                "x": B64URL.encode(point.x().unwrap()),
                "y": B64URL.encode(point.y().unwrap())}}, "type_": "ES256"},
            "cred_id": id,
            "extensions": {"appid": "NotRequested", "cred_props": {"Unsigned": {"rk": true}},
                "cred_protect": "Ignored", "hmac_create_secret": "NotRequested"},
            "registration_policy": "required",
            "transports": null,
            "user_verified": true
        }});
        Pk { sk, id, cred }
    }

    fn passkey(&self) -> Passkey {
        Passkey {
            id: self.id.clone(),
            cred: self.cred.clone(),
            added: 1,
            rp_id: Some("example.org".into()),
            library_key: None,
        }
    }

    fn answer(&self, challenge: &[u8]) -> String {
        use p256::ecdsa::signature::Signer as _;
        use sha2::Digest as _;
        let mut auth = sha2::Sha256::digest(b"example.org").to_vec();
        auth.push(0x05);
        auth.extend_from_slice(&[0, 0, 0, 7]);
        let client = serde_json::to_vec(&json!({
            "type": "webauthn.get",
            "challenge": B64URL.encode(challenge),
            "origin": "https://home.example.org",
            "crossOrigin": false
        }))
        .unwrap();
        let mut data = auth.clone();
        data.extend_from_slice(&sha2::Sha256::digest(&client));
        let sig: p256::ecdsa::Signature = self.sk.sign(&data);
        B64.encode(
            serde_json::to_vec(&json!({
                "authenticatorData": B64URL.encode(&auth),
                "clientDataJSON": B64URL.encode(&client),
                "signature": B64URL.encode(sig.to_der().as_bytes()),
            }))
            .unwrap(),
        )
    }

    /// `e` as an envelope its passkey signed: the challenge is sha256 of the
    /// envelope's message
    fn seal(&self, e: &Entry) -> SignedEntry {
        let body = serde_json::to_vec(e).unwrap();
        let env = envelope::Envelope {
            v: 2,
            kind: envelope::ENTRY.into(),
            body: B64.encode(&body),
            sig: self.answer(&envelope::passkey_challenge(envelope::ENTRY, &body)),
            recovery_sig: None,
        };
        SignedEntry::from_envelope(&env).unwrap()
    }
}

fn passkey_entry(pk: &Pk, version: u64) -> Entry {
    Entry {
        name: "eve".into(),
        root: passkey_root(&pk.id, &pk.cred),
        recovery: String::new(),
        recovered_from: vec![],
        devices: vec![],
        passkeys: vec![pk.passkey()],
        libraries: vec![],
        grant: None,
        terms: None,
        removed_boxes: vec![],
        version,
        updated: version,
    }
}

#[test]
fn a_passkey_root_signs_a_v2_entry_with_an_assertion_over_its_message() {
    let pk = Pk::new(9);
    let first = pk.seal(&passkey_entry(&pk, 1));
    verify(&first).unwrap();
    accept(None, &first).unwrap();
    accept(Some(&first), &pk.seal(&passkey_entry(&pk, 2))).unwrap();

    // an assertion made for another body does not carry over
    let mut env = first.envelope().unwrap();
    let mut other = passkey_entry(&pk, 1);
    other.name = "mallory".into();
    env.body = B64.encode(serde_json::to_vec(&other).unwrap());
    assert!(verify(&SignedEntry::from_envelope(&env).unwrap()).is_err());

    // nor does a v1 assertion (the entry's labelled hash) for a v2 body
    let mut env = first.envelope().unwrap();
    env.sig = pk.answer(&challenge(&passkey_entry(&pk, 1)).unwrap());
    assert!(verify(&SignedEntry::from_envelope(&env).unwrap()).is_err());
}
