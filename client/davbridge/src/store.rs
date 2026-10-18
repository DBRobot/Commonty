//! Where the bridge's calendars and address books are: a member's
//! libraries through the gate (gate.rs), or memory, for the tests.

use std::collections::BTreeMap;
use std::sync::Mutex;

use anyhow::Result;
pub use media::gate::{Saved, Version};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Calendars,
    Contacts,
}

impl Kind {
    pub fn root(self) -> &'static str {
        match self {
            Kind::Calendars => "Calendars",
            Kind::Contacts => "Contacts",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Calendars => ".ics",
            Kind::Contacts => ".vcf",
        }
    }
}

/// One calendar or address book. `id` is its name in the bridge's urls:
/// its folder's name in the member's main library, or `<folder>~<library>`
/// for another library's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coll {
    pub id: String,
    pub name: String,
    pub color: String,
    pub writable: bool,
}

/// one event or person: its file name and the gate's version of it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub etag: String,
}

#[async_trait::async_trait]
pub trait Store: Send + Sync + 'static {
    async fn collections(&self, kind: Kind) -> Result<Vec<Coll>>;
    async fn items(&self, kind: Kind, coll: &str) -> Result<Vec<Item>>;
    async fn get(&self, kind: Kind, coll: &str, name: &str) -> Result<Option<(Vec<u8>, String)>>;
    async fn put(
        &self,
        kind: Kind,
        coll: &str,
        name: &str,
        body: &[u8],
        from: &Version,
    ) -> Result<Saved>;
    /// to the trash; false when another save got there first
    async fn delete(&self, kind: Kind, coll: &str, name: &str, from: &Version) -> Result<bool>;
    /// a new one in the member's main library, its folder `id`
    async fn make(&self, kind: Kind, id: &str, name: &str) -> Result<()>;
    async fn rename(
        &self,
        kind: Kind,
        coll: &str,
        name: Option<&str>,
        color: Option<&str>,
    ) -> Result<()>;
}

/// a file in memory, by (kind, calendar, name): its bytes and version
type Files = BTreeMap<(String, String, String), (Vec<u8>, String)>;

/// The store in memory: what the bridge's tests run against.
#[derive(Default)]
pub struct Memory {
    colls: Mutex<BTreeMap<(String, String), Coll>>,
    files: Mutex<Files>,
    n: Mutex<u64>,
}

fn kid(kind: Kind) -> String {
    kind.root().to_string()
}

impl Memory {
    pub fn with(kind: Kind, id: &str, name: &str, writable: bool) -> Self {
        let m = Memory::default();
        m.add(kind, id, name, writable);
        m
    }
    pub fn add(&self, kind: Kind, id: &str, name: &str, writable: bool) {
        self.colls.lock().unwrap().insert(
            (kid(kind), id.into()),
            Coll {
                id: id.into(),
                name: name.into(),
                color: String::new(),
                writable,
            },
        );
    }
    fn tag(&self) -> String {
        let mut n = self.n.lock().unwrap();
        *n += 1;
        format!("\"v{n}\"")
    }
    pub fn file(&self, kind: Kind, coll: &str, name: &str) -> Option<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(&(kid(kind), coll.into(), name.into()))
            .map(|(b, _)| b.clone())
    }
}

#[async_trait::async_trait]
impl Store for Memory {
    async fn collections(&self, kind: Kind) -> Result<Vec<Coll>> {
        Ok(self
            .colls
            .lock()
            .unwrap()
            .iter()
            .filter(|((k, _), _)| *k == kid(kind))
            .map(|(_, c)| c.clone())
            .collect())
    }
    async fn items(&self, kind: Kind, coll: &str) -> Result<Vec<Item>> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .iter()
            .filter(|((k, c, _), _)| *k == kid(kind) && c == coll)
            .map(|((_, _, n), (_, e))| Item {
                name: n.clone(),
                etag: e.clone(),
            })
            .collect())
    }
    async fn get(&self, kind: Kind, coll: &str, name: &str) -> Result<Option<(Vec<u8>, String)>> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .get(&(kid(kind), coll.into(), name.into()))
            .cloned())
    }
    async fn put(
        &self,
        kind: Kind,
        coll: &str,
        name: &str,
        body: &[u8],
        from: &Version,
    ) -> Result<Saved> {
        let tag = self.tag();
        let mut f = self.files.lock().unwrap();
        let k = (kid(kind), coll.to_string(), name.to_string());
        let now = f.get(&k).map(|(_, e)| e.clone());
        let ok = match (from, &now) {
            (Version::Any, _) => true,
            (Version::New, n) => n.is_none(),
            (Version::From(e), Some(n)) => e == n,
            (Version::From(_), None) => false,
        };
        if !ok {
            return Ok(Saved::Changed);
        }
        f.insert(k, (body.to_vec(), tag.clone()));
        Ok(Saved::Done(tag))
    }
    async fn delete(&self, kind: Kind, coll: &str, name: &str, from: &Version) -> Result<bool> {
        let mut f = self.files.lock().unwrap();
        let k = (kid(kind), coll.to_string(), name.to_string());
        if let (Version::From(e), Some((_, n))) = (from, f.get(&k))
            && e != n
        {
            return Ok(false);
        }
        f.remove(&k);
        Ok(true)
    }
    async fn make(&self, kind: Kind, id: &str, name: &str) -> Result<()> {
        self.add(kind, id, name, true);
        Ok(())
    }
    async fn rename(
        &self,
        kind: Kind,
        coll: &str,
        name: Option<&str>,
        color: Option<&str>,
    ) -> Result<()> {
        if let Some(c) = self
            .colls
            .lock()
            .unwrap()
            .get_mut(&(kid(kind), coll.to_string()))
        {
            if let Some(n) = name {
                c.name = n.into();
            }
            if let Some(col) = color {
                c.color = col.into();
            }
        }
        Ok(())
    }
}
