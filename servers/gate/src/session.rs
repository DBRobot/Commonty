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

/// Which cookies this box's sessions are. VERIFY_ID and VERIFY_ID_STAGE say.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cookies {
    /// no id Worker: the whole domain's
    Domain,
    /// the Worker can be tried: a hand-off from it is for its name alone,
    /// and a sign-in here is the domain's as before. Both are read
    Trying,
    /// sign-in is the Worker's: every session is for one name alone, and the
    /// domain-wide cookie is not read
    Host,
}

pub struct Sessions {
    secret: Vec<u8>,
    domain: String,
    cookies: Cookies,
}

impl Sessions {
    pub fn open(dir: &Path, domain: &str, cookies: Cookies) -> Result<Self> {
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
            cookies,
        })
    }

    pub fn cookies(&self) -> Cookies {
        self.cookies
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
        self.cookie(
            user,
            &B64.encode(random(9).unwrap_or_default()),
            now() + TTL,
            self.cookies == Cookies::Host,
        )
    }

    /// A session with a given id and end: a fleet sign-in's (sso.rs), whose
    /// id every box's session from it shares. For this name alone.
    pub fn issue_as(&self, user: &str, id: &str, exp: u64) -> String {
        self.cookie(user, id, exp, self.cookies != Cookies::Domain)
    }

    fn cookie(&self, user: &str, id: &str, exp: u64, one_name: bool) -> String {
        let who = format!("{user}~{id}");
        let value = format!("{who}.{exp}.{}", self.mac(&who, exp));
        let age = exp.saturating_sub(now());
        if one_name {
            format!("{HOST_COOKIE}={value}; Path=/; Max-Age={age}; Secure; HttpOnly; SameSite=Lax")
        } else {
            format!(
                "{COOKIE}={value}; Domain={}; Path=/; Max-Age={age}; Secure; HttpOnly; SameSite=Lax",
                self.domain
            )
        }
    }

    /// Set-Cookie values that end this browser's session here: with
    /// sessions for each name, the domain-wide one from before goes too.
    pub fn clear_all(&self) -> Vec<String> {
        let host = format!("{HOST_COOKIE}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax");
        let domain = format!(
            "{COOKIE}=; Domain={}; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax",
            self.domain
        );
        match self.cookies {
            Cookies::Domain => vec![domain],
            Cookies::Trying | Cookies::Host => vec![host, domain],
        }
    }

    /// The value of the cookie this box's sessions go by, as the browser sent
    /// it: checked by `session`, which is what says whose it is.
    pub fn raw<'a>(&self, header: &'a str) -> Option<&'a str> {
        let named = |name: &str| {
            header
                .split(';')
                .map(str::trim)
                .find_map(|c| c.strip_prefix(name)?.strip_prefix('='))
                .filter(|v| !v.is_empty())
        };
        match self.cookies {
            Cookies::Domain => named(COOKIE),
            Cookies::Trying => named(HOST_COOKIE).or_else(|| named(COOKIE)),
            Cookies::Host => named(HOST_COOKIE),
        }
    }

    /// The user a Cookie header names, if the session is ours and unexpired.
    /// The user, the session's own id ("" for one from before ids) and when
    /// it runs out - of a cookie this box made and that has not.
    /// With sessions for each name (Cookies::Host), only the __Host- cookie
    /// counts. The domain-wide one from before is not read: any name of the
    /// domain - a box someone else runs - can set one of those in a browser,
    /// its owner's own session included, and sign the browser in as them.
    pub fn session(&self, cookie_header: Option<&str>) -> Option<(String, String, u64)> {
        let raw = self.raw(cookie_header?)?;
        let mut parts = raw.rsplitn(3, '.');
        let mac = parts.next()?;
        let exp: u64 = parts.next()?.parse().ok()?;
        let who = parts.next()?;
        if exp < now() || !same(self.mac(who, exp).as_bytes(), mac.as_bytes()) {
            return None;
        }
        let (user, id) = who.split_once('~').unwrap_or((who, ""));
        // the boxes' own names are nobody's, whatever a cookie says: every
        // way a cookie names someone goes through here
        if identity::reserved_name(user) {
            return None;
        }
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
            cookies: Cookies::Domain,
        }
    }

    impl Sessions {
        fn clone_for_test(&self) -> Self {
            Sessions {
                secret: self.secret.clone(),
                domain: self.domain.clone(),
                cookies: self.cookies,
            }
        }
    }

    fn value(set_cookie: &str) -> String {
        set_cookie.split(';').next().unwrap().to_string()
    }

    /// a session under one of the boxes' own names, made by whoever holds
    /// the secret, names nobody: domain-wide or host-only, any id
    #[test]
    fn a_session_for_the_boxes_own_names_is_nobody() {
        for cookies in [Cookies::Domain, Cookies::Trying, Cookies::Host] {
            let s = Sessions {
                cookies,
                ..sessions()
            };
            for name in identity::RESERVED_NAMES {
                let c = value(&s.issue(name));
                assert_eq!(s.session(Some(&c)), None, "{name}");
                let c = value(&s.issue_as(name, "fleet", now() + 60));
                assert_eq!(s.session(Some(&c)), None, "{name} by sso");
            }
            let c = value(&s.issue("sarah"));
            assert_eq!(s.session(Some(&c)).map(|x| x.0).as_deref(), Some("sarah"));
        }
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
            cookies: Cookies::Host,
        };
        let set = s.issue_as("tom", "sid-sid-sid-sid-1", now() + 60);
        assert!(set.starts_with("__Host-dd="), "{set}");
        assert_eq!(
            s.raw(&format!("a=1; {}", value(&set))),
            value(&set).strip_prefix("__Host-dd=")
        );
        assert!(!set.contains("Domain="), "for this name alone: {set}");
        let (user, id, _) = s.session(Some(&value(&set))).unwrap();
        assert_eq!((user.as_str(), id.as_str()), ("tom", "sid-sid-sid-sid-1"));
        // the domain-wide cookie is not read: any name of the domain can set one
        let old = Sessions {
            cookies: Cookies::Domain,
            ..s.clone_for_test()
        };
        assert!(s.session(Some(&value(&old.issue("tom")))).is_none());
        assert_eq!(s.clear_all().len(), 2, "and logging out clears it as well");
        // and another box's (another secret) is nobody here
        let other = Sessions {
            secret: vec![8; 32],
            domain: "x".into(),
            cookies: Cookies::Host,
        };
        assert!(s.session(Some(&value(&other.issue("tom")))).is_none());
        assert!(s.clear_all()[0].starts_with("__Host-dd=;"));
    }

    // what lets the Worker be tried with nobody's sign-in changing
    #[test]
    fn while_the_worker_is_tried_both_cookies_hold() {
        let s = Sessions {
            cookies: Cookies::Trying,
            ..sessions()
        };
        let old = s.issue("tom");
        assert!(old.starts_with("dd_session="), "{old}");
        assert!(old.contains("Domain=x"), "{old}");
        let handed = s.issue_as("tom", "sid-sid-sid-sid-1", now() + 60);
        assert!(handed.starts_with("__Host-dd="), "{handed}");
        assert!(!handed.contains("Domain="), "{handed}");
        assert!(s.session(Some(&value(&old))).is_some());
        assert!(s.session(Some(&value(&handed))).is_some());
        // with both, the one for this name
        let both = format!("{}; {}", value(&old), value(&handed));
        assert_eq!(s.session(Some(&both)).unwrap().1, "sid-sid-sid-sid-1");
        assert_eq!(s.clear_all().len(), 2);
    }

    #[test]
    fn the_secret_is_the_gates_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = std::env::temp_dir().join(format!("dd-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("session.secret");
        let mode = || std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        let first = Sessions::open(&d, "x", Cookies::Domain).unwrap();
        assert_eq!(mode(), 0o600);
        // kept across a restart
        let again = Sessions::open(&d, "x", Cookies::Domain).unwrap();
        assert_eq!(first.secret, again.secret);
        // one others could read is replaced, and its sessions end
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let after = Sessions::open(&d, "x", Cookies::Domain).unwrap();
        assert_ne!(first.secret, after.secret);
        assert_eq!(mode(), 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }
}
