//! The door: the only thing on the home controller that listens on the
//! network. It ends TLS, signs people in from the keys it saved, and passes
//! each signed-in person to Home Assistant from a loopback address of their
//! own (docs/dev/home-controller.md).

pub mod backup;
pub mod ct;
pub mod gate;
pub mod ha;
pub mod members;
pub mod name;
pub mod pages;
pub mod pair;
pub mod people;
pub mod proxy;
pub mod signin;
pub mod state;
pub mod tls;
pub mod updates;
mod ws;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, State as AxState};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use identity::SigningKey;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Fut<'a, T> = futures_util::future::BoxFuture<'a, anyhow::Result<T>>;

pub const COOKIE: &str = "door";
/// how long a sign-in lasts
pub const SESSION_LIFE: u64 = 8 * 3600;

#[derive(Debug, Clone)]
pub struct Config {
    pub state_dir: PathBuf,
    pub pages_dir: PathBuf,
    /// the door: commonty0's :443
    pub listen: SocketAddr,
    /// pairing, on the house network, until /pair/finish
    pub pair_listen: SocketAddr,
    pub pair_key: PathBuf,
    /// certificate and key, PEM; None serves plain HTTP (tests)
    pub tls: Option<(PathBuf, PathBuf)>,
    /// the door's name, when it is not <label>.home.<domain> from pairing
    pub name: Option<String>,
    pub ha: SocketAddr,
    pub users_file: PathBuf,
    pub restart: Vec<String>,
    pub network_key_file: PathBuf,
    /// the Home Assistant group a person is made in
    pub ha_group: String,
    pub ct_every: Duration,
    /// the release agent's state: the release it runs, and the hold on updates
    pub agent_state: PathBuf,
    /// the fleet's members, as the release says; None only in tests
    pub members: Option<members::Members>,
    /// the gate's base url; None is https://files.<domain>
    pub gate: Option<String>,
    /// Home Assistant's backups directory
    pub ha_backups: PathBuf,
    /// the first backup this long after start, then one every backup_every
    pub backup_first: Duration,
    pub backup_every: Duration,
    /// a backup that failed is tried again this long after
    pub backup_retry: Duration,
}

pub struct App {
    pub cfg: Config,
    pub key: SigningKey,
    pub store: Mutex<state::Store>,
    nonces: Mutex<HashMap<String, u64>>,
    pub assets: pages::Assets,
    pub ha: Arc<dyn ha::Ha>,
    pub ct: Arc<dyn ct::CtSource>,
    pub hooks: Arc<dyn pair::Hooks>,
    pub directory: Arc<dyn signin::Directory>,
    pub upload: Arc<dyn backup::Upload>,
    /// one change to Home Assistant's users, or its backups, at a time
    ha_work: tokio::sync::Mutex<()>,
    /// the users file was written and Home Assistant not yet restarted
    restart_owed: std::sync::atomic::AtomicBool,
    /// the backup key, K
    pub backup_key: Mutex<library::Key>,
    /// someone was let in or removed: a backup
    pub backup_now: tokio::sync::Notify,
    /// how a restore at pairing is going
    pub restoring: Mutex<Option<backup::Restoring>>,
    /// /pair/finish done: the pairing server stops
    pub pairing_done: tokio::sync::Notify,
    /// someone was let in or paired: make their Home Assistant user
    pub reconcile: tokio::sync::Notify,
}

pub struct Parts {
    pub ha: Arc<dyn ha::Ha>,
    pub ct: Arc<dyn ct::CtSource>,
    pub hooks: Arc<dyn pair::Hooks>,
    pub directory: Arc<dyn signin::Directory>,
    pub upload: Arc<dyn backup::Upload>,
}

impl App {
    pub fn new(cfg: Config, assets: pages::Assets, parts: Parts) -> anyhow::Result<Arc<Self>> {
        let key = state::controller_key(&cfg.state_dir)?;
        let backup_key = backup::backup_key(&cfg.state_dir)?;
        let mut store = state::Store::open(&cfg.state_dir)?;
        store.drop_expired(identity::now());
        Ok(Arc::new(Self {
            cfg,
            key,
            store: Mutex::new(store),
            nonces: Mutex::new(HashMap::new()),
            assets,
            ha: parts.ha,
            ct: parts.ct,
            hooks: parts.hooks,
            directory: parts.directory,
            upload: parts.upload,
            ha_work: tokio::sync::Mutex::new(()),
            restart_owed: Default::default(),
            backup_key: Mutex::new(backup_key),
            backup_now: tokio::sync::Notify::new(),
            restoring: Mutex::new(None),
            pairing_done: tokio::sync::Notify::new(),
            reconcile: tokio::sync::Notify::new(),
        }))
    }

    /// a member of the fleet, as this release says
    pub fn member(&self, e: &identity::SignedEntry) -> bool {
        self.cfg.members.as_ref().is_none_or(|m| m.has(e))
    }

    pub fn controller(&self) -> String {
        identity::encode_public(&self.key.verifying_key())
    }

    /// `<label>.home.<domain>`, once pairing has finished
    pub fn name(&self) -> Option<String> {
        if let Some(n) = &self.cfg.name {
            return Some(n.clone());
        }
        let s = self.store.lock().unwrap();
        Some(format!(
            "{}.home.{}",
            s.state.label.as_ref()?,
            s.state.domain.as_ref()?
        ))
    }

    pub fn origin(&self) -> Option<String> {
        let scheme = if self.cfg.tls.is_some() {
            "https"
        } else {
            "http"
        };
        Some(format!("{scheme}://{}", self.name()?))
    }

    pub fn with_store<T>(&self, f: impl FnOnce(&mut state::Store) -> T) -> T {
        f(&mut self.store.lock().unwrap())
    }

    /// A new sign-in challenge, good once, for two minutes. Kept in memory:
    /// a restart forgets them, which only means asking again. At most a
    /// few thousand outstanding, so asking for them cannot fill the Pi.
    pub fn new_nonce(&self) -> String {
        let nonce = random_b64url(32);
        let now = identity::now();
        let mut n = self.nonces.lock().unwrap();
        n.retain(|_, exp| *exp > now);
        if n.len() >= 4096
            && let Some(oldest) = n.iter().min_by_key(|(_, e)| **e).map(|(k, _)| k.clone())
        {
            n.remove(&oldest);
        }
        n.insert(nonce.clone(), now + signin::NONCE_LIFE);
        nonce
    }

    /// Used up whether or not the sign-in it came with succeeds.
    pub fn take_nonce(&self, nonce: &str) -> bool {
        let now = identity::now();
        matches!(self.nonces.lock().unwrap().remove(nonce), Some(exp) if exp > now)
    }

    /// Who this request is: (name, whether they are in).
    pub fn who(&self, headers: &HeaderMap) -> Option<(String, bool)> {
        let c = cookie(headers)?;
        let s = self.store.lock().unwrap();
        let sess = s.session(&c, identity::now())?;
        // still a member, as this release says: a session made before they
        // were revoked ends with the release that revokes them
        if let Some(m) = &self.cfg.members {
            let entry = s
                .state
                .people
                .get(&sess.name)
                .map(|p| &p.entry)
                .or(sess.entry.as_ref())
                .or(s.state.asked.get(&sess.name).map(|a| &a.entry));
            if !entry.is_some_and(|e| m.has(e)) {
                return None;
            }
        }
        let is_in = s
            .state
            .people
            .get(&sess.name)
            .is_some_and(|p| p.root == sess.root);
        Some((sess.name.clone(), is_in))
    }

    /// Make the Home Assistant admin, a user for each person who has none,
    /// the users file; restart Home Assistant if the file changed. True if
    /// it was restarted.
    pub async fn reconcile_ha(&self) -> anyhow::Result<bool> {
        let _one = self.ha_work.lock().await;
        let mut admin = self.admin()?;
        let result = self.make_users(&mut admin).await;
        self.keep_admin(admin)?;
        result?;
        self.write_users().await
    }

    /// The door's Home Assistant admin, its password made first if need be
    /// (saved before Home Assistant hears it: onboarding happens once).
    fn admin(&self) -> anyhow::Result<state::HaAdmin> {
        self.with_store(|s| {
            if s.state.ha.password.is_none() {
                s.state.ha.password = Some(random_b64url(24));
                s.save()?;
            }
            Ok(s.state.ha.clone())
        })
    }

    /// What Home Assistant's answers changed of it, kept.
    fn keep_admin(&self, admin: state::HaAdmin) -> anyhow::Result<()> {
        self.with_store(|s| {
            if s.state.ha == admin {
                return Ok(());
            }
            s.state.ha = admin;
            s.save()
        })
    }

    async fn make_users(&self, admin: &mut state::HaAdmin) -> anyhow::Result<()> {
        self.ha.setup(admin).await?;
        // Home Assistant saves a new user a little later: restarted before
        // then, it never had it. Such a person gets a user again.
        let have = self.ha.users(admin).await?;
        self.with_store(|s| -> anyhow::Result<()> {
            let mut lost = false;
            for (n, p) in s.state.people.iter_mut() {
                if p.ha_user.as_ref().is_some_and(|u| !have.contains(u)) {
                    eprintln!("home assistant: {n}'s user is gone; making it again");
                    p.ha_user = None;
                    lost = true;
                }
            }
            if lost {
                s.save()?;
            }
            Ok(())
        })?;
        let missing: Vec<String> = self.with_store(|s| {
            s.state
                .people
                .iter()
                .filter(|(_, p)| p.ha_user.is_none())
                .map(|(n, _)| n.clone())
                .collect()
        });
        for name in missing {
            let user = self
                .ha
                .create_user(admin, &name, &self.cfg.ha_group)
                .await?;
            self.with_store(|s| -> anyhow::Result<()> {
                if let Some(p) = s.state.people.get_mut(&name) {
                    p.ha_user = Some(user);
                }
                s.save()
            })?;
        }
        Ok(())
    }

    /// Every person with a user gets an address; the file is rewritten and
    /// Home Assistant restarted only when it says something new, or when the
    /// last restart failed.
    async fn write_users(&self) -> anyhow::Result<bool> {
        let yaml = self.with_store(|s| -> anyhow::Result<String> {
            let names: Vec<String> = s
                .state
                .people
                .iter()
                .filter(|(_, p)| p.address.is_none() && p.ha_user.is_some())
                .map(|(n, _)| n.clone())
                .collect();
            for n in names {
                let a = s
                    .free_address()
                    .ok_or_else(|| anyhow::anyhow!("no loopback address left"))?;
                s.state.people.get_mut(&n).unwrap().address = Some(a);
            }
            s.save()?;
            Ok(state::users_yaml(&s.state))
        })?;
        let on_disk = std::fs::read_to_string(&self.cfg.users_file).unwrap_or_default();
        let owed = &self.restart_owed;
        if on_disk == yaml && !owed.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(false);
        }
        owed.store(true, std::sync::atomic::Ordering::SeqCst);
        state::write_atomic(&self.cfg.users_file, yaml.as_bytes(), 0o644)?;
        self.ha.restart().await?;
        owed.store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(true)
    }

    /// Removing someone takes their address out at once, whatever else is
    /// going on with Home Assistant.
    pub async fn rewrite_users(&self) -> anyhow::Result<()> {
        let _one = self.ha_work.lock().await;
        self.write_users().await.map(|_| ())
    }
}

pub fn random_b64url(n: usize) -> String {
    use base64::Engine as _;
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("randomness");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

pub fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

/// A POST of ours comes from our own page: SameSite=Strict already keeps
/// the cookie off another site's request, and an Origin, when sent, must
/// be the door's.
pub fn same_origin(app: &App, headers: &HeaderMap) -> bool {
    match headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        None => true,
        Some(o) => app.origin().as_deref() == Some(o),
    }
}

pub fn refuse(status: StatusCode, why: &str) -> Response {
    (status, why.to_string()).into_response()
}

async fn static_file(
    AxState(app): AxState<Arc<App>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    app.assets.serve(&name, &headers)
}

/// The door on :443: its own routes under /_door, everything else to
/// Home Assistant for a person who is in.
pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/_door/challenge", get(signin::challenge))
        .route("/_door/signin", post(signin::signin))
        .route("/_door/signout", post(signin::signout))
        .route("/_door/", get(people::page))
        .route("/_door/people", get(people::list))
        .route("/_door/ask", post(people::ask))
        .route("/_door/let-in", post(people::let_in))
        .route("/_door/remove", post(people::remove))
        .route("/_door/tls", get(updates::tls))
        .route("/_door/status", get(updates::status))
        .route("/_door/updates", post(updates::set))
        .route("/_door/static/{name}", get(static_file))
        .route("/_dd/static/{name}", get(static_file))
        .route("/_door/{*rest}", any(|| async { StatusCode::NOT_FOUND }))
        .fallback(proxy::handle)
        .with_state(app)
}

/// Pairing, on the house network: the page and its calls.
pub fn pair_router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(pair::page))
        .route("/pair/status", get(pair::status))
        .route("/pair", post(pair::pair))
        .route("/pair/finish", post(pair::finish))
        .route(
            "/pair/restore",
            post(pair::restore).layer(DefaultBodyLimit::max(backup::MAX_FILE)),
        )
        .route("/_door/static/{name}", get(static_file))
        .route("/_dd/static/{name}", get(static_file))
        .with_state(app)
}

/// Make Home Assistant's users when someone is let in, and retry while it
/// is not up.
pub async fn reconcile_loop(app: Arc<App>) {
    loop {
        let wait = match app.reconcile_ha().await {
            // once it is back, see that it kept the users it was given
            Ok(true) => Duration::from_secs(30),
            Ok(false) => Duration::from_secs(600),
            Err(e) => {
                eprintln!("home assistant: {e:#}");
                Duration::from_secs(30)
            }
        };
        tokio::select! {
            _ = app.reconcile.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use identity::{Device, Entry, SignedEntry};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A directory of the test's own, gone when the test is.
    pub struct Temp(PathBuf);
    impl std::ops::Deref for Temp {
        type Target = std::path::Path;
        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn tempdir(tag: &str) -> Temp {
        let d = std::env::temp_dir().join(format!(
            "door-test-{tag}-{}-{}",
            std::process::id(),
            random_b64url(6)
        ));
        std::fs::create_dir_all(&d).unwrap();
        Temp(d)
    }

    /// An entry whose root is also its one device, as the app makes one.
    pub fn signed_entry(name: &str) -> (SignedEntry, SigningKey) {
        let root = identity::generate();
        let pk = identity::encode_public(&root.verifying_key());
        let entry = Entry {
            name: name.into(),
            root: pk.clone(),
            recovery: String::new(),
            recovered_from: vec![],
            terms: None,
            devices: vec![Device {
                fingerprint: identity::fingerprint(&pk),
                public_key: pk,
                added: 1,
            }],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            removed_boxes: vec![],
            version: 1,
            updated: 1,
        };
        (identity::sign(entry, &root).unwrap(), root)
    }

    #[derive(Default)]
    pub struct FakeHa {
        pub made: Mutex<Vec<(String, String)>>,
        pub restarts: AtomicUsize,
        pub down: std::sync::atomic::AtomicBool,
        /// its backups directory: a backup lands there
        pub dir: Mutex<PathBuf>,
        pub backups: Mutex<u32>,
        pub restored: Mutex<Vec<String>>,
        /// users it made but never saved
        pub lost: Mutex<Vec<String>>,
        pub restart_fails: std::sync::atomic::AtomicBool,
    }

    impl ha::Ha for FakeHa {
        fn users<'a>(&'a self, _admin: &'a mut state::HaAdmin) -> Fut<'a, Vec<String>> {
            Box::pin(async move {
                let lost = self.lost.lock().unwrap().clone();
                Ok((1..=self.made.lock().unwrap().len())
                    .map(|i| format!("user{i:02}"))
                    .filter(|u| !lost.contains(u))
                    .collect())
            })
        }
        fn setup<'a>(&'a self, admin: &'a mut state::HaAdmin) -> Fut<'a, ()> {
            Box::pin(async move {
                anyhow::ensure!(!self.down.load(Ordering::SeqCst), "home assistant is down");
                admin.refresh_token.get_or_insert_with(|| "refresh".into());
                Ok(())
            })
        }
        fn create_user<'a>(
            &'a self,
            _admin: &'a mut state::HaAdmin,
            name: &'a str,
            group: &'a str,
        ) -> Fut<'a, String> {
            Box::pin(async move {
                let mut m = self.made.lock().unwrap();
                m.push((name.to_string(), group.to_string()));
                Ok(format!("user{:02}", m.len()))
            })
        }
        fn restart(&self) -> Fut<'_, ()> {
            Box::pin(async move {
                anyhow::ensure!(
                    !self.restart_fails.load(Ordering::SeqCst),
                    "the restart failed"
                );
                self.restarts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
        fn backup<'a>(&'a self, _admin: &'a mut state::HaAdmin) -> Fut<'a, ()> {
            Box::pin(async move {
                anyhow::ensure!(!self.down.load(Ordering::SeqCst), "home assistant is down");
                let n = {
                    let mut b = self.backups.lock().unwrap();
                    *b += 1;
                    *b
                };
                let dir = self.dir.lock().unwrap().clone();
                std::fs::create_dir_all(&dir)?;
                std::fs::write(
                    dir.join(format!("Commonty_{n}.tar")),
                    backup::tests::ha_backup(&format!("fake{n:04}")),
                )?;
                Ok(())
            })
        }
        fn restore<'a>(&'a self, _admin: &'a mut state::HaAdmin, id: &'a str) -> Fut<'a, ()> {
            Box::pin(async move {
                anyhow::ensure!(!self.down.load(Ordering::SeqCst), "home assistant is down");
                let dir = self.dir.lock().unwrap().clone();
                anyhow::ensure!(
                    dir.join(format!("commonty-restore-{id}.tar")).exists(),
                    "no such backup"
                );
                self.restored.lock().unwrap().push(id.to_string());
                Ok(())
            })
        }
    }

    #[derive(Default)]
    pub struct FakeCt(pub Mutex<Vec<ct::Issued>>);
    impl ct::CtSource for FakeCt {
        fn issued<'a>(&'a self, _name: &'a str) -> Fut<'a, Vec<ct::Issued>> {
            Box::pin(async move { Ok(self.0.lock().unwrap().clone()) })
        }
    }

    #[derive(Default)]
    pub struct FakeHooks {
        pub network: Mutex<Option<String>>,
        pub registered: Mutex<Option<pair::Registration>>,
    }
    impl pair::Hooks for FakeHooks {
        fn network<'a>(&'a self, key: &'a str) -> Fut<'a, ()> {
            Box::pin(async move {
                *self.network.lock().unwrap() = Some(key.to_string());
                Ok(())
            })
        }
        fn register<'a>(&'a self, r: &'a pair::Registration) -> Fut<'a, ()> {
            Box::pin(async move {
                *self.registered.lock().unwrap() = Some(r.clone());
                Ok(())
            })
        }
    }

    #[derive(Default)]
    pub struct FakeDirectory(pub Mutex<HashMap<String, SignedEntry>>);
    impl signin::Directory for FakeDirectory {
        fn entry<'a>(&'a self, _domain: &'a str, name: &'a str) -> Fut<'a, Option<SignedEntry>> {
            Box::pin(async move { Ok(self.0.lock().unwrap().get(name).cloned()) })
        }
    }

    pub const NAME: &str = "abcdefghijklmnop.home.commonty.org";

    pub struct Rig {
        pub app: Arc<App>,
        pub ha: Arc<FakeHa>,
        pub ct: Arc<FakeCt>,
        pub hooks: Arc<FakeHooks>,
        pub directory: Arc<FakeDirectory>,
        pub upload: Arc<backup::tests::FakeUpload>,
        _dir: Temp,
    }

    pub fn config(dir: &std::path::Path) -> Config {
        Config {
            state_dir: dir.join("state"),
            pages_dir: dir.into(),
            listen: "127.0.0.1:0".parse().unwrap(),
            pair_listen: "127.0.0.1:0".parse().unwrap(),
            pair_key: dir.join("pair"),
            tls: None,
            name: Some(NAME.into()),
            ha: "127.0.0.1:8123".parse().unwrap(),
            users_file: dir.join("users.yaml"),
            restart: vec![],
            network_key_file: dir.join("network-key"),
            ha_group: "system-admin".into(),
            ct_every: Duration::from_secs(3600),
            agent_state: dir.join("agent"),
            members: None,
            gate: None,
            ha_backups: dir.join("ha-backups"),
            backup_first: Duration::from_secs(3600),
            backup_every: Duration::from_secs(24 * 3600),
            backup_retry: Duration::from_secs(300),
        }
    }

    pub fn rig_with(tag: &str, edit: impl FnOnce(&mut Config)) -> Rig {
        let dir = tempdir(tag);
        let mut cfg = config(&dir);
        edit(&mut cfg);
        let ha = Arc::new(FakeHa::default());
        let ct = Arc::new(FakeCt::default());
        let hooks = Arc::new(FakeHooks::default());
        let directory = Arc::new(FakeDirectory::default());
        let upload = Arc::new(backup::tests::FakeUpload::default());
        *ha.dir.lock().unwrap() = cfg.ha_backups.clone();
        let app = App::new(
            cfg,
            pages::Assets::tree().unwrap(),
            Parts {
                ha: ha.clone(),
                ct: ct.clone(),
                hooks: hooks.clone(),
                directory: directory.clone(),
                upload: upload.clone(),
            },
        )
        .unwrap();
        Rig {
            app,
            ha,
            ct,
            hooks,
            directory,
            upload,
            _dir: dir,
        }
    }

    pub fn rig(tag: &str) -> Rig {
        rig_with(tag, |_| {})
    }

    /// Serve a router on a free port; its base url.
    pub async fn serve(r: Router) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
        format!("http://{a}")
    }

    pub fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    /// Put someone in, as pairing or a let-in would, with a session.
    pub fn person(app: &App, name: &str) -> (SignedEntry, SigningKey, String) {
        let (e, k) = signed_entry(name);
        let cookie = random_b64url(32);
        app.with_store(|s| {
            s.state.people.insert(
                name.into(),
                state::Person {
                    root: e.entry.root.clone(),
                    entry: e.clone(),
                    ha_user: None,
                    address: None,
                    since: 1,
                },
            );
            s.state.sessions.insert(
                state::hash(&cookie),
                state::Session {
                    name: name.into(),
                    root: e.entry.root.clone(),
                    expires: identity::now() + 60,
                    entry: None,
                },
            );
            s.save().unwrap();
        });
        (e, k, format!("{COOKIE}={cookie}"))
    }

    #[test]
    fn the_cookie_is_found_among_others() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "a=1; door=xyz; b=2".parse().unwrap());
        assert_eq!(cookie(&h).as_deref(), Some("xyz"));
        h.insert(header::COOKIE, "doorx=1; adoor=2".parse().unwrap());
        assert_eq!(cookie(&h), None);
    }

    #[test]
    fn a_nonce_is_good_once_and_not_forever() {
        let r = rig("nonce");
        let n = r.app.new_nonce();
        assert!(r.app.take_nonce(&n));
        assert!(!r.app.take_nonce(&n), "used");
        assert!(!r.app.take_nonce("made-up"));
        let old = r.app.new_nonce();
        r.app
            .nonces
            .lock()
            .unwrap()
            .insert(old.clone(), identity::now() - 1);
        assert!(!r.app.take_nonce(&old), "ran out");
    }

    #[tokio::test]
    async fn the_doors_own_routes() {
        let r = rig("routes");
        let (_, _, ann) = person(&r.app, "ann");
        let url = serve(router(r.app.clone())).await;
        let c = client();
        let page = |name: &str| String::from_utf8(r.app.assets.page(name).to_vec()).unwrap();
        let get = |path: &str, cookie: Option<&str>| {
            let mut q = c.get(format!("{url}{path}"));
            if let Some(k) = cookie {
                q = q.header("cookie", k);
            }
            async move {
                let res = q.send().await.unwrap();
                (res.status().as_u16(), res.text().await.unwrap())
            }
        };
        assert_eq!(get("/_door/", None).await, (200, page("signin")));
        assert_eq!(get("/_door/", Some(&ann)).await, (200, page("people")));
        assert_eq!(get("/_door/nope", Some(&ann)).await.0, 404);
        let post = c
            .post(format!("{url}/_door/nope"))
            .header("cookie", &ann)
            .send()
            .await
            .unwrap();
        assert_eq!(post.status(), 404, "never on to Home Assistant");
        assert_eq!(get("/_door/static/door.css", None).await.0, 200);
        // the shared stylesheets ask for the bar and fonts here
        assert_eq!(get("/_dd/static/bar.css", None).await.0, 200);
        assert_eq!(get("/_door/static/state.json", None).await.0, 404);
    }

    #[tokio::test]
    async fn people_get_users_addresses_and_one_restart() {
        let r = rig("reconcile");
        person(&r.app, "ann");
        person(&r.app, "bob");
        r.app.reconcile_ha().await.unwrap();
        assert_eq!(r.ha.made.lock().unwrap().len(), 2);
        assert_eq!(r.ha.restarts.load(Ordering::SeqCst), 1);
        let yaml = std::fs::read_to_string(&r.app.cfg.users_file).unwrap();
        assert_eq!(
            yaml,
            "\"127.0.1.1\": \"user01\"\n\"127.0.1.2\": \"user02\"\n"
        );
        let pw = r.app.with_store(|s| s.state.ha.password.clone());
        assert!(pw.is_some_and(|p| p.len() >= 32));
        // nothing new: no restart
        r.app.reconcile_ha().await.unwrap();
        assert_eq!(r.ha.restarts.load(Ordering::SeqCst), 1);
        assert_eq!(r.ha.made.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_user_home_assistant_lost_is_made_again() {
        let r = rig("lost-user");
        person(&r.app, "ann");
        assert!(r.app.reconcile_ha().await.unwrap());
        // restarted before it saved ann's user
        r.ha.lost.lock().unwrap().push("user01".into());
        assert!(r.app.reconcile_ha().await.unwrap());
        assert_eq!(r.ha.made.lock().unwrap().len(), 2);
        let yaml = std::fs::read_to_string(&r.app.cfg.users_file).unwrap();
        assert_eq!(yaml, "\"127.0.1.1\": \"user02\"\n");
        assert_eq!(r.ha.restarts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_restart_is_tried_again() {
        let r = rig("restart-fails");
        person(&r.app, "ann");
        r.ha.restart_fails.store(true, Ordering::SeqCst);
        assert!(r.app.reconcile_ha().await.is_err());
        assert!(r.app.cfg.users_file.exists());
        r.ha.restart_fails.store(false, Ordering::SeqCst);
        // the file is already right, and still it restarts
        assert!(r.app.reconcile_ha().await.unwrap());
        assert_eq!(r.ha.restarts.load(Ordering::SeqCst), 1);
        assert!(!r.app.reconcile_ha().await.unwrap());
    }

    #[tokio::test]
    async fn home_assistant_down_leaves_people_waiting_not_lost() {
        let r = rig("ha-down");
        person(&r.app, "ann");
        r.ha.down.store(true, Ordering::SeqCst);
        assert!(r.app.reconcile_ha().await.is_err());
        assert!(
            r.app
                .with_store(|s| s.state.people["ann"].ha_user.is_none())
        );
        assert!(!r.app.cfg.users_file.exists());
        r.ha.down.store(false, Ordering::SeqCst);
        r.app.reconcile_ha().await.unwrap();
        assert!(
            r.app
                .with_store(|s| s.state.people["ann"].address == Some(1))
        );
    }
}
