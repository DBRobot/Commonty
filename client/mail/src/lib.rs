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
    let ids = |k: &str| -> Vec<String> {
        list[k]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(String::from))
            .collect()
    };
    let release = identity::decode_public(release).map_err(|e| e.to_string())?;
    // earlier roots count only as far as `admit` held them to the entry
    // pinned before: a first sight carries none (identity::accept)
    Ok(identity::is_member(
        &e.entry,
        &ids("members"),
        &ids("revoked"),
        Some(&release),
    ))
}

/// The release lists `e`'s root as its name's current root, and none of its
/// roots is revoked (identity::anchored): an entry from before a recovery,
/// or one let in only by an invite, is not enough.
fn anchored(e: &SignedEntry, members: &str) -> R<()> {
    let list: serde_json::Value = serde_json::from_str(members).map_err(|e| e.to_string())?;
    let ids = |k: &str| -> Vec<String> {
        list[k]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(String::from))
            .collect()
    };
    let names: std::collections::BTreeMap<String, String> =
        serde_json::from_value(list["names"].clone()).unwrap_or_default();
    identity::anchored(e, &ids("members"), &ids("revoked"), &names).map_err(|e| e.to_string())
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

/// The Worker's proof for a directory read of `name` at `at`: its read key
/// (DIRECTORY_KEY, made by `dd mail reader-key`), whose public half the
/// release names (fleet/mail.json), so the boxes show it single entries.
pub fn read_proof(secret: &str, name: &str, at: u64) -> R<String> {
    let key = identity::decode_secret(secret).map_err(|e| format!("DIRECTORY_KEY: {e}"))?;
    Ok(identity::read_proof(&key, name, at))
}

#[wasm_bindgen(js_name = readProof)]
pub fn read_proof_js(secret: &str, name: &str, at: f64) -> Result<String, JsValue> {
    read_proof(secret, name, at as u64).map_err(|e| JsValue::from_str(&e))
}

/// How long a member's things are kept after they are suspended, for them
/// to take; their own leaving keeps nothing (docs/dev/leaving.md). The
/// gate's window (servers/gate/src/leaving.rs WINDOW), to the day
pub const LEAVE_WINDOW: u64 = 90 * 24 * 3600;

/// May `name`'s address go? At once for their own leaving, signed by the
/// root of the entry this Worker pinned, and never a coming back; or the
/// window after the release's removal of them, by any root they have had.
/// `said` is what the gate serves at /_dd/leaving/<name>: a box can hold
/// back a statement, never make one. `fresh` is the directory's entry now,
/// taken over the pinned one only if it follows from it (a recovery since).
pub fn forget_due(
    name: &str,
    pinned: &str,
    fresh: Option<&str>,
    said: Option<&str>,
    members: &str,
    now: u64,
    window: u64,
) -> R<()> {
    let mut e = parse(pinned)?;
    if e.entry.name != name {
        return Err("that entry is someone else's".into());
    }
    if let Some(f) = fresh.map(parse).transpose()?
        && f.entry.version > e.entry.version
        && identity::accept(Some(&e), &f).is_ok()
    {
        e = f;
    }
    let list: serde_json::Value = serde_json::from_str(members).map_err(|e| e.to_string())?;
    for id in identity::member_ids(&e.entry) {
        let revoked = list["revoked"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(&id)));
        if revoked
            && let Some(at) = list["removed"][&id].as_u64()
            && at.saturating_add(window) <= now
        {
            return Ok(());
        }
    }
    let Some(said) = said else {
        return Err("they have not left".into());
    };
    let v: serde_json::Value = serde_json::from_str(said).map_err(|e| e.to_string())?;
    let s: identity::SignedLeave = serde_json::from_value(v["statement"].clone())
        .map_err(|_| "not a statement".to_string())?;
    identity::verify_leave(&s, &e.entry).map_err(|e| e.to_string())?;
    if s.leave.back {
        return Err("they came back".into());
    }
    if s.leave.at > now {
        return Err("they have not left yet".into());
    }
    Ok(())
}

#[wasm_bindgen(js_name = forgetDue)]
pub fn forget_due_js(
    name: &str,
    pinned: &str,
    fresh: Option<String>,
    said: Option<String>,
    members: &str,
    now: f64,
) -> Result<(), JsValue> {
    forget_due(
        name,
        pinned,
        fresh.as_deref(),
        said.as_deref(),
        members,
        now as u64,
        LEAVE_WINDOW,
    )
    .map_err(|e| JsValue::from_str(&e))
}

/// The words a member's root signs to give a home controller its name
/// (docs/dev/home-controller.md): the name, and the controller's key.
pub fn controller_name_message(label: &str, domain: &str, controller: &str) -> Vec<u8> {
    identity::controller_name_message(&format!("{label}.home.{domain}"), controller)
}

/// A controller's label: 16 of base32's lowercase letters and digits, from its key,
/// so the name says nothing about the house it is in
pub fn valid_label(label: &str) -> bool {
    label.len() == 16
        && label
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7'))
}

/// `sig` (ed25519, base64) by `public` (base64) over `message`
pub fn signed_by(public: &str, message: &[u8], sig: &str) -> bool {
    use base64::engine::general_purpose::STANDARD as B64;
    use ed25519_dalek::Verifier;
    let Ok(key) = identity::decode_public(public) else {
        return false;
    };
    let Ok(sig) = B64.decode(sig.trim()) else {
        return false;
    };
    let Ok(sig) = ed25519_dalek::Signature::from_slice(&sig) else {
        return false;
    };
    key.verify(message, &sig).is_ok()
}

/// May `controller` take `<label>.home.<domain>`? Only on the word of a
/// member's root, the one the release lists as theirs now (an old entry,
/// from before a recovery, names nothing: its root may be a thief's). The
/// member is who it returns. A box can sign none of this, so no box can name a
/// controller, nor get a certificate for one's name.
pub fn controller_name(
    entry: &str,
    label: &str,
    controller: &str,
    sig: &str,
    domain: &str,
    members: &str,
) -> R<String> {
    let e = parse(entry)?;
    identity::verify(&e).map_err(|e| e.to_string())?;
    if !valid_label(label) {
        return Err("not a controller's label".into());
    }
    if identity::decode_public(controller).is_err() {
        return Err("not a controller's key".into());
    }
    anchored(&e, members).map_err(|e| format!("not a member: {e}"))?;
    // a passkey root answers on the gate's page for it (/_dd/controller-name)
    identity::controller_name_signed(
        &e.entry,
        &format!("{label}.home.{domain}"),
        controller,
        sig,
        &[format!("https://home.{domain}")],
    )
    .map_err(|_| "not signed by that member's root".to_string())?;
    Ok(e.entry.name)
}

#[wasm_bindgen(js_name = controllerName)]
pub fn controller_name_js(
    entry: &str,
    label: &str,
    controller: &str,
    sig: &str,
    domain: &str,
    members: &str,
) -> Result<String, JsValue> {
    controller_name(entry, label, controller, sig, domain, members)
        .map_err(|e| JsValue::from_str(&e))
}

#[wasm_bindgen(js_name = signedBy)]
pub fn signed_by_js(public: &str, message: &str, sig: &str) -> bool {
    signed_by(public, message.as_bytes(), sig)
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
                recovered_from: vec![],
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
                terms: None,
                version,
                updated: version,
            };
            identity::sign(entry, &self.root).unwrap()
        }

        fn members(&self) -> String {
            let id = identity::member_id(&identity::encode_public(&self.root.verifying_key()));
            let names = serde_json::json!({ id.clone(): identity::name_tag(&id, "tester") });
            serde_json::json!({ "members": [id], "revoked": [], "names": names }).to_string()
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

    /// What the Worker sends with a read is what a box takes from the key
    /// the release names, for that name only.
    #[test]
    fn the_workers_read_proof_is_the_boxes_kind() {
        let k = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let secret = identity::encode_secret(&k);
        let p = read_proof(&secret, "tester", 1000).unwrap();
        assert_eq!(
            identity::check_read_proof(&p, "tester", 1010),
            Some(identity::encode_public(&k.verifying_key()))
        );
        assert_eq!(identity::check_read_proof(&p, "other", 1010), None);
        assert!(read_proof("not a key", "tester", 1000).is_err());
    }

    #[test]
    fn an_address_goes_at_once_on_leaving_and_the_window_after_a_removal() {
        let t = Tester::new();
        let e = serde_json::to_string(&t.entry(3)).unwrap();
        let w = LEAVE_WINDOW;
        let said = |at, back| {
            let s = identity::sign_leave(
                identity::Leave {
                    name: "tester".into(),
                    root: identity::encode_public(&t.root.verifying_key()),
                    at,
                    back,
                },
                &t.root,
            )
            .unwrap();
            serde_json::json!({ "statement": s, "agreed": at }).to_string()
        };
        let m = t.members();
        assert!(forget_due("tester", &e, None, None, &m, 10 * w, w).is_err());
        assert!(
            forget_due("tester", &e, None, Some(&said(100, false)), &m, 100, w).is_ok(),
            "their own word: at once"
        );
        assert!(forget_due("tester", &e, None, Some(&said(100, false)), &m, 99, w).is_err());
        assert!(forget_due("tester", &e, None, Some(&said(100, true)), &m, 100 + w, w).is_err());
        assert!(forget_due("other", &e, None, Some(&said(100, false)), &m, 100 + w, w).is_err());
        let mut forged: serde_json::Value = serde_json::from_str(&said(100, false)).unwrap();
        forged["statement"]["leave"]["at"] = 1.into();
        assert!(
            forget_due(
                "tester",
                &e,
                None,
                Some(&forged.to_string()),
                &m,
                100 + w,
                w
            )
            .is_err()
        );
        let id = identity::member_id(&identity::encode_public(&t.root.verifying_key()));
        let removed =
            serde_json::json!({ "members": [], "revoked": [id], "removed": { id.clone(): 50 } })
                .to_string();
        assert!(forget_due("tester", &e, None, None, &removed, 50 + w, w).is_ok());
        assert!(forget_due("tester", &e, None, None, &removed, 49 + w, w).is_err());
        let not_revoked =
            serde_json::json!({ "members": [id], "removed": { id.clone(): 50 } }).to_string();
        assert!(forget_due("tester", &e, None, None, &not_revoked, 50 + w, w).is_err());
    }

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

    #[test]
    fn an_entry_from_before_a_recovery_names_no_controller() {
        use base64::engine::general_purpose::STANDARD as B64;
        use ed25519_dalek::Signer as _;
        let t = Tester::new();
        let entry = serde_json::to_string(&t.entry(3)).unwrap();
        let controller = identity::encode_public(
            &ed25519_dalek::SigningKey::from_bytes(&[5; 32]).verifying_key(),
        );
        let label = &identity::controller_label(&controller).unwrap();
        let sig = B64.encode(
            t.root
                .sign(&controller_name_message(label, "commonty.org", &controller))
                .to_bytes(),
        );
        // the release now lists another root as the tester's: this one, from
        // before, may be in a thief's hands
        let old = identity::member_id(&identity::encode_public(&t.root.verifying_key()));
        let new = identity::member_id(&identity::encode_public(
            &ed25519_dalek::SigningKey::from_bytes(&[9; 32]).verifying_key(),
        ));
        let moved = serde_json::json!({
            "members": [old, new.clone()],
            "revoked": [],
            "names": { new.clone(): identity::name_tag(&new, "tester") },
        })
        .to_string();
        let e = controller_name(&entry, label, &controller, &sig, "commonty.org", &moved);
        assert!(e.unwrap_err().contains("not a member"));
        // and a list that names nobody as the tester's names nothing
        let unnamed = serde_json::json!({ "members": [old], "revoked": [] }).to_string();
        assert!(
            controller_name(&entry, label, &controller, &sig, "commonty.org", &unnamed).is_err()
        );
        assert!(
            controller_name(
                &entry,
                label,
                &controller,
                &sig,
                "commonty.org",
                &t.members()
            )
            .is_ok()
        );
    }

    #[test]
    fn a_members_root_names_a_controller_and_nothing_else_does() {
        use base64::engine::general_purpose::STANDARD as B64;
        use ed25519_dalek::Signer as _;
        let t = Tester::new();
        let entry = serde_json::to_string(&t.entry(3)).unwrap();
        let controller = identity::encode_public(
            &ed25519_dalek::SigningKey::from_bytes(&[5; 32]).verifying_key(),
        );
        let label = &identity::controller_label(&controller).unwrap();
        let sign = |k: &ed25519_dalek::SigningKey, label: &str, c: &str| {
            B64.encode(
                k.sign(&controller_name_message(label, "commonty.org", c))
                    .to_bytes(),
            )
        };
        let good = sign(&t.root, label, &controller);
        let name = controller_name(
            &entry,
            label,
            &controller,
            &good,
            "commonty.org",
            &t.members(),
        );
        assert_eq!(name.unwrap(), "tester");

        // someone else's key, a box's for instance
        let other = sign(
            &ed25519_dalek::SigningKey::from_bytes(&[8; 32]),
            label,
            &controller,
        );
        assert!(
            controller_name(
                &entry,
                label,
                &controller,
                &other,
                "commonty.org",
                &t.members()
            )
            .is_err()
        );
        // the root's word for another name, or another controller, or another fleet
        assert!(
            controller_name(
                &entry,
                "bbbbbbbbbbbbbbbb",
                &controller,
                &good,
                "commonty.org",
                &t.members()
            )
            .is_err()
        );
        let swapped = identity::encode_public(
            &ed25519_dalek::SigningKey::from_bytes(&[6; 32]).verifying_key(),
        );
        assert!(
            controller_name(&entry, label, &swapped, &good, "commonty.org", &t.members()).is_err()
        );
        assert!(
            controller_name(
                &entry,
                label,
                &controller,
                &good,
                "example.org",
                &t.members()
            )
            .is_err()
        );
        // not a member any more
        let id = identity::member_id(&identity::encode_public(&t.root.verifying_key()));
        let gone = serde_json::json!({ "members": [], "revoked": [id] }).to_string();
        assert!(controller_name(&entry, label, &controller, &good, "commonty.org", &gone).is_err());
        // a label that could carry a word
        let word = sign(&t.root, "living-room", &controller);
        assert!(
            controller_name(
                &entry,
                "living-room",
                &controller,
                &word,
                "commonty.org",
                &t.members()
            )
            .is_err()
        );
    }

    #[test]
    fn a_passkey_root_names_a_controller_on_the_gates_page_alone() {
        use base64::engine::general_purpose::STANDARD as B64;
        let t = Tester::new();
        // the same member, but the root is the passkey
        let mut e = t.entry(3).entry;
        let cred = e.passkeys[0].cred.clone();
        e.root = identity::passkey_root("pk1", &cred);
        e.devices.clear();
        let answer_b64 = |challenge: &[u8], origin: &str| {
            B64.encode(t.answer(challenge, "commonty.org", 0x05, origin))
        };
        let sig = answer_b64(
            &identity::challenge(&e).unwrap(),
            "https://home.commonty.org",
        );
        let signed = SignedEntry {
            entry: e.clone(),
            signature: sig,
            recovery_signature: None,
        };
        identity::verify(&signed).unwrap();
        let id = identity::member_id(&e.root);
        let members = serde_json::json!({
            "members": [id.clone()],
            "revoked": [],
            "names": { id.clone(): identity::name_tag(&id, "tester") },
        })
        .to_string();
        let entry = serde_json::to_string(&signed).unwrap();
        let controller = identity::encode_public(
            &ed25519_dalek::SigningKey::from_bytes(&[5; 32]).verifying_key(),
        );
        let label = &identity::controller_label(&controller).unwrap();
        let msg = controller_name_message(label, "commonty.org", &controller);
        let challenge = <sha2::Sha256 as sha2::Digest>::digest(&msg);
        let good = answer_b64(&challenge, "https://home.commonty.org");
        assert_eq!(
            controller_name(&entry, label, &controller, &good, "commonty.org", &members).unwrap(),
            "tester"
        );
        // answered on another page, or for another controller
        let elsewhere = answer_b64(&challenge, "https://evil.commonty.org");
        assert!(
            controller_name(
                &entry,
                label,
                &controller,
                &elsewhere,
                "commonty.org",
                &members
            )
            .is_err()
        );
        let other = identity::encode_public(
            &ed25519_dalek::SigningKey::from_bytes(&[6; 32]).verifying_key(),
        );
        assert!(controller_name(&entry, label, &other, &good, "commonty.org", &members).is_err());
    }
}
