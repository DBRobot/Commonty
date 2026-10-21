//! Signing in at the door: a passkey from the person's entry, or one of
//! their devices, answering a challenge for this door's name. Decided from
//! the entries the door saved; a box can hand the door an old entry or
//! none, never a forged one (identity::accept).

use crate::{App, COOKIE, Fut, SESSION_LIFE, refuse, same_origin, state};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use identity::{Assertion, SignedEntry};
use serde::Deserialize;
use std::sync::Arc;

/// how long a challenge may be answered
pub const NONCE_LIFE: u64 = 120;
pub const LABEL: &[u8] = b"commonty door v1\0";

/// What a device signs: the label, the door's name, the nonce.
pub fn message(host: &str, nonce: &str) -> Vec<u8> {
    let mut m = LABEL.to_vec();
    m.extend_from_slice(host.as_bytes());
    m.push(0);
    m.extend_from_slice(nonce.as_bytes());
    m
}

/// What a device signs: that, and the name it signs in as. The same
/// signature is never good for anyone else, whatever entries list the
/// device's key.
pub fn device_message(host: &str, nonce: &str, name: &str) -> Vec<u8> {
    let mut m = message(host, nonce);
    m.push(0);
    m.extend_from_slice(name.as_bytes());
    m
}

/// What a passkey signs: the same, hashed, as the webauthn challenge.
pub fn challenge_for(host: &str, nonce: &str) -> Vec<u8> {
    use sha2::Digest as _;
    sha2::Sha256::digest(message(host, nonce)).to_vec()
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum SignIn {
    Device {
        name: String,
        #[serde(default)]
        entry: Option<SignedEntry>,
        /// the device's public key, as the entry lists it
        device: String,
        /// base64 ed25519 over `message`
        sig: String,
        nonce: String,
    },
    Passkey {
        /// left out: the door finds the person whose entry has the passkey
        #[serde(default)]
        name: Option<String>,
        id: String,
        assertion: Assertion,
        nonce: String,
        /// for someone not in yet, who has no entry here to check against
        #[serde(default)]
        entry: Option<SignedEntry>,
    },
}

/// Reads a person's entry from the fleet's directory, when it can: the
/// gate's /_dd/controller/directory/<name> (gate.rs). A box serving it can
/// only withhold or replay; whatever comes back goes through identity::accept.
pub trait Directory: Send + Sync {
    fn entry<'a>(&'a self, domain: &'a str, name: &'a str) -> Fut<'a, Option<SignedEntry>>;
    /// Whether the gate gives this person the fleet's services, by the
    /// root their entry has (the gate's /_dd/controller/standing/<name>):
    /// a member the release's list does not name, by an invite since or a
    /// box of theirs. Asked as `reg` says, or as this door's registration
    /// once it has one. None: the gate does not say.
    fn standing<'a>(
        &'a self,
        _reg: Option<&'a crate::pair::Registration>,
        _name: &'a str,
    ) -> Fut<'a, Option<Standing>> {
        Box::pin(async { Ok(None) })
    }
}

/// The gate's word on someone (Directory::standing)
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct Standing {
    pub root: String,
    pub member: bool,
}

/// Ask the gate about someone the release's list does not name, and keep
/// its answer for a while (App::member): before a sign-in is checked, and
/// for everyone in at each refresh
pub async fn vouch(app: &App, reg: Option<&crate::pair::Registration>, name: &str) {
    match app.directory.standing(reg, name).await {
        Ok(Some(s)) if s.member => app.vouched(name, &s.root),
        Ok(_) => app.unvouched(name),
        // not reached: what it said before stands until it runs out
        Err(e) => eprintln!("door: asking the gate about {name}: {e:#}"),
    }
}

/// `<label>.home.<domain>` → `<domain>`
pub fn domain_of(name: &str) -> Option<&str> {
    let mut p = name.splitn(3, '.');
    let (_, home, domain) = (p.next()?, p.next()?, p.next()?);
    (home == "home" && !domain.is_empty()).then_some(domain)
}

type Refusal = (StatusCode, String);

fn no(status: StatusCode, why: &str) -> Refusal {
    (status, why.to_string())
}

fn same(a: &SignedEntry, b: &SignedEntry) -> bool {
    identity::canonical(&a.entry).ok() == identity::canonical(&b.entry).ok()
        && a.signature == b.signature
}

/// The entry a sign-in is checked against, and whether its owner is in.
/// An offered entry replaces the saved one only where identity::accept
/// allows; the same one again is fine, anything else is refused.
async fn entry_for(
    app: &App,
    name: &str,
    offered: Option<SignedEntry>,
) -> Result<(SignedEntry, bool), Refusal> {
    if let Some(e) = &offered
        && e.entry.name != name
    {
        return Err(no(StatusCode::BAD_REQUEST, "that entry is someone else's"));
    }
    if let Some(e) = &offered
        && !app.member(e)
        && !app.member_vouching(e).await
    {
        return Err(no(StatusCode::FORBIDDEN, "not a member of this fleet"));
    }
    let (saved, is_in) = app.with_store(|s| match s.state.people.get(name) {
        Some(p) => (Some(p.entry.clone()), true),
        None => (s.state.asked.get(name).map(|a| a.entry.clone()), false),
    });
    match (saved, offered) {
        (Some(saved), None) => Ok((saved, is_in)),
        (Some(saved), Some(new)) if same(&saved, &new) => Ok((saved, is_in)),
        (Some(saved), Some(new)) => match identity::accept(Some(&saved), &new) {
            Ok(()) => Ok((new, is_in)),
            Err(_) if is_in => Err(no(
                StatusCode::FORBIDDEN,
                "that entry is older than the one this home holds, or not theirs",
            )),
            Err(_) => Err(no(
                StatusCode::FORBIDDEN,
                "that entry does not follow the one asked with",
            )),
        },
        (None, offered) => {
            let new = match offered {
                Some(e) => e,
                None => {
                    let domain = app
                        .name()
                        .and_then(|n| domain_of(&n).map(str::to_string))
                        .ok_or_else(|| no(StatusCode::FORBIDDEN, "not known here"))?;
                    app.directory
                        .entry(&domain, name)
                        .await
                        .ok()
                        .flatten()
                        .ok_or_else(|| no(StatusCode::NOT_FOUND, "not known here"))?
                }
            };
            identity::accept(None, &new).map_err(|_| {
                no(
                    StatusCode::FORBIDDEN,
                    "that entry is not signed by its root",
                )
            })?;
            Ok((new, false))
        }
    }
}

/// The entry, if its owner is still a member of the fleet: one the release
/// revoked is not let through, whatever this home saved of them.
async fn member_entry(
    app: &App,
    name: &str,
    offered: Option<SignedEntry>,
) -> Result<(SignedEntry, bool), Refusal> {
    let (e, is_in) = entry_for(app, name, offered).await?;
    if !app.member(&e) && !app.member_vouching(&e).await {
        return Err(no(StatusCode::FORBIDDEN, "not a member of this fleet"));
    }
    Ok((e, is_in))
}

/// The checks, all of them: (name, the entry they signed in under, in).
pub async fn check(app: &App, req: SignIn) -> Result<(String, SignedEntry, bool), Refusal> {
    let host = app
        .name()
        .ok_or_else(|| no(StatusCode::SERVICE_UNAVAILABLE, "this door has no name yet"))?;
    let nonce = match &req {
        SignIn::Device { nonce, .. } | SignIn::Passkey { nonce, .. } => nonce.clone(),
    };
    // spent before anything else is looked at: one try per challenge
    if !app.take_nonce(&nonce) {
        return Err(no(
            StatusCode::UNAUTHORIZED,
            "the challenge ran out; try again",
        ));
    }
    match req {
        SignIn::Device {
            name,
            entry,
            device,
            sig,
            ..
        } => {
            let (entry, is_in) = member_entry(app, &name, entry).await?;
            if !entry.entry.devices.iter().any(|d| d.public_key == device) {
                return Err(no(
                    StatusCode::FORBIDDEN,
                    "that device is not in their entry",
                ));
            }
            check_device(&device, &sig, &device_message(&host, &nonce, &name))
                .map_err(|_| no(StatusCode::FORBIDDEN, "the signature does not check"))?;
            Ok((name, entry, is_in))
        }
        SignIn::Passkey {
            name,
            id,
            assertion,
            entry,
            ..
        } => {
            let name = match name {
                Some(n) => n,
                None => app
                    .with_store(|s| {
                        s.state
                            .people
                            .iter()
                            .find(|(_, p)| p.entry.entry.passkeys.iter().any(|k| k.id == id))
                            .map(|(n, _)| n.clone())
                    })
                    .ok_or_else(|| {
                        no(
                            StatusCode::NOT_FOUND,
                            "this home does not know that passkey",
                        )
                    })?,
            };
            // a passkey is one person's: listed in two entries of people
            // let in, it is nobody's here, so no one in can copy another's
            // into their own entry and be taken for them. Only those let in
            // count: someone merely asking cannot shut anyone out this way
            let holders = app.with_store(|s| {
                s.state
                    .people
                    .values()
                    .filter(|p| p.entry.entry.passkeys.iter().any(|k| k.id == id))
                    .count()
            });
            // and someone not in may not answer with the passkey of
            // someone who is
            let is_in = app.with_store(|s| s.state.people.contains_key(&name));
            if holders > 1 || (!is_in && holders > 0) {
                return Err(no(
                    StatusCode::FORBIDDEN,
                    "that passkey is listed by more than one person here",
                ));
            }
            let (entry, is_in) = member_entry(app, &name, entry).await?;
            identity::check_login(
                &entry.entry,
                &id,
                &assertion,
                &challenge_for(&host, &nonce),
                &[format!("https://{host}")],
            )
            .map_err(|_| no(StatusCode::FORBIDDEN, "the passkey's answer does not check"))?;
            Ok((name, entry, is_in))
        }
    }
}

fn check_device(device: &str, sig: &str, msg: &[u8]) -> anyhow::Result<()> {
    use base64::Engine as _;
    use ed25519_dalek::Verifier as _;
    let key = identity::decode_public(device)?;
    let sig = base64::engine::general_purpose::STANDARD.decode(sig.trim())?;
    let sig = ed25519_dalek::Signature::from_slice(&sig)?;
    Ok(key.verify(msg, &sig)?)
}

pub async fn challenge(State(app): State<Arc<App>>) -> Response {
    Json(serde_json::json!({ "nonce": app.new_nonce() })).into_response()
}

pub async fn signin(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SignIn>,
) -> Response {
    if !same_origin(&app, &headers) {
        return refuse(StatusCode::FORBIDDEN, "not from this door's page");
    }
    let (name, entry, is_in) = match check(&app, req).await {
        Ok(ok) => ok,
        Err((status, why)) => return refuse(status, &why),
    };
    let cookie = crate::random_b64url(32);
    let now = identity::now();
    let saved = app.with_store(|s| {
        if let Some(p) = s.state.people.get_mut(&name) {
            // a newer entry they brought: its root is the one pinned now
            if !same(&p.entry, &entry) {
                p.root = entry.entry.root.clone();
                p.entry = entry.clone();
            }
        }
        s.drop_expired(now);
        s.state.sessions.insert(
            state::hash(&cookie),
            state::Session {
                name: name.clone(),
                root: entry.entry.root.clone(),
                expires: now + SESSION_LIFE,
                entry: (!is_in).then_some(entry.clone()),
            },
        );
        s.save()
    });
    if let Err(e) = saved {
        eprintln!("signin: {e:#}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    (
        [(
            header::SET_COOKIE,
            format!(
                "{COOKIE}={cookie}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_LIFE}"
            ),
        )],
        Json(serde_json::json!({ "name": name, "in": is_in })),
    )
        .into_response()
}

pub async fn signout(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !same_origin(&app, &headers) {
        return refuse(StatusCode::FORBIDDEN, "not from this door's page");
    }
    if let Some(c) = crate::cookie(&headers) {
        let _ = app.with_store(|s| {
            s.state.sessions.remove(&state::hash(&c));
            s.save()
        });
    }
    (
        [(
            header::SET_COOKIE,
            format!("{COOKIE}=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"),
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

/// Newer entries for the people in, from the directory, where it answers.
pub async fn refresh(app: &App) {
    let Some(domain) = app.name().and_then(|n| domain_of(&n).map(str::to_string)) else {
        return;
    };
    let people: Vec<(String, SignedEntry)> = app.with_store(|s| {
        s.state
            .people
            .iter()
            .map(|(n, p)| (n.clone(), p.entry.clone()))
            .collect()
    });
    for (name, saved) in people {
        // whoever the list does not name is asked about again, so a
        // member by a box stays in while the gate says so and no longer
        if !app.cfg.members.as_ref().is_none_or(|m| m.has(&saved)) {
            vouch(app, None, &name).await;
        }
        let Ok(Some(new)) = app.directory.entry(&domain, &name).await else {
            continue;
        };
        if same(&saved, &new) || identity::accept(Some(&saved), &new).is_err() {
            continue;
        }
        let _ = app.with_store(|s| {
            if let Some(p) = s.state.people.get_mut(&name)
                && same(&p.entry, &saved)
            {
                p.root = new.entry.root.clone();
                p.entry = new;
            }
            s.save()
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tests::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64_URL};
    use ed25519_dalek::Signer as _;
    use identity::Passkey;
    use sha2::Digest as _;

    /// A passkey as an authenticator would be (as in identity's own tests).
    pub struct Authenticator {
        key: p256::ecdsa::SigningKey,
        pub id: String,
    }

    impl Authenticator {
        pub fn new(id: &str) -> Self {
            Self {
                key: p256::ecdsa::SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng),
                id: id.into(),
            }
        }
        pub fn on_file(&self) -> Passkey {
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
        pub fn assert(&self, challenge: &[u8], origin: &str) -> Assertion {
            let client = serde_json::json!({
                "type": "webauthn.get",
                "challenge": B64_URL.encode(challenge),
                "origin": origin,
            })
            .to_string();
            let mut auth = sha2::Sha256::digest(b"commonty.org").to_vec();
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

    pub fn device_sig(k: &identity::SigningKey, host: &str, nonce: &str, name: &str) -> String {
        B64.encode(k.sign(&device_message(host, nonce, name)).to_bytes())
    }

    async fn nonce(url: &str) -> String {
        let v: serde_json::Value = client()
            .get(format!("{url}/_door/challenge"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["nonce"].as_str().unwrap().to_string()
    }

    async fn post(url: &str, body: serde_json::Value) -> reqwest::Response {
        client()
            .post(format!("{url}/_door/signin"))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    fn pk(e: &SignedEntry) -> String {
        e.entry.root.clone()
    }

    #[test]
    fn the_challenge_is_for_one_door() {
        assert_eq!(
            message("a.home.x", "n"),
            b"commonty door v1\0a.home.x\0n".to_vec()
        );
        assert_ne!(
            challenge_for("a.home.x", "n"),
            challenge_for("b.home.x", "n")
        );
        assert_eq!(domain_of("abc.home.commonty.org"), Some("commonty.org"));
        assert_eq!(domain_of("abc.commonty.org"), None);
    }

    #[tokio::test]
    async fn a_device_in_the_entry_signs_in_once_per_challenge() {
        let r = rig("device");
        let (e, k, _) = person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;
        let n = nonce(&url).await;
        let body = serde_json::json!({
            "name": "ann", "device": pk(&e), "sig": device_sig(&k, NAME, &n, "ann"), "nonce": n,
        });
        let res = post(&url, body.clone()).await;
        assert_eq!(res.status(), 200);
        let set = res.headers()["set-cookie"].to_str().unwrap().to_string();
        assert!(set.starts_with("door="));
        for part in [
            "Secure",
            "HttpOnly",
            "SameSite=Strict",
            "Path=/",
            "Max-Age=28800",
        ] {
            assert!(set.contains(part), "{set}");
        }
        let v: serde_json::Value = res.json().await.unwrap();
        assert_eq!(v["in"], true);
        // the same answer again: the nonce is spent
        assert_eq!(post(&url, body).await.status(), 401);
    }

    #[tokio::test]
    async fn what_the_door_refuses_from_a_device() {
        let r = rig("device-no");
        let (e, k, _) = person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;
        let other = identity::generate();

        // signed for another door
        let n = nonce(&url).await;
        let s = device_sig(&k, "other.home.commonty.org", &n, "ann");
        let res = post(
            &url,
            serde_json::json!({"name":"ann","device":pk(&e),"sig":s,"nonce":n}),
        )
        .await;
        assert_eq!(res.status(), 403);

        // a key that is not in her entry, signing properly
        let n = nonce(&url).await;
        let s = device_sig(&other, NAME, &n, "ann");
        let dev = identity::encode_public(&other.verifying_key());
        let res = post(
            &url,
            serde_json::json!({"name":"ann","device":dev,"sig":s,"nonce":n}),
        )
        .await;
        assert_eq!(res.status(), 403);

        // her own signature, made for another name
        let n = nonce(&url).await;
        let s = device_sig(&k, NAME, &n, "bob");
        let res = post(
            &url,
            serde_json::json!({"name":"ann","device":pk(&e),"sig":s,"nonce":n}),
        )
        .await;
        assert_eq!(res.status(), 403);

        // her device, someone else's signature
        let n = nonce(&url).await;
        let s = device_sig(&other, NAME, &n, "ann");
        let res = post(
            &url,
            serde_json::json!({"name":"ann","device":pk(&e),"sig":s,"nonce":n}),
        )
        .await;
        assert_eq!(res.status(), 403);

        // a nonce the door never gave, and one that ran out
        let s = device_sig(&k, NAME, "made-up", "ann");
        let res = post(
            &url,
            serde_json::json!({"name":"ann","device":pk(&e),"sig":s,"nonce":"made-up"}),
        )
        .await;
        assert_eq!(res.status(), 401);
        let n = r.app.new_nonce();
        r.app
            .nonces
            .lock()
            .unwrap()
            .insert(n.clone(), identity::now() - 1);
        let s = device_sig(&k, NAME, &n, "ann");
        let res = post(
            &url,
            serde_json::json!({"name":"ann","device":pk(&e),"sig":s,"nonce":n}),
        )
        .await;
        assert_eq!(res.status(), 401);

        // from another site's page
        let n = nonce(&url).await;
        let s = device_sig(&k, NAME, &n, "ann");
        let res = client()
            .post(format!("{url}/_door/signin"))
            .header("origin", "https://evil.example")
            .json(&serde_json::json!({"name":"ann","device":pk(&e),"sig":s,"nonce":n}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403);
        assert!(
            r.app.with_store(|s| s.state.sessions.len()) == 1,
            "only person()'s own"
        );
    }

    #[tokio::test]
    async fn a_newer_entry_replaces_the_saved_one_an_older_or_forged_one_never() {
        let r = rig("entries");
        let (e, k, _) = person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;

        // v2 adds a phone, signed by her root: the phone signs in with it
        let phone = identity::generate();
        let phone_pk = identity::encode_public(&phone.verifying_key());
        let mut v2 = e.entry.clone();
        v2.version = 2;
        v2.devices.push(identity::Device {
            fingerprint: identity::fingerprint(&phone_pk),
            public_key: phone_pk.clone(),
            added: 2,
        });
        let v2 = identity::sign(v2, &k).unwrap();
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"ann","entry":v2,"device":phone_pk,"sig":device_sig(&phone, NAME, &n, "ann"),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 200);
        assert_eq!(
            r.app
                .with_store(|s| s.state.people["ann"].entry.entry.version),
            2
        );

        // v1 again, with the old device: refused, it is older
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"ann","entry":e,"device":pk(&e),"sig":device_sig(&k, NAME, &n, "ann"),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 403);

        // a v3 with a stranger's device, signed by the stranger as if root
        let mallory = identity::generate();
        let m_pk = identity::encode_public(&mallory.verifying_key());
        let mut v3 = v2.entry.clone();
        v3.version = 3;
        v3.devices = vec![identity::Device {
            fingerprint: identity::fingerprint(&m_pk),
            public_key: m_pk.clone(),
            added: 3,
        }];
        let forged = SignedEntry {
            signature: B64.encode(mallory.sign(&identity::canonical(&v3).unwrap()).to_bytes()),
            entry: v3,
            recovery_signature: None,
        };
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"ann","entry":forged,"device":m_pk,"sig":device_sig(&mallory, NAME, &n, "ann"),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 403);
        assert_eq!(
            r.app
                .with_store(|s| s.state.people["ann"].entry.entry.version),
            2
        );

        // someone else's entry under her name
        let (bob, bk) = signed_entry("bob");
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"ann","entry":bob,"device":pk(&bob),"sig":device_sig(&bk,NAME,&n,"bob"),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 400);
    }

    fn with_passkey(r: &Rig, name: &str, a: &Authenticator) {
        let (e, k) = signed_entry(name);
        let mut entry = e.entry.clone();
        entry.passkeys.push(a.on_file());
        let e = identity::sign(entry, &k).unwrap();
        r.app.with_store(|s| {
            s.state.people.insert(
                name.into(),
                state::Person {
                    root: e.entry.root.clone(),
                    entry: e,
                    ha_user: None,
                    address: None,
                    since: 0,
                },
            );
        });
    }

    #[tokio::test]
    async fn a_passkey_in_the_entry_signs_in_here_and_nowhere_else() {
        let r = rig("passkey");
        let a = Authenticator::new("cred-1");
        with_passkey(&r, "ann", &a);
        let url = serve(crate::router(r.app.clone())).await;
        let origin = format!("https://{NAME}");

        // with no name: the door finds her by the passkey
        let n = nonce(&url).await;
        let body = serde_json::json!({"id":"cred-1","assertion":a.assert(&challenge_for(NAME,&n),&origin),"nonce":n});
        let res = post(&url, body).await;
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = res.json().await.unwrap();
        assert_eq!(v["name"], "ann");

        // answered on another page
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"ann","id":"cred-1","assertion":a.assert(&challenge_for(NAME,&n),"https://home.commonty.org"),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 403);

        // another door's challenge
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"ann","id":"cred-1","assertion":a.assert(&challenge_for("x.home.commonty.org",&n),&origin),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 403);

        // a passkey nobody here has
        let b = Authenticator::new("cred-2");
        let n = nonce(&url).await;
        let body = serde_json::json!({"id":"cred-2","assertion":b.assert(&challenge_for(NAME,&n),&origin),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 404);

        // her passkey id, someone else's key
        let n = nonce(&url).await;
        let fake = Authenticator {
            key: b.key.clone(),
            id: "cred-1".into(),
        };
        let body = serde_json::json!({"name":"ann","id":"cred-1","assertion":fake.assert(&challenge_for(NAME,&n),&origin),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 403);
    }

    #[tokio::test]
    async fn a_passkey_two_entries_list_signs_nobody_in() {
        let r = rig("passkey-twice");
        let a = Authenticator::new("cred-1");
        with_passkey(&r, "ann", &a);
        // bob copies ann's passkey, its id and its key, into his own entry
        with_passkey(&r, "bob", &a);
        let url = serve(crate::router(r.app.clone())).await;
        let origin = format!("https://{NAME}");
        for name in [None, Some("ann"), Some("bob")] {
            let n = nonce(&url).await;
            let mut body = serde_json::json!({"id":"cred-1","assertion":a.assert(&challenge_for(NAME,&n),&origin),"nonce":n});
            if let Some(name) = name {
                body["name"] = name.into();
            }
            assert_eq!(post(&url, body).await.status(), 403, "{name:?}");
        }
    }

    #[tokio::test]
    async fn someone_asking_cannot_shut_a_passkey_out() {
        let r = rig("passkey-asked");
        let a = Authenticator::new("cred-1");
        with_passkey(&r, "ann", &a);
        // eve, only asking, lists ann's passkey in her own entry
        let (mut eve, ek) = signed_entry("eve");
        eve.entry.passkeys = r
            .app
            .with_store(|s| s.state.people["ann"].entry.entry.passkeys.clone());
        eve.entry.version += 1;
        let eve = identity::sign(eve.entry, &ek).unwrap();
        r.app.with_store(|s| {
            s.state.asked.insert(
                "eve".into(),
                crate::state::Asked {
                    entry: eve.clone(),
                    at: 1,
                },
            );
            s.save().unwrap();
        });
        let url = serve(crate::router(r.app.clone())).await;
        let n = nonce(&url).await;
        let body = serde_json::json!({"id":"cred-1","assertion":a.assert(&challenge_for(NAME,&n),&format!("https://{NAME}")),"nonce":n});
        let res = post(&url, body).await;
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = res.json().await.unwrap();
        assert_eq!(v["name"], "ann");
    }

    #[tokio::test]
    async fn someone_asking_cannot_answer_with_the_passkey_of_someone_in() {
        let r = rig("passkey-borrowed");
        let a = Authenticator::new("cred-1");
        with_passkey(&r, "ann", &a);
        // eve, only asking, lists ann's passkey as her own and names herself
        let (mut eve, ek) = signed_entry("eve");
        eve.entry.passkeys = r
            .app
            .with_store(|s| s.state.people["ann"].entry.entry.passkeys.clone());
        eve.entry.version += 1;
        let eve = identity::sign(eve.entry, &ek).unwrap();
        let url = serve(crate::router(r.app.clone())).await;
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"eve","entry":eve,"id":"cred-1","assertion":a.assert(&challenge_for(NAME,&n),&format!("https://{NAME}")),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 403);
    }

    #[tokio::test]
    async fn only_a_member_of_the_fleet_is_let_through() {
        let (bob, bk) = signed_entry("bob");
        let (eve, ek) = signed_entry("bob");
        let list = crate::members::Members::listing(&[&bob]);
        let r = rig_with("members", |c| {
            c.members = Some(crate::members::Members::parse(&list).unwrap())
        });
        // ann is in this home, but the release no longer lists her
        let (ann, ak, _) = person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;
        let sign = |e: &SignedEntry, k: &identity::SigningKey, n: &str| serde_json::json!({"name": e.entry.name, "entry": e, "device": pk(e), "sig": device_sig(k, NAME, n, &e.entry.name), "nonce": n});
        // someone taking bob's name with a root of their own
        let n = nonce(&url).await;
        assert_eq!(post(&url, sign(&eve, &ek, &n)).await.status(), 403);
        // bob himself
        let n = nonce(&url).await;
        assert_eq!(post(&url, sign(&bob, &bk, &n)).await.status(), 200);
        let n = nonce(&url).await;
        assert_eq!(post(&url, sign(&ann, &ak, &n)).await.status(), 403);
        // and a session she already had ends with the release that revoked her
        let (_, _, cookie) = person(&r.app, "ann");
        let res = client()
            .get(format!("{url}/_door/people"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
    }

    /// A member the release's list does not name - by a box of theirs, or
    /// an invite since - is let through while the gate says so for the
    /// root their entry has, and never past the list's own revocation
    #[tokio::test]
    async fn a_member_the_list_does_not_name_is_let_through_on_the_gates_word() {
        let (bob, _) = signed_entry("bob");
        let (cat, ck) = signed_entry("cat");
        let (eve, ek) = signed_entry("cat");
        let list = crate::members::Members::listing(&[&bob]);
        let r = rig_with("vouched", |c| {
            c.members = Some(crate::members::Members::parse(&list).unwrap())
        });
        let url = serve(crate::router(r.app.clone())).await;
        let sign = |e: &SignedEntry, k: &identity::SigningKey, n: &str| serde_json::json!({"name": e.entry.name, "entry": e, "device": pk(e), "sig": device_sig(k, NAME, n, &e.entry.name), "nonce": n});
        // the gate says nothing of her, or that she is no member
        let n = nonce(&url).await;
        assert_eq!(post(&url, sign(&cat, &ck, &n)).await.status(), 403);
        r.directory.says("cat", &cat.entry.root, false);
        let n = nonce(&url).await;
        assert_eq!(post(&url, sign(&cat, &ck, &n)).await.status(), 403);
        // a member, by the gate: in
        r.directory.says("cat", &cat.entry.root, true);
        let n = nonce(&url).await;
        assert_eq!(post(&url, sign(&cat, &ck, &n)).await.status(), 200);
        // the gate's word is for her root: another root under her name is not her
        let n = nonce(&url).await;
        assert_eq!(post(&url, sign(&eve, &ek, &n)).await.status(), 403);
        // a session lasts while the gate says so, and ends with its word at
        // the next refresh
        let (_, _, cookie) = person(&r.app, "cat");
        r.app.with_store(|s| {
            let p = s.state.people.get_mut("cat").unwrap();
            p.entry = cat.clone();
            p.root = cat.entry.root.clone();
        });
        let people = |c: &str| {
            client()
                .get(format!("{url}/_door/people"))
                .header("cookie", c.to_string())
                .send()
        };
        assert_ne!(people(&cookie).await.unwrap().status(), 401);
        r.directory.says("cat", &cat.entry.root, false);
        refresh(&r.app).await;
        assert_eq!(people(&cookie).await.unwrap().status(), 401);
        // someone the release revokes, whatever the gate says
        let mut gone: serde_json::Value = serde_json::from_str(&list).unwrap();
        gone["revoked"] = serde_json::json!([identity::member_id(&cat.entry.root)]);
        let m = crate::members::Members::parse(&gone.to_string()).unwrap();
        assert!(m.shut_out(&cat) && !m.shut_out(&bob));
        r.directory.says("cat", &cat.entry.root, true);
        refresh(&r.app).await;
        assert!(r.app.member(&cat));
        let r2 = rig_with("vouched-revoked", |c| c.members = Some(m));
        r2.directory.says("cat", &cat.entry.root, true);
        crate::signin::vouch(&r2.app, None, "cat").await;
        assert!(!r2.app.member(&cat), "revoked by the release");
        // a listed member is never asked about
        let asked = *r.directory.2.lock().unwrap();
        assert!(r.app.member_vouching(&bob).await);
        assert_eq!(*r.directory.2.lock().unwrap(), asked);
    }

    #[tokio::test]
    async fn someone_not_in_signs_in_as_not_in_and_the_directory_is_checked() {
        let r = rig("stranger");
        person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;
        let (bob, bk) = signed_entry("bob");

        // no entry offered and none in the directory: nobody
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"bob","device":pk(&bob),"sig":device_sig(&bk,NAME,&n,"bob"),"nonce":n});
        assert_eq!(post(&url, body.clone()).await.status(), 404);

        // the directory has him
        r.directory
            .0
            .lock()
            .unwrap()
            .insert("bob".into(), bob.clone());
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"bob","device":pk(&bob),"sig":device_sig(&bk,NAME,&n,"bob"),"nonce":n});
        let res = post(&url, body).await;
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = res.json().await.unwrap();
        assert_eq!(v["in"], false);

        // a forged one from the directory is nobody either
        let mut forged = bob.clone();
        forged.entry.updated = 99;
        r.directory.0.lock().unwrap().insert("bob".into(), forged);
        let n = nonce(&url).await;
        let body = serde_json::json!({"name":"bob","device":pk(&bob),"sig":device_sig(&bk,NAME,&n,"bob"),"nonce":n});
        assert_eq!(post(&url, body).await.status(), 403);
    }

    #[tokio::test]
    async fn the_directory_refresh_takes_only_what_accept_takes() {
        let r = rig("refresh");
        let (e, k, _) = person(&r.app, "ann");
        let mut v2 = e.entry.clone();
        v2.version = 2;
        let v2 = identity::sign(v2, &k).unwrap();
        let mut bad = v2.clone();
        bad.entry.version = 3;
        r.directory.0.lock().unwrap().insert("ann".into(), bad);
        refresh(&r.app).await;
        assert_eq!(
            r.app
                .with_store(|s| s.state.people["ann"].entry.entry.version),
            1
        );
        r.directory.0.lock().unwrap().insert("ann".into(), v2);
        refresh(&r.app).await;
        assert_eq!(
            r.app
                .with_store(|s| s.state.people["ann"].entry.entry.version),
            2
        );
    }

    #[tokio::test]
    async fn signing_out_ends_the_session() {
        let r = rig("signout");
        let (_, _, cookie) = person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;
        let people = |c: String| {
            let req = client()
                .get(format!("{url}/_door/people"))
                .header("cookie", c);
            async move { req.send().await.unwrap().status().as_u16() }
        };
        assert_eq!(people(cookie.clone()).await, 200);
        let res = client()
            .post(format!("{url}/_door/signout"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204);
        assert_eq!(people(cookie).await, 401);
    }
}
