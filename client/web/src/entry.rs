//! A member whose root is their passkey changes their entry here: the
//! Devices tab removes a device or a passkey, the passkey signs the new
//! entry's challenge, and the boxes check it with identity::accept as they
//! check any update. A member whose root is a device key does this with
//! `dd` instead; the browser holds no such key.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64_URL};
use identity::{Assertion, SignedEntry};
use wasm_bindgen::prelude::*;

type R<T> = Result<T, String>;

/// The entry the box handed the page, held to its own signature: the passkey
/// is about to sign the next version of it, and must not sign over a device
/// or a sealed key a box slipped into "the current one".
fn verified(signed: &str) -> R<()> {
    let s: SignedEntry = serde_json::from_str(signed).map_err(|e| e.to_string())?;
    identity::verify(&s)
        .map_err(|_| "this account's entry is not signed by its own key".to_string())
}

fn change(
    signed: &str,
    device: Option<&str>,
    passkey: Option<&str>,
    now: u64,
) -> R<identity::Entry> {
    let s: SignedEntry = serde_json::from_str(signed).map_err(|e| e.to_string())?;
    if !s.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        return Err("this account's main key is a device's: it changes the account itself".into());
    }
    let mut e = s.entry;
    if let Some(fp) = device {
        let before = e.devices.len();
        e.devices.retain(|d| d.fingerprint != fp);
        if e.devices.len() == before {
            return Err("no such device in the entry".into());
        }
        let to = format!("device:{fp}");
        for l in &mut e.libraries {
            l.keys.retain(|k| k.to != to);
        }
    }
    if let Some(id) = passkey {
        if e.root
            .starts_with(&format!("{}{id}:", identity::WEBAUTHN_ROOT))
        {
            return Err("that passkey is the account's root; it stays".into());
        }
        let before = e.passkeys.len();
        e.passkeys.retain(|p| p.id != id);
        if e.passkeys.len() == before {
            return Err("no such passkey in the entry".into());
        }
    }
    identity::drop_passkey_keys(&mut e);
    e.version += 1;
    e.updated = now;
    Ok(e)
}

/// The entry with a new device in it - the key a QR code's device offered -
/// and the challenge its root passkey has to sign: `{ "entry": …,
/// "challenge": base64url }`. No key is sealed here: a box page holds none
/// to seal, and a passkey's account keeps none in its entry.
fn with_device(signed: &str, public_key: &str, now: u64) -> R<String> {
    let s: SignedEntry = serde_json::from_str(signed).map_err(|e| e.to_string())?;
    if !s.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        return Err("this entry's root is a device key: it adds devices itself".into());
    }
    identity::decode_public(public_key).map_err(|e| e.to_string())?;
    let mut e = s.entry;
    if e.devices.iter().any(|d| d.public_key == public_key) {
        return Err("that device is in the account already".into());
    }
    e.devices.push(identity::Device {
        fingerprint: identity::fingerprint(public_key),
        public_key: public_key.to_string(),
        added: now,
    });
    identity::drop_passkey_keys(&mut e);
    e.version += 1;
    e.updated = now;
    plan(&e)
}

/// What a passkey signs for `e`, both ways: `challenge` for today's entry
/// (v1), and `body` with `challenge_v2` for a v2 envelope - the body's bytes
/// exactly as they will be signed, and sha256 of the envelope's message
/// (docs/dev/identity-state.md). The page asks the passkey for the one its
/// box takes (`/_dd/config`'s `envelope`), never both.
fn plan(e: &identity::Entry) -> R<String> {
    let c = identity::challenge(e).map_err(|e| e.to_string())?;
    let body = serde_json::to_vec(e).map_err(|e| e.to_string())?;
    let c2 = identity::envelope::passkey_challenge(identity::envelope::ENTRY, &body);
    Ok(serde_json::json!({
        "entry": e,
        "challenge": B64_URL.encode(c),
        "body": B64.encode(&body),
        "challenge_v2": B64_URL.encode(c2),
    })
    .to_string())
}

#[wasm_bindgen]
pub fn entry_with_device(signed: &str, public_key: &str, now: u64) -> Result<String, JsValue> {
    let run = || -> R<String> {
        verified(signed)?;
        with_device(signed, public_key, now)
    };
    run().map_err(|e| JsValue::from_str(&e))
}

/// A QR code for `text`, as an SVG the page puts in place.
#[wasm_bindgen]
pub fn qr_svg(text: &str) -> Result<String, JsValue> {
    let code =
        qrcode::QrCode::new(text.as_bytes()).map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(220, 220)
        .quiet_zone(true)
        .build())
}

/// The entry without that device or passkey, and the challenge its root
/// passkey has to sign: `{ "entry": …, "challenge": base64url }`.
#[wasm_bindgen]
pub fn entry_without(
    signed: &str,
    device: Option<String>,
    passkey: Option<String>,
    now: u64,
) -> Result<String, JsValue> {
    let run = || -> R<String> {
        verified(signed)?;
        let e = change(signed, device.as_deref(), passkey.as_deref(), now)?;
        plan(&e)
    };
    run().map_err(|e| JsValue::from_str(&e))
}

/// The entry with a box this person set up removed - the one that came
/// with join ticket `ticket` (docs/dev/joining.md) - and the challenge its
/// root passkey has to sign: `{ "entry": …, "challenge": base64url }`.
fn without_box(signed: &str, ticket: &str, now: u64) -> R<String> {
    let s: SignedEntry = serde_json::from_str(signed).map_err(|e| e.to_string())?;
    if !s.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        return Err("this entry's root is a device key: the app removes the box".into());
    }
    if ticket.len() != 32 || !ticket.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("not a join ticket's id".into());
    }
    let mut e = s.entry;
    if !e.removed_boxes.iter().any(|t| t == ticket) {
        e.removed_boxes.push(ticket.to_string());
    }
    identity::drop_passkey_keys(&mut e);
    e.version += 1;
    e.updated = now;
    let c = identity::challenge(&e).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "entry": e, "challenge": B64_URL.encode(c) }).to_string())
}

#[wasm_bindgen]
pub fn entry_without_box(signed: &str, ticket: &str, now: u64) -> Result<String, JsValue> {
    let run = || -> R<String> {
        verified(signed)?;
        without_box(signed, ticket, now)
    };
    run().map_err(|e| JsValue::from_str(&e))
}

/// The entry, signed with the passkey's answer (base64url parts, as the
/// browser hands them over): what the directory takes.
#[wasm_bindgen]
pub fn entry_signed(entry: &str, assertion: &str) -> Result<String, JsValue> {
    let run = || -> R<String> {
        let entry: identity::Entry = serde_json::from_str(entry).map_err(|e| e.to_string())?;
        let a: Assertion = serde_json::from_str(assertion).map_err(|e| e.to_string())?;
        let signature = B64.encode(serde_json::to_vec(&a).map_err(|e| e.to_string())?);
        let s = SignedEntry {
            entry,
            signature,
            recovery_signature: None,
        };
        serde_json::to_string(&s).map_err(|e| e.to_string())
    };
    run().map_err(|e| JsValue::from_str(&e))
}

/// The entry as it is, signed again with its root passkey for the move to
/// v2 envelopes: the same in every field but version and time. The plan the
/// page has the passkey sign (`body`, `challenge_v2`).
#[wasm_bindgen]
pub fn entry_resign(signed: &str, now: u64) -> Result<String, JsValue> {
    let run = || -> R<String> {
        verified(signed)?;
        let s: SignedEntry = serde_json::from_str(signed).map_err(|e| e.to_string())?;
        if !s.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
            return Err("this account's main key is a device's: its device signs it again".into());
        }
        let mut e = s.entry;
        identity::drop_passkey_keys(&mut e);
        e.version += 1;
        e.updated = now;
        plan(&e)
    };
    run().map_err(|e| JsValue::from_str(&e))
}

/// A v2 envelope: the plan's `body`, exactly as signed, and the passkey's
/// answer to its `challenge_v2` (base64url parts, as the browser hands
/// them over) - what a box that takes v2 takes.
#[wasm_bindgen]
pub fn entry_envelope(body: &str, assertion: &str) -> Result<String, JsValue> {
    let run = || -> R<String> {
        let a: Assertion = serde_json::from_str(assertion).map_err(|e| e.to_string())?;
        // the body is ours: it parses as an entry, or nothing is signed
        let raw = B64
            .decode(body)
            .map_err(|_| "the body is not base64".to_string())?;
        serde_json::from_slice::<identity::Entry>(&raw).map_err(|e| e.to_string())?;
        let env = identity::envelope::Envelope {
            v: 2,
            kind: identity::envelope::ENTRY.into(),
            body: body.to_string(),
            sig: B64.encode(serde_json::to_vec(&a).map_err(|e| e.to_string())?),
            recovery_sig: None,
        };
        serde_json::to_string(&env).map_err(|e| e.to_string())
    };
    run().map_err(|e| JsValue::from_str(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan's v2 body is the entry's bytes as the envelope carries them,
    /// and its challenge is sha256 of the envelope's message: what the judge
    /// checks a passkey's answer against
    #[test]
    fn a_v2_plan_signs_the_body_the_envelope_carries() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
        let e = identity::Entry {
            name: "pia".into(),
            root: "webauthn:abc:def".into(),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            terms: None,
            version: 3,
            updated: 9,
        };
        let _ = key;
        let p: serde_json::Value = serde_json::from_str(&plan(&e).unwrap()).unwrap();
        let body = B64.decode(p["body"].as_str().unwrap()).unwrap();
        assert_eq!(serde_json::from_slice::<identity::Entry>(&body).unwrap(), e);
        let want = identity::envelope::passkey_challenge(identity::envelope::ENTRY, &body);
        assert_eq!(
            B64_URL.decode(p["challenge_v2"].as_str().unwrap()).unwrap(),
            want
        );
        // and v1's challenge is still today's
        assert_eq!(
            B64_URL.decode(p["challenge"].as_str().unwrap()).unwrap(),
            identity::challenge(&e).unwrap()
        );
    }

    #[test]
    fn a_device_root_is_left_to_dd_and_the_root_passkey_stays() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let public = identity::encode_public(&key.verifying_key());
        let e = identity::Entry {
            name: "tom".into(),
            root: public.clone(),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![identity::Device {
                fingerprint: "f1".into(),
                public_key: public,
                added: 1,
            }],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            terms: None,
            removed_boxes: vec![],
            version: 1,
            updated: 1,
        };
        let signed = serde_json::to_string(&identity::sign(e.clone(), &key).unwrap()).unwrap();
        assert!(
            change(&signed, Some("f1"), None, 2)
                .unwrap_err()
                .contains("main key")
        );

        let mut p = e;
        p.root = format!("{}pk1:digest", identity::WEBAUTHN_ROOT);
        p.devices.push(identity::Device {
            fingerprint: "f2".into(),
            public_key: String::new(),
            added: 1,
        });
        let signed = serde_json::to_string(&SignedEntry {
            entry: p,
            signature: String::new(),
            recovery_signature: None,
        })
        .unwrap();
        let out = change(&signed, Some("f2"), None, 9).unwrap();
        assert_eq!(out.devices.len(), 1);
        assert_eq!((out.version, out.updated), (2, 9));
        assert!(
            change(&signed, None, Some("pk1"), 9)
                .unwrap_err()
                .contains("root")
        );
    }

    #[test]
    fn an_entry_the_box_made_up_is_not_signed_over() {
        let e = identity::Entry {
            name: "tom".into(),
            root: format!("{}pk1:digest", identity::WEBAUTHN_ROOT),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            terms: None,
            removed_boxes: vec![],
            version: 4,
            updated: 4,
        };
        let forged = serde_json::to_string(&SignedEntry {
            entry: e,
            signature: "c2lnbmVk".into(),
            recovery_signature: None,
        })
        .unwrap();
        assert!(verified(&forged).is_err());
    }

    #[test]
    fn a_device_joins_and_no_passkey_key_rides_along() {
        let newcomer = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
        let pk = identity::encode_public(&newcomer.verifying_key());
        let e = identity::Entry {
            name: "tom".into(),
            root: format!("{}pk1:digest", identity::WEBAUTHN_ROOT),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![],
            passkeys: vec![identity::Passkey {
                id: "pk1".into(),
                cred: serde_json::json!({}),
                added: 1,
                rp_id: None,
                library_key: Some("an old PRF key".into()),
            }],
            grant: None,
            libraries: vec![identity::Library {
                id: "0123456789abcdef0123456789abcdef".into(),
                keys: vec![identity::SealedKey {
                    to: "passkey:pk1".into(),
                    sealed: "x".into(),
                }],
                readers: vec![],
                created: 1,
            }],
            terms: None,
            removed_boxes: vec![],
            version: 4,
            updated: 4,
        };
        let signed = serde_json::to_string(&SignedEntry {
            entry: e,
            signature: String::new(),
            recovery_signature: None,
        })
        .unwrap();
        let out: serde_json::Value =
            serde_json::from_str(&with_device(&signed, &pk, 50).unwrap()).unwrap();
        let e: identity::Entry = serde_json::from_value(out["entry"].clone()).unwrap();
        assert_eq!((e.version, e.devices.len()), (5, 1));
        assert!(!identity::holds_passkey_keys(&e), "{e:?}");
        assert!(!out["challenge"].as_str().unwrap().is_empty());
    }
}
