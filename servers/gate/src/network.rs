//! The network's door: an admitted device asks for a key to join the
//! fleet's own network, the gate asks Headscale for one in that member's
//! name, single use, minutes to live. The control server itself is behind
//! the same host; this only vouches for who is asking.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};

use std::collections::HashMap;
use std::sync::Mutex;

use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use p256::pkcs8::DecodePublicKey;

use crate::App;

/// how long a join key is good for: long enough to hand it over, no more
const TTL: Duration = Duration::from_secs(600);

pub struct Door {
    /// headscale's api, on this box
    api: String,
    /// what a device is told to join
    control_url: String,
    key: String,
    http: reqwest::Client,
    /// the boxes that may ask for a key of their own: the key each signs
    /// with (in its TPM) and the age recipient the answer is sealed to (its
    /// secrets key, also in its TPM)
    boxes: HashMap<String, BoxKeys>,
    /// age, with whatever plugin the recipients need on its path
    age: String,
    /// when each box last had a key, and the time it signed for it
    asked: Mutex<HashMap<String, (u64, u64)>>,
}

pub struct BoxKeys {
    signs: VerifyingKey,
    recipient: String,
}

/// how far a box's clock may be from this one's
const SKEW: u64 = 120;
/// one key a box a minute
const EVERY: u64 = 60;

pub fn from_env() -> Result<Option<Door>> {
    let Ok(api) = std::env::var("VERIFY_HEADSCALE_API") else {
        return Ok(None);
    };
    let key = std::fs::read_to_string(
        std::env::var("VERIFY_HEADSCALE_KEY_FILE").context("VERIFY_HEADSCALE_KEY_FILE")?,
    )
    .context("reading the headscale api key")?;
    Ok(Some(Door {
        api: api.trim_end_matches('/').to_string(),
        control_url: std::env::var("VERIFY_HEADSCALE_URL").context("VERIFY_HEADSCALE_URL")?,
        key: key.trim().to_string(),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?,
        boxes: match std::env::var("VERIFY_NETWORK_BOXES") {
            Ok(f) => boxes_from(&std::fs::read_to_string(&f).with_context(|| f.clone())?)?,
            Err(_) => HashMap::new(),
        },
        age: std::env::var("VERIFY_AGE").unwrap_or_else(|_| "age".into()),
        asked: Mutex::default(),
    }))
}

/// {"node2": {"key": "<base64 SPKI P-256>", "recipient": "age1..."}}
fn boxes_from(json: &str) -> Result<HashMap<String, BoxKeys>> {
    use base64::Engine;
    let v: HashMap<String, serde_json::Value> = serde_json::from_str(json)?;
    v.into_iter()
        .map(|(name, b)| {
            let der = base64::engine::general_purpose::STANDARD
                .decode(b["key"].as_str().context("key")?)
                .with_context(|| format!("{name}'s key"))?;
            let signs = VerifyingKey::from_public_key_der(&der)
                .map_err(|e| anyhow::anyhow!("{name}'s key: {e}"))?;
            let recipient = b["recipient"].as_str().context("recipient")?.to_string();
            Ok((name, BoxKeys { signs, recipient }))
        })
        .collect()
}

/// what a box signs (servers/system/control/cmd/dd-control, joinMessage)
fn join_message(name: &str, time: u64) -> Vec<u8> {
    format!("commonty box join v1\0{name}\0{time}").into_bytes()
}

/// a TPM signs plain r||s, a test's soft key DER: either will do
fn signature(b64: &str) -> Option<Signature> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64.trim_end_matches('='))
        .ok()?;
    Signature::from_slice(&raw)
        .or_else(|_| Signature::from_der(&raw))
        .ok()
}

impl Door {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let mut r = self
            .http
            .request(method, format!("{}{path}", self.api))
            .bearer_auth(&self.key);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await?;
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("headscale: {status} {text}");
        }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }

    /// the member's user in headscale, made on first ask
    async fn user_id(&self, name: &str) -> Result<u64> {
        let v = self
            .call(
                reqwest::Method::GET,
                &format!("/api/v1/user?name={name}"),
                None,
            )
            .await?;
        // matched by name here: the filter is not trusted to
        if let Some(u) = v["users"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|u| u["name"].as_str() == Some(name))
        {
            return id_of(u);
        }
        let v = self
            .call(
                reqwest::Method::POST,
                "/api/v1/user",
                Some(serde_json::json!({ "name": name })),
            )
            .await?;
        id_of(&v["user"])
    }

    /// Every person's machines' addresses on the network, in one ask of the
    /// control server rather than one a person.
    pub async fn addresses_by_owner(
        &self,
    ) -> Result<std::collections::HashMap<String, Vec<String>>> {
        let v = self
            .call(reqwest::Method::GET, "/api/v1/node", None)
            .await?;
        let mut out: std::collections::HashMap<String, Vec<String>> = Default::default();
        for n in v["nodes"].as_array().into_iter().flatten() {
            let Some(user) = n["user"]["name"].as_str() else {
                continue;
            };
            out.entry(user.to_string()).or_default().extend(
                n["ipAddresses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str().map(str::to_string)),
            );
        }
        Ok(out)
    }

    /// Take every device off the network whose owner is no longer a member.
    ///
    /// A node outlives everything that let it join: revoke the member, and
    /// their laptop stays on the network with whatever the policy lets it
    /// reach, for good. Boxes are left alone - they are tagged, and they
    /// joined on the boxes' key, not a person's.
    pub async fn reap(&self, member: impl Fn(&str) -> bool) -> Result<usize> {
        let v = self
            .call(reqwest::Method::GET, "/api/v1/node", None)
            .await?;
        let mut gone = 0;
        for n in v["nodes"].as_array().into_iter().flatten() {
            let tagged = ["tags", "forcedTags", "validTags"]
                .iter()
                .any(|k| n[*k].as_array().is_some_and(|a| !a.is_empty()));
            if tagged {
                continue;
            }
            let Some(user) = n["user"]["name"].as_str() else {
                continue;
            };
            if member(user) {
                continue;
            }
            let id = match &n["id"] {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(i) => i.to_string(),
                _ => continue,
            };
            self.call(reqwest::Method::DELETE, &format!("/api/v1/node/{id}"), None)
                .await?;
            eprintln!(
                "network: removed {} - {user} is not a member",
                n["givenName"]
                    .as_str()
                    .or(n["name"].as_str())
                    .unwrap_or(&id)
            );
            gone += 1;
        }
        Ok(gone)
    }

    /// one key, one device, ten minutes
    pub async fn join_key(&self, name: &str) -> Result<(String, u64)> {
        self.mint(name, &[]).await
    }

    async fn mint(&self, name: &str, tags: &[&str]) -> Result<(String, u64)> {
        let user = self.user_id(name).await?;
        let expires = SystemTime::now() + TTL;
        let stamp = humantime(expires);
        let v = self
            .call(
                reqwest::Method::POST,
                "/api/v1/preauthkey",
                Some(serde_json::json!({
                    "user": user,
                    "reusable": false,
                    "ephemeral": false,
                    "expiration": stamp,
                    "aclTags": tags
                })),
            )
            .await?;
        let key = v["preAuthKey"]["key"]
            .as_str()
            .context("headscale gave no key")?
            .to_string();
        Ok((
            key,
            expires
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ))
    }
}

/// A box's own key to the network, sealed to the box.
///
/// The box signs the time with the key in its TPM; the answer is a
/// single-use key, tagged a box, encrypted to the secrets key in that same
/// TPM. Whoever carries the answer (the front door, Cloudflare) holds
/// ciphertext only, and a copied request gets nothing a second time.
pub(crate) async fn box_key(
    door: &Door,
    name: &str,
    time: u64,
    sig: &str,
    now: u64,
) -> Result<String, (StatusCode, String)> {
    let keys = door
        .boxes
        .get(name)
        .ok_or((StatusCode::FORBIDDEN, "not a box of this fleet".into()))?;
    if time.abs_diff(now) > SKEW {
        return Err((StatusCode::FORBIDDEN, "the time is off".into()));
    }
    let sig = signature(sig).ok_or((StatusCode::BAD_REQUEST, "no signature".into()))?;
    keys.signs
        .verify(&join_message(name, time), &sig)
        .map_err(|_| (StatusCode::FORBIDDEN, "not signed by that box".into()))?;
    {
        // once a minute, and each signed time once: a request copied off
        // the wire is refused for good, not just for the minute
        let mut asked = door.asked.lock().unwrap();
        if let Some(&(at, signed)) = asked.get(name) {
            if now < at + EVERY {
                return Err((StatusCode::TOO_MANY_REQUESTS, "one a minute".into()));
            }
            if time <= signed {
                return Err((StatusCode::FORBIDDEN, "that request was used".into()));
            }
        }
        asked.insert(name.to_string(), (now, time));
    }
    let bad = |e: anyhow::Error| (StatusCode::BAD_GATEWAY, format!("{e:#}"));
    let (key, _) = door.mint("boxes", &["tag:box"]).await.map_err(bad)?;
    seal(&door.age, &keys.recipient, &key).map_err(bad)
}

/// armoured age, to one recipient
fn seal(age: &str, recipient: &str, plain: &str) -> Result<String> {
    use std::io::Write;
    let mut c = std::process::Command::new(age)
        .args(["-a", "-r", recipient])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("running age")?;
    c.stdin
        .take()
        .context("age's stdin")?
        .write_all(plain.as_bytes())?;
    let out = c.wait_with_output()?;
    if !out.status.success() {
        bail!("age: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// headscale gives ids as strings in json
fn id_of(u: &serde_json::Value) -> Result<u64> {
    match &u["id"] {
        serde_json::Value::String(s) => s.parse().context("user id"),
        serde_json::Value::Number(n) => n.as_u64().context("user id"),
        _ => bail!("headscale gave no user id"),
    }
}

/// RFC 3339, whole seconds, for a timestamp field
fn humantime(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn civil(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = secs / 86400;
    let rem = secs % 86400;
    // days since 1970-01-01 to civil date (Howard Hinnant's algorithm)
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y as u64,
        m as u64,
        d as u64,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
    )
}

/// POST /_dd/network/join, bearer device token -> { control_url, key, expires }
pub(crate) async fn join(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(door) = app.network.as_ref() else {
        return (StatusCode::NOT_FOUND, "this box has no network door").into_response();
    };
    let Some(token) = crate::bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "a device token is required").into_response();
    };
    let user = match app.verify_biscuit(token, "access") {
        Ok(u) => u,
        Err(e) => return (StatusCode::UNAUTHORIZED, e.to_string()).into_response(),
    };
    // a key on the fleet's network is not something signing up earns: the
    // network carries boxes, and every box trusts the interface it arrives
    // on. Membership is the release's word, and the demo never leaves the
    // browser it was made in.
    // A guest joins too: their devices come under the same rule as a
    // member's (the web port, which is public anyway, and the game ports,
    // which the games box opens per server to the players invited).
    if !(app.member(&user) || app.guest(&user)) || user == crate::pages::DEMO_USER {
        return (StatusCode::FORBIDDEN, "not a member of this fleet").into_response();
    }
    match door.join_key(&user).await {
        Ok((key, expires)) => Json(serde_json::json!({
            "control_url": door.control_url,
            "key": key,
            "expires": expires,
            "user": user
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

/// POST /_dd/network/controller, bearer device token -> { control_url, key, expires }
///
/// A household's home controller's key to the network (docs/dev/home-
/// controller.md), asked for by the member pairing it, from the app, and
/// carried to the Pi over the house network. Single use, tagged a
/// controller and made in the boxes' user, so the reaper leaves it and the
/// policy lets it reach the boxes' front doors and nothing else; members'
/// devices reach it on 443 alone. A member's, never a guest's.
pub(crate) async fn controller_join(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(door) = app.network.as_ref() else {
        return (StatusCode::NOT_FOUND, "this box has no network door").into_response();
    };
    let Some(token) = crate::bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "a device token is required").into_response();
    };
    let user = match app.verify_biscuit(token, "access") {
        Ok(u) => u,
        Err(e) => return (StatusCode::UNAUTHORIZED, e.to_string()).into_response(),
    };
    if !app.member(&user) || user == crate::pages::DEMO_USER {
        return (
            StatusCode::FORBIDDEN,
            "only a member sets up a home controller",
        )
            .into_response();
    }
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut asked = door.asked.lock().unwrap();
        let at = format!("controller:{user}");
        if asked.get(&at).is_some_and(|&(t, _)| now < t + EVERY) {
            return (StatusCode::TOO_MANY_REQUESTS, "one a minute").into_response();
        }
        asked.insert(at, (now, 0));
    }
    match door.mint("boxes", &["tag:controller"]).await {
        Ok((key, expires)) => Json(serde_json::json!({
            "control_url": door.control_url,
            "key": key,
            "expires": expires
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct BoxAsk {
    #[serde(rename = "box")]
    name: String,
    time: u64,
    sig: String,
}

/// POST /_dd/network/box, {box, time, sig} -> the key, sealed to that box
pub(crate) async fn box_join(State(app): State<Arc<App>>, Json(ask): Json<BoxAsk>) -> Response {
    let Some(door) = app.network.as_ref() else {
        return (StatusCode::NOT_FOUND, "this box has no network door").into_response();
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match box_key(door, &ask.name, ask.time, &ask.sig, now).await {
        Ok(sealed) => sealed.into_response(),
        Err((code, why)) => (code, why).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake headscale with three nodes: a box, a member's laptop and a
    /// stranger's phone. Only the phone may go.
    #[tokio::test]
    async fn only_a_non_members_devices_come_off() {
        use axum::{
            Json, Router,
            extract::{Path, State},
            routing::{delete, get},
        };
        use std::sync::{Arc, Mutex};

        let deleted: Arc<Mutex<Vec<String>>> = Arc::default();
        let nodes = serde_json::json!({ "nodes": [
            { "id": "1", "givenName": "node1", "user": { "name": "tagged-devices" }, "tags": ["tag:box"] },
            { "id": "2", "givenName": "laptop", "user": { "name": "david" }, "tags": [] },
            { "id": "3", "givenName": "phone", "user": { "name": "stranger" } },
        ]});
        let app = Router::new()
            .route(
                "/api/v1/node",
                get(move || async move { Json(nodes.clone()) }),
            )
            .route(
                "/api/v1/node/{id}",
                delete(
                    |State(d): State<Arc<Mutex<Vec<String>>>>, Path(id): Path<String>| async move {
                        d.lock().unwrap().push(id);
                        Json(serde_json::json!({}))
                    },
                ),
            )
            .with_state(deleted.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let door = Door {
            api: format!("http://{addr}"),
            control_url: String::new(),
            key: "k".into(),
            http: reqwest::Client::new(),
            boxes: HashMap::new(),
            age: "age".into(),
            asked: Mutex::default(),
        };
        let gone = door.reap(|u| u == "david").await.unwrap();
        assert_eq!(gone, 1);
        assert_eq!(*deleted.lock().unwrap(), vec!["3".to_string()]);
    }

    /// A box's key: only for a box of the fleet, signed by that box, now,
    /// and once a minute; what comes back went through age to the box.
    #[tokio::test]
    async fn a_box_gets_a_key_sealed_to_it() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        use base64::Engine;
        use p256::ecdsa::{SigningKey, signature::Signer};

        let app = Router::new()
            .route(
                "/api/v1/user",
                get(|| async {
                    Json(serde_json::json!({ "users": [{ "id": "7", "name": "boxes" }] }))
                }),
            )
            .route(
                "/api/v1/preauthkey",
                post(|Json(b): Json<serde_json::Value>| async move {
                    assert_eq!(b["aclTags"], serde_json::json!(["tag:box"]));
                    assert_eq!(b["reusable"], false);
                    Json(serde_json::json!({ "preAuthKey": { "key": "the-key" } }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // age that only marks what it was given, and to whom
        let dir = std::env::temp_dir().join(format!("box-join-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let age = dir.join("age");
        std::fs::write(&age, "#!/bin/sh\nprintf 'sealed to %s: ' \"$3\"; cat\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&age, std::fs::Permissions::from_mode(0o755)).unwrap();

        let node2 = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let stranger = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let mut boxes = HashMap::new();
        boxes.insert(
            "node2".to_string(),
            BoxKeys {
                signs: *node2.verifying_key(),
                recipient: "age1node2".into(),
            },
        );
        let door = Door {
            api: format!("http://{addr}"),
            control_url: String::new(),
            key: "k".into(),
            http: reqwest::Client::new(),
            boxes,
            age: age.display().to_string(),
            asked: Mutex::default(),
        };
        let sign = |k: &SigningKey, name: &str, t: u64| {
            let s: Signature = k.sign(&join_message(name, t));
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.to_bytes())
        };
        let now = 1_790_000_000;
        let refused = |r: Result<String, (StatusCode, String)>| r.unwrap_err().0;

        assert_eq!(
            refused(box_key(&door, "node9", now, &sign(&node2, "node9", now), now).await),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            refused(box_key(&door, "node2", now, &sign(&stranger, "node2", now), now).await),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            refused(
                box_key(
                    &door,
                    "node2",
                    now - 600,
                    &sign(&node2, "node2", now - 600),
                    now
                )
                .await
            ),
            StatusCode::FORBIDDEN
        );
        // signed for another time than it claims
        assert_eq!(
            refused(box_key(&door, "node2", now, &sign(&node2, "node2", now - 1), now).await),
            StatusCode::FORBIDDEN
        );

        let sealed = box_key(&door, "node2", now, &sign(&node2, "node2", now), now)
            .await
            .unwrap();
        assert_eq!(sealed, "sealed to age1node2: the-key");
        assert_eq!(
            refused(
                box_key(
                    &door,
                    "node2",
                    now + 5,
                    &sign(&node2, "node2", now + 5),
                    now + 5
                )
                .await
            ),
            StatusCode::TOO_MANY_REQUESTS
        );
        // the same request a minute later still gets nothing
        assert_eq!(
            refused(box_key(&door, "node2", now, &sign(&node2, "node2", now), now + 61).await),
            StatusCode::FORBIDDEN
        );
        // the DER a soft key writes is taken too
        let der: Signature = node2.sign(&join_message("node2", now + 61));
        let der = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(der.to_der().as_bytes());
        assert!(
            box_key(&door, "node2", now + 61, &der, now + 61)
                .await
                .is_ok()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stamps() {
        assert_eq!(humantime(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            humantime(UNIX_EPOCH + Duration::from_secs(1_790_000_000)),
            "2026-09-21T14:13:20Z"
        );
    }
}
