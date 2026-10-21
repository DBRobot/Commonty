//! A household's home controller at the gate (docs/dev/home-controller.md,
//! "The gate, for a controller"). A controller is no member and holds no
//! device token: it asks with its own key and the word of the member who
//! paired it, the member's root signing its name for that key. It may read
//! one person's entry and keep its backup in the libraries bucket; the
//! member restores from it. The passkey's naming mailbox is here too.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use ed25519_dalek::Verifier as _;
use library_gate::{controller_backup, controller_label};
use serde::{Deserialize, Serialize};

use crate::{App, pages};

pub const HEADER: &str = "x-dd-controller";
/// how far a request's own time may be from this box's, either way
const SKEW: u64 = 300;
/// a presigned backup url: one upload or one download
const URL_SECS: u64 = 600;
/// a passkey's signature waits this long for the app to collect it
const NAME_SECS: u64 = 600;
const MAX_SIG: usize = 8 * 1024;

/// what a controller's request says of itself (x-dd-controller, base64url JSON)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub member: String,
    pub label: String,
    pub controller: String,
    pub name_sig: String,
    pub at: u64,
    pub proof: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    pub member: String,
    pub label: String,
    pub controller: String,
}

/// what the controller's key signs for one request
pub fn proof_message(method: &str, path: &str, at: u64) -> Vec<u8> {
    format!("commonty controller request v1\0{method}\0{path}\0{at}").into_bytes()
}

/// The checks that need no box: the label's shape, the time, the
/// controller's proof for this very request, and the member's root naming
/// that controller. Whether `member` is a member is the caller's to say.
pub fn check(
    domain: &str,
    c: &Claim,
    entry: &identity::Entry,
    method: &str,
    path: &str,
    now: u64,
) -> Result<Caller, &'static str> {
    if !controller_label(&c.label) {
        return Err("not a controller's label");
    }
    if c.at.abs_diff(now) > SKEW {
        return Err("too far from now");
    }
    let key = identity::decode_public(&c.controller).map_err(|_| "not a controller key")?;
    let proof = B64
        .decode(c.proof.trim())
        .ok()
        .and_then(|b| ed25519_dalek::Signature::from_slice(&b).ok())
        .ok_or("not a signature")?;
    key.verify(&proof_message(method, path, c.at), &proof)
        .map_err(|_| "the controller did not sign this request")?;
    if entry.name != c.member {
        return Err("not that member's entry");
    }
    let name = format!("{}.home.{domain}", c.label);
    let origins = [format!("https://home.{domain}")];
    identity::controller_name_signed(entry, &name, &c.controller, &c.name_sig, &origins)
        .map_err(|_| "the member did not name this controller")?;
    Ok(Caller {
        member: c.member.clone(),
        label: c.label.clone(),
        controller: c.controller.clone(),
    })
}

/// The controller asking, or why not: 401 for anything it says that does
/// not hold, 403 for someone who is no member.
#[allow(clippy::result_large_err)]
fn caller(app: &App, headers: &HeaderMap, method: &Method, uri: &Uri) -> Result<Caller, Response> {
    let refused = |why: &str| Err((StatusCode::UNAUTHORIZED, why.to_string()).into_response());
    let Some(raw) = headers.get(HEADER).and_then(|v| v.to_str().ok()) else {
        return refused("a controller's request is signed");
    };
    let Some(c) = (raw.len() <= 16 * 1024)
        .then(|| B64_URL.decode(raw.trim().trim_end_matches('=')).ok())
        .flatten()
        .and_then(|b| serde_json::from_slice::<Claim>(&b).ok())
    else {
        return refused("not a controller's request");
    };
    if !identity::valid_name(&c.member) || c.member == pages::DEMO_USER {
        return refused("not a member's name");
    }
    let Some(e) = app.directory.entry(&c.member).ok().flatten() else {
        return refused("no such member");
    };
    let who = match check(
        &app.domain,
        &c,
        &e.entry,
        method.as_str(),
        uri.path(),
        identity::now(),
    ) {
        Ok(w) => w,
        Err(why) => return refused(why),
    };
    if !app.member(&who.member) {
        return Err((StatusCode::FORBIDDEN, "not a member").into_response());
    }
    Ok(who)
}

/// which member vouched for which controller, by label
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub member: String,
    pub controller: String,
}

pub struct Store {
    path: Option<PathBuf>,
    labels: Mutex<BTreeMap<String, Record>>,
    /// a passkey's signature of a name, by (member, label), until collected
    names: Mutex<HashMap<(String, String), (Instant, String)>>,
    /// the controller key the member's app asked a name for, by (member,
    /// label): the page signs this and nothing a link hands it
    asks: Mutex<HashMap<(String, String), (Instant, String)>>,
}

impl Store {
    pub fn open(path: Option<PathBuf>) -> Store {
        let labels = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Store {
            path,
            labels: Mutex::new(labels),
            names: Default::default(),
            asks: Default::default(),
        }
    }

    fn save(&self, m: &BTreeMap<String, Record>) -> std::io::Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        crate::private::write(p, &serde_json::to_vec_pretty(m)?)
    }

    /// The first word for a label holds it: the same member and controller
    /// again is fine, any other is refused, so no one reaches another
    /// household's backup by naming its label.
    pub fn vouch(&self, c: &Caller) -> Result<(), StatusCode> {
        let mut m = self.labels.lock().unwrap();
        let r = Record {
            member: c.member.clone(),
            controller: c.controller.clone(),
        };
        match m.get(&c.label) {
            Some(had) if *had == r => return Ok(()),
            Some(_) => return Err(StatusCode::CONFLICT),
            None => {}
        }
        m.insert(c.label.clone(), r);
        if let Err(e) = self.save(&m) {
            m.remove(&c.label);
            eprintln!("controller: keeping a label: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Ok(())
    }

    pub fn of(&self, label: &str) -> Option<Record> {
        self.labels.lock().unwrap().get(label).cloned()
    }

    pub fn labels_of(&self, member: &str) -> Vec<String> {
        self.labels
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, r)| r.member == member)
            .map(|(l, _)| l.clone())
            .collect()
    }

    pub fn drop_labels(&self, labels: &[String]) -> std::io::Result<()> {
        let mut m = self.labels.lock().unwrap();
        for l in labels {
            m.remove(l);
        }
        self.save(&m)
    }

    fn ask(&self, member: &str, label: &str, controller: String) {
        let mut m = self.asks.lock().unwrap();
        m.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(NAME_SECS));
        m.insert((member.into(), label.into()), (Instant::now(), controller));
    }

    fn asked(&self, member: &str, label: &str) -> Option<String> {
        self.asks
            .lock()
            .unwrap()
            .get(&(member.to_string(), label.to_string()))
            .filter(|(t, _)| t.elapsed() < Duration::from_secs(NAME_SECS))
            .map(|(_, c)| c.clone())
    }

    fn leave_name(&self, member: &str, label: &str, sig: String) {
        self.asks
            .lock()
            .unwrap()
            .remove(&(member.to_string(), label.to_string()));
        let mut m = self.names.lock().unwrap();
        m.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(NAME_SECS));
        m.insert((member.into(), label.into()), (Instant::now(), sig));
    }

    fn take_name(&self, member: &str, label: &str) -> Option<String> {
        self.names
            .lock()
            .unwrap()
            .remove(&(member.to_string(), label.to_string()))
            .filter(|(t, _)| t.elapsed() < Duration::from_secs(NAME_SECS))
            .map(|(_, s)| s)
    }
}

/// Every backup of the controllers `member` vouched for, gone, and the
/// records with them: they left 90 days ago (leaving.rs). Safe to run again.
pub(crate) async fn forget(
    store: &Store,
    g: &library_gate::Gate,
    member: &str,
) -> anyhow::Result<usize> {
    let mut n = 0;
    let mut gone = Vec::new();
    let mut failed = None;
    for label in store.labels_of(member) {
        match g.forget_controller(&label).await {
            Ok(k) => {
                n += k;
                gone.push(label);
            }
            Err(e) => failed = Some(e),
        }
    }
    store.drop_labels(&gone)?;
    match failed {
        Some(e) => Err(e),
        None => Ok(n),
    }
}

// ---- over http

/// GET /_dd/controller/directory/{name}: one person's entry, for the door's
/// refresh; never the list
pub(crate) async fn directory(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = caller(&app, &headers, &method, &uri) {
        return r;
    }
    if !identity::valid_name(&name) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match app.directory.entry(&name) {
        Ok(Some(e)) => Json(e).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            eprintln!("controller: directory: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /_dd/controller/standing/{name}: whether the agreed state gives this
/// person the fleet's services (an invite, or a box of theirs that counts),
/// and under which root, where this gate goes by it (agreed): what a door
/// holds someone the release's list does not name to
pub(crate) async fn standing(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = caller(&app, &headers, &method, &uri) {
        return r;
    }
    if !identity::valid_name(&name) || name == pages::DEMO_USER {
        return StatusCode::BAD_REQUEST.into_response();
    }
    crate::directory::standing_answer(&app.directory, &name)
}

/// POST /_dd/controller/backup: urls to put and get its one backup
pub(crate) async fn backup(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let Some(g) = &app.library else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let c = match caller(&app, &headers, &method, &uri) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(st) = app.controllers.vouch(&c) {
        return (st, "that controller's label is another's").into_response();
    }
    let object = controller_backup(&c.label);
    Json(serde_json::json!({
        "put": g.presign_for("PUT", &object, URL_SECS),
        "get": g.presign_for("GET", &object, URL_SECS),
    }))
    .into_response()
}

/// GET /_dd/controller/backup/{label}: a member restoring their controller
pub(crate) async fn restore(
    State(app): State<Arc<App>>,
    Path(label): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(g) = &app.library else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if user == pages::DEMO_USER || !app.member(&user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // someone else's controller is as unknown as no controller at all
    if !controller_label(&label) || app.controllers.of(&label).is_none_or(|r| r.member != user) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let object = controller_backup(&label);
    match g.stat(&object).await {
        Ok(Some(_)) => Json(serde_json::json!({ "get": g.presign_for("GET", &object, URL_SECS) }))
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            eprintln!("controller: a backup: {e:#}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// GET /_dd/controller-name: the page that asks the passkey
pub(crate) async fn name_page() -> Response {
    (
        [("cache-control", "no-cache")],
        Html(pages::page("controller-name").unwrap_or_default()),
    )
        .into_response()
}

#[derive(Deserialize)]
pub(crate) struct Named {
    sig: String,
}

/// POST /_dd/controller-name/{label} {sig}: the page leaves the passkey's
/// answer for the app. No secret: it names a controller and opens nothing.
pub(crate) async fn leave_name(
    State(app): State<Arc<App>>,
    Path(label): Path<String>,
    headers: HeaderMap,
    Json(n): Json<Named>,
) -> Response {
    if !crate::from_our_site(&app.domain, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(user) = app.signed_in(headers.get("cookie").and_then(|v| v.to_str().ok())) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if user == pages::DEMO_USER || !app.member(&user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !controller_label(&label) || n.sig.is_empty() || n.sig.len() > MAX_SIG {
        return StatusCode::BAD_REQUEST.into_response();
    }
    // only the answer to what this member's app asked, and only their root's
    let Some(controller) = app.controllers.asked(&user, &label) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(e) = app.directory.entry(&user).ok().flatten() else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let name = format!("{label}.home.{}", app.domain);
    let origins = [format!("https://home.{}", app.domain)];
    if identity::controller_name_signed(&e.entry, &name, &controller, &n.sig, &origins).is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    app.controllers.leave_name(&user, &label, n.sig);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(crate) struct Ask {
    controller: String,
}

/// PUT /_dd/controller-name/{label} {controller}: the member's app asks for
/// a name for the controller it is pairing, with their device token. A link
/// to the page cannot: it signs only what is asked here.
pub(crate) async fn ask_name(
    State(app): State<Arc<App>>,
    Path(label): Path<String>,
    headers: HeaderMap,
    Json(a): Json<Ask>,
) -> Response {
    let Some((user, true)) = app.identify_how(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if user == pages::DEMO_USER || !app.member(&user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if identity::controller_label(&a.controller).ok().as_deref() != Some(label.as_str()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    app.controllers.ask(&user, &label, a.controller);
    StatusCode::NO_CONTENT.into_response()
}

/// GET /_dd/controller-name/{label}/ask: the page reads what the signed-in
/// member's app asked
pub(crate) async fn asked_name(
    State(app): State<Arc<App>>,
    Path(label): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = app.signed_in(headers.get("cookie").and_then(|v| v.to_str().ok())) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.controllers.asked(&user, &label) {
        Some(c) => (
            [("cache-control", "no-store")],
            Json(serde_json::json!({ "controller": c })),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET /_dd/controller-name/{label}: the app collects it, once, with a
/// device token of the member who left it
pub(crate) async fn take_name(
    State(app): State<Arc<App>>,
    Path(label): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some((user, true)) = app.identify_how(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.controllers.take_name(&user, &label) {
        Some(sig) => Json(serde_json::json!({ "sig": sig })).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::Signer as _;

    pub const LABEL: &str = "abcdefghijklmn23";

    /// a controller of the tests', and the label its key gives it
    pub fn ctl() -> identity::SigningKey {
        identity::SigningKey::from_bytes(&[7; 32])
    }
    pub fn label_of(k: &identity::SigningKey) -> String {
        identity::controller_label(&identity::encode_public(&k.verifying_key())).unwrap()
    }

    /// what the controller sends: `member` named it with `root`
    #[allow(clippy::too_many_arguments)]
    pub fn header(
        member: &str,
        root: &identity::SigningKey,
        controller: &identity::SigningKey,
        label: &str,
        domain: &str,
        method: &str,
        path: &str,
        at: u64,
    ) -> String {
        let key = identity::encode_public(&controller.verifying_key());
        let name = format!("{label}.home.{domain}");
        let c = Claim {
            member: member.into(),
            label: label.into(),
            controller: key.clone(),
            name_sig: B64.encode(
                root.sign(&identity::controller_name_message(&name, &key))
                    .to_bytes(),
            ),
            at,
            proof: B64.encode(controller.sign(&proof_message(method, path, at)).to_bytes()),
        };
        B64_URL.encode(serde_json::to_vec(&c).unwrap())
    }

    fn entry(name: &str, root: &identity::SigningKey) -> identity::Entry {
        identity::Entry {
            name: name.into(),
            root: identity::encode_public(&root.verifying_key()),
            recovery: String::new(),
            recovered_from: vec![],
            terms: None,
            devices: vec![],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            removed_boxes: vec![],
            version: 1,
            updated: 1,
        }
    }

    fn claim(h: &str) -> Claim {
        serde_json::from_slice(&B64_URL.decode(h).unwrap()).unwrap()
    }

    #[test]
    fn a_controller_is_its_key_and_its_members_word() {
        let (root, ctl, other) = (
            identity::generate(),
            identity::generate(),
            identity::generate(),
        );
        let e = entry("tom", &root);
        let label = &label_of(&ctl);
        let now = 1_000_000;
        let p = "/_dd/controller/backup";
        let h = |root: &identity::SigningKey, ctl: &identity::SigningKey, label: &str, at| {
            claim(&header("tom", root, ctl, label, "x.org", "POST", p, at))
        };
        let ok = check("x.org", &h(&root, &ctl, label, now), &e, "POST", p, now).unwrap();
        assert_eq!(ok.member, "tom");
        assert_eq!(&ok.label, label);
        assert!(
            check(
                "x.org",
                &h(&root, &ctl, label, now - 300),
                &e,
                "POST",
                p,
                now
            )
            .is_ok()
        );
        for (c, why) in [
            (h(&root, &ctl, label, now - 301), "too old"),
            (h(&root, &ctl, label, now + 301), "ahead"),
            (h(&other, &ctl, label, now), "another root named it"),
            (
                h(&root, &ctl, "ABCDEFGHIJKLMN23", now),
                "a label of another shape",
            ),
            (h(&root, &ctl, "abcdefghijklmn2", now), "short"),
        ] {
            assert!(check("x.org", &c, &e, "POST", p, now).is_err(), "{why}");
        }
        let good = h(&root, &ctl, label, now);
        assert!(
            check("y.org", &good, &e, "POST", p, now).is_err(),
            "another domain's name"
        );
        assert!(
            check("x.org", &good, &e, "GET", p, now).is_err(),
            "another method"
        );
        assert!(
            check(
                "x.org",
                &good,
                &e,
                "POST",
                "/_dd/controller/directory/tom",
                now
            )
            .is_err(),
            "another path"
        );
        assert!(check("x.org", &good, &entry("ann", &root), "POST", p, now).is_err());
        // a name signed for one controller is not another's
        let mut swapped = good.clone();
        swapped.controller = identity::encode_public(&other.verifying_key());
        swapped.proof = B64.encode(other.sign(&proof_message("POST", p, now)).to_bytes());
        assert!(check("x.org", &swapped, &e, "POST", p, now).is_err());
        // nor another label's
        let mut moved = good.clone();
        moved.label = "zyxwvutsrqponm76".into();
        assert!(check("x.org", &moved, &e, "POST", p, now).is_err());
        // nor another controller's label, even on the root's own word: a
        // label is held by its controller's key
        let theirs = label_of(&other);
        assert!(check("x.org", &h(&root, &ctl, &theirs, now), &e, "POST", p, now).is_err());
    }

    #[test]
    fn a_label_is_the_first_vouchers_and_kept() {
        let d = std::env::temp_dir().join(format!("dd-ctl-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("controller-backups.json");
        let s = Store::open(Some(path.clone()));
        let tom = Caller {
            member: "tom".into(),
            label: LABEL.into(),
            controller: "K1".into(),
        };
        s.vouch(&tom).unwrap();
        s.vouch(&tom).unwrap();
        let mut ann = tom.clone();
        ann.member = "ann".into();
        assert_eq!(s.vouch(&ann), Err(StatusCode::CONFLICT));
        let mut other_key = tom.clone();
        other_key.controller = "K2".into();
        assert_eq!(s.vouch(&other_key), Err(StatusCode::CONFLICT));
        let s = Store::open(Some(path));
        assert_eq!(s.of(LABEL).unwrap().member, "tom");
        assert_eq!(s.labels_of("tom"), vec![LABEL.to_string()]);
        assert!(s.labels_of("ann").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_name_is_collected_once_by_whoever_left_it() {
        let s = Store::open(None);
        s.leave_name("tom", LABEL, "SIG".into());
        assert_eq!(s.take_name("ann", LABEL), None);
        assert_eq!(s.take_name("tom", "zyxwvutsrqponm76"), None);
        assert_eq!(s.take_name("tom", LABEL).as_deref(), Some("SIG"));
        assert_eq!(s.take_name("tom", LABEL), None);
    }

    /// A bucket in memory: HEAD, a listing and DELETE, as much of S3 as
    /// the gate's controller routes and leaving use. Signatures unchecked.
    pub type Bucket = Arc<Mutex<std::collections::BTreeSet<String>>>;

    pub async fn bucket() -> (library_gate::Gate, Bucket) {
        use axum::extract::Request;
        let b: Bucket = Default::default();
        let held = b.clone();
        let app = axum::Router::new().fallback(move |req: Request| {
            let b = held.clone();
            async move {
                let path = req.uri().path().trim_start_matches("/b/").to_string();
                let query = req.uri().query().unwrap_or("").to_string();
                let mut b = b.lock().unwrap();
                match req.method().as_str() {
                    "GET" if query.contains("list-type") => {
                        let prefix = query
                            .split('&')
                            .find_map(|kv| kv.strip_prefix("prefix="))
                            .unwrap_or("")
                            .replace("%2F", "/");
                        let mut out = String::from("<ListBucketResult>");
                        for k in b.iter().filter(|k| k.starts_with(&prefix)) {
                            out.push_str(&format!(
                                "<Contents><Key>{k}</Key><Size>1</Size></Contents>"
                            ));
                        }
                        out.push_str("<IsTruncated>false</IsTruncated></ListBucketResult>");
                        out.into_response()
                    }
                    "HEAD" if b.contains(&path) => StatusCode::OK.into_response(),
                    "HEAD" => StatusCode::NOT_FOUND.into_response(),
                    "DELETE" => {
                        b.remove(&path);
                        StatusCode::NO_CONTENT.into_response()
                    }
                    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
                }
            }
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
        let g = library_gate::Gate {
            endpoint: format!("http://{at}"),
            bucket: "b".into(),
            key_id: "k".into(),
            key_secret: "s".into(),
            region: "garage".into(),
        };
        (g, b)
    }

    #[tokio::test]
    async fn leaving_takes_the_controllers_backups_they_vouched_for() {
        let d = std::env::temp_dir().join(format!("dd-ctl-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("controller-backups.json");
        let s = Store::open(Some(path.clone()));
        let (g, b) = bucket().await;
        let labels = [LABEL, "zyxwvutsrqponm76", "aaaaaaaaaaaaaaaa"];
        for (label, member) in labels.iter().zip(["tom", "tom", "ann"]) {
            s.vouch(&Caller {
                member: member.into(),
                label: (*label).into(),
                controller: format!("K-{label}"),
            })
            .unwrap();
            b.lock().unwrap().insert(controller_backup(label));
        }
        assert_eq!(forget(&s, &g, "tom").await.unwrap(), 2);
        assert_eq!(
            b.lock().unwrap().iter().cloned().collect::<Vec<_>>(),
            vec![controller_backup("aaaaaaaaaaaaaaaa")],
            "ann's stays"
        );
        let s = Store::open(Some(path));
        assert!(s.labels_of("tom").is_empty(), "the records go too, kept so");
        assert_eq!(s.labels_of("ann"), vec!["aaaaaaaaaaaaaaaa".to_string()]);
        assert_eq!(forget(&s, &g, "tom").await.unwrap(), 0, "again is nothing");
        let _ = std::fs::remove_dir_all(&d);
    }
}
