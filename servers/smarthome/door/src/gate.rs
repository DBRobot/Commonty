//! The gate, for a controller (docs/dev/home-controller.md): the door asks
//! with its own key and its pairing person's word for it, in
//! `x-dd-controller`, for people's entries and for where to put a backup.

use crate::pair::Registration;
use crate::{Fut, backup, signin};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64_URL};
use ed25519_dalek::Signer as _;
use identity::{SignedEntry, SigningKey};
use std::path::PathBuf;

pub const HEADER: &str = "x-dd-controller";

/// `commonty controller request v1\0<METHOD>\0<path>\0<at>`
pub fn request_message(method: &str, path: &str, at: u64) -> Vec<u8> {
    format!("commonty controller request v1\0{method}\0{path}\0{at}").into_bytes()
}

/// base64url of {member, label, controller, name_sig, at, proof}; proof is
/// base64 ed25519 by the controller key over request_message.
pub fn header(reg: &Registration, key: &SigningKey, method: &str, path: &str, at: u64) -> String {
    let proof = B64.encode(key.sign(&request_message(method, path, at)).to_bytes());
    let v = serde_json::json!({
        "member": reg.entry.entry.name,
        "label": reg.label,
        "controller": reg.controller,
        "name_sig": reg.name_sig,
        "at": at,
        "proof": proof,
    });
    B64_URL.encode(v.to_string())
}

/// A name in a path: anything but the unreserved characters escaped.
fn segment(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub struct Gate {
    pub http: reqwest::Client,
    /// DOOR_GATE; else https://files.<domain> of the door's name
    pub base: Option<String>,
    pub registration: PathBuf,
    pub key: SigningKey,
}

impl Gate {
    fn registration(&self) -> Result<Registration> {
        let b = std::fs::read(&self.registration)
            .with_context(|| format!("{} (not paired yet)", self.registration.display()))?;
        serde_json::from_slice(&b).context("registration.json")
    }

    fn base(&self, reg: &Registration) -> Result<String> {
        if let Some(b) = &self.base {
            return Ok(b.trim_end_matches('/').to_string());
        }
        let d = signin::domain_of(&reg.name).context("the door's name has no domain")?;
        Ok(format!("https://files.{d}"))
    }

    fn request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder> {
        self.request_as(&self.registration()?, method, path)
    }

    fn request_as(
        &self,
        reg: &Registration,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder> {
        let h = header(reg, &self.key, method.as_str(), path, identity::now());
        Ok(self
            .http
            .request(method, format!("{}{path}", self.base(reg)?))
            .header(HEADER, h))
    }
}

impl signin::Directory for Gate {
    fn entry<'a>(&'a self, _domain: &'a str, name: &'a str) -> Fut<'a, Option<SignedEntry>> {
        Box::pin(async move {
            let path = format!("/_dd/controller/directory/{}", segment(name));
            let r = self
                .request(reqwest::Method::GET, &path)?
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await?;
            if r.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            Ok(Some(r.error_for_status()?.json().await?))
        })
    }

    fn standing<'a>(
        &'a self,
        reg: Option<&'a Registration>,
        name: &'a str,
    ) -> Fut<'a, Option<signin::Standing>> {
        Box::pin(async move {
            let path = format!("/_dd/controller/standing/{}", segment(name));
            let req = match reg {
                Some(r) => self.request_as(r, reqwest::Method::GET, &path)?,
                None => self.request(reqwest::Method::GET, &path)?,
            };
            let r = req
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await?;
            // no such name, or the asker no member: nobody the gate vouches for
            if matches!(
                r.status(),
                reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN
            ) {
                return Ok(Some(signin::Standing {
                    root: String::new(),
                    member: false,
                }));
            }
            Ok(Some(r.error_for_status()?.json().await?))
        })
    }
}

impl backup::Upload for Gate {
    fn put(&self, file: Vec<u8>) -> Fut<'_, ()> {
        Box::pin(async move {
            let r = self
                .request(reqwest::Method::POST, "/_dd/controller/backup")?
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
                .context("asking the gate where to put the backup")?;
            let status = r.status();
            if !status.is_success() {
                bail!(
                    "the gate said {status}: {}",
                    r.text().await.unwrap_or_default().trim()
                );
            }
            let v: serde_json::Value = r.json().await?;
            let put = v["put"].as_str().context("the gate gave no url")?;
            let r = self
                .http
                .put(put)
                .timeout(std::time::Duration::from_secs(3600))
                .body(file)
                .send()
                .await
                .context("sending the backup")?;
            if !r.status().is_success() {
                bail!("the backup store said {}", r.status());
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signin::Directory as _;
    use crate::tests::*;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post, put};
    use backup::Upload as _;
    use ed25519_dalek::Verifier as _;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Seen {
        entries: std::collections::HashMap<String, SignedEntry>,
        headers: Vec<(String, String, serde_json::Value)>,
        put: Vec<Vec<u8>>,
    }
    type Shared = Arc<Mutex<Seen>>;

    /// The gate's checks of the header, the ones the door can be held to.
    fn check(s: &Shared, h: &HeaderMap, method: &str, path: &str) -> bool {
        let Some(v) = h
            .get(HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| B64_URL.decode(v).ok())
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            return false;
        };
        let at = v["at"].as_u64().unwrap_or(0);
        let ok = identity::decode_public(v["controller"].as_str().unwrap_or(""))
            .ok()
            .zip(
                B64.decode(v["proof"].as_str().unwrap_or(""))
                    .ok()
                    .and_then(|p| ed25519_dalek::Signature::from_slice(&p).ok()),
            )
            .is_some_and(|(k, sig)| k.verify(&request_message(method, path, at), &sig).is_ok());
        s.lock()
            .unwrap()
            .headers
            .push((method.into(), path.into(), v));
        ok && at.abs_diff(identity::now()) < 300
    }

    async fn fake_gate() -> (String, Shared) {
        let s: Shared = Default::default();
        let r = axum::Router::new()
            .route(
                "/_dd/controller/directory/{name}",
                get(
                    |State(s): State<Shared>, Path(name): Path<String>, h: HeaderMap| async move {
                        let path = format!("/_dd/controller/directory/{}", segment(&name));
                        if !check(&s, &h, "GET", &path) {
                            return Err(StatusCode::FORBIDDEN);
                        }
                        let e = s.lock().unwrap().entries.get(&name).cloned();
                        e.map(axum::Json).ok_or(StatusCode::NOT_FOUND)
                    },
                ),
            )
            .route(
                "/_dd/controller/backup",
                post(|State(s): State<Shared>, h: HeaderMap| async move {
                    if !check(&s, &h, "POST", "/_dd/controller/backup") {
                        return Err(StatusCode::FORBIDDEN);
                    }
                    let host = h["host"].to_str().unwrap().to_string();
                    Ok(axum::Json(serde_json::json!({
                        "put": format!("http://{host}/bucket/put?sig=x"),
                        "get": format!("http://{host}/bucket/get?sig=x"),
                    })))
                }),
            )
            .route(
                "/bucket/put",
                put(
                    |State(s): State<Shared>, body: axum::body::Bytes| async move {
                        s.lock().unwrap().put.push(body.to_vec());
                        StatusCode::OK
                    },
                ),
            )
            .with_state(s.clone());
        (serve(r).await, s)
    }

    fn registration(dir: &std::path::Path, key: &SigningKey) -> Registration {
        let (e, _) = signed_entry("ann");
        let reg = Registration {
            name: "abcdefghijklmnop.home.commonty.org".into(),
            label: "abcdefghijklmnop".into(),
            controller: identity::encode_public(&key.verifying_key()),
            entry: e,
            name_sig: "c2ln".into(),
        };
        std::fs::write(
            dir.join("registration.json"),
            serde_json::to_vec(&reg).unwrap(),
        )
        .unwrap();
        reg
    }

    #[test]
    fn the_header_is_the_contracts() {
        let key = SigningKey::from_bytes(&[4; 32]);
        let dir = tempdir("gate-h");
        let reg = registration(&dir, &key);
        let h = header(&reg, &key, "POST", "/_dd/controller/backup", 77);
        let v: serde_json::Value = serde_json::from_slice(&B64_URL.decode(h).unwrap()).unwrap();
        assert_eq!(v["member"], "ann");
        assert_eq!(v["label"], "abcdefghijklmnop");
        assert_eq!(v["controller"], reg.controller);
        assert_eq!(v["name_sig"], "c2ln");
        assert_eq!(v["at"], 77);
        let sig = ed25519_dalek::Signature::from_slice(
            &B64.decode(v["proof"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let msg = b"commonty controller request v1\0POST\0/_dd/controller/backup\x0077";
        assert!(key.verifying_key().verify(msg, &sig).is_ok());
        assert_eq!(segment("a b/c"), "a%20b%2Fc");
    }

    #[tokio::test]
    async fn entries_and_backups_go_through_the_gate_with_the_header() {
        let key = SigningKey::from_bytes(&[4; 32]);
        let dir = tempdir("gate");
        let (url, seen) = fake_gate().await;
        let gate = Gate {
            http: client(),
            base: Some(url.clone()),
            registration: dir.join("registration.json"),
            key: key.clone(),
        };
        // not paired: nothing asked
        assert!(gate.entry("x", "bob").await.is_err());
        registration(&dir, &key);
        let (bob, _) = signed_entry("bob");
        seen.lock()
            .unwrap()
            .entries
            .insert("bob".into(), bob.clone());
        let got = gate.entry("x", "bob").await.unwrap().unwrap();
        assert_eq!(got.signature, bob.signature);
        assert!(gate.entry("x", "cy").await.unwrap().is_none());

        gate.put(b"CBK1sealed".to_vec()).await.unwrap();
        assert_eq!(seen.lock().unwrap().put, vec![b"CBK1sealed".to_vec()]);
        let h = seen.lock().unwrap().headers.clone();
        assert!(h.iter().all(|(_, _, v)| v["member"] == "ann"));

        // another controller's key in its place: the gate refuses
        let other = Gate {
            key: SigningKey::from_bytes(&[5; 32]),
            ..gate
        };
        assert!(other.entry("x", "bob").await.is_err());
        assert!(other.put(vec![1]).await.is_err());
        assert_eq!(seen.lock().unwrap().put.len(), 1);
    }

    #[test]
    fn the_gate_is_files_of_the_doors_domain_unless_told() {
        let key = SigningKey::from_bytes(&[4; 32]);
        let dir = tempdir("gate-base");
        let reg = registration(&dir, &key);
        let mut g = Gate {
            http: client(),
            base: None,
            registration: dir.join("registration.json"),
            key,
        };
        assert_eq!(g.base(&reg).unwrap(), "https://files.commonty.org");
        g.base = Some("http://127.0.0.1:9/".into());
        assert_eq!(g.base(&reg).unwrap(), "http://127.0.0.1:9");
    }
}
