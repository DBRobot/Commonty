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

fn change(
    signed: &str,
    device: Option<&str>,
    passkey: Option<&str>,
    now: u64,
) -> R<identity::Entry> {
    let s: SignedEntry = serde_json::from_str(signed).map_err(|e| e.to_string())?;
    if !s.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        return Err("this entry's root is a device key: change it with dd".into());
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
    e.version += 1;
    e.updated = now;
    Ok(e)
}

/// The entry with a new device in it - the key a QR code's device offered -
/// and every library key sealed to it as well, and the challenge its root
/// passkey has to sign: `{ "entry": …, "challenge": base64url }`. The
/// library is open in this page (its key, base64, from library.js).
#[wasm_bindgen]
pub fn entry_with_device(
    signed: &str,
    public_key: &str,
    library_id: Option<String>,
    library_key: Option<String>,
    now: u64,
) -> Result<String, JsValue> {
    let run = || -> R<String> {
        let s: SignedEntry = serde_json::from_str(signed).map_err(|e| e.to_string())?;
        if !s.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
            return Err("this entry's root is a device key: it adds devices itself".into());
        }
        identity::decode_public(public_key).map_err(|e| e.to_string())?;
        let mut e = s.entry;
        if e.devices.iter().any(|d| d.public_key == public_key) {
            return Err("that device is in the account already".into());
        }
        let fp = identity::fingerprint(public_key);
        e.devices.push(identity::Device {
            fingerprint: fp.clone(),
            public_key: public_key.to_string(),
            added: now,
        });
        if let (Some(id), Some(key)) = (library_id, library_key) {
            let raw = B64.decode(&key).map_err(|e| e.to_string())?;
            let sealed = library::seal_to(public_key, &raw).map_err(|e| e.to_string())?;
            if let Some(l) = e.libraries.iter_mut().find(|l| l.id == id) {
                l.keys.push(identity::SealedKey {
                    to: format!("device:{fp}"),
                    sealed,
                });
            }
        }
        e.version += 1;
        e.updated = now;
        let c = identity::challenge(&e).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "entry": e, "challenge": B64_URL.encode(c) }).to_string())
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
        let e = change(signed, device.as_deref(), passkey.as_deref(), now)?;
        let c = identity::challenge(&e).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "entry": e, "challenge": B64_URL.encode(c) }).to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_root_is_left_to_dd_and_the_root_passkey_stays() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let public = identity::encode_public(&key.verifying_key());
        let e = identity::Entry {
            name: "tom".into(),
            root: public.clone(),
            recovery: String::new(),
            devices: vec![identity::Device {
                fingerprint: "f1".into(),
                public_key: public,
                added: 1,
            }],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            version: 1,
            updated: 1,
        };
        let signed = serde_json::to_string(&identity::sign(e.clone(), &key).unwrap()).unwrap();
        assert!(
            change(&signed, Some("f1"), None, 2)
                .unwrap_err()
                .contains("dd")
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
    fn a_device_joins_with_the_library_sealed_to_it() {
        use base64::Engine as _;
        let newcomer = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
        let pk = identity::encode_public(&newcomer.verifying_key());
        let e = identity::Entry {
            name: "tom".into(),
            root: format!("{}pk1:digest", identity::WEBAUTHN_ROOT),
            recovery: String::new(),
            devices: vec![],
            passkeys: vec![],
            grant: None,
            libraries: vec![identity::Library {
                id: "0123456789abcdef0123456789abcdef".into(),
                keys: vec![],
                readers: vec![],
                created: 1,
            }],
            version: 4,
            updated: 4,
        };
        let signed = serde_json::to_string(&SignedEntry {
            entry: e,
            signature: String::new(),
            recovery_signature: None,
        })
        .unwrap();
        let key = B64.encode([9u8; 32]);
        let out: serde_json::Value = serde_json::from_str(
            &entry_with_device(
                &signed,
                &pk,
                Some("0123456789abcdef0123456789abcdef".into()),
                Some(key),
                50,
            )
            .unwrap(),
        )
        .unwrap();
        let e: identity::Entry = serde_json::from_value(out["entry"].clone()).unwrap();
        assert_eq!((e.version, e.devices.len()), (5, 1));
        let sealed = &e.libraries[0].keys[0];
        assert_eq!(sealed.to, format!("device:{}", identity::fingerprint(&pk)));
        // and only the new device opens it, to the same key
        let opened = library::open_with(&newcomer, &sealed.sealed).unwrap();
        assert_eq!(&opened[..], &[9u8; 32]);
        assert!(!out["challenge"].as_str().unwrap().is_empty());
    }
}
