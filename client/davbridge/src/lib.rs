//! A member's calendars and address books as CalDAV and CardDAV, served on
//! this computer alone (127.0.0.1) to the programs already on it:
//! Thunderbird, GNOME's and Apple's calendars and contacts, DAVx5 on a
//! phone. Every event and person is read and sealed here (store.rs,
//! gate.rs) and the box keeps only what it cannot read; this speaks the
//! protocols those programs know over it.
//!
//! ```text
//! /.well-known/caldav, /.well-known/carddav, /dav/   the member
//! /dav/calendars/              their calendars, own and shared
//! /dav/calendars/<id>/         one: its events, by file
//! /dav/calendars/<id>/<file>   one event (a VCALENDAR with its changed occurrences)
//! /dav/contacts/...            the same for address books and people
//! ```
//!
//! A program signs in as `commonty` with this computer's own password for it,
//! by Digest (RFC 7616, SHA-256 or MD5): the password itself never crosses,
//! so whatever answers on the port while the app is closed learns only a
//! digest of a long random one. Only requests to 127.0.0.1 or localhost on
//! the bridge's port, and never from a page in a browser (Origin, or
//! Sec-Fetch-Site other than none), are answered. Saves carry the version
//! they were made from, as the gate's own do, so two programs editing one
//! event cannot both win.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

pub mod gate;
pub mod store;
pub mod subscribe;

pub use store::{Coll, Item, Kind, Memory, Saved, Store, Version};

/// the largest event or person taken: a contact's photo fits easily
const MAX_BODY: usize = 10 << 20;
/// sync tokens kept per calendar: a program asking from an older one is
/// told to read it all again
const TOKENS_KEPT: usize = 16;

/// per calendar, the versions each recent sync token stood for, oldest first
type Syncs = HashMap<(Kind, String), VecDeque<(String, BTreeMap<String, String>)>>;

struct Bridge<S> {
    store: Arc<S>,
    password: String,
    /// the Host a request must name: 127.0.0.1:<port> or localhost:<port>
    port: u16,
    /// this run's own secret, so no challenge outlives the bridge that made it
    run: String,
    /// per challenge, the highest count a program has answered it with and
    /// when it was made: an answer heard once is never good again
    counts: Mutex<HashMap<String, (u64, u64)>>,
    /// per calendar, the versions each recent sync token stood for
    syncs: Mutex<Syncs>,
}

/// the one name a program signs in as
pub const USER: &str = "commonty";
const REALM: &str = "Commonty";
/// how long a sign-in challenge is good for
const NONCE_SECS: u64 = 600;

/// The bridge on `port`, answering every path; `password` is what a
/// program signs in with.
pub fn router<S: Store>(store: Arc<S>, password: String, port: u16) -> axum::Router {
    let run = {
        let mut r = [0u8; 32];
        getrandom::fill(&mut r).expect("the system's randomness");
        hex(&r)
    };
    let b = Arc::new(Bridge {
        store,
        password,
        port,
        run,
        counts: Mutex::new(HashMap::new()),
        syncs: Mutex::new(HashMap::new()),
    });
    axum::Router::new().fallback(handle::<S>).with_state(b)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum At {
    Principal,
    Home(Kind),
    Coll(Kind, String),
    Item(Kind, String, String),
}

fn kind_seg(s: &str) -> Option<Kind> {
    match s {
        "calendars" => Some(Kind::Calendars),
        "contacts" => Some(Kind::Contacts),
        _ => None,
    }
}
fn seg_of(k: Kind) -> &'static str {
    match k {
        Kind::Calendars => "calendars",
        Kind::Contacts => "contacts",
    }
}

fn at(path: &str) -> Option<At> {
    let p = decode(path);
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [] | [".well-known", _] | ["dav"] => Some(At::Principal),
        ["dav", k] => Some(At::Home(kind_seg(k)?)),
        ["dav", k, c] if good_name(c) => Some(At::Coll(kind_seg(k)?, c.to_string())),
        ["dav", k, c, f] if good_name(c) && good_name(f) => {
            Some(At::Item(kind_seg(k)?, c.to_string(), f.to_string()))
        }
        _ => None,
    }
}

/// a name that is one path segment and nothing more
pub(crate) fn good_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains(['/', '\\', '\0'])
}

fn href(a: &At) -> String {
    match a {
        At::Principal => "/dav/".into(),
        At::Home(k) => format!("/dav/{}/", seg_of(*k)),
        At::Coll(k, c) => format!("/dav/{}/{}/", seg_of(*k), encode(c)),
        At::Item(k, c, f) => format!("/dav/{}/{}/{}", seg_of(*k), encode(c), encode(f)),
    }
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~@+".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// an href a program sent, as one of ours: a full url or a path
fn href_at(h: &str) -> Option<At> {
    let path = match h.find("://") {
        Some(i) => &h[i + 3..][h[i + 3..].find('/').unwrap_or(h.len() - i - 3)..],
        None => h,
    };
    at(path.split('?').next().unwrap_or(path))
}

/// What a request's body asks, by local names: its root, the properties
/// named, the hrefs, a sync token, the components a query filters on, and
/// a name or colour being set.
#[derive(Debug, Default)]
struct Ask {
    root: String,
    props: Vec<String>,
    allprop: bool,
    hrefs: Vec<String>,
    sync_token: Option<String>,
    comps: Vec<String>,
    displayname: Option<String>,
    color: Option<String>,
}

fn read_ask(body: &[u8]) -> Ask {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_reader(body);
    let mut ask = Ask::default();
    let mut stack: Vec<String> = Vec::new();
    let mut text = String::new();
    let mut buf = Vec::new();
    let local = |n: &str| n.rsplit(':').next().unwrap_or(n).to_string();
    loop {
        match r.read_event_into(&mut buf) {
            Ok(ev @ (Event::Start(_) | Event::Empty(_))) => {
                let (e, empty) = match &ev {
                    Event::Start(e) => (e, false),
                    Event::Empty(e) => (e, true),
                    _ => unreachable!(),
                };
                let name = local(e.name().as_ref());
                if stack.is_empty() {
                    ask.root = name.clone();
                }
                if stack.last().is_some_and(|p| p == "prop") {
                    ask.props.push(name.clone());
                }
                if name == "allprop" {
                    ask.allprop = true;
                }
                if name == "comp-filter"
                    && let Some(Ok(a)) = e
                        .attributes()
                        .find(|a| a.as_ref().is_ok_and(|a| a.key.as_ref() == "name"))
                {
                    ask.comps.push(a.value.to_uppercase());
                }
                text.clear();
                // an empty element (<D:getetag/>) closes itself
                if !empty {
                    stack.push(name);
                }
            }
            Ok(Event::Text(t)) => text.push_str(&t.xml10_content()),
            Ok(Event::CData(t)) => text.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(g)) => {
                let name: &str = g.as_ref();
                text.push_str(match name {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => "",
                });
                if let Some(n) = name.strip_prefix('#') {
                    let c = match n.strip_prefix('x') {
                        Some(h) => u32::from_str_radix(h, 16).ok(),
                        None => n.parse().ok(),
                    };
                    if let Some(c) = c.and_then(char::from_u32) {
                        text.push(c);
                    }
                }
            }
            Ok(Event::End(_)) => {
                let t = std::mem::take(&mut text).trim().to_string();
                match stack.pop().as_deref() {
                    Some("href") => ask.hrefs.push(t),
                    Some("sync-token") => ask.sync_token = Some(t),
                    Some("displayname") => ask.displayname = Some(t),
                    Some("calendar-color") => ask.color = Some(t),
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    ask
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const NS: &str = r#"xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CR="urn:ietf:params:xml:ns:carddav" xmlns:CS="http://calendarserver.org/ns/" xmlns:A="http://apple.com/ns/ical/""#;

fn prefix(local: &str) -> &'static str {
    match local {
        "calendar-home-set"
        | "supported-calendar-component-set"
        | "calendar-data"
        | "calendar-description"
        | "calendar-timezone"
        | "supported-calendar-data"
        | "calendar-user-address-set"
        | "schedule-inbox-URL"
        | "schedule-outbox-URL"
        | "max-resource-size" => "C",
        "addressbook-home-set"
        | "supported-address-data"
        | "address-data"
        | "addressbook-description" => "CR",
        "getctag" => "CS",
        "calendar-color" | "calendar-order" => "A",
        _ => "D",
    }
}

fn multistatus(body: String) -> Response {
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus {NS}>{body}</D:multistatus>"
        ),
    )
        .into_response()
}

/// one resource's answer: the properties found, and those not (404)
fn response(href: &str, found: &[(String, String)], missing: &[String]) -> String {
    let mut out = format!("<D:response><D:href>{}</D:href>", xml(href));
    if !found.is_empty() {
        out.push_str("<D:propstat><D:prop>");
        for (name, value) in found {
            let p = prefix(name);
            if value.is_empty() {
                out.push_str(&format!("<{p}:{name}/>"));
            } else {
                out.push_str(&format!("<{p}:{name}>{value}</{p}:{name}>"));
            }
        }
        out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    }
    if !missing.is_empty() {
        out.push_str("<D:propstat><D:prop>");
        for name in missing {
            out.push_str(&format!("<{}:{name}/>", prefix(name)));
        }
        out.push_str("</D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>");
    }
    out.push_str("</D:response>");
    out
}

fn gone(href: &str) -> String {
    format!(
        "<D:response><D:href>{}</D:href><D:status>HTTP/1.1 404 Not Found</D:status></D:response>",
        xml(href)
    )
}

async fn handle<S: Store>(State(b): State<Arc<Bridge<S>>>, req: Request) -> Response {
    let method = req.method().clone();
    // this computer's own programs, by its own name for itself: a page in
    // a browser (an Origin, a cross-site fetch) or a name pointed here by
    // someone else's DNS is never one
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let local = [
        format!("127.0.0.1:{}", b.port),
        format!("localhost:{}", b.port),
    ];
    let from_page = req.headers().contains_key(header::ORIGIN)
        || req
            .headers()
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v != "none");
    if !local.iter().any(|h| h.eq_ignore_ascii_case(host)) || from_page {
        return (StatusCode::FORBIDDEN, "for this computer's own programs").into_response();
    }
    if method == Method::OPTIONS {
        return options();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let target = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    let secret = format!("{}\0{}", b.password, b.run);
    match signed_in(
        req.headers(),
        method.as_str(),
        &target,
        &secret,
        &b.password,
        now,
    ) {
        Ok((n, nc)) => {
            let mut counts = b.counts.lock().unwrap();
            counts.retain(|_, (_, at)| *at + NONCE_SECS >= now);
            if counts.get(&n).is_some_and(|(c, _)| nc <= *c) {
                return challenge(&secret, now, true);
            }
            let at = u64::from_str_radix(n.split('.').next().unwrap_or("0"), 16).unwrap_or(now);
            counts.insert(n, (nc, at));
        }
        Err(stale) => return challenge(&secret, now, stale),
    }
    let Some(place) = at(req.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let headers = req.headers().clone();
    let body = match axum::body::to_bytes(req.into_body(), MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let out = match method.as_str() {
        "PROPFIND" => b.propfind(&place, &headers, &body).await,
        "REPORT" => b.report(&place, &body).await,
        "GET" | "HEAD" => b.get(&place, method == Method::HEAD).await,
        "PUT" => b.put(&place, &headers, &body).await,
        "DELETE" => b.delete(&place, &headers).await,
        "MKCALENDAR" | "MKCOL" => b.make(&place, &body).await,
        "PROPPATCH" => b.proppatch(&place, &body).await,
        _ => Ok(StatusCode::METHOD_NOT_ALLOWED.into_response()),
    };
    out.unwrap_or_else(|e| (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response())
}

fn options() -> Response {
    (
        StatusCode::OK,
        [
            ("DAV", "1, 3, calendar-access, addressbook, extended-mkcol"),
            (
                "Allow",
                "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, REPORT, MKCALENDAR, MKCOL",
            ),
        ],
    )
        .into_response()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hash(algorithm: &str, s: &str) -> String {
    use sha2::Digest as _;
    if algorithm.eq_ignore_ascii_case("MD5") {
        use md5::Digest as _;
        hex(&md5::Md5::digest(s.as_bytes()))
    } else {
        hex(&sha2::Sha256::digest(s.as_bytes()))
    }
}

/// a challenge's nonce: when it was made, a part of its own, and a mac of
/// both under the password and this run's secret, so the bridge keeps
/// nothing to check one and no two challenges are the same
fn nonce(secret: &str, at: u64, own: u64) -> String {
    use hmac::Mac as _;
    let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("any key");
    m.update(format!("commonty bridge nonce\0{at}\0{own}").as_bytes());
    let mac = hex(&m.finalize().into_bytes());
    format!("{at:x}.{own:x}.{}", &mac[..32])
}

fn fresh_part() -> u64 {
    let mut r = [0u8; 8];
    getrandom::fill(&mut r).expect("the system's randomness");
    u64::from_le_bytes(r)
}

/// Ok((nonce, count)), or Err(stale): whether the answer was right for a
/// challenge that has run out, so the program asks again without asking
/// its person. `secret` is what this run's challenges are made under.
fn signed_in(
    headers: &HeaderMap,
    method: &str,
    target: &str,
    secret: &str,
    password: &str,
    now: u64,
) -> std::result::Result<(String, u64), bool> {
    use subtle::ConstantTimeEq as _;
    let Some(v) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Digest "))
    else {
        return Err(false);
    };
    let f = digest_fields(v);
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    let algorithm = if get("algorithm").is_empty() {
        "MD5"
    } else {
        get("algorithm")
    };
    if password.is_empty()
        || get("username") != USER
        || get("realm") != REALM
        || !(algorithm.eq_ignore_ascii_case("MD5") || algorithm.eq_ignore_ascii_case("SHA-256"))
    {
        return Err(false);
    }
    // the challenge is one this bridge made, under this password
    let n = get("nonce");
    let mut parts = n.split('.');
    let (Some(Ok(at)), Some(Ok(own))) = (
        parts.next().map(|t| u64::from_str_radix(t, 16)),
        parts.next().map(|t| u64::from_str_radix(t, 16)),
    ) else {
        return Err(false);
    };
    if !bool::from(nonce(secret, at, own).as_bytes().ct_eq(n.as_bytes())) {
        return Err(false);
    }
    // qop=auth alone: its count is what makes an answer good once
    if get("qop") != "auth" {
        return Err(false);
    }
    let Ok(nc) = u64::from_str_radix(get("nc"), 16) else {
        return Err(false);
    };
    // the request the answer is for: this one (a program may send the
    // whole url or the path)
    let uri = get("uri");
    let path_of = |u: &str| match u.find("://") {
        Some(i) => u[i + 3..]
            .find('/')
            .map(|j| u[i + 3 + j..].to_string())
            .unwrap_or_default(),
        None => u.to_string(),
    };
    if path_of(uri) != target {
        return Err(false);
    }
    let ha1 = hash(algorithm, &format!("{USER}:{REALM}:{password}"));
    let ha2 = hash(algorithm, &format!("{method}:{uri}"));
    let want = hash(
        algorithm,
        &format!("{ha1}:{n}:{}:{}:auth:{ha2}", get("nc"), get("cnonce")),
    );
    if !bool::from(
        want.as_bytes()
            .ct_eq(get("response").to_ascii_lowercase().as_bytes()),
    ) {
        return Err(false);
    }
    if at + NONCE_SECS < now || at > now + 60 {
        return Err(true);
    }
    Ok((n.to_string(), nc))
}

/// an Authorization: Digest header's fields, quoted or not
fn digest_fields(v: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = v.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else {
            break;
        };
        let k = rest[..eq]
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        rest = rest[eq + 1..].trim_start();
        let val;
        if let Some(r) = rest.strip_prefix('"') {
            let mut s = String::new();
            let mut chars = r.char_indices();
            let mut end = r.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            s.push(n);
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    c => s.push(c),
                }
            }
            val = s;
            rest = r[end.min(r.len())..].trim_start();
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            val = rest[..end].trim().to_string();
            rest = &rest[end..];
        }
        rest = rest.trim_start_matches(',').trim_start();
        out.insert(k, val);
    }
    out
}

fn challenge(secret: &str, now: u64, stale: bool) -> Response {
    let n = nonce(secret, now, fresh_part());
    let stale = if stale { ", stale=true" } else { "" };
    let mut r = (
        StatusCode::UNAUTHORIZED,
        "this computer's calendar password is needed",
    )
        .into_response();
    for alg in ["SHA-256", "MD5"] {
        if let Ok(v) = axum::http::HeaderValue::from_str(&format!(
            "Digest realm=\"{REALM}\", qop=\"auth\", algorithm={alg}, nonce=\"{n}\"{stale}"
        )) {
            r.headers_mut().append(header::WWW_AUTHENTICATE, v);
        }
    }
    r
}

fn privileges(writable: bool) -> String {
    let mut p = String::from("<D:privilege><D:read/></D:privilege>");
    if writable {
        p.push_str("<D:privilege><D:write/></D:privilege><D:privilege><D:write-content/></D:privilege><D:privilege><D:bind/></D:privilege><D:privilege><D:unbind/></D:privilege>");
    }
    p
}

fn content_type(k: Kind) -> &'static str {
    match k {
        Kind::Calendars => "text/calendar; charset=utf-8",
        Kind::Contacts => "text/vcard; charset=utf-8",
    }
}

/// a calendar's version as a whole: from its items' versions
fn ctag(items: &[Item]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut v: Vec<&Item> = items.iter().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    for i in v {
        h.update(i.name.as_bytes());
        h.update([0]);
        h.update(i.etag.as_bytes());
        h.update([0]);
    }
    let d = h.finalize();
    d[..12].iter().map(|b| format!("{b:02x}")).collect()
}

fn token_of(c: &str) -> String {
    format!("https://commonty.org/ns/sync/{c}")
}

impl<S: Store> Bridge<S> {
    async fn coll(&self, k: Kind, id: &str) -> anyhow::Result<Option<Coll>> {
        Ok(self
            .store
            .collections(k)
            .await?
            .into_iter()
            .find(|c| c.id == id))
    }

    /// Remember what a sync token stands for, and give it.
    fn remember(&self, k: Kind, coll: &str, items: &[Item]) -> String {
        let token = token_of(&ctag(items));
        let mut s = self.syncs.lock().unwrap();
        let q = s.entry((k, coll.to_string())).or_default();
        if !q.iter().any(|(t, _)| *t == token) {
            q.push_back((
                token.clone(),
                items
                    .iter()
                    .map(|i| (i.name.clone(), i.etag.clone()))
                    .collect(),
            ));
            while q.len() > TOKENS_KEPT {
                q.pop_front();
            }
        }
        token
    }

    /// The properties of one place, as asked: (found, missing).
    fn props(
        &self,
        place: &At,
        coll: Option<&Coll>,
        item: Option<&Item>,
        token: Option<&str>,
        asked: &[String],
    ) -> (Vec<(String, String)>, Vec<String>) {
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for name in asked {
            let v = match (name.as_str(), place) {
                ("resourcetype", At::Principal) => {
                    Some("<D:principal/><D:collection/>".to_string())
                }
                ("resourcetype", At::Home(_)) => Some("<D:collection/>".into()),
                ("resourcetype", At::Coll(Kind::Calendars, _)) => {
                    Some("<D:collection/><C:calendar/>".into())
                }
                ("resourcetype", At::Coll(Kind::Contacts, _)) => {
                    Some("<D:collection/><CR:addressbook/>".into())
                }
                ("resourcetype", At::Item(..)) => Some(String::new()),
                ("current-user-principal" | "principal-URL" | "owner", _) => {
                    Some("<D:href>/dav/</D:href>".into())
                }
                ("calendar-home-set", _) => Some("<D:href>/dav/calendars/</D:href>".into()),
                ("addressbook-home-set", _) => Some("<D:href>/dav/contacts/</D:href>".into()),
                ("displayname", At::Principal) => Some("Commonty".into()),
                ("displayname", At::Home(Kind::Calendars)) => Some("Calendars".into()),
                ("displayname", At::Home(Kind::Contacts)) => Some("Contacts".into()),
                ("displayname", At::Coll(..)) => coll.map(|c| xml(&c.name)),
                ("calendar-color", At::Coll(Kind::Calendars, _)) => coll
                    .filter(|c| !c.color.is_empty())
                    .map(|c| xml(&c.color)),
                ("calendar-description" | "addressbook-description", At::Coll(..)) => {
                    Some(String::new())
                }
                ("getctag" | "sync-token", At::Coll(..)) => token.map(|t| match name.as_str() {
                    "getctag" => xml(t.rsplit('/').next().unwrap_or(t)),
                    _ => xml(t),
                }),
                ("supported-calendar-component-set", At::Coll(Kind::Calendars, _)) => Some(
                    "<C:comp name=\"VEVENT\"/><C:comp name=\"VTODO\"/>".into(),
                ),
                ("supported-address-data", At::Coll(Kind::Contacts, _)) => Some(
                    "<CR:address-data-type content-type=\"text/vcard\" version=\"3.0\"/><CR:address-data-type content-type=\"text/vcard\" version=\"4.0\"/>".into(),
                ),
                ("supported-calendar-data", At::Coll(Kind::Calendars, _)) => Some(
                    "<C:calendar-data content-type=\"text/calendar\" version=\"2.0\"/>".into(),
                ),
                ("max-resource-size", At::Coll(..)) => Some(MAX_BODY.to_string()),
                ("supported-report-set", At::Coll(k, _)) => {
                    let (multi, query) = match k {
                        Kind::Calendars => ("C:calendar-multiget", "C:calendar-query"),
                        Kind::Contacts => ("CR:addressbook-multiget", "CR:addressbook-query"),
                    };
                    Some(format!(
                        "<D:supported-report><D:report><{multi}/></D:report></D:supported-report><D:supported-report><D:report><{query}/></D:report></D:supported-report><D:supported-report><D:report><D:sync-collection/></D:report></D:supported-report>"
                    ))
                }
                ("current-user-privilege-set", At::Coll(..)) => {
                    Some(privileges(coll.is_some_and(|c| c.writable)))
                }
                ("current-user-privilege-set", At::Item(..)) => {
                    Some(privileges(coll.is_some_and(|c| c.writable)))
                }
                ("current-user-privilege-set", _) => Some(privileges(false)),
                ("getetag", At::Item(..)) => item.map(|i| xml(&i.etag)),
                ("getcontenttype", At::Item(k, ..)) => Some(content_type(*k).into()),
                ("getcontenttype", At::Coll(..) | At::Home(_) | At::Principal) => {
                    Some("httpd/unix-directory".into())
                }
                _ => None,
            };
            match v {
                Some(v) => found.push((name.clone(), v)),
                None => missing.push(name.clone()),
            }
        }
        (found, missing)
    }

    async fn propfind(
        &self,
        place: &At,
        headers: &HeaderMap,
        body: &Bytes,
    ) -> anyhow::Result<Response> {
        let ask = read_ask(body);
        let depth = headers
            .get("depth")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("1")
            .trim()
            .to_string();
        let everything = ask.allprop || ask.props.is_empty();
        let defaults = |p: &At| -> Vec<String> {
            let base: &[&str] = match p {
                At::Principal => &[
                    "resourcetype",
                    "displayname",
                    "current-user-principal",
                    "principal-URL",
                    "calendar-home-set",
                    "addressbook-home-set",
                ],
                At::Home(_) => &["resourcetype", "displayname", "current-user-principal"],
                At::Coll(Kind::Calendars, _) => &[
                    "resourcetype",
                    "displayname",
                    "calendar-color",
                    "getctag",
                    "sync-token",
                    "supported-calendar-component-set",
                    "current-user-privilege-set",
                ],
                At::Coll(Kind::Contacts, _) => &[
                    "resourcetype",
                    "displayname",
                    "getctag",
                    "sync-token",
                    "supported-address-data",
                    "current-user-privilege-set",
                ],
                At::Item(..) => &["resourcetype", "getetag", "getcontenttype"],
            };
            base.iter().map(|s| s.to_string()).collect()
        };
        let asked = |p: &At| {
            if everything {
                defaults(p)
            } else {
                ask.props.clone()
            }
        };
        let mut out = String::new();
        match place {
            At::Principal => {
                let (f, m) = self.props(place, None, None, None, &asked(place));
                out.push_str(&response(&href(place), &f, &m));
                if depth != "0" {
                    for k in [Kind::Calendars, Kind::Contacts] {
                        let h = At::Home(k);
                        let (f, m) = self.props(&h, None, None, None, &asked(&h));
                        out.push_str(&response(&href(&h), &f, &m));
                    }
                }
            }
            At::Home(k) => {
                let (f, m) = self.props(place, None, None, None, &asked(place));
                out.push_str(&response(&href(place), &f, &m));
                if depth != "0" {
                    for c in self.store.collections(*k).await? {
                        let p = At::Coll(*k, c.id.clone());
                        let a = asked(&p);
                        let token = if a.iter().any(|x| x == "getctag" || x == "sync-token") {
                            let items = self.store.items(*k, &c.id).await?;
                            Some(self.remember(*k, &c.id, &items))
                        } else {
                            None
                        };
                        let (f, m) = self.props(&p, Some(&c), None, token.as_deref(), &a);
                        out.push_str(&response(&href(&p), &f, &m));
                    }
                }
            }
            At::Coll(k, id) => {
                let Some(c) = self.coll(*k, id).await? else {
                    return Ok(StatusCode::NOT_FOUND.into_response());
                };
                let items = self.store.items(*k, id).await?;
                let token = self.remember(*k, id, &items);
                let (f, m) = self.props(place, Some(&c), None, Some(&token), &asked(place));
                out.push_str(&response(&href(place), &f, &m));
                if depth != "0" {
                    for i in &items {
                        let p = At::Item(*k, id.clone(), i.name.clone());
                        let (f, m) = self.props(&p, Some(&c), Some(i), None, &asked(&p));
                        out.push_str(&response(&href(&p), &f, &m));
                    }
                }
            }
            At::Item(k, id, name) => {
                let Some(c) = self.coll(*k, id).await? else {
                    return Ok(StatusCode::NOT_FOUND.into_response());
                };
                let Some(i) = self
                    .store
                    .items(*k, id)
                    .await?
                    .into_iter()
                    .find(|i| i.name == *name)
                else {
                    return Ok(StatusCode::NOT_FOUND.into_response());
                };
                let (f, m) = self.props(place, Some(&c), Some(&i), None, &asked(place));
                out.push_str(&response(&href(place), &f, &m));
            }
        }
        Ok(multistatus(out))
    }

    /// Items with their data, one after another.
    async fn with_data(
        &self,
        k: Kind,
        coll: &Coll,
        names: Vec<(String, Option<String>)>,
        asked: &[String],
    ) -> anyhow::Result<String> {
        let data_name = match k {
            Kind::Calendars => "calendar-data",
            Kind::Contacts => "address-data",
        };
        let want_data = asked.iter().any(|a| a == data_name);
        let mut out = String::new();
        for (name, etag) in names {
            let p = At::Item(k, coll.id.clone(), name.clone());
            let got = if want_data {
                match self.store.get(k, &coll.id, &name).await? {
                    Some(g) => Some(g),
                    None => {
                        out.push_str(&gone(&href(&p)));
                        continue;
                    }
                }
            } else {
                None
            };
            let item = Item {
                name: name.clone(),
                etag: got
                    .as_ref()
                    .map(|(_, e)| e.clone())
                    .or(etag)
                    .unwrap_or_default(),
            };
            let rest: Vec<String> = asked.iter().filter(|a| *a != data_name).cloned().collect();
            let (mut f, m) = self.props(&p, Some(coll), Some(&item), None, &rest);
            if let Some((bytes, _)) = got {
                f.push((data_name.into(), xml(&String::from_utf8_lossy(&bytes))));
            }
            out.push_str(&response(&href(&p), &f, &m));
        }
        Ok(out)
    }

    async fn report(&self, place: &At, body: &Bytes) -> anyhow::Result<Response> {
        let ask = read_ask(body);
        let At::Coll(k, id) = place else {
            return Ok(StatusCode::FORBIDDEN.into_response());
        };
        let Some(c) = self.coll(*k, id).await? else {
            return Ok(StatusCode::NOT_FOUND.into_response());
        };
        let asked = if ask.props.is_empty() {
            vec!["getetag".to_string()]
        } else {
            ask.props.clone()
        };
        match ask.root.as_str() {
            "calendar-multiget" | "addressbook-multiget" => {
                let names: Vec<(String, Option<String>)> = ask
                    .hrefs
                    .iter()
                    .filter_map(|h| match href_at(h) {
                        Some(At::Item(hk, hc, f)) if hk == *k && hc == *id => Some((f, None)),
                        _ => None,
                    })
                    .collect();
                Ok(multistatus(self.with_data(*k, &c, names, &asked).await?))
            }
            "calendar-query" | "addressbook-query" => {
                let items = self.store.items(*k, id).await?;
                // a query for one kind of component (VTODO, VEVENT): read
                // each and keep those that have it; times are the program's
                // to filter, so all of them otherwise
                let kinds: Vec<&String> = ask.comps.iter().filter(|c| *c != "VCALENDAR").collect();
                let mut names = Vec::new();
                for i in items {
                    if !kinds.is_empty() {
                        let Some((bytes, _)) = self.store.get(*k, id, &i.name).await? else {
                            continue;
                        };
                        let text = String::from_utf8_lossy(&bytes);
                        if !kinds.iter().any(|kd| text.contains(&format!("BEGIN:{kd}"))) {
                            continue;
                        }
                    }
                    names.push((i.name, Some(i.etag)));
                }
                Ok(multistatus(self.with_data(*k, &c, names, &asked).await?))
            }
            "sync-collection" => {
                let items = self.store.items(*k, id).await?;
                let now: BTreeMap<String, String> = items
                    .iter()
                    .map(|i| (i.name.clone(), i.etag.clone()))
                    .collect();
                let before = match ask.sync_token.as_deref().filter(|t| !t.is_empty()) {
                    None => Some(BTreeMap::new()),
                    Some(t) => self
                        .syncs
                        .lock()
                        .unwrap()
                        .get(&(*k, id.clone()))
                        .and_then(|q| q.iter().find(|(x, _)| x == t).map(|(_, m)| m.clone())),
                };
                let Some(before) = before else {
                    // a token from before this program started, or long ago
                    return Ok((
                        StatusCode::FORBIDDEN,
                        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                        format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><D:error {NS}><D:valid-sync-token/></D:error>"),
                    )
                        .into_response());
                };
                let token = self.remember(*k, id, &items);
                let changed: Vec<(String, Option<String>)> = now
                    .iter()
                    .filter(|(n, e)| before.get(*n) != Some(e))
                    .map(|(n, e)| (n.clone(), Some(e.clone())))
                    .collect();
                let mut out = self.with_data(*k, &c, changed, &asked).await?;
                for n in before.keys().filter(|n| !now.contains_key(*n)) {
                    out.push_str(&gone(&href(&At::Item(*k, id.clone(), n.clone()))));
                }
                out.push_str(&format!("<D:sync-token>{}</D:sync-token>", xml(&token)));
                Ok(multistatus(out))
            }
            _ => Ok(StatusCode::NOT_IMPLEMENTED.into_response()),
        }
    }

    async fn get(&self, place: &At, head: bool) -> anyhow::Result<Response> {
        let At::Item(k, id, name) = place else {
            return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
        };
        if self.coll(*k, id).await?.is_none() {
            return Ok(StatusCode::NOT_FOUND.into_response());
        }
        let Some((bytes, etag)) = self.store.get(*k, id, name).await? else {
            return Ok(StatusCode::NOT_FOUND.into_response());
        };
        let mut r = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(*k))
            .header(header::CONTENT_LENGTH, bytes.len());
        if !etag.is_empty() {
            r = r.header(header::ETAG, etag);
        }
        Ok(r.body(if head {
            Body::empty()
        } else {
            Body::from(bytes)
        })?)
    }

    async fn put(&self, place: &At, headers: &HeaderMap, body: &Bytes) -> anyhow::Result<Response> {
        let At::Item(k, id, name) = place else {
            return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
        };
        let Some(c) = self.coll(*k, id).await? else {
            return Ok(StatusCode::NOT_FOUND.into_response());
        };
        if !c.writable {
            return Ok(
                (StatusCode::FORBIDDEN, "this one is shared with you to see").into_response(),
            );
        }
        let begin = match k {
            Kind::Calendars => "BEGIN:VCALENDAR",
            Kind::Contacts => "BEGIN:VCARD",
        };
        let text = String::from_utf8_lossy(body);
        if !text
            .trim_start_matches('\u{feff}')
            .trim_start()
            .to_ascii_uppercase()
            .starts_with(begin)
        {
            return Ok(
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, format!("not a {begin}")).into_response(),
            );
        }
        let from = version(headers);
        Ok(match self.store.put(*k, id, name, body, &from).await? {
            Saved::Changed => StatusCode::PRECONDITION_FAILED.into_response(),
            Saved::Done(etag) => {
                let mut r = Response::builder().status(if from == Version::New {
                    StatusCode::CREATED
                } else {
                    StatusCode::NO_CONTENT
                });
                if !etag.is_empty() {
                    r = r.header(header::ETAG, etag);
                }
                r.body(Body::empty())?
            }
        })
    }

    async fn delete(&self, place: &At, headers: &HeaderMap) -> anyhow::Result<Response> {
        let At::Item(k, id, name) = place else {
            return Ok(StatusCode::FORBIDDEN.into_response());
        };
        let Some(c) = self.coll(*k, id).await? else {
            return Ok(StatusCode::NOT_FOUND.into_response());
        };
        if !c.writable {
            return Ok(StatusCode::FORBIDDEN.into_response());
        }
        let from = version(headers);
        Ok(if self.store.delete(*k, id, name, &from).await? {
            StatusCode::NO_CONTENT.into_response()
        } else {
            StatusCode::PRECONDITION_FAILED.into_response()
        })
    }

    async fn make(&self, place: &At, body: &Bytes) -> anyhow::Result<Response> {
        let At::Coll(k, id) = place else {
            return Ok(StatusCode::FORBIDDEN.into_response());
        };
        let plain = id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && id.len() <= 64;
        if !plain {
            return Ok((StatusCode::FORBIDDEN, "a plain name, please").into_response());
        }
        if self.coll(*k, id).await?.is_some() {
            return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
        }
        let ask = read_ask(body);
        let name = ask.displayname.clone().unwrap_or_else(|| id.clone());
        self.store.make(*k, id, &name).await?;
        if let Some(c) = ask.color.as_deref().filter(|c| good_color(c)) {
            self.store.rename(*k, id, None, Some(c)).await?;
        }
        Ok(StatusCode::CREATED.into_response())
    }

    async fn proppatch(&self, place: &At, body: &Bytes) -> anyhow::Result<Response> {
        let At::Coll(k, id) = place else {
            return Ok(multistatus(response(&href(place), &[], &[])));
        };
        let Some(c) = self.coll(*k, id).await? else {
            return Ok(StatusCode::NOT_FOUND.into_response());
        };
        let ask = read_ask(body);
        let color = ask.color.as_deref().filter(|c| good_color(c));
        if c.writable && (ask.displayname.is_some() || color.is_some()) {
            self.store
                .rename(*k, id, ask.displayname.as_deref(), color)
                .await?;
        }
        let set: Vec<(String, String)> = ask
            .props
            .iter()
            .map(|p| (p.clone(), String::new()))
            .collect();
        Ok(multistatus(response(&href(place), &set, &[])))
    }
}

/// a colour as calendar programs write one: #RGB, #RRGGBB or #RRGGBBAA
fn good_color(c: &str) -> bool {
    c.strip_prefix('#')
        .is_some_and(|h| matches!(h.len(), 3 | 6 | 8) && h.chars().all(|x| x.is_ascii_hexdigit()))
}

fn version(headers: &HeaderMap) -> Version {
    if let Some(v) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) {
        if v.trim() == "*" {
            return Version::Any;
        }
        return Version::From(v.trim().to_string());
    }
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == "*")
    {
        return Version::New;
    }
    Version::Any
}

#[cfg(test)]
mod tests;
