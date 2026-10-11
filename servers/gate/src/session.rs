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
/// The cookie for one name alone, when sign-in is the id Worker's: the
/// browser keeps a __Host- cookie to the name that set it, on https, and
/// lets no page or other name of the domain set one in its place.
pub const HOST_COOKIE: &str = "__Host-dd";
pub const TTL: u64 = 8 * 3600;

pub struct Sessions {
    secret: Vec<u8>,
    domain: String,
    /// sessions for each name alone (HOST_COOKIE), not the whole domain
    host_only: bool,
}

impl Sessions {
    pub fn open(dir: &Path, domain: &str, host_only: bool) -> Result<Self> {
        let p = dir.join("session.secret");
        // one anyone else could read may have been: a new one, and every
        // session signs in again
        if crate::private::loose(&p) {
            eprintln!("{} was readable by others: making a new one", p.display());
        }
        let secret = match std::fs::read(&p) {
            Ok(b) if b.len() == 32 && !crate::private::loose(&p) => b,
            _ => {
                let b = random(32)?;
                crate::private::write(&p, &b).context("writing the session secret")?;
                b
            }
        };
        Ok(Self {
            secret,
            domain: domain.to_string(),
            host_only,
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
        self.issue_as(
            user,
            &B64.encode(random(9).unwrap_or_default()),
            now() + TTL,
        )
    }

    /// A session with a given id and end: a fleet sign-in's (sso.rs), whose
    /// id every box's session from it shares.
    pub fn issue_as(&self, user: &str, id: &str, exp: u64) -> String {
        let who = format!("{user}~{id}");
        let value = format!("{who}.{exp}.{}", self.mac(&who, exp));
        let age = exp.saturating_sub(now());
        if self.host_only {
            format!("{HOST_COOKIE}={value}; Path=/; Max-Age={age}; Secure; HttpOnly; SameSite=Lax")
        } else {
            format!(
                "{COOKIE}={value}; Domain={}; Path=/; Max-Age={age}; Secure; HttpOnly; SameSite=Lax",
                self.domain
            )
        }
    }

    pub fn clear(&self) -> String {
        if self.host_only {
            format!("{HOST_COOKIE}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax")
        } else {
            format!(
                "{COOKIE}=; Domain={}; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax",
                self.domain
            )
        }
    }

    /// The user a Cookie header names, if the session is ours and unexpired.
    /// The user, the session's own id ("" for one from before ids) and when
    /// it runs out - of a cookie this box made and that has not.
    /// With sessions for each name, the domain-wide cookie from before is
    /// still read until it runs out: this box made it too.
    pub fn session(&self, cookie_header: Option<&str>) -> Option<(String, String, u64)> {
        let header = cookie_header?;
        let named = |name: &str| {
            header
                .split(';')
                .map(str::trim)
                .find_map(|c| c.strip_prefix(&format!("{name}=")))
                .filter(|v| !v.is_empty())
        };
        let raw = if self.host_only {
            named(HOST_COOKIE).or_else(|| named(COOKIE))
        } else {
            named(COOKIE)
        }?;
        let mut parts = raw.rsplitn(3, '.');
        let mac = parts.next()?;
        let exp: u64 = parts.next()?.parse().ok()?;
        let who = parts.next()?;
        if exp < now() || !same(self.mac(who, exp).as_bytes(), mac.as_bytes()) {
            return None;
        }
        let (user, id) = who.split_once('~').unwrap_or((who, ""));
        Some((user.to_string(), id.to_string(), exp))
    }

    /// A fresh session's Set-Cookie, with its id and expiry for the record.
    pub fn issue_noted(&self, user: &str) -> (String, String, u64) {
        let cookie = self.issue(user);
        let header = cookie.split(';').next().unwrap_or_default().to_string();
        let (_, id, exp) = self.session(Some(&header)).unwrap_or_default();
        (cookie, id, exp)
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
            host_only: false,
        }
    }

    impl Sessions {
        fn clone_for_test(&self) -> Self {
            Sessions {
                secret: self.secret.clone(),
                domain: self.domain.clone(),
                host_only: self.host_only,
            }
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
        assert_eq!(s.session(Some(&a)).map(|x| x.0).as_deref(), Some("demo"));
        assert_eq!(s.session(Some(&b)).map(|x| x.0).as_deref(), Some("demo"));
        // a name with dots in it is still that name
        let c = value(&s.issue("tom.b"));
        assert_eq!(s.session(Some(&c)).map(|x| x.0).as_deref(), Some("tom.b"));
        // a session issued before the id: the user and the expiry alone
        let exp = now() + 60;
        let old = format!("{COOKIE}=tom.{exp}.{}", s.mac("tom", exp));
        assert_eq!(s.session(Some(&old)).map(|x| x.0).as_deref(), Some("tom"));
        // and the id cannot be changed to pass as another session's
        let forged = a.replacen("demo~", "demo~x", 1);
        assert_eq!(s.session(Some(&forged)).map(|x| x.0), None);
    }

    #[test]
    fn a_session_for_one_name_alone() {
        let s = Sessions {
            secret: vec![7; 32],
            domain: "x".into(),
            host_only: true,
        };
        let set = s.issue_as("tom", "sid-sid-sid-sid-1", now() + 60);
        assert!(set.starts_with("__Host-dd="), "{set}");
        assert!(!set.contains("Domain="), "for this name alone: {set}");
        let (user, id, _) = s.session(Some(&value(&set))).unwrap();
        assert_eq!((user.as_str(), id.as_str()), ("tom", "sid-sid-sid-sid-1"));
        // the domain-wide cookie from before still reads, until it runs out
        let old = Sessions {
            host_only: false,
            ..s.clone_for_test()
        };
        assert_eq!(
            s.session(Some(&value(&old.issue("tom"))))
                .map(|x| x.0)
                .as_deref(),
            Some("tom")
        );
        // and another box's (another secret) is nobody here
        let other = Sessions {
            secret: vec![8; 32],
            domain: "x".into(),
            host_only: true,
        };
        assert!(s.session(Some(&value(&other.issue("tom")))).is_none());
        assert!(s.clear().starts_with("__Host-dd=;"));
    }

    #[test]
    fn the_secret_is_the_gates_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = std::env::temp_dir().join(format!("dd-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("session.secret");
        let mode = || std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        let first = Sessions::open(&d, "x", false).unwrap();
        assert_eq!(mode(), 0o600);
        // kept across a restart
        let again = Sessions::open(&d, "x", false).unwrap();
        assert_eq!(first.secret, again.secret);
        // one others could read is replaced, and its sessions end
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let after = Sessions::open(&d, "x", false).unwrap();
        assert_ne!(first.secret, after.secret);
        assert_eq!(mode(), 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }
}
