//! A bearer token that renews itself: this device signs one good for an hour
//! and hands out the same one until it is close to expiry, or immediately
//! after the server rejected it. No server is involved in making one.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use auth::KeyStore;
use zeroize::Zeroizing;

pub struct TokenProvider<K: KeyStore> {
    keys: K,
    cached: Mutex<Option<Cached>>,
}

struct Cached {
    token: Zeroizing<String>,
    expires: u64,
}

impl<K: KeyStore + Send + Sync> TokenProvider<K> {
    pub fn new(keys: K) -> Self {
        Self {
            keys,
            cached: Mutex::new(None),
        }
    }

    pub fn token(&self) -> Result<Zeroizing<String>> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let mut slot = self.cached.lock().unwrap();
        if let Some(c) = slot.as_ref()
            && c.expires > now + 60
        {
            return Ok(c.token.clone());
        }
        let kp = auth::device::load(&self.keys)?
            .context("no device key here - `dd device show` makes one")?;
        let user = self
            .keys
            .get("user")?
            .context("no name on this machine - `dd identity new` or `dd identity import`")?;
        let ttl = 3600;
        let tok = Zeroizing::new(auth::device::mint(
            &kp,
            &user,
            std::time::Duration::from_secs(ttl),
        )?);
        *slot = Some(Cached {
            token: tok.clone(),
            expires: now + ttl,
        });
        Ok(tok)
    }

    /// The server said 401: whatever is cached is no good, mint on next ask.
    pub fn invalidate(&self) {
        *self.cached.lock().unwrap() = None;
    }
}
