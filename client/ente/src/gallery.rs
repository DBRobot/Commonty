//! One account's photos, read the way Ente's own apps read them: signed in
//! with its password, each album's key opened with the master key, each
//! file's key with its album's, and the file itself with its own. For the
//! demo's photos, which the box holds the password to and shows to anyone.

use anyhow::{Context, Result, anyhow};
use ente_core::{b64, crypto};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::{AuthFlow, AuthFlowUi, LoginParams};

/// A photo, and what it takes to open it.
#[derive(Clone)]
pub struct Photo {
    pub id: i64,
    pub title: String,
    /// when it was taken, microseconds since the epoch
    pub taken: i64,
    /// 0 an image, 1 a video, 2 a live photo: only an image opens whole
    pub kind: i64,
    key: Vec<u8>,
    thumbnail: String,
    file: String,
}

pub struct Gallery {
    origin: String,
    token: String,
    http: reqwest::Client,
    master: Zeroizing<Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Collection {
    id: i64,
    encrypted_key: String,
    key_decryption_nonce: Option<String>,
    #[serde(default)]
    is_deleted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attrs {
    encrypted_data: Option<String>,
    decryption_header: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    id: i64,
    encrypted_key: String,
    key_decryption_nonce: String,
    file: Attrs,
    thumbnail: Attrs,
    metadata: Attrs,
    #[serde(default)]
    is_deleted: bool,
    updation_time: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Diff {
    diff: Vec<File>,
    has_more: bool,
}

/// the login asks nothing of anyone: the demo has no second factor
struct NoUi;

impl AuthFlowUi for NoUi {
    fn read_email_otp(&mut self, _: &str, _: crate::OtpPurpose, _: bool) -> crate::Result<String> {
        Err(crate::Error::Decode(
            "this account asks for an email code".into(),
        ))
    }
    fn read_totp_code(&mut self, _: crate::TotpPurpose) -> crate::Result<String> {
        Err(crate::Error::Decode(
            "this account has a second factor".into(),
        ))
    }
    fn report_retryable_error(&mut self, message: &str) -> crate::Result<()> {
        Err(crate::Error::Decode(message.into()))
    }
    fn choose_second_factor(
        &mut self,
        _: &[crate::SecondFactorMethod],
    ) -> crate::Result<crate::SecondFactorMethod> {
        Err(crate::Error::Decode(
            "this account has a second factor".into(),
        ))
    }
    fn present_passkey_verification(&mut self, _: &str) -> crate::Result<()> {
        Err(crate::Error::Decode(
            "this account has a second factor".into(),
        ))
    }
    fn wait_for_passkey_verification(&mut self) -> crate::Result<()> {
        Err(crate::Error::Decode(
            "this account has a second factor".into(),
        ))
    }
    fn present_totp_secret(&mut self, _: &str, _: &str) -> crate::Result<()> {
        Ok(())
    }
}

fn open(sealed: &str, nonce: &str, key: &[u8]) -> Result<Vec<u8>> {
    Ok(crypto::secretbox::decrypt(
        &b64::decode(sealed)?,
        &crypto::Nonce::try_from_slice(&b64::decode(nonce)?)?,
        &crypto::Key::try_from_slice(key)?,
    )?)
}

impl Gallery {
    pub async fn sign_in(origin: &str, email: &str, password: &str) -> Result<Self> {
        let client = crate::client(origin)?;
        let mut ui = NoUi;
        let account = AuthFlow::new(&client, &mut ui)
            .login(LoginParams {
                email: email.to_string(),
                password: Zeroizing::new(password.to_string()),
            })
            .await?;
        Ok(Self {
            origin: origin.trim_end_matches('/').to_string(),
            // museum reads the token as url-safe base64, as Ente's apps send it
            token: b64::encode_url_safe(&account.secrets.token),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            master: Zeroizing::new(account.secrets.master_key.clone()),
        })
    }

    /// the session, as museum reads it: for an account that may use its
    /// admin api, the way to call that too
    pub fn token(&self) -> &str {
        &self.token
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        q: &[(&str, String)],
    ) -> Result<T> {
        let r = self
            .http
            .get(format!("{}{path}", self.origin))
            .header("X-Auth-Token", &self.token)
            .header("X-Client-Package", "org.commonty.client")
            .query(q)
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("{path}: {}", r.status()));
        }
        Ok(r.json().await?)
    }

    /// every photo in every album, newest first
    pub async fn photos(&self) -> Result<Vec<Photo>> {
        #[derive(Deserialize)]
        struct Collections {
            collections: Vec<Collection>,
        }
        let cs: Collections = self
            .get("/collections/v2", &[("sinceTime", "0".into())])
            .await?;
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for c in cs.collections.into_iter().filter(|c| !c.is_deleted) {
            let Some(nonce) = &c.key_decryption_nonce else {
                continue;
            };
            let Ok(ckey) = open(&c.encrypted_key, nonce, &self.master) else {
                continue;
            };
            let mut since = 0i64;
            loop {
                let d: Diff = self
                    .get(
                        "/collections/v2/diff",
                        &[
                            ("collectionID", c.id.to_string()),
                            ("sinceTime", since.to_string()),
                        ],
                    )
                    .await?;
                for f in &d.diff {
                    since = since.max(f.updation_time);
                    if f.is_deleted || !seen.insert(f.id) {
                        continue;
                    }
                    let Ok(key) = open(&f.encrypted_key, &f.key_decryption_nonce, &ckey) else {
                        continue;
                    };
                    let meta = f
                        .metadata
                        .encrypted_data
                        .as_deref()
                        .and_then(|data| {
                            let header = crypto::Header::try_from_slice(
                                &b64::decode(&f.metadata.decryption_header).ok()?,
                            )
                            .ok()?;
                            let plain = crypto::blob::decrypt(
                                &b64::decode(data).ok()?,
                                &header,
                                &crypto::Key::try_from_slice(&key).ok()?,
                            )
                            .ok()?;
                            serde_json::from_slice::<serde_json::Value>(&plain).ok()
                        })
                        .unwrap_or_default();
                    out.push(Photo {
                        id: f.id,
                        title: meta["title"].as_str().unwrap_or_default().to_string(),
                        taken: meta["creationTime"].as_i64().unwrap_or(f.updation_time),
                        kind: meta["fileType"].as_i64().unwrap_or(0),
                        key,
                        thumbnail: f.thumbnail.decryption_header.clone(),
                        file: f.file.decryption_header.clone(),
                    });
                }
                if !d.has_more || d.diff.is_empty() {
                    break;
                }
            }
        }
        out.sort_by_key(|p| std::cmp::Reverse(p.taken));
        Ok(out)
    }

    /// museum answers with the sealed bytes, by way of a redirect to storage
    async fn fetch(&self, path: String, header: &str, key: &[u8], cap: usize) -> Result<Vec<u8>> {
        let r = self
            .http
            .get(format!("{}{path}", self.origin))
            .header("X-Auth-Token", &self.token)
            .header("X-Client-Package", "org.commonty.client")
            .send()
            .await?
            .error_for_status()?;
        if r.content_length().is_some_and(|n| n as usize > cap) {
            return Err(anyhow!("bigger than {cap} bytes"));
        }
        let sealed = r.bytes().await?;
        if sealed.len() > cap {
            return Err(anyhow!("bigger than {cap} bytes"));
        }
        crypto::stream::decrypt_file_data(
            &sealed,
            &crypto::Header::try_from_slice(&b64::decode(header)?)?,
            &crypto::Key::try_from_slice(key)?,
        )
        .context("decrypting")
    }

    /// the small picture Ente keeps for a grid
    pub async fn thumbnail(&self, p: &Photo) -> Result<Vec<u8>> {
        self.fetch(
            format!("/files/preview/v2/{}", p.id),
            &p.thumbnail,
            &p.key,
            2 << 20,
        )
        .await
    }

    /// the image itself, for an image only, and up to `cap` bytes
    pub async fn original(&self, p: &Photo, cap: usize) -> Result<Vec<u8>> {
        if p.kind != 0 {
            return Err(anyhow!("not a still image"));
        }
        self.fetch(format!("/files/download/v2/{}", p.id), &p.file, &p.key, cap)
            .await
    }
}
