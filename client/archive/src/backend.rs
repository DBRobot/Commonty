//! restic's repository layout over webdav, with a token that renews itself.
//!
//! Mirrors rustic_backend's REST backend: flat `<type>/<hex id>` paths plus
//! `config` at the root. The only things it adds are the bearer header on
//! every request and a retry on 401, which is what lets an upload outlive the
//! hour-long id token (token.rs).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use auth::KeyStore;
use bytes::Bytes;
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use rustic_core::{
    ALL_FILE_TYPES, BytesList, ErrorKind, FileType, Id, ReadBackend, RusticError, RusticResult,
    WriteBackend,
};
use url::Url;

use crate::token::TokenProvider;

pub struct Webdav<K: KeyStore> {
    base: Url,
    client: Client,
    tokens: Arc<TokenProvider<K>>,
}

impl<K: KeyStore + Send + Sync + 'static> Webdav<K> {
    pub fn new(base: Url, tokens: Arc<TokenProvider<K>>) -> Result<Self> {
        anyhow::ensure!(base.path().ends_with('/'), "repository url must end in /");
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(600)) // a pack over a slow uplink
            .build()?;
        Ok(Self {
            base,
            client,
            tokens,
        })
    }

    fn url(&self, tpe: FileType, id: &Id) -> Result<Url> {
        let path = if tpe == FileType::Config {
            "config".to_string()
        } else {
            // to_hex(), NOT Display: Display is the abbreviated 8-character
            // form, and a key file stored under that name is never found again
            format!("{}/{}", tpe.dirname(), id.to_hex().as_str())
        };
        Ok(self.base.join(&path)?)
    }

    /// Sends with a bearer token, retrying once on 401 with a fresh one and
    /// patiently on the failures a long upload will meet. Patient because a
    /// nixos-rebuild on the server restarts nginx and oauth2-proxy for a
    /// minute or two, and an hour of packs was once lost to a 500 in that
    /// window on the very last pack. Ten attempts, backoff capped at 30 s:
    /// about four minutes, which outlives a deploy.
    fn send(&self, build: impl Fn() -> RequestBuilder) -> Result<Response> {
        const ATTEMPTS: u32 = 10;
        let mut delay = Duration::from_secs(1);
        let mut refreshed = false;
        for attempt in 1..=ATTEMPTS {
            let token = self.tokens.token()?;
            match build().bearer_auth(&*token).send() {
                Ok(r) if r.status() == StatusCode::UNAUTHORIZED && !refreshed => {
                    self.tokens.invalidate();
                    refreshed = true;
                }
                Ok(r)
                    if (r.status().is_server_error()
                        || r.status() == StatusCode::TOO_MANY_REQUESTS)
                        && attempt < ATTEMPTS =>
                {
                    eprintln!(
                        "server answered {}, retrying in {}s",
                        r.status(),
                        delay.as_secs()
                    );
                }
                Ok(r) => return Ok(r),
                Err(e)
                    if attempt < ATTEMPTS
                        && (e.is_connect() || e.is_timeout() || e.is_request()) =>
                {
                    eprintln!("retrying in {}s after: {e}", delay.as_secs());
                }
                Err(e) => return Err(e.into()),
            }
            std::thread::sleep(delay);
            delay = (delay * 2).min(Duration::from_secs(30));
        }
        Err(anyhow!("gave up after {ATTEMPTS} attempts"))
    }

    fn ok(r: Response) -> Result<Response> {
        let status = r.status();
        if status.is_success() {
            Ok(r)
        } else {
            Err(anyhow!("{} {}", status, r.url()))
        }
    }
}

fn rustic(err: anyhow::Error, what: &str) -> Box<RusticError> {
    RusticError::with_source(ErrorKind::Backend, what.to_string(), err)
}

#[derive(serde::Deserialize)]
struct Listing {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    size: u32,
}

impl<K: KeyStore + Send + Sync + 'static> ReadBackend for Webdav<K> {
    fn location(&self) -> String {
        self.base.to_string()
    }

    fn list_with_size(&self, tpe: FileType) -> RusticResult<Vec<(Id, u32)>> {
        if tpe == FileType::Config {
            let url = self
                .base
                .join("config")
                .map_err(|e| rustic(e.into(), "url"))?;
            let r = self
                .send(|| self.client.head(url.clone()))
                .map_err(|e| rustic(e, "HEAD config"))?;
            return Ok(if r.status().is_success() {
                vec![(Id::default(), 0)]
            } else {
                Vec::new()
            });
        }
        let url = self
            .base
            .join(&format!("{}/", tpe.dirname()))
            .map_err(|e| rustic(e.into(), "url"))?;
        let r = self
            .send(|| self.client.get(url.clone()))
            .map_err(|e| rustic(e, "list"))?;
        // an empty repository has no type directories yet: nothing there is
        // the same as nothing listed
        if r.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let r = Self::ok(r).map_err(|e| rustic(e, "list"))?;
        // nginx autoindex_format json on the /images/ location - see
        // modules/webdav-media.nix. No xml, no PROPFIND parsing.
        let entries: Vec<Listing> = r.json().map_err(|e| rustic(e.into(), "list json"))?;
        Ok(entries
            .into_iter()
            .filter(|e| e.kind == "file")
            .filter_map(|e| Id::parse_some(&e.name, tpe).map(|id| (id, e.size)))
            .collect())
    }

    fn read_full(&self, tpe: FileType, id: &Id) -> RusticResult<Bytes> {
        let url = self.url(tpe, id).map_err(|e| rustic(e, "url"))?;
        let r = self
            .send(|| self.client.get(url.clone()))
            .and_then(Self::ok)
            .map_err(|e| rustic(e, "read"))?;
        r.bytes().map_err(|e| rustic(e.into(), "read body"))
    }

    fn read_partial(
        &self,
        tpe: FileType,
        id: &Id,
        _cacheable: bool,
        offset: u32,
        length: u32,
    ) -> RusticResult<Bytes> {
        let url = self.url(tpe, id).map_err(|e| rustic(e, "url"))?;
        let range = format!("bytes={}-{}", offset, offset + length - 1);
        let r = self
            .send(|| self.client.get(url.clone()).header("Range", range.clone()))
            .and_then(Self::ok)
            .map_err(|e| rustic(e, "read range"))?;
        r.bytes().map_err(|e| rustic(e.into(), "read body"))
    }

    fn warmup_path(&self, tpe: FileType, id: &Id) -> String {
        self.url(tpe, id).map(|u| u.to_string()).unwrap_or_default()
    }
}

impl<K: KeyStore + Send + Sync + 'static> WriteBackend for Webdav<K> {
    fn create(&self) -> RusticResult<()> {
        // MKCOL each type directory. 405 means it already exists, which is
        // fine - create is asked to converge, not to insist on emptiness.
        for tpe in ALL_FILE_TYPES {
            if tpe == FileType::Config {
                continue;
            }
            let url = self
                .base
                .join(&format!("{}/", tpe.dirname()))
                .map_err(|e| rustic(e.into(), "url"))?;
            let r = self
                .send(|| {
                    self.client
                        .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), url.clone())
                })
                .map_err(|e| rustic(e, "MKCOL"))?;
            if !(r.status().is_success() || r.status() == StatusCode::METHOD_NOT_ALLOWED) {
                return Err(rustic(
                    anyhow!("{} creating {}", r.status(), tpe.dirname()),
                    "MKCOL",
                ));
            }
        }
        Ok(())
    }

    fn write_bytes(
        &self,
        tpe: FileType,
        id: &Id,
        _cacheable: bool,
        content: BytesList,
    ) -> RusticResult<()> {
        let url = self.url(tpe, id).map_err(|e| rustic(e, "url"))?;
        // one body, known length: this is what lets nginx take it - it has no
        // streaming PUT, every request needs a Content-Length
        let body: Vec<u8> = content
            .slice()
            .iter()
            .flat_map(|b| b.iter().copied())
            .collect();
        self.send(|| self.client.put(url.clone()).body(body.clone()))
            .and_then(Self::ok)
            .map(|_| ())
            .map_err(|e| rustic(e, "write"))
    }

    fn remove(&self, tpe: FileType, id: &Id, _cacheable: bool) -> RusticResult<()> {
        let url = self.url(tpe, id).map_err(|e| rustic(e, "url"))?;
        self.send(|| self.client.delete(url.clone()))
            .and_then(Self::ok)
            .map(|_| ())
            .map_err(|e| rustic(e, "remove"))
    }
}
