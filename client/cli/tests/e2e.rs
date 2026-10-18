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
    async fn start(full: bool) -> Self {
        Self::start_members(full, None).await
    }
    /// `members`: Some(list) gates the services the way a real box does
    async fn start_members(full: bool, members: Option<Vec<String>>) -> Self {
        Self::start_with(full, members.map(verify::Members::list), None).await
    }
    /// everything: the member list with revocations, and the release key
    /// whose invites count
    async fn start_with(
        full: bool,
        members: Option<verify::Members>,
        release_pub: Option<String>,
    ) -> Self {
        Self::start_home(full, members, release_pub, vec![]).await
    }
    /// and the tiles this box offers
    async fn start_home(
        full: bool,
        members: Option<verify::Members>,
        release_pub: Option<String>,
        home: Vec<verify::pages::Service>,
    ) -> Self {
        Self::start_on(
            scratch("box").join("keys"),
            full,
            members,
            release_pub,
            home,
        )
        .await
    }
    /// a box whose release names a reader key, as one does once the mail
    /// Worker has its own: no single entry is anyone's any more
    async fn start_closed(full: bool, members: Option<verify::Members>, forge: &Forge) -> Self {
        let reader = identity::encode_public(&identity::generate().verifying_key());
        Self::start_full(
            scratch("box").join("keys"),
            full,
            members,
            None,
            vec![],
            vec![reader],
            Some(forge.url.clone()),
        )
        .await
    }
    /// a box that serves the member list `forge` publishes
    async fn start_listed(full: bool, members: Option<verify::Members>, forge: &Forge) -> Self {
        Self::start_on_listed(scratch("box").join("keys"), full, members, forge).await
    }
    async fn start_on_listed(
        dir: PathBuf,
        full: bool,
        members: Option<verify::Members>,
        forge: &Forge,
    ) -> Self {
        Self::start_full(
            dir,
            full,
            members,
            None,
            vec![],
            vec![],
            Some(forge.url.clone()),
        )
        .await
    }
    /// on a state directory another box already used: the same box, started
    /// again with a new release
    async fn start_on(
        dir: PathBuf,
        full: bool,
        members: Option<verify::Members>,
        release_pub: Option<String>,
        home: Vec<verify::pages::Service>,
    ) -> Self {
        Self::start_full(dir, full, members, release_pub, home, vec![], None).await
    }
    async fn start_full(
        dir: PathBuf,
        full: bool,
        members: Option<verify::Members>,
        release_pub: Option<String>,
        home: Vec<verify::pages::Service>,
        directory_readers: Vec<String>,
        member_list: Option<String>,
    ) -> Self {
        Self::start_full_with(
            dir,
            full,
            members,
            release_pub,
            home,
            directory_readers,
            member_list,
            None,
        )
        .await
    }
    /// a box that holds the libraries bucket (`library`), with the member
    /// list `forge` publishes
    async fn start_with_libraries(forge: &Forge, library: verify::library::Gate) -> Self {
        Self::start_full_with(
            scratch("box").join("keys"),
            true,
            None,
            None,
            vec![],
            vec![],
            Some(forge.url.clone()),
            Some(library),
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn start_full_with(
        dir: PathBuf,
        full: bool,
        members: Option<verify::Members>,
        release_pub: Option<String>,
        home: Vec<verify::pages::Service>,
        directory_readers: Vec<String>,
        member_list: Option<String>,
        library: Option<verify::library::Gate>,
    ) -> Self {
        let (addr, _task) = verify::start(verify::Config {
            mail: None,
            home,
            members,
            release_pub,
            web_dir: None,
            photos: None,
            library,
            fleet: Default::default(),
            thanos: None,
            adblock: None,
            forge_events: None,
            storage_ledger: None,
            demo_library: None,
            tmdb: None,
            search: None,
            id_origin: None,
            box_only: false,
            owner: None,
            admin: None,
            boxes: Default::default(),
            oidc: None,
            app_manifest: None,
            member_list,
            network: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            dir: dir.clone(),
            directory_readers,
            names: vec!["127.0.0.1".into()],
            control: None,
            identity_from: Default::default(),
            house_peers: vec![],
            domain: full.then(|| "localhost".to_string()),
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

/// the release key every `dd` here holds other members' entries to
/// (DD_RELEASE_PUB): the test's own, standing in for fleet/release.pub
fn release_key() -> &'static identity::SigningKey {
    static K: std::sync::OnceLock<identity::SigningKey> = std::sync::OnceLock::new();
    K.get_or_init(identity::generate)
}

fn release_pub() -> String {
    identity::encode_public(&release_key().verifying_key())
}

/// The releases branch, as far as a box reads the member list from it:
/// signed by `release_key`, a newer one each `list`. A box keeps what it
/// read for minutes, so a test lists before anything asks.
struct Forge {
    body: std::sync::Arc<std::sync::Mutex<String>>,
    url: String,
    counter: std::sync::atomic::AtomicU64,
}

impl Forge {
    fn new() -> Self {
        let body = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let url = serve(body.clone()).replace("/app.json", "/members.json");
        Self {
            body,
            url,
            counter: Default::default(),
        }
    }
    /// each member by id, under the name `dd member add` confirmed
    fn list(&self, members: &[(String, &str)], revoked: &[String]) {
        let list = identity::MemberList {
            // from the floor the build holds out for, as a real release's would be
            counter: identity::fleet::MEMBER_LIST_FLOOR
                + self.counter.fetch_add(1, Ordering::Relaxed)
                + 1,
            members: members.iter().map(|(id, _)| id.clone()).collect(),
            revoked: revoked.to_vec(),
            names: members
                .iter()
                .map(|(id, name)| (id.clone(), identity::name_tag(id, name)))
                .collect(),
        };
        let signed = identity::sign_member_list(list, release_key()).unwrap();
        *self.body.lock().unwrap() = serde_json::to_string(&signed).unwrap();
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
            .env("DD_RELEASE_PUB", release_pub())
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

/// the member id `dd identity show` prints for this device's own entry
fn member_id_of(dev: &Device, d: &[String]) -> String {
    dev.dd_ok(&args(&["identity", "show"], d))
        .lines()
        .find_map(|l| l.strip_prefix("member id:"))
        .expect("identity show prints the member id")
        .trim()
        .to_string()
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

/// what the directory answers a request with no credential at all
async fn anonymous(b: &Box_, path: &str) -> u16 {
    reqwest::get(format!("{}{path}", b.directory()))
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Once the release names a reader key, an entry is nobody's to read
/// without a credential: and still a name is taken, a device admitted by
/// the root, an identity recovered by the paper key on a bare machine, and
/// a browser signed in by its passkey.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_directory_still_signs_up_admits_recovers_and_signs_in() {
    let forge = Forge::new();
    let a = Box_::start_closed(true, None, &forge).await;
    let d = dirs([&a]);
    let laptop = Device::new();
    let out = laptop.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
    let paper = recovery_key(&out);
    // the release that lets him in, which a paper key on a new machine
    // holds his entry to
    forge.list(&[(member_id_of(&laptop, &d), "tom")], &[]);
    assert_eq!(anonymous(&a, "").await, 401);
    assert_eq!(anonymous(&a, "/tom").await, 401);
    assert_eq!(anonymous(&a, "/nobody").await, 401);

    // the name is taken: the publish says so, not a read
    Device::new().dd_fails(
        &args(
            &[
                "identity",
                "new",
                "--born",
                "1990-01",
                "--agree-terms",
                "--name",
                "tom",
            ],
            &d,
        ),
        "already an entry for tom",
    );

    // the laptop reads its own entry and admits a phone
    let show = laptop.dd_ok(&args(&["identity", "show"], &d));
    assert!(
        show.contains("version 1") && show.contains("(yours)"),
        "{show}"
    );
    let phone = Device::new();
    let pk = phone.public_key();
    laptop.dd_ok(&args(&["device", "admit", &pk], &d));
    let show = laptop.dd_ok(&args(&["identity", "show"], &d));
    assert!(show.contains("version 2"), "{show}");

    // the paper key, on a machine that is no device of tom's
    let bare = Device::new();
    bare.dd_ok(&["device", "show"]);
    let out = run_with_stdin(
        &bare,
        &args(&["identity", "recover", "--name", "tom", "--key-stdin"], &d),
        &format!("{paper}\n"),
        true,
    );
    let _ = recovery_key(&out);
    let show = bare.dd_ok(&args(&["identity", "show"], &d));
    assert!(
        show.contains("version 3") && show.contains("(this one)"),
        "{show}"
    );
    // the laptop is no device of tom's now, nor its root his: it reads nothing
    let after = laptop.dd_ok(&args(&["identity", "show"], &d));
    assert!(after.contains("401"), "{after}");

    // a browser: an account by passkey, then a sign-in by it
    let mut eve = SoftPasskey::new(true);
    assert_eq!(join_in_browser(&a, &mut eve, "eve").await.0, 200);
    let (st, _) = login_in_browser(&a, &mut eve, "eve").await;
    assert_eq!(st, 200);
    assert_eq!(anonymous(&a, "/eve").await, 401);
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
    let forge = Forge::new();
    let a = Box_::start_listed(true, None, &forge).await;
    let b = Box_::start(false).await;
    let d = dirs([&a, &b]);
    let laptop = Device::new();
    let phone = Device::new();
    let stranger = Device::new();

    // birth: published to both, recovery key printed once
    let out = laptop.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "sarah",
        ],
        &d,
    ));
    let paper = recovery_key(&out);
    forge.list(&[(member_id_of(&laptop, &d), "sarah")], &[]);
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

/// The paper key keeps a member a member: the recovered entry names the
/// root it replaces, signed by the paper key the entry on file named, and
/// the gate follows that, so the member lands on their home page and not
/// the waiting one. A recovery signed by any other key moves nothing, and
/// one by the right key escapes no removal.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_member_stays_a_member_and_a_forged_recovery_moves_nothing() {
    let a = Box_::start_members(true, Some(vec![])).await;
    let laptop = Device::new();
    let out = laptop.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "sam",
        ],
        &dirs([&a]),
    ));
    let paper = recovery_key(&out);
    let e = entry(&a, "sam").await.unwrap();
    let old_root = e["entry"]["root"].as_str().unwrap().to_string();
    let id = identity::member_id(&old_root);
    let forge = Forge::new();
    forge.list(&[(id.clone(), "sam")], &[]);
    let b = Box_::start_on_listed(
        a.dir.clone(),
        true,
        Some(verify::Members::list(vec![id.clone()])),
        &forge,
    )
    .await;
    let removed = Box_::start_on(
        a.dir.clone(),
        true,
        Some(verify::Members {
            members: vec![id.clone()],
            revoked: vec![id.clone()],
            ..Default::default()
        }),
        None,
        vec![],
    )
    .await;
    let d = dirs([&b]);
    assert_eq!(verify_status(&b, &laptop.token()).await, 200);
    assert_eq!(verify_status(&removed, &laptop.token()).await, 403);

    // the laptop is lost; the paper key, on a new machine
    let bare = Device::new();
    bare.dd_ok(&["device", "show"]);
    let out = run_with_stdin(
        &bare,
        &args(&["identity", "recover", "--name", "sam", "--key-stdin"], &d),
        &format!("{paper}\n"),
        true,
    );
    let next_paper = recovery_key(&out);
    let now = entry(&b, "sam").await.unwrap();
    assert_ne!(now["entry"]["root"], old_root.as_str());
    assert_eq!(
        now["entry"]["recovered_from"],
        serde_json::json!([old_root])
    );
    assert_eq!(
        verify_status(&b, &bare.token()).await,
        200,
        "still a member"
    );
    assert_eq!(verify_status(&b, &laptop.token()).await, 401);
    // signed in, the home page is a member's, not the waiting one
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let r = http
        .post(b.url("/_dd/app/signin"))
        .form(&[("token", bare.token().as_str())])
        .send()
        .await
        .unwrap();
    let cookie = r
        .headers()
        .get("set-cookie")
        .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_string())
        .expect("signed in");
    let (st, home) = get_with_cookie(&b, "/_dd/home", &cookie).await;
    assert_eq!(st, 200);
    assert!(
        home.contains("id=\"services\"") && !home.contains("Your account is made"),
        "{home}"
    );
    assert_eq!(get_with_cookie(&b, "/_dd/me", &cookie).await.0, 200);
    // the owner removed the old root: the recovery does not undo that
    assert_eq!(verify_status(&removed, &bare.token()).await, 403);

    // a recovery to a root of someone else's choosing: refused, whether
    // co-signed by a key of theirs, by the old paper key (no longer on
    // file), or by the right one with the earlier roots made up
    let cur: identity::SignedEntry = serde_json::from_value(now.clone()).unwrap();
    let thief = identity::generate();
    let forged = |recovery: &ed25519_dalek::SigningKey, earlier: Vec<String>| {
        let mut e = cur.entry.clone();
        e.root = identity::encode_public(&thief.verifying_key());
        e.recovery = identity::encode_public(&identity::generate().verifying_key());
        e.recovered_from = earlier;
        e.devices = vec![identity::Device {
            fingerprint: "thief".into(),
            public_key: identity::encode_public(&thief.verifying_key()),
            added: 1,
        }];
        e.version += 1;
        serde_json::to_value(identity::sign_recovery(e, &thief, recovery).unwrap()).unwrap()
    };
    let chain = || {
        cur.entry
            .recovered_from
            .iter()
            .cloned()
            .chain([cur.entry.root.clone()])
            .collect::<Vec<_>>()
    };
    let old_paper = identity::decode_secret(&paper).unwrap();
    let new_paper = identity::decode_secret(&next_paper).unwrap();
    for (why, body) in [
        ("a stranger's key", forged(&identity::generate(), chain())),
        ("the old paper key", forged(&old_paper, chain())),
        ("made-up earlier roots", forged(&new_paper, vec![])),
    ] {
        let (st, said) = put_raw(&b, "sam", &body).await;
        assert_eq!(st, 403, "{why}: {said}");
    }
    assert_eq!(entry(&b, "sam").await.unwrap(), now, "nothing moved");
    assert_eq!(verify_status(&b, &bare.token()).await, 200);
}

/// An entry signed by `root`, for a person or a box the test plays: one
/// device, the libraries given, the terms agreed.
fn made_entry(
    name: &str,
    root: &identity::SigningKey,
    recovery: &str,
    recovered_from: Vec<String>,
    libraries: Vec<identity::Library>,
) -> identity::Entry {
    let device = identity::encode_public(&identity::generate().verifying_key());
    identity::Entry {
        name: name.into(),
        root: identity::encode_public(&root.verifying_key()),
        recovery: recovery.into(),
        recovered_from,
        devices: vec![identity::Device {
            fingerprint: identity::fingerprint(&device),
            public_key: device,
            added: 1,
        }],
        passkeys: vec![],
        grant: None,
        libraries,
        terms: Some(identity::TERMS.into()),
        version: 1,
        updated: identity::now(),
    }
}

/// What a box that lies serves for a name: written where it keeps the
/// name's entry, past every rule an honest one applies on the way in.
fn plant(b: &Box_, signed: &identity::SignedEntry) {
    std::fs::write(
        b.dir.join(format!("{}.json", signed.entry.name)),
        serde_json::to_vec(signed).unwrap(),
    )
    .unwrap();
}

/// A recovery the box says it took, beside the entry
fn plant_handover(b: &Box_, before: &identity::SignedEntry, after: &identity::SignedEntry) {
    let dir = b.dir.parent().unwrap().join("recoveries");
    std::fs::write(
        dir.join(format!("{}.json", after.entry.name)),
        serde_json::to_vec(&[identity::Handover {
            before: before.clone(),
            after: after.clone(),
        }])
        .unwrap(),
    )
    .unwrap();
}

/// Finding: a recovery took the entry it reseals on first sight, so a box
/// could serve one under a root of its own - the member's library ids, keys
/// of its choosing sealed to the paper key (public) - and read every write
/// after. The entry is held to the release now, and every recovery that
/// led to it has to be proven; the honest ones still go through.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_reseals_only_an_entry_the_release_anchors() {
    let forge = Forge::new();
    let a = Box_::start_listed(true, None, &forge).await;
    let d = dirs([&a]);
    let laptop = Device::new();
    let out = laptop.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "sam",
        ],
        &d,
    ));
    let paper = recovery_key(&out);
    let paper_pub =
        identity::encode_public(&identity::decode_secret(&paper).unwrap().verifying_key());
    let first = member_id_of(&laptop, &d);
    forge.list(&[(first.clone(), "sam")], &[]);
    // the owner's repo lists him too
    let repo = scratch("repo");
    std::fs::create_dir_all(repo.join("fleet")).unwrap();
    let repo = repo.to_str().unwrap().to_string();
    let file = || -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(format!("{repo}/fleet/members.json")).unwrap(),
        )
        .unwrap()
    };
    laptop.dd_ok(&args(
        &["member", "add", "sam", "--repo", &repo, "--confirm", &first],
        &d,
    ));
    let real: identity::SignedEntry =
        serde_json::from_value(entry(&a, "sam").await.unwrap()).unwrap();

    // the box's own root, its own library key sealed to the paper key
    let boxed = identity::generate();
    let lib = |key: &library::Key| identity::Library {
        id: library::random_id(),
        keys: vec![identity::SealedKey {
            to: "recovery".into(),
            sealed: library::seal_to(&paper_pub, &key[..]).unwrap(),
        }],
        readers: vec![],
        created: 1,
    };
    let theirs = library::random_key();
    let mut forged = made_entry("sam", &boxed, &paper_pub, vec![], vec![lib(&theirs)]);
    forged.version = real.entry.version + 1;
    let bare = Device::new();
    bare.dd_ok(&["device", "show"]);
    let recover = |dev: &Device, paper: &str, ok: bool| {
        run_with_stdin(
            dev,
            &args(&["identity", "recover", "--name", "sam", "--key-stdin"], &d),
            &format!("{paper}\n"),
            ok,
        )
    };
    plant(&a, &identity::sign(forged.clone(), &boxed).unwrap());
    let out = recover(&bare, &paper, false);
    assert!(out.contains("does not follow"), "{out}");

    // the same, naming sam's real root as the one it replaced, with a
    // handover the box made up: its own key co-signs, not sam's paper key
    forged.recovered_from = vec![real.entry.root.clone()];
    let forged = identity::sign_recovery(forged, &boxed, &boxed).unwrap();
    plant(&a, &forged);
    plant_handover(&a, &real, &forged);
    let out = recover(&bare, &paper, false);
    assert!(out.contains("does not follow"), "{out}");
    assert_eq!(
        entry(&a, "sam").await.unwrap()["entry"]["root"],
        forged.entry.root.as_str()
    );

    // the honest box again: the paper key recovers, anchored by the list
    plant(&a, &real);
    std::fs::remove_file(a.dir.parent().unwrap().join("recoveries/sam.json")).unwrap();
    let next = recovery_key(&recover(&bare, &paper, true));
    // and again on another new machine: the root now is on no list, and the
    // recovery the box kept proves where it came from
    let other = Device::new();
    other.dd_ok(&["device", "show"]);
    let third = recovery_key(&recover(&other, &next, true));
    let now = entry(&a, "sam").await.unwrap();
    assert_eq!(now["entry"]["recovered_from"].as_array().unwrap().len(), 2);
    // the owner lists the root he has now: the first leaves the list, so
    // whoever holds it vouches for nobody, and is not revoked, which would
    // shut sam himself out at every box
    let current = member_id_of(&other, &d);
    let out = laptop.dd_ok(&args(
        &[
            "member",
            "add",
            "sam",
            "--repo",
            &repo,
            "--confirm",
            &current,
        ],
        &d,
    ));
    assert!(out.contains("every step proven"), "{out}");
    let f = file();
    assert_eq!(f["members"], serde_json::json!([current]), "{f}");
    assert!(f["revoked"].as_array().unwrap().is_empty(), "{f}");
    assert_eq!(f["names"].as_object().unwrap().len(), 1, "{f}");
    // without the proofs, that last root anchors to nothing
    std::fs::remove_file(a.dir.parent().unwrap().join("recoveries/sam.json")).unwrap();
    let last = Device::new();
    last.dd_ok(&["device", "show"]);
    let out = recover(&last, &third, false);
    assert!(out.contains("does not follow"), "{out}");
}

/// Finding: `dd member add` and `remove` took a name's root on first
/// sight and followed its `recovered_from`, so a box could put its own root
/// on the list, take a revoked id off it, or have a made-up one revoked.
#[tokio::test(flavor = "multi_thread")]
async fn the_member_list_takes_no_directory_s_word_for_whose_root_a_name_has() {
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
    let id = member_id_of(&dev, &d);
    let repo = scratch("repo");
    std::fs::create_dir_all(repo.join("fleet")).unwrap();
    let repo = repo.to_str().unwrap().to_string();
    let file = || std::fs::read_to_string(format!("{repo}/fleet/members.json")).unwrap();
    dev.dd_ok(&args(
        &["member", "add", "tom", "--repo", &repo, "--confirm", &id],
        &d,
    ));
    dev.dd_ok(&args(
        &[
            "member", "suspend", &id, "--reason", "spam", "--repo", &repo,
        ],
        &d,
    ));
    let before = file();

    // the box serves tom under its own root, recovered from his revoked one
    let boxed = identity::generate();
    let real: identity::SignedEntry =
        serde_json::from_value(entry(&a, "tom").await.unwrap()).unwrap();
    let mut forged = made_entry("tom", &boxed, "", vec![real.entry.root.clone()], vec![]);
    forged.version = real.entry.version + 1;
    let forged = identity::sign(forged, &boxed).unwrap();
    plant(&a, &forged);
    let boxed_id = identity::member_id(&forged.entry.root);
    // the id shown is the box's, and confirming tom's real one fails
    let out = dev.dd_fails(
        &args(
            &["member", "add", "tom", "--repo", &repo, "--confirm", &id],
            &d,
        ),
        "is not the member id",
    );
    assert!(out.contains(&boxed_id), "{out}");
    // an operator who confirms what was shown still takes no revoked id
    // off the list on a directory's say
    dev.dd_fails(
        &args(
            &[
                "member",
                "add",
                "tom",
                "--repo",
                &repo,
                "--confirm",
                &boxed_id,
            ],
            &d,
        ),
        "is revoked",
    );
    // by name, the repo's own list says which ids: tom's, revoked already
    dev.dd_fails(
        &args(
            &[
                "member",
                "suspend",
                "tom",
                "--reason",
                "spam",
                "--repo",
                &repo,
                "--confirm",
                &id,
            ],
            &d,
        ),
        "already suspended",
    );
    assert_eq!(file(), before, "nothing changed");
    // and the list puts tom's name beside no id for the box's entry
    let out = dev.dd_ok(&args(&["member", "list", "--repo", &repo], &d));
    assert!(!out.contains("tom"), "{out}");
}

/// Finding: a library "shared by" anyone with an entry was opened and
/// mounted, so a box could plant one sealed to a member's device and read
/// what they put in it. Only a sharer the release lets in counts now.
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_library_comes_only_from_a_member_the_release_lets_in() {
    let forge = Forge::new();
    let a = Box_::start_listed(true, None, &forge).await;
    let d = dirs([&a]);
    let bob = Device::new();
    bob.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "bob",
        ],
        &d,
    ));
    let to_bob = bob.public_key();
    let shared = |owner: &str| identity::Library {
        id: library::random_id(),
        keys: vec![],
        readers: vec![identity::Reader {
            name: "bob".into(),
            keys: vec![identity::SealedKey {
                to: format!("device:{}", identity::fingerprint(&to_bob)),
                sealed: library::seal_to(&to_bob, &library::random_key()[..]).unwrap(),
            }],
            writes: false,
        }],
        created: owner.len() as u64,
    };
    // alice is a member, and shares one with him
    let alice = identity::generate();
    let hers = shared("alice");
    let e = made_entry("alice", &alice, "", vec![], vec![hers.clone()]);
    let (st, why) = put_raw(
        &a,
        "alice",
        &serde_json::to_value(identity::sign(e, &alice).unwrap()).unwrap(),
    )
    .await;
    assert_eq!(st, 200, "{why}");
    // carol is anyone with an entry; dave is the box's, naming alice's root
    // as one it was recovered from
    let carol = identity::generate();
    let planted = shared("carol");
    let e = made_entry("carol", &carol, "", vec![], vec![planted.clone()]);
    let (st, why) = put_raw(
        &a,
        "carol",
        &serde_json::to_value(identity::sign(e, &carol).unwrap()).unwrap(),
    )
    .await;
    assert_eq!(st, 200, "{why}");
    let boxed = identity::generate();
    let borrowed = shared("dave");
    let e = made_entry(
        "dave",
        &boxed,
        "",
        vec![identity::encode_public(&alice.verifying_key())],
        vec![borrowed.clone()],
    );
    plant(&a, &identity::sign(e, &boxed).unwrap());
    // and mallory, a member, whose own listed root the box has her sign
    // an entry under another member's name with
    let mallory = identity::generate();
    let mallory_id = identity::member_id(&identity::encode_public(&mallory.verifying_key()));
    forge.list(
        &[
            (
                identity::member_id(&identity::encode_public(&alice.verifying_key())),
                "alice",
            ),
            (member_id_of(&bob, &d), "bob"),
            (mallory_id, "mallory"),
        ],
        &[],
    );

    let out = bob.dd_ok(&args(&["library", "list"], &d));
    assert!(out.contains(&format!("{}  alice's", hers.id)), "{out}");
    assert!(!out.contains(&planted.id), "{out}");
    assert!(!out.contains(&borrowed.id), "{out}");

    let posing = shared("alice");
    let mut e = made_entry("alice", &mallory, "", vec![], vec![posing.clone()]);
    e.version = 2;
    plant(&a, &identity::sign(e, &mallory).unwrap());
    let out = bob.dd_ok(&args(&["library", "list"], &d));
    assert!(
        !out.contains(&posing.id),
        "a member's root under alice's name: {out}"
    );
}

/// Enough of S3 for the library gate, in threads: presigned GET, HEAD, PUT
/// and DELETE of one object, a signed copy (x-amz-copy-source) and a
/// listing. Etags are from the bytes, as S3's are.
fn fake_bucket() -> verify::library::Gate {
    use std::collections::BTreeMap;
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::sync::{Arc, Mutex};
    let store: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Default::default();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", l.local_addr().unwrap());
    let unpct = |s: &str| {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    };
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let store = store.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let mut len = 0usize;
                let mut copy = None;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap_or((&h, ""));
                    let v = v.trim().to_string();
                    match k.to_ascii_lowercase().as_str() {
                        "content-length" => len = v.parse().unwrap_or(0),
                        "x-amz-copy-source" => copy = Some(unpct(&v)),
                        _ => {}
                    }
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let target = parts.next().unwrap_or("");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let object = unpct(path.trim_start_matches("/b/"));
                let tag = |v: &[u8]| {
                    use sha2::Digest as _;
                    let d = sha2::Sha256::digest(v);
                    format!("\"{:x}\"", u128::from_be_bytes(d[..16].try_into().unwrap()))
                };
                let mut s = store.lock().unwrap();
                let (status, etag, out) = match method.as_str() {
                    "GET" if query.contains("list-type") => {
                        let prefix = query
                            .split('&')
                            .find_map(|kv| kv.strip_prefix("prefix="))
                            .map(unpct)
                            .unwrap_or_default();
                        let delim = query.contains("delimiter=");
                        let mut xml = String::from("<ListBucketResult>");
                        let mut dirs = std::collections::BTreeSet::new();
                        for (k, v) in s.iter().filter(|(k, _)| k.starts_with(&prefix)) {
                            let rest = &k[prefix.len()..];
                            if delim && let Some(i) = rest.find('/') {
                                dirs.insert(format!("{prefix}{}", &rest[..=i]));
                                continue;
                            }
                            xml.push_str(&format!(
                                "<Contents><Key>{k}</Key><Size>{}</Size><ETag>{}</ETag></Contents>",
                                v.len(),
                                tag(v).replace('"', "&quot;")
                            ));
                        }
                        for d in dirs {
                            xml.push_str(&format!(
                                "<CommonPrefixes><Prefix>{d}</Prefix></CommonPrefixes>"
                            ));
                        }
                        xml.push_str("<IsTruncated>false</IsTruncated></ListBucketResult>");
                        ("200 OK", String::new(), xml.into_bytes())
                    }
                    "GET" | "HEAD" => match s.get(&object) {
                        Some(v) => ("200 OK", tag(v), v.clone()),
                        None => ("404 Not Found", String::new(), vec![]),
                    },
                    "PUT" => {
                        let v = match copy.as_deref() {
                            Some(from) => s
                                .get(from.trim_start_matches("/b/"))
                                .cloned()
                                .unwrap_or_default(),
                            None => body,
                        };
                        let t = tag(&v);
                        s.insert(object, v);
                        ("200 OK", t, vec![])
                    }
                    "DELETE" => {
                        s.remove(&object);
                        ("204 No Content", String::new(), vec![])
                    }
                    _ => ("405 Method Not Allowed", String::new(), vec![]),
                };
                drop(s);
                let mut c = c;
                let etag = if etag.is_empty() {
                    String::new()
                } else {
                    format!("etag: {etag}\r\n")
                };
                let _ = write!(
                    c,
                    "HTTP/1.1 {status}\r\n{etag}content-length: {}\r\nconnection: close\r\n\r\n",
                    out.len()
                );
                if method != "HEAD" {
                    let _ = c.write_all(&out);
                }
            });
        }
    });
    verify::library::Gate {
        endpoint,
        bucket: "b".into(),
        key_id: "k".into(),
        key_secret: "s".into(),
        region: "garage".into(),
    }
}

/// An owner who is not a member (left, or suspended) has their own
/// library to read and nothing more, and no one they named a writer has
/// more than that either: the writer reads it and writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_writer_for_an_owner_no_longer_a_member_only_reads() {
    let forge = Forge::new();
    let a = Box_::start_with_libraries(&forge, fake_bucket()).await;
    let d = dirs([&a]);
    let bob = Device::new();
    bob.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "bob",
        ],
        &d,
    ));
    let to = bob.public_key();
    let family = identity::Library {
        id: library::random_id(),
        keys: vec![],
        readers: vec![identity::Reader {
            name: "bob".into(),
            keys: vec![identity::SealedKey {
                to: format!("device:{}", identity::fingerprint(&to)),
                sealed: library::seal_to(&to, &library::random_key()[..]).unwrap(),
            }],
            writes: true,
        }],
        created: 1,
    };
    let alice = identity::generate();
    let e = made_entry("alice", &alice, "", vec![], vec![family.clone()]);
    let (st, why) = put_raw(
        &a,
        "alice",
        &serde_json::to_value(identity::sign(e, &alice).unwrap()).unwrap(),
    )
    .await;
    assert_eq!(st, 200, "{why}");
    // the same box again, now holding to a list where bob is a member and
    // alice, who named him, is not
    let bob_id = member_id_of(&bob, &d);
    forge.list(&[(bob_id.clone(), "bob")], &[]);
    let a = Box_::start_full_with(
        a.dir.clone(),
        true,
        Some(verify::Members::list(vec![bob_id])),
        None,
        vec![],
        vec![],
        Some(forge.url.clone()),
        Some(fake_bucket()),
    )
    .await;
    let http = reqwest::Client::new();
    let at = |p: &str| a.url(&format!("/_dd/dav/{}/{p}", family.id));
    let r = http
        .put(at("cal/e.ics"))
        .bearer_auth(bob.token())
        .body("late")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "a writer for an owner who is not a member");
    let r = http
        .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), at(""))
        .bearer_auth(bob.token())
        .header("depth", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 207, "and still reads it");
}

/// A library its owner lets one member write and another only read: the
/// writer saves and trashes and never deletes for good; the reader writes
/// nothing; a save from an old version is refused for either.
#[tokio::test(flavor = "multi_thread")]
async fn a_writer_saves_into_a_shared_library_and_a_reader_does_not() {
    let forge = Forge::new();
    let a = Box_::start_with_libraries(&forge, fake_bucket()).await;
    let d = dirs([&a]);
    let (bob, carol) = (Device::new(), Device::new());
    for (dev, name) in [(&bob, "bob"), (&carol, "carol")] {
        dev.dd_ok(&args(
            &[
                "identity",
                "new",
                "--born",
                "1990-01",
                "--agree-terms",
                "--name",
                name,
            ],
            &d,
        ));
    }
    let reader = |dev: &Device, name: &str, writes| {
        let to = dev.public_key();
        identity::Reader {
            name: name.into(),
            keys: vec![identity::SealedKey {
                to: format!("device:{}", identity::fingerprint(&to)),
                sealed: library::seal_to(&to, &library::random_key()[..]).unwrap(),
            }],
            writes,
        }
    };
    let family = identity::Library {
        id: library::random_id(),
        keys: vec![],
        readers: vec![reader(&bob, "bob", true), reader(&carol, "carol", false)],
        created: 1,
    };
    let alice = identity::generate();
    let e = made_entry("alice", &alice, "", vec![], vec![family.clone()]);
    let (st, why) = put_raw(
        &a,
        "alice",
        &serde_json::to_value(identity::sign(e, &alice).unwrap()).unwrap(),
    )
    .await;
    assert_eq!(st, 200, "{why}");
    forge.list(
        &[
            (
                identity::member_id(&identity::encode_public(&alice.verifying_key())),
                "alice",
            ),
            (member_id_of(&bob, &d), "bob"),
            (member_id_of(&carol, &d), "carol"),
        ],
        &[],
    );
    let http = reqwest::Client::new();
    let at = |p: &str| a.url(&format!("/_dd/dav/{}/{p}", family.id));
    let send = |dev: &Device, m: &str, p: &str, hs: &[(&str, &str)], body: &str| {
        let mut r = http
            .request(reqwest::Method::from_bytes(m.as_bytes()).unwrap(), at(p))
            .bearer_auth(dev.token())
            .body(body.to_string());
        for (k, v) in hs {
            r = r.header(*k, *v);
        }
        r.send()
    };
    // the reader sees, and changes nothing
    let r = send(&carol, "PUT", "cal/e.ics", &[], "carol's")
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // the writer makes an event, and gets its version back
    let r = send(&bob, "PUT", "cal/e.ics", &[("if-none-match", "*")], "one")
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let one = r.headers()["etag"].to_str().unwrap().to_string();
    let r = send(&carol, "PROPFIND", "cal", &[("depth", "1")], "")
        .await
        .unwrap();
    assert_eq!(r.status(), 207);
    assert!(r.text().await.unwrap().contains("getetag"));
    // saves from version one: the first wins, the second is told
    let from_one = [("if-match", one.as_str())];
    let r = send(&bob, "PUT", "cal/e.ics", &from_one, "two")
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = send(&bob, "PUT", "cal/e.ics", &from_one, "stale")
        .await
        .unwrap();
    assert_eq!(r.status(), 412);
    let r = send(&carol, "GET", "cal/e.ics", &[], "").await.unwrap();
    assert_eq!(r.text().await.unwrap(), "two");
    // into the trash, yes; out of it for good, only the owner
    let r = send(&bob, "DELETE", "cal/e.ics", &[], "").await.unwrap();
    assert_eq!(r.status(), 204);
    let r = send(&bob, "DELETE", "trash", &[], "").await.unwrap();
    assert_eq!(r.status(), 403);
    let r = send(&carol, "DELETE", "cal/x.ics", &[], "").await.unwrap();
    assert_eq!(r.status(), 403);
}

/// `dd library share`: the owner seals a library to another member's
/// devices, out of their entry as the release holds it; with --write they
/// save into it, without it they only read it, and unshared they do neither.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_shares_a_library_and_takes_it_back() {
    let forge = Forge::new();
    let a = Box_::start_with_libraries(&forge, fake_bucket()).await;
    let d = dirs([&a]);
    let (alice, bob, carol) = (Device::new(), Device::new(), Device::new());
    for (dev, name) in [(&alice, "alice"), (&bob, "bob"), (&carol, "carol")] {
        dev.dd_ok(&args(
            &[
                "identity",
                "new",
                "--born",
                "1990-01",
                "--agree-terms",
                "--name",
                name,
            ],
            &d,
        ));
    }
    forge.list(
        &[
            (member_id_of(&alice, &d), "alice"),
            (member_id_of(&bob, &d), "bob"),
            (member_id_of(&carol, &d), "carol"),
        ],
        &[],
    );
    let out = alice.dd_ok(&args(&["library", "new"], &d));
    let id = out
        .lines()
        .find_map(|l| l.strip_prefix("library "))
        .and_then(|l| l.split(':').next())
        .expect("the new library's id")
        .to_string();
    alice.dd_ok(&args(&["library", "share", &id, "bob", "--write"], &d));
    alice.dd_ok(&args(&["library", "share", &id, "carol"], &d));
    alice.dd_fails(
        &args(&["library", "share", &id, "mallory"], &d),
        "not a member",
    );
    let out = bob.dd_ok(&args(&["library", "list"], &d));
    assert!(out.contains(&format!("{id}  alice's")), "{out}");
    let dir = scratch("share");
    let event = dir.join("e.ics");
    std::fs::write(&event, "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
    let ev = event.to_str().unwrap();
    bob.dd_ok(&args(
        &["library", "put", &id, ev, "--as", "Calendars/f/e.ics"],
        &d,
    ));
    // carol reads what bob wrote, sealed under alice's key, and writes nothing
    let back = dir.join("back.ics");
    carol.dd_ok(&args(
        &[
            "library",
            "get",
            &id,
            "Calendars/f/e.ics",
            back.to_str().unwrap(),
        ],
        &d,
    ));
    assert_eq!(
        std::fs::read(&back).unwrap(),
        std::fs::read(&event).unwrap()
    );
    carol.dd_fails(
        &args(
            &["library", "put", &id, ev, "--as", "Calendars/f/c.ics"],
            &d,
        ),
        "403",
    );
    // taken back from bob: he can no longer write, or find it
    alice.dd_ok(&args(&["library", "unshare", &id, "bob"], &d));
    bob.dd_fails(
        &args(
            &["library", "put", &id, ev, "--as", "Calendars/f/again.ics"],
            &d,
        ),
        "no library",
    );
    let out = bob.dd_ok(&args(&["library", "list"], &d));
    assert!(!out.contains(&id), "{out}");
}

fn run_with_stdin(dev: &Device, a: &[&str], stdin: &str, want_ok: bool) -> String {
    use std::io::Write as _;
    let mut c = Command::new(env!("CARGO_BIN_EXE_dd"))
        .args(a)
        .env("DD_KEYRING_FILE", &dev.keyring)
        .env("DD_RELEASE_PUB", release_pub())
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
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
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
    let a = Box_::start_members(true, Some(vec![])).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
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
    let b = Box_::start_members(true, Some(vec![id.clone()])).await;
    dev.dd_ok(&args(&["identity", "publish"], &dirs([&a, &b])));
    assert_eq!(verify_status(&b, &access).await, 200);
    assert_eq!(verify_status(&a, &access).await, 403);

    // the list is edited by name, stored by id, and never holds the name
    let repo = scratch("repo");
    std::fs::create_dir_all(repo.join("fleet")).unwrap();
    let repo = repo.to_str().unwrap().to_string();
    let file = || std::fs::read_to_string(format!("{repo}/fleet/members.json")).unwrap_or_default();
    // by name, the id is the operator's to confirm: not the directory's
    dev.dd_fails(
        &args(&["member", "add", "tom", "--repo", &repo], &d),
        "--confirm",
    );
    let wrong = identity::member_id("someone else");
    dev.dd_fails(
        &args(
            &["member", "add", "tom", "--repo", &repo, "--confirm", &wrong],
            &d,
        ),
        "is not the member id",
    );
    assert!(!file().contains(&id), "{}", file());
    dev.dd_ok(&args(
        &["member", "add", "tom", "--repo", &repo, "--confirm", &id],
        &d,
    ));
    assert!(file().contains(&id), "{}", file());
    assert!(!file().contains("tom"));
    // under the name confirmed, as a tag the release signs for clients
    assert!(
        file().contains(&identity::name_tag(&id, "tom")),
        "{}",
        file()
    );
    // the names come from a box where tom is a member: the list is members'
    let out = dev.dd_ok(&args(&["member", "list", "--repo", &repo], &dirs([&b])));
    assert!(out.contains(&id) && out.contains("tom"), "{out}");
    dev.dd_ok(&args(
        &[
            "member",
            "suspend",
            "tom",
            "--reason",
            "spam",
            "--repo",
            &repo,
            "--confirm",
            &id,
        ],
        &d,
    ));
    let f: serde_json::Value = serde_json::from_str(&file()).unwrap();
    assert!(f["members"].as_array().unwrap().is_empty(), "{}", file());
    assert_eq!(f["revoked"][0], id, "{}", file());
    assert!(
        !dev.dd(&args(
            &[
                "member",
                "suspend",
                "tom",
                "--reason",
                "spam",
                "--repo",
                &repo,
                "--confirm",
                &id
            ],
            &d
        ))
        .ok
    );
    // by name, a revoked id stays revoked; given as itself, it comes off
    dev.dd_fails(
        &args(
            &["member", "add", "tom", "--repo", &repo, "--confirm", &id],
            &d,
        ),
        "is revoked",
    );
    dev.dd_ok(&args(&["member", "add", &id, "--repo", &repo], &d));
    let f: serde_json::Value = serde_json::from_str(&file()).unwrap();
    assert_eq!(f["members"][0], id, "{}", file());
    assert!(f["revoked"].as_array().unwrap().is_empty(), "{}", file());
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
        .json(&serde_json::json!({ "username": name, "agree": true }))
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
    let a = Box_::start_members(true, Some(vec![])).await;
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
    // and none of a member's own pages open to an account that is not one;
    // Settings does, for its way out
    for page in ["/_dd/files", "/_dd/media", "/_dd/boxes"] {
        let (st, _) = get_with_cookie(&a, page, &cookie).await;
        assert_eq!(st, 303, "{page} opened to a non-member");
    }
    assert_eq!(get_with_cookie(&a, "/_dd/settings", &cookie).await.0, 200);
    assert_eq!(get_with_cookie(&a, "/_dd/fleet.json", &cookie).await.0, 403);

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
    // her passkey holds no key, so a page that would is the app's to serve
    assert_eq!(get_with_cookie(&b, "/_dd/settings", &cookie_b).await.0, 200);
    let (st, body) = get_with_cookie(&b, "/_dd/files", &cookie_b).await;
    assert_eq!(st, 200);
    assert!(body.contains("use-app"), "{body}");

    // and she leaves from that browser, by her passkey: out at once
    let (st, s) = get_with_cookie(&b, "/_dd/leave", &cookie_b).await;
    assert_eq!(st, 200, "{s}");
    let s: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(s["member"], true);
    assert_eq!(s["passkey"], true);
    assert!(s["left"].is_null());
    leave_in_browser(&b, &mut eve, &cookie_b).await;
    assert_ne!(get_with_cookie(&b, "/verify", &cookie_b).await.0, 200);
    // signed in again, she sees where she stands: left, everything due to
    // go at once, and no coming back
    let (st, cookie_b) = login_in_browser(&b, &mut eve, "eve").await;
    assert_eq!(st, 200);
    assert_ne!(get_with_cookie(&b, "/verify", &cookie_b).await.0, 200);
    assert_eq!(get_with_cookie(&b, "/_dd/settings", &cookie_b).await.0, 200);
    let (_, s) = get_with_cookie(&b, "/_dd/leave", &cookie_b).await;
    let s: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(s["left"].is_u64(), "{s}");
    assert_eq!(s["deleted"], true, "{s}");
    assert_eq!(s["until"], s["left"], "nothing is kept: {s}");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    come_back_refused(&b, &cookie_b).await;
    assert_ne!(get_with_cookie(&b, "/verify", &cookie_b).await.0, 200);

    // dd member add, by name, finds the same id
    let repo = scratch("repo");
    std::fs::create_dir_all(repo.join("fleet")).unwrap();
    let repo = repo.to_str().unwrap().to_string();
    let dev = Device::new();
    dev.dd_ok(&args(
        &["member", "add", "eve", "--repo", &repo, "--confirm", &id],
        &dirs([&a]),
    ));
    let file = std::fs::read_to_string(format!("{repo}/fleet/members.json")).unwrap();
    assert!(file.contains(&id), "{file}");
}

/// Every sign-up asks age and agreement first: `dd` from its flags (or at a
/// terminal), the browser's join from the page. Under 13 is refused and
/// stays refused on that device; 13 to 17 needs a parent's or guardian's
/// agreement; the entry records which terms were agreed to, and a box
/// refuses a new one that says none.
#[tokio::test(flavor = "multi_thread")]
async fn a_sign_up_asks_age_and_agreement_and_records_the_terms() {
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    // a year that is surely this one's, give or take the new year
    let year = 1970 + identity::now() / 31_556_952;
    let child = format!("{}-06", year - 5);
    let teen = format!("{}-06", year - 15);

    let kid = Device::new();
    kid.dd_fails(&args(&["identity", "new", "--name", "kid"], &d), "--born");
    kid.dd_fails(
        &args(
            &[
                "identity",
                "new",
                "--born",
                &child,
                "--agree-terms",
                "--name",
                "kid",
            ],
            &d,
        ),
        "13 or over",
    );
    // and going back to answer again changes nothing on this device
    kid.dd_fails(
        &args(
            &[
                "identity",
                "new",
                "--born",
                "1990-01",
                "--agree-terms",
                "--name",
                "kid",
            ],
            &d,
        ),
        "13 or over",
    );
    assert!(entry(&a, "kid").await.is_none());

    let ted = Device::new();
    ted.dd_fails(
        &args(
            &[
                "identity",
                "new",
                "--born",
                &teen,
                "--agree-terms",
                "--name",
                "ted",
            ],
            &d,
        ),
        "parent or guardian",
    );
    ted.dd_fails(
        &args(
            &[
                "identity",
                "new",
                "--born",
                &teen,
                "--guardian-agrees",
                "--name",
                "ted",
            ],
            &d,
        ),
        "--agree-terms",
    );
    ted.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            &teen,
            "--guardian-agrees",
            "--agree-terms",
            "--name",
            "ted",
        ],
        &d,
    ));
    let e = entry(&a, "ted").await.unwrap();
    assert_eq!(e["entry"]["terms"], identity::TERMS);
    let raw = serde_json::to_string(&e).unwrap();
    assert!(!raw.contains(&teen), "the birth date went to the box");

    // a new entry that agreed to nothing is refused, made by hand
    let k = identity::generate();
    let p = identity::encode_public(&identity::generate().verifying_key());
    let bare = identity::Entry {
        name: "bare".into(),
        root: identity::encode_public(&k.verifying_key()),
        recovery: identity::encode_public(&identity::generate().verifying_key()),
        recovered_from: vec![],
        devices: vec![identity::Device {
            fingerprint: identity::fingerprint(&p),
            public_key: p,
            added: identity::now(),
        }],
        passkeys: vec![],
        grant: None,
        libraries: vec![],
        terms: None,
        version: 1,
        updated: identity::now(),
    };
    let signed = identity::sign(bare, &k).unwrap();
    let (st, body) = put_raw(&a, "bare", &serde_json::to_value(&signed).unwrap()).await;
    assert_eq!(st, 403, "{body}");
    assert!(body.contains("Terms of use"), "{body}");

    // the browser's join: not without the box ticked, and its entry says
    // what was agreed, under the passkey's own signature
    let r = reqwest::Client::new()
        .post(a.url("/_dd/join/start"))
        .json(&serde_json::json!({ "username": "eve" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let mut eve = SoftPasskey::new(true);
    assert_eq!(join_in_browser(&a, &mut eve, "eve").await.0, 200);
    let held = a.dir.parent().unwrap().join("held").join("eve.json");
    let e: identity::SignedEntry = serde_json::from_slice(&std::fs::read(&held).unwrap()).unwrap();
    assert_eq!(e.entry.terms.as_deref(), Some(identity::TERMS));
    identity::accept(None, &e).unwrap();
}

/// An account whose root is on a device leaves by that root: what the
/// app's Settings asks of it (`account::leave`, behind `/_dd/app/leaving`)
/// is what `dd leave` does. The tab reads where it stands with the device's
/// token. Leaving deletes at once, so `dd` asks for --yes first.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_with_its_root_on_a_device_leaves_by_it_for_good() {
    let a = Box_::start_members(true, Some(vec![])).await;
    let sam = Device::new();
    sam.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "sam",
        ],
        &dirs([&a]),
    ));
    let e = entry(&a, "sam").await.unwrap();
    let id = identity::member_id(e["entry"]["root"].as_str().unwrap());
    let b = Box_::start_on(
        a.dir.clone(),
        true,
        Some(verify::Members::list(vec![id])),
        None,
        vec![],
    )
    .await;
    let d = dirs([&b]);
    assert_eq!(verify_status(&b, &sam.token()).await, 200);
    let mine = |token: String| {
        let r = reqwest::Client::new()
            .get(b.url("/_dd/leave"))
            .bearer_auth(token)
            .send();
        async move {
            let r = r.await.unwrap();
            assert_eq!(r.status(), 200);
            r.json::<serde_json::Value>().await.unwrap()
        }
    };
    let s = mine(sam.token()).await;
    assert_eq!(s["member"], true);
    assert_eq!(s["passkey"], false);
    assert!(s["left"].is_null());

    sam.dd_fails(&args(&["leave"], &d), "cannot be undone");
    assert_eq!(
        verify_status(&b, &sam.token()).await,
        200,
        "asked, not left"
    );
    sam.dd_ok(&args(&["leave", "--yes"], &d));
    assert_ne!(verify_status(&b, &sam.token()).await, 200);
    let s = mine(sam.token()).await;
    assert!(s["left"].is_u64());
    assert_eq!(s["deleted"], true);
    sam.dd_fails(&args(&["leave", "--yes"], &d), "left already");
}

/// Coming back is gone: leaving deleted everything at once
async fn come_back_refused(b: &Box_, cookie: &str) {
    let r = reqwest::Client::new()
        .post(b.url("/_dd/leave/start"))
        .header("cookie", cookie)
        .json(&serde_json::json!({ "back": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "{}", r.text().await.unwrap_or_default());
}

/// Settings' Leave, as its page does it: the box hands the statement over,
/// the root passkey signs it
async fn leave_in_browser(b: &Box_, authenticator: &mut SoftPasskey, cookie: &str) {
    let http = reqwest::Client::new();
    let r = http
        .post(b.url("/_dd/leave/start"))
        .header("cookie", cookie)
        .json(&serde_json::json!({ "back": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let rcr: RequestChallengeResponse = serde_json::from_value(j).unwrap();
    let cred = authenticator
        .do_authentication(Url::parse("https://localhost").unwrap(), rcr)
        .unwrap();
    let r = http
        .post(b.url("/_dd/leave/sign"))
        .header("cookie", cookie)
        .header("x-dd-ceremony", &ceremony)
        .json(&cred)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
}

/// Sign in on `b` with a passkey already in the entry; the session cookie.
async fn login_in_browser(b: &Box_, authenticator: &mut SoftPasskey, name: &str) -> (u16, String) {
    login_in_browser_at(b, authenticator, name, "https://localhost").await
}

/// The same, on a page at `origin`: what a box serving some other host of
/// the domain would get if it handed this box's challenge to the person.
async fn login_in_browser_at(
    b: &Box_,
    authenticator: &mut SoftPasskey,
    name: &str,
    origin: &str,
) -> (u16, String) {
    let origin = Url::parse(origin).unwrap();
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

    // eve makes her account with the code, in dd (the app does the same):
    // the code is typed on her own device and never into a box's page
    let eve = Device::new();
    eve.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "eve",
            "--code",
            &code,
        ],
        &d,
    ));
    assert_eq!(verify_status(&a, &eve.token()).await, 200);
    let e = entry(&a, "eve").await.unwrap();
    assert!(e["entry"]["grant"]["proof"].is_string());

    // the same code, another person: refused, the code is used up
    let mallory = Device::new();
    mallory.dd_fails(
        &args(
            &[
                "identity",
                "new",
                "--born",
                "1990-01",
                "--agree-terms",
                "--name",
                "mallory",
                "--code",
                &code,
            ],
            &d,
        ),
        "already been used",
    );
    // and a code nobody issued
    mallory.dd_fails(
        &args(
            &[
                "identity",
                "new",
                "--born",
                "1990-01",
                "--agree-terms",
                "--name",
                "mallory",
                "--code",
                "zzzzz-zzzzz",
            ],
            &d,
        ),
        "not valid",
    );

    // a box's page takes no code: a browser account comes with none, and
    // one typed there is refused, as is the old way to add one later
    let out = owner.dd_ok(&args(&["invite"], &d));
    let code2 = out
        .lines()
        .find_map(|l| l.strip_prefix("code: "))
        .unwrap()
        .trim()
        .to_string();
    let mut fay = SoftPasskey::new(true);
    let (st, why) = join_with_code(&a, &mut fay, "fay", Some(&code2)).await;
    assert_eq!(st, 400, "{why}");
    assert!(why.contains("app"), "{why}");
    let (st, fay_cookie) = join_in_browser(&a, &mut fay, "fay").await;
    assert_eq!(st, 200);
    assert_eq!(get_with_cookie(&a, "/verify", &fay_cookie).await.0, 403);
    let r = http
        .post(a.url("/_dd/redeem/start"))
        .header("cookie", &fay_cookie)
        .header("x-dd-grant", grant_header(&code2, "webauthn:fay"))
        .send()
        .await
        .unwrap();
    assert!(matches!(r.status().as_u16(), 404 | 405), "{}", r.status());
    // the code is still good, on a device
    let fay2 = Device::new();
    fay2.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "fay2",
            "--code",
            &code2,
        ],
        &d,
    ));

    // the owner sees them, as a member: the directory's names are members'.
    // They let themselves in with a code of their own
    let out = owner.dd_ok(&args(&["invite", "--ttl", "10m"], &d));
    let own = out
        .lines()
        .find_map(|l| l.strip_prefix("code: "))
        .unwrap()
        .trim()
        .to_string();
    owner.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "owner",
            "--code",
            &own,
        ],
        &d,
    ));
    // and can shut one out again: revoked beats a grant
    let out = owner.dd_ok(&args(&["member", "list", "--repo", &repo], &d));
    assert!(
        out.contains("eve  invited") && out.contains("fay2  invited"),
        "{out}"
    );
    let eve_id = identity::member_id(e["entry"]["root"].as_str().unwrap());
    owner.dd_ok(&args(
        &[
            "member",
            "suspend",
            "eve",
            "--reason",
            "spam",
            "--repo",
            &repo,
            "--confirm",
            &eve_id,
        ],
        &d,
    ));
    let b_dir = scratch("b").join("keys");
    copy_entries(&a, &b_dir);
    let b = Box_::start_on(
        b_dir,
        true,
        Some(verify::Members {
            revoked: vec![eve_id],
            ..Default::default()
        }),
        Some(release_pub.clone()),
        vec![],
    )
    .await;
    assert_eq!(
        verify_status(&b, &fay2.token()).await,
        200,
        "fay2's grant carries to b"
    );
    assert_eq!(verify_status(&b, &eve.token()).await, 403, "eve is revoked");
    // a page of another host of the domain is not this box's to ask from
    let (st, _) = login_in_browser_at(&a, &mut fay, "fay", "https://other.localhost").await;
    assert_ne!(st, 200, "a sign-in made on another host was taken");
    // her own page's is, and she waits as before
    let (st, c) = login_in_browser(&a, &mut fay, "fay").await;
    assert_eq!(st, 200);
    assert_eq!(get_with_cookie(&a, "/verify", &c).await.0, 403);

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
    mallory.dd_fails(
        &args(
            &[
                "identity",
                "new",
                "--born",
                "1990-01",
                "--agree-terms",
                "--name",
                "mallory",
                "--code",
                &code3,
            ],
            &d,
        ),
        "expired",
    );
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
        Some(verify::Members::list(vec![])),
        None,
        vec![tile("files", None)],
    )
    .await;
    assert_eq!(get_with_cookie(&plain, "/_dd/demo", "").await.0, 404);

    let a = Box_::start_home(
        true,
        Some(verify::Members::list(vec![])),
        None,
        vec![
            tile("files", Some("read")),
            tile("llm", Some("rate:2/3")),
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
    // another click on the demo is another demo, with two of its own; the
    // three between them all is the ceiling
    let other = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(a.url("/_dd/demo"))
        .send()
        .await
        .unwrap()
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_ne!(other, cookie, "each click is a demo of its own");
    let other_prompt = || {
        let u = a.url("/verify");
        let c = other.clone();
        async move {
            reqwest::Client::new()
                .get(u)
                .header("cookie", c)
                .header("x-original-method", "POST")
                .header("x-original-host", "llm.x")
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(other_prompt().await, 200);
    assert_eq!(other_prompt().await, 403, "three between them all");
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
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "amy",
        ],
        &d,
    ));

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
    // a passkey holds no keys: one sent with a key a page made from its PRF
    // is refused, and the ceremony still stands for the plain one
    let mut keyed = serde_json::to_value(&reg).unwrap();
    keyed["library_key"] = serde_json::Value::String(identity::encode_public(
        &identity::generate().verifying_key(),
    ));
    let r = http
        .post(a.url("/_dd/enrol/finish"))
        .bearer_auth(&tok)
        .header("x-dd-ceremony", &ceremony)
        .json(&keyed)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
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
    assert!(e["entry"]["passkeys"][0].get("library_key").is_none());

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

    // One sign-in, and that is all: what the app serves (Settings' Devices,
    // its library, its Photos) asks the box with the device's token, which
    // needs no passkey shown again, and the browser's one session opens the
    // box's own pages
    let token = dev.token();
    let r = http
        .get(a.url("/_dd/signins"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "the app's Devices tab, no cookie");
    let r = http
        .post(a.url("/_dd/signins/end"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "others": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "a device's token ends sessions without a passkey prompt: {}",
        r.text().await.unwrap_or_default()
    );
    assert_eq!(
        verify_status(&a, &token).await,
        200,
        "the app's token is its own"
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

/// another box's entries, as that box holds them: what the agreed log
/// gives every box (the directory no longer pulls one box from another)
fn copy_entries(from: &Box_, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in std::fs::read_dir(&from.dir).unwrap() {
        let f = f.unwrap().path();
        if f.extension().is_some_and(|e| e == "json") {
            std::fs::copy(&f, to.join(f.file_name().unwrap())).unwrap();
        }
    }
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
    let a = Box_::start(false).await;
    let b = Box_::start(false).await;
    let dev = Device::new();
    dev.dd_ok(&[
        "identity",
        "new",
        "--born",
        "1990-01",
        "--agree-terms",
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
    serve(std::sync::Arc::new(std::sync::Mutex::new(body)))
}

/// the same, with whatever `body` holds at the time
fn serve(body: std::sync::Arc<std::sync::Mutex<String>>) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let body = body.lock().unwrap().clone();
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
        mail: None,
        home: vec![],
        members: Some(verify::Members::list(vec![])),
        release_pub: Some(release_pub),
        web_dir: None,
        photos: None,
        library: None,
        fleet: Default::default(),
        thanos: None,
        adblock: None,
        forge_events: None,
        storage_ledger: None,
        demo_library: None,
        tmdb: None,
        search: None,
        id_origin: None,
        box_only: false,
        owner: None,
        admin: None,
        boxes: Default::default(),
        oidc: None,
        app_manifest: Some(manifest_url),
        member_list: None,
        network: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        dir: scratch("box").join("keys"),
        directory_readers: vec![],
        names: vec!["127.0.0.1".into()],
        control: None,
        identity_from: Default::default(),
        house_peers: vec![],
        domain: Some("localhost".to_string()),
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

#[tokio::test(flavor = "multi_thread")]
async fn a_friend_link_makes_a_guest_who_reaches_games_and_nothing_else() {
    let tile = |name: &str| verify::pages::Service {
        name: name.into(),
        url: format!("https://{name}.localhost/"),
        icon: "".into(),
        color: "#000".into(),
        blurb: String::new(),
        demo: None,
        demo_url: None,
        menu_only: false,
    };
    let home = vec![tile("files"), tile("photos"), tile("games")];
    // david signs up, and the box is released with him on the member list
    let a = Box_::start_members(true, Some(vec![])).await;
    let mut david = SoftPasskey::new(true);
    assert_eq!(join_in_browser(&a, &mut david, "david").await.0, 200);
    let held = a.dir.parent().unwrap().join("held").join("david.json");
    let e: serde_json::Value = serde_json::from_slice(&std::fs::read(&held).unwrap()).unwrap();
    let id = identity::member_id(e["entry"]["root"].as_str().unwrap());
    let b = Box_::start_on(
        a.dir.clone(),
        true,
        Some(verify::Members::list(vec![id])),
        None,
        home,
    )
    .await;
    wait_for(|| async { entry(&b, "david").await.is_some() }).await;
    let (st, david_c) = login_in_browser(&b, &mut david, "david").await;
    assert_eq!(st, 200);

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let post = |path: &str, cookie: &str, json: bool| {
        let r = http.post(b.url(path)).header("cookie", cookie.to_string());
        let r = if json {
            r.header("content-type", "application/json").body("{}")
        } else {
            r.header("content-type", "application/x-www-form-urlencoded")
                .body("")
        };
        async move {
            let r = r.send().await.unwrap();
            (r.status().as_u16(), r.text().await.unwrap_or_default())
        }
    };
    let gate = |cookie: &str, host: &str| {
        let r = http
            .get(b.url("/verify"))
            .header("cookie", cookie.to_string())
            .header("x-original-host", host.to_string());
        async move {
            let r = r.send().await.unwrap();
            (
                r.status().as_u16(),
                r.headers()
                    .get("x-dd-role")
                    .map(|v| v.to_str().unwrap().to_string()),
            )
        }
    };
    assert_eq!(
        gate(&david_c, "files.localhost").await,
        (200, Some("member".into()))
    );

    // a member makes a link
    let (st, made) = post("/_dd/friends/link", &david_c, true).await;
    assert_eq!(st, 200, "{made}");
    let made: serde_json::Value = serde_json::from_str(&made).unwrap();
    let secret = made["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let accept = format!("/_dd/friend/{secret}");

    // tom signs up to open it: held, and let in nowhere, until he accepts
    let mut tom = SoftPasskey::new(true);
    let (st, tom_c) = join_in_browser(&b, &mut tom, "tom").await;
    assert_eq!(st, 200);
    assert_eq!(gate(&tom_c, "games.localhost").await.0, 403);
    // a form from another site cannot accept it for him
    assert_eq!(post(&accept, &tom_c, false).await.0, 415);
    let (st, body) = post(&accept, &tom_c, true).await;
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"guest\""), "{body}");
    // his account is published now, not left to lapse with the held ones
    assert!(entry(&b, "tom").await.is_some());

    // the games page, and not one other door
    assert_eq!(
        gate(&tom_c, "games.localhost").await,
        (200, Some("guest".into()))
    );
    for host in [
        "files.localhost",
        "photos.localhost",
        "home.localhost",
        "grafana.localhost",
        "elsewhere.x",
    ] {
        assert_eq!(gate(&tom_c, host).await.0, 403, "{host} let a guest in");
    }
    let (st, me) = get_with_cookie(&b, "/_dd/me", &tom_c).await;
    assert_eq!(st, 200, "{me}");
    let me: serde_json::Value = serde_json::from_str(&me).unwrap();
    assert_eq!(me["role"], "guest");
    assert_eq!(me["guestOf"], "david");
    let names: Vec<&str> = me["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["games"]);
    assert!(me.get("tmdb").is_none());
    for (page, want) in [
        ("/_dd/files", 303),
        ("/_dd/media", 303),
        ("/_dd/boxes", 303),
        ("/_dd/settings", 200),
        ("/_dd/devices", 303),
        ("/_dd/network", 303),
        ("/_dd/friends", 200),
    ] {
        assert_eq!(get_with_cookie(&b, page, &tom_c).await.0, want, "{page}");
    }
    assert_eq!(get_with_cookie(&b, "/_dd/fleet.json", &tom_c).await.0, 403);
    // a guest brings nobody in
    assert_eq!(post("/_dd/friends/link", &tom_c, true).await.0, 403);
    // and the link is spent
    let mut sarah = SoftPasskey::new(true);
    let (_, sarah_c) = join_in_browser(&b, &mut sarah, "sarah").await;
    assert_eq!(post(&accept, &sarah_c, true).await.0, 404);
    assert_eq!(gate(&sarah_c, "games.localhost").await.0, 403);

    // friends, both ways
    let (_, list) = get_with_cookie(&b, "/_dd/friends/list", &david_c).await;
    assert!(
        list.contains("\"tom\"") && list.contains("\"guest\""),
        "{list}"
    );

    // the games box asks who may play: the owner and his friends, no one else
    let r = http
        .post(b.url("/internal/games/access"))
        .json(&serde_json::json!([{ "owner": "david", "players": ["tom", "sarah", "david"] }]))
        .send()
        .await
        .unwrap();
    let who: serde_json::Value = r.json().await.unwrap();
    assert_eq!(who[0]["players"], serde_json::json!(["david", "tom"]));
    // a guest owns no server
    let r = http
        .post(b.url("/internal/games/access"))
        .json(&serde_json::json!([{ "owner": "tom", "players": ["david"] }]))
        .send()
        .await
        .unwrap();
    let who: serde_json::Value = r.json().await.unwrap();
    assert_eq!(who[0]["players"], serde_json::json!([]));

    // a passkey is all a browser account needs: signed in again, tom is at
    // his games, and sarah, still nobody's guest, waits
    let (st, tom_c) = login_in_browser(&b, &mut tom, "tom").await;
    assert_eq!(st, 200);
    assert_eq!(
        gate(&tom_c, "games.localhost").await,
        (200, Some("guest".into()))
    );
    let (st, sarah_c) = login_in_browser(&b, &mut sarah, "sarah").await;
    assert_eq!(st, 200);
    let (st, body) = get_with_cookie(&b, "/_dd/home", &sarah_c).await;
    assert_eq!(st, 200);
    assert!(body.contains("Your account is made"), "{body}");

    // david ends it: tom is a guest of no one, and let in nowhere
    let r = http
        .delete(b.url("/_dd/friends/remove/tom"))
        .header("cookie", david_c.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 204);
    assert_eq!(gate(&tom_c, "games.localhost").await.0, 403);
    assert_eq!(get_with_cookie(&b, "/_dd/me", &tom_c).await.0, 403);
}

/// Anyone may leave, not only members: a friend's guest and an account
/// still waiting leave with their passkey, every way in ends at once, and
/// there is no coming back. Nothing of anyone else's moves.
#[tokio::test(flavor = "multi_thread")]
async fn a_guest_and_a_waiting_account_leave_for_good() {
    let tile = |name: &str| verify::pages::Service {
        name: name.into(),
        url: format!("https://{name}.localhost/"),
        icon: "".into(),
        color: "#000".into(),
        blurb: String::new(),
        demo: None,
        demo_url: None,
        menu_only: false,
    };
    let a = Box_::start_members(true, Some(vec![])).await;
    let mut david = SoftPasskey::new(true);
    assert_eq!(join_in_browser(&a, &mut david, "david").await.0, 200);
    let held = a.dir.parent().unwrap().join("held").join("david.json");
    let e: serde_json::Value = serde_json::from_slice(&std::fs::read(&held).unwrap()).unwrap();
    let id = identity::member_id(e["entry"]["root"].as_str().unwrap());
    let b = Box_::start_on(
        a.dir.clone(),
        true,
        Some(verify::Members::list(vec![id])),
        None,
        vec![tile("games")],
    )
    .await;
    wait_for(|| async { entry(&b, "david").await.is_some() }).await;
    let (_, david_c) = login_in_browser(&b, &mut david, "david").await;

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let post_json = |path: &str, cookie: &str| {
        let r = http
            .post(b.url(path))
            .header("cookie", cookie.to_string())
            .header("content-type", "application/json")
            .body("{}");
        async move {
            let r = r.send().await.unwrap();
            (r.status().as_u16(), r.text().await.unwrap_or_default())
        }
    };
    let games = |cookie: &str| {
        let r = http
            .get(b.url("/verify"))
            .header("cookie", cookie.to_string())
            .header("x-original-host", "games.localhost");
        async move { r.send().await.unwrap().status().as_u16() }
    };
    let stands = |cookie: &str| {
        let r = http
            .get(b.url("/_dd/leave"))
            .header("cookie", cookie.to_string());
        async move {
            let r = r.send().await.unwrap();
            assert_eq!(r.status(), 200);
            r.json::<serde_json::Value>().await.unwrap()
        }
    };

    // tom comes in as david's guest
    let (_, made) = post_json("/_dd/friends/link", &david_c).await;
    let made: serde_json::Value = serde_json::from_str(&made).unwrap();
    let secret = made["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let mut tom = SoftPasskey::new(true);
    let (_, tom_c) = join_in_browser(&b, &mut tom, "tom").await;
    assert_eq!(
        post_json(&format!("/_dd/friend/{secret}"), &tom_c).await.0,
        200
    );
    assert_eq!(games(&tom_c).await, 200);
    let (_, still) = post_json("/_dd/friends/link", &david_c).await;
    let still: serde_json::Value = serde_json::from_str(&still).unwrap();

    // a guest's Settings offers the way out, and the way out works
    assert_eq!(get_with_cookie(&b, "/_dd/settings", &tom_c).await.0, 200);
    let s = stands(&tom_c).await;
    assert_eq!(s["role"], "guest");
    assert!(s["left"].is_null());
    leave_in_browser(&b, &mut tom, &tom_c).await;
    assert_ne!(games(&tom_c).await, 200, "the session they left in ended");
    let (st, tom_c) = login_in_browser(&b, &mut tom, "tom").await;
    assert_eq!(st, 200);
    assert_eq!(
        games(&tom_c).await,
        403,
        "a guest who left is let in nowhere"
    );
    assert_eq!(get_with_cookie(&b, "/_dd/me", &tom_c).await.0, 403);
    assert!(stands(&tom_c).await["left"].is_u64());
    assert_eq!(get_with_cookie(&b, "/_dd/settings", &tom_c).await.0, 200);

    // david's things are his: his access, his other link, his friend list
    assert_eq!(verify_status_cookie(&b, &david_c).await, 200);
    let link = still["url"].as_str().unwrap().rsplit('/').next().unwrap();
    assert_eq!(
        get_with_cookie(&b, &format!("/_dd/friend/{link}/about"), &tom_c)
            .await
            .0,
        200
    );

    // and tom cannot come back
    tokio::time::sleep(Duration::from_millis(1100)).await;
    come_back_refused(&b, &tom_c).await;
    assert_eq!(games(&tom_c).await, 403);

    // sarah signed up and is still waiting: no member list names her, no
    // friend link brought her in. She leaves too.
    let mut sarah = SoftPasskey::new(true);
    let (_, sarah_c) = join_in_browser(&b, &mut sarah, "sarah").await;
    assert_eq!(get_with_cookie(&b, "/_dd/settings", &sarah_c).await.0, 200);
    let (_, home) = get_with_cookie(&b, "/_dd/home", &sarah_c).await;
    assert!(home.contains("/_dd/settings#leave"), "{home}");
    let s = stands(&sarah_c).await;
    assert_eq!(s["role"], "waiting");
    leave_in_browser(&b, &mut sarah, &sarah_c).await;
    assert!(
        entry(&b, "sarah").await.is_none(),
        "her word stays on this box"
    );
    let (st, sarah_c) = login_in_browser(&b, &mut sarah, "sarah").await;
    assert_eq!(st, 200);
    assert!(stands(&sarah_c).await["left"].is_u64());
    // a friend link cannot make a guest of someone who left
    let (_, l2) = post_json("/_dd/friends/link", &david_c).await;
    let l2: serde_json::Value = serde_json::from_str(&l2).unwrap();
    let s2 = l2["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        post_json(&format!("/_dd/friend/{s2}"), &sarah_c).await.0,
        403
    );
    assert_eq!(games(&sarah_c).await, 403);
    assert!(entry(&b, "sarah").await.is_none());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    come_back_refused(&b, &sarah_c).await;

    // and david was never touched
    assert_eq!(verify_status_cookie(&b, &david_c).await, 200);
}

/// /verify for a browser's session
async fn verify_status_cookie(b: &Box_, cookie: &str) -> u16 {
    reqwest::Client::new()
        .get(b.url("/verify"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// A stand-in museum: answers every request 200 with `{}` and keeps the
/// request line and body of each, in order.
async fn fake_museum() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let keep = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let keep = keep.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // headers, then as much body as content-length says
                loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            let line = text.lines().next().unwrap_or_default().to_string();
                            keep.lock()
                                .unwrap()
                                .push(format!("{line}\n{}", &text[end + 4..]));
                            break;
                        }
                    }
                }
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
                    .await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

/// Photos' sign-up code is one value for the whole fleet: whoever holds it
/// can claim any address at users.<domain>. So the page never gets it; the
/// box puts it into the two museum calls that need it, for a member and
/// only for that member's own address.
#[tokio::test(flavor = "multi_thread")]
async fn the_photos_code_is_filled_in_here_and_never_handed_out() {
    let (museum, seen) = fake_museum().await;
    // tom's account first, on a box that knows nobody, for his member id
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
    let show = dev.dd_ok(&args(&["identity", "show"], &d));
    let id = show
        .lines()
        .find_map(|l| l.strip_prefix("member id:"))
        .unwrap()
        .trim()
        .to_string();
    let dir = scratch("photos").join("keys");
    let (addr, _task) = verify::start(verify::Config {
        mail: None,
        home: vec![],
        members: Some(verify::Members::list(vec![id])),
        release_pub: None,
        web_dir: None,
        photos: Some(verify::Photos {
            api: museum.clone(),
            email_suffix: "@users.localhost".into(),
            code: "424242".into(),
            demo_password: None,
        }),
        library: None,
        fleet: Default::default(),
        thanos: None,
        adblock: None,
        forge_events: None,
        storage_ledger: None,
        demo_library: None,
        tmdb: None,
        search: None,
        id_origin: None,
        box_only: false,
        owner: None,
        admin: None,
        boxes: Default::default(),
        oidc: None,
        app_manifest: None,
        member_list: None,
        network: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        dir: {
            copy_entries(&a, &dir);
            dir
        },
        directory_readers: vec![],
        names: vec!["127.0.0.1".into()],
        control: None,
        identity_from: Default::default(),
        house_peers: vec![],
        domain: Some("localhost".to_string()),
    })
    .await
    .unwrap();
    let b = format!("http://{addr}");
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    // tom's entry reaches the new box, then his app token is a session
    let token = dev.token();
    let mut cookie = String::new();
    for _ in 0..50 {
        let r = http
            .post(format!("{b}/_dd/app/signin"))
            .form(&[("token", token.as_str())])
            .send()
            .await
            .unwrap();
        if let Some(c) = r.headers().get("set-cookie") {
            cookie = c.to_str().unwrap().split(';').next().unwrap().to_string();
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(!cookie.is_empty(), "tom signs in");

    // the page's config has no code in it
    let cfg: serde_json::Value = http
        .post(format!("{b}/_dd/photos/config"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(cfg.get("code").is_none(), "{cfg}");
    assert_eq!(cfg["accountApi"], "/_dd/museum");

    let call = |op: &'static str, cookie: Option<String>, email: &'static str| {
        let http = http.clone();
        let b = b.clone();
        async move {
            let mut r = http
                .post(format!("{b}/_dd/photos/museum/{op}"))
                .header("x-auth-token", "his-museum-session")
                .json(&serde_json::json!({ "email": email, "ott": "" }));
            if let Some(c) = cookie {
                r = r.header("cookie", c);
            }
            r.send().await.unwrap().status().as_u16()
        }
    };
    // nobody signed in, someone else's address, or any other museum call: no
    assert_eq!(call("verify-email", None, "tom@users.localhost").await, 401);
    assert_eq!(
        call(
            "verify-email",
            Some(cookie.clone()),
            "sarah@users.localhost"
        )
        .await,
        403
    );
    assert_eq!(
        call("send-otp", Some(cookie.clone()), "tom@users.localhost").await,
        404
    );
    assert!(seen.lock().unwrap().is_empty(), "museum heard nothing yet");

    // his own address: museum gets the call with the code in it, and his
    // museum session for the one that acts on his account
    assert_eq!(
        call("verify-email", Some(cookie.clone()), "tom@users.localhost").await,
        200
    );
    assert_eq!(
        call("change-email", Some(cookie.clone()), "tom@users.localhost").await,
        200
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(
        seen[0].starts_with("POST /users/verify-email "),
        "{}",
        seen[0]
    );
    assert!(
        seen[1].starts_with("POST /users/change-email "),
        "{}",
        seen[1]
    );
    for s in &seen {
        assert!(s.contains("\"ott\":\"424242\""), "{s}");
        assert!(s.contains("\"email\":\"tom@users.localhost\""), "{s}");
    }

    // the app's own Photos page makes the account through the app, which
    // brings the device's token and no session: the same answer, the same
    // one address
    let by_app = |email: &'static str| {
        let http = http.clone();
        let b = b.clone();
        let token = token.clone();
        async move {
            http.post(format!("{b}/_dd/photos/museum/verify-email"))
                .bearer_auth(token)
                .json(&serde_json::json!({ "email": email }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(by_app("sarah@users.localhost").await, 403);
    assert_eq!(by_app("tom@users.localhost").await, 200);
}

/// A demo door - demo-<service>.<domain> - is on the open internet so anyone
/// can look around, and lets the demo in and nobody else: a member's session
/// is refused there (sent back to the service's own name for a page), and a
/// request with no session is forbidden, never let through as anonymous.
#[tokio::test(flavor = "multi_thread")]
async fn a_demo_door_lets_in_the_demo_alone() {
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
    let show = dev.dd_ok(&args(&["identity", "show"], &d));
    let id = show
        .lines()
        .find_map(|l| l.strip_prefix("member id:"))
        .unwrap()
        .trim()
        .to_string();
    let tile = verify::pages::Service {
        name: "git".into(),
        url: "https://git.x/".into(),
        icon: String::new(),
        color: "#000".into(),
        blurb: String::new(),
        demo: Some("full".into()),
        demo_url: Some("https://demo-git.x/".into()),
        menu_only: false,
    };
    let b_dir = scratch("demodoor").join("keys");
    copy_entries(&a, &b_dir);
    let b = Box_::start_on(
        b_dir,
        true,
        Some(verify::Members::list(vec![id])),
        None,
        vec![tile],
    )
    .await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let cookie_of = |r: &reqwest::Response| {
        r.headers()
            .get("set-cookie")
            .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_string())
    };
    let demo = cookie_of(&http.get(b.url("/_dd/demo")).send().await.unwrap()).unwrap();
    let token = dev.token();
    let mut tom = None;
    for _ in 0..50 {
        let r = http
            .post(b.url("/_dd/app/signin"))
            .form(&[("token", token.as_str())])
            .send()
            .await
            .unwrap();
        tom = cookie_of(&r);
        if tom.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let tom = tom.expect("tom signs in");
    let gate = |cookie: Option<&str>, host: &str| {
        let mut r = http
            .get(b.url("/verify"))
            .header("x-original-method", "GET")
            .header("x-original-host", host);
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        async move { r.send().await.unwrap().status().as_u16() }
    };
    // behind nginx: the demo in, tom and nobody out
    assert_eq!(gate(Some(&demo), "demo-git.x").await, 200);
    assert_eq!(gate(Some(&tom), "demo-git.x").await, 403);
    assert_eq!(gate(None, "demo-git.x").await, 403);
    // and on the service's own name, as ever
    assert_eq!(gate(Some(&tom), "git.x").await, 200);
    assert_eq!(gate(None, "git.x").await, 401);
    // the gate's own pages on the door, which is one of this box's names
    let page = |cookie: Option<&str>| {
        let mut r = http
            .get(b.url("/_dd/me"))
            .header("host", "demo-git.localhost");
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        async move {
            let r = r.send().await.unwrap();
            let to = r
                .headers()
                .get("location")
                .map(|v| v.to_str().unwrap().to_string());
            (r.status().as_u16(), to)
        }
    };
    assert_eq!(page(Some(&demo)).await.0, 200);
    assert_eq!(
        page(Some(&tom)).await,
        (303, Some("https://git.localhost/_dd/me".into()))
    );
    assert_eq!(
        page(None).await,
        (303, Some("https://home.localhost/_dd/demo".into()))
    );
}

/// A gate for one member, syncing its directory from `a`.
async fn gate_for(
    a: &Box_,
    member_id: String,
    scratch_name: &str,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    verify::start(verify::Config {
        mail: None,
        home: vec![],
        members: Some(verify::Members::list(vec![member_id])),
        release_pub: None,
        web_dir: None,
        photos: None,
        library: None,
        fleet: Default::default(),
        thanos: None,
        adblock: None,
        forge_events: None,
        storage_ledger: None,
        demo_library: None,
        tmdb: None,
        search: None,
        id_origin: None,
        box_only: false,
        owner: None,
        admin: None,
        boxes: Default::default(),
        oidc: None,
        app_manifest: None,
        member_list: None,
        network: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        dir: {
            let d = scratch(scratch_name).join("keys");
            copy_entries(a, &d);
            d
        },
        directory_readers: vec![],
        names: vec!["127.0.0.1".into()],
        control: None,
        identity_from: Default::default(),
        house_peers: vec![],
        domain: Some("localhost".to_string()),
    })
    .await
    .unwrap()
}

/// Sign in as the app does, with a device's token: the session cookie.
async fn app_signin(http: &reqwest::Client, addr: std::net::SocketAddr, token: &str) -> String {
    for _ in 0..50 {
        let r = http
            .post(format!("http://{addr}/_dd/app/signin"))
            .form(&[("token", token)])
            .send()
            .await
            .unwrap();
        if let Some(c) = r.headers().get("set-cookie") {
            return c.to_str().unwrap().split(';').next().unwrap().to_string();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("never signed in");
}

/// A disk image goes only just after the passkey: a browser session that
/// came from somewhere else - a stolen cookie, a tab left open - is refused
/// a DELETE under /images/, while reading them and a device's own signed
/// token are let through.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_image_wants_the_passkey_just_now() {
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    let dev = Device::new();
    dev.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
    let id = dev
        .dd_ok(&args(&["identity", "show"], &d))
        .lines()
        .find_map(|l| l.strip_prefix("member id:"))
        .unwrap()
        .trim()
        .to_string();
    let (addr, _task) = gate_for(&a, id, "imagefresh").await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let token = dev.token();
    let mut cookie = None;
    for _ in 0..50 {
        let r = http
            .post(format!("http://{addr}/_dd/app/signin"))
            .form(&[("token", token.as_str())])
            .send()
            .await
            .unwrap();
        cookie = r
            .headers()
            .get("set-cookie")
            .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_string());
        if cookie.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let cookie = cookie.expect("tom signs in");
    let ask = |method: &str, auth: (&str, String)| {
        http.get(format!("http://{addr}/verify"))
            .header("x-original-method", method)
            .header("x-original-uri", "/images/snapshots/abc")
            .header("x-original-host", "files.localhost")
            .header(auth.0, auth.1)
            .send()
    };
    let deleting = ask("DELETE", ("cookie", cookie.clone())).await.unwrap();
    assert_eq!(
        deleting.status().as_u16(),
        403,
        "a session alone deletes nothing"
    );
    let reading = ask("GET", ("cookie", cookie)).await.unwrap();
    assert_eq!(reading.status().as_u16(), 200, "reading needs no passkey");
    let device = ask(
        "DELETE",
        ("authorization", format!("Bearer {}", dev.token())),
    )
    .await
    .unwrap();
    assert_eq!(
        device.status().as_u16(),
        200,
        "`dd image delete` signs for itself"
    );
}

/// The Devices tab's promises: every session is listed, ending another one
/// wants the passkey just now, signing out ends a session for good, and
/// removing a device ends what it signed in.
#[tokio::test(flavor = "multi_thread")]
async fn sessions_are_listed_and_end_when_their_device_goes() {
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    let laptop = Device::new();
    let phone = Device::new();
    laptop.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
    laptop.dd_ok(&args(&["device", "admit", &phone.public_key()], &d));
    // the phone signs as tom too (it is handed the root, as in the test above)
    let root = laptop.dd_ok(&["identity", "export"]);
    {
        use std::io::Write as _;
        let mut c = Command::new(env!("CARGO_BIN_EXE_dd"))
            .args(["identity", "import", "--name", "tom"])
            .env("DD_KEYRING_FILE", &phone.keyring)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(root.as_bytes()).unwrap();
        assert!(c.wait().unwrap().success());
    }
    let id = laptop
        .dd_ok(&args(&["identity", "show"], &d))
        .lines()
        .find_map(|l| l.strip_prefix("member id:"))
        .unwrap()
        .trim()
        .to_string();
    let (addr, _task) = gate_for(&a, id, "signins").await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mine = app_signin(&http, addr, &laptop.token()).await;
    let phones = app_signin(&http, addr, &phone.token()).await;
    let list = |c: String| {
        let http = http.clone();
        async move {
            http.get(format!("http://{addr}/_dd/signins"))
                .header("cookie", c)
                .send()
                .await
                .unwrap()
        }
    };

    let r = list(mine.clone()).await;
    assert_eq!(r.status().as_u16(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    let sessions = v["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "{v}");
    assert_eq!(sessions.iter().filter(|s| s["current"] == true).count(), 1);
    assert_eq!(
        v["keys"].as_object().unwrap().len(),
        2,
        "both devices were used"
    );

    // another session, ended without the passkey: refused
    let r = http
        .post(format!("http://{addr}/_dd/signins/end"))
        .header("cookie", &mine)
        .json(&serde_json::json!({ "others": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    assert_eq!(list(phones.clone()).await.status().as_u16(), 200);

    // the phone removed: what it signed in is nobody from then on
    let fp = phone
        .dd_ok(&["device", "show"])
        .lines()
        .next()
        .unwrap()
        .trim()
        .to_string();
    // to both boxes: each holds the directory (the agreed log, in a fleet)
    let both = [
        d.clone(),
        vec!["--directory".into(), format!("http://{addr}/_dd/directory")],
    ]
    .concat();
    laptop.dd_ok(&args(&["device", "remove", &fp[..8]], &both));
    let mut gone = false;
    for _ in 0..50 {
        if list(phones.clone()).await.status().as_u16() == 401 {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(gone, "the removed phone's session still works");
    assert_eq!(list(mine.clone()).await.status().as_u16(), 200);

    // signing out ends the session itself, not only the cookie
    http.post(format!("http://{addr}/_dd/logout"))
        .header("cookie", &mine)
        .send()
        .await
        .unwrap();
    assert_eq!(list(mine).await.status().as_u16(), 401);
}

/// A Wi-Fi change is believed for the member's passkey signature over it,
/// on every box, whoever hands it over: the right one reaches the box's
/// folder; the same one again, an edited one, an old one, or one from a
/// name that is not a member, does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_wifi_change_counts_only_with_the_members_passkey() {
    use base64::Engine as _;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64U};
    use p256::ecdsa::signature::Signer as _;
    use p256::pkcs8::DecodePrivateKey as _;
    use sha2::Digest as _;

    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../mail/tests/tester.json")).unwrap();
    let members: serde_json::Value =
        serde_json::from_str(fixture["members"].as_str().unwrap()).unwrap();
    let id = members["members"][0].as_str().unwrap().to_string();
    let key = p256::ecdsa::SigningKey::from_pkcs8_der(
        &STANDARD.decode(fixture["pkcs8"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let house = scratch("house");
    let house2 = scratch("house2");
    // the folder the root unit would watch (VERIFY_HOUSE), read at start
    unsafe { std::env::set_var("VERIFY_HOUSE", &house) };
    // the one page a change may be signed on
    unsafe { std::env::set_var("VERIFY_HOUSE_ORIGINS", "https://files.commonty.test") };
    let a = Box_::start(true).await;
    let (addr, _task) = gate_for(&a, id.clone(), "house-gate").await;
    // and a box that runs only the directory, as node2 does
    unsafe { std::env::set_var("VERIFY_HOUSE", &house2) };
    let (addr2, _task2) = verify::start(verify::Config {
        mail: None,
        home: vec![],
        members: Some(verify::Members::list(vec![id])),
        release_pub: None,
        web_dir: None,
        photos: None,
        library: None,
        fleet: Default::default(),
        thanos: None,
        adblock: None,
        forge_events: None,
        storage_ledger: None,
        demo_library: None,
        tmdb: None,
        search: None,
        id_origin: None,
        box_only: false,
        owner: None,
        admin: None,
        boxes: Default::default(),
        oidc: None,
        app_manifest: None,
        member_list: None,
        network: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        dir: scratch("house-dir").join("keys"),
        directory_readers: vec![],
        names: vec!["127.0.0.1".into()],
        control: None,
        identity_from: Default::default(),
        house_peers: vec![],
        domain: None,
    })
    .await
    .unwrap();
    unsafe { std::env::remove_var("VERIFY_HOUSE") };
    unsafe { std::env::remove_var("VERIFY_HOUSE_ORIGINS") };
    let http = reqwest::Client::new();
    // the member's entry, published to this gate
    let r = http
        .put(format!("http://{addr}/_dd/directory/tester"))
        .json(&fixture["v3"])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());

    let sign_at = |payload: &str, origin: &str| {
        let mut h = sha2::Sha256::new();
        h.update(b"commonty wifi v1\0");
        h.update(payload.as_bytes());
        let challenge = B64U.encode(h.finalize());
        let mut auth = sha2::Sha256::digest(b"commonty.test").to_vec();
        auth.extend_from_slice(&[5, 0, 0, 0, 1]);
        let client = serde_json::json!({
            "type": "webauthn.get",
            "challenge": challenge,
            "origin": origin,
        })
        .to_string();
        let mut data = auth.clone();
        data.extend_from_slice(&sha2::Sha256::digest(client.as_bytes()));
        let sig: p256::ecdsa::Signature = key.sign(&data);
        serde_json::json!({
            "payload": payload,
            "id": "cGsx",
            "assertion": {
                "authenticatorData": B64U.encode(&auth),
                "clientDataJSON": B64U.encode(client.as_bytes()),
                "signature": B64U.encode(sig.to_der().as_bytes()),
            },
        })
    };
    let sign = |payload: &str| sign_at(payload, "https://files.commonty.test");
    let change = |user: &str, at: u64, nonce: &str| {
        serde_json::json!({ "user": user, "ssid": "House", "psk": "a-new-password", "at": at, "nonce": nonce })
            .to_string()
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let relay = |body: serde_json::Value| {
        let http = http.clone();
        async move {
            http.post(format!("http://{addr}/_dd/house/wifi/relay"))
                .json(&body)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };

    // the directory-only box: the member's entry, its status, a change
    let r = http
        .put(format!("http://{addr2}/_dd/directory/tester"))
        .json(&fixture["v3"])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let here = http
        .get(format!("http://{addr2}/_dd/house/here"))
        .send()
        .await
        .unwrap();
    assert_eq!(here.status().as_u16(), 200);
    let r = http
        .post(format!("http://{addr2}/_dd/house/wifi/relay"))
        .json(&sign(&change("tester", now, "d1")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await.unwrap());
    assert!(
        house2.join("request").exists(),
        "the directory-only box took it"
    );
    let r = http
        .post(format!("http://{addr2}/_dd/house/wifi/relay"))
        .json(&sign(&change("nobody", now, "d2")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);

    // signed on a page of the domain that is not Settings': refused
    assert_eq!(
        relay(sign_at(
            &change("tester", now, "n0"),
            "https://home.commonty.test"
        ))
        .await,
        403,
        "a change signed on another page"
    );
    let good = sign(&change("tester", now, "n1"));
    assert_eq!(relay(good.clone()).await, 200);
    let req: serde_json::Value =
        serde_json::from_slice(&std::fs::read(house.join("request")).unwrap()).unwrap();
    assert_eq!(
        (req["ssid"].as_str(), req["nonce"].as_str()),
        (Some("House"), Some("n1"))
    );
    assert_eq!(relay(good).await, 403, "the same change twice");

    let mut edited = sign(&change("tester", now, "n2"));
    edited["payload"] = change("tester", now, "n2")
        .replace("a-new-password", "someone-elses")
        .into();
    assert_eq!(relay(edited).await, 403, "an edited change");
    assert_eq!(
        relay(sign(&change("tester", now - 3600, "n3"))).await,
        403,
        "an old change"
    );
    assert_eq!(
        relay(sign(&change("nobody", now, "n4"))).await,
        403,
        "not a member"
    );
}

/// Adding a device by code: the signed-in device asks for a code, the new
/// one offers its key against it and gets four digits, the first sees the
/// same digits and the key; one offer per code, and a stranger's session
/// sees nothing of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_is_offered_by_code_and_seen_by_its_owner_alone() {
    let a = Box_::start(true).await;
    let d = dirs([&a]);
    let laptop = Device::new();
    laptop.dd_ok(&args(
        &[
            "identity",
            "new",
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "tom",
        ],
        &d,
    ));
    let id = laptop
        .dd_ok(&args(&["identity", "show"], &d))
        .lines()
        .find_map(|l| l.strip_prefix("member id:"))
        .unwrap()
        .trim()
        .to_string();
    let (addr, _task) = gate_for(&a, id, "adding").await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let tom = app_signin(&http, addr, &laptop.token()).await;

    let started: serde_json::Value = http
        .post(format!("http://{addr}/_dd/add/start"))
        .header("cookie", &tom)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = started["code"].as_str().unwrap().to_string();
    assert!(
        started["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/add#{code}"))
    );
    let status = |c: String| {
        let http = http.clone();
        let code = code.clone();
        async move {
            let r = http
                .get(format!("http://{addr}/_dd/add/status?code={code}"))
                .header("cookie", c)
                .send()
                .await
                .unwrap();
            if r.status().is_success() {
                r.json::<serde_json::Value>().await.unwrap()
            } else {
                serde_json::json!({ "status": r.status().as_u16() })
            }
        }
    };
    assert_eq!(status(tom.clone()).await["state"], "waiting");

    // the new device, typed in lower case and without the dash: its key,
    // and a commitment to a number of its own
    let phone = Device::new();
    let pk = phone.public_key();
    let mine = "0123456789abcdef0123456789abcdef";
    let typed = code.replace('-', " ").to_lowercase();
    let r = http
        .post(format!("http://{addr}/_dd/add/offer"))
        .json(&serde_json::json!({
            "code": typed,
            "public_key": pk,
            "commit": identity::pairing_commit(&pk, mine),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let offered: serde_json::Value = r.json().await.unwrap();
    assert_eq!(offered["user"], "tom");

    let s = status(tom.clone()).await;
    assert_eq!(s["state"], "offered");
    assert_eq!(s["offer"]["public_key"], pk.as_str());
    assert!(
        s["revealed"].is_null(),
        "nothing revealed before the other number"
    );
    // the approving device's number; then the new device sees it and reveals
    let theirs = "fedcba9876543210fedcba9876543210";
    let r = http
        .post(format!("http://{addr}/_dd/add/theirs"))
        .header("cookie", &tom)
        .json(&serde_json::json!({ "code": code, "nonce": theirs }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 204);
    let w: serde_json::Value = http
        .get(format!("http://{addr}/_dd/add/wait?code={code}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(w["theirs"], theirs);
    let r = http
        .post(format!("http://{addr}/_dd/add/reveal"))
        .json(&serde_json::json!({ "code": code, "nonce": mine }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 204);
    // the approving side checks the reveal against the commitment and both
    // sides come to the same digits
    let s = status(tom.clone()).await;
    let revealed = s["revealed"].as_str().unwrap();
    assert_eq!(
        identity::pairing_commit(&pk, revealed),
        s["offer"]["commit"].as_str().unwrap()
    );
    assert_eq!(
        identity::pairing_digits(&pk, revealed, theirs),
        identity::pairing_digits(&pk, mine, theirs)
    );

    // a second device cannot take the same code
    let r = http
        .post(format!("http://{addr}/_dd/add/offer"))
        .json(&serde_json::json!({
            "code": code,
            "public_key": Device::new().public_key(),
            "commit": "0".repeat(64),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 409);
    // and nobody signed out sees any of it
    assert_eq!(status("nobody=1".into()).await["status"], 401);

    http.post(format!("http://{addr}/_dd/add/cancel"))
        .header("cookie", &tom)
        .json(&serde_json::json!({ "code": code }))
        .send()
        .await
        .unwrap();
    assert_eq!(status(tom).await["state"], "gone");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_opened_again_is_told_nothing_changed() {
    let a = Box_::start(true).await;
    let http = reqwest::Client::new();
    let first = http
        .get(a.url("/_dd/static/shell.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let tag = first.headers()["etag"].to_str().unwrap().to_string();
    assert!(!first.bytes().await.unwrap().is_empty());
    let again = |t: String| {
        http.get(a.url("/_dd/static/shell.js"))
            .header("if-none-match", t)
            .send()
    };
    let r = again(tag.clone()).await.unwrap();
    assert_eq!(r.status(), 304);
    assert!(r.bytes().await.unwrap().is_empty());
    // as nginx's gzip passes it on, weakened
    assert_eq!(again(format!("W/{tag}")).await.unwrap().status(), 304);
    assert_eq!(again("\"other\"".into()).await.unwrap().status(), 200);
}
