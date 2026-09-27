//! A member's libraries from a device: the key out of the entry, the gate on
//! the box for one object at a time. `dd library`, `dd media` and the app
//! all come through here.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use auth::KeyStore;
use library::Library;

pub use account::{ROOT, USER, load_root};

/// what this machine can open a library with
pub struct Opener {
    device: (String, ed25519_dalek::SigningKey),
    /// the root key, on a machine that made the identity
    pub root: Option<ed25519_dalek::SigningKey>,
}

impl Opener {
    pub fn load(keys: &impl KeyStore) -> Result<(Self, String, String)> {
        let kp = auth::device::load(keys)?.context("no device key here - `dd device show`")?;
        let user = keys
            .get(USER)?
            .context("no name on this machine - `dd identity new` or `dd identity import`")?
            .to_string();
        let public = auth::device::public_b64(&kp);
        let seed: [u8; 32] = kp.private().to_bytes()[..]
            .try_into()
            .context("device key is not 32 bytes")?;
        let token = auth::device::mint(&kp, &user, Duration::from_secs(3600))?;
        Ok((
            Opener {
                device: (
                    identity::fingerprint(&public),
                    ed25519_dalek::SigningKey::from_bytes(&seed),
                ),
                root: load_root(keys)?,
            },
            user,
            token,
        ))
    }
    fn open(&self, lib: &Library) -> Result<library::Key> {
        library::open_library(
            lib,
            Some((&self.device.0, &self.device.1)),
            self.root.as_ref(),
        )
        .with_context(|| format!("library {}: no key of this machine opens it", lib.id))
    }
}

/// The gate on the box, for one library: WebDAV over the library's prefix,
/// the device token as bearer. Names and bytes are rclone's crypt format,
/// made and read here with the library's cipher; the gate moves ciphertext.
pub struct Gate {
    base: String,
    token: String,
    http: reqwest::Client,
    cipher: library::crypt::Cipher,
}

/// one thing in a library, by its plain name
#[derive(Debug, Clone, serde::Serialize)]
pub struct Item {
    /// the plain path under the library
    pub path: String,
    pub dir: bool,
    /// plain bytes (the sealed size less headers and tags)
    pub size: u64,
    pub modified: String,
}

impl Gate {
    pub fn new(files_base: &str, lib: &str, token: &str, key: &library::Key) -> Self {
        Gate {
            base: format!("{}/_dd/dav/{lib}", files_base.trim_end_matches('/')),
            token: token.to_string(),
            http: directory::builder()
                .and_then(|b| b.build().map_err(Into::into))
                .unwrap_or_default(),
            cipher: library::crypt::Cipher::for_library(key, lib),
        }
    }

    pub fn cipher(&self) -> &library::crypt::Cipher {
        &self.cipher
    }

    /// the url of a plain path: every segment encrypted, then url-encoded
    pub fn url(&self, plain: &str) -> String {
        let enc = self.cipher.encrypt_path(plain);
        let mut u = self.base.clone();
        u.push('/');
        u.push_str(
            &enc.split('/')
                .filter(|s| !s.is_empty())
                .map(urlencoding)
                .collect::<Vec<_>>()
                .join("/"),
        );
        u
    }

    fn req(&self, method: &str, url: &str) -> reqwest::RequestBuilder {
        self.http
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).expect("a method"),
                url,
            )
            .bearer_auth(&self.token)
    }

    /// what is directly under a plain folder ("" for the root)
    pub async fn list(&self, plain_dir: &str) -> Result<Vec<Item>> {
        let mut url = self.url(plain_dir);
        if !url.ends_with('/') {
            url.push('/');
        }
        let r = self
            .req("PROPFIND", &url)
            .header("depth", "1")
            .send()
            .await?;
        let status = r.status();
        let body = r.text().await.unwrap_or_default();
        if status.as_u16() == 404 {
            return Ok(Vec::new());
        }
        if status.as_u16() != 207 {
            bail!(
                "gate: {status} {}",
                body.chars().take(200).collect::<String>()
            );
        }
        let prefix = format!("{}/", self.base.trim_end_matches('/'));
        let prefix_path = prefix
            .split_once("/_dd/dav/")
            .map(|(_, p)| format!("/_dd/dav/{p}"))
            .unwrap_or_default();
        let mut out = Vec::new();
        for resp in body.split("<D:response>").skip(1) {
            let href = between(resp, "<D:href>", "</D:href>").unwrap_or_default();
            let href = unxml(&href);
            let Some(rel) = href.strip_prefix(&prefix_path) else {
                continue;
            };
            let dir = resp.contains("<D:collection/>");
            let rel = rel.trim_end_matches('/');
            if rel.is_empty() {
                continue;
            }
            let enc = rel
                .split('/')
                .map(percent_decode)
                .collect::<Vec<_>>()
                .join("/");
            let Ok(path) = self.cipher.decrypt_path(&enc) else {
                // a name not in the format (the trash, a stray): not ours to show
                continue;
            };
            // the folder asked about lists itself first
            if path == plain_dir.trim_matches('/') {
                continue;
            }
            let sealed: u64 = between(resp, "<D:getcontentlength>", "</D:getcontentlength>")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            out.push(Item {
                size: if dir {
                    0
                } else {
                    library::crypt::plain_size(sealed).unwrap_or(0)
                },
                path,
                dir,
                modified: between(resp, "<D:getlastmodified>", "</D:getlastmodified>")
                    .map(|s| unxml(&s))
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Everything under a plain folder, all levels. Folders come too: a
    /// library that holds only folders is not an empty library, and
    /// listing it as one is how it looks broken.
    pub async fn walk(&self, plain_dir: &str) -> Result<Vec<Item>> {
        let mut out = Vec::new();
        let mut todo = vec![plain_dir.to_string()];
        while let Some(d) = todo.pop() {
            for it in self.list(&d).await? {
                if it.dir {
                    todo.push(it.path.clone());
                }
                out.push(it);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// a file in: read plain bytes, out go header and sealed blocks, as a
    /// stream with a known length. `progress` gets plain bytes done.
    pub async fn put(
        &self,
        plain_path: &str,
        mut src: impl std::io::Read + Send + 'static,
        plain_len: u64,
        progress: impl Fn(u64) + Send + 'static,
    ) -> Result<()> {
        let enc = self.cipher.encrypter();
        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<Vec<u8>, std::io::Error>>(4);
        std::thread::spawn(move || {
            let _ = tx.blocking_send(Ok(enc.header()));
            let mut buf = vec![0u8; library::crypt::BLOCK];
            let mut n = 0u64;
            let mut done = 0u64;
            loop {
                let got = match read_full(&mut src, &mut buf) {
                    Ok(g) => g,
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        return;
                    }
                };
                if got == 0 {
                    break;
                }
                if tx.blocking_send(Ok(enc.block(n, &buf[..got]))).is_err() {
                    return;
                }
                n += 1;
                done += got as u64;
                progress(done);
                if got < library::crypt::BLOCK {
                    break;
                }
            }
        });
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let r = self
            .req("PUT", &self.url(plain_path))
            .header("content-length", library::crypt::sealed_size(plain_len))
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await?;
        if !r.status().is_success() {
            bail!("storing {plain_path}: {}", r.status());
        }
        Ok(())
    }

    /// a file out: sealed bytes in, plain bytes to `dst`
    pub async fn get(
        &self,
        plain_path: &str,
        mut dst: impl std::io::Write,
        progress: impl Fn(u64),
    ) -> Result<u64> {
        let r = self.req("GET", &self.url(plain_path)).send().await?;
        if !r.status().is_success() {
            bail!("fetching {plain_path}: {}", r.status());
        }
        let mut body = r.bytes_stream();
        use futures::StreamExt as _;
        let mut pending: Vec<u8> = Vec::new();
        let mut dec = None;
        let mut n = 0u64;
        let mut done = 0u64;
        while let Some(chunk) = body.next().await {
            pending.extend_from_slice(&chunk?);
            if dec.is_none() {
                if pending.len() < library::crypt::HEADER {
                    continue;
                }
                dec = Some(self.cipher.decrypter(&pending[..library::crypt::HEADER])?);
                pending.drain(..library::crypt::HEADER);
            }
            let d = dec.as_ref().expect("decrypter");
            while pending.len() >= library::crypt::SEALED_BLOCK {
                let plain = d.block(n, &pending[..library::crypt::SEALED_BLOCK])?;
                dst.write_all(&plain)?;
                done += plain.len() as u64;
                progress(done);
                pending.drain(..library::crypt::SEALED_BLOCK);
                n += 1;
            }
        }
        if dec.is_none() {
            bail!("{plain_path}: shorter than a header");
        }
        if !pending.is_empty() {
            let plain = dec.as_ref().expect("decrypter").block(n, &pending)?;
            dst.write_all(&plain)?;
            done += plain.len() as u64;
            progress(done);
        }
        Ok(done)
    }

    /// a url a box may fetch the sealed file from for a few minutes, for
    /// the transcode handoff
    pub async fn handoff_url(&self, plain_path: &str) -> Result<String> {
        let r = self
            .req("GET", &self.url(plain_path))
            .header("x-dd-presign", "1")
            .send()
            .await?;
        if !r.status().is_success() {
            bail!("handing off {plain_path}: {}", r.status());
        }
        let v: serde_json::Value = r.json().await?;
        v["url"]
            .as_str()
            .map(str::to_string)
            .context("gate gave no url")
    }

    /// to the trash (the gate never deletes)
    pub async fn trash(&self, plain_path: &str) -> Result<()> {
        let r = self.req("DELETE", &self.url(plain_path)).send().await?;
        if !r.status().is_success() {
            bail!("trashing {plain_path}: {}", r.status());
        }
        Ok(())
    }

    /// a folder, so an empty one shows
    pub async fn mkdir(&self, plain_path: &str) -> Result<()> {
        let r = self.req("MKCOL", &self.url(plain_path)).send().await?;
        if !r.status().is_success() {
            bail!("making {plain_path}: {}", r.status());
        }
        Ok(())
    }

    /// the base url and token, for something else that speaks WebDAV
    /// (rclone) to open the same library
    pub fn dav(&self) -> (&str, &str) {
        (&self.base, &self.token)
    }
}

fn read_full(r: &mut impl std::io::Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut at = 0;
    while at < buf.len() {
        match r.read(&mut buf[at..]) {
            Ok(0) => break,
            Ok(k) => at += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(at)
}

fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 3 <= b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn between(s: &str, a: &str, b: &str) -> Option<String> {
    let start = s.find(a)? + a.len();
    let end = s[start..].find(b)? + start;
    Some(s[start..end].to_string())
}

fn unxml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// every library this machine can open: the member's own, then those
/// shared with them by anyone whose entry names them
pub async fn openable(
    dirs: &[String],
    user: &str,
    opener: &Opener,
) -> Result<Vec<(String, Library, library::Key)>> {
    let mut out = Vec::new();
    let found = directory::fetch(dirs, user).await;
    // the library ids and sealed keys below go straight into rclone mounts
    // and the app, so the entry they come out of has to verify and the
    // directories have to agree about it
    let mine = directory::resolve(&found, directory::Anchor::FirstSight)
        .context("no directory has a usable entry for you")?;
    // every id here is about to be a folder and an argument; a box may have
    // refused a bad one already, and this machine does not take its word
    for lib in mine
        .entry
        .libraries
        .iter()
        .filter(|l| identity::valid_library_id(&l.id))
    {
        if let Ok(k) = opener.open(lib) {
            out.push((user.to_string(), lib.clone(), k));
        }
    }
    // shared with us: a reader's keys are sealed the same way, under the
    // owner's entry. The gate lists every entry; a reader looks through
    // them once
    if let Some(dir) = dirs.first()
        && let Ok(names) = directory::names(dir).await
    {
        for name in names.into_iter().filter(|n| n != user) {
            let f = directory::fetch(dirs, &name).await;
            // an owner who shared with us: the same rule. A forged entry
            // here would name a library and a key of the liar's choosing
            let Ok(e) = directory::resolve(&f, directory::Anchor::FirstSight) else {
                continue;
            };
            for lib in e
                .entry
                .libraries
                .iter()
                .filter(|l| identity::valid_library_id(&l.id))
            {
                if let Some(r) = lib.readers.iter().find(|r| r.name == user) {
                    let as_mine = Library {
                        keys: r.keys.clone(),
                        ..lib.clone()
                    };
                    if let Ok(k) = opener.open(&as_mine) {
                        out.push((name.clone(), lib.clone(), k));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// where the directory is: the front door, which answers from anywhere
pub fn door_base(dirs: &[String]) -> Result<String> {
    let d = dirs.first().context("no directory")?;
    Ok(d.trim_end_matches("/_dd/directory").to_string())
}

/// The library gate: files.<base> beside the front door's home.<base>. It
/// is not public; a device reaches it on the owner's tailnet or through
/// the fleet's network. A directory anywhere else (a test's) is its own gate.
pub fn files_base(dirs: &[String]) -> Result<String> {
    let door = door_base(dirs)?;
    Ok(match door.split_once("://home.") {
        Some((scheme, rest)) => format!("{scheme}://files.{rest}"),
        None => door,
    })
}

/// Just the host out of a base url: what a token names as its audience.
pub fn host_of(base: &str) -> String {
    base.rsplit("://")
        .next()
        .unwrap_or(base)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

#[cfg(test)]
mod audience_tests {
    use super::*;

    #[test]
    fn a_base_url_names_one_host() {
        assert_eq!(host_of("https://files.commonty.org"), "files.commonty.org");
        assert_eq!(host_of("https://files.commonty.org/"), "files.commonty.org");
        assert_eq!(host_of("http://100.95.10.10:4181"), "100.95.10.10");
        assert_eq!(host_of("https://FILES.Commonty.ORG"), "files.commonty.org");
        // the gate's own base, as files_base builds it
        let dirs = ["https://files.commonty.org/_dd/directory".to_string()];
        assert_eq!(host_of(&files_base(&dirs).unwrap()), "files.commonty.org");
        // the front door's directory, and the gate beside it
        let dirs = ["https://home.commonty.org/_dd/directory".to_string()];
        assert_eq!(files_base(&dirs).unwrap(), "https://files.commonty.org");
        assert_eq!(door_base(&dirs).unwrap(), "https://home.commonty.org");
        let dirs = ["http://127.0.0.1:4181/_dd/directory".to_string()];
        assert_eq!(files_base(&dirs).unwrap(), "http://127.0.0.1:4181");
    }
}
