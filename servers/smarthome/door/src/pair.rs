//! Pairing, on the house network before the controller has a name: every
//! message carries an HMAC-SHA256 under the key the installer wrote to the
//! card (docs/dev/home-controller.md, Pairing).
//!
//! The mac is over `commonty controller pair v1\0` and the message without
//! its mac: serde_json of the message's struct, fields in the order below
//! (an entry in identity's canonical form). `mac` is base64.

use crate::{App, Fut, refuse};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hmac::Mac as _;
use identity::SignedEntry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub const LABEL: &[u8] = b"commonty controller pair v1\0";
/// K sealed to the controller's key, beside a /pair/restore
pub const BACKUP_KEY_HEADER: &str = "x-commonty-backup-key";
/// how far a /pair's `at` may be from the controller's clock
pub const SKEW: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pair {
    pub entry: SignedEntry,
    pub at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Paired {
    /// the controller's ed25519 public key, base64
    pub controller: String,
    /// 16 base32 characters, lowercase: the first label of the door's name
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finish {
    /// sealed to the controller's key (library::seal_to), base64url unpadded
    pub network_key: String,
    pub name_sig: String,
    /// the fleet's domain: the name is <label>.home.<domain>
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finished {
    pub name: String,
}

/// Where a pairing message's mac travels: HMAC-SHA256 with the pairing key
/// over LABEL and the body's exact bytes, base64. Over the bytes as sent,
/// never a message read and written again: any client in any language makes
/// the same mac.
pub const MAC_HEADER: &str = "x-commonty-mac";

fn hmac(key: &[u8], body: &[u8]) -> hmac::Hmac<sha2::Sha256> {
    let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("any key length");
    m.update(LABEL);
    m.update(body);
    m
}

pub fn mac(key: &[u8], body: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(hmac(key, body).finalize().into_bytes())
}

pub fn check_mac(key: &[u8], body: &[u8], mac: &str) -> bool {
    use base64::Engine as _;
    let Ok(m) = base64::engine::general_purpose::STANDARD.decode(mac.trim()) else {
        return false;
    };
    hmac(key, body).verify_slice(&m).is_ok()
}

/// A message as it goes on the wire: its bytes, and their mac.
pub fn sealed<T: Serialize>(key: &[u8], msg: &T) -> (Vec<u8>, String) {
    let body = serde_json::to_vec(msg).expect("serialisable");
    let m = mac(key, &body);
    (body, m)
}

/// The body, if its mac is the pairing key's, read as T.
#[allow(clippy::result_large_err)]
fn opened<T: serde::de::DeserializeOwned>(
    key: &[u8],
    headers: &axum::http::HeaderMap,
    body: &[u8],
) -> Result<T, Response> {
    let m = headers
        .get(MAC_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !check_mac(key, body, m) {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "the pairing key does not match",
        ));
    }
    serde_json::from_slice(body).map_err(|_| refuse(StatusCode::BAD_REQUEST, "not that message"))
}

fn reply<T: Serialize>(key: &[u8], msg: &T) -> Response {
    let (body, m) = sealed(key, msg);
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/json".to_string(),
            ),
            (axum::http::HeaderName::from_static(MAC_HEADER), m),
        ],
        body,
    )
        .into_response()
}

/// What the name step hands on: POST /api/controller at the DNS Worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registration {
    pub name: String,
    pub label: String,
    pub controller: String,
    pub entry: SignedEntry,
    pub name_sig: String,
}

/// What /pair/finish sets going: the network, then the name and its
/// certificate.
pub trait Hooks: Send + Sync {
    fn network<'a>(&'a self, key: &'a str) -> Fut<'a, ()>;
    fn register<'a>(&'a self, r: &'a Registration) -> Fut<'a, ()>;
}

/// The network key to a file the network's unit reads; the registration to
/// a file beside the state.
// TODO: register at the DNS Worker (POST /api/controller) and run ACME with
// its challenge endpoint once that is built; until then the registration
// waits in registration.json for whatever does.
pub struct FileHooks {
    pub network_key_file: PathBuf,
    pub registration_file: PathBuf,
}

impl Hooks for FileHooks {
    fn network<'a>(&'a self, key: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            crate::state::write_atomic(&self.network_key_file, key.as_bytes(), 0o600)
        })
    }
    fn register<'a>(&'a self, r: &'a Registration) -> Fut<'a, ()> {
        Box::pin(async move {
            crate::state::write_atomic(
                &self.registration_file,
                &serde_json::to_vec_pretty(r)?,
                0o600,
            )
        })
    }
}

// the label is the controller key's, so only this controller can claim it
fn new_label(controller: &str) -> String {
    identity::controller_label(controller).expect("the controller's own key")
}

fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 200
        && d.contains('.')
        && d.split('.').all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn pair_key(app: &App) -> Option<Vec<u8>> {
    if app.with_store(|s| s.state.finished) {
        return None;
    }
    std::fs::read(&app.cfg.pair_key)
        .ok()
        .filter(|k| k.len() >= 16)
}

pub async fn page(State(app): State<Arc<App>>) -> Response {
    app.assets.page_response("pair", StatusCode::OK)
}

/// For the pairing page: nothing secret, only how far along it is.
pub async fn status(State(app): State<Arc<App>>) -> Response {
    let (paired, finished) = app.with_store(|s| (s.state.paired.is_some(), s.state.finished));
    let keyed = pair_key(&app).is_some();
    Json(serde_json::json!({
        "pairing": keyed,
        // a card written without the app's key: the app adds one to it
        "needs_key": !keyed && !finished,
        "paired": paired,
        "finished": finished,
        "restore": app.restoring.lock().unwrap().clone(),
    }))
    .into_response()
}

#[allow(clippy::result_large_err)]
pub async fn pair(
    State(app): State<Arc<App>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(key) = pair_key(&app) else {
        return refuse(StatusCode::NOT_FOUND, "this controller is not pairing");
    };
    let m: Pair = match opened(&key, &headers, &body) {
        Ok(m) => m,
        Err(r) => return r,
    };
    if m.at.abs_diff(identity::now()) > SKEW {
        return refuse(StatusCode::FORBIDDEN, "the clocks disagree; try again");
    }
    let entry = m.entry;
    // someone the release's list does not name (a member by a box, or by an
    // invite since) is asked about at /pair/finish, when this door has its
    // name to ask the gate with; until then they are paired and nothing
    // more: no sign-in lets them in (App::member)
    if app
        .cfg
        .members
        .as_ref()
        .is_some_and(|m| !m.has(&entry) && m.shut_out(&entry))
    {
        return refuse(StatusCode::FORBIDDEN, "not a member of this fleet");
    }
    if identity::accept(None, &entry).is_err() {
        return refuse(
            StatusCode::FORBIDDEN,
            "that entry is not signed by its root",
        );
    }
    let controller = app.controller();
    let label = app.with_store(|s| -> Result<String, Response> {
        if s.state.finished {
            return Err(refuse(StatusCode::CONFLICT, "already paired"));
        }
        if let Some(label) = &s.state.label {
            // the same person again (the reply was lost): the same answer
            return match s.state.people.get(&entry.entry.name) {
                Some(p) if p.root == entry.entry.root => Ok(label.clone()),
                _ => Err(refuse(
                    StatusCode::CONFLICT,
                    "already paired with someone else",
                )),
            };
        }
        let label = new_label(&controller);
        s.state.label = Some(label.clone());
        s.state.paired = Some(identity::now());
        s.state.pairer = Some(entry.entry.name.clone());
        s.state.people.insert(
            entry.entry.name.clone(),
            crate::state::Person {
                root: entry.entry.root.clone(),
                entry: entry.clone(),
                ha_user: None,
                address: None,
                since: identity::now(),
            },
        );
        s.save().map_err(|e| {
            eprintln!("pair: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?;
        Ok(label)
    });
    let label = match label {
        Ok(l) => l,
        Err(r) => return r,
    };
    eprintln!("pair: paired with {}", entry.entry.name);
    app.reconcile.notify_one();
    reply(&key, &Paired { controller, label })
}

pub async fn finish(
    State(app): State<Arc<App>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(key) = pair_key(&app) else {
        return refuse(StatusCode::NOT_FOUND, "this controller is not pairing");
    };
    let f: Finish = match opened(&key, &headers, &body) {
        Ok(m) => m,
        Err(r) => return r,
    };
    if !valid_domain(&f.domain) {
        return refuse(StatusCode::BAD_REQUEST, "not a domain");
    }
    let Some((label, entry)) = app.with_store(|s| {
        let label = s.state.label.clone()?;
        let pairer = match &s.state.pairer {
            Some(n) => s.state.people.get(n)?,
            None => s.state.people.values().min_by_key(|p| p.since)?,
        };
        Some((label, pairer.entry.clone()))
    }) else {
        return refuse(StatusCode::CONFLICT, "pair first");
    };
    let controller = app.controller();
    let name = format!("{label}.home.{}", f.domain);
    if !check_name_sig(&entry.entry, &f.name_sig, &name, &controller, &f.domain) {
        return refuse(
            StatusCode::FORBIDDEN,
            "the name is not signed by the household's root",
        );
    }
    // sealed to this controller's key: the house network carries it, and
    // a mac keeps it whole but not secret
    let network_key = match library::open_bytes_with(&app.key, &f.network_key)
        .ok()
        .and_then(|k| String::from_utf8(k.to_vec()).ok())
    {
        Some(k) => k,
        None => {
            return refuse(
                StatusCode::BAD_REQUEST,
                "the network key is not sealed to this controller",
            );
        }
    };
    if let Err(e) = app.hooks.network(&network_key).await {
        eprintln!("pair: network key: {e:#}");
        return refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not keep the network key",
        );
    }
    let reg = Registration {
        name: name.clone(),
        label,
        controller,
        entry,
        name_sig: f.name_sig,
    };
    if !app.member(&reg.entry) {
        crate::signin::vouch(&app, Some(&reg), &reg.entry.entry.name).await;
        if !app.member(&reg.entry) {
            // not theirs to pair: the card pairs again from the start
            let _ = app.with_store(|s| {
                s.state.label = None;
                s.state.paired = None;
                s.state.pairer = None;
                s.state.people.clear();
                s.save()
            });
            return refuse(StatusCode::FORBIDDEN, "not a member of this fleet");
        }
    }
    if let Err(e) = app.hooks.register(&reg).await {
        eprintln!("pair: name: {e:#}");
        return refuse(StatusCode::BAD_GATEWAY, "could not register the name");
    }
    let saved = app.with_store(|s| {
        s.state.domain = Some(f.domain.clone());
        s.state.finished = true;
        s.save()
    });
    if let Err(e) = saved {
        eprintln!("pair: {e:#}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if let Err(e) = std::fs::remove_file(&app.cfg.pair_key)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("pair: removing the pairing key: {e}");
    }
    eprintln!("pair: finished as {name}");
    app.pairing_done.notify_waiters();
    // the home's first backup, now it has a name to go up under
    app.backup_now.notify_one();
    reply(&key, &Finished { name })
}

/// The household's root gave this controller this name: a device root's
/// signature, or a passkey root's assertion made on the gate's
/// /_dd/controller-name page (https://home.<domain>).
fn check_name_sig(
    entry: &identity::Entry,
    sig: &str,
    name: &str,
    controller: &str,
    domain: &str,
) -> bool {
    let origins = [format!("https://home.{domain}")];
    identity::controller_name_signed(entry, name, controller, sig, &origins).is_ok()
}

/// POST /pair/restore, after /pair and before /pair/finish: the body is a
/// backup file, macd as /pair's; its K comes sealed to this controller's
/// key in x-commonty-backup-key. Checked and opened here; Home Assistant's
/// part, which restarts it, goes on after the answer (/pair/status says
/// how it went).
pub async fn restore(
    State(app): State<Arc<App>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(key) = pair_key(&app) else {
        return refuse(StatusCode::NOT_FOUND, "this controller is not pairing");
    };
    let m = headers
        .get(MAC_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !check_mac(&key, &body, m) {
        return refuse(StatusCode::FORBIDDEN, "the pairing key does not match");
    }
    if app.with_store(|s| s.state.label.is_none()) {
        return refuse(StatusCode::CONFLICT, "pair first");
    }
    if matches!(&*app.restoring.lock().unwrap(), Some(r) if r.state == "working") {
        return refuse(StatusCode::CONFLICT, "a restore is going already");
    }
    let sealed = headers
        .get(BACKUP_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Ok(k) = library::open_with(&app.key, sealed.trim()) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "the backup key is not sealed to this controller",
        );
    };
    let opened = crate::backup::open(Vec::from(body), &k).and_then(crate::backup::read);
    let o = match opened {
        Ok(o) => o,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, &format!("{e:#}")),
    };
    *app.restoring.lock().unwrap() = Some(crate::backup::Restoring {
        state: "working",
        error: None,
    });
    eprintln!("pair: restoring Home Assistant's backup {}", o.ha_id);
    let a = app.clone();
    tokio::spawn(async move {
        let r = crate::backup::restore(&a, o, k).await;
        if let Err(e) = &r {
            eprintln!("pair: restore: {e:#}");
        }
        *a.restoring.lock().unwrap() = Some(crate::backup::Restoring {
            state: if r.is_ok() { "done" } else { "failed" },
            error: r.err().map(|e| format!("{e:#}")),
        });
    });
    reply(&key, &serde_json::json!({ "restoring": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;

    const KEY: [u8; 32] = [7; 32];

    fn setup(tag: &str) -> Rig {
        let r = rig_with(tag, |c| c.name = None);
        std::fs::write(&r.app.cfg.pair_key, KEY).unwrap();
        r
    }

    async fn send<T: Serialize>(url: &str, path: &str, msg: &T) -> reqwest::Response {
        send_with(url, path, &KEY, msg).await
    }

    async fn send_with<T: Serialize>(
        url: &str,
        path: &str,
        key: &[u8],
        msg: &T,
    ) -> reqwest::Response {
        let (body, m) = sealed(key, msg);
        client()
            .post(format!("{url}{path}"))
            .header("content-type", "application/json")
            .header(MAC_HEADER, m)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    /// a reply, its mac checked
    async fn read<T: serde::de::DeserializeOwned>(res: reqwest::Response) -> T {
        let m = res.headers()[MAC_HEADER].to_str().unwrap().to_string();
        let body = res.bytes().await.unwrap();
        assert!(check_mac(&KEY, &body, &m), "the reply is macd too");
        serde_json::from_slice(&body).unwrap()
    }

    fn sign_name(k: &identity::SigningKey, name: &str, controller: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(
            k.sign(&identity::controller_name_message(name, controller))
                .to_bytes(),
        )
    }

    #[test]
    fn the_mac_covers_every_byte_and_only_this_key() {
        let (e, _) = signed_entry("ann");
        let (body, m) = sealed(&KEY, &Pair { entry: e, at: 5 });
        assert!(check_mac(&KEY, &body, &m));
        assert!(!check_mac(&[8; 32], &body, &m));
        let mut other = body.clone();
        other.push(b' ');
        assert!(!check_mac(&KEY, &other, &m));
    }

    #[tokio::test]
    async fn a_message_not_macd_with_the_key_is_refused() {
        let r = setup("pairmac");
        let url = serve(crate::pair_router(r.app.clone())).await;
        let (e, _) = signed_entry("ann");
        let msg = Pair {
            entry: e,
            at: identity::now(),
        };
        assert_eq!(send_with(&url, "/pair", &[8; 32], &msg).await.status(), 403);
        let res = client()
            .post(format!("{url}/pair"))
            .json(&msg)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403);
        assert!(r.app.with_store(|s| s.state.people.is_empty()));
    }

    #[test]
    fn labels_are_sixteen_base32_characters_and_domains_are_names() {
        let k = identity::generate();
        let l = new_label(&identity::encode_public(&k.verifying_key()));
        assert_eq!(l.len(), 16);
        assert!(
            l.bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        );
        let other = identity::generate();
        assert_ne!(
            l,
            new_label(&identity::encode_public(&other.verifying_key()))
        );
        assert!(valid_domain("commonty.org"));
        for bad in ["", "org", "a..b", "-a.b", "A.org", "a.org/x", "a.org\0"] {
            assert!(!valid_domain(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn pairing_end_to_end() {
        let r = setup("pair");
        let url = serve(crate::pair_router(r.app.clone())).await;
        let (e, k) = signed_entry("ann");
        let res = send(
            &url,
            "/pair",
            &Pair {
                entry: e.clone(),
                at: identity::now(),
            },
        )
        .await;
        assert_eq!(res.status(), 200);
        let reply: Paired = read(res).await;
        assert_eq!(reply.controller, r.app.controller());
        assert!(r.app.with_store(|s| s.state.people.contains_key("ann")));

        // the reply lost, ann asks again: the same label
        let again: Paired = read(
            send(
                &url,
                "/pair",
                &Pair {
                    entry: e.clone(),
                    at: identity::now(),
                },
            )
            .await,
        )
        .await;
        assert_eq!(again, reply);
        // someone else, with the key: refused
        let (bob, _) = signed_entry("bob");
        let res = send(
            &url,
            "/pair",
            &Pair {
                entry: bob,
                at: identity::now(),
            },
        )
        .await;
        assert_eq!(res.status(), 409);

        let name = format!("{}.home.commonty.org", reply.label);
        let finish = Finish {
            network_key: library::seal_to(&reply.controller, b"hskey-auth-123").unwrap(),
            name_sig: sign_name(&k, &name, &reply.controller),
            domain: "commonty.org".into(),
        };
        // a name signed for another controller
        let wrong = Finish {
            name_sig: sign_name(&k, &name, "someone-else"),
            ..finish.clone()
        };
        assert_eq!(send(&url, "/pair/finish", &wrong).await.status(), 403);

        let res = send(&url, "/pair/finish", &finish).await;
        assert_eq!(res.status(), 200);
        let done: Finished = read(res).await;
        assert_eq!(done.name, name);
        assert_eq!(
            r.hooks.network.lock().unwrap().as_deref(),
            Some("hskey-auth-123")
        );
        let reg = r.hooks.registered.lock().unwrap().clone().unwrap();
        assert_eq!(reg.name, name);
        assert_eq!(reg.entry.entry.name, "ann");
        assert!(!r.app.cfg.pair_key.exists(), "the pairing key is gone");
        assert_eq!(r.app.name().as_deref(), Some(name.as_str()));

        // and pairing is over
        assert_eq!(send(&url, "/pair/finish", &finish).await.status(), 404);
    }

    /// Someone the release's list does not name pairs on the gate's word,
    /// asked at the finish with the name they signed; without it the card
    /// pairs again from the start. The list's revocation refuses at once.
    #[tokio::test]
    async fn a_member_the_list_does_not_name_pairs_on_the_gates_word() {
        let (bob, _) = signed_entry("bob");
        let list = crate::members::Members::listing(&[&bob]);
        let r = rig_with("pair-vouched", |c| {
            c.name = None;
            c.members = Some(crate::members::Members::parse(&list).unwrap());
        });
        std::fs::write(&r.app.cfg.pair_key, KEY).unwrap();
        let url = serve(crate::pair_router(r.app.clone())).await;
        let (cat, ck) = signed_entry("cat");
        let pair = || Pair {
            entry: cat.clone(),
            at: identity::now(),
        };
        let reply: Paired = read(send(&url, "/pair", &pair()).await).await;
        let name = format!("{}.home.commonty.org", reply.label);
        let finish = Finish {
            network_key: library::seal_to(&reply.controller, b"hskey-auth-123").unwrap(),
            name_sig: sign_name(&ck, &name, &reply.controller),
            domain: "commonty.org".into(),
        };
        // the gate says she is no member: refused, and nothing kept
        r.directory.says("cat", &cat.entry.root, false);
        assert_eq!(send(&url, "/pair/finish", &finish).await.status(), 403);
        assert!(
            r.app
                .with_store(|s| s.state.people.is_empty() && s.state.label.is_none())
        );
        assert!(r.hooks.registered.lock().unwrap().is_none());
        // a member by the gate's word: paired
        let reply: Paired = read(send(&url, "/pair", &pair()).await).await;
        let name = format!("{}.home.commonty.org", reply.label);
        let finish = Finish {
            name_sig: sign_name(&ck, &name, &reply.controller),
            ..finish
        };
        r.directory.says("cat", &cat.entry.root, true);
        assert_eq!(send(&url, "/pair/finish", &finish).await.status(), 200);
        assert_eq!(
            r.hooks
                .registered
                .lock()
                .unwrap()
                .as_ref()
                .map(|g| g.entry.entry.name.clone()),
            Some("cat".into())
        );

        // revoked by the release: refused at /pair
        let mut gone: serde_json::Value = serde_json::from_str(&list).unwrap();
        gone["revoked"] = serde_json::json!([identity::member_id(&cat.entry.root)]);
        let r = rig_with("pair-revoked", |c| {
            c.name = None;
            c.members = Some(crate::members::Members::parse(&gone.to_string()).unwrap());
        });
        std::fs::write(&r.app.cfg.pair_key, KEY).unwrap();
        let url = serve(crate::pair_router(r.app.clone())).await;
        r.directory.says("cat", &cat.entry.root, true);
        assert_eq!(send(&url, "/pair", &pair()).await.status(), 403);
    }

    #[tokio::test]
    async fn pairing_refuses_without_the_key_or_with_a_bad_entry() {
        let r = setup("pair-no");
        let url = serve(crate::pair_router(r.app.clone())).await;
        let (e, _) = signed_entry("ann");
        let body = |at| Pair {
            entry: e.clone(),
            at,
        };

        // the wrong key
        let res = send_with(&url, "/pair", &[1; 32], &body(identity::now())).await;
        assert_eq!(res.status(), 403);
        // a body changed after the mac
        let (mut bytes, m) = sealed(&KEY, &body(identity::now()));
        let at = bytes.iter().rposition(|b| b.is_ascii_digit()).unwrap();
        bytes[at] = if bytes[at] == b'9' {
            b'8'
        } else {
            bytes[at] + 1
        };
        let res = client()
            .post(format!("{url}/pair"))
            .header(MAC_HEADER, m)
            .body(bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403);
        // an old one, replayed
        assert_eq!(
            send(&url, "/pair", &body(identity::now() - 3600))
                .await
                .status(),
            403
        );
        // an entry not signed by its root
        let mut forged = e.clone();
        forged.entry.version = 9;
        let res = send(
            &url,
            "/pair",
            &Pair {
                entry: forged,
                at: identity::now(),
            },
        )
        .await;
        assert_eq!(res.status(), 403);
        // finish before pairing
        let f = Finish {
            network_key: "k".into(),
            name_sig: "s".into(),
            domain: "commonty.org".into(),
        };
        assert_eq!(send(&url, "/pair/finish", &f).await.status(), 409);
        assert!(r.app.with_store(|s| s.state.people.is_empty()));

        // no key on the card: not pairing at all
        std::fs::remove_file(&r.app.cfg.pair_key).unwrap();
        assert_eq!(
            send(&url, "/pair", &body(identity::now())).await.status(),
            404
        );
    }

    #[test]
    fn a_passkey_roots_name_signature_is_checked_here_too() {
        use crate::signin::tests::Authenticator;
        use sha2::Digest as _;
        let a = Authenticator::new("cred-1");
        let on_file = a.on_file();
        let (e, _) = signed_entry("ann");
        let mut entry = e.entry;
        entry.root = identity::passkey_root(&a.id, &on_file.cred);
        entry.passkeys = vec![on_file];
        let ctl = identity::encode_public(&identity::generate().verifying_key());
        let other = identity::encode_public(&identity::generate().verifying_key());
        let name = &format!("{}.home.commonty.org", new_label(&ctl));
        let said = |name: &str, origin: &str| {
            let challenge = sha2::Sha256::digest(identity::controller_name_message(name, &ctl));
            base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&a.assert(&challenge, origin)).unwrap())
        };
        let sig = said(name, "https://home.commonty.org");
        assert!(check_name_sig(&entry, &sig, name, &ctl, "commonty.org"));
        // another controller, another name, another page, another domain
        assert!(!check_name_sig(&entry, &sig, name, &other, "commonty.org"));
        assert!(!check_name_sig(
            &entry,
            &sig,
            "x.home.commonty.org",
            &ctl,
            "commonty.org"
        ));
        let elsewhere = said(name, "https://evil.example");
        assert!(!check_name_sig(
            &entry,
            &elsewhere,
            name,
            &ctl,
            "commonty.org"
        ));
        assert!(!check_name_sig(&entry, &sig, name, &ctl, "other.org"));
        // someone else's passkey
        let b = Authenticator::new("cred-1");
        let challenge = sha2::Sha256::digest(identity::controller_name_message(name, &ctl));
        let theirs = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&b.assert(&challenge, "https://home.commonty.org")).unwrap(),
        );
        assert!(!check_name_sig(&entry, &theirs, name, &ctl, "commonty.org"));
    }

    #[tokio::test]
    async fn a_card_without_a_key_says_it_needs_one() {
        let r = rig_with("pair-nokey", |c| c.name = None);
        let url = serve(crate::pair_router(r.app.clone())).await;
        let s: serde_json::Value = client()
            .get(format!("{url}/pair/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(s["pairing"], false);
        assert_eq!(s["needs_key"], true);
        std::fs::write(&r.app.cfg.pair_key, KEY).unwrap();
        let s: serde_json::Value = client()
            .get(format!("{url}/pair/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(s["pairing"], true);
        assert_eq!(s["needs_key"], false);
    }

    /// A backup of an earlier controller of ann's: ann and bob in, and
    /// Home Assistant's admin as it was.
    fn old_backup(ann: &SignedEntry, k: &[u8; 32]) -> Vec<u8> {
        use crate::backup;
        let mut st = crate::state::State::default();
        let (bob, _) = signed_entry("bob");
        for (e, n) in [(ann, 1), (&bob, 2)] {
            st.people.insert(
                e.entry.name.clone(),
                crate::state::Person {
                    root: e.entry.root.clone(),
                    entry: e.clone(),
                    ha_user: Some(format!("user{n}")),
                    address: Some(n),
                    since: 1,
                },
            );
        }
        st.ha.password = Some("old-password".into());
        st.label = Some("oldlabeloldlabel".into());
        let tar = backup::tar::write(
            &[
                (backup::HA_TAR, &backup::tests::ha_backup("old1")),
                (backup::STATE_JSON, &serde_json::to_vec(&st).unwrap()),
            ],
            1,
        );
        backup::seal(k, std::slice::from_ref(&ann.entry.root), tar, 1).unwrap()
    }

    async fn send_restore(
        url: &str,
        key: &[u8],
        body: Vec<u8>,
        sealed_k: &str,
    ) -> reqwest::Response {
        client()
            .post(format!("{url}/pair/restore"))
            .header(MAC_HEADER, mac(key, &body))
            .header(BACKUP_KEY_HEADER, sealed_k)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_backup_restored_at_pairing_brings_the_home_and_its_people_back() {
        let r = setup("restore");
        let url = serve(crate::pair_router(r.app.clone())).await;
        let (ann, _) = signed_entry("ann");
        let k = [9u8; 32];
        let file = old_backup(&ann, &k);
        let sealed = library::seal_to(&r.app.controller(), &k).unwrap();

        // not before /pair
        assert_eq!(
            send_restore(&url, &KEY, file.clone(), &sealed)
                .await
                .status(),
            409
        );
        let res = send(
            &url,
            "/pair",
            &Pair {
                entry: ann.clone(),
                at: identity::now(),
            },
        )
        .await;
        assert_eq!(res.status(), 200);

        let res = send_restore(&url, &KEY, file.clone(), &sealed).await;
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = read(res).await;
        assert_eq!(v["restoring"], true);
        for _ in 0..100 {
            if r.app
                .restoring
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|x| x.state != "working")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let done = r.app.restoring.lock().unwrap().clone().unwrap();
        assert_eq!(done.state, "done", "{done:?}");
        assert_eq!(*r.ha.restored.lock().unwrap(), vec!["old1".to_string()]);
        assert!(r.ha.restarts.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        r.app.with_store(|s| {
            assert!(s.state.people.contains_key("bob"));
            assert_eq!(s.state.people["ann"].ha_user.as_deref(), Some("user1"));
            assert_eq!(s.state.ha.password.as_deref(), Some("old-password"));
            assert_ne!(
                s.state.label.as_deref(),
                Some("oldlabeloldlabel"),
                "its own label"
            );
            assert_eq!(s.state.pairer.as_deref(), Some("ann"));
        });
        assert_eq!(**r.app.backup_key.lock().unwrap(), k, "K is its own now");
        assert_eq!(
            std::fs::read(r.app.cfg.state_dir.join(crate::backup::KEY_FILE)).unwrap(),
            k
        );
        let s: serde_json::Value = client()
            .get(format!("{url}/pair/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(s["restore"]["state"], "done");
    }

    #[tokio::test]
    async fn a_restore_not_macd_not_sealed_here_or_damaged_is_refused() {
        let r = setup("restore-no");
        let url = serve(crate::pair_router(r.app.clone())).await;
        let (ann, _) = signed_entry("ann");
        send(
            &url,
            "/pair",
            &Pair {
                entry: ann.clone(),
                at: identity::now(),
            },
        )
        .await;
        let k = [9u8; 32];
        let file = old_backup(&ann, &k);
        let sealed = library::seal_to(&r.app.controller(), &k).unwrap();
        // the wrong pairing key
        assert_eq!(
            send_restore(&url, &[1; 32], file.clone(), &sealed)
                .await
                .status(),
            403
        );
        // K sealed to another key, or no K
        let elsewhere = library::seal_to(&ann.entry.root, &k).unwrap();
        assert_eq!(
            send_restore(&url, &KEY, file.clone(), &elsewhere)
                .await
                .status(),
            400
        );
        assert_eq!(
            send_restore(&url, &KEY, file.clone(), "").await.status(),
            400
        );
        // another K
        let other = library::seal_to(&r.app.controller(), &[8; 32]).unwrap();
        assert_eq!(
            send_restore(&url, &KEY, file.clone(), &other)
                .await
                .status(),
            400
        );
        // a byte changed
        let mut bad = file.clone();
        let n = bad.len() - 10;
        bad[n] ^= 1;
        assert_eq!(send_restore(&url, &KEY, bad, &sealed).await.status(), 400);
        // a person in it whose entry is not theirs
        let mut forged = ann.clone();
        forged.entry.version = 7;
        assert_eq!(
            send_restore(&url, &KEY, old_backup(&forged, &k), &sealed)
                .await
                .status(),
            400
        );
        assert!(r.ha.restored.lock().unwrap().is_empty());
        assert!(r.app.with_store(|s| !s.state.people.contains_key("bob")));
        assert_ne!(**r.app.backup_key.lock().unwrap(), k);
    }
}
