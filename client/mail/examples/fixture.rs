// The made-up member the mail Worker's end-to-end check signs in as
// (nix/tests/mail-worker): a device root, one passkey on commonty.test, at
// two versions. Its keys are test keys and nobody's. Regenerate with
//   cargo run -q -p dd-mail --example fixture > client/mail/tests/tester.json
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use identity::{Device, Entry, Passkey, SignedEntry};
use p256::ecdsa::signature::Signer as _;
use p256::pkcs8::EncodePrivateKey as _;
use sha2::Digest as _;
use webauthn_rs_core::proto::{COSEAlgorithm, COSEEC2Key, COSEKey, COSEKeyType, ECDSACurve};
// a made-up member: a device root, and one passkey on commonty.org
struct Tester {
    root: ed25519_dalek::SigningKey,
    passkey: p256::ecdsa::SigningKey,
}

impl Tester {
    fn new() -> Tester {
        Tester {
            root: ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
            passkey: p256::ecdsa::SigningKey::from_bytes(&[9; 32].into()).unwrap(),
        }
    }

    fn entry(&self, version: u64) -> SignedEntry {
        let p = self.passkey.verifying_key().to_encoded_point(false);
        let cose = COSEKey {
            type_: COSEAlgorithm::ES256,
            key: COSEKeyType::EC_EC2(COSEEC2Key {
                curve: ECDSACurve::SECP256R1,
                x: p.x().unwrap().to_vec(),
                y: p.y().unwrap().to_vec(),
            }),
        };
        let public = identity::encode_public(&self.root.verifying_key());
        let entry = Entry {
            name: "tester".into(),
            root: public.clone(),
            recovery: String::new(),
            devices: vec![Device {
                fingerprint: "test".into(),
                public_key: public,
                added: 1,
            }],
            passkeys: vec![Passkey {
                id: "cGsx".into(),
                cred: serde_json::json!({ "cred": { "cred": cose } }),
                added: 1,
                rp_id: Some("commonty.test".into()),
                library_key: None,
            }],
            grant: None,
            libraries: vec![],
            version,
            updated: version,
        };
        identity::sign(entry, &self.root).unwrap()
    }

    fn members(&self) -> String {
        let id = identity::member_id(&identity::encode_public(&self.root.verifying_key()));
        serde_json::json!({ "members": [id], "revoked": [] }).to_string()
    }

    #[allow(dead_code)]
    fn answer(&self, challenge: &[u8], rp: &str, flags: u8, origin: &str) -> String {
        let mut auth = sha2::Sha256::digest(rp.as_bytes()).to_vec();
        auth.push(flags);
        auth.extend_from_slice(&[0, 0, 0, 1]);
        let client = serde_json::json!({
        "type": "webauthn.get",
        "challenge": B64_URL.encode(challenge),
        "origin": origin,
        })
        .to_string();
        let mut data = auth.clone();
        data.extend_from_slice(&sha2::Sha256::digest(client.as_bytes()));
        let sig: p256::ecdsa::Signature = self.passkey.sign(&data);
        serde_json::json!({
        "authenticatorData": B64_URL.encode(&auth),
        "clientDataJSON": B64_URL.encode(client.as_bytes()),
        "signature": B64_URL.encode(sig.to_der().as_bytes()),
        })
        .to_string()
    }
}

fn main() {
    let t = Tester::new();
    let key = t.passkey.to_pkcs8_der().unwrap();
    println!(
        "{}",
        serde_json::json!({
            "v3": t.entry(3),
            "v4": t.entry(4),
            "members": t.members(),
            "pkcs8": base64::engine::general_purpose::STANDARD.encode(key.as_bytes()),
        })
    );
}
