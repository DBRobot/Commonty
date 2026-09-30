//! The per-box browser session.
//!
//! A cookie that only this box issues and only this box accepts, HMAC'd with
//! a secret generated here on first start and never copied anywhere. It is
//! not a key that speaks for anyone elsewhere: a stolen secret forges
//! sessions for this box's services, which the thief already had root on.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const COOKIE: &str = "dd_session";
pub const TTL: u64 = 8 * 3600;

pub struct Sessions {
    secret: Vec<u8>,
    domain: String,
}

impl Sessions {
    pub fn open(dir: &Path, domain: &str) -> Result<Self> {
        let p = dir.join("session.secret");
        let secret = match std::fs::read(&p) {
            Ok(b) if b.len() == 32 => b,
            _ => {
                let b = random(32)?;
                std::fs::write(&p, &b).context("writing the session secret")?;
                b
            }
        };
        Ok(Self {
            secret,
            domain: domain.to_string(),
        })
    }

    /// `who` is the user, then `~` and the session's own random id: names
    /// never hold a `~`, and a session from before the id is the user alone
    fn mac(&self, who: &str, exp: u64) -> String {
        let mut m =
            Hmac::<Sha256>::new_from_slice(&self.secret).expect("hmac accepts any key length");
        m.update(who.as_bytes());
        m.update(b".");
        m.update(exp.to_string().as_bytes());
        B64.encode(m.finalize().into_bytes())
    }

    /// The Set-Cookie header value for a fresh session.
    /// Each is its own, even for one user in one second: the demo counts
    /// what each visitor does by the session they were given.
    pub fn issue(&self, user: &str) -> String {
        let exp = now() + TTL;
        let who = format!("{user}~{}", B64.encode(random(9).unwrap_or_default()));
        let value = format!("{who}.{exp}.{}", self.mac(&who, exp));
        format!(
            "{COOKIE}={value}; Domain={}; Path=/; Max-Age={TTL}; Secure; HttpOnly; SameSite=Lax",
            self.domain
        )
    }

    pub fn clear(&self) -> String {
        format!(
            "{COOKIE}=; Domain={}; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax",
            self.domain
        )
    }

    /// The user a Cookie header names, if the session is ours and unexpired.
    pub fn user(&self, cookie_header: Option<&str>) -> Option<String> {
        let raw = cookie_header?
            .split(';')
            .map(str::trim)
            .find_map(|c| c.strip_prefix(&format!("{COOKIE}=")))?;
        let mut parts = raw.rsplitn(3, '.');
        let mac = parts.next()?;
        let exp: u64 = parts.next()?.parse().ok()?;
        let who = parts.next()?;
        if exp < now() || !same(self.mac(who, exp).as_bytes(), mac.as_bytes()) {
            return None;
        }
        Some(who.split('~').next()?.to_string())
    }
}

/// equal, in time that does not depend on where they first differ
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn random(n: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

pub fn random_id() -> String {
    B64.encode(random(18).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessions() -> Sessions {
        Sessions {
            secret: vec![7; 32],
            domain: "x".into(),
        }
    }

    fn value(set_cookie: &str) -> String {
        set_cookie.split(';').next().unwrap().to_string()
    }

    #[test]
    fn each_session_is_its_own_and_old_ones_still_hold() {
        let s = sessions();
        let (a, b) = (value(&s.issue("demo")), value(&s.issue("demo")));
        assert_ne!(a, b, "two in one second are still two");
        assert_eq!(s.user(Some(&a)).as_deref(), Some("demo"));
        assert_eq!(s.user(Some(&b)).as_deref(), Some("demo"));
        // a name with dots in it is still that name
        let c = value(&s.issue("tom.b"));
        assert_eq!(s.user(Some(&c)).as_deref(), Some("tom.b"));
        // a session issued before the id: the user and the expiry alone
        let exp = now() + 60;
        let old = format!("{COOKIE}=tom.{exp}.{}", s.mac("tom", exp));
        assert_eq!(s.user(Some(&old)).as_deref(), Some("tom"));
        // and the id cannot be changed to pass as another session's
        let forged = a.replacen("demo~", "demo~x", 1);
        assert_eq!(s.user(Some(&forged)), None);
    }
}
