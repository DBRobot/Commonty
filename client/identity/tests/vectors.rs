//! Test vectors for the control plane's judge (servers/system/control/state/
//! ident): every signed item, in the old format (v1, signed over serde_json's
//! bytes) and the envelope (v2), made here by the code that signs them today,
//! so the Go side must reach the same bytes and the same verdicts.
//!
//! `IDENT_VECTORS_OUT=<file> cargo test -p identity --test vectors` writes
//! them; without it the test only checks they still come out the same as the
//! committed file.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use identity::envelope;
use identity::*;
use serde_json::json;

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

/// a P-256 passkey, serialised as webauthn-rs serialises one, and a way to
/// answer a challenge with it the way a browser would
struct Pk {
    sk: p256::ecdsa::SigningKey,
    id: String,
    cred: serde_json::Value,
    rp: String,
}

impl Pk {
    fn new(seed: u8, rp: &str) -> Pk {
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
        Pk {
            sk,
            id,
            cred,
            rp: rp.into(),
        }
    }

    fn passkey(&self) -> Passkey {
        Passkey {
            id: self.id.clone(),
            cred: self.cred.clone(),
            added: 1_700_000_000,
            rp_id: Some(self.rp.clone()),
            library_key: None,
        }
    }

    fn root(&self) -> String {
        passkey_root(&self.id, &self.cred)
    }

    /// base64 of the assertion json, as an entry's signature carries one
    fn assert(&self, challenge: &[u8]) -> String {
        use p256::ecdsa::signature::Signer as _;
        use sha2::Digest as _;
        let mut auth = sha2::Sha256::digest(self.rp.as_bytes()).to_vec();
        auth.push(0x05); // user present, user verified
        auth.extend_from_slice(&[0, 0, 0, 7]);
        let client = serde_json::to_vec(&json!({
            "type": "webauthn.get",
            "challenge": B64URL.encode(challenge),
            "origin": format!("https://home.{}", self.rp),
            "crossOrigin": false
        }))
        .unwrap();
        let mut data = auth.clone();
        data.extend_from_slice(&sha2::Sha256::digest(&client));
        let sig: p256::ecdsa::Signature = self.sk.sign(&data);
        let a = json!({
            "authenticatorData": B64URL.encode(&auth),
            "clientDataJSON": B64URL.encode(&client),
            "signature": B64URL.encode(sig.to_der().as_bytes()),
        });
        B64.encode(serde_json::to_vec(&a).unwrap())
    }
}

fn device(k: &SigningKey, added: u64) -> Device {
    let pk = encode_public(&k.verifying_key());
    Device {
        fingerprint: fingerprint(&pk),
        public_key: pk,
        added,
    }
}

/// an entry as the log keeps it: the gate submits a serde_json::Value, so
/// its keys come out sorted
fn logged(kind: &str, v: impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(json!({ kind: v })).unwrap()
}

fn all() -> serde_json::Value {
    let release = key(1);
    let ada_root = key(2);
    let ada_paper = key(3);
    let ada_phone = key(4);
    let ada_new_root = key(5);
    let code = code_key("abcde-fghjk");
    let pk = Pk::new(9, "example.org");

    // ada: a device root, a library with awkward strings, terms
    let ada1 = Entry {
        name: "ada".into(),
        root: encode_public(&ada_root.verifying_key()),
        recovery: encode_public(&ada_paper.verifying_key()),
        recovered_from: vec![],
        devices: vec![device(&ada_phone, 1_700_000_001)],
        passkeys: vec![],
        grant: None,
        libraries: vec![Library {
            id: "0123456789abcdef0123456789abcdef".into(),
            keys: vec![SealedKey {
                to: "device:abc".into(),
                // every escape serde_json makes, and what it leaves alone
                sealed: "q\"b\\s/<&>\u{1}\u{1f}\n\t\r\u{8}\u{c}é漢\u{2028}\u{7f}".into(),
            }],
            readers: vec![],
            created: 1_700_000_002,
        }],
        terms: Some("2026-10-11".into()),
        version: 1,
        updated: 1_791_700_000,
    };
    let ada1_signed = sign(ada1.clone(), &ada_root).unwrap();

    // ada recovers: a new root, co-signed by the paper key
    let mut ada2 = ada1.clone();
    ada2.root = encode_public(&ada_new_root.verifying_key());
    ada2.recovered_from = vec![ada1.root.clone()];
    ada2.version = 2;
    ada2.updated = 1_791_700_100;
    let ada2_signed = sign_recovery(ada2.clone(), &ada_new_root, &ada_paper).unwrap();

    // bea: a passkey root, let in by an invite
    let invite = sign_invite(
        Invite {
            public_key: encode_public(&code.verifying_key()),
            issued: 1_791_600_000,
            expires: 1_791_800_000,
        },
        &release,
    )
    .unwrap();
    let mut bea = Entry {
        name: "bea".into(),
        root: pk.root(),
        recovery: String::new(),
        recovered_from: vec![],
        devices: vec![],
        passkeys: vec![pk.passkey()],
        grant: None,
        libraries: vec![],
        terms: Some("2026-10-11".into()),
        version: 1,
        updated: 1_791_700_200,
    };
    bea.grant = Some(Grant {
        invite: invite.clone(),
        redeemed: 1_791_700_150,
        proof: prove(&code, &invite.invite.public_key, &bea.root, 1_791_700_150),
    });
    let bea_signed = SignedEntry {
        signature: pk.assert(&challenge(&bea).unwrap()),
        entry: bea.clone(),
        recovery_signature: None,
    };
    verify(&bea_signed).expect("the passkey vector verifies in Rust");
    verify_grant(&bea, &release.verifying_key()).expect("the grant verifies");

    let leave = sign_leave(
        Leave {
            name: "ada".into(),
            root: ada2.root.clone(),
            at: 1_791_700_300,
            back: false,
        },
        &ada_new_root,
    )
    .unwrap();

    // v2: the same things in envelopes
    let ada_v2 = envelope::seal_json(envelope::ENTRY, &ada1, &ada_root).unwrap();
    let mut ada2_v2 = envelope::seal_json(envelope::ENTRY, &ada2, &ada_new_root).unwrap();
    ada2_v2.recovery_sig = Some({
        use ed25519_dalek::Signer as _;
        B64.encode(ada_paper.sign(&ada2_v2.message().unwrap()).to_bytes())
    });
    let bea_body = serde_json::to_vec(&bea).unwrap();
    let bea_v2 = envelope::Envelope {
        v: 2,
        kind: envelope::ENTRY.into(),
        body: B64.encode(&bea_body),
        sig: pk.assert(&envelope::passkey_challenge(envelope::ENTRY, &bea_body)),
        recovery_sig: None,
    };
    let leave_v2 = envelope::seal_json(envelope::LEAVE, &leave.leave, &ada_new_root).unwrap();
    let invite_v2 = envelope::seal_json(envelope::INVITE, &invite.invite, &release).unwrap();
    let enlist_v2 = envelope::seal_json(
        envelope::ENLIST,
        &json!({"member": member_id(&ada2.root), "box": "j0123456789",
                "key": "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE", "ticket": "00112233445566778899aabbccddeeff",
                "expires": 1_792_000_000u64}),
        &ada_new_root,
    )
    .unwrap();

    json!({
        "release": encode_public(&release.verifying_key()),
        "v1": {
            "entries": [
                {"what": "ada, a device root", "canonical": B64.encode(canonical(&ada1).unwrap()),
                 "signed": ada1_signed, "logged": logged("entry", &ada1_signed),
                 "member_id": member_id(&ada1.root)},
                {"what": "ada recovered", "canonical": B64.encode(canonical(&ada2).unwrap()),
                 "signed": ada2_signed, "logged": logged("entry", &ada2_signed),
                 "previous": 0},
                {"what": "bea, a passkey root let in by an invite",
                 "canonical": B64.encode(canonical(&bea).unwrap()),
                 "challenge": B64.encode(challenge(&bea).unwrap()),
                 "signed": bea_signed, "logged": logged("entry", &bea_signed),
                 "root": bea.root, "cose_digest_root": passkey_root(&pk.id, &pk.cred)}
            ],
            "invite": {"signed": invite, "bytes": B64.encode(serde_json::to_vec(&invite.invite).unwrap()),
                       "logged": logged("invite", &invite)},
            "proof_bytes": B64.encode(proof_bytes(&bea.grant.as_ref().unwrap().invite.invite.public_key, &bea.root, 1_791_700_150)),
            "leave": {"signed": leave, "challenge": B64.encode(leave_challenge(&leave.leave).unwrap()),
                      "logged": logged("leave", &leave), "on_file": 1},
            "ack": {"logged": logged("ack", "deadbeef"),
                    "hash": item_hash(&logged("ack", "deadbeef"))},
            "item_hash": item_hash(&logged("entry", &ada1_signed)),
        },
        "v2": {
            "ada": {"envelope": ada_v2, "key": ada1.root},
            "ada_recovered": {"envelope": ada2_v2, "key": ada2.root, "recovery": ada1.recovery},
            "bea": {"envelope": bea_v2, "passkey": bea.passkeys[0], "root": bea.root},
            "leave": {"envelope": leave_v2, "key": ada2.root},
            "invite": {"envelope": invite_v2, "key": encode_public(&release.verifying_key())},
            "enlist": {"envelope": enlist_v2, "key": ada2.root},
        }
    })
}

/// the gate's item hash (servers/gate/src/directory.rs): sha256 of the
/// item's serde_json bytes, keys sorted
fn item_hash(v: &serde_json::Value) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(serde_json::to_vec(v).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn vectors() {
    let v = serde_json::to_string_pretty(&all()).unwrap() + "\n";
    let committed = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../servers/system/control/state/ident/testdata/vectors.json"
    );
    if let Ok(out) = std::env::var("IDENT_VECTORS_OUT") {
        std::fs::write(out, &v).unwrap();
        return;
    }
    if let Ok(had) = std::fs::read_to_string(committed) {
        assert_eq!(
            had, v,
            "the vectors changed: regenerate them (IDENT_VECTORS_OUT)"
        );
    }
}
