//! Photos and Passwords in the member's own browser: the rules of pairing a
//! browser with the app, the browser's sessions, the install's names and
//! the commonty:// link, kept apart from the servers (local.rs) so each can
//! be tested on its own. Why each rule exists is docs/dev/app.md, "Photos
//! and Passwords in a browser".

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::Digest as _;

/// Photos' port: fixed, because a browser keeps an origin's storage by its
/// port and Ente keeps a session there
pub const PHOTOS_PORT: u16 = 41661;
/// Passwords': the vault keeps its settings and the sign-in's state there
pub const VAULT_PORT: u16 = 41662;
/// The site's own pages that hold a key (Files, Movies & TV, Chat,
/// Settings): no box serves them to a member, so the app serves them to
/// the member's browser, with this device's keys and token behind them
pub const SITE_PORT: u16 = 41660;

/// the site's pages the app serves a browser, by the path the site has them at
pub const SITE_PAGES: &[&str] = &[
    "/_dd/home",
    "/_dd/files",
    "/_dd/media",
    "/_dd/settings",
    "/_dd/chat",
    "/_dd/mail",
];

/// how long after a link a browser may ask to be let in
pub const PAIR_LIFE: Duration = Duration::from_secs(120);
/// how long a let-in browser has to go through its session's door
pub const DOOR_LIFE: Duration = Duration::from_secs(60);
/// how long after the door the Photos page may ask for its password, once
pub const KEY_LIFE: Duration = Duration::from_secs(60);
/// browsers remembered at once; the one longest unseen goes first past this
const MAX_BROWSERS: usize = 32;
/// how long a let-in browser keeps its cookie: Chrome's own limit, 400 days
pub const BROWSER_LIFE_SECS: u64 = 400 * 24 * 3600;
/// doors open at once; a door is a minute's, so this is plenty
const MAX_DOORS: usize = 64;

/// the window's cookie (this launch's secret) and a browser's (its session)
pub const WINDOW_COOKIE: &str = "dd_local";
pub const BROWSER_COOKIE: &str = "dd_browser";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Photos,
    Vault,
    Site,
}

impl Which {
    pub fn port(self) -> u16 {
        match self {
            Which::Photos => PHOTOS_PORT,
            Which::Vault => VAULT_PORT,
            Which::Site => SITE_PORT,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Which::Photos => "p",
            Which::Vault => "v",
            Which::Site => "s",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Which::Photos => "photos",
            Which::Vault => "vault",
            Which::Site => "site",
        }
    }

    pub fn of(name: &str) -> Option<Which> {
        [Which::Photos, Which::Vault, Which::Site]
            .into_iter()
            .find(|w| w.name() == name)
    }
}

/// One of the site's pages the app serves (SITE_PAGES), with whatever query
/// or fragment follows it
pub fn site_page(path: &str) -> bool {
    let base = path.split(['?', '#']).next().unwrap_or_default();
    SITE_PAGES.contains(&base)
}

/// which way a request came in, by its Host: the app's window on
/// 127.0.0.1, a browser asking to be let in on the install's name, or a
/// browser on this device's own name for let-in browsers (which one, its
/// cookie says)
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Via {
    Window,
    Pair,
    Browser,
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn valid_label(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn host_of(w: Which, label: &str) -> String {
    format!("{}-{label}.localhost", w.prefix())
}

/// This install's names for its two pairing origins, p-<label>.localhost
/// and v-<label>.localhost. They are not secret from other programs on the
/// device (they ride in the browser's command line when the app opens it),
/// so nothing is kept or let in there: they only start a pairing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Names {
    label: String,
}

impl Names {
    pub fn new(label: &str) -> Option<Names> {
        valid_label(label).then(|| Names {
            label: label.to_string(),
        })
    }

    pub fn host(&self, w: Which) -> String {
        host_of(w, &self.label)
    }

    pub fn origin(&self, w: Which) -> String {
        format!("http://{}:{}", self.host(w), w.port())
    }

    /// the label kept in the app's data folder, made the first time
    pub fn load(path: &Path) -> std::io::Result<Names> {
        if let Ok(s) = std::fs::read_to_string(path)
            && let Some(n) = Names::new(s.trim())
        {
            return Ok(n);
        }
        let label = random_hex(16);
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        std::io::Write::write_all(&mut o.open(path)?, label.as_bytes())?;
        Ok(Names { label })
    }
}

/// This device's own name for the browsers it lets in, s-/p-/v-<own>.localhost:
/// made from its device key, so the same across the app's restarts (the
/// browser's cookie, and what Ente or the vault keep, stay where they
/// were) and new with a new device. A hash, so it tells nothing of the key.
/// No program but the browsers let in learns it: it is never in an address
/// the app opens, only in an answer to a browser's own request.
pub fn own_label(device_seed: &[u8]) -> String {
    let mut h = sha2::Sha256::new();
    h.update(b"commonty: the name a let-in browser opens the app's pages at\0");
    h.update(device_seed);
    h.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The Host a request names, exactly: 127.0.0.1:<port> for the window, the
/// install's name for pairing, this device's own name for a browser let in.
/// Anything else (a rebound DNS name, localhost, another port, an old name)
/// is no one this server answers.
pub fn via(host: Option<&str>, w: Which, names: &Names, own: Option<&str>) -> Option<Via> {
    let h = host?.strip_suffix(&format!(":{}", w.port()))?;
    if h == "127.0.0.1" {
        return Some(Via::Window);
    }
    if h == names.host(w) {
        return Some(Via::Pair);
    }
    own.filter(|o| valid_label(o) && h == host_of(w, o))
        .map(|_| Via::Browser)
}

/// The request comes from this origin's own pages. A browser always names
/// the origin of a request that changes something, so from a browser one
/// without it is refused, as is anything it says came from another site.
/// The window is let off both: it arrives from the app's own scheme, and
/// nothing but the app's pages is ever loaded in it.
pub fn same_origin(
    method: &str,
    origin: Option<&str>,
    fetch_site: Option<&str>,
    own: &str,
    window: bool,
) -> bool {
    if origin.is_some_and(|o| o != own) {
        return false;
    }
    if !window && fetch_site.is_some_and(|s| !matches!(s, "same-origin" | "none")) {
        return false;
    }
    let safe = matches!(method, "GET" | "HEAD" | "OPTIONS");
    safe || origin.is_some() || window
}

/// The requests a browser makes on its way in from another page, so
/// without the Strict cookie and as cross-site: its session's door (the
/// pairing page sends it there), and the vault's sign-in coming back from
/// the gate to the app's own page that passes the code on.
pub fn way_in(w: Which, via: &Via, method: &str, path: &str) -> bool {
    *via == Via::Browser
        && method == "GET"
        && (matches!(path, "/_dd/app/enter" | "/_dd/app/resume")
            || (w == Which::Vault && path == "/sso-connector.html"))
}

/// What a server of the app does with a request, before anything is read
/// or opened for it
#[derive(Debug, PartialEq, Eq)]
pub enum Admit {
    Refuse(u16, &'static str),
    /// the pairing page and its two asks, and nothing else
    Pairing,
    /// a session's door, spent here for its cookie
    Door,
    /// a let-in browser, or the app's window: the service's own pages and
    /// what they ask, keys among them
    Serve,
    /// a page opened from somewhere else (a link, a bookmark's tab, the
    /// pairing page): nothing yet, only a page that opens itself again, so
    /// it comes from this origin and with the Strict cookie
    GoOn,
}

/// The one rule every request to the app's servers passes, for each way in
/// (`via`). `cookie_ok`: the request carries the cookie of this way in -
/// the window's launch secret, or a let-in browser's own. A key is only
/// ever behind `Serve`, so only behind that cookie: a browser the member
/// let in from the app's window (the pairing), or the window itself.
/// `nav`: the browser opening a page in a tab (Sec-Fetch-Mode navigate,
/// Sec-Fetch-Dest document), never a frame, a script or a fetch.
#[allow(clippy::too_many_arguments)]
pub fn admit(
    w: Which,
    via: &Via,
    cookie_ok: bool,
    method: &str,
    path: &str,
    origin: Option<&str>,
    fetch_site: Option<&str>,
    nav: bool,
    own: &str,
) -> Admit {
    let way = way_in(w, via, method, path);
    if !way && !same_origin(method, origin, fetch_site, own, *via == Via::Window) {
        // A tab opened at one of these pages from anywhere else used to be
        // refused outright. It is shown nothing it was not already: the
        // pairing page, or a page that opens itself again from here.
        return match via {
            Via::Pair if nav && method == "GET" => Admit::Pairing,
            Via::Browser if nav && method == "GET" => Admit::GoOn,
            _ => Admit::Refuse(403, "not this page's own request"),
        };
    }
    match via {
        Via::Pair => Admit::Pairing,
        // the window has the site's pages on its own scheme (site.rs)
        Via::Window if w == Which::Site => Admit::Refuse(404, "not here"),
        _ if path == "/_dd/app/enter" => Admit::Door,
        _ if cookie_ok => Admit::Serve,
        Via::Browser if way => Admit::Serve,
        Via::Window => Admit::Refuse(403, "open this from the Commonty app"),
        Via::Browser => Admit::Refuse(
            403,
            "open this from its page on the site, through the Commonty app",
        ),
    }
}

/// A page to go on to after a door: a path on this origin and nothing else.
/// No scheme, no //host, no backslash, no character a page's address would
/// need escaping for (it goes into the door page's meta refresh).
pub fn safe_then(t: &str) -> Option<&str> {
    let ok = t.starts_with('/')
        && !t.starts_with("//")
        && t.len() <= 512
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.~?=&%#+:@,!".contains(&b));
    ok.then_some(t)
}

pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = b
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn query<'a>(q: &'a str, key: &str) -> Option<&'a str> {
    q.split('&')
        .find_map(|kv| kv.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// What a commonty:// link asks: `commonty://open?service=photos|vault`,
/// and maybe `&path=<a path on that service>`. Anything else, or a path
/// that is not plainly one on the service, is no link of ours.
#[derive(Debug, PartialEq, Eq)]
pub struct Link {
    pub which: Which,
    pub path: Option<String>,
}

pub fn parse_link(url: &str) -> Option<Link> {
    let rest = url.strip_prefix("commonty://open")?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let q = rest.strip_prefix('?')?;
    let which = Which::of(query(q, "service")?)?;
    let path = match query(q, "path") {
        None => None,
        Some(p) => Some(safe_then(&dec(p))?.to_string()),
    };
    if which == Which::Site && !path.as_deref().is_none_or(site_page) {
        return None;
    }
    Some(Link { which, path })
}

/// where the browser goes once it is let in: Photos' opening page (with
/// the page asked for after it), the vault's, or the site's page asked for
pub fn then_for(link: &Link) -> String {
    match (link.which, link.path.as_deref()) {
        (Which::Photos, Some(p)) if p != "/" && !p.starts_with("/_dd/") => {
            format!("/_dd/photos?to={}", enc(p))
        }
        (Which::Photos, _) => "/_dd/photos".into(),
        (Which::Vault, Some(p)) if !p.starts_with("/_dd/") => p.into(),
        (Which::Vault, _) => "/".into(),
        (Which::Site, Some(p)) if site_page(p) => p.into(),
        (Which::Site, _) => "/_dd/home".into(),
    }
}

/// The address the browser is opened at. Other programs on the device can
/// read it (a browser's command line), so it holds nothing secret: the
/// install's pairing page, and which service it is for. Every service
/// pairs on the site's: what a browser keeps there (pair.js) is one place.
pub fn browser_url(names: &Names, w: Which) -> String {
    format!(
        "{}/_dd/app/pair?service={}",
        names.origin(Which::Site),
        w.name()
    )
}

/// what a pairing page sends: the SHA-256 of its verifier, in hex
pub fn hash_of(verifier: &str) -> String {
    sha2::Sha256::digest(verifier.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The six digits the app's window and the browser's page both show for
/// one request, from its hash (the page works it out the same way).
pub fn code(hash: &str) -> Option<String> {
    if !valid_hash(hash) {
        return None;
    }
    let n = u32::from_str_radix(&hash[..8], 16).ok()? % 1_000_000;
    Some(format!("{n:06}"))
}

/// what the app's window shows for one service
#[derive(Debug, PartialEq, Eq)]
pub enum View {
    Idle,
    Waiting,
    Asking(String),
    Crowded,
}

/// what a pairing page is told when it shows its verifier
#[derive(Debug, PartialEq, Eq)]
pub enum Finish {
    Wait,
    In(String),
    No,
}

struct Arm {
    then: String,
    made: Instant,
}

/// One service's pairing: armed by a link, asked by one browser with the
/// hash of a verifier only it holds, allowed in the app's window by the
/// member, finished by that browser showing the verifier. Two asking at
/// once is one too many: both are refused.
#[derive(Default)]
pub struct Pairing {
    armed: Option<Arm>,
    asked: Option<String>,
    allowed: bool,
    crowded: bool,
}

impl Pairing {
    pub fn arm(&mut self, then: &str, now: Instant) {
        *self = Pairing {
            armed: Some(Arm {
                then: then.to_string(),
                made: now,
            }),
            ..Default::default()
        };
    }

    fn live(&self, now: Instant) -> bool {
        self.armed
            .as_ref()
            .is_some_and(|a| now.duration_since(a.made) < PAIR_LIFE)
    }

    fn close(&mut self) {
        self.armed = None;
        self.asked = None;
        self.allowed = false;
    }

    /// a browser asks, with its hash: the code to show, or why not
    pub fn ask(&mut self, hash: &str, now: Instant) -> Result<String, &'static str> {
        let code = code(hash).ok_or("not a request")?;
        if !self.live(now) {
            return Err("open it from the page first");
        }
        match self.asked.as_deref() {
            None => self.asked = Some(hash.to_string()),
            Some(h) if h == hash => {}
            Some(_) => {
                self.close();
                self.crowded = true;
                return Err("two browsers asked at once");
            }
        }
        Ok(code)
    }

    pub fn view(&self, now: Instant) -> View {
        if self.crowded {
            return View::Crowded;
        }
        if !self.live(now) {
            return View::Idle;
        }
        match self.asked.as_deref().and_then(code) {
            Some(c) if !self.allowed => View::Asking(c),
            _ => View::Waiting,
        }
    }

    /// the member's click in the app's window, for the code it showed
    pub fn allow(&mut self, shown: &str, now: Instant) -> Result<(), &'static str> {
        if !self.live(now) || self.crowded {
            return Err("nothing is asking now");
        }
        match self.asked.as_deref().and_then(code) {
            Some(c) if c == shown => {
                self.allowed = true;
                Ok(())
            }
            _ => Err("that is not the code asking now"),
        }
    }

    /// A browser let in before has come straight in: the page the link
    /// asked for, and nothing left for another to ask with
    pub fn take_then(&mut self, now: Instant) -> Option<String> {
        if self.crowded || !self.live(now) {
            return None;
        }
        let then = self.armed.take().map(|a| a.then);
        self.close();
        then
    }

    pub fn refuse(&mut self) {
        self.close();
        self.crowded = false;
    }

    /// the browser shows its verifier: in (with the page to go on to) once
    /// allowed, and only the browser whose hash it matches
    pub fn finish(&mut self, verifier: &str, now: Instant) -> Finish {
        if self.crowded || !self.live(now) {
            return Finish::No;
        }
        if self.asked.as_deref() != Some(hash_of(verifier).as_str()) {
            return Finish::No;
        }
        if !self.allowed {
            return Finish::Wait;
        }
        let then = self.armed.take().map(|a| a.then).unwrap_or_default();
        self.close();
        Finish::In(then)
    }
}

pub fn cookie<'a>(headers: impl Iterator<Item = &'a str>, name: &str) -> Option<&'a str> {
    headers
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(name)?.strip_prefix('='))
}

/// Vaultwarden binds its sign-in's two halves (/identity/connect/authorize
/// and the gate's return to oidc-signin) to one browser by this cookie. It
/// is the one cookie a box may set through the app, and only the vault's.
pub const SSO_BINDING: &str = "VW_SSO_BINDING";

/// The vault's binding cookie as this browser keeps it: host-only, so on the
/// session's own v-<hex>.localhost and nowhere else, without Secure (the
/// page is plain http on this computer). Any other cookie is dropped.
pub fn binding_set_cookie(v: &str) -> Option<String> {
    let mut parts = v.split(';').map(str::trim);
    let (name, value) = parts.next()?.split_once('=')?;
    if name != SSO_BINDING || value.is_empty() || value.contains(['"', ',', ' ']) {
        return None;
    }
    let mut out = format!("{SSO_BINDING}={value}");
    for a in parts {
        let key = a.split('=').next().unwrap_or("").to_ascii_lowercase();
        if matches!(
            key.as_str(),
            "path" | "max-age" | "expires" | "samesite" | "httponly"
        ) {
            out.push_str("; ");
            out.push_str(a);
        }
    }
    Some(out)
}

/// What of a request's cookies goes on to the vault: its binding, for its
/// sign-in's own paths, and nothing of the app's
pub fn binding_cookie_for<'a>(
    path: &str,
    headers: impl Iterator<Item = &'a str>,
) -> Option<String> {
    if !path.starts_with("/identity/connect/") {
        return None;
    }
    cookie(headers, SSO_BINDING).map(|v| format!("{SSO_BINDING}={v}"))
}

/// a value good once, for a short while
#[derive(Default)]
pub struct Once(Option<(String, Instant)>);

impl Once {
    pub const fn new() -> Once {
        Once(None)
    }

    pub fn set(&mut self, v: String, now: Instant) {
        self.0 = Some((v, now));
    }

    pub fn take(&mut self, now: Instant) -> Option<String> {
        let (v, made) = self.0.take()?;
        (now.duration_since(made) < KEY_LIFE).then_some(v)
    }
}

/// A browser the member let in, remembered until they forget it (Settings
/// in the app's window, or signing out in that browser). Its cookie on
/// this device's own names is `<id>.<secret>`, one secret for each
/// service; only their hashes are kept here.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Browser {
    pub id: String,
    /// what it said it was when let in ("Chrome on Linux")
    pub name: String,
    /// seconds since 1970
    pub made: u64,
    pub seen: u64,
    #[serde(default)]
    secrets: HashMap<String, String>,
}

/// A door: once, for a minute, into one service for one browser
struct Door {
    id: String,
    which: Which,
    then: String,
    pw: Option<String>,
    made: Instant,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    device: String,
    browsers: Vec<Browser>,
}

/// The browsers this device let in, kept beside its other state
/// (browsers.json, 0600) for the device they were let in by: a new device
/// key starts with none.
#[derive(Default)]
pub struct Browsers {
    device: String,
    list: Vec<Browser>,
    doors: HashMap<String, Door>,
    keys: HashMap<(String, &'static str), Once>,
    /// where a browser just let in goes once its site cookie is set
    pending: HashMap<(String, &'static str), (String, Instant)>,
}

fn secret_hash(s: &str) -> String {
    hash_of(s)
}

/// equal, in a time that does not say how much of it was
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

impl Browsers {
    /// what was kept, if it is this device's
    pub fn from_json(device: &str, kept: &str) -> Browsers {
        let list = serde_json::from_str::<Kept>(kept)
            .ok()
            .filter(|k| k.device == device)
            .map(|k| k.browsers)
            .unwrap_or_default();
        Browsers {
            device: device.to_string(),
            list,
            ..Default::default()
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&Kept {
            device: self.device.clone(),
            browsers: self.list.clone(),
        })
        .unwrap_or_default()
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn list(&self) -> &[Browser] {
        &self.list
    }

    /// one just let in by the member's click: its id
    pub fn add(&mut self, name: &str, now: u64) -> String {
        if self.list.len() >= MAX_BROWSERS
            && let Some(i) = self
                .list
                .iter()
                .enumerate()
                .min_by_key(|(_, b)| b.seen)
                .map(|(i, _)| i)
        {
            let gone = self.list.remove(i);
            self.keys.retain(|(id, _), _| *id != gone.id);
        }
        let id = random_hex(16);
        self.list.push(Browser {
            id: id.clone(),
            name: name.chars().take(64).collect(),
            made: now,
            seen: now,
            secrets: HashMap::new(),
        });
        id
    }

    /// A door into `which` for a browser already let in: the one-time
    /// secret its address carries
    pub fn door(
        &mut self,
        id: &str,
        which: Which,
        then: &str,
        pw: Option<String>,
        now: Instant,
    ) -> Option<String> {
        if !self.list.iter().any(|b| b.id == id) {
            return None;
        }
        self.doors
            .retain(|_, d| now.duration_since(d.made) < DOOR_LIFE);
        if self.doors.len() >= MAX_DOORS {
            return None;
        }
        let k = random_hex(32);
        self.doors.insert(
            k.clone(),
            Door {
                id: id.to_string(),
                which,
                then: then.to_string(),
                pw,
                made: now,
            },
        );
        Some(k)
    }

    /// The door, once and in time, for its own service: the browser's new
    /// cookie there, and the page after
    pub fn enter(&mut self, which: Which, k: &str, now: Instant) -> Option<(String, String)> {
        let d = self.doors.remove(k)?;
        if d.which != which || now.duration_since(d.made) >= DOOR_LIFE {
            return None;
        }
        let b = self.list.iter_mut().find(|b| b.id == d.id)?;
        let secret = random_hex(32);
        b.secrets
            .insert(which.name().to_string(), secret_hash(&secret));
        if let Some(pw) = d.pw {
            self.set_key(which, &d.id, pw, now);
        }
        Some((format!("{}.{secret}", d.id), d.then))
    }

    /// which browser a cookie on `which`'s own name is, if any
    pub fn check(&self, which: Which, cookie: Option<&str>) -> Option<String> {
        let (id, secret) = cookie?.split_once('.')?;
        let b = self.list.iter().find(|b| b.id == id)?;
        let want = b.secrets.get(which.name())?;
        same(want, &secret_hash(secret)).then(|| b.id.clone())
    }

    /// seen now; whether that is worth writing down (an hour since)
    pub fn seen(&mut self, id: &str, now: u64) -> bool {
        match self.list.iter_mut().find(|b| b.id == id) {
            Some(b) if now.saturating_sub(b.seen) >= 3600 => {
                b.seen = now;
                true
            }
            _ => false,
        }
    }

    pub fn set_key(&mut self, which: Which, id: &str, pw: String, now: Instant) {
        let mut o = Once::new();
        o.set(pw, now);
        self.keys.insert((id.to_string(), which.name()), o);
    }

    pub fn take_key(&mut self, which: Which, id: &str, now: Instant) -> Option<String> {
        self.keys
            .get_mut(&(id.to_string(), which.name()))?
            .take(now)
    }

    /// the page a browser just let in goes on to, once in (resume)
    pub fn pending(&mut self, id: &str, which: Which, then: &str, now: Instant) {
        self.pending
            .retain(|_, (_, made)| now.duration_since(*made) < DOOR_LIFE);
        self.pending
            .insert((id.to_string(), which.name()), (then.to_string(), now));
    }

    pub fn take_pending(&mut self, id: &str, which: Which, now: Instant) -> Option<String> {
        let (then, made) = self.pending.remove(&(id.to_string(), which.name()))?;
        (now.duration_since(made) < DOOR_LIFE).then_some(then)
    }

    /// forgotten: its cookies open nothing from now on
    pub fn forget(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|b| b.id != id);
        self.keys.retain(|(i, _), _| i != id);
        self.pending.retain(|(i, _), _| i != id);
        self.doors.retain(|_, d| d.id != id);
        self.list.len() != before
    }
}

/// "Chrome on Linux", from what a browser says it is: what the member sees
/// in the app's list of browsers
pub fn browser_name(agent: &str) -> String {
    let browser = [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("Chromium/", "Chromium"),
        ("Safari/", "Safari"),
    ]
    .iter()
    .find(|(k, _)| agent.contains(k))
    .map_or("A browser", |(_, v)| v);
    let os = [
        ("Android", "Android"),
        ("Windows", "Windows"),
        ("iPhone", "iPhone"),
        ("Mac OS X", "macOS"),
        ("CrOS", "ChromeOS"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(k, _)| agent.contains(k))
    .map(|(_, v)| *v);
    match os {
        Some(os) => format!("{browser} on {os}"),
        None => browser.to_string(),
    }
}

/// this device's own name for `w`'s let-in browsers, with its port: what a
/// pairing page kept, to go there itself next time
pub fn host_port(w: Which, label: &str) -> String {
    format!("{}:{}", host_of(w, label), w.port())
}

/// the door a browser let in goes through, on this device's own name
pub fn session_door(w: Which, label: &str, k: &str) -> String {
    format!(
        "http://{}:{}/_dd/app/enter?k={k}",
        host_of(w, label),
        w.port()
    )
}

/// The door's answer: a page that goes on by itself, so the next request
/// starts from this origin's own page and carries the Strict cookie.
pub fn enter_page(then: &str) -> String {
    let t = then
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\">\
<meta http-equiv=\"refresh\" content=\"0;url={t}\"><title>Commonty</title><a href=\"{t}\">Continue</a>"
    )
}

/// Opens the browser only for a port this app holds: if anything else had
/// it, the browser would be asking that for its way in.
pub fn door_if_up(up: bool, port: u16) -> Result<(), String> {
    if up {
        Ok(())
    } else {
        Err(format!(
            "something else on this computer holds port {port}, so this app will not open its page there"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABEL: &str = "0123456789abcdef0123456789abcdef";

    fn names() -> Names {
        Names::new(LABEL).unwrap()
    }

    fn paired(now: Instant) -> (Pairing, String) {
        let mut p = Pairing::default();
        p.arm("/_dd/photos", now);
        let v = "the browser's own verifier".to_string();
        p.ask(&hash_of(&v), now).unwrap();
        (p, v)
    }

    #[test]
    fn the_vaults_sign_in_binding_and_only_it_passes() {
        let set = "VW_SSO_BINDING=abc_-9; Path=/identity/connect/; Max-Age=600; SameSite=Lax; HttpOnly; Secure; Domain=vault.example.org";
        assert_eq!(
            binding_set_cookie(set).as_deref(),
            Some(
                "VW_SSO_BINDING=abc_-9; Path=/identity/connect/; Max-Age=600; SameSite=Lax; HttpOnly"
            )
        );
        for other in [
            "dd_browser=x; Path=/; HttpOnly",
            "VW_SSO_BINDINGX=y",
            "VW_SSO_BINDING=",
            "session=VW_SSO_BINDING=z",
        ] {
            assert_eq!(binding_set_cookie(other), None, "{other}");
        }
        let sent = ["dd_browser=secret; VW_SSO_BINDING=abc"];
        assert_eq!(
            binding_cookie_for("/identity/connect/oidc-signin", sent.iter().copied()).as_deref(),
            Some("VW_SSO_BINDING=abc")
        );
        assert_eq!(binding_cookie_for("/api/sync", sent.iter().copied()), None);
        assert_eq!(
            binding_cookie_for(
                "/identity/connect/token",
                ["dd_browser=secret"].iter().copied()
            ),
            None
        );
    }

    #[test]
    fn the_launched_url_holds_no_secret() {
        let n = names();
        assert_eq!(
            browser_url(&n, Which::Photos),
            format!("http://s-{LABEL}.localhost:41660/_dd/app/pair?service=photos")
        );
        let u = browser_url(&n, Which::Vault);
        assert_eq!(
            u,
            format!("http://s-{LABEL}.localhost:41660/_dd/app/pair?service=vault")
        );
        assert!(!u.contains(crate::site::launch_secret()));
    }

    #[test]
    fn a_request_without_the_members_click_gets_nothing() {
        let now = Instant::now();
        let (mut p, v) = paired(now);
        assert_eq!(p.finish(&v, now), Finish::Wait);
        assert_eq!(p.finish(&v, now), Finish::Wait);
        let c = code(&hash_of(&v)).unwrap();
        assert_eq!(p.view(now), View::Asking(c.clone()));
        p.allow(&c, now).unwrap();
        assert_eq!(p.finish(&v, now), Finish::In("/_dd/photos".into()));
        assert_eq!(p.finish(&v, now), Finish::No, "once");
    }

    #[test]
    fn an_allowed_request_is_not_let_in_without_its_verifier() {
        let now = Instant::now();
        let (mut p, v) = paired(now);
        p.allow(&code(&hash_of(&v)).unwrap(), now).unwrap();
        assert_eq!(p.finish("someone else's guess", now), Finish::No);
        assert_eq!(
            p.finish(&hash_of(&v), now),
            Finish::No,
            "the hash is not the verifier"
        );
        assert_eq!(p.finish(&v, now), Finish::In("/_dd/photos".into()));
    }

    #[test]
    fn two_requests_at_once_are_both_refused() {
        let now = Instant::now();
        let (mut p, v) = paired(now);
        let other = "a local program's verifier";
        assert!(p.ask(&hash_of(other), now).is_err());
        assert_eq!(p.view(now), View::Crowded);
        assert!(p.allow(&code(&hash_of(&v)).unwrap(), now).is_err());
        assert!(p.allow(&code(&hash_of(other)).unwrap(), now).is_err());
        assert_eq!(p.finish(&v, now), Finish::No);
        assert_eq!(p.finish(other, now), Finish::No);
        assert!(
            p.ask(&hash_of(&v), now).is_err(),
            "closed until the next link"
        );
        p.arm("/", now);
        assert_eq!(p.view(now), View::Waiting);
    }

    #[test]
    fn a_second_request_after_the_click_still_refuses_both() {
        let now = Instant::now();
        let (mut p, v) = paired(now);
        p.allow(&code(&hash_of(&v)).unwrap(), now).unwrap();
        assert!(p.ask(&hash_of("late"), now).is_err());
        assert_eq!(p.finish(&v, now), Finish::No);
    }

    #[test]
    fn the_click_is_for_the_code_shown() {
        let now = Instant::now();
        let (mut p, v) = paired(now);
        let c = code(&hash_of(&v)).unwrap();
        let wrong = if c == "000000" { "000001" } else { "000000" };
        assert!(p.allow(wrong, now).is_err());
        assert_eq!(p.finish(&v, now), Finish::Wait);
    }

    #[test]
    fn nothing_pairs_without_a_link_or_after_it_is_old() {
        let now = Instant::now();
        let mut p = Pairing::default();
        assert!(p.ask(&hash_of("v"), now).is_err());
        assert_eq!(p.view(now), View::Idle);
        p.arm("/", now);
        assert!(p.ask(&hash_of("v"), now + PAIR_LIFE).is_err());
        let (mut p, v) = paired(now);
        p.allow(&code(&hash_of(&v)).unwrap(), now).unwrap();
        assert_eq!(p.finish(&v, now + PAIR_LIFE), Finish::No);
        p.refuse();
        assert_eq!(p.view(now), View::Idle);
    }

    #[test]
    fn a_request_is_a_hash_and_its_code_is_six_digits() {
        assert!(Pairing::default().ask("not hex", Instant::now()).is_err());
        let h = hash_of("x");
        assert_eq!(h.len(), 64);
        let c = code(&h).unwrap();
        assert_eq!(c.len(), 6);
        assert_eq!(
            code("0000000a00000000000000000000000000000000000000000000000000000000").as_deref(),
            Some("000010")
        );
        assert_eq!(
            code("ffffffff00000000000000000000000000000000000000000000000000000000").as_deref(),
            Some("967295")
        );
        assert_eq!(code("FFFF"), None);
    }

    fn let_in(
        b: &mut Browsers,
        w: Which,
        then: &str,
        pw: Option<&str>,
        now: Instant,
    ) -> (String, String) {
        let id = b.add("Chrome on Linux", 1000);
        let k = b.door(&id, w, then, pw.map(Into::into), now).unwrap();
        let (c, _) = b.enter(w, &k, now).unwrap();
        (id, c)
    }

    #[test]
    fn a_door_opens_once_in_time_for_its_service() {
        let now = Instant::now();
        let mut b = Browsers::from_json("dev", "");
        let id = b.add("Firefox on Windows", 1000);
        assert!(b.door("not one", Which::Photos, "/", None, now).is_none());
        let k = b
            .door(&id, Which::Photos, "/_dd/photos", None, now)
            .unwrap();
        assert!(b.enter(Which::Photos, "wrong", now).is_none());
        assert!(
            b.enter(Which::Vault, &k, now).is_none(),
            "another service's"
        );
        assert!(
            b.enter(Which::Photos, &k, now).is_none(),
            "spent by that try"
        );
        let k = b
            .door(&id, Which::Photos, "/_dd/photos", None, now)
            .unwrap();
        assert!(b.enter(Which::Photos, &k, now + DOOR_LIFE).is_none());
        let k = b.door(&id, Which::Vault, "/#/vault", None, now).unwrap();
        let (c, then) = b.enter(Which::Vault, &k, now).unwrap();
        assert_eq!(then, "/#/vault");
        assert_eq!(
            b.check(Which::Vault, Some(&c)).as_deref(),
            Some(id.as_str())
        );
        assert_eq!(b.check(Which::Photos, Some(&c)), None, "its own service's");
        assert_eq!(b.check(Which::Vault, Some(&format!("{id}.other"))), None);
        assert_eq!(b.check(Which::Vault, Some(&id)), None);
        assert_eq!(b.check(Which::Vault, None), None);
        assert!(b.enter(Which::Vault, &k, now).is_none(), "once");
    }

    #[test]
    fn the_photos_password_is_handed_once_and_only_to_its_browser() {
        let now = Instant::now();
        let mut b = Browsers::from_json("dev", "");
        let id = b.add("Chrome on Linux", 1000);
        let k = b
            .door(&id, Which::Photos, "/_dd/photos", Some("pw".into()), now)
            .unwrap();
        assert_eq!(
            b.take_key(Which::Photos, &id, now),
            None,
            "not before the door"
        );
        b.enter(Which::Photos, &k, now).unwrap();
        let (other, _) = let_in(&mut b, Which::Photos, "/", None, now);
        assert_eq!(b.take_key(Which::Photos, &other, now), None);
        assert_eq!(b.take_key(Which::Vault, &id, now), None);
        assert_eq!(b.take_key(Which::Photos, &id, now).as_deref(), Some("pw"));
        assert_eq!(b.take_key(Which::Photos, &id, now), None);
    }

    #[test]
    fn the_photos_password_waits_only_a_minute() {
        let now = Instant::now();
        let mut b = Browsers::from_json("dev", "");
        let (id, _) = let_in(&mut b, Which::Photos, "/", Some("pw"), now);
        assert_eq!(b.take_key(Which::Photos, &id, now + KEY_LIFE), None);
    }

    #[test]
    fn browsers_are_capped_and_the_longest_unseen_goes() {
        let mut b = Browsers::from_json("dev", "");
        let first = b.add("old", 1);
        for i in 0..MAX_BROWSERS {
            let id = b.add("newer", 100 + i as u64);
            b.seen(&id, 100_000);
        }
        assert_eq!(b.list().len(), MAX_BROWSERS);
        assert!(!b.list().iter().any(|x| x.id == first));
    }

    /// Pair once: a browser let in is still let in after the app restarts
    /// (what is kept is read back), with the same cookie, at the same name
    #[test]
    fn a_browser_let_in_is_remembered_across_restarts() {
        let now = Instant::now();
        let mut b = Browsers::from_json("dev", "");
        let (id, c) = let_in(&mut b, Which::Site, "/_dd/files", None, now);
        let kept = b.to_json();
        assert!(
            !kept.contains(c.split('.').nth(1).unwrap()),
            "only the hash is kept"
        );
        let again = Browsers::from_json("dev", &kept);
        assert_eq!(
            again.check(Which::Site, Some(&c)).as_deref(),
            Some(id.as_str())
        );
        // another device key (signed in again as a new device) starts afresh
        assert!(Browsers::from_json("other", &kept).list().is_empty());
        assert!(Browsers::from_json("dev", "not json").list().is_empty());
        assert_eq!(own_label(b"seed"), own_label(b"seed"));
        assert_ne!(own_label(b"seed"), own_label(b"another"));
        assert!(valid_label(&own_label(b"seed")));
    }

    #[test]
    fn a_forgotten_browser_opens_nothing() {
        let now = Instant::now();
        let mut b = Browsers::from_json("dev", "");
        let (id, c) = let_in(&mut b, Which::Site, "/", None, now);
        let (kept, _) = let_in(&mut b, Which::Site, "/", None, now);
        let k = b
            .door(&id, Which::Photos, "/", Some("pw".into()), now)
            .unwrap();
        assert!(b.forget(&id));
        assert!(!b.forget(&id));
        assert_eq!(b.check(Which::Site, Some(&c)), None);
        assert!(
            b.enter(Which::Photos, &k, now).is_none(),
            "its doors go too"
        );
        assert!(b.door(&id, Which::Vault, "/", None, now).is_none());
        assert!(b.list().iter().any(|x| x.id == kept), "the others stay");
        let back = Browsers::from_json("dev", &b.to_json());
        assert_eq!(back.check(Which::Site, Some(&c)), None);
    }

    #[test]
    fn a_browser_let_in_before_takes_the_links_page_and_nothing_is_left_to_ask() {
        let now = Instant::now();
        let mut p = Pairing::default();
        assert_eq!(p.take_then(now), None, "no link, no page");
        p.arm("/_dd/files", now);
        assert_eq!(p.take_then(now).as_deref(), Some("/_dd/files"));
        assert!(p.ask(&hash_of("v"), now).is_err(), "the link is spent");
        p.arm("/_dd/files", now);
        assert_eq!(p.take_then(now + PAIR_LIFE), None);
    }

    #[test]
    fn a_browser_is_named_for_the_member() {
        let chrome = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
        assert_eq!(browser_name(chrome), "Chrome on Linux");
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0";
        assert_eq!(browser_name(edge), "Edge on Windows");
        let ff = "Mozilla/5.0 (Android 15; Mobile; rv:143.0) Gecko/143.0 Firefox/143.0";
        assert_eq!(browser_name(ff), "Firefox on Android");
        assert_eq!(browser_name(""), "A browser");
    }

    #[test]
    fn only_the_exact_host_and_port_are_answered() {
        let n = names();
        let own = own_label(b"this device");
        let p = Which::Photos;
        let o = Some(own.as_str());
        assert_eq!(via(Some("127.0.0.1:41661"), p, &n, o), Some(Via::Window));
        assert_eq!(
            via(Some(&format!("p-{LABEL}.localhost:41661")), p, &n, o),
            Some(Via::Pair)
        );
        assert_eq!(
            via(Some(&format!("p-{own}.localhost:41661")), p, &n, o),
            Some(Via::Browser)
        );
        assert_eq!(
            via(Some(&format!("p-{own}.localhost:41661")), p, &n, None),
            None,
            "no device key, no browsers"
        );
        for h in [
            None,
            Some("127.0.0.1".to_string()),
            Some("127.0.0.1:41662".into()),
            Some("localhost:41661".into()),
            Some("evil.example:41661".into()),
            Some(format!("p-{LABEL}.localhost")),
            Some(format!("p-{LABEL}.localhost:41662")),
            Some(format!("v-{LABEL}.localhost:41661")),
            Some(format!("v-{own}.localhost:41661")),
            Some(format!("p-{own}.localhost:41662")),
            Some("P-0123456789ABCDEF0123456789ABCDEF.localhost:41661".into()),
            Some(format!("x.p-{LABEL}.localhost:41661")),
            Some(format!("x.p-{own}.localhost:41661")),
            Some(format!("p-{}.localhost:41661", "f".repeat(32))),
            Some("[::1]:41661".into()),
        ] {
            assert_eq!(via(h.as_deref(), p, &n, o), None, "{h:?}");
        }
        assert_eq!(
            via(
                Some(&format!("v-{LABEL}.localhost:41662")),
                Which::Vault,
                &n,
                o
            ),
            Some(Via::Pair)
        );
    }

    #[test]
    fn the_devices_own_name_is_not_the_install_name() {
        let own = own_label(b"seed");
        assert!(valid_label(&own) && own != LABEL);
        assert_eq!(
            session_door(Which::Photos, &own, "k"),
            format!("http://p-{own}.localhost:41661/_dd/app/enter?k=k")
        );
        assert_eq!(
            host_port(Which::Site, &own),
            format!("s-{own}.localhost:41660")
        );
    }

    #[test]
    fn names_are_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("commonty-names-{}", random_hex(8)));
        let f = dir.join("local-names");
        let a = Names::load(&f).unwrap();
        let b = Names::load(&f).unwrap();
        assert_eq!(a, b);
        assert!(valid_label(&a.label));
        assert!(a.host(Which::Photos).ends_with(".localhost"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
        std::fs::write(&f, "not a label").unwrap();
        let c = Names::load(&f).unwrap();
        assert_ne!(c, a);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(Names::new("0123").is_none());
        assert!(Names::new("0123456789ABCDEF0123456789ABCDEF").is_none());
    }

    #[test]
    fn then_is_a_path_on_this_origin_only() {
        for ok in [
            "/",
            "/_dd/photos",
            "/gallery?x=1&y=2",
            "/#/vault",
            "/#/send/a/b",
        ] {
            assert_eq!(safe_then(ok), Some(ok), "{ok}");
        }
        for bad in [
            "",
            "gallery",
            "//evil.example",
            "/\\evil.example",
            "https://evil.example",
            "javascript:alert(1)",
            "/\"><script>",
            "/a b",
            "/a\nb",
            "/a'b",
            "/a;url=https://evil.example",
        ] {
            assert_eq!(safe_then(bad), None, "{bad:?}");
        }
        assert_eq!(safe_then(&"/a".repeat(300)), None);
    }

    #[test]
    fn the_door_page_goes_on_to_its_path_and_nowhere_else() {
        let p = enter_page("/gallery?a=1&b=2");
        assert!(p.contains("content=\"0;url=/gallery?a=1&amp;b=2\""), "{p}");
        assert!(p.contains("no-referrer"));
    }

    #[test]
    fn a_link_names_a_service_and_a_path_on_it() {
        assert_eq!(
            parse_link("commonty://open?service=photos"),
            Some(Link {
                which: Which::Photos,
                path: None
            })
        );
        assert_eq!(
            parse_link("commonty://open/?service=vault&path=%2F%23%2Fsend%2Fx"),
            Some(Link {
                which: Which::Vault,
                path: Some("/#/send/x".into())
            })
        );
        for bad in [
            "commonty://open?service=files",
            "commonty://open",
            "commonty://other?service=photos",
            "https://open?service=photos",
            "commonty://open?service=photos&path=https%3A%2F%2Fevil.example",
            "commonty://open?service=photos&path=%2F%2Fevil.example",
            "commonty://open?service=vault&path=%2F%22%3E",
        ] {
            assert_eq!(parse_link(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_link_goes_to_the_service_own_pages() {
        let l = |which, path: Option<&str>| Link {
            which,
            path: path.map(Into::into),
        };
        assert_eq!(then_for(&l(Which::Photos, None)), "/_dd/photos");
        assert_eq!(
            then_for(&l(Which::Photos, Some("/gallery"))),
            "/_dd/photos?to=%2Fgallery"
        );
        assert_eq!(
            then_for(&l(Which::Photos, Some("/_dd/app/photos-key"))),
            "/_dd/photos"
        );
        assert_eq!(then_for(&l(Which::Vault, Some("/#/send/x"))), "/#/send/x");
        assert_eq!(then_for(&l(Which::Vault, Some("/_dd/app/enter"))), "/");
    }

    #[test]
    fn no_browser_for_a_port_this_app_does_not_hold() {
        assert!(door_if_up(false, 41661).is_err());
        assert!(door_if_up(true, 41661).is_ok());
    }

    #[test]
    fn changes_come_only_from_this_origin() {
        let own = format!("http://p-{LABEL}.localhost:41661");
        let own = own.as_str();
        assert!(same_origin(
            "POST",
            Some(own),
            Some("same-origin"),
            own,
            false
        ));
        assert!(same_origin("GET", None, None, own, false));
        assert!(same_origin("GET", None, Some("none"), own, false));
        assert!(!same_origin("POST", None, None, own, false), "no origin");
        assert!(!same_origin(
            "POST",
            Some("http://evil.example"),
            None,
            own,
            false
        ));
        assert!(!same_origin("POST", Some("null"), None, own, false));
        assert!(!same_origin(
            "GET",
            Some("http://127.0.0.1:41661"),
            None,
            own,
            false
        ));
        assert!(!same_origin("GET", None, Some("same-site"), own, false));
        assert!(!same_origin("GET", None, Some("cross-site"), own, false));
        let w = "http://127.0.0.1:41661";
        assert!(same_origin("POST", None, None, w, true));
        assert!(same_origin("GET", None, Some("cross-site"), w, true));
        assert!(!same_origin("POST", Some(own), None, w, true));
    }

    #[test]
    fn only_a_browsers_ways_in_come_without_a_cookie() {
        let b = Via::Browser;
        assert!(way_in(Which::Vault, &b, "GET", "/sso-connector.html"));
        assert!(way_in(Which::Photos, &b, "GET", "/_dd/app/enter"));
        assert!(way_in(Which::Vault, &b, "GET", "/_dd/app/enter"));
        assert!(way_in(Which::Site, &b, "GET", "/_dd/app/resume"));
        assert!(!way_in(Which::Site, &b, "POST", "/_dd/app/resume"));
        assert!(!way_in(Which::Site, &Via::Pair, "GET", "/_dd/app/resume"));
        assert!(!way_in(Which::Vault, &b, "POST", "/sso-connector.html"));
        assert!(!way_in(Which::Vault, &b, "GET", "/sso-connector.html/x"));
        assert!(!way_in(Which::Vault, &b, "GET", "/api/sync"));
        assert!(!way_in(Which::Photos, &b, "GET", "/sso-connector.html"));
        assert!(!way_in(
            Which::Vault,
            &Via::Window,
            "GET",
            "/sso-connector.html"
        ));
        assert!(!way_in(Which::Vault, &Via::Pair, "GET", "/_dd/app/enter"));
    }

    #[test]
    fn a_link_can_ask_for_the_sites_own_pages_and_nothing_else() {
        let l = |u: &str| parse_link(u).map(|l| then_for(&l));
        assert_eq!(
            l("commonty://open?service=site&path=%2F_dd%2Ffiles").as_deref(),
            Some("/_dd/files")
        );
        assert_eq!(
            l("commonty://open?service=site&path=%2F_dd%2Fsettings%23backups").as_deref(),
            Some("/_dd/settings#backups")
        );
        // Mail too: its link was refused, so it never opened in a browser
        assert_eq!(
            l("commonty://open?service=site&path=%2F_dd%2Fmail").as_deref(),
            Some("/_dd/mail")
        );
        assert_eq!(
            l("commonty://open?service=site").as_deref(),
            Some("/_dd/home")
        );
        for bad in [
            "commonty://open?service=site&path=%2F_dd%2Fapp%2Flibrary",
            "commonty://open?service=site&path=%2F",
            "commonty://open?service=site&path=%2F_dd%2Ffiles%2F..",
            "commonty://open?service=site&path=https%3A%2F%2Fevil.example",
        ] {
            assert_eq!(parse_link(bad), None, "{bad}");
        }
    }

    /// The library's key, the archive password and the Photos password are
    /// behind `Serve`, and so behind the cookie a browser gets only after
    /// the member let it in from the app's window.
    #[test]
    fn keys_are_served_only_to_a_browser_the_member_let_in() {
        let own = format!("http://s-{LABEL}.localhost:41660");
        let b = Via::Browser;
        let ask = |via: &Via, cookie_ok, method, origin, site, nav| {
            admit(
                Which::Site,
                via,
                cookie_ok,
                method,
                "/_dd/app/library",
                origin,
                site,
                nav,
                &own,
            )
        };
        assert_eq!(
            ask(&b, true, "GET", None, Some("same-origin"), false),
            Admit::Serve
        );
        // the pairing name serves its page and two asks, never this
        assert_eq!(
            ask(&Via::Pair, true, "GET", None, None, false),
            Admit::Pairing
        );
        assert!(matches!(
            ask(&b, false, "GET", None, Some("same-origin"), false),
            Admit::Refuse(403, _)
        ));
        assert!(matches!(
            ask(&b, true, "GET", None, Some("cross-site"), false),
            Admit::Refuse(403, _)
        ));
        // opened in a tab from elsewhere: a page that opens itself again,
        // and no key
        assert_eq!(
            ask(&b, true, "GET", None, Some("cross-site"), true),
            Admit::GoOn
        );
        assert!(matches!(
            ask(&b, true, "POST", Some("http://evil.example"), None, false),
            Admit::Refuse(403, _)
        ));
        assert!(matches!(
            ask(&Via::Window, true, "GET", None, None, false),
            Admit::Refuse(404, _)
        ));
        // its door is open without the cookie, and spends a one-time secret
        assert_eq!(
            admit(
                Which::Site,
                &b,
                false,
                "GET",
                "/_dd/app/enter",
                None,
                None,
                false,
                &own
            ),
            Admit::Door
        );
        // the window's servers: its cookie, or nothing
        let w = "http://127.0.0.1:41661";
        let win = |ok| {
            admit(
                Which::Photos,
                &Via::Window,
                ok,
                "POST",
                "/_dd/app/photos-key",
                None,
                None,
                false,
                w,
            )
        };
        assert_eq!(win(true), Admit::Serve);
        assert!(matches!(win(false), Admit::Refuse(403, _)));
    }

    /// A tab opened at an app-served page from a link, a bookmark or the
    /// pairing page is sent on to itself, so it arrives again from its own
    /// origin with its cookie. Nothing else from another site gets past:
    /// not a frame, not a fetch, not a form, not a change.
    #[test]
    fn a_page_opened_from_elsewhere_opens_itself_and_nothing_more() {
        let own = format!("http://s-{LABEL}.localhost:41660");
        let at = |via: &Via, method, site, nav| {
            admit(
                Which::Site,
                via,
                false,
                method,
                "/_dd/media",
                None,
                site,
                nav,
                &own,
            )
        };
        for site in ["cross-site", "same-site"] {
            assert_eq!(at(&Via::Browser, "GET", Some(site), true), Admit::GoOn);
            assert_eq!(at(&Via::Pair, "GET", Some(site), true), Admit::Pairing);
            assert!(matches!(
                at(&Via::Browser, "GET", Some(site), false),
                Admit::Refuse(403, _)
            ));
            assert!(matches!(
                at(&Via::Browser, "POST", Some(site), true),
                Admit::Refuse(403, _)
            ));
            assert!(matches!(
                at(&Via::Pair, "POST", Some(site), true),
                Admit::Refuse(403, _)
            ));
        }
        // a let-in browser's tab comes back from its own page: the page,
        // if its cookie is good
        assert!(matches!(
            at(&Via::Browser, "GET", Some("same-origin"), true),
            Admit::Refuse(403, _)
        ));
        assert_eq!(
            admit(
                Which::Site,
                &Via::Browser,
                true,
                "GET",
                "/_dd/media",
                None,
                Some("same-origin"),
                true,
                &own
            ),
            Admit::Serve
        );
    }

    #[test]
    fn cookies_are_read_by_exact_name() {
        let h = ["a=1; dd_browser=xyz", "dd_browser_x=no"];
        assert_eq!(cookie(h.iter().copied(), BROWSER_COOKIE), Some("xyz"));
        assert_eq!(
            cookie(["dd_browserx=1"].iter().copied(), BROWSER_COOKIE),
            None
        );
    }
}
