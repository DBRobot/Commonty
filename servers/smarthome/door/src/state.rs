//! What the door keeps on the controller's locked partition: who is in,
//! who asked, the sessions, Home Assistant's admin, and what Certificate
//! Transparency showed. One JSON file, written whole and renamed into
//! place, so a power cut leaves the old one or the new one.

use anyhow::{Context, Result};
use identity::SignedEntry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

pub const STATE_FILE: &str = "state.json";
pub const KEY_FILE: &str = "controller.key";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct State {
    /// the random first label of the door's name, chosen at pairing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// the fleet's domain, told at /pair/finish
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// when the first person was saved
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired: Option<u64>,
    /// /pair/finish done: the pairing key is gone and :8080 stays shut
    #[serde(default)]
    pub finished: bool,
    /// who paired it: their root signs the door's name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairer: Option<String>,
    #[serde(default)]
    pub people: BTreeMap<String, Person>,
    #[serde(default)]
    pub asked: BTreeMap<String, Asked>,
    /// by sha256 of the cookie, so the file hands no session to its reader
    #[serde(default)]
    pub sessions: BTreeMap<String, Session>,
    #[serde(default)]
    pub ha: HaAdmin,
    /// serials of the certificates this door has served, normalised
    #[serde(default)]
    pub certs: Vec<String>,
    #[serde(default)]
    pub ct: CtSeen,
    /// loopback addresses of people removed, never handed out again: Home
    /// Assistant may still map one to its old user until it restarts
    #[serde(default)]
    pub retired: std::collections::BTreeSet<u8>,
    /// how the last backup went
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<BackupOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupOutcome {
    pub at: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    /// the root pinned for this name; changes only through identity::accept
    pub root: String,
    /// the newest entry accepted for them
    pub entry: SignedEntry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ha_user: Option<String>,
    /// n of 127.0.1.n, their address towards Home Assistant
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u8>,
    pub since: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asked {
    pub entry: SignedEntry,
    pub at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    pub root: String,
    pub expires: u64,
    /// someone not in yet: the entry they signed in with, which asking hands on
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<SignedEntry>,
}

/// The door's own Home Assistant user: made at onboarding, from 127.0.0.1,
/// which the trusted networks never cover.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HaAdmin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CtSeen {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<u64>,
    #[serde(default)]
    pub unexpected: Vec<Unexpected>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Unexpected {
    pub serial: String,
    pub issuer: String,
    pub not_before: String,
    /// crt.sh's id, for a link to the certificate
    pub id: u64,
    pub seen: u64,
}

pub struct Store {
    dir: PathBuf,
    pub state: State,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
        let path = dir.join(STATE_FILE);
        let state = match std::fs::read(&path) {
            Ok(b) => {
                serde_json::from_slice(&b).with_context(|| format!("reading {}", path.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            state,
        })
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(
            &self.dir.join(STATE_FILE),
            &serde_json::to_vec_pretty(&self.state)?,
            0o600,
        )
    }

    /// The person behind a session cookie, if it is live.
    pub fn session(&self, cookie: &str, now: u64) -> Option<&Session> {
        self.state
            .sessions
            .get(&hash(cookie))
            .filter(|s| s.expires > now)
    }

    pub fn drop_expired(&mut self, now: u64) {
        self.state.sessions.retain(|_, s| s.expires > now);
    }

    /// The lowest n in 127.0.1.n nobody holds; 0 and 255 are never handed out.
    pub fn free_address(&self) -> Option<u8> {
        (1..=254u8).find(|n| {
            !self.state.retired.contains(n)
                && !self.state.people.values().any(|p| p.address == Some(*n))
        })
    }
}

pub fn hash(cookie: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(cookie.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Home Assistant's trusted_users: each person's address to their user id.
/// Only people with both; sorted, so the same people make the same file.
pub fn users_yaml(state: &State) -> String {
    let mut lines: Vec<(u8, &str)> = state
        .people
        .values()
        .filter_map(|p| Some((p.address?, p.ha_user.as_deref()?)))
        .collect();
    lines.sort();
    if lines.is_empty() {
        return "{}\n".into();
    }
    lines
        .iter()
        .map(|(n, u)| format!("\"127.0.1.{n}\": \"{u}\"\n"))
        .collect()
}

/// Written beside, synced, renamed over: never half a file.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// The controller's key, made on first start and kept beside the state.
pub fn controller_key(dir: &Path) -> Result<identity::SigningKey> {
    let path = dir.join(KEY_FILE);
    match std::fs::read_to_string(&path) {
        Ok(s) => identity::decode_secret(&s).with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
            let k = identity::generate();
            write_atomic(&path, identity::encode_secret(&k).as_bytes(), 0o600)?;
            Ok(k)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn an_address_once_retired_is_never_handed_out_again() {
        let mut st = Store {
            dir: std::env::temp_dir(),
            state: State::default(),
        };
        assert_eq!(st.free_address(), Some(1));
        st.state.retired.insert(1);
        st.state.retired.insert(2);
        assert_eq!(st.free_address(), Some(3));
    }

    use super::*;
    use crate::tests::{signed_entry, tempdir};

    #[test]
    fn state_survives_a_save_and_the_file_is_private() {
        let dir = tempdir("state");
        let mut s = Store::open(&dir).unwrap();
        assert!(s.state.people.is_empty());
        let (e, _) = signed_entry("ann");
        s.state.people.insert(
            "ann".into(),
            Person {
                root: e.entry.root.clone(),
                entry: e,
                ha_user: Some("u1".into()),
                address: Some(1),
                since: 5,
            },
        );
        s.state.sessions.insert(
            hash("cookie"),
            Session {
                name: "ann".into(),
                root: "r".into(),
                expires: 100,
                entry: None,
            },
        );
        s.save().unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.join(STATE_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let again = Store::open(&dir).unwrap();
        assert_eq!(again.state.people["ann"].address, Some(1));
        assert!(again.session("cookie", 99).is_some());
        assert!(again.session("cookie", 100).is_none(), "run out");
        assert!(again.session("other", 0).is_none());
        // the cookie itself is nowhere in the file
        let raw = std::fs::read_to_string(dir.join(STATE_FILE)).unwrap();
        assert!(!raw.contains("\"cookie\""));
    }

    #[test]
    fn the_controller_key_is_made_once() {
        let dir = tempdir("key");
        let a = controller_key(&dir).unwrap();
        let b = controller_key(&dir).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn addresses_are_handed_out_lowest_first_and_the_users_file_follows() {
        let dir = tempdir("addr");
        let mut s = Store::open(&dir).unwrap();
        assert_eq!(users_yaml(&s.state), "{}\n");
        assert_eq!(s.free_address(), Some(1));
        for (name, n, user) in [
            ("ann", 1, Some("aa")),
            ("bob", 3, Some("bb")),
            ("cy", 2, None),
        ] {
            let (e, _) = signed_entry(name);
            s.state.people.insert(
                name.into(),
                Person {
                    root: e.entry.root.clone(),
                    entry: e,
                    ha_user: user.map(str::to_string),
                    address: Some(n),
                    since: 0,
                },
            );
        }
        assert_eq!(s.free_address(), Some(4));
        // cy has no Home Assistant user yet: not in the file
        assert_eq!(
            users_yaml(&s.state),
            "\"127.0.1.1\": \"aa\"\n\"127.0.1.3\": \"bb\"\n"
        );
    }
}
