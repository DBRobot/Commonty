//! The bridge's store over a member's libraries: the gate on the box for
//! every read and write, names and bytes sealed here (media::gate). Their
//! own library's calendars by folder name, another library's (a shared
//! calendar, their own or another member's) as `<folder>~<library>`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::sync::Mutex;

use crate::store::{Coll, Item, Kind, Saved, Store, Version};

/// one library this computer opens: its gate, and whether it may write
#[derive(Clone)]
pub struct Lib {
    pub id: String,
    pub key: library::Key,
    /// the member's main library: its calendars are named by folder alone
    pub main: bool,
    /// the member's own (main, or another of theirs: a shared calendar)
    pub owned: bool,
    pub writable: bool,
}

/// Where the libraries and a token come from: the app's or dd's own
/// keys, asked again when the last answer is a while old.
#[async_trait::async_trait]
pub trait Source: Send + Sync + 'static {
    /// every library this computer opens, the main one first
    async fn libraries(&self) -> Result<Vec<Lib>>;
    /// a device token for the gate, and its base url
    async fn token(&self) -> Result<(String, String)>;
    /// the calendars from a link added on this computer: (library, folder, link)
    fn links(&self) -> Vec<(String, String, String)> {
        Vec::new()
    }
}

pub struct GateStore<S> {
    source: S,
    known: Mutex<Option<(Instant, Arc<Vec<Lib>>)>>,
    /// a folder's name and colour, by the file's version
    meta: Mutex<std::collections::HashMap<String, (String, String)>>,
}

const FRESH: Duration = Duration::from_secs(60);

impl<S: Source> GateStore<S> {
    pub fn new(source: S) -> Self {
        GateStore {
            source,
            known: Mutex::new(None),
            meta: Mutex::new(Default::default()),
        }
    }

    async fn libs(&self) -> Result<Arc<Vec<Lib>>> {
        let mut k = self.known.lock().await;
        if let Some((at, libs)) = &*k
            && at.elapsed() < FRESH
        {
            return Ok(libs.clone());
        }
        let libs = Arc::new(self.source.libraries().await?);
        *k = Some((Instant::now(), libs.clone()));
        Ok(libs)
    }

    async fn gate(&self, lib: &Lib) -> Result<media::gate::Gate> {
        let (token, base) = self.source.token().await?;
        Ok(media::gate::Gate::new(&base, &lib.id, &token, &lib.key))
    }

    /// a calendar's id back to its library and folder
    async fn find(&self, coll: &str) -> Result<(Lib, String)> {
        let libs = self.libs().await?;
        let (folder, lib) = match coll.split_once('~') {
            Some((f, l)) => (
                f,
                libs.iter()
                    .find(|x| x.id == l && !x.main)
                    .context("no such library")?,
            ),
            None => (coll, libs.iter().find(|x| x.main).context("no library")?),
        };
        Ok((lib.clone(), folder.to_string()))
    }

    async fn meta(&self, g: &media::gate::Gate, path: &str, etag: &str) -> String {
        let key = format!("{path}\0{etag}");
        if let Some((_, v)) = self.meta.lock().await.get(&key) {
            return v.clone();
        }
        let v = match g.get_bytes(path).await {
            Ok(Some((b, _))) => String::from_utf8_lossy(&b).trim().to_string(),
            _ => String::new(),
        };
        let mut m = self.meta.lock().await;
        // a name or colour by version: old versions are let go of now and then
        if m.len() > 1024 {
            m.clear();
        }
        m.insert(key, (path.to_string(), v.clone()));
        v
    }
}

impl<S: Source> GateStore<S> {
    /// The calendars from a link this computer keeps (Source::links): its
    /// gate, its folder, and the link. Only in the member's own libraries,
    /// and only links added here: the list is this computer's, never a file
    /// another member who can write a calendar could have put there.
    pub async fn linked(&self) -> Result<Vec<(media::gate::Gate, String, String)>> {
        let libs = self.libs().await?;
        let mut out = Vec::new();
        for (lib_id, folder, url) in self.source.links() {
            let ours = folder.starts_with("Calendars/")
                && !folder.contains("..")
                && folder.matches('/').count() == 1;
            let Some(lib) = libs.iter().find(|l| l.id == lib_id && l.owned) else {
                continue;
            };
            if ours {
                out.push((self.gate(lib).await?, folder, url));
            }
        }
        Ok(out)
    }
}

#[async_trait::async_trait]
impl<S: Source> Store for GateStore<S> {
    async fn collections(&self, kind: Kind) -> Result<Vec<Coll>> {
        let mut out = Vec::new();
        for lib in self.libs().await?.iter() {
            let g = self.gate(lib).await?;
            for folder in g.list(kind.root()).await?.into_iter().filter(|i| i.dir) {
                let slug = folder.path.rsplit('/').next().unwrap_or("").to_string();
                let files = g.list(&folder.path).await.unwrap_or_default();
                let tag = |n: &str| {
                    files
                        .iter()
                        .find(|f| f.path.ends_with(&format!("/{n}")))
                        .map(|f| f.etag.clone())
                };
                let name = match tag("displayname") {
                    Some(t) => {
                        self.meta(&g, &format!("{}/displayname", folder.path), &t)
                            .await
                    }
                    None => String::new(),
                };
                let color = match tag("color") {
                    Some(t) => self.meta(&g, &format!("{}/color", folder.path), &t).await,
                    None => String::new(),
                };
                // a calendar from a link is the link's: changes here would
                // be written over at its next refresh
                let linked = tag("source").is_some();
                out.push(Coll {
                    id: if lib.main {
                        slug.clone()
                    } else {
                        format!("{slug}~{}", lib.id)
                    },
                    name: if name.is_empty() { slug } else { name },
                    color: if crate::good_color(&color) {
                        color
                    } else {
                        String::new()
                    },
                    writable: lib.writable && !linked,
                });
            }
        }
        Ok(out)
    }

    async fn items(&self, kind: Kind, coll: &str) -> Result<Vec<Item>> {
        let (lib, folder) = self.find(coll).await?;
        let g = self.gate(&lib).await?;
        Ok(g.list(&format!("{}/{folder}", kind.root()))
            .await?
            .into_iter()
            .filter(|i| !i.dir && i.path.to_ascii_lowercase().ends_with(kind.ext()))
            .map(|i| Item {
                name: i.path.rsplit('/').next().unwrap_or("").to_string(),
                etag: i.etag,
            })
            .collect())
    }

    async fn get(&self, kind: Kind, coll: &str, name: &str) -> Result<Option<(Vec<u8>, String)>> {
        let (lib, folder) = self.find(coll).await?;
        self.gate(&lib)
            .await?
            .get_bytes(&format!("{}/{folder}/{name}", kind.root()))
            .await
    }

    async fn put(
        &self,
        kind: Kind,
        coll: &str,
        name: &str,
        body: &[u8],
        from: &Version,
    ) -> Result<Saved> {
        let (lib, folder) = self.find(coll).await?;
        if !lib.writable {
            bail!("shared with you to see");
        }
        self.gate(&lib)
            .await?
            .put_bytes(&format!("{}/{folder}/{name}", kind.root()), body, from)
            .await
    }

    async fn delete(&self, kind: Kind, coll: &str, name: &str, from: &Version) -> Result<bool> {
        let (lib, folder) = self.find(coll).await?;
        self.gate(&lib)
            .await?
            .trash_if(&format!("{}/{folder}/{name}", kind.root()), from)
            .await
    }

    async fn make(&self, kind: Kind, id: &str, name: &str) -> Result<()> {
        let libs = self.libs().await?;
        let lib = libs.iter().find(|l| l.main).context("no library")?;
        let g = self.gate(lib).await?;
        let at = format!("{}/{id}", kind.root());
        g.mkdir(kind.root()).await.ok();
        g.mkdir(&at).await?;
        g.put_bytes(
            &format!("{at}/displayname"),
            format!("{name}\n").as_bytes(),
            &Version::Any,
        )
        .await?;
        Ok(())
    }

    async fn rename(
        &self,
        kind: Kind,
        coll: &str,
        name: Option<&str>,
        color: Option<&str>,
    ) -> Result<()> {
        let (lib, folder) = self.find(coll).await?;
        let g = self.gate(&lib).await?;
        let at = format!("{}/{folder}", kind.root());
        if let Some(n) = name {
            g.put_bytes(
                &format!("{at}/displayname"),
                format!("{n}\n").as_bytes(),
                &Version::Any,
            )
            .await?;
        }
        if let Some(c) = color {
            g.put_bytes(
                &format!("{at}/color"),
                format!("{c}\n").as_bytes(),
                &Version::Any,
            )
            .await?;
        }
        Ok(())
    }
}
