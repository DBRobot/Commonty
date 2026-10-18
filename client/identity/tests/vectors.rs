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
                "expires": 1_792_000_000u64, "at": 1_791_700_000u64}),
        &ada_new_root,
    )
    .unwrap();

    // random entries: every awkward string serde writes, nested values with
    // awkward keys, the Go rebuild held to each byte
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let random: Vec<serde_json::Value> = (0..300)
        .map(|i| {
            let e = random_entry(&mut rng, i);
            let signed = SignedEntry {
                entry: e.clone(),
                signature: B64.encode([i as u8; 64]),
                recovery_signature: (i % 7 == 0).then(|| B64.encode([1u8; 64])),
            };
            json!({"logged": logged("entry", &signed),
                   "canonical": B64.encode(canonical(&e).unwrap())})
        })
        .collect();

    let shadow = shadow_log(&release, &pk);

    // ada's v2 entry with another (forged) signature over the same body
    let mut rewrapped = serde_json::to_value(&ada_v2).unwrap();
    rewrapped["sig"] = json!("cmV3cmFwcGVk");
    json!({
        "shadow_log": shadow,
        "random": random,
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
        // the item hash (identity::item_hash, Go's ItemHash): sha256 of what
        // was signed and its signatures. Another signature over the same body
        // (the last) is another item, with another hash
        "item_hashes": [
            {"item": logged("entry", &ada1_signed), "hash": identity::item_hash(&logged("entry", &ada1_signed))},
            {"item": logged("invite", &invite), "hash": identity::item_hash(&logged("invite", &invite))},
            {"item": logged("leave", &leave), "hash": identity::item_hash(&logged("leave", &leave))},
            {"item": logged("ack", "deadbeef"), "hash": identity::item_hash(&logged("ack", "deadbeef"))},
            {"item": serde_json::to_value(&ada_v2).unwrap(), "hash": identity::item_hash(&serde_json::to_value(&ada_v2).unwrap())},
            {"item": serde_json::to_value(&leave_v2).unwrap(), "hash": identity::item_hash(&serde_json::to_value(&leave_v2).unwrap())},
            {"item": rewrapped.clone(), "hash": identity::item_hash(&rewrapped)},
        ],
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

/// A log that exercises every rule of the gate's replay, for the shadow
/// check: replayed by the gate (shadow_dump) and by the judge, the answers
/// must be the same (testdata/shadow-gate.json is the gate's)
fn shadow_log(release: &SigningKey, pk: &Pk) -> serde_json::Value {
    let mut items = vec![];
    let mut t: i64 = 1_791_700_000_000;
    let mut put = |boxn: &str, item: serde_json::Value, t: &mut i64| {
        *t += 1000;
        items.push(json!({"time": *t, "box": boxn, "item": item}));
    };
    let ack = |item: &serde_json::Value| json!({ "ack": item_hash(item) });
    let ent = |name: &str,
               root: &SigningKey,
               paper: &SigningKey,
               dev: &SigningKey,
               libs: &[&str],
               v: u64| Entry {
        name: name.into(),
        root: encode_public(&root.verifying_key()),
        recovery: encode_public(&paper.verifying_key()),
        recovered_from: vec![],
        devices: vec![device(dev, 1)],
        passkeys: vec![],
        grant: None,
        libraries: libs
            .iter()
            .map(|id| Library {
                id: (*id).into(),
                keys: vec![],
                readers: vec![],
                created: 1,
            })
            .collect(),
        terms: Some("2026-10-11".into()),
        version: v,
        updated: 1_791_700_000 + v,
    };
    let (ann, ann_p, ann_d, ann2) = (key(20), key(21), key(22), key(23));
    let (bob, cat1, cat2, eve, gil) = (key(30), key(31), key(32), key(33), key(34));
    let lib_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    // ann claims; both voters ack; it lands
    let a1 = logged(
        "entry",
        sign(ent("ann", &ann, &ann_p, &ann_d, &[lib_a], 1), &ann).unwrap(),
    );
    put("node1", a1.clone(), &mut t);
    put("node1", ack(&a1), &mut t);
    put("node2", ack(&a1), &mut t);
    // an ack from a box that does not vote counts for nothing
    let b1 = logged(
        "entry",
        sign(ent("bob", &bob, &bob, &bob, &[], 1), &bob).unwrap(),
    );
    put("node2", b1.clone(), &mut t);
    put("node1", ack(&b1), &mut t);
    put("node3", ack(&b1), &mut t);
    // two claims to one name: the first acked in log order takes it
    let c1 = logged(
        "entry",
        sign(ent("cat", &cat1, &cat1, &cat1, &[], 1), &cat1).unwrap(),
    );
    let c2 = logged(
        "entry",
        sign(ent("cat", &cat2, &cat2, &cat2, &[], 1), &cat2).unwrap(),
    );
    put("node1", c1.clone(), &mut t);
    put("node2", c2.clone(), &mut t);
    for b in ["node1", "node2"] {
        put(b, ack(&c2), &mut t);
        put(b, ack(&c1), &mut t);
    }
    // ann's update lands; the same version again, and a forgery, do not
    let mut a2e = ent("ann", &ann, &ann_p, &ann_d, &[lib_a], 2);
    a2e.devices.push(device(&key(24), 2));
    put(
        "node1",
        logged("entry", sign(a2e.clone(), &ann).unwrap()),
        &mut t,
    );
    put(
        "node2",
        logged("entry", sign(a2e.clone(), &ann).unwrap()),
        &mut t,
    );
    let mut forged = sign(ent("ann", &ann, &ann_p, &ann_d, &[lib_a], 3), &ann).unwrap();
    forged.entry.version = 4;
    put("node1", logged("entry", forged), &mut t);
    // ann recovers: a new root, the paper key co-signing
    let mut a3e = a2e.clone();
    a3e.root = encode_public(&ann2.verifying_key());
    a3e.recovered_from = vec![a2e.root.clone()];
    a3e.version = 3;
    put(
        "node2",
        logged("entry", sign_recovery(a3e.clone(), &ann2, &ann_p).unwrap()),
        &mut t,
    );
    // without the paper key, a new root is refused
    let mut a4e = a3e.clone();
    a4e.root = encode_public(&eve.verifying_key());
    a4e.recovered_from = vec![a2e.root.clone(), a3e.root.clone()];
    a4e.version = 4;
    put("node1", logged("entry", sign(a4e, &eve).unwrap()), &mut t);
    // invites: the release key's lands, an expired one and another key's do not
    let code = code_key("qrstu-vwxyz");
    let inv = sign_invite(
        Invite {
            public_key: encode_public(&code.verifying_key()),
            issued: 1_791_600_000,
            expires: 1_792_000_000,
        },
        release,
    )
    .unwrap();
    put("node1", logged("invite", &inv), &mut t);
    put(
        "node1",
        logged(
            "invite",
            sign_invite(
                Invite {
                    public_key: "eA==".into(),
                    issued: 1,
                    expires: 2,
                },
                release,
            )
            .unwrap(),
        ),
        &mut t,
    );
    put(
        "node2",
        logged(
            "invite",
            sign_invite(
                Invite {
                    public_key: "eQ==".into(),
                    issued: 1_791_600_000,
                    expires: 1_792_000_000,
                },
                &eve,
            )
            .unwrap(),
        ),
        &mut t,
    );
    // dan joins by the invite; fin, another root, cannot use it again
    let dan = key(40);
    let mut de = ent("dan", &dan, &dan, &dan, &[], 1);
    de.grant = Some(Grant {
        invite: inv.clone(),
        redeemed: 1_791_700_000,
        proof: prove(&code, &inv.invite.public_key, &de.root, 1_791_700_000),
    });
    let d1 = logged("entry", sign(de, &dan).unwrap());
    put("node1", d1.clone(), &mut t);
    put("node1", ack(&d1), &mut t);
    put("node2", ack(&d1), &mut t);
    let fin = key(41);
    let mut fe = ent("fin", &fin, &fin, &fin, &[], 1);
    fe.grant = Some(Grant {
        invite: inv.clone(),
        redeemed: 1_791_700_000,
        proof: prove(&code, &inv.invite.public_key, &fe.root, 1_791_700_000),
    });
    put("node2", logged("entry", sign(fe, &fin).unwrap()), &mut t);
    // a library someone else holds is not gil's to claim
    put(
        "node1",
        logged(
            "entry",
            sign(ent("gil", &gil, &gil, &gil, &[lib_a], 1), &gil).unwrap(),
        ),
        &mut t,
    );
    // a passkey root claims
    let mut be = Entry {
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
        updated: 1_791_700_000,
    };
    be.updated += 1;
    let bs = SignedEntry {
        signature: pk.assert(&challenge(&be).unwrap()),
        entry: be,
        recovery_signature: None,
    };
    let bea1 = logged("entry", &bs);
    put("node2", bea1.clone(), &mut t);
    put("node1", ack(&bea1), &mut t);
    put("node2", ack(&bea1), &mut t);
    // ann leaves, in her new root's words; an older word and the old root's do not stand
    let at = (t / 1000) as u64;
    put(
        "node1",
        logged(
            "leave",
            sign_leave(
                Leave {
                    name: "ann".into(),
                    root: a3e.root.clone(),
                    at,
                    back: false,
                },
                &ann2,
            )
            .unwrap(),
        ),
        &mut t,
    );
    put(
        "node2",
        logged(
            "leave",
            sign_leave(
                Leave {
                    name: "ann".into(),
                    root: a3e.root.clone(),
                    at: at - 10,
                    back: false,
                },
                &ann2,
            )
            .unwrap(),
        ),
        &mut t,
    );
    put(
        "node1",
        logged(
            "leave",
            sign_leave(
                Leave {
                    name: "ann".into(),
                    root: a2e.root.clone(),
                    at,
                    back: false,
                },
                &ann,
            )
            .unwrap(),
        ),
        &mut t,
    );
    // a leave for nobody, and a statement too old to be taken
    put(
        "node2",
        logged(
            "leave",
            sign_leave(
                Leave {
                    name: "zed".into(),
                    root: a3e.root.clone(),
                    at,
                    back: false,
                },
                &ann2,
            )
            .unwrap(),
        ),
        &mut t,
    );
    put(
        "node2",
        logged(
            "leave",
            sign_leave(
                Leave {
                    name: "dan".into(),
                    root: encode_public(&dan.verifying_key()),
                    at: 1_000,
                    back: false,
                },
                &dan,
            )
            .unwrap(),
        ),
        &mut t,
    );
    // junk
    put("node1", json!({"foo": 1}), &mut t);
    put("node1", json!({"entry": {"entry": 1}}), &mut t);
    json!({ "items": items, "next": items.len() })
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn string(&mut self) -> String {
        const POOL: &[char] = &[
            'a',
            'z',
            '0',
            ' ',
            '"',
            '\\',
            '/',
            '<',
            '>',
            '&',
            '\'',
            '\u{0}',
            '\u{1}',
            '\u{8}',
            '\t',
            '\n',
            '\u{b}',
            '\u{c}',
            '\r',
            '\u{1f}',
            '\u{7f}',
            '\u{80}',
            'é',
            'ß',
            '漢',
            '\u{2028}',
            '\u{2029}',
            '\u{feff}',
            '\u{fffd}',
            '😀',
            '\u{10ffff}',
            '-',
            '_',
            '.',
            ':',
            '+',
            '=',
        ];
        (0..self.below(12))
            .map(|_| POOL[self.below(POOL.len() as u64) as usize])
            .collect()
    }
    fn value(&mut self, depth: u32) -> serde_json::Value {
        match if depth > 2 {
            self.below(4)
        } else {
            self.below(6)
        } {
            0 => serde_json::Value::Null,
            1 => json!(self.below(2) == 0),
            2 => json!(self.string()),
            3 => match self.below(3) {
                0 => json!(self.next()),
                1 => json!(-(self.below(1 << 40) as i64)),
                _ => json!(self.below(1000)),
            },
            4 => serde_json::Value::Array(
                (0..self.below(4)).map(|_| self.value(depth + 1)).collect(),
            ),
            _ => serde_json::Value::Object(
                (0..self.below(4))
                    .map(|_| (self.string(), self.value(depth + 1)))
                    .collect(),
            ),
        }
    }
}

fn random_entry(r: &mut Rng, i: usize) -> Entry {
    let opt = |r: &mut Rng| r.below(2) == 0;
    let n = |r: &mut Rng, k: u64| r.below(k);
    let devices = (0..n(r, 3))
        .map(|_| Device {
            fingerprint: r.string(),
            public_key: r.string(),
            added: r.next(),
        })
        .collect();
    let passkeys = (0..n(r, 3))
        .map(|_| Passkey {
            id: r.string(),
            cred: r.value(0),
            added: r.next(),
            rp_id: opt(r).then(|| r.string()),
            library_key: opt(r).then(|| r.string()),
        })
        .collect();
    let libraries = (0..n(r, 3))
        .map(|_| Library {
            id: r.string(),
            keys: (0..n(r, 3))
                .map(|_| SealedKey {
                    to: r.string(),
                    sealed: r.string(),
                })
                .collect(),
            readers: (0..n(r, 2))
                .map(|_| Reader {
                    name: r.string(),
                    keys: (0..n(r, 2))
                        .map(|_| SealedKey {
                            to: r.string(),
                            sealed: r.string(),
                        })
                        .collect(),
                })
                .collect(),
            created: r.next(),
        })
        .collect();
    let grant = opt(r).then(|| Grant {
        invite: SignedInvite {
            invite: Invite {
                public_key: r.string(),
                issued: r.next(),
                expires: r.next(),
            },
            signer: r.string(),
            signature: r.string(),
        },
        redeemed: r.next(),
        proof: r.string(),
    });
    Entry {
        name: format!("n{i}"),
        root: r.string(),
        recovery: r.string(),
        recovered_from: (0..n(r, 3)).map(|_| r.string()).collect(),
        devices,
        passkeys,
        grant,
        libraries,
        terms: opt(r).then(|| r.string()),
        version: r.next(),
        updated: r.next(),
    }
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
    // in the per-crate test build the flake hands the path in, so the check
    // runs there too instead of finding nothing and passing
    let committed = std::env::var("IDENT_VECTORS").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../servers/system/control/state/ident/testdata/vectors.json"
        )
        .to_string()
    });
    if let Ok(out) = std::env::var("IDENT_VECTORS_OUT") {
        std::fs::write(out, &v).unwrap();
        return;
    }
    if let Ok(had) = std::fs::read_to_string(&committed) {
        assert_eq!(
            had, v,
            "the vectors changed: regenerate them (IDENT_VECTORS_OUT)"
        );
    }
}
