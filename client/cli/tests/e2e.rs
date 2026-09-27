//! The identity model, end to end: real boxes in-process, the real `dd`
//! binary against them, and every hostile move we know of. This is the
//! test a change to any of it has to pass before it merges.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

static N: AtomicUsize = AtomicUsize::new(0);

fn scratch(what: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "dd-e2e-{}-{}-{what}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A box. `full` runs the verifier with the browser login; otherwise the
/// directory alone.
struct Box_ {
    addr: std::net::SocketAddr,
    dir: PathBuf,
}

impl Box_ {
    async fn start(full: bool, peers: Vec<String>, sync_secs: u64) -> Self {
        Self::start_members(full, peers, sync_secs, None).await
    }
    /// `members`: Some(list) gates the services the way a real box does
    async fn start_members(
        full: bool,
        peers: Vec<String>,
        sync_secs: u64,
        members: Option<Vec<String>>,
    ) -> Self {
        Self::start_with(
            full,
            peers,
            sync_secs,
            members.map(verify::Members::list),
            None,
        )
        .await
    }
    /// everything: the member list with revocations, and the release key
    /// whose invites count
    async fn start_with(
        full: bool,
        peers: Vec<String>,
        sync_secs: u64,
        members: Option<verify::Members>,
        release_pub: Option<String>,
    ) -> Self {
        Self::start_home(full, peers, sync_secs, members, release_pub, vec![]).await
    }
    /// and the tiles this box offers
    async fn start_home(
        full: bool,
        peers: Vec<String>,
        sync_secs: u64,
        members: Option<verify::Members>,
        release_pub: Option<String>,
        home: Vec<verify::pages::Service>,
    ) -> Self {
        Self::start_on(
            scratch("box").join("keys"),
            full,
            peers,
            sync_secs,
            members,
            release_pub,
            home,
        )
        .await
    }
    /// on a state directory another box already used: the same box, started
    /// again with a new release
    async fn start_on(
        dir: PathBuf,
        full: bool,
        peers: Vec<String>,
        sync_secs: u64,
        members: Option<verify::Members>,
        release_pub: Option<String>,
        home: Vec<verify::pages::Service>,
    ) -> Self {
        let (addr, _task) = verify::start(verify::Config {
            home,
            members,
            release_pub,
            web_dir: None,
            photos: None,
            library: None,
            fleet: Default::default(),
            demo_library: None,
            tmdb: None,
            app_manifest: None,
            network: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            dir: dir.clone(),
            peers,
            sync_secs,
            domain: full.then(|| "localhost".to_string()),
            oidc: None,
        })
        .await
        .unwrap();
        Self { addr, dir }
    }
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
    fn directory(&self) -> String {
        self.url("/_dd/directory")
    }
}

/// One "device": its own key store file.
struct Device {
    keyring: PathBuf,
}

struct Ran {
    ok: bool,
    out: String,
}

impl Device {
    fn new() -> Self {
        Self {
            keyring: scratch("device").join("keys.json"),
        }
    }
    fn dd(&self, args: &[&str]) -> Ran {
        let o = Command::new(env!("CARGO_BIN_EXE_dd"))
            .args(args)
            .env("DD_KEYRING_FILE", &self.keyring)
            .output()
            .unwrap();
        Ran {
            ok: o.status.success(),
            out: format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        }
    }
    fn dd_ok(&self, args: &[&str]) -> String {
        let r = self.dd(args);
        assert!(r.ok, "dd {} failed:\n{}", args.join(" "), r.out);
        r.out
    }
    fn dd_fails(&self, args: &[&str], containing: &str) -> String {
        self.dd_fails_with(args, &[containing])
    }
    /// failed, and for one of these reasons
    fn dd_fails_with(&self, args: &[&str], any_of: &[&str]) -> String {
        let r = self.dd(args);
        assert!(
            !r.ok,
            "dd {} should have failed:\n{}",
            args.join(" "),
            r.out
        );
        assert!(
            any_of.iter().any(|c| r.out.contains(c)),
            "dd {} failed for the wrong reason - wanted one of {any_of:?} in:\n{}",
            args.join(" "),
            r.out
        );
        r.out
    }
    fn token(&self) -> String {
        self.dd_ok(&["token"]).trim().to_string()
    }
    fn public_key(&self) -> String {
        self.dd_ok(&["device", "show"])
            .lines()
            .nth(1)
            .unwrap()
            .trim()
            .to_string()
    }
}

fn dirs<'a>(boxes: impl IntoIterator<Item = &'a Box_>) -> Vec<String> {
    boxes
        .into_iter()
        .flat_map(|b| ["--directory".to_string(), b.directory()])
        .collect()
}

fn args<'a>(fixed: &[&'a str], extra: &'a [String]) -> Vec<&'a str> {
    fixed
        .iter()
        .copied()
        .chain(extra.iter().map(String::as_str))
        .collect()
}

/// The paper key out of `dd identity new` / `recover` output.
fn recovery_key(out: &str) -> String {
    out.lines()
        .map(str::trim)
        .find(|l| l.len() == 44 && l.ends_with('=') && !l.contains(' '))
        .expect("no recovery key printed")
        .to_string()
}

async fn verify_status(b: &Box_, token: &str) -> u16 {
    reqwest::Client::new()
        .get(b.url("/verify"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn entry(b: &Box_, name: &str) -> Option<serde_json::Value> {
    let r = reqwest::get(format!("{}/{name}", b.directory()))
        .await
        .unwrap();
    if r.status().as_u16() == 404 {
        return None;
    }
    Some(r.json().await.unwrap())
}

async fn put_raw(b: &Box_, name: &str, body: &serde_json::Value) -> (u16, String) {
    let r = reqwest::Client::new()
        .put(format!("{}/{name}", b.directory()))
        .json(body)
        .send()
        .await
        .unwrap();
    let s = r.status().as_u16();
    (s, r.text().await.unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_devices_tokens_and_recovery() {
    let a = Box_::start(true, vec![], 300).await;
    let b = Box_::start(false, vec![], 300).await;
    let d = dirs([&a, &b]);
    let laptop = Device::new();
    let phone = Device::new();
    let stranger = Device::new();

    // birth: published to both, recovery key printed once
    let out = laptop.dd_ok(&args(&["identity", "new", "--name", "sarah"], &d));
    let paper = recovery_key(&out);
    assert_eq!(entry(&a, "sarah").await.unwrap()["entry"]["version"], 1);
    assert_eq!(entry(&b, "sarah").await.unwrap()["entry"]["version"], 1);
    assert_eq!(verify_status(&a, &laptop.token()).await, 200);

    // a second device: admitted by the root, signs its own tokens after
    let pk = phone.public_key();
    laptop.dd_ok(&args(&["device", "admit", &pk], &d));
    // the phone also gets the root, so it could admit devices itself
    let root = laptop.dd_ok(&["identity", "export"]);
    {
        use std::io::Write as _;
        let mut c = Command::new(env!("CARGO_BIN_EXE_dd"))
            .args(["identity", "import", "--name", "sarah"])
            .env("DD_KEYRING_FILE", &phone.keyring)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(root.as_bytes()).unwrap();
        assert!(c.wait().unwrap().success());
    }
    assert_eq!(verify_status(&a, &phone.token()).await, 200);
    assert_eq!(entry(&a, "sarah").await.unwrap()["entry"]["version"], 2);

    // a device nobody admitted, even one holding the name, is refused
    stranger.dd_ok(&["device", "show"]);
    {
        use std::io::Write as _;
        let mut c = Command::new(env!("CARGO_BIN_EXE_dd"))
            .args(["identity", "import", "--name", "sarah"])
            .env("DD_KEYRING_FILE", &stranger.keyring)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(root.as_bytes()).unwrap();
        c.wait().unwrap();
    }
    assert_eq!(verify_status(&a, &stranger.token()).await, 401);

    // forgery and replay at the directory
    let mut forged = entry(&a, "sarah").await.unwrap();
    forged["entry"]["version"] = serde_json::json!(9);
    forged["signature"] = serde_json::json!(base64_zero());
    let (s, why) = put_raw(&a, "sarah", &forged).await;
    assert_eq!(s, 403, "{why}");
    let mut replay = entry(&a, "sarah").await.unwrap();
    replay["entry"]["version"] = serde_json::json!(1);
    let (s, why) = put_raw(&a, "sarah", &replay).await;
    assert_eq!(s, 403, "{why}");

    // recovery: wrong paper key refused, right one installs a new root and
    // drops every device
    let recovered = Device::new();
    recovered.dd_ok(&["device", "show"]);
    run_with_stdin(
        &recovered,
        &args(
            &["identity", "recover", "--name", "sarah", "--key-stdin"],
            &d,
        ),
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n",
        false,
    );
    let out = run_with_stdin(
        &recovered,
        &args(
            &["identity", "recover", "--name", "sarah", "--key-stdin"],
            &d,
        ),
        &format!("{paper}\n"),
        true,
    );
    let _next_paper = recovery_key(&out);
    assert_eq!(entry(&b, "sarah").await.unwrap()["entry"]["version"], 3);
    assert_eq!(verify_status(&a, &laptop.token()).await, 401);
    assert_eq!(verify_status(&a, &phone.token()).await, 401);
    assert_eq!(verify_status(&a, &recovered.token()).await, 200);
    // the old root can do nothing any more
    laptop.dd_fails(&args(&["device", "admit", &pk], &d), "different root");
}

fn run_with_stdin(dev: &Device, a: &[&str], stdin: &str, want_ok: bool) -> String {
    use std::io::Write as _;
    let mut c = Command::new(env!("CARGO_BIN_EXE_dd"))
        .args(a)
        .env("DD_KEYRING_FILE", &dev.keyring)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(o.status.success(), want_ok, "dd {}:\n{out}", a.join(" "));
    out
}

fn base64_zero() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode([0u8; 64])
}

#[tokio::test(flavor = "multi_thread")]
async fn enrol_links_and_access_tokens_do_not_swap() {
    let a = Box_::start(true, vec![], 300).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(&["identity", "new", "--name", "tom"], &d));
    let access = dev.token();
    // dd enrol prints the link then waits; take the link and stop it
    let enrol = enrol_token(&dev, &a);
    let http = reqwest::Client::new();
    let post = |path: &str, tok: &str| http.post(a.url(path)).bearer_auth(tok.to_string()).send();
    assert_eq!(verify_status(&a, &access).await, 200);
    assert_eq!(
        verify_status(&a, &enrol).await,
        401,
        "an enrol link opens no file"
    );
    assert_eq!(
        post("/_dd/enrol/start", &enrol).await.unwrap().status(),
        200
    );
    assert_eq!(
        post("/_dd/enrol/start", &access).await.unwrap().status(),
        401,
        "an access token cannot enrol a passkey"
    );
    assert_eq!(
        http.get(a.url("/_dd/enrol/result"))
            .bearer_auth(&access)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

/// Start `dd enrol`, read the link it prints, kill it. Just the token.
#[tokio::test(flavor = "multi_thread")]
async fn membership_gates_the_services_not_the_account() {
    // a was released with nobody on the list. tom makes his account there:
    // that works, and gets him nothing
    let a = Box_::start_members(true, vec![], 300, Some(vec![])).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(&["identity", "new", "--name", "tom"], &d));
    let access = dev.token();
    assert_eq!(
        verify_status(&a, &access).await,
        403,
        "an account is not a membership"
    );

    // his member id comes from his own entry; a box released with it on
    // the list lets the same token through
    let show = dev.dd_ok(&args(&["identity", "show"], &d));
    let id = show
        .lines()
        .find_map(|l| l.strip_prefix("member id:"))
        .expect("identity show prints the member id")
        .trim()
        .to_string();
    assert_eq!(id.len(), 64);
    let b = Box_::start_members(true, vec![], 300, Some(vec![id.clone()])).await;
    dev.dd_ok(&args(&["identity", "publish"], &dirs([&a, &b])));
    assert_eq!(verify_status(&b, &access).await, 200);
    assert_eq!(verify_status(&a, &access).await, 403);

    // the list is edited by name, stored by id, and never holds the name
    let repo = scratch("repo");
    std::fs::create_dir_all(repo.join("fleet")).unwrap();
    let repo = repo.to_str().unwrap().to_string();
    let file = || std::fs::read_to_string(format!("{repo}/fleet/members.json")).unwrap_or_default();
    dev.dd_ok(&args(&["member", "add", "tom", "--repo", &repo], &d));
    assert!(file().contains(&id), "{}", file());
    assert!(!file().contains("tom"));
    let out = dev.dd_ok(&args(&["member", "list", "--repo", &repo], &d));
    assert!(out.contains(&id) && out.contains("tom"), "{out}");
    dev.dd_ok(&args(&["member", "remove", "tom", "--repo", &repo], &d));
    let f: serde_json::Value = serde_json::from_str(&file()).unwrap();
    assert!(f["members"].as_array().unwrap().is_empty(), "{}", file());
    assert_eq!(f["revoked"][0], id, "{}", file());
    assert!(
        !dev.dd(&args(&["member", "remove", "tom", "--repo", &repo], &d))
            .ok
    );
}

/// The browser side of a join, against a box: name, passkey, signed entry.
/// Returns the session cookie the box hands back.
async fn join_in_browser(b: &Box_, authenticator: &mut SoftPasskey, name: &str) -> (u16, String) {
    join_with_code(b, authenticator, name, None).await
}

/// What the browser sends for a code: the proof the code's key makes for
/// this root, as the x-dd-grant header carries it.
fn grant_header(code: &str, root: &str) -> String {
    use base64::Engine as _;
    let key = identity::code_key(code);
    let pk = identity::encode_public(&key.verifying_key());
    let redeemed = identity::now();
    let proof = identity::prove(&key, &pk, root, redeemed);
    base64::engine::general_purpose::STANDARD.encode(
        serde_json::json!({ "invite_public_key": pk, "redeemed": redeemed, "proof": proof })
            .to_string(),
    )
}

async fn join_with_code(
    b: &Box_,
    authenticator: &mut SoftPasskey,
    name: &str,
    code: Option<&str>,
) -> (u16, String) {
    let origin = Url::parse("https://localhost").unwrap();
    let http = reqwest::Client::new();
    let r = http
        .post(b.url("/_dd/join/start"))
        .json(&serde_json::json!({ "username": name }))
        .send()
        .await
        .unwrap();
    if r.status() != 200 {
        return (r.status().as_u16(), r.text().await.unwrap_or_default());
    }
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let ccr: CreationChallengeResponse = serde_json::from_value(j).unwrap();
    let reg = authenticator
        .do_registration(origin.clone(), ccr)
        .expect("software authenticator registration");
    let mut req = http
        .post(b.url("/_dd/join/finish"))
        .header("x-dd-ceremony", &ceremony);
    if let Some(code) = code {
        req = req.header(
            "x-dd-grant",
            grant_header(code, &format!("webauthn:{}", reg.id)),
        );
    }
    let r = req.json(&reg).send().await.unwrap();
    if r.status() != 200 {
        return (r.status().as_u16(), r.text().await.unwrap_or_default());
    }
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    // the box asks for an assertion over the entry's hash
    let rcr: RequestChallengeResponse = serde_json::from_value(j).unwrap();
    let cred = authenticator
        .do_authentication(origin, rcr)
        .expect("software authenticator assertion");
    let r = http
        .post(b.url("/_dd/join/sign"))
        .header("x-dd-ceremony", &ceremony)
        .json(&cred)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    if status != 200 {
        return (status, r.text().await.unwrap_or_default());
    }
    let cookie = r
        .headers()
        .get("set-cookie")
        .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_string())
        .unwrap_or_default();
    (status, cookie)
}

async fn get_with_cookie(b: &Box_, path: &str, cookie: &str) -> (u16, String) {
    let r = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(b.url(path))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_made_in_a_browser_is_a_passkey_root_and_waits_for_membership() {
    let a = Box_::start_members(true, vec![], 300, Some(vec![])).await;
    let mut eve = SoftPasskey::new(true);
    let (status, cookie) = join_in_browser(&a, &mut eve, "eve").await;
    assert_eq!(status, 200);
    assert!(!cookie.is_empty());

    // No code, so it is held on this box and not published: the directory
    // has no eve, and nothing a peer could pull.
    assert!(
        entry(&a, "eve").await.is_none(),
        "a sign-up without a code is published"
    );
    let held = a.dir.parent().unwrap().join("held").join("eve.json");

    // what is held: a passkey for a root, no device, no recovery key, and
    // it verifies on its own
    let e: serde_json::Value = serde_json::from_slice(&std::fs::read(&held).unwrap()).unwrap();
    let root = e["entry"]["root"].as_str().unwrap().to_string();
    assert!(root.starts_with("webauthn:"), "{root}");
    assert!(e["entry"]["devices"].as_array().unwrap().is_empty());
    assert_eq!(e["entry"]["passkeys"].as_array().unwrap().len(), 1);
    let signed: identity::SignedEntry = serde_json::from_value(e.clone()).unwrap();
    identity::accept(None, &signed).unwrap();
    // and a box refuses the same bytes with the signature of another entry
    let mut forged = signed.clone();
    forged.entry.name = "eva".into();
    assert!(identity::accept(None, &forged).is_err());

    // signed in, not a member: the home page says so, nothing opens
    let (st, body) = get_with_cookie(&a, "/_dd/home", &cookie).await;
    assert_eq!(st, 200);
    assert!(body.contains("Your account is made"), "{body}");
    assert_eq!(get_with_cookie(&a, "/verify", &cookie).await.0, 403);
    assert_eq!(get_with_cookie(&a, "/_dd/me", &cookie).await.0, 403);
    // and none of a member's own pages open to an account that is not one
    for page in [
        "/_dd/files",
        "/_dd/media",
        "/_dd/boxes",
        "/_dd/backups",
        "/_dd/devices",
        "/_dd/network",
    ] {
        let (st, _) = get_with_cookie(&a, page, &cookie).await;
        assert_eq!(st, 303, "{page} opened to a non-member");
    }
    assert_eq!(get_with_cookie(&a, "/_dd/fleet.json", &cookie).await.0, 403);
    assert_eq!(
        get_with_cookie(&a, "/_dd/network/mine", &cookie).await.0,
        403
    );

    // the name is taken now, by a different passkey too
    let mut mallory = SoftPasskey::new(true);
    assert_eq!(join_in_browser(&a, &mut mallory, "eve").await.0, 409);

    // Following through by the member list: the member id is of the passkey
    // root, and the box holding her, released with it, publishes her - then
    // lets her in once she signs in with the same passkey.
    let id = identity::member_id(&root);
    let b = Box_::start_on(
        a.dir.clone(),
        true,
        vec![],
        1,
        Some(verify::Members::list(vec![id.clone()])),
        None,
        vec![],
    )
    .await;
    wait_for(|| async { entry(&b, "eve").await.is_some() }).await;
    assert!(!held.exists(), "published, and the hold is gone");
    let origin = Url::parse("https://localhost").unwrap();
    let http = reqwest::Client::new();
    let r = http
        .post(b.url("/_dd/login/start"))
        .json(&serde_json::json!({ "username": "eve" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let rcr: RequestChallengeResponse = serde_json::from_value(j).unwrap();
    let cred = eve.do_authentication(origin, rcr).unwrap();
    let r = http
        .post(b.url("/_dd/login/finish"))
        .header("x-dd-ceremony", &ceremony)
        .json(&cred)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
    let cookie_b = r
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(get_with_cookie(&b, "/verify", &cookie_b).await.0, 200);
    let (_, body) = get_with_cookie(&b, "/_dd/home", &cookie_b).await;
    assert!(body.contains("id=\"services\""), "{body}");
    let (st, me) = get_with_cookie(&b, "/_dd/me", &cookie_b).await;
    assert_eq!(st, 200, "{me}");

    // dd member add, by name, finds the same id
    let repo = scratch("repo");
    std::fs::create_dir_all(repo.join("fleet")).unwrap();
    let repo = repo.to_str().unwrap().to_string();
    let dev = Device::new();
    dev.dd_ok(&args(
        &["member", "add", "eve", "--repo", &repo],
        &dirs([&a]),
    ));
    let file = std::fs::read_to_string(format!("{repo}/fleet/members.json")).unwrap();
    assert!(file.contains(&id), "{file}");
}

/// Sign in on `b` with a passkey already in the entry; the session cookie.
async fn login_in_browser(b: &Box_, authenticator: &mut SoftPasskey, name: &str) -> (u16, String) {
    let origin = Url::parse("https://localhost").unwrap();
    let http = reqwest::Client::new();
    let r = http
        .post(b.url("/_dd/login/start"))
        .json(&serde_json::json!({ "username": name }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let rcr: RequestChallengeResponse = serde_json::from_value(j).unwrap();
    let cred = authenticator.do_authentication(origin, rcr).unwrap();
    let r = http
        .post(b.url("/_dd/login/finish"))
        .header("x-dd-ceremony", &ceremony)
        .json(&cred)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let cookie = r
        .headers()
        .get("set-cookie")
        .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_string())
        .unwrap_or_default();
    (status, cookie)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invite_code_lets_one_person_in_once() {
    // the owner's laptop: a release key, a repo
    let owner = Device::new();
    let repo = scratch("repo");
    std::fs::create_dir_all(repo.join("fleet")).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    let repo = repo.to_str().unwrap().to_string();
    let out = owner.dd_ok(&["release", "init", "--repo", &repo]);
    let release_pub = out.lines().last().unwrap().trim().to_string();
    assert_eq!(release_pub.len(), 44, "{out}");

    // a box released with nobody on the list, holding the release key
    let a = Box_::start_with(
        true,
        vec![],
        300,
        Some(verify::Members::list(vec![])),
        Some(release_pub.clone()),
    )
    .await;
    let d = dirs([&a]);

    // an invite from the owner; one from anyone else is refused
    let out = owner.dd_ok(&args(&["invite", "--ttl", "10m"], &d));
    let code = out
        .lines()
        .find_map(|l| l.strip_prefix("code: "))
        .expect("dd invite prints the code")
        .trim()
        .to_string();
    assert_eq!(code.len(), 11, "{out}");
    let stranger = identity::generate();
    let forged = identity::sign_invite(
        identity::Invite {
            public_key: identity::encode_public(&identity::code_key("nope-nope").verifying_key()),
            issued: identity::now(),
            expires: identity::now() + 600,
        },
        &stranger,
    )
    .unwrap();
    let http = reqwest::Client::new();
    {
        use base64::Engine as _;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(forged.invite.public_key.as_bytes());
        let r = http
            .put(a.url(&format!("/_dd/invite/{key}")))
            .json(&forged)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
    }

    // eve joins with the code: a member at once
    let mut eve = SoftPasskey::new(true);
    let (st, cookie) = join_with_code(&a, &mut eve, "eve", Some(&code)).await;
    assert_eq!(st, 200);
    assert_eq!(get_with_cookie(&a, "/verify", &cookie).await.0, 200);
    let (_, body) = get_with_cookie(&a, "/_dd/home", &cookie).await;
    assert!(body.contains("id=\"services\""), "{body}");
    assert_eq!(get_with_cookie(&a, "/_dd/me", &cookie).await.0, 200);
    let e = entry(&a, "eve").await.unwrap();
    assert!(e["entry"]["grant"]["proof"].is_string());

    // the same code, another person: refused, the code is used up
    let mut mallory = SoftPasskey::new(true);
    let (st, why) = join_with_code(&a, &mut mallory, "mallory", Some(&code)).await;
    assert_eq!(st, 409, "{why}");
    assert!(why.contains("already been used"), "{why}");
    // and a code nobody issued
    let (st, _) = join_with_code(&a, &mut mallory, "mallory", Some("zzzzz-zzzzz")).await;
    assert_eq!(st, 404);

    // fay already has an account and is waiting; a second code, typed there
    let mut fay = SoftPasskey::new(true);
    let (st, fay_cookie) = join_in_browser(&a, &mut fay, "fay").await;
    assert_eq!(st, 200);
    assert_eq!(get_with_cookie(&a, "/verify", &fay_cookie).await.0, 403);
    let out = owner.dd_ok(&args(&["invite"], &d));
    let code2 = out
        .lines()
        .find_map(|l| l.strip_prefix("code: "))
        .unwrap()
        .trim()
        .to_string();
    // held, not published, until she follows through
    assert!(entry(&a, "fay").await.is_none());
    let fay_held = a.dir.parent().unwrap().join("held").join("fay.json");
    let fay_root = serde_json::from_slice::<serde_json::Value>(&std::fs::read(&fay_held).unwrap())
        .unwrap()["entry"]["root"]
        .as_str()
        .unwrap()
        .to_string();
    let r = http
        .post(a.url("/_dd/redeem/start"))
        .header("cookie", &fay_cookie)
        .header("x-dd-grant", grant_header(&code2, &fay_root))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let rcr: RequestChallengeResponse = serde_json::from_value(j).unwrap();
    let cred = fay
        .do_authentication(Url::parse("https://localhost").unwrap(), rcr)
        .unwrap();
    let r = http
        .post(a.url("/_dd/redeem/sign"))
        .header("x-dd-ceremony", &ceremony)
        .json(&cred)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
    assert_eq!(get_with_cookie(&a, "/verify", &fay_cookie).await.0, 200);
    // the code is her follow-through: published now, and the hold is gone
    assert_eq!(entry(&a, "fay").await.unwrap()["entry"]["version"], 2);
    assert!(!fay_held.exists());

    // the owner sees them, and can shut one out again: revoked beats a grant
    let out = owner.dd_ok(&args(&["member", "list", "--repo", &repo], &d));
    assert!(
        out.contains("eve  invited") && out.contains("fay  invited"),
        "{out}"
    );
    owner.dd_ok(&args(&["member", "remove", "eve", "--repo", &repo], &d));
    let eve_id = identity::member_id(e["entry"]["root"].as_str().unwrap());
    let b = Box_::start_with(
        true,
        vec![a.directory()],
        1,
        Some(verify::Members {
            members: vec![],
            revoked: vec![eve_id],
        }),
        Some(release_pub.clone()),
    )
    .await;
    wait_for(|| async { entry(&b, "fay").await.is_some() && entry(&b, "eve").await.is_some() })
        .await;
    let (st, c) = login_in_browser(&b, &mut fay, "fay").await;
    assert_eq!(st, 200);
    assert_eq!(
        get_with_cookie(&b, "/verify", &c).await.0,
        200,
        "fay's grant carries to b"
    );
    let (st, c) = login_in_browser(&b, &mut eve, "eve").await;
    assert_eq!(st, 200);
    assert_eq!(
        get_with_cookie(&b, "/verify", &c).await.0,
        403,
        "eve is revoked"
    );

    // guest names are for probes, and a probe's account is dropped in time
    let mut probe = SoftPasskey::new(true);
    let (st, why) = join_in_browser(&a, &mut probe, "guest7").await;
    assert_eq!(st, 400, "{why}");
    assert!(why.contains("reserved"), "{why}");

    // an expired code is nothing
    let out = owner.dd_ok(&args(&["invite", "--ttl", "2s"], &d));
    let code3 = out
        .lines()
        .find_map(|l| l.strip_prefix("code: "))
        .unwrap()
        .trim()
        .to_string();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (st, _) = join_with_code(&a, &mut mallory, "mallory", Some(&code3)).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_demo_is_an_account_with_a_small_permission_set() {
    let tile = |name: &str, demo: Option<&str>| verify::pages::Service {
        name: name.into(),
        url: format!("https://{name}.x/"),
        icon: "".into(),
        color: "#000".into(),
        blurb: String::new(),
        demo: demo.map(String::from),
        demo_url: None,
        menu_only: false,
    };
    // no tile lets the demo do anything: there is no demo
    let plain = Box_::start_home(
        true,
        vec![],
        300,
        Some(verify::Members::list(vec![])),
        None,
        vec![tile("files", None)],
    )
    .await;
    assert_eq!(get_with_cookie(&plain, "/_dd/demo", "").await.0, 404);

    let a = Box_::start_home(
        true,
        vec![],
        300,
        Some(verify::Members::list(vec![])),
        None,
        vec![
            tile("files", Some("read")),
            tile("llm", Some("rate:2")),
            tile("grafana", Some("full")),
            tile("photos", None),
        ],
    )
    .await;
    // one click: a session as demo, sent to the home page
    let r = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(a.url("/_dd/demo"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    let cookie = r
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    // the gate, told the method and host by nginx, answers by the tile's
    // permission: read, a counted rate, everything, or nothing
    let gate = |method: &str, host: &str| {
        let c = cookie.clone();
        let u = a.url("/verify");
        let (method, host) = (method.to_string(), host.to_string());
        async move {
            let r = reqwest::Client::new()
                .get(u)
                .header("cookie", c)
                .header("x-original-method", method)
                .header("x-original-host", host)
                .send()
                .await
                .unwrap();
            (
                r.status().as_u16(),
                r.headers()
                    .get("x-auth-request-preferred-username")
                    .map(|v| v.to_str().unwrap().to_string()),
            )
        }
    };
    assert_eq!(gate("GET", "files.x").await, (200, Some("demo".into())));
    assert_eq!(gate("PROPFIND", "files.x").await.0, 200);
    assert_eq!(gate("PUT", "files.x").await.0, 403);
    assert_eq!(gate("DELETE", "files.x").await.0, 403);
    assert_eq!(gate("POST", "grafana.x").await.0, 200);
    // chat: reads are free, two prompts this hour, then no
    assert_eq!(gate("GET", "llm.x").await.0, 200);
    assert_eq!(gate("POST", "llm.x").await.0, 200);
    assert_eq!(gate("POST", "llm.x").await.0, 200);
    assert_eq!(gate("POST", "llm.x").await.0, 403);
    assert_eq!(gate("GET", "llm.x").await.0, 200);
    // no permission, no door; and a host that is no tile at all
    assert_eq!(gate("GET", "photos.x").await.0, 403);
    assert_eq!(gate("GET", "elsewhere.x").await.0, 403);
    // every service on the page, the shut one marked; the page fills
    // itself from /_dd/me, so that is where the demo's view is
    let (st, body) = get_with_cookie(&a, "/_dd/home", &cookie).await;
    assert_eq!(st, 200);
    assert!(body.contains("id=\"demo\""), "{body}");
    let (st, me) = get_with_cookie(&a, "/_dd/me", &cookie).await;
    assert_eq!(st, 200, "{me}");
    let me: serde_json::Value = serde_json::from_str(&me).unwrap();
    assert_eq!(me["demo"], true);
    let svc = |n: &str| {
        me["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(svc("files")["url"], "https://files.x/");
    assert_eq!(svc("files")["shut"], false);
    assert_eq!(svc("photos")["shut"], true, "photos is not in the demo");
    // nobody takes the demo's name
    let mut key = SoftPasskey::new(true);
    let (st, why) = join_in_browser(&a, &mut key, "demo").await;
    assert_eq!(st, 400, "{why}");
}

fn enrol_token(dev: &Device, b: &Box_) -> String {
    use std::io::BufRead as _;
    let mut c = Command::new(env!("CARGO_BIN_EXE_dd"))
        .args([
            "enrol",
            "--url",
            &b.url("/_dd/enrol"),
            "--directory",
            &b.directory(),
        ])
        .env("DD_KEYRING_FILE", &dev.keyring)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let out = std::io::BufReader::new(c.stdout.take().unwrap());
    let mut tok = None;
    for line in out.lines() {
        let line = line.unwrap();
        if let Some(i) = line.find("?t=") {
            tok = Some(line[i + 3..].trim().to_string());
            break;
        }
    }
    let _ = c.kill();
    let _ = c.wait();
    tok.expect("dd enrol printed no link")
}

#[tokio::test(flavor = "multi_thread")]
async fn passkey_made_in_a_browser_is_signed_into_the_entry_and_logs_in() {
    let a = Box_::start(true, vec![], 300).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(&["identity", "new", "--name", "amy"], &d));

    // dd enrol runs for real this time: it prints the link, waits for the
    // credential, signs it in, publishes
    let mut enrol = Command::new(env!("CARGO_BIN_EXE_dd"))
        .args([
            "enrol",
            "--url",
            &a.url("/_dd/enrol"),
            "--directory",
            &a.directory(),
        ])
        .env("DD_KEYRING_FILE", &dev.keyring)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // keep reading its stdout for the whole run: dropping the pipe would
    // make dd's later println fail
    use std::io::BufRead as _;
    let mut stdout = std::io::BufReader::new(enrol.stdout.take().unwrap());
    let mut line = String::new();
    let tok = loop {
        line.clear();
        assert!(
            stdout.read_line(&mut line).unwrap() > 0,
            "dd enrol ended early"
        );
        if let Some(i) = line.find("?t=") {
            break line[i + 3..].trim().to_string();
        }
    };

    // the browser: a software authenticator against the box's pages
    let origin = Url::parse("https://localhost").unwrap();
    let mut authenticator = SoftPasskey::new(true);
    let http = reqwest::Client::new();
    let r = http
        .post(a.url("/_dd/enrol/start"))
        .bearer_auth(&tok)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let ccr: CreationChallengeResponse = serde_json::from_value(j).unwrap();
    let reg = authenticator
        .do_registration(origin.clone(), ccr)
        .expect("software authenticator registration");
    let r = http
        .post(a.url("/_dd/enrol/finish"))
        .bearer_auth(&tok)
        .header("x-dd-ceremony", &ceremony)
        .json(&reg)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());

    // dd collects it and signs it in
    let mut out = String::new();
    std::io::Read::read_to_string(&mut stdout, &mut out).unwrap();
    let status = enrol.wait().unwrap();
    let mut err = String::new();
    std::io::Read::read_to_string(&mut enrol.stderr.take().unwrap(), &mut err).unwrap();
    assert!(status.success(), "dd enrol:\n{out}{err}");
    assert!(out.contains("signed into your entry"), "{out}");
    let e = entry(&a, "amy").await.unwrap();
    assert_eq!(e["entry"]["version"], 2);
    assert_eq!(e["entry"]["passkeys"].as_array().unwrap().len(), 1);

    // login with it: the box builds the challenge from the entry
    let r = http
        .post(a.url("/_dd/login/start"))
        .json(&serde_json::json!({ "username": "amy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let rcr: RequestChallengeResponse = serde_json::from_value(j).unwrap();
    let cred = authenticator
        .do_authentication(origin, rcr)
        .expect("software authenticator assertion");
    let r = http
        .post(a.url("/_dd/login/finish"))
        .header("x-dd-ceremony", &ceremony)
        .json(&cred)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
    let cookie = r
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let r = http
        .get(a.url("/verify"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()["x-auth-request-preferred-username"]
            .to_str()
            .unwrap(),
        "amy"
    );

    // removed from the entry, the passkey is gone everywhere at once
    dev.dd_ok(&args(
        &[
            "passkey",
            "remove",
            e["entry"]["passkeys"][0]["id"].as_str().unwrap(),
        ],
        &d,
    ));
    let r = http
        .post(a.url("/_dd/login/start"))
        .json(&serde_json::json!({ "username": "amy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    // the cookie it earned is still this box's for its lifetime - that is
    // the stated trade; a stolen session is bounded by the box and the clock
    assert_eq!(verify_status(&a, "").await, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn boxes_share_the_directory_and_refuse_squats() {
    // a and b peer with each other, quickly. We need each other's addresses
    // before starting, so bind first, drop, and start on those ports; another
    // test can take a port in between, so try again when that happens
    let (a, b) = loop {
        let a_dir = scratch("a").join("keys");
        let b_dir = scratch("b").join("keys");
        let la = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let lb = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (pa, pb) = (la.local_addr().unwrap(), lb.local_addr().unwrap());
        drop((la, lb));
        if let (Ok(a), Ok(b)) = tokio::join!(
            start_at(
                pa,
                a_dir,
                true,
                vec![format!("http://{pb}/_dd/directory")],
                1
            ),
            start_at(
                pb,
                b_dir,
                false,
                vec![format!("http://{pa}/_dd/directory")],
                1
            )
        ) {
            break (a, b);
        }
    };
    tokio::time::sleep(Duration::from_millis(1500)).await; // first pulls

    let sarah = Device::new();
    sarah.dd_ok(&[
        "identity",
        "new",
        "--name",
        "sarah",
        "--directory",
        &a.directory(),
    ]);
    // b pulls it
    wait_for(|| async { entry(&b, "sarah").await.is_some() }).await;

    // a stranger tries the name on b: the client sees it is taken
    let stranger = Device::new();
    stranger.dd_fails(
        &[
            "identity",
            "new",
            "--name",
            "sarah",
            "--directory",
            &b.directory(),
        ],
        "already has an entry",
    );

    // a box that never managed to pull its peer takes no new names
    let lonely = Box_::start(false, vec!["http://127.0.0.1:1/_dd/directory".into()], 1).await;
    stranger.dd_fails(
        &[
            "identity",
            "new",
            "--name",
            "tom",
            "--directory",
            &lonely.directory(),
        ],
        "did not take the update",
    );

    // an update via b alone reaches a
    let phone = Device::new();
    let pk = phone.public_key();
    sarah.dd_ok(&["device", "admit", &pk, "--directory", &b.directory()]);
    wait_for(|| async { entry(&a, "sarah").await.unwrap()["entry"]["version"] == 2 }).await;

    // a recovery published to a reaches b, which checks the old paper key
    let amy = Device::new();
    let out = amy.dd_ok(&[
        "identity",
        "new",
        "--name",
        "amy",
        "--directory",
        &a.directory(),
    ]);
    let paper = recovery_key(&out);
    wait_for(|| async { entry(&b, "amy").await.is_some() }).await;
    let amy2 = Device::new();
    amy2.dd_ok(&["device", "show"]);
    run_with_stdin(
        &amy2,
        &[
            "identity",
            "recover",
            "--name",
            "amy",
            "--key-stdin",
            "--directory",
            &a.directory(),
        ],
        &format!("{paper}\n"),
        true,
    );
    wait_for(|| async { entry(&b, "amy").await.unwrap()["entry"]["version"] == 2 }).await;
    assert!(entry(&b, "amy").await.unwrap()["recovery_signature"].is_string());

    // a brand-new box pointed at a fills itself, at whatever version things are
    let fresh = Box_::start(false, vec![a.directory()], 1).await;
    wait_for(|| async {
        entry(&fresh, "amy").await.is_some() && entry(&fresh, "sarah").await.is_some()
    })
    .await;
    assert_eq!(entry(&fresh, "amy").await.unwrap()["entry"]["version"], 2);

    // bob is born on a. Whether the fresh box has pulled him yet is a
    // race with its one-second pull: if it has, the cli sees the entry and
    // refuses before asking; if not, the box asks its peer before taking a
    // new name and answers 409; and if the pull lands between the cli's
    // look and its write, the box refuses the write as not newer. A
    // stranger's bob is refused every way.
    let bob = Device::new();
    bob.dd_ok(&[
        "identity",
        "new",
        "--name",
        "bob",
        "--directory",
        &a.directory(),
    ]);
    let sq = Device::new();
    sq.dd_fails_with(
        &[
            "identity",
            "new",
            "--name",
            "bob",
            "--directory",
            &fresh.directory(),
        ],
        &["409", "already has an entry for bob", "not newer"],
    );

    // a forged newer entry pushed straight at the fresh box
    let mut forged = entry(&fresh, "amy").await.unwrap();
    forged["entry"]["version"] = serde_json::json!(9);
    forged["signature"] = serde_json::json!(base64_zero());
    assert_eq!(put_raw(&fresh, "amy", &forged).await.0, 403);
}

async fn start_at(
    addr: std::net::SocketAddr,
    dir: PathBuf,
    full: bool,
    peers: Vec<String>,
    sync_secs: u64,
) -> anyhow::Result<Box_> {
    let (addr, _task) = verify::start(verify::Config {
        home: vec![],
        fleet: Default::default(),
        demo_library: None,
        tmdb: None,
        app_manifest: None,
        members: None,
        release_pub: None,
        web_dir: None,
        photos: None,
        library: None,
        network: None,
        bind: addr,
        dir: dir.clone(),
        peers,
        sync_secs,
        domain: full.then(|| "localhost".to_string()),
        oidc: None,
    })
    .await?;
    Ok(Box_ { addr, dir })
}

async fn wait_for<F, Fut>(mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("condition never became true");
}

#[tokio::test(flavor = "multi_thread")]
async fn publish_brings_a_lagging_box_up() {
    let a = Box_::start(false, vec![], 300).await;
    let b = Box_::start(false, vec![], 300).await;
    let dev = Device::new();
    dev.dd_ok(&[
        "identity",
        "new",
        "--name",
        "kim",
        "--directory",
        &a.directory(),
    ]);
    assert!(entry(&b, "kim").await.is_none());
    let out = dev.dd_ok(&args(&["identity", "publish"], &dirs([&a, &b])));
    assert!(out.contains("accepted version 1"), "{out}");
    assert!(entry(&b, "kim").await.is_some());
    let _ = &a.dir;
}

/// Answer every request with `body`, from a thread: enough of a forge to
/// serve one file.
fn serve_forever(body: String) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let r = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = std::io::Write::write_all(&mut s, r.as_bytes());
        }
    });
    format!("http://{addr}/app.json")
}

async fn download_page_of(release_pub: String, manifest_url: String) -> (u16, String) {
    let (addr, _task) = verify::start(verify::Config {
        home: vec![],
        members: Some(verify::Members::list(vec![])),
        release_pub: Some(release_pub),
        web_dir: None,
        photos: None,
        library: None,
        fleet: Default::default(),
        demo_library: None,
        tmdb: None,
        app_manifest: Some(manifest_url),
        network: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        dir: scratch("box").join("keys"),
        peers: vec![],
        sync_secs: 300,
        domain: Some("localhost".to_string()),
        oidc: None,
    })
    .await
    .unwrap();
    let r = reqwest::get(format!("http://{addr}/_dd/download"))
        .await
        .unwrap();
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

/// The downloads page is the one page a stranger is sent to, and what it
/// offers is what they install. It offers exactly the files a manifest
/// signed with the release key names, with their hashes - and nothing at
/// all if the manifest is signed by anything else.
#[tokio::test(flavor = "multi_thread")]
async fn the_download_page_offers_only_what_the_release_key_signed() {
    let key = release::generate();
    let public = release::encode_public(&key.verifying_key());
    let apk_sha = "a1b2c3d4e5f6".repeat(5) + "abcd";
    let manifest = serde_json::json!({
        "tag": "v9.9.9",
        "commit": "0123456789abcdef",
        "from": "someone/commonty",
        "files": {
            "commonty.apk": { "sha256": apk_sha, "url": "https://example.invalid/commonty.apk" },
            // not a sha256: dropped rather than shown
            "commonty-setup.exe": { "sha256": "trust me", "url": "https://example.invalid/setup.exe" },
        }
    });

    let good = release::sign_doc("app", manifest.clone(), &key).unwrap();
    let (st, page) = download_page_of(
        public.clone(),
        serve_forever(serde_json::to_string(&good).unwrap()),
    )
    .await;
    assert_eq!(st, 200, "{page}");
    assert!(
        page.contains("https://example.invalid/commonty.apk"),
        "{page}"
    );
    assert!(page.contains(&apk_sha[..12]), "the hash is shown");
    assert!(
        !page.contains("setup.exe"),
        "a file with no real hash is not offered"
    );
    assert!(page.contains("v9.9.9"));

    // the same manifest, signed by some other key: nothing offered
    let forged = release::sign_doc("app", manifest.clone(), &release::generate()).unwrap();
    let (st, _) = download_page_of(
        public.clone(),
        serve_forever(serde_json::to_string(&forged).unwrap()),
    )
    .await;
    assert_eq!(st, 404);

    // signed by the right key but edited on the way: nothing offered
    let tampered = serde_json::to_string(&good)
        .unwrap()
        .replace("example.invalid/commonty.apk", "example.invalid/evil.apk");
    let (st, _) = download_page_of(public, serve_forever(tampered)).await;
    assert_eq!(st, 404);
}

/// A new box learns names from its peers. When they disagree about whose a
/// name is, it takes neither: taking the first one heard would let a lying
/// peer seed `david` under its own root, and the accept rule would then
/// refuse the real one for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_box_takes_no_side_when_its_peers_disagree_about_a_name() {
    // two directories that never talk: each has its own `david`
    let honest = Box_::start(false, vec![], 300).await;
    let liar = Box_::start(false, vec![], 300).await;
    Device::new().dd_ok(&[
        "identity",
        "new",
        "--name",
        "david",
        "--directory",
        &honest.directory(),
    ]);
    Device::new().dd_ok(&[
        "identity",
        "new",
        "--name",
        "david",
        "--directory",
        &liar.directory(),
    ]);
    let (h, l) = (
        entry(&honest, "david").await.unwrap(),
        entry(&liar, "david").await.unwrap(),
    );
    assert_ne!(
        h["entry"]["root"], l["entry"]["root"],
        "two different people"
    );

    // a new box that pulls from both, and a name only one of them has
    Device::new().dd_ok(&[
        "identity",
        "new",
        "--name",
        "carol",
        "--directory",
        &honest.directory(),
    ]);
    let fresh = Box_::start(false, vec![honest.directory(), liar.directory()], 1).await;
    wait_for(|| async { entry(&fresh, "carol").await.is_some() }).await;

    // carol, whom nobody disputes, arrived; david, whom they dispute, did not
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        entry(&fresh, "david").await.is_none(),
        "the new box took a side on a disputed name"
    );
}
