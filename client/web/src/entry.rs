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
}
