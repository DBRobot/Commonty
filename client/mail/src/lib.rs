//! What the mail Worker (client/mail/worker) asks before it touches a
//! member's address: is this entry the member's own, is the member on the
//! list the release was signed with, and did their passkey just answer.
//! The list and the release key come from fleet/ at build time (the
//! Worker's fleet.js), so the Worker takes neither from a box.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use identity::{Assertion, SignedEntry};
use wasm_bindgen::prelude::*;

type R<T> = std::result::Result<T, String>;

fn parse(json: &str) -> R<SignedEntry> {
    serde_json::from_str(json).map_err(|e| format!("not an entry: {e}"))
}

/// The entry to go by: `fresh` from the directory, held to `pinned` - the
/// newest this Worker has accepted for the name - by the boxes' own rule.
/// A box can withhold or replay an entry; it cannot get an older one past
/// a newer, nor a new root past the recovery key.
pub fn admit(
    name: &str,
    fresh: &str,
    pinned: Option<&str>,
    members: &str,
    release: &str,
) -> R<String> {
    let fresh = parse(fresh)?;
    if fresh.entry.name != name {
        return Err("that entry is someone else's".into());
    }
    let take = match pinned.map(parse).transpose()? {
        Some(old) if fresh.entry.version < old.entry.version => old,
        Some(old) if fresh.entry.version == old.entry.version => {
            if fresh.entry != old.entry {
                return Err("two different entries at one version".into());
            }
            old
        }
        Some(old) => {
            identity::accept(Some(&old), &fresh).map_err(|e| e.to_string())?;
            fresh
        }
        None => {
            identity::accept(None, &fresh).map_err(|e| e.to_string())?;
            fresh
        }
    };
    if !member(&take, members, release)? {
        return Err("not a member".into());
    }
    serde_json::to_string(&take).map_err(|e| e.to_string())
}

fn member(e: &SignedEntry, members: &str, release: &str) -> R<bool> {
    let list: serde_json::Value = serde_json::from_str(members).map_err(|e| e.to_string())?;
    let has = |k: &str, id: &str| {
        list[k]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(id)))
    };
    let id = identity::member_id(&e.entry.root);
    if has("revoked", &id) {
        return Ok(false);
    }
    if has("members", &id) {
        return Ok(true);
    }
    let release = identity::decode_public(release).map_err(|e| e.to_string())?;
    Ok(e.entry.grant.is_some() && identity::verify_grant(&e.entry, &release).is_ok())
}

/// The member's passkey `id`, in `entry`, answering `challenge` (base64url),
/// on the Worker's own page `origin` and nowhere else.
pub fn login(entry: &str, id: &str, assertion: &str, challenge: &str, origin: &str) -> R<()> {
    let e = parse(entry)?;
    let a: Assertion =
        serde_json::from_str(assertion).map_err(|_| "not an assertion".to_string())?;
    let c = B64_URL
        .decode(challenge)
        .map_err(|_| "not a challenge".to_string())?;
    identity::check_login(&e.entry, id, &a, &c, &[origin.to_string()]).map_err(|e| e.to_string())
}

#[wasm_bindgen(js_name = admit)]
pub fn admit_js(
    name: &str,
    fresh: &str,
    pinned: Option<String>,
    members: &str,
    release: &str,
) -> Result<String, JsValue> {
    admit(name, fresh, pinned.as_deref(), members, release).map_err(|e| JsValue::from_str(&e))
}

#[wasm_bindgen(js_name = login)]
pub fn login_js(
    entry: &str,
    id: &str,
    assertion: &str,
    challenge: &str,
    origin: &str,
) -> Result<(), JsValue> {
    login(entry, id, assertion, challenge, origin).map_err(|e| JsValue::from_str(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use identity::{Device, Entry, Passkey};
    use p256::ecdsa::signature::Signer as _;
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
                    id: "pk1".into(),
                    cred: serde_json::json!({ "cred": { "cred": cose } }),
                    added: 1,
                    rp_id: Some("commonty.org".into()),
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

    const RELEASE: &str = "fdpYPlUGQCA+lolOtDKO1cgc4bM1syI3JX/0hFPthvo=";
    const NOBODY: &str = r#"{"members":[],"revoked":[]}"#;

    #[test]
    fn a_member_is_admitted_and_nobody_else() {
        let t = Tester::new();
        let e = serde_json::to_string(&t.entry(3)).unwrap();
        assert!(admit("tester", &e, None, &t.members(), RELEASE).is_ok());
        assert!(
            admit("tester", &e, None, NOBODY, RELEASE)
                .unwrap_err()
                .contains("not a member")
        );
        let revoked = t.members().replace(
            "\"revoked\":[]",
            &format!(
                "\"revoked\":[\"{}\"]",
                identity::member_id(&t.entry(3).entry.root)
            ),
        );
        assert!(admit("tester", &e, None, &revoked, RELEASE).is_err());
        assert!(admit("someone", &e, None, &t.members(), RELEASE).is_err());
    }

    #[test]
    fn a_box_cannot_replay_or_rewrite_an_entry() {
        let t = Tester::new();
        let old = serde_json::to_string(&t.entry(3)).unwrap();
        let new = serde_json::to_string(&t.entry(4)).unwrap();
        // newer is taken; older after newer is answered with the newer
        let pinned = admit("tester", &new, Some(&old), &t.members(), RELEASE).unwrap();
        assert_eq!(parse(&pinned).unwrap().entry.version, 4);
        let back = admit("tester", &old, Some(&pinned), &t.members(), RELEASE).unwrap();
        assert_eq!(parse(&back).unwrap().entry.version, 4);
        // an edited entry no longer carries its root's signature
        let mut forged = t.entry(5);
        forged.entry.passkeys.clear();
        let forged = serde_json::to_string(&forged).unwrap();
        assert!(admit("tester", &forged, Some(&pinned), &t.members(), RELEASE).is_err());
        assert!(admit("tester", &forged, None, &t.members(), RELEASE).is_err());
    }

    #[test]
    fn only_the_passkey_answering_this_challenge_here_signs_in() {
        let t = Tester::new();
        let e = serde_json::to_string(&t.entry(3)).unwrap();
        let c = [5u8; 32];
        let ch = B64_URL.encode(c);
        let here = "https://mail.commonty.org";
        let try_ = |id: &str, answer: String| login(&e, id, &answer, &ch, here).is_ok();
        assert!(try_("pk1", t.answer(&c, "commonty.org", 0b101, here)));
        // another challenge, nobody present, another site, another passkey
        assert!(!try_(
            "pk1",
            t.answer(&[6; 32], "commonty.org", 0b101, here)
        ));
        assert!(!try_("pk1", t.answer(&c, "commonty.org", 0b100, here)));
        assert!(!try_(
            "pk1",
            t.answer(&c, "evil.example", 0b101, "https://evil.example")
        ));
        assert!(!try_("pk2", t.answer(&c, "commonty.org", 0b101, here)));
        // the same passkey, the same relying party, on a page a box serves:
        // a box that asked for it there must not get into the mail
        assert!(!try_(
            "pk1",
            t.answer(&c, "commonty.org", 0b101, "https://home.commonty.org")
        ));
    }
}
