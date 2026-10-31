//! A browser the member's app let in (app/src/local.rs), signed in here by
//! that app, with no passkey. This box hands the browser a challenge of its
//! own, bound to that browser by a cookie, and sends it to the app's page
//! on this computer; the app, which knows the browser by its cookie there
//! and by a one-time ticket it gave for this name, signs a grant with its
//! device key and sends the browser back with it. The grant is a device
//! token (verify_device_token) for one purpose, `browser-signin`, at this
//! name alone, for a minute, naming the challenge. Checked against this
//! box's directory, it becomes a session cookie for this name alone, as a
//! passkey sign-in through the id page would (sso.rs). No key reaches the
//! browser, and a grant is good once, here, and nowhere else.
//!
//! Only where sign-in is the id Worker's and its stage has sessions for
//! each name alone (__Host-dd, `Cookies::Host`). Otherwise the gate has the
//! domain-wide cookie, which a grant here must never set.
//!
//! What this name keeps in the browser, all for this name alone:
//! - `dd_app_at`: the app's address for this browser and the ticket the
//!   app gave for this name, HttpOnly and sent to this route alone
//!   (Path=/_dd/app-signin), so no page reads it and no service behind the
//!   gate is sent it. A name is first given one by a click on one of the
//!   app's own pages (the app's `/_dd/app/to-box`).
//! - `dd_app_linked=1`: that it has one. No secret and no address: the
//!   pages read it to send what the app carries to the app's copy.
//! - `__Host-dd-app-bind`: the challenge's other half, for a minute.
//! - `__Host-dd-app-tried`: sent to the app a moment ago; back without a
//!   grant is the id page.
//!
//! Nothing for the whole domain: any name under it could write that.
//!
//! Signing out is the app's to keep. A browser the app signed in here and
//! signs out (`/_dd/logout`) goes through this route to the app on its way
//! to the id page; the app marks it signed out and grants nothing for it
//! until a click on one of its own pages. Every grant for one browser names
//! the app's sign-in id for it (`sid`), and the session here is
//! `sso::session_id(user, "app:" + sid)`, as a fleet sign-in's is: on
//! signing out the app ends that id on every name it granted, with a
//! device-signed `POST /_dd/app-signin` (purpose `browser-signout`) to
//! each, so a sign-out on one name ends the browser's sessions on all.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

use crate::{App, session, sso, urlencode, valid_user};

/// what the app's grant is for, and nothing else takes it
pub(crate) const PURPOSE: &str = "browser-signin";
/// what the app's call that ends a browser's sessions is for
pub(crate) const SIGNOUT: &str = "browser-signout";
/// how long a challenge waits for the app's grant
const WAIT: Duration = Duration::from_secs(60);
/// the longest a grant may say it lives
const GRANT_MOST: u64 = 120;
const ROUTE: &str = "/_dd/app-signin";
const AT: &str = "dd_app_at";
const LINKED: &str = "dd_app_linked";
const BIND: &str = "__Host-dd-app-bind";
const TRIED: &str = "__Host-dd-app-tried";
/// challenges waiting at once: past it a new one is refused, and none
/// that waits is ever pushed out by someone asking for more
const MOST_WAITING: usize = 4096;
const KEEP: u64 = 30 * 24 * 3600;

/// A challenge this box gave out: for which of its names, the browser's
/// half of it, where the app answers that browser, where it was going
struct Waiting {
    at: Instant,
    host: String,
    bind: String,
    app: String,
    rd: String,
}

/// one store for the gate's life; each challenge is good once
#[derive(Default)]
pub(crate) struct Challenges(Mutex<HashMap<String, Waiting>>);

impl Challenges {
    /// a challenge and the browser's half of it
    fn give(&self, host: &str, app: &str, rd: &str, now: Instant) -> Option<(String, String)> {
        let nonce = session::random_id();
        let bind = session::random_id();
        let mut w = self.0.lock().unwrap();
        w.retain(|_, v| now.duration_since(v.at) < WAIT);
        if w.len() >= MOST_WAITING {
            return None;
        }
        w.insert(
            nonce.clone(),
            Waiting {
                at: now,
                host: host.to_string(),
                bind: bind.clone(),
                app: app.to_string(),
                rd: rd.to_string(),
            },
        );
        Some((nonce, bind))
    }

    /// The challenge, spent, if this browser holds its other half, it was
    /// this name's and it is still waiting: where the app is and where the
    /// browser was going. Another browser's answer leaves it as it was.
    fn take(
        &self,
        nonce: &str,
        bind: Option<&str>,
        host: &str,
        now: Instant,
    ) -> Option<(String, String)> {
        let mut all = self.0.lock().unwrap();
        let w = all.get(nonce)?;
        if !bind.is_some_and(|b| session::same(b.as_bytes(), w.bind.as_bytes())) {
            return None;
        }
        let w = all.remove(nonce)?;
        (w.host == host && !host.is_empty() && now.duration_since(w.at) < WAIT)
            .then_some((w.app, w.rd))
    }
}

static CHALLENGES: LazyLock<Challenges> = LazyLock::new(Challenges::default);

/// The app's address for a let-in browser, exactly: anything else is not
/// somewhere this box sends one
pub(crate) fn valid_at(at: &str) -> bool {
    at.strip_prefix("s-")
        .and_then(|r| r.strip_suffix(".localhost:41660"))
        .is_some_and(hex32)
}

fn hex32(h: &str) -> bool {
    h.len() == 32 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// a ticket the app gave for this name: 64 hex
fn valid_ticket(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(name)?.strip_prefix('='))
        .filter(|v| !v.is_empty())
}

/// what this name keeps: the app's address and its ticket here
fn kept(headers: &HeaderMap) -> Option<(&str, &str)> {
    cookie(headers, AT)
        .and_then(|v| v.split_once('~'))
        .filter(|(a, t)| valid_at(a) && valid_ticket(t))
}

/// What the route needs of the gate, so its rules are tested on their own
pub(crate) trait Desk {
    /// sign-in is the id Worker's here, and sessions this name's alone
    fn on(&self) -> bool;
    fn here(&self, headers: &HeaderMap) -> String;
    fn entry(&self, user: &str) -> anyhow::Result<Option<identity::SignedEntry>>;
    fn member(&self, user: &str) -> bool;
    fn signed_in(&self, headers: &HeaderMap) -> bool;
    /// App::no_new_session
    fn wait(&self) -> Option<Response>;
    /// a session for this name alone, its Set-Cookie
    fn issue(&self, user: &str, id: &str, exp: u64) -> String;
    fn saw(&self, user: &str, id: &str, exp: u64, agent: &str, device: &str);
    /// a session ended on every name of this box (signins.rs)
    fn end(&self, id: &str);
    /// the id page's origin, where signing out goes on to
    fn id_page(&self) -> Option<&str>;
}

impl Desk for App {
    fn on(&self) -> bool {
        self.id_origin.is_some() && self.sessions.cookies() == session::Cookies::Host
    }
    fn here(&self, headers: &HeaderMap) -> String {
        self.directory.here(headers)
    }
    fn entry(&self, user: &str) -> anyhow::Result<Option<identity::SignedEntry>> {
        App::entry(self, user)
    }
    fn member(&self, user: &str) -> bool {
        user != crate::pages::DEMO_USER && App::member(self, user)
    }
    fn signed_in(&self, headers: &HeaderMap) -> bool {
        App::signed_in(self, headers.get("cookie").and_then(|v| v.to_str().ok())).is_some()
    }
    fn wait(&self) -> Option<Response> {
        self.no_new_session()
    }
    fn issue(&self, user: &str, id: &str, exp: u64) -> String {
        self.sessions.issue_as(user, id, exp)
    }
    fn saw(&self, user: &str, id: &str, exp: u64, agent: &str, device: &str) {
        // a session a device signed in ends with that device (signins.rs)
        self.signins.saw(user, id, exp, agent, Some(device));
        self.signins.used(user, device, agent);
    }
    fn end(&self, id: &str) {
        self.signins.end(id);
    }
    fn id_page(&self) -> Option<&str> {
        self.id_origin.as_deref()
    }
}

/// /_dd/login, before the id page: a browser the app let in here goes to
/// this route, which reads what this name keeps for it - unless it signed
/// out, or was sent to the app a moment ago and came back
pub(crate) fn first(desk: &impl Desk, headers: &HeaderMap, rd: &str) -> Option<Response> {
    let ok = desk.on() && cookie(headers, LINKED).is_some() && cookie(headers, TRIED).is_none();
    ok.then(|| {
        Redirect::to(&format!("{ROUTE}?rd={}", urlencode(&sso::local_path(rd)))).into_response()
    })
}

/// Where `/_dd/logout` sends a browser on to (`to`, the id page's out):
/// through this route first, when the app signed this browser in here, so
/// the app hears of it (`?out=`)
pub(crate) fn out_via(desk: &impl Desk, headers: &HeaderMap, to: &str) -> String {
    if desk.on() && cookie(headers, LINKED).is_some() {
        format!("{ROUTE}?out={}", urlencode(to))
    } else {
        to.to_string()
    }
}

/// `?out=`: on to the app with where to go after, by what this name kept,
/// which is forgotten. Only on to the id page's out or a path here.
fn out(desk: &impl Desk, headers: &HeaderMap, to: &str) -> Response {
    let to = match desk.id_page() {
        Some(id) if to.starts_with(&format!("{id}/out?")) => to.to_string(),
        _ => sso::local_path(to),
    };
    let next = match kept(headers) {
        Some((at, _)) => format!("http://{at}/_dd/app/out?then={}", urlencode(&to)),
        None => to,
    };
    let mut r = Redirect::to(&next).into_response();
    for c in forget() {
        set(&mut r, &c);
    }
    r
}

fn forget() -> Vec<String> {
    vec![
        format!("{AT}=; Path={ROUTE}; Max-Age=0; Secure; HttpOnly; SameSite=Lax"),
        format!("{LINKED}=; Path=/; Max-Age=0; Secure; SameSite=Lax"),
    ]
}

fn keep(at: &str, ticket: &str) -> Vec<String> {
    vec![
        format!("{AT}={at}~{ticket}; Path={ROUTE}; Max-Age={KEEP}; Secure; HttpOnly; SameSite=Lax"),
        format!("{LINKED}=1; Path=/; Max-Age={KEEP}; Secure; SameSite=Lax"),
    ]
}

fn to_app(store: &Challenges, host: &str, at: &str, ticket: &str, rd: &str) -> Response {
    let Some((nonce, bind)) = store.give(host, at, rd, Instant::now()) else {
        return (StatusCode::TOO_MANY_REQUESTS, "too many sign-ins at once").into_response();
    };
    let mut r = Redirect::to(&format!(
        "http://{at}/_dd/app/signin?host={}&nonce={}&ticket={ticket}",
        urlencode(host),
        urlencode(&nonce)
    ))
    .into_response();
    set(
        &mut r,
        &format!("{BIND}={bind}; Path=/; Max-Age=60; Secure; HttpOnly; SameSite=Lax"),
    );
    set(
        &mut r,
        &format!("{TRIED}=1; Path=/; Max-Age=60; Secure; HttpOnly; SameSite=Lax"),
    );
    r
}

fn set(r: &mut Response, c: &str) {
    if let Ok(v) = HeaderValue::from_str(c) {
        r.headers_mut().append("set-cookie", v);
    }
}

fn id_page(rd: &str) -> Response {
    Redirect::to(&format!("/_dd/sso/start?rd={}", urlencode(rd))).into_response()
}

#[derive(serde::Deserialize, Default)]
pub(crate) struct Ask {
    #[serde(default)]
    nonce: String,
    #[serde(default)]
    grant: String,
    /// with no challenge: the app's address, from a click on its page
    #[serde(default)]
    at: String,
    /// the app's ticket for this name: from that click, or with a grant
    /// the next one
    #[serde(default)]
    ticket: String,
    #[serde(default)]
    rd: String,
    /// one of the app's pages, for a page here that the app carries
    #[serde(default)]
    open: String,
    #[serde(default)]
    path: String,
    /// signing out: where to after the app
    #[serde(default)]
    out: String,
}

/// GET /_dd/app-signin:
/// - `?at=&ticket=&rd=`: a click on one of the app's pages; on to the app
///   with a challenge, and kept here once the app has taken the ticket
/// - `?rd=`: from /_dd/login, by what this name keeps; the same
/// - `?nonce=&grant=&ticket=`: the app's answer; a session, or the id page
/// - `?nonce=` alone: the app does not take this browser; the id page, and
///   what this name kept forgotten
/// - `?open=<site|photos|vault>&path=`: the app's copy of a page it carries
/// - `?out=`: signing out, through the app (out_via)
pub(crate) async fn route(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<Ask>,
) -> Response {
    answer(&*app, &CHALLENGES, &headers, q, session::now())
}

fn answer(desk: &impl Desk, store: &Challenges, headers: &HeaderMap, q: Ask, now: u64) -> Response {
    if !desk.on() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let host = desk.here(headers);
    if host.is_empty() {
        return (StatusCode::BAD_REQUEST, "not one of this box's names").into_response();
    }
    if !q.open.is_empty() {
        return open(headers, &q.open, &q.path);
    }
    if !q.out.is_empty() {
        return out(desk, headers, &q.out);
    }
    if q.nonce.is_empty() {
        return start(desk, store, headers, &host, &q);
    }
    // the challenge is this browser's own, or nothing is spent
    let Some((at, rd)) = store.take(&q.nonce, cookie(headers, BIND), &host, Instant::now()) else {
        return id_page("/");
    };
    if q.grant.is_empty() {
        let mut r = id_page(&rd);
        for c in forget() {
            set(&mut r, &c);
        }
        return r;
    }
    let (user, device, sid) = match check(desk, &q.grant, &q.nonce, &host, now) {
        Ok(v) => v,
        Err(why) => {
            eprintln!("app-signin: refused: {why}");
            return id_page(&rd);
        }
    };
    if let Some(r) = desk.wait() {
        return r;
    }
    // the app's sign-in for this browser: one id on every name of every box
    // it granted, which the app ends on all of them when it signs out
    let id = sso::session_id(&user, &format!("app:{sid}"));
    let exp = now + session::TTL;
    let set_session = desk.issue(&user, &id, exp);
    // never the domain-wide cookie, whatever this gate was started with
    if !set_session.starts_with(&format!("{}=", session::HOST_COOKIE)) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    desk.saw(&user, &id, exp, agent, &device);
    let mut r = Redirect::to(&rd).into_response();
    r.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    set(&mut r, &set_session);
    for c in [
        format!("{TRIED}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax"),
        format!("{BIND}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax"),
    ] {
        set(&mut r, &c);
    }
    // the app's next ticket for this name, in place of the one it spent
    let next = if valid_ticket(&q.ticket) {
        keep(&at, &q.ticket)
    } else {
        forget()
    };
    for c in next {
        set(&mut r, &c);
    }
    r
}

/// A click on the app's page (with its address and ticket): to the app,
/// which takes the ticket or not, and nothing is kept here until it has.
/// Else what this name keeps (from /_dd/login); whether the browser is
/// signed out is the app's to say.
fn start(
    desk: &impl Desk,
    store: &Challenges,
    headers: &HeaderMap,
    host: &str,
    q: &Ask,
) -> Response {
    let rd = sso::local_path(&q.rd);
    if valid_at(&q.at) && valid_ticket(&q.ticket) {
        return to_app(store, host, &q.at, &q.ticket, &rd);
    }
    if desk.signed_in(headers) {
        return Redirect::to(&rd).into_response();
    }
    match kept(headers) {
        Some((at, ticket)) => to_app(store, host, at, ticket, &rd),
        None => {
            let mut r = id_page(&rd);
            for c in forget() {
                set(&mut r, &c);
            }
            r
        }
    }
}

/// The app's own copy of one of its pages, where it answers this browser;
/// with nothing kept, the page here and the pages told so
fn open(headers: &HeaderMap, service: &str, path: &str) -> Response {
    let path = sso::local_path(path);
    let Some((at, _)) = kept(headers).filter(|_| matches!(service, "site" | "photos" | "vault"))
    else {
        let mut r = Redirect::to(if service == "site" { &path } else { "/" }).into_response();
        for c in forget() {
            set(&mut r, &c);
        }
        return r;
    };
    let mut to = format!("http://{at}/_dd/app/go?service={service}");
    if path != "/" {
        to.push_str(&format!("&path={}", urlencode(&path)));
    }
    Redirect::to(&to).into_response()
}

/// The grant: a token of one of the user's devices in this box's directory
/// (so not one removed from the entry), for this purpose at this name, in
/// time, naming this challenge in the block the device signed. The user
/// and the device's fingerprint.
fn check(
    desk: &impl Desk,
    grant: &str,
    nonce: &str,
    host: &str,
    now: u64,
) -> Result<(String, String, String), String> {
    let (user, device) =
        crate::verify_device_token_at(|u| desk.entry(u), grant, PURPOSE, host, now)
            .map_err(|e| format!("{e:#}"))?;
    let src = biscuit_auth::UnverifiedBiscuit::from_base64(grant)
        .and_then(|b| b.print_block_source(0))
        .map_err(|e| e.to_string())?;
    if facts(&src, "nonce") != [nonce] {
        return Err("not this challenge's grant".into());
    }
    let sid = match facts(&src, "sid").as_slice() {
        [s] if hex32(s) => s.to_string(),
        _ => return Err("a grant naming no sign-in of the app's".into()),
    };
    // the device says how long; a minute or two is all a grant is for
    let lives = expiry(&src).ok_or("a grant with no end")?;
    if lives > now + GRANT_MOST {
        return Err("a grant that lives too long".into());
    }
    if !valid_user(&user) || !desk.member(&user) {
        return Err(format!("{user} is no member here"));
    }
    Ok((user, device, sid))
}

#[derive(serde::Deserialize)]
pub(crate) struct Revoke {
    sid: String,
}

/// POST /_dd/app-signin, from the member's app (a device token for
/// `browser-signout` at this name): the app signed one of its browsers
/// out, and the sessions it signed in for it here end, on every name of
/// this box. A device ends only its own person's: the id is made from them.
pub(crate) async fn revoke(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    axum::Json(b): axum::Json<Revoke>,
) -> Response {
    ended(&*app, &headers, &b.sid, session::now())
}

fn ended(desk: &impl Desk, headers: &HeaderMap, sid: &str, now: u64) -> Response {
    if !desk.on() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let host = desk.here(headers);
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some((user, _)) = token
        .filter(|_| !host.is_empty() && hex32(sid))
        .and_then(|t| {
            crate::verify_device_token_at(|u| desk.entry(u), t, SIGNOUT, &host, now).ok()
        })
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    desk.end(&sso::session_id(&user, &format!("app:{sid}")));
    StatusCode::NO_CONTENT.into_response()
}

/// every `name("…")` fact's value in a block's source
fn facts<'a>(src: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("{name}(\"");
    src.lines()
        .filter_map(|l| l.trim().strip_prefix(&open)?.strip_suffix("\");"))
        .collect()
}

/// the end the device's own block puts on it (`check if time($t), $t < N`)
fn expiry(src: &str) -> Option<u64> {
    src.lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("check if time($t), $t < ")?
                .strip_suffix(';')?
                .parse()
                .ok()
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use biscuit_auth::{Biscuit, KeyPair, builder::Algorithm};
    use std::cell::RefCell;

    const HOST: &str = "files.commonty.test";
    const AT_HERE: &str = "s-0123456789abcdef0123456789abcdef.localhost:41660";
    const TICKET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEXT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    /// the app's sign-in id for one browser
    const SID: &str = "cccccccccccccccccccccccccccccccc";

    struct Fake {
        on: bool,
        settled: bool,
        host_only: bool,
        signed_in: bool,
        entries: HashMap<String, identity::SignedEntry>,
        members: Vec<String>,
        saw: RefCell<Vec<(String, String)>>,
        ids: RefCell<Vec<String>>,
        ended: RefCell<Vec<String>>,
    }

    impl Desk for Fake {
        fn on(&self) -> bool {
            self.on
        }
        fn here(&self, headers: &HeaderMap) -> String {
            headers
                .get("host")
                .and_then(|h| h.to_str().ok())
                .unwrap_or_default()
                .to_string()
        }
        fn entry(&self, user: &str) -> anyhow::Result<Option<identity::SignedEntry>> {
            Ok(self.entries.get(user).cloned())
        }
        fn member(&self, user: &str) -> bool {
            self.members.iter().any(|m| m == user)
        }
        fn signed_in(&self, _: &HeaderMap) -> bool {
            self.signed_in
        }
        fn wait(&self) -> Option<Response> {
            (!self.settled).then(|| StatusCode::SERVICE_UNAVAILABLE.into_response())
        }
        fn issue(&self, user: &str, id: &str, exp: u64) -> String {
            let name = if self.host_only {
                session::HOST_COOKIE
            } else {
                session::COOKIE
            };
            format!("{name}={user}~{id}.{exp}.mac; Path=/")
        }
        fn saw(&self, user: &str, id: &str, _: u64, _: &str, device: &str) {
            self.saw.borrow_mut().push((user.into(), device.into()));
            self.ids.borrow_mut().push(id.into());
        }
        fn end(&self, id: &str) {
            self.ended.borrow_mut().push(id.into());
        }
        fn id_page(&self) -> Option<&str> {
            Some("https://id.commonty.test")
        }
    }

    fn entry_with(name: &str, kps: &[&KeyPair]) -> identity::SignedEntry {
        let root = identity::generate();
        let devices = kps
            .iter()
            .map(|kp| {
                let pk = base64::engine::general_purpose::STANDARD.encode(kp.public().to_bytes());
                identity::Device {
                    fingerprint: identity::fingerprint(&pk),
                    public_key: pk,
                    added: 1,
                }
            })
            .collect();
        let e = identity::Entry {
            name: name.into(),
            root: identity::encode_public(&root.verifying_key()),
            recovery: String::new(),
            recovered_from: vec![],
            devices,
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            terms: None,
            removed_boxes: vec![],
            version: 1,
            updated: 1,
        };
        identity::sign(e, &root).unwrap()
    }

    fn fake(kp: &KeyPair) -> Fake {
        Fake {
            on: true,
            settled: true,
            host_only: true,
            signed_in: false,
            entries: [("ann".to_string(), entry_with("ann", &[kp]))].into(),
            members: vec!["ann".into()],
            saw: RefCell::new(vec![]),
            ids: RefCell::new(vec![]),
            ended: RefCell::new(vec![]),
        }
    }

    /// as auth::device::mint_signin makes one
    fn grant(kp: &KeyPair, user: &str, host: &str, nonce: &str, exp: u64) -> String {
        grant_as(kp, user, host, nonce, exp, Some(SID))
    }

    fn grant_as(
        kp: &KeyPair,
        user: &str,
        host: &str,
        nonce: &str,
        exp: u64,
        sid: Option<&str>,
    ) -> String {
        let b = Biscuit::builder();
        let b = match sid {
            Some(s) => b.fact(format!("sid({s:?})").as_str()).unwrap(),
            None => b,
        };
        b.fact(format!("user({user:?})").as_str())
            .unwrap()
            .fact(format!("purpose({PURPOSE:?})").as_str())
            .unwrap()
            .fact(format!("audience({host:?})").as_str())
            .unwrap()
            .fact(format!("nonce({nonce:?})").as_str())
            .unwrap()
            .check(format!("check if time($t), $t < {exp}").as_str())
            .unwrap()
            .check(format!("check if here({host:?})").as_str())
            .unwrap()
            .check(format!("check if operation({PURPOSE:?})").as_str())
            .unwrap()
            .build(kp)
            .unwrap()
            .to_base64()
            .unwrap()
    }

    fn device() -> KeyPair {
        KeyPair::new_with_algorithm(Algorithm::Ed25519)
    }

    fn headers(host: &str, cookies: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("host", HeaderValue::from_str(host).unwrap());
        if !cookies.is_empty() {
            h.insert("cookie", HeaderValue::from_str(cookies).unwrap());
        }
        h
    }

    /// a challenge for HOST, and the browser that holds its other half
    fn challenge(store: &Challenges, host: &str, rd: &str) -> (String, HeaderMap) {
        let (n, bind) = store.give(host, AT_HERE, rd, Instant::now()).unwrap();
        (n, headers(HOST, &format!("{BIND}={bind}")))
    }

    fn ask(nonce: &str, grant: &str) -> Ask {
        Ask {
            nonce: nonce.into(),
            grant: grant.into(),
            ticket: NEXT.into(),
            ..Default::default()
        }
    }

    fn location(r: &Response) -> &str {
        r.headers()["location"].to_str().unwrap()
    }

    fn cookies(r: &Response) -> Vec<String> {
        r.headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect()
    }

    fn refused(r: &Response) -> bool {
        location(r).starts_with("/_dd/sso/start")
            && !cookies(r).iter().any(|c| c.contains("__Host-dd="))
    }

    #[test]
    fn a_grant_from_a_device_of_the_members_is_a_session_for_this_name_alone() {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let (n, h) = challenge(&store, HOST, "/_dd/files");
        let now = session::now();
        let r = answer(
            &d,
            &store,
            &h,
            ask(&n, &grant(&kp, "ann", HOST, &n, now + 60)),
            now,
        );
        assert_eq!(location(&r), "/_dd/files");
        let c = cookies(&r);
        assert!(c[0].starts_with("__Host-dd=ann~"), "{c:?}");
        // the app's next ticket, kept for this name and this route alone
        let kept = format!("{AT}={AT_HERE}~{NEXT}; Path=/_dd/app-signin;");
        assert!(
            c.iter()
                .any(|c| c.starts_with(&kept) && c.contains("HttpOnly") && !c.contains("Domain")),
            "{c:?}"
        );
        assert!(
            c.iter()
                .any(|c| c.starts_with(&format!("{LINKED}=1; Path=/;")) && !c.contains("Domain"))
        );
        assert!(c.iter().any(|c| c.starts_with(&format!("{TRIED}=;"))));
        assert!(c.iter().any(|c| c.starts_with(&format!("{BIND}=;"))));
        // the session is the device's: removing it ends the session
        let fp = identity::fingerprint(
            &base64::engine::general_purpose::STANDARD.encode(kp.public().to_bytes()),
        );
        assert_eq!(d.saw.borrow().as_slice(), [("ann".into(), fp)]);
    }

    #[test]
    fn a_challenge_is_answered_only_in_the_browser_it_was_given_to() {
        // someone starts a sign-in of their own, has their own app sign it,
        // and sends another browser to the answer: nothing, and the
        // challenge is not spent by it
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let (n, theirs) = challenge(&store, HOST, "/");
        let now = session::now();
        let g = grant(&kp, "ann", HOST, &n, now + 60);
        for other in [
            headers(HOST, ""),
            headers(HOST, &format!("{BIND}=someone-else")),
        ] {
            assert!(refused(&answer(&d, &store, &other, ask(&n, &g), now)));
        }
        assert!(d.saw.borrow().is_empty());
        assert!(
            !refused(&answer(&d, &store, &theirs, ask(&n, &g), now)),
            "still theirs"
        );
    }

    #[test]
    fn a_challenge_is_good_once() {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let (n, h) = challenge(&store, HOST, "/");
        let now = session::now();
        let g = grant(&kp, "ann", HOST, &n, now + 60);
        assert!(!refused(&answer(&d, &store, &h, ask(&n, &g), now)));
        assert!(
            refused(&answer(&d, &store, &h, ask(&n, &g), now)),
            "a grant replayed"
        );
        assert_eq!(d.saw.borrow().len(), 1);
    }

    #[test]
    fn a_grant_for_another_name_or_challenge_is_nothing_here() {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let now = session::now();
        // made for another box's name, with this one's challenge
        let (n, h) = challenge(&store, HOST, "/");
        let g = grant(&kp, "ann", "git.commonty.test", &n, now + 60);
        assert!(refused(&answer(&d, &store, &h, ask(&n, &g), now)));
        // the challenge another name of this box gave
        let (n, h) = challenge(&store, "git.commonty.test", "/");
        let g = grant(&kp, "ann", HOST, &n, now + 60);
        assert!(refused(&answer(&d, &store, &h, ask(&n, &g), now)));
        // another challenge than the one it names
        let (n, h) = challenge(&store, HOST, "/");
        let (other, _) = challenge(&store, HOST, "/");
        let g = grant(&kp, "ann", HOST, &other, now + 60);
        assert!(refused(&answer(&d, &store, &h, ask(&n, &g), now)));
        // no challenge this box gave at all
        let g = grant(&kp, "ann", HOST, "made-up", now + 60);
        assert!(refused(&answer(&d, &store, &h, ask("made-up", &g), now)));
        assert!(d.saw.borrow().is_empty());
    }

    #[test]
    fn an_old_or_long_grant_or_challenge_is_refused() {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let now = session::now();
        let (n, h) = challenge(&store, HOST, "/");
        let g = grant(&kp, "ann", HOST, &n, now - 1);
        assert!(
            refused(&answer(&d, &store, &h, ask(&n, &g), now)),
            "expired"
        );
        let (n, h) = challenge(&store, HOST, "/");
        let g = grant(&kp, "ann", HOST, &n, now + 3600);
        assert!(
            refused(&answer(&d, &store, &h, ask(&n, &g), now)),
            "an hour"
        );
        // a challenge that waited too long
        let (n, bind) = store.give(HOST, AT_HERE, "/", Instant::now()).unwrap();
        assert!(
            store
                .take(&n, Some(&bind), HOST, Instant::now() + WAIT)
                .is_none()
        );
    }

    #[test]
    fn a_device_not_in_the_entry_or_not_a_member_signs_no_one_in() {
        let kp = device();
        let gone = device();
        let mut d = fake(&kp);
        let store = Challenges::default();
        let now = session::now();
        // removed from the entry: the directory no longer has its key
        let (n, h) = challenge(&store, HOST, "/");
        let g = grant(&gone, "ann", HOST, &n, now + 60);
        assert!(refused(&answer(&d, &store, &h, ask(&n, &g), now)));
        // a grant naming someone else, by ann's device
        let (n, h) = challenge(&store, HOST, "/");
        let g = grant(&kp, "bob", HOST, &n, now + 60);
        assert!(refused(&answer(&d, &store, &h, ask(&n, &g), now)));
        // in the directory, but no member here
        d.members.clear();
        let (n, h) = challenge(&store, HOST, "/");
        let g = grant(&kp, "ann", HOST, &n, now + 60);
        assert!(refused(&answer(&d, &store, &h, ask(&n, &g), now)));
    }

    #[test]
    fn an_access_token_is_no_grant_and_a_grant_no_access_token() {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let now = session::now();
        let (n, h) = challenge(&store, HOST, "/");
        // a plain device token, with the challenge added
        let plain = Biscuit::builder()
            .fact("user(\"ann\")")
            .unwrap()
            .fact(format!("nonce({n:?})").as_str())
            .unwrap()
            .check(format!("check if time($t), $t < {}", now + 60).as_str())
            .unwrap()
            .build(&kp)
            .unwrap()
            .to_base64()
            .unwrap();
        assert!(refused(&answer(&d, &store, &h, ask(&n, &plain), now)));
        let g = grant(&kp, "ann", HOST, &n, now + 60);
        assert!(crate::verify_device_token_at(|u| d.entry(u), &g, "access", HOST, now).is_err());
    }

    #[test]
    fn nothing_without_the_worker_nor_while_who_is_who_is_unknown() {
        let kp = device();
        let store = Challenges::default();
        let now = session::now();
        // no id Worker: the gate has only the domain-wide cookie
        let mut d = fake(&kp);
        d.on = false;
        let (n, h) = challenge(&store, HOST, "/");
        let r = answer(
            &d,
            &store,
            &h,
            ask(&n, &grant(&kp, "ann", HOST, &n, now + 60)),
            now,
        );
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        assert!(first(&d, &headers(HOST, &format!("{LINKED}=1")), "/").is_none());
        // sessions still domain-wide for any reason: none is made
        let mut d = fake(&kp);
        d.host_only = false;
        let (n, h) = challenge(&store, HOST, "/");
        let r = answer(
            &d,
            &store,
            &h,
            ask(&n, &grant(&kp, "ann", HOST, &n, now + 60)),
            now,
        );
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        assert!(cookies(&r).is_empty());
        assert!(d.saw.borrow().is_empty());
        // the directory not read yet
        let mut d = fake(&kp);
        d.settled = false;
        let (n, h) = challenge(&store, HOST, "/");
        let r = answer(
            &d,
            &store,
            &h,
            ask(&n, &grant(&kp, "ann", HOST, &n, now + 60)),
            now,
        );
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(d.saw.borrow().is_empty());
    }

    fn kept_here() -> String {
        format!("{AT}={AT_HERE}~{TICKET}")
    }

    #[test]
    fn a_linked_browser_goes_to_its_app_with_its_ticket_and_back_to_the_id_page_when_none_answers()
    {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        // /_dd/login: to this route, which the kept address is sent to
        let r = first(&d, &headers(HOST, &format!("{LINKED}=1")), "/_dd/files").unwrap();
        assert_eq!(location(&r), "/_dd/app-signin?rd=%2F_dd%2Ffiles");
        let rd = |rd: &str| Ask {
            rd: rd.into(),
            ..Default::default()
        };
        let r = answer(
            &d,
            &store,
            &headers(HOST, &kept_here()),
            rd("/_dd/files"),
            session::now(),
        );
        let l = location(&r);
        assert!(
            l.starts_with(&format!(
                "http://{AT_HERE}/_dd/app/signin?host=files.commonty.test&nonce="
            )),
            "{l}"
        );
        assert!(l.ends_with(&format!("&ticket={TICKET}")), "{l}");
        let c = cookies(&r);
        assert!(
            c.iter()
                .any(|c| c.starts_with(&format!("{BIND}=")) && c.contains("HttpOnly"))
        );
        assert!(c.iter().any(|c| c.starts_with(&format!("{TRIED}=1; Path=/; Max-Age=60; Secure; HttpOnly"))));
        // sent a moment ago and back without a grant: the id page
        assert!(first(&d, &headers(HOST, &format!("{LINKED}=1; {TRIED}=1")), "/").is_none());
        // nothing kept, or nothing that is the app's
        assert!(first(&d, &headers(HOST, ""), "/").is_none());
        for bad in [
            format!("{AT}=evil.example:41660~{TICKET}"),
            format!("{AT}=s-0123456789abcdef0123456789abcdef.localhost:8080~{TICKET}"),
            format!("{AT}={AT_HERE}~short"),
            format!("{AT}={AT_HERE}"),
        ] {
            let r = answer(&d, &store, &headers(HOST, &bad), rd("/"), session::now());
            assert!(location(&r).starts_with("/_dd/sso/start"), "{bad}");
        }
        // the app takes no such browser or ticket: the id page, all forgotten
        let (n, h) = challenge(&store, HOST, "/x");
        let r = answer(&d, &store, &h, ask(&n, ""), session::now());
        assert_eq!(location(&r), "/_dd/sso/start?rd=%2Fx");
        let c = cookies(&r);
        assert!(
            c.iter()
                .any(|c| c.starts_with(&format!("{AT}=; Path=/_dd/app-signin; Max-Age=0")))
        );
        assert!(
            c.iter()
                .any(|c| c.starts_with(&format!("{LINKED}=; Path=/; Max-Age=0")))
        );
    }

    #[test]
    fn signing_out_goes_through_the_app_and_nothing_is_set_for_the_whole_domain() {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let to = "https://id.commonty.test/out?then=https%3A%2F%2Fhome.commonty.test%2F";
        // a browser the app signed in here: through this route on the way
        let linked = headers(HOST, &format!("{LINKED}=1"));
        let via = out_via(&d, &linked, to);
        assert_eq!(via, format!("/_dd/app-signin?out={}", urlencode(to)));
        assert_eq!(
            out_via(&d, &headers(HOST, ""), to),
            to,
            "not linked: as before"
        );
        // here, by what was kept: on to the app, and what was kept forgotten
        let q = Ask {
            out: to.into(),
            ..Default::default()
        };
        let r = answer(&d, &store, &headers(HOST, &kept_here()), q, session::now());
        assert_eq!(
            location(&r),
            format!("http://{AT_HERE}/_dd/app/out?then={}", urlencode(to))
        );
        let c = cookies(&r);
        assert!(
            c.iter()
                .any(|c| c.starts_with(&format!("{AT}=; Path=/_dd/app-signin; Max-Age=0")))
        );
        assert!(
            c.iter()
                .any(|c| c.starts_with(&format!("{LINKED}=; Path=/; Max-Age=0")))
        );
        // nothing any other name could write or clear: no Domain= at all
        assert!(!c.iter().any(|c| c.contains("Domain=")), "{c:?}");
        // only on to the id page's out or a path here, never elsewhere
        for (bad, want) in [
            ("https://evil.example/out?then=x", "/"),
            ("https://id.commonty.test.evil.example/out?x", "/"),
            ("//evil.example", "/"),
            ("/_dd/home", "/_dd/home"),
        ] {
            let q = Ask {
                out: bad.into(),
                ..Default::default()
            };
            let r = answer(&d, &store, &headers(HOST, ""), q, session::now());
            assert_eq!(location(&r), want, "{bad}");
        }
        // and whether it is signed out is the app's: a kept ticket still goes
        // to the app, which says no for a browser it signed out
        let r = answer(
            &d,
            &store,
            &headers(HOST, &kept_here()),
            Ask::default(),
            session::now(),
        );
        assert!(location(&r).starts_with(&format!("http://{AT_HERE}/_dd/app/signin?")));
    }

    #[test]
    fn one_browsers_sign_in_is_one_id_on_every_name_and_the_app_ends_it() {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let now = session::now();
        for host in [HOST, "git.commonty.test"] {
            let (n, bind) = store.give(host, AT_HERE, "/", Instant::now()).unwrap();
            let h = headers(host, &format!("{BIND}={bind}"));
            let r = answer(
                &d,
                &store,
                &h,
                ask(&n, &grant(&kp, "ann", host, &n, now + 60)),
                now,
            );
            assert!(!refused(&r));
        }
        let want = sso::session_id("ann", &format!("app:{SID}"));
        assert_eq!(d.ids.borrow().as_slice(), [want.clone(), want.clone()]);
        // a grant naming no sign-in of the app's, or a malformed one: nothing
        for sid in [
            None,
            Some("short"),
            Some("CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"),
        ] {
            let (n, h) = challenge(&store, HOST, "/");
            let g = grant_as(&kp, "ann", HOST, &n, now + 60, sid);
            assert!(
                refused(&answer(&d, &store, &h, ask(&n, &g), now)),
                "{sid:?}"
            );
        }
        // the app's call ends it: its own device's token, for this, here
        let signout = |kp: &KeyPair, user: &str, host: &str, purpose: &str| {
            Biscuit::builder()
                .fact(format!("user({user:?})").as_str())
                .unwrap()
                .fact(format!("purpose({purpose:?})").as_str())
                .unwrap()
                .fact(format!("audience({host:?})").as_str())
                .unwrap()
                .check(format!("check if time($t), $t < {}", now + 60).as_str())
                .unwrap()
                .check(format!("check if here({host:?})").as_str())
                .unwrap()
                .check(format!("check if operation({purpose:?})").as_str())
                .unwrap()
                .build(kp)
                .unwrap()
                .to_base64()
                .unwrap()
        };
        let call = |token: &str, sid: &str| {
            let mut h = headers(HOST, "");
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
            ended(&d, &h, sid, now).status()
        };
        for (token, sid) in [
            (signout(&kp, "ann", HOST, PURPOSE), SID),
            (signout(&kp, "ann", "git.commonty.test", SIGNOUT), SID),
            (signout(&device(), "ann", HOST, SIGNOUT), SID),
            (signout(&kp, "ann", HOST, SIGNOUT), "not-a-sid"),
        ] {
            assert_eq!(call(&token, sid), StatusCode::UNAUTHORIZED);
        }
        assert!(d.ended.borrow().is_empty());
        assert_eq!(
            call(&signout(&kp, "ann", HOST, SIGNOUT), SID),
            StatusCode::NO_CONTENT
        );
        assert_eq!(d.ended.borrow().as_slice(), [want]);
        assert_eq!(
            ended(&d, &headers(HOST, ""), SID, now).status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn asking_for_many_sign_ins_pushes_out_no_one_elses() {
        let store = Challenges::default();
        let (n, bind) = store.give(HOST, AT_HERE, "/x", Instant::now()).unwrap();
        let mut given = 1;
        while given <= MOST_WAITING && store.give(HOST, AT_HERE, "/", Instant::now()).is_some() {
            given += 1;
        }
        assert_eq!(given, MOST_WAITING, "refused once full");
        let r = to_app(&store, HOST, AT_HERE, TICKET, "/");
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            store.take(&n, Some(&bind), HOST, Instant::now()),
            Some((AT_HERE.to_string(), "/x".to_string())),
            "still waiting, and still its own"
        );
    }

    #[test]
    fn a_click_on_the_apps_page_goes_to_the_app_and_only_to_its_address() {
        let kp = device();
        let mut d = fake(&kp);
        let store = Challenges::default();
        let q = |at: &str, ticket: &str, rd: &str| Ask {
            at: at.into(),
            ticket: ticket.into(),
            rd: rd.into(),
            ..Default::default()
        };
        let r = answer(
            &d,
            &store,
            &headers(HOST, ""),
            q(AT_HERE, TICKET, "/_dd/x"),
            session::now(),
        );
        assert!(location(&r).starts_with(&format!("http://{AT_HERE}/_dd/app/signin?")));
        assert!(location(&r).ends_with(&format!("&ticket={TICKET}")));
        // nothing kept from a link alone: someone else's would overwrite it
        assert!(
            !cookies(&r)
                .iter()
                .any(|c| c.starts_with(AT) || c.starts_with(LINKED))
        );
        for (at, t) in [("evil.example", TICKET), (AT_HERE, ""), (AT_HERE, "x")] {
            let r = answer(
                &d,
                &store,
                &headers(HOST, ""),
                q(at, t, "/"),
                session::now(),
            );
            assert!(location(&r).starts_with("/_dd/sso/start"), "{at} {t}");
        }
        // and back only to a path on this name
        let r = answer(
            &d,
            &store,
            &headers(HOST, ""),
            q("evil.example", "", "//evil.example"),
            session::now(),
        );
        assert_eq!(location(&r), "/_dd/sso/start?rd=%2F");
        // signed in here already: still to the app, for its ticket to be kept
        d.signed_in = true;
        let r = answer(
            &d,
            &store,
            &headers(HOST, ""),
            q(AT_HERE, TICKET, "/_dd/x"),
            session::now(),
        );
        assert!(location(&r).starts_with(&format!("http://{AT_HERE}/_dd/app/signin?")));
        // by what is kept, signed in already: straight on
        let rd = Ask {
            rd: "/_dd/x".into(),
            ..Default::default()
        };
        let r = answer(&d, &store, &headers(HOST, &kept_here()), rd, session::now());
        assert_eq!(location(&r), "/_dd/x");
    }

    #[test]
    fn a_page_the_app_carries_opens_the_apps_copy_by_what_is_kept_here() {
        let kp = device();
        let d = fake(&kp);
        let store = Challenges::default();
        let open = |service: &str, path: &str| Ask {
            open: service.into(),
            path: path.into(),
            ..Default::default()
        };
        let r = answer(
            &d,
            &store,
            &headers(HOST, &kept_here()),
            open("site", "/_dd/settings#backups"),
            session::now(),
        );
        assert_eq!(
            location(&r),
            format!("http://{AT_HERE}/_dd/app/go?service=site&path=%2F_dd%2Fsettings%23backups")
        );
        let r = answer(
            &d,
            &store,
            &headers(HOST, &kept_here()),
            open("vault", "/"),
            session::now(),
        );
        assert_eq!(
            location(&r),
            format!("http://{AT_HERE}/_dd/app/go?service=vault")
        );
        // nothing kept: the page here, and the pages told to stop asking
        let r = answer(
            &d,
            &store,
            &headers(HOST, ""),
            open("site", "/_dd/settings"),
            session::now(),
        );
        assert_eq!(location(&r), "/_dd/settings");
        assert!(
            cookies(&r)
                .iter()
                .any(|c| c.starts_with(&format!("{LINKED}=;")))
        );
        let r = answer(
            &d,
            &store,
            &headers(HOST, &kept_here()),
            open("evil", "/"),
            session::now(),
        );
        assert_eq!(location(&r), "/");
    }

    #[test]
    fn a_blocks_facts_and_end_are_read_exactly() {
        let src = "user(\"ann\");\nnonce(\"abc\");\ncheck if time($t), $t < 1700000060;\n";
        assert_eq!(facts(src, "nonce"), ["abc"]);
        assert_eq!(expiry(src), Some(1_700_000_060));
        assert!(facts("xnonce(\"abc\");", "nonce").is_empty());
    }
}
