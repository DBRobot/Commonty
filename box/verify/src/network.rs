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
}

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
    }))
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

    /// The machines this member has on the network, as the control
    /// server has them. Read only: a page shows them, nothing here takes
    /// one away.
    pub async fn mine(&self, name: &str) -> Result<Vec<serde_json::Value>> {
        let v = self
            .call(
                reqwest::Method::GET,
                &format!("/api/v1/node?user={name}"),
                None,
            )
            .await?;
        Ok(v["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            // headscale's filter is not trusted to: match the owner here
            .filter(|n| n["user"]["name"].as_str() == Some(name))
            .map(|n| {
                serde_json::json!({
                    "name": n["givenName"].as_str().or(n["name"].as_str()),
                    "addresses": n["ipAddresses"],
                    "lastSeen": n["lastSeen"],
                    "online": n["online"],
                    "os": n["hostinfo"]["OS"],
                })
            })
            .collect())
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
                    "expiration": stamp
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
        };
        let gone = door.reap(|u| u == "david").await.unwrap();
        assert_eq!(gone, 1);
        assert_eq!(*deleted.lock().unwrap(), vec!["3".to_string()]);
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
