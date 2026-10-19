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
    /// the albums it is in
    pub albums: Vec<i64>,
    /// its stored size, sealed, when museum says
    pub size: Option<u64>,
    key: Vec<u8>,
    thumbnail: String,
    file: String,
}

pub struct Gallery {
    origin: String,
    token: String,
    http: reqwest::Client,
    master: Zeroizing<Vec<u8>>,
    /// where a presigned address to the store goes instead, as
    /// `{via}/{host}{path}?{query}`: a page that may only ask its own origin
    store_via: Option<String>,
}

/// An album, by the name its owner gave it.
#[derive(Clone)]
pub struct Album {
    pub id: i64,
    pub name: String,
    /// Ente's kind of collection: "album", "folder", "favorites", or
    /// "uncategorized" for what is in no album
    pub kind: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Collection {
    id: i64,
    encrypted_key: String,
    key_decryption_nonce: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    encrypted_name: Option<String>,
    #[serde(default)]
    name_decryption_nonce: Option<String>,
    #[serde(default)]
    is_deleted: bool,
    #[serde(default, rename = "type")]
    kind: String,
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
    #[serde(default)]
    info: Option<Info>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    #[serde(default)]
    file_size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Diff {
    diff: Vec<File>,
    has_more: bool,
}

/// the login asks nothing of anyone: the demo has no second factor, nor
/// does an account the fleet made
pub struct NoUi;

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
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn sign_in(origin: &str, email: &str, password: &str) -> Result<Self> {
        Self::sign_in_with(
            origin,
            origin,
            email,
            password,
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
        )
        .await
    }

    /// The same, for a caller that reaches museum its own way: the sign-in
    /// through `login` (an address that leads to museum), and everything
    /// after with `http`, at `origin`
    pub async fn sign_in_with(
        login: &str,
        origin: &str,
        email: &str,
        password: &str,
        http: reqwest::Client,
    ) -> Result<Self> {
        let client = crate::client(login)?;
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
            http,
            master: Zeroizing::new(account.secrets.master_key.clone()),
            store_via: None,
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

    /// every album, and every photo in them, newest first
    pub async fn photos(&self) -> Result<(Vec<Album>, Vec<Photo>)> {
        #[derive(Deserialize)]
        struct Collections {
            collections: Vec<Collection>,
        }
        let cs: Collections = self
            .get("/collections/v2", &[("sinceTime", "0".into())])
            .await?;
        let mut out: Vec<Photo> = Vec::new();
        let mut albums = Vec::new();
        let mut seen: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        for c in cs.collections.into_iter().filter(|c| !c.is_deleted) {
            let Some(nonce) = &c.key_decryption_nonce else {
                continue;
            };
            let Ok(ckey) = open(&c.encrypted_key, nonce, &self.master) else {
                continue;
            };
            // the name is sealed with the album's own key, or plain on an
            // album from before names were
            let name = match (&c.encrypted_name, &c.name_decryption_nonce) {
                (Some(n), Some(nn)) => open(n, nn, &ckey)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default(),
                _ => c.name.clone().unwrap_or_default(),
            };
            albums.push(Album {
                id: c.id,
                name,
                kind: c.kind.clone(),
            });
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
                    if f.is_deleted {
                        continue;
                    }
                    // in another album already: one more album for it
                    if let Some(&i) = seen.get(&f.id) {
                        out[i].albums.push(c.id);
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
                    seen.insert(f.id, out.len());
                    out.push(Photo {
                        albums: vec![c.id],
                        size: f.info.as_ref().and_then(|i| i.file_size),
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
        // an album with nothing in it, or only a name nobody can read, is
        // not one to show
        albums.retain(|a| !a.name.is_empty() && out.iter().any(|p| p.albums.contains(&a.id)));
        Ok((albums, out))
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

    /// Any original - an image, a video, a live photo (Ente keeps that as a
    /// zip of its two halves) - opened as it comes, a piece at a time
    pub async fn original_stream(&self, p: &Photo) -> Result<Original> {
        let resp = self
            .http
            .get(format!("{}/files/download/v2/{}", self.origin, p.id))
            .header("X-Auth-Token", &self.token)
            .header("X-Client-Package", "org.commonty.client")
            .send()
            .await?
            .error_for_status()?;
        Ok(Original {
            resp: Some(resp),
            buf: Vec::new(),
            dec: crypto::stream::Decryptor::new(
                &crypto::Header::try_from_slice(&b64::decode(&p.file)?)?,
                &crypto::Key::try_from_slice(&p.key)?,
            ),
            done: false,
        })
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

/// The kind of a file, as Ente's metadata says it
pub const IMAGE: i64 = 0;
pub const VIDEO: i64 = 1;

/// An album's id and key, owned by this account
struct Owned {
    id: i64,
    name: String,
    kind: String,
    key: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadUrl {
    object_key: String,
    url: String,
}

fn sealed_with(data: &[u8], key: &[u8]) -> Result<(String, String)> {
    let b = crypto::secretbox::encrypt(data, &crypto::Key::try_from_slice(key)?);
    Ok((
        b64::encode(&b.encrypted_data),
        b64::encode(b.nonce.as_bytes()),
    ))
}

/// Ente's metadata and magic metadata: json sealed as one final message
fn blob_json(v: &serde_json::Value, key: &[u8]) -> Result<(String, String)> {
    let b = crypto::blob::encrypt(&serde_json::to_vec(v)?, &crypto::Key::try_from_slice(key)?)?;
    Ok((
        b64::encode(&b.encrypted_data),
        b64::encode(b.decryption_header.as_bytes()),
    ))
}

/// the file sealed as Ente's clients seal it: 4 MiB messages, the last one final
fn stream_sealed(data: &[u8], key: &crypto::Key) -> Result<(Vec<u8>, String)> {
    let mut out = Vec::with_capacity(crypto::stream::estimate_encrypted_size(data.len()));
    let header = crypto::stream::encrypt_file(&mut std::io::Cursor::new(data), &mut out, key)?;
    Ok((out, b64::encode(header.as_bytes())))
}

/// base58 as Bitcoin writes it, which a share link carries its album's key in
pub fn base58(bytes: &[u8]) -> String {
    const A: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut digits: Vec<u8> = Vec::new();
    for &b in bytes {
        let mut carry = b as u32;
        for d in digits.iter_mut() {
            carry += (*d as u32) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let zeros = bytes.iter().take_while(|&&b| b == 0).count();
    std::iter::repeat_n('1', zeros)
        .chain(digits.iter().rev().map(|&d| A[d as usize] as char))
        .collect()
}

impl Gallery {
    /// the same account, with each presigned address to the store sent by
    /// way of `via` (see `store_via`)
    pub fn with_store_via(mut self, via: &str) -> Self {
        self.store_via = Some(via.trim_end_matches('/').to_string());
        self
    }

    fn store_url(&self, url: &str) -> String {
        match (&self.store_via, url.strip_prefix("https://")) {
            (Some(via), Some(rest)) => format!("{via}/{rest}"),
            _ => url.to_string(),
        }
    }

    async fn send(&self, path: &str, body: &serde_json::Value) -> Result<reqwest::Response> {
        let r = self
            .http
            .post(format!("{}{path}", self.origin))
            .header("X-Auth-Token", &self.token)
            .header("X-Client-Package", "org.commonty.client")
            .json(body)
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(anyhow!("{path}: {}", r.status()));
        }
        Ok(r)
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        Ok(self.send(path, body).await?.json().await?)
    }

    async fn owned(&self) -> Result<Vec<Owned>> {
        #[derive(Deserialize)]
        struct Collections {
            collections: Vec<Collection>,
        }
        let cs: Collections = self
            .get("/collections/v2", &[("sinceTime", "0".into())])
            .await?;
        Ok(cs
            .collections
            .into_iter()
            .filter(|c| !c.is_deleted)
            .filter_map(|c| {
                let key = open(
                    &c.encrypted_key,
                    c.key_decryption_nonce.as_ref()?,
                    &self.master,
                )
                .ok()?;
                let name = match (&c.encrypted_name, &c.name_decryption_nonce) {
                    (Some(n), Some(nn)) => open(n, nn, &key)
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .unwrap_or_default(),
                    _ => c.name.clone().unwrap_or_default(),
                };
                Some(Owned {
                    id: c.id,
                    name,
                    kind: c.kind,
                    key,
                })
            })
            .collect())
    }

    /// a new album of this account's, made as Ente's web app makes one;
    /// `magic` its private metadata (a quick link's kind), if any
    async fn new_album(&self, name: &str, magic: Option<serde_json::Value>) -> Result<Owned> {
        let key = crypto::Key::generate();
        let (encrypted_key, key_nonce) = sealed_with(key.as_bytes(), &self.master)?;
        let (encrypted_name, name_nonce) = sealed_with(name.as_bytes(), key.as_bytes())?;
        let mut body = serde_json::json!({
            "encryptedKey": encrypted_key,
            "keyDecryptionNonce": key_nonce,
            "encryptedName": encrypted_name,
            "nameDecryptionNonce": name_nonce,
            "type": "album",
        });
        if let Some(m) = magic {
            let count = m.as_object().map_or(0, |o| o.len());
            let (data, header) = blob_json(&m, key.as_bytes())?;
            body["magicMetadata"] =
                serde_json::json!({ "version": 1, "count": count, "data": data, "header": header });
        }
        #[derive(Deserialize)]
        struct Made {
            collection: Collection,
        }
        let made: Made = self.post("/collections", &body).await?;
        Ok(Owned {
            id: made.collection.id,
            name: name.to_string(),
            kind: "album".into(),
            key: key.as_bytes().to_vec(),
        })
    }

    /// One file put in the album called `album` (made if this account has
    /// none by that name), sealed as Ente's own apps seal one: its bytes
    /// with a key of its own, the small picture beside it, what it is in its
    /// metadata, and its key sealed with the album's. `taken` is in
    /// microseconds since the epoch. Its id comes back.
    pub async fn save(
        &self,
        album: &str,
        title: &str,
        kind: i64,
        taken: i64,
        data: &[u8],
        thumbnail: &[u8],
    ) -> Result<i64> {
        let owned = self.owned().await?;
        let into = match owned
            .into_iter()
            .find(|c| c.kind == "album" && c.name == album)
        {
            Some(c) => c,
            None => self.new_album(album, None).await?,
        };
        let key = crypto::Key::generate();
        let (file, file_header) = stream_sealed(data, &key)?;
        let thumb = crypto::blob::encrypt(thumbnail, &key)?;
        let meta = serde_json::json!({
            "fileType": kind,
            "title": title,
            "creationTime": taken,
            "modificationTime": taken,
            "hash": b64::encode(&crypto::hash::hash(data, Some(64), None)?),
        });
        let (meta_data, meta_header) = blob_json(&meta, key.as_bytes())?;
        #[derive(Deserialize)]
        struct Urls {
            urls: Vec<UploadUrl>,
        }
        let urls: Urls = self
            .get("/files/upload-urls", &[("count", "2".into())])
            .await?;
        let [f, t] = <[UploadUrl; 2]>::try_from(urls.urls)
            .map_err(|u| anyhow!("museum gave {} upload addresses, not 2", u.len()))?;
        for (u, body) in [(&f, file.clone()), (&t, thumb.encrypted_data.clone())] {
            let r = self
                .http
                .put(self.store_url(&u.url))
                .body(body)
                .send()
                .await?;
            if !r.status().is_success() {
                return Err(anyhow!("the store took no upload: {}", r.status()));
            }
        }
        let (encrypted_key, key_nonce) = sealed_with(key.as_bytes(), &into.key)?;
        #[derive(Deserialize)]
        struct Made {
            id: i64,
        }
        let made: Made = self
            .post(
                "/files",
                &serde_json::json!({
                    "collectionID": into.id,
                    "encryptedKey": encrypted_key,
                    "keyDecryptionNonce": key_nonce,
                    "file": { "objectKey": f.object_key, "decryptionHeader": file_header, "size": file.len() },
                    "thumbnail": {
                        "objectKey": t.object_key,
                        "decryptionHeader": b64::encode(thumb.decryption_header.as_bytes()),
                        "size": thumb.encrypted_data.len(),
                    },
                    "metadata": { "encryptedData": meta_data, "decryptionHeader": meta_header },
                }),
            )
            .await?;
        Ok(made.id)
    }

    /// A link anyone can open these photos at, as Ente's "share" makes one:
    /// a quick-link album with them in it, its public address, and the
    /// album's key after the `#`, which no server is sent
    pub async fn share(&self, name: &str, photos: &[Photo]) -> Result<String> {
        if photos.is_empty() {
            return Err(anyhow!("nothing to share"));
        }
        // subType 2 is a quick link, visibility 0 is shown
        let album = self
            .new_album(
                name,
                Some(serde_json::json!({ "subType": 2, "visibility": 0 })),
            )
            .await?;
        let files = photos
            .iter()
            .map(|p| {
                let (k, n) = sealed_with(&p.key, &album.key)?;
                Ok(serde_json::json!({ "id": p.id, "encryptedKey": k, "keyDecryptionNonce": n }))
            })
            .collect::<Result<Vec<_>>>()?;
        // it answers with no body
        self.send(
            "/collections/add-files",
            &serde_json::json!({ "collectionID": album.id, "files": files }),
        )
        .await?;
        #[derive(Deserialize)]
        struct Shared {
            result: Url,
        }
        #[derive(Deserialize)]
        struct Url {
            url: String,
        }
        let s: Shared = self
            .post(
                "/collections/share-url",
                &serde_json::json!({ "collectionID": album.id, "enableComment": true }),
            )
            .await?;
        Ok(format!("{}#{}", s.result.url, base58(&album.key)))
    }
}

impl Photo {
    /// a photo with no key, for a test of what is done with its name and
    /// albums
    #[doc(hidden)]
    pub fn for_test(id: i64, title: &str, kind: i64, albums: Vec<i64>) -> Photo {
        Photo {
            id,
            title: title.into(),
            taken: 0,
            kind,
            albums,
            size: None,
            key: Vec::new(),
            thumbnail: String::new(),
            file: String::new(),
        }
    }
}

/// An original as it downloads: `next` hands back its plain bytes a chunk
/// (4 MiB) at a time, and None at its end
pub struct Original {
    resp: Option<reqwest::Response>,
    buf: Vec<u8>,
    dec: crypto::stream::Decryptor,
    done: bool,
}

impl Original {
    /// the next bytes as they come; in a browser, where reqwest reads no
    /// body a piece at a time, all of it at once
    async fn more(&mut self) -> Result<Option<Vec<u8>>> {
        #[cfg(not(target_arch = "wasm32"))]
        let got = match self.resp.as_mut() {
            Some(r) => r.chunk().await?.map(|b| b.to_vec()),
            None => None,
        };
        #[cfg(target_arch = "wasm32")]
        let got = match self.resp.take() {
            Some(r) => Some(r.bytes().await?.to_vec()),
            None => None,
        };
        Ok(got)
    }

    pub async fn next(&mut self) -> Result<Option<Vec<u8>>> {
        const CHUNK: usize = crypto::stream::DECRYPTION_CHUNK_SIZE;
        loop {
            if self.done {
                return Ok(None);
            }
            if self.buf.len() >= CHUNK {
                let rest = self.buf.split_off(CHUNK);
                let chunk = std::mem::replace(&mut self.buf, rest);
                let (plain, last) = self.dec.pull(&chunk).context("decrypting")?;
                self.done = last;
                if last && !self.buf.is_empty() {
                    return Err(anyhow!("more after the end"));
                }
                return Ok(Some(plain));
            }
            match self.more().await? {
                Some(b) => self.buf.extend_from_slice(&b),
                None => {
                    if self.buf.is_empty() {
                        return Err(anyhow!("cut short"));
                    }
                    let chunk = std::mem::take(&mut self.buf);
                    let (plain, last) = self.dec.pull(&chunk).context("decrypting")?;
                    if !last {
                        return Err(anyhow!("cut short"));
                    }
                    self.done = true;
                    return Ok(Some(plain));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base58_reads_as_bitcoin_writes_it() {
        assert_eq!(base58(b""), "");
        assert_eq!(base58(&[0, 0, 1]), "112");
        assert_eq!(base58(b"hello world"), "StV1DL6CwTryKyV");
    }

    #[test]
    fn a_saved_file_opens_as_one_ente_made() {
        // past one 4 MiB message, so the last one is the final one
        let data: Vec<u8> = (0..(5 << 20)).map(|i| (i % 251) as u8).collect();
        let key = crypto::Key::generate();
        let (sealed, header) = stream_sealed(&data, &key).unwrap();
        let header = crypto::Header::try_from_slice(&b64::decode(&header).unwrap()).unwrap();
        assert_eq!(
            crypto::stream::decrypt_file_data(&sealed, &header, &key).unwrap(),
            data
        );
        let (meta, h) = blob_json(&serde_json::json!({"title": "a.jpg"}), key.as_bytes()).unwrap();
        let plain = crypto::blob::decrypt(
            &b64::decode(&meta).unwrap(),
            &crypto::Header::try_from_slice(&b64::decode(&h).unwrap()).unwrap(),
            &key,
        )
        .unwrap();
        assert_eq!(plain, br#"{"title":"a.jpg"}"#);
    }
}
