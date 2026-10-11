use zeroize::Zeroizing;

/// Where a device's secrets live between runs: the OS credential store, or
/// a file (DD_KEYRING_FILE, and the app's private storage on a phone).
pub trait KeyStore {
    fn get(&self, account: &str) -> crate::Result<Option<Zeroizing<String>>>;
    fn set(&self, account: &str, secret: &str) -> crate::Result<()>;
    fn clear(&self, account: &str) -> crate::Result<()>;
}

/// The OS credential store. Encrypted at rest by the OS and unlocked by the
/// user's normal login, so there is no key material of ours on disk.
#[cfg(not(target_os = "android"))]
pub struct OsKeyring {
    service: String,
}

#[cfg(not(target_os = "android"))]
impl OsKeyring {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

/// the service name before the fleet was called commonty: an entry still
/// under it is copied on first read of the renamed service, so nothing on
/// a machine has to be re-made; the old copy stays for a build from before
/// the rename. Any other service (the app's) starts empty.
const OLD_SERVICE: &str = "distributed-datacenter";
const RENAMED: &str = "commonty";

#[cfg(not(target_os = "android"))]
impl KeyStore for OsKeyring {
    fn get(&self, account: &str) -> crate::Result<Option<Zeroizing<String>>> {
        let entry = keyring::Entry::new(&self.service, account)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) if self.service != RENAMED => Ok(None),
            Err(keyring::Error::NoEntry) => {
                let old = keyring::Entry::new(OLD_SERVICE, account)?;
                match old.get_password() {
                    Ok(secret) => {
                        entry.set_password(&secret)?;
                        Ok(Some(Zeroizing::new(secret)))
                    }
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(e) => Err(e.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, account: &str, secret: &str) -> crate::Result<()> {
        keyring::Entry::new(&self.service, account)?.set_password(secret)?;
        Ok(())
    }

    fn clear(&self, account: &str) -> crate::Result<()> {
        match keyring::Entry::new(&self.service, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// A json file, 0600, for tests and for machines with no credential store.
/// DD_KEYRING_FILE selects it. Not encrypted at rest: that is the trade.
pub struct FileStore {
    path: std::path::PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
    fn read(&self) -> crate::Result<std::collections::BTreeMap<String, String>> {
        // the root key, readable by others: refused, as ssh refuses such a key
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(m) = std::fs::metadata(&self.path)
                && m.permissions().mode() & 0o077 != 0
            {
                return Err(crate::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "{} is readable by others: `chmod 600` it, and treat its keys as seen",
                        self.path.display()
                    ),
                )));
            }
        }
        match std::fs::read(&self.path) {
            Ok(b) => Ok(serde_json::from_slice(&b).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(e.into()),
        }
    }
    fn write(&self, m: &std::collections::BTreeMap<String, String>) -> crate::Result<()> {
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let tmp = self.path.with_extension("tmp");
        // 0600 from the moment it exists, not after. Writing first and
        // narrowing second leaves the root key readable for as long as the
        // write takes, to anyone who happened to be looking.
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            // a leftover keeps its own mode: made anew, never reused
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(m).unwrap_or_default())?;
            f.sync_all()?;
        }
        #[cfg(not(unix))]
        std::fs::write(&tmp, serde_json::to_vec_pretty(m).unwrap_or_default())?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

impl KeyStore for FileStore {
    fn get(&self, account: &str) -> crate::Result<Option<Zeroizing<String>>> {
        Ok(self.read()?.remove(account).map(Zeroizing::new))
    }
    fn set(&self, account: &str, secret: &str) -> crate::Result<()> {
        let mut m = self.read()?;
        m.insert(account.to_string(), secret.to_string());
        self.write(&m)
    }
    fn clear(&self, account: &str) -> crate::Result<()> {
        let mut m = self.read()?;
        m.remove(account);
        self.write(&m)
    }
}

/// The store a run uses: the file DD_KEYRING_FILE names, else the OS
/// credential store under `service`; on Android, a file the app names in
/// its private storage (`open_file`).
pub enum Store {
    #[cfg(not(target_os = "android"))]
    Os(OsKeyring),
    File(FileStore),
}

#[cfg(not(target_os = "android"))]
pub fn open(service: &str) -> Store {
    match std::env::var("DD_KEYRING_FILE") {
        Ok(p) if !p.is_empty() => Store::File(FileStore::new(p)),
        _ => Store::Os(OsKeyring::new(service)),
    }
}

/// a store in a file, wherever the caller keeps its private things
pub fn open_file(path: impl Into<std::path::PathBuf>) -> Store {
    Store::File(FileStore::new(path.into()))
}

impl KeyStore for Store {
    fn get(&self, account: &str) -> crate::Result<Option<Zeroizing<String>>> {
        match self {
            #[cfg(not(target_os = "android"))]
            Store::Os(s) => s.get(account),
            Store::File(s) => s.get(account),
        }
    }
    fn set(&self, account: &str, secret: &str) -> crate::Result<()> {
        match self {
            #[cfg(not(target_os = "android"))]
            Store::Os(s) => s.set(account, secret),
            Store::File(s) => s.set(account, secret),
        }
    }
    fn clear(&self, account: &str) -> crate::Result<()> {
        match self {
            #[cfg(not(target_os = "android"))]
            Store::Os(s) => s.clear(account),
            Store::File(s) => s.clear(account),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ignored by default: this talks to the real OS credential store, which
    /// does not exist on a CI runner. Run it with
    ///   cargo test -p auth -- --ignored
    /// on a desktop session.
    #[test]
    #[ignore]
    fn round_trips_through_the_os_store() {
        let ks = OsKeyring::new("commonty-test");
        let account = "roundtrip";

        ks.clear(account).unwrap();
        assert!(ks.get(account).unwrap().is_none(), "should start empty");

        ks.set(account, "hello").unwrap();
        assert_eq!(
            ks.get(account).unwrap().as_deref().map(|s| s.to_string()),
            Some("hello".to_string())
        );

        ks.clear(account).unwrap();
        assert!(ks.get(account).unwrap().is_none(), "clear should remove it");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_store_is_its_owners_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = std::env::temp_dir().join(format!("dd-filestore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("keys.json");
        let s = FileStore::new(&p);
        // a leftover temp file others could read is not written into
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, "").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        s.set("user", "tom").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            s.get("user").unwrap().as_deref().map(|x| x.to_string()),
            Some("tom".into())
        );
        // widened by someone: refused, not read
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(s.get("user").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
