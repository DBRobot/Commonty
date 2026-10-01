//! Friends: who plays and shares with whom. Nobody is looked up: a member
//! makes a link, sends it however they like, and whoever opens it while
//! signed in becomes their friend - both ways, until either takes it back.
//!
//! Someone who signs up to open a link comes in as a guest of the member
//! who made it: they may join the game servers their friends invite them
//! to and nothing else (App::guest, and verify refusing every other host).
//! A guest with no member left among their friends is a guest of no one,
//! and is let in nowhere.
//!
//! Kept here, on the box that runs the gate, like its sessions: the friend
//! graph never goes into the directory, so no other box learns it. Links
//! are kept as the hash of their secret, never the secret.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use serde::{Deserialize, Serialize};

/// a link works for a day, and once
pub const LINK_TTL: u64 = 24 * 3600;
/// links one member may have waiting at once
const LINKS_EACH: usize = 20;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Link {
    /// shown on the page to cancel it by; not the secret
    pub id: String,
    pub by: String,
    pub made: u64,
    pub expires: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pair {
    pub a: String,
    pub b: String,
    pub since: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Guest {
    /// the member whose link brought them in
    pub by: String,
    pub since: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    /// sha256 of the link's secret → the link
    #[serde(default)]
    links: BTreeMap<String, Link>,
    #[serde(default)]
    pairs: Vec<Pair>,
    #[serde(default)]
    guests: BTreeMap<String, Guest>,
}

pub struct Store {
    path: PathBuf,
    file: Mutex<File>,
}

#[derive(Debug, PartialEq)]
pub enum Refused {
    /// unknown, used, cancelled or out of date: all look the same from outside
    NoLink,
    Yours,
    Already,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hash(secret: &str) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(secret.as_bytes()))
}

fn pair_of<'a>(p: &'a Pair, name: &str) -> Option<&'a str> {
    if p.a == name {
        Some(&p.b)
    } else if p.b == name {
        Some(&p.a)
    } else {
        None
    }
}

impl Store {
    pub fn open(state_dir: &std::path::Path) -> Result<Self> {
        let path = state_dir.join("friends.json");
        let file = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).context("friends.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// the whole file, written aside and moved into place
    fn save(&self, f: &File) -> Result<()> {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(f)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn change<T>(&self, apply: impl FnOnce(&mut File) -> T) -> Result<T> {
        let mut f = self
            .file
            .lock()
            .map_err(|_| anyhow::anyhow!("friends lock"))?;
        let at = now();
        f.links.retain(|_, l| l.expires > at);
        let out = apply(&mut f);
        self.save(&f)?;
        Ok(out)
    }

    fn read<T>(&self, look: impl FnOnce(&File) -> T) -> T {
        match self.file.lock() {
            Ok(f) => look(&f),
            Err(p) => look(&p.into_inner()),
        }
    }

    /// A new link from `by`: the secret to send, and the link as kept.
    pub fn make_link(&self, by: &str) -> Result<(String, Link)> {
        let secret = B64_URL.encode(crate::session::random(18)?);
        let id = B64_URL.encode(crate::session::random(6)?);
        let at = now();
        let link = Link {
            id,
            by: by.to_string(),
            made: at,
            expires: at + LINK_TTL,
        };
        self.change(|f| {
            anyhow::ensure!(
                f.links.values().filter(|l| l.by == by).count() < LINKS_EACH,
                "{LINKS_EACH} links are waiting already; cancel one"
            );
            f.links.insert(hash(&secret), link.clone());
            Ok(())
        })??;
        Ok((secret, link))
    }

    /// who made the link, while it still works
    pub fn link(&self, secret: &str) -> Option<Link> {
        let at = now();
        self.read(|f| {
            f.links
                .get(&hash(secret))
                .filter(|l| l.expires > at)
                .cloned()
        })
    }

    pub fn links_of(&self, by: &str) -> Vec<Link> {
        let at = now();
        let mut v: Vec<Link> = self.read(|f| {
            f.links
                .values()
                .filter(|l| l.by == by && l.expires > at)
                .cloned()
                .collect()
        });
        v.sort_by_key(|l| l.made);
        v
    }

    pub fn cancel(&self, by: &str, id: &str) -> Result<bool> {
        self.change(|f| {
            let before = f.links.len();
            f.links.retain(|_, l| !(l.by == by && l.id == id));
            f.links.len() != before
        })
    }

    /// `user` opens the link: friends with its maker, and the link spent.
    /// `as_guest` records them as the maker's guest if they are not one of
    /// anyone's yet (someone who signed up to open it).
    pub fn accept(
        &self,
        secret: &str,
        user: &str,
        as_guest: bool,
    ) -> Result<Result<String, Refused>> {
        let at = now();
        self.change(|f| {
            let key = hash(secret);
            let Some(link) = f.links.get(&key).filter(|l| l.expires > at).cloned() else {
                return Err(Refused::NoLink);
            };
            if link.by == user {
                return Err(Refused::Yours);
            }
            f.links.remove(&key);
            if f.pairs
                .iter()
                .any(|p| pair_of(p, user) == Some(link.by.as_str()))
            {
                return Err(Refused::Already);
            }
            f.pairs.push(Pair {
                a: link.by.clone(),
                b: user.to_string(),
                since: at,
            });
            if as_guest && !f.guests.contains_key(user) {
                f.guests.insert(
                    user.to_string(),
                    Guest {
                        by: link.by.clone(),
                        since: at,
                    },
                );
            }
            Ok(link.by)
        })
    }

    /// either side ends it
    pub fn remove(&self, user: &str, other: &str) -> Result<bool> {
        self.change(|f| {
            let before = f.pairs.len();
            f.pairs.retain(|p| pair_of(p, user) != Some(other));
            f.pairs.len() != before
        })
    }

    /// `user`'s friends and since when, oldest first
    pub fn friends_of(&self, user: &str) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = self.read(|f| {
            f.pairs
                .iter()
                .filter_map(|p| pair_of(p, user).map(|o| (o.to_string(), p.since)))
                .collect()
        });
        v.sort_by_key(|(_, s)| *s);
        v
    }

    pub fn are_friends(&self, a: &str, b: &str) -> bool {
        self.read(|f| f.pairs.iter().any(|p| pair_of(p, a) == Some(b)))
    }

    /// came in through a link, whatever they are now
    pub fn guest(&self, user: &str) -> Option<Guest> {
        self.read(|f| f.guests.get(user).cloned())
    }
}

// ---- over http

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

use crate::App;
use crate::pages::{self, Role};

fn signed_in(app: &App, headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    app.signed_in(cookie)
}

/// A change asked for by this site's own page: a form another site posts
/// cannot set this type without the browser asking first, which it refuses.
fn from_our_page(headers: &HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("application/json"))
}

/// GET /_dd/friends: the page, for a member or a guest
pub(crate) async fn page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match signed_in(&app, &headers) {
        Some(u) if matches!(app.role(&u), Some(Role::Member | Role::Guest)) => {
            crate::page("friends")
        }
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None => Redirect::to("/_dd/login?rd=/_dd/friends").into_response(),
    }
}

/// GET /_dd/friends/list: yours, what each is, and your waiting links
pub(crate) async fn list(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = signed_in(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(role @ (Role::Member | Role::Guest)) = app.role(&user) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let friends: Vec<serde_json::Value> = app
        .friends
        .friends_of(&user)
        .into_iter()
        .map(|(name, since)| {
            let kind = match app.role(&name) {
                Some(Role::Member) => "member",
                Some(Role::Guest) => "guest",
                _ => "away",
            };
            serde_json::json!({ "name": name, "initial": pages::initial(&name), "kind": kind, "since": since })
        })
        .collect();
    let links: Vec<serde_json::Value> = app
        .friends
        .links_of(&user)
        .into_iter()
        .map(|l| serde_json::json!({ "id": l.id, "made": l.made, "expires": l.expires }))
        .collect();
    let guest_of = app.friends.guest(&user).map(|g| g.by);
    (
        [("cache-control", "no-store")],
        Json(serde_json::json!({
            "user": user,
            "role": role.as_str(),
            // only members bring people in
            "canInvite": role == Role::Member,
            "guestOf": if role == Role::Guest { guest_of } else { None },
            "friends": friends,
            "links": links,
        })),
    )
        .into_response()
}

/// POST /_dd/friends/link: a new link, from a member
pub(crate) async fn make_link(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = signed_in(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !from_our_page(&headers) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    if app.role(&user) != Some(Role::Member) {
        return (StatusCode::FORBIDDEN, "only a member brings someone in").into_response();
    }
    match app.friends.make_link(&user) {
        Ok((secret, link)) => (
            [("cache-control", "no-store")],
            Json(serde_json::json!({
                "url": format!("https://home.{}/_dd/friend/{secret}", app.domain),
                "id": link.id,
                "expires": link.expires,
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::CONFLICT, e.to_string()).into_response(),
    }
}

/// DELETE /_dd/friends/link/{id}
pub(crate) async fn cancel_link(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(user) = signed_in(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.friends.cancel(&user, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            eprintln!("friends: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// DELETE /_dd/friends/remove/{name}: either side ends it
pub(crate) async fn unfriend(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let Some(user) = signed_in(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.friends.remove(&user, &name) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            eprintln!("friends: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /_dd/friend/{secret}: the page a link opens, for anyone holding it
pub(crate) async fn link_page() -> Response {
    crate::page("friend")
}

/// GET /_dd/friend/{secret}/about: whose link, and who is looking
pub(crate) async fn about(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(secret): Path<String>,
) -> Response {
    let Some(link) = app.friends.link(&secret) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let me = signed_in(&app, &headers);
    let already = me
        .as_deref()
        .is_some_and(|u| app.friends.are_friends(u, &link.by));
    (
        [("cache-control", "no-store")],
        Json(serde_json::json!({
            "by": link.by,
            "initial": pages::initial(&link.by),
            "expires": link.expires,
            "me": me,
            "already": already,
            "demo": me.as_deref() == Some(pages::DEMO_USER),
        })),
    )
        .into_response()
}

/// POST /_dd/friend/{secret}: accept it. Someone who is not a member comes
/// in as the maker's guest, and an account held since sign-up is published
/// now - a guest's is a real account, not one that lapses after a week.
pub(crate) async fn accept(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(secret): Path<String>,
) -> Response {
    let Some(user) = signed_in(&app, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !from_our_page(&headers) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    if user == pages::DEMO_USER {
        return (
            StatusCode::FORBIDDEN,
            "the demo cannot make friends; sign in or make an account",
        )
            .into_response();
    }
    // the maker has to still be a member for the link to bring anyone in
    match app.friends.link(&secret) {
        Some(l) if app.role(&l.by) == Some(Role::Member) => {}
        _ => {
            return (
                StatusCode::NOT_FOUND,
                "that link has been used, cancelled, or has run out",
            )
                .into_response();
        }
    }
    let as_guest = !app.member(&user);
    if as_guest && let Some(held) = app.directory.held(&user) {
        if let Err((status, why)) = app.directory.admit(held).await {
            return (status, why).into_response();
        }
        app.directory.unhold(&user);
    }
    match app.friends.accept(&secret, &user, as_guest) {
        Ok(Ok(by)) => {
            Json(serde_json::json!({ "by": by, "role": app.role(&user).map(Role::as_str) }))
                .into_response()
        }
        Ok(Err(Refused::Yours)) => (
            StatusCode::CONFLICT,
            "that is your own link: send it to someone",
        )
            .into_response(),
        Ok(Err(Refused::Already)) => {
            (StatusCode::CONFLICT, "you are friends already").into_response()
        }
        Ok(Err(Refused::NoLink)) => (
            StatusCode::NOT_FOUND,
            "that link has been used, cancelled, or has run out",
        )
            .into_response(),
        Err(e) => {
            eprintln!("friends: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ---- for the games manager, on this box only
//
// Not under /_dd/, so nginx never forwards a request here: only a process
// on this box reaches them, on the gate's own port.

#[derive(serde::Deserialize)]
pub(crate) struct Server {
    owner: String,
    #[serde(default)]
    players: Vec<String>,
}

/// POST /internal/games/access: for each server, who may play on it now
/// and their devices' addresses on the network. The owner while a member;
/// each player while still the owner's friend and a member or guest. Asked
/// again every time, so a friendship ended closes the door within a beat.
pub(crate) async fn game_access(
    State(app): State<Arc<App>>,
    Json(servers): Json<Vec<Server>>,
) -> Response {
    let mut out = Vec::new();
    for s in servers {
        let mut who = Vec::new();
        if app.role(&s.owner) == Some(Role::Member) {
            who.push(s.owner.clone());
            for p in &s.players {
                if p != &s.owner
                    && p != pages::DEMO_USER
                    && app.friends.are_friends(&s.owner, p)
                    && matches!(app.role(p), Some(Role::Member | Role::Guest))
                {
                    who.push(p.clone());
                }
            }
        }
        let mut addresses = Vec::new();
        if let Some(door) = &app.network {
            for u in &who {
                match door.mine(u).await {
                    Ok(machines) => {
                        for m in machines {
                            for a in m["addresses"].as_array().into_iter().flatten() {
                                if let Some(a) = a.as_str() {
                                    addresses.push(a.to_string());
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("games access: {u}: {e:#}"),
                }
            }
        }
        out.push(serde_json::json!({ "players": who, "addresses": addresses }));
    }
    Json(out).into_response()
}

/// GET /internal/games/friends/{user}: who they may invite
pub(crate) async fn game_friends(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
) -> Response {
    if app.role(&user) != Some(Role::Member) {
        return Json(Vec::<String>::new()).into_response();
    }
    let v: Vec<String> = app
        .friends
        .friends_of(&user)
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| matches!(app.role(n), Some(Role::Member | Role::Guest)))
        .collect();
    Json(v).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(what: &str) -> Store {
        let d = std::env::temp_dir().join(format!("dd-friends-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Store::open(&d).unwrap()
    }

    #[test]
    fn a_link_works_once_and_makes_friends_both_ways() {
        let s = scratch("once");
        let (secret, link) = s.make_link("david").unwrap();
        assert_eq!(s.link(&secret).unwrap().by, "david");
        assert_eq!(s.links_of("david"), vec![link]);
        assert_eq!(s.accept(&secret, "tom", true).unwrap(), Ok("david".into()));
        assert!(s.are_friends("david", "tom") && s.are_friends("tom", "david"));
        assert_eq!(s.friends_of("david")[0].0, "tom");
        assert_eq!(s.guest("tom").unwrap().by, "david");
        // spent: a second person gets nothing, and it is gone from the list
        assert_eq!(
            s.accept(&secret, "sarah", true).unwrap(),
            Err(Refused::NoLink)
        );
        assert!(s.links_of("david").is_empty());
        assert!(s.guest("sarah").is_none() && !s.are_friends("david", "sarah"));
    }

    #[test]
    fn your_own_link_and_a_made_up_one_do_nothing() {
        let s = scratch("own");
        let (secret, _) = s.make_link("david").unwrap();
        assert_eq!(
            s.accept(&secret, "david", false).unwrap(),
            Err(Refused::Yours)
        );
        assert_eq!(
            s.accept("nonsense", "tom", true).unwrap(),
            Err(Refused::NoLink)
        );
        assert!(s.friends_of("david").is_empty());
        // still there for the person it was meant for
        assert!(s.link(&secret).is_some());
    }

    #[test]
    fn a_member_opening_a_link_is_not_made_a_guest() {
        let s = scratch("member");
        let (secret, _) = s.make_link("david").unwrap();
        s.accept(&secret, "lingyun", false).unwrap().unwrap();
        assert!(s.guest("lingyun").is_none());
        assert!(s.are_friends("lingyun", "david"));
    }

    #[test]
    fn either_side_ends_it_and_a_cancelled_link_is_dead() {
        let s = scratch("end");
        let (secret, _) = s.make_link("david").unwrap();
        s.accept(&secret, "tom", true).unwrap().unwrap();
        assert!(s.remove("tom", "david").unwrap());
        assert!(!s.are_friends("david", "tom"));
        let (secret, link) = s.make_link("david").unwrap();
        // only its maker cancels it
        assert!(!s.cancel("tom", &link.id).unwrap());
        assert!(s.cancel("david", &link.id).unwrap());
        assert_eq!(
            s.accept(&secret, "tom", true).unwrap(),
            Err(Refused::NoLink)
        );
    }

    #[test]
    fn it_is_all_still_there_after_a_restart() {
        let d = std::env::temp_dir().join(format!("dd-friends-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (secret, _) = {
            let s = Store::open(&d).unwrap();
            let (a, _) = s.make_link("david").unwrap();
            s.accept(&a, "tom", true).unwrap().unwrap();
            s.make_link("david").unwrap()
        };
        let s = Store::open(&d).unwrap();
        assert!(s.are_friends("david", "tom"));
        assert_eq!(s.guest("tom").unwrap().by, "david");
        assert_eq!(s.link(&secret).unwrap().by, "david");
    }

    #[test]
    fn a_member_cannot_pile_up_links() {
        let s = scratch("many");
        for _ in 0..LINKS_EACH {
            s.make_link("david").unwrap();
        }
        assert!(s.make_link("david").is_err());
        assert!(s.make_link("lingyun").is_ok());
    }
}
