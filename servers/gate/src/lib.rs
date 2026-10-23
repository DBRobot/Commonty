//! The verifier: answers nginx's auth_request for every service on this box.
//!
//! Two credentials exist and nothing else: a biscuit signed by one of the
//! devices in the person's own entry, or this box's session cookie from a
//! passkey login. The passkey itself was enrolled on a link that a device
//! signed. Nothing here can sign as anyone, so nothing here is worth taking;
//! no identity server is asked, because there is none.
//!
//! Which keys are a user's is the directory (directory.rs): an entry the
//! person signs with a key they alone hold. This box stores and serves it and
//! can add nothing to it. With VERIFY_ROLE=directory that is all a box does.

pub mod adblock;
mod adddevice;
mod computers;
mod controller;
mod demo_photos;
mod directory;
mod docs;
pub mod fleet;
mod forge_events;
mod friends;
mod home;
pub mod ident;
mod joke;
mod leaving;
pub mod library;
pub mod mail;
mod member_boxes;
pub mod network;
mod oidc;
pub mod pages;
mod photos;
mod private;
mod releases;
mod session;
mod signins;
mod sitemap;
mod sso;
pub mod status;
mod storage;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::AUTHORIZATION},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post, put},
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use biscuit_auth::{Biscuit, PublicKey, UnverifiedBiscuit, builder::Algorithm};
use serde::{Deserialize, Serialize};
use webauthn_rs::prelude::*;

struct App {
    directory: Arc<directory::Directory>,
    sessions: session::Sessions,
    /// who is signed in where, and what each key last did (signins.rs)
    signins: signins::SignIns,
    /// where this gate keeps what it keeps
    state_dir: PathBuf,
    /// the house's boxes and their Wi-Fi (home.rs)
    home_net: home::Home,
    /// devices being added by QR code, ten minutes each (adddevice.rs)
    adding: adddevice::Adding,
    webauthn: Webauthn,
    ceremonies: Mutex<HashMap<String, (Instant, Ceremony)>>,
    /// a passkey the browser just made, waiting for `dd enrol` to collect
    /// it and sign it into the entry. Keyed by the enrol token, so only the
    /// terminal that printed the link can pick it up. Never stored.
    pending: Mutex<HashMap<String, (Instant, identity::Passkey)>>,
    oidc: Option<oidc::Issuer>,
    home: Vec<pages::Service>,
    members: Option<Members>,
    /// the passkeys' relying party: what a join asks the browser to sign for
    domain: String,
    web_dir: Option<PathBuf>,
    photos: Option<Photos>,
    /// the demo's counted requests this hour, per host and per demo on it
    /// (`rate:N/M`)
    demo_rate: Mutex<HashMap<String, (u64, u32)>>,
    /// photo passwords on their way from a browser to the app (photos.rs)
    handoffs: Mutex<HashMap<String, photos::Handoff>>,
    /// where sign-in is, when it is the id Worker's (sso.rs)
    id_origin: Option<String>,
    /// this box's owner and the fleet's admin (Config)
    owner: Option<String>,
    admin: Option<String>,
    boxes: BTreeMap<String, BoxOf>,
    /// challenges given to the id page, waiting for its answer (sso.rs)
    sso: Mutex<HashMap<String, sso::Waiting>>,
    /// the encrypted libraries' gate (library.rs), on a box with the bucket
    library: Option<library::Gate>,
    /// members' locked mail, on a box with the mail bucket (mail.rs)
    mail: Option<mail::Mail>,
    /// whose entry named a reader of each library last time (library.rs)
    library_owners: Mutex<HashMap<String, String>>,
    network: Option<network::Door>,
    demo_library: Option<(String, String)>,
    app_manifest: Option<String>,
    tmdb: Option<String>,
    search: Option<String>,
    /// the app manifest as last read, and when: a good one for ten
    /// minutes, the lack of one for one
    app_seen: Mutex<Option<(Instant, Option<SignedApp>)>>,
    /// the forge's release notes (releases.rs), from where the member
    /// list is published; None: there are none to tell
    releases: Option<releases::Notes>,
    fleet: fleet::Fleet,
    thanos: Option<String>,
    /// which boxes may say whether each service works (status.rs)
    service_boxes: status::Speakers,
    /// the uptime page's answer, worked out now and then (status.rs)
    status: status::Cache,
    /// this box's dd-control: the agreed state the fleet pages read
    control: Option<String>,
    /// the network reaper's brake on a state that looks wrong (network.rs)
    reap_brake: Arc<network::Brake>,
    /// the joined boxes members set up, as last read (member_boxes.rs)
    member_boxes: Mutex<member_boxes::Cache>,
    /// Ad blocking at home: Pi-hole's api on this box, and its household
    adblock: Option<adblock::Adblock>,
    /// the forge's changes as they happen (forge_events.rs)
    forge_events: Option<forge_events::ForgeEvents>,
    /// each member's allowance, as the storage ledger counted it (storage.rs)
    storage: storage::Ledger,
    /// who has just shown their passkey, and until when: deleting a disk
    /// image asks for that, not only a session
    fresh: Mutex<HashMap<String, u64>>,
    /// the demo's Photos, read through the gate (demo_photos.rs)
    demo_photos: Arc<demo_photos::DemoPhotos>,
    /// who is friends with whom, and who came in as a guest (friends.rs)
    friends: friends::Store,
    /// which member vouched for which home controller, and passkeys'
    /// names on their way to the app (controller.rs)
    controllers: controller::Store,
}

/// The app, as the release key vouched for it.
#[derive(Clone)]
struct SignedApp {
    tag: String,
    commit: String,
    from: String,
    files: std::collections::BTreeMap<String, pages::AppFile>,
}

enum Ceremony {
    Enrol {
        user: String,
        state: PasskeyRegistration,
    },
    /// a browser making an account: the passkey first
    Join {
        user: String,
        state: PasskeyRegistration,
    },
    /// then the entry that names it as root, waiting for that passkey's
    /// assertion over its hash
    JoinSign { entry: Box<identity::Entry> },
    Login {
        user: String,
        state: PasskeyAuthentication,
    },
}

/// Everything a box needs to know to run. main.rs reads it from the
/// environment; the tests build it directly and run boxes in-process.
pub struct Config {
    pub bind: std::net::SocketAddr,
    pub dir: PathBuf,
    /// the other boxes in this house, which a Wi-Fi change is relayed to
    /// and the house page asks after; not the rest of the fleet
    pub house_peers: Vec<String>,
    /// keys that may read any one entry and never the list (the mail
    /// Worker's, fleet/mail.json). Empty: single entries stay anyone's
    pub directory_readers: Vec<String>,
    /// names this box answers to besides its domain's own (home.<domain>
    /// and VERIFY_HOSTS): a directory box's address on the fleet's network.
    /// A token naming any other audience is refused here
    pub names: Vec<String>,
    /// the directory as the control plane's agreed log: dd-control's
    /// address, its submit socket, and its config (the voting boxes);
    /// None: this box's directory alone
    pub control: Option<(String, PathBuf, PathBuf)>,
    /// where who-is-who comes from (VERIFY_IDENTITY_FROM, ident.rs):
    /// replay, shadow or agreed; agreed and shadow need `control`
    pub identity_from: ident::From,
    /// None: the directory alone. Some(domain): the full verifier, with the
    /// browser login scoped to that domain.
    pub domain: Option<String>,
    /// the issuer Passwords signs people in through (oidc.rs)
    pub oidc: Option<OidcConfig>,
    /// the tiles on the home page: what this box offers a signed-in person
    pub home: Vec<pages::Service>,
    /// Who may use the services: member ids (identity::member_id of each
    /// person's root), from the signed release. An entry says who someone
    /// is; only this says they may come in. None: no gate - the directory
    /// alone, or a test. A full box always has a list, empty meaning nobody.
    pub members: Option<Members>,
    /// The release key, base64: what signs an invite. A person whose entry
    /// carries a grant this key made is a member too, unless revoked.
    pub release_pub: Option<String>,
    /// Our Rust for the browser (crates/web, built by the flake), served
    /// under /_dd/web/. None: no pages that need it.
    pub web_dir: Option<PathBuf>,
    /// Photos: the ente account a passkey opens (pages::photos). None on a
    /// box without the photos role.
    pub photos: Option<Photos>,
    /// the library gate, if this box holds the libraries bucket
    pub library: Option<library::Gate>,
    /// members' locked mail (mail.rs), if this box keeps the mail bucket
    pub mail: Option<mail::Mail>,
    pub network: Option<network::Door>,
    /// The library the demo account reads: an id and its key, both in the
    /// open on purpose (see the option in modules/library/libraries.nix).
    pub demo_library: Option<(String, String)>,
    /// Where the signed app manifest is published (the releases branch,
    /// beside the box releases). The downloads page offers what it names,
    /// and nothing else. None: no page.
    pub app_manifest: Option<String>,
    /// Where the release's signed member list is published (the releases
    /// branch, members.json): the directory serves it at /_dd/members, for
    /// a client to anchor other members' entries to. None: not served.
    pub member_list: Option<String>,
    /// Every box in the fleet, for Settings (Network) and the Git page. Empty on a
    /// box that is not told.
    pub fleet: fleet::Fleet,
    /// Thanos on this box, which holds every box's facts (fleet.rs). None:
    /// this box does not gather them, and every box shows as unknown.
    pub thanos: Option<String>,
    /// service -> the boxes the release builds with it: only their word on
    /// it counts (status.rs)
    pub service_boxes: status::Speakers,
    /// Pi-hole on this box (adblock.rs). None: no ad blocking here.
    pub adblock: Option<adblock::Adblock>,
    /// how to reach Forgejo's database to listen for its changes
    /// (a libpq connection string); None: the Git pages do not update live
    pub forge_events: Option<String>,
    /// where the storage ledger writes what each member keeps (storage.rs)
    pub storage_ledger: Option<PathBuf>,
    /// The fleet's TMDB key, handed to signed-in pages so a member's own
    /// device can look up a film's poster by its title. The box never
    /// sees the titles: they are sealed in the library. None: no posters,
    /// the pages draw stills and title cards instead.
    pub tmdb: Option<String>,
    /// SearXNG on this box, for Chat's web search: None, no search
    pub search: Option<String>,
    /// The id Worker's origin (https://id.<domain>): sign-in is there, once
    /// for every box, and this box's sessions are for each of its names
    /// alone (sso.rs). None: this box's own sign-in page, as before.
    pub id_origin: Option<String>,
    /// A box that serves its own pages and no one else's (role "box"):
    /// sign-in by the id Worker, and its pages for its owner and the admin
    /// alone. No member pages, no sign-up, no demo.
    pub box_only: bool,
    /// Whose box this is (box.json owner), and the fleet's admin
    /// (policy.json owner): who sees this box's metrics. The admin sees
    /// every box's.
    pub owner: Option<String>,
    pub admin: Option<String>,
    /// Every box: whose it is and where its own metrics answer (the
    /// Metrics page's sources)
    pub boxes: BTreeMap<String, BoxOf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BoxOf {
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub metrics: String,
}

/// What the photos page needs to make or open an ente account for a person:
/// museum's address, the address suffix under which museum takes our
/// verification code, and that code.
#[derive(Clone, Debug)]
pub struct Photos {
    pub api: String,
    pub email_suffix: String,
    pub code: String,
    /// the demo account's password: a member's comes from their passkey,
    /// the demo has none, so the box holds one. None: no demo photos.
    pub demo_password: Option<String>,
}

/// What the demo may do on one host, by the tile whose door that host is.
/// The demo's door is not always the member's: a tile that sends the demo
/// somewhere else is a permission for THAT host and for nothing on the
/// one members go to.
fn demo_allowance(home: &[pages::Service], host: &str) -> Option<String> {
    home.iter().find_map(|s| {
        let door = s.demo_url.as_ref().unwrap_or(&s.url);
        let h = door.split("//").nth(1)?.split('/').next()?;
        h.eq_ignore_ascii_case(host)
            .then(|| s.demo.clone())
            .flatten()
    })
}

/// Which name this request came in under: the vhost nginx was asked for,
/// not the address the verifier happens to listen on. A token that names
/// its audience names one of these.
pub(crate) fn asked_host(headers: &HeaderMap) -> String {
    headers
        .get("x-original-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).to_lowercase())
        .unwrap_or_default()
}

/// fleet/members.json: ids let in, ids shut out. The file is either a bare
/// list (the first shape) or {"members": [...], "revoked": [...]}.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Members {
    pub members: Vec<String>,
    pub revoked: Vec<String>,
    /// when each revoked id was suspended (leaving.rs)
    pub removed: std::collections::BTreeMap<String, u64>,
    /// id -> why it is suspended, as the member reads it
    pub suspended: std::collections::BTreeMap<String, String>,
}

impl Members {
    pub fn list(members: Vec<String>) -> Self {
        Self {
            members,
            ..Default::default()
        }
    }
    pub fn parse(json: &str) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum File {
            List(Vec<String>),
            Full {
                #[serde(default)]
                members: Vec<String>,
                #[serde(default)]
                revoked: Vec<String>,
                #[serde(default)]
                removed: std::collections::BTreeMap<String, u64>,
                #[serde(default)]
                suspended: std::collections::BTreeMap<String, String>,
            },
        }
        Ok(match serde_json::from_str::<File>(json)? {
            File::List(members) => Self::list(members),
            File::Full {
                members,
                revoked,
                removed,
                suspended,
            } => Self {
                members,
                revoked,
                removed,
                suspended,
            },
        })
    }
}

pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect: String,
}

/// The one client for the calls this gate makes out - Pi-hole, thanos, the
/// other boxes, search: connections are kept and used again. Each call sets
/// how long it will wait.
pub(crate) fn http() -> reqwest::Client {
    static C: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default()
    });
    C.clone()
}

/// Usernames are also filenames here, so the whitelist is strict.
fn valid_user(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !s.starts_with('.')
}

impl App {
    /// The person's entry, or their held sign-up: this box took the hold,
    /// so it answers for it - the name is taken here, they can sign in, and
    /// they land on the waiting page, where a code finishes the job.
    fn entry(&self, user: &str) -> Result<Option<identity::SignedEntry>> {
        Ok(self
            .directory
            .entry(user)?
            .or_else(|| self.directory.held(user)))
    }

    /// The signed app manifest. A page that offers downloads to strangers
    /// offers exactly the files the release key vouched for, so anything
    /// that does not verify against it is as good as absent - including a
    /// manifest this box was handed by a forge, a mirror, or a proxy that
    /// had been got at.
    async fn signed_app(&self) -> Option<SignedApp> {
        let url = self.app_manifest.as_ref()?;
        if let Some((at, seen)) = &*self.app_seen.lock().unwrap() {
            let fresh = if seen.is_some() { 600 } else { 60 };
            if at.elapsed() < Duration::from_secs(fresh) {
                return seen.clone();
            }
        }
        let read = async {
            let key = self.directory.release()?;
            let raw = http()
                .get(url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .text()
                .await
                .ok()?;
            let p = match release::verify_doc(&raw, "app", key) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("app manifest refused: {e}");
                    return None;
                }
            };
            let sha256 = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
            let files = p["files"]
                .as_object()?
                .iter()
                .filter_map(|(name, f)| {
                    let sha = f["sha256"].as_str().filter(|s| sha256(s))?;
                    let url = f["url"].as_str().filter(|u| u.starts_with("https://"))?;
                    Some((
                        name.clone(),
                        pages::AppFile {
                            url: url.to_string(),
                            sha256: sha.to_string(),
                        },
                    ))
                })
                .collect();
            Some(SignedApp {
                tag: p["tag"].as_str()?.to_string(),
                commit: p["commit"].as_str()?.to_string(),
                from: p["from"].as_str()?.to_string(),
                files,
            })
        };
        let seen = read.await;
        *self.app_seen.lock().unwrap() = Some((Instant::now(), seen.clone()));
        seen
    }

    /// A sign-up that may go into the directory now rather than be held:
    /// its root on the member list and not shut out, or (agreed) a name the
    /// agreed state already gives the services to under this very root - a
    /// member by an invite or by a box, signing up again on this box.
    /// Shadow serves the list's answer and logs where the agreed state's
    /// differs.
    fn earned(&self, name: &str, root: &str) -> bool {
        let id = identity::member_id(root);
        let listed = self
            .members
            .as_ref()
            .is_some_and(|m| m.members.contains(&id) && !m.revoked.contains(&id));
        let agreed = || {
            let m = self.directory.agreed_member(name);
            let ok = m
                .as_ref()
                .is_some_and(|m| m.has_access() && (m.ids.contains(&id) || m.id == id));
            (ok, m)
        };
        match self.directory.ident_from {
            ident::From::Replay => listed,
            ident::From::Agreed => listed || agreed().0,
            ident::From::Shadow => {
                if self.directory.agreed_known() {
                    let (a, m) = agreed();
                    if listed != (listed || a) {
                        ident::shadow_differs(name, "sign-up let in", &listed, &a, m.as_ref());
                    }
                }
                listed
            }
        }
    }

    /// Signed in is not let in: the person's root has to be on the member
    /// list this box was released with.
    /// the demo is on when any tile has somewhere to send it
    fn demo(&self) -> bool {
        self.home.iter().any(|s| s.demo.is_some())
    }

    /// What the demo may do, by the tile whose door the request is for:
    /// `full`, `read`, `rate:N`, or nothing. nginx passes the original
    /// method and host with the gate's subrequest. This is the permission
    /// set of one account; the services' own permissions do the rest.
    fn demo_allows(&self, headers: &HeaderMap) -> bool {
        let method = headers
            .get("x-original-method")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("GET");
        // Starting a film is a POST that writes nothing: the box's player
        // decrypts it for this viewer and wipes it after, and holds only a
        // few at once. A demo that may read the films may play them.
        let playing = method == "POST"
            && headers
                .get("x-original-uri")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|u| u == "/_dd/transcode/session");
        let reading = playing || matches!(method, "GET" | "HEAD" | "OPTIONS" | "PROPFIND");
        let host = headers
            .get("x-original-host")
            .or_else(|| headers.get("host"))
            .and_then(|v| v.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or(h).to_lowercase())
            .unwrap_or_default();
        let Some(allow) = demo_allowance(&self.home, &host) else {
            return false;
        };
        match allow.as_str() {
            "full" => true,
            "read" => reading,
            a => {
                // `rate:N` is N an hour for each demo, every click on the demo
                // being a demo of its own; `rate:N/M` also holds all of them
                // together to M, since a fresh demo is only a cleared cookie away
                let Some(spec) = a.strip_prefix("rate:") else {
                    return false;
                };
                let (each, all) = match spec.split_once('/') {
                    Some((e, a)) => (e.parse::<u32>().ok(), a.parse::<u32>().ok()),
                    None => (spec.parse::<u32>().ok(), None),
                };
                let Some(each) = each else {
                    return false;
                };
                if reading {
                    return true;
                }
                // which demo: its session cookie, one per click
                let this = headers
                    .get("cookie")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|c| {
                        c.split(';')
                            .find_map(|p| p.trim().strip_prefix(&format!("{}=", session::COOKIE)))
                    })
                    .unwrap_or_default();
                let hour = session::now() / 3600;
                let mut m = self.demo_rate.lock().unwrap();
                m.retain(|_, (h, _)| *h == hour);
                let mine = format!("{host} {this}");
                let used = |m: &HashMap<String, (u64, u32)>, k: &str| m.get(k).map_or(0, |e| e.1);
                if used(&m, &mine) >= each || all.is_some_and(|all| used(&m, &host) >= all) {
                    return false;
                }
                for k in [mine, host] {
                    m.entry(k).or_insert((hour, 0)).1 += 1;
                }
                true
            }
        }
    }

    /// This session showed its passkey just now: for five minutes, what asks
    /// for that may go ahead - in this browser, not in every one the person
    /// is signed in on.
    fn saw_passkey(&self, session: &str) {
        let now = session::now();
        let mut f = self.fresh.lock().unwrap();
        f.retain(|_, until| *until > now);
        f.insert(session.to_string(), now + 300);
    }

    /// May this person see this box as a whole (its metrics): its owner,
    /// or the fleet's admin. Nobody, where neither is known.
    fn sees_box(&self, user: &str) -> bool {
        [&self.owner, &self.admin]
            .iter()
            .any(|w| w.as_deref() == Some(user))
    }

    fn is_admin(&self, user: &str) -> bool {
        self.admin.as_deref() == Some(user)
    }

    /// the boxes whose metrics this person sees: every box, for the admin;
    /// the ones they own, for anyone else
    fn boxes_seen(&self, user: &str) -> Vec<(&String, &BoxOf)> {
        self.boxes
            .iter()
            .filter(|(_, b)| self.is_admin(user) || b.owner.as_deref() == Some(user))
            .collect()
    }

    fn passkey_fresh(&self, session: &str) -> bool {
        !session.is_empty()
            && self
                .fresh
                .lock()
                .unwrap()
                .get(session)
                .is_some_and(|until| *until > session::now())
    }

    fn member(&self, user: &str) -> bool {
        if user == pages::DEMO_USER {
            return self.demo();
        }
        // agreed: the agreed view's access alone, an invite or a box that
        // counts; not read yet, nobody's in
        self.directory
            .has_access(user, "member", || self.replay_member(user))
    }

    /// Signed up and not shut out - left, suspended or removed - whatever
    /// their access: their boxes on Devices, and the backup a box they set
    /// up makes on its way in, which is how a box comes to give them access
    fn registered(&self, user: &str) -> bool {
        if user == pages::DEMO_USER {
            return false;
        }
        let agreed = || {
            self.directory
                .agreed_member(user)
                .is_some_and(|m| m.not_banned())
        };
        let replay = || {
            let Some(e) = self.directory.replay_or_held(user) else {
                return false;
            };
            !self.directory.leaving.has_left(user)
                && self.members.as_ref().is_none_or(|m| {
                    !identity::member_ids(&e.entry)
                        .iter()
                        .any(|id| m.revoked.contains(id))
                })
        };
        match self.directory.ident_from {
            ident::From::Agreed => agreed(),
            ident::From::Replay => replay(),
            ident::From::Shadow => {
                let r = replay();
                if self.directory.agreed_known() && r != agreed() {
                    let m = self.directory.agreed_member(user);
                    ident::shadow_differs(user, "registered", &r, &!r, m.as_ref());
                }
                r
            }
        }
    }

    /// Membership as this box's replay judges it: a root on the list, or an
    /// invite the owner signed, followed across recoveries
    fn replay_member(&self, user: &str) -> bool {
        let Some(m) = &self.members else {
            return true;
        };
        let Some(e) = self.directory.replay_or_held(user) else {
            return false;
        };
        !self.directory.leaving.has_left(user)
            && identity::is_member(&e.entry, &m.members, &m.revoked, self.directory.release())
    }

    /// Membership as a tri-state (ident.rs Answer): Unknown only while the
    /// identity this box goes by is not settled (agreed: not read, behind,
    /// or not recent); settled, a name it has nothing of is No. Nothing is
    /// taken away on Unknown, and nothing new is given (a network key, a
    /// session). `member` is the same answer for what was already given,
    /// where the last state read still serves.
    pub(crate) fn member_answer(&self, user: &str) -> ident::Answer {
        if user == pages::DEMO_USER {
            return ident::Answer::of(self.demo());
        }
        self.directory.access_answer(user, || {
            if self.members.is_none() {
                return ident::Answer::Yes;
            }
            ident::Answer::of(self.replay_member(user))
        })
    }

    /// Whether this person's devices belong on the fleet's network: a
    /// member, or a guest a member still vouches for. No on what is known
    /// once settled: not a member, and their own leaving, no guest record,
    /// or every friend no member. Unknown while not settled. The demo's
    /// never do.
    pub(crate) fn network_answer(&self, user: &str) -> ident::Answer {
        if user == pages::DEMO_USER {
            return ident::Answer::No;
        }
        let friends = || {
            self.friends
                .friends_of(user)
                .into_iter()
                .filter(|(f, _)| f != pages::DEMO_USER)
                .map(|(f, _)| self.member_answer(&f))
                .collect::<Vec<_>>()
        };
        on_network(
            self.member_answer(user),
            self.directory.has_left(user),
            self.friends.guest(user).is_some(),
            friends,
        )
    }

    /// What may hold the network reaper back (network.rs Guard): who it
    /// never takes off, how many it takes off at most, and whether it may
    /// take anyone off now
    fn reap_guard(&self) -> network::Guard {
        let mut keep: std::collections::BTreeSet<String> = network::people_named();
        keep.extend(self.admin.iter().cloned());
        keep.extend(self.owner.iter().cloned());
        keep.extend(self.boxes.values().filter_map(|b| b.owner.clone()));
        let grace = network::reap_grace();
        let hold = if !self.directory.settled() {
            Some("who is a member is not known yet: the agreed state is not read, not current, or not recent".to_string())
        } else {
            match self.directory.settled_for() {
                Some(t) if t >= grace => None,
                t => Some(format!(
                    "who is a member has been known for {} s, under the {} s grace",
                    t.unwrap_or_default().as_secs(),
                    grace.as_secs()
                )),
            }
        };
        network::Guard {
            keep,
            hold,
            members: self.members_now(),
            brake: Some(self.reap_brake.clone()),
        }
    }

    /// Who has access by the state this box goes by: the agreed state's
    /// members with access, or the replay's members. What the reaper's
    /// brake compares with the last state it acted on
    fn members_now(&self) -> std::collections::BTreeSet<String> {
        if self.directory.ident_from == ident::From::Agreed {
            return self
                .directory
                .ident
                .as_ref()
                .and_then(|a| a.state())
                .map(|s| {
                    s.members
                        .iter()
                        .filter(|(_, m)| m.has_access())
                        .map(|(n, _)| n.clone())
                        .collect()
                })
                .unwrap_or_default();
        }
        self.directory
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|l| l.name)
            .filter(|n| n != pages::DEMO_USER && self.replay_member(n))
            .collect()
    }

    /// A new session, while who is who is not known: none (503). One held
    /// already goes on; nothing new is given on Unknown
    pub(crate) fn no_new_session(&self) -> Option<Response> {
        (!self.directory.settled()).then(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "this box is still reading who is who; try again in a moment",
            )
                .into_response()
        })
    }

    /// One pass of the network reaper, guarded (reap_guard)
    pub(crate) async fn reap_network(&self) {
        let Some(door) = &self.network else { return };
        // an empty member list is a broken release rather than everyone
        // revoked, and it would take every device off at once
        if self.members.as_ref().is_none_or(|m| m.members.is_empty()) {
            return;
        }
        if let Err(e) = door
            .reap(
                |u| self.network_answer(u),
                self.control.as_deref(),
                &self.reap_guard(),
            )
            .await
        {
            eprintln!("network: reaping: {e:#}");
        }
    }

    /// Someone who came in through a friend link and is not a member: let
    /// in to the game servers they are invited to and nowhere else, and
    /// only while a member is still their friend. A guest made a member
    /// is a member.
    fn guest(&self, user: &str) -> bool {
        user != pages::DEMO_USER
            && !self.member(user)
            && !self.directory.has_left(user)
            && self.friends.guest(user).is_some()
            && self
                .friends
                .friends_of(user)
                .iter()
                .any(|(f, _)| f != pages::DEMO_USER && self.member(f))
    }

    fn role(&self, user: &str) -> Option<pages::Role> {
        if user == pages::DEMO_USER {
            self.demo().then_some(pages::Role::Demo)
        } else if self.member(user) {
            Some(pages::Role::Member)
        } else if self.guest(user) {
            Some(pages::Role::Guest)
        } else if self.suspension(user).is_some() {
            Some(pages::Role::Suspended)
        } else {
            None
        }
    }

    /// A member suspended for breaking the terms, within the window: why,
    /// and until when they may read and take their own things. The release-
    /// signed list says so (fleet/members.json suspended); their own
    /// leaving, if they said it, ends it at once.
    pub(crate) fn suspension(&self, user: &str) -> Option<(String, u64)> {
        let m = self.members.as_ref()?;
        if user == pages::DEMO_USER || self.directory.has_left(user) {
            return None;
        }
        // agreed: the judge said suspended, and why; the window still runs
        // from when the list removed them (the release's list, which the
        // gate is configured with as every box is)
        let agreed = || {
            let a = self.directory.agreed_member(user)?;
            if a.status != "suspended" {
                return None;
            }
            let why = a.suspended.clone().unwrap_or_default();
            let until = a
                .ids
                .iter()
                .find_map(|id| m.removed.get(id))?
                .saturating_add(leaving::window());
            (identity::now() < until).then_some((why, until))
        };
        let replay = || {
            let e = self.entry(user).ok().flatten()?;
            identity::member_ids(&e.entry).into_iter().find_map(|id| {
                let why = m.suspended.get(&id).filter(|_| m.revoked.contains(&id))?;
                let until = m.removed.get(&id)?.saturating_add(leaving::window());
                (identity::now() < until).then(|| (why.clone(), until))
            })
        };
        match self.directory.ident_from {
            ident::From::Agreed => agreed(),
            ident::From::Replay => replay(),
            ident::From::Shadow => {
                let r = replay();
                if self.directory.agreed_known() {
                    let a = agreed();
                    if r.is_some() != a.is_some() {
                        let m = self.directory.agreed_member(user);
                        ident::shadow_differs(
                            user,
                            "suspended",
                            &r.is_some(),
                            &a.is_some(),
                            m.as_ref(),
                        );
                    }
                }
                r
            }
        }
    }

    /// The hosts a guest may reach through the gate: the game servers'
    /// page and the gate's own (their friends, devices and the network).
    /// Every other service answers them 403 here, whatever its own page
    /// would have done.
    fn guest_host(&self, headers: &HeaderMap) -> bool {
        let host = headers
            .get("x-original-host")
            .or_else(|| headers.get("host"))
            .and_then(|v| v.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or(h).to_lowercase())
            .unwrap_or_default();
        host == format!("games.{}", self.domain)
    }

    /// The passkeys in the user's signed entry, as webauthn-rs credentials.
    /// A record that does not parse is skipped, never fatal: one odd entry
    /// must not lock the others out.
    fn passkeys(&self, user: &str) -> Vec<Passkey> {
        self.entry(user)
            .ok()
            .flatten()
            .map(|e| {
                e.entry
                    .passkeys
                    .iter()
                    .filter_map(|p| serde_json::from_value(p.cred.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Verify a biscuit against the devices in the user's entry. `operation`
    /// is what the request is for: a token minted for enrolment carries a
    /// check that only "enrol" satisfies, so a leaked enrol link cannot read
    /// a file, and an access token is not an enrol link.
    /// A token, with no particular name asked for: anything it says about
    /// its audience cannot match, so an audience-bearing token is refused.
    /// Only the network door uses this, and it is this box's own door.
    pub(crate) fn verify_biscuit(&self, token: &str, operation: &str) -> Result<String> {
        self.verify_biscuit_for(token, operation, &self.domain)
    }

    pub(crate) fn verify_biscuit_for(
        &self,
        token: &str,
        operation: &str,
        here: &str,
    ) -> Result<String> {
        self.verify_biscuit_device(token, operation, here)
            .map(|(user, _)| user)
    }

    /// As verify_biscuit_for, and which of the user's devices signed it.
    pub(crate) fn verify_biscuit_device(
        &self,
        token: &str,
        operation: &str,
        here: &str,
    ) -> Result<(String, String)> {
        verify_device_token(|u| self.entry(u), token, operation, here)
    }
}

/// A device's biscuit: the user it names, signed by one of that user's
/// devices in `entry_of`, good for `operation` at `here`. The user and
/// that device's fingerprint.
pub(crate) fn verify_device_token(
    entry_of: impl Fn(&str) -> Result<Option<identity::SignedEntry>>,
    token: &str,
    operation: &str,
    here: &str,
) -> Result<(String, String)> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    verify_device_token_at(entry_of, token, operation, here, now)
}

/// The same, at `now` (seconds): a join ticket is judged at the agreed
/// time its box joined, so every gate judges it alike (member_boxes.rs).
pub(crate) fn verify_device_token_at(
    entry_of: impl Fn(&str) -> Result<Option<identity::SignedEntry>>,
    token: &str,
    operation: &str,
    here: &str,
    now: u64,
) -> Result<(String, String)> {
    let unverified = UnverifiedBiscuit::from_base64(token).context("not a biscuit")?;
    // the user is named in the authority block; it has to be read before the
    // signature can be checked, because the key to check with depends on it
    let source = unverified
        .print_block_source(0)
        .context("unreadable authority block")?;
    let user = peek_user(&source).context("no user fact")?;
    anyhow::ensure!(valid_user(&user), "bad user in token");
    // the boxes' own names are nobody's, whatever entry is on file
    anyhow::ensure!(!identity::reserved_name(&user), "that name is the boxes'");
    let signed = entry_of(&user)?.context("no identity published for that name")?;
    let mut verified: Option<Biscuit> = None;
    let mut device = String::new();
    for k in &signed.entry.devices {
        let pk = match B64
            .decode(&k.public_key)
            .ok()
            .and_then(|b| PublicKey::from_bytes(&b, Algorithm::Ed25519).ok())
        {
            Some(pk) => pk,
            None => continue,
        };
        if let Ok(b) = unverified.clone().verify(|_| Ok(pk)) {
            verified = Some(b);
            device = k.fingerprint.clone();
            break;
        }
    }
    let biscuit = verified.ok_or_else(|| anyhow!("signature matches no device of {user}"))?;

    let mut authorizer = biscuit_auth::builder::AuthorizerBuilder::new()
        // the library allows the datalog run 1ms by default, wall clock;
        // on a loaded box that refused good tokens. The policy here is a
        // few facts, so a generous cap still ends a runaway token fast
        .set_limits(biscuit_auth::AuthorizerLimits {
            max_time: Duration::from_millis(200),
            ..Default::default()
        })
        .fact(format!("time({now})").as_str())
        .map_err(|e| anyhow!("{e}"))?
        .fact(format!("operation({operation:?})").as_str())
        .map_err(|e| anyhow!("{e}"))?
        .fact(format!("here({here:?})").as_str())
        .map_err(|e| anyhow!("{e}"))?
        // A token that says where it is for is only good there. Today
        // none of them do, and one opens everything its holder owns on
        // every box for as long as it lives; this is the half that has
        // to be in place before minting can start saying so, and it
        // costs nothing until then. Refusing comes first, so a token
        // meant for another box cannot fall through to the allow below.
        .policy("deny if audience($a), here($h), $a != $h")
        .map_err(|e| anyhow!("{e}"))?
        // access takes any token of the user's; anything else demands a
        // token minted for exactly that, so an hour-long access token in
        // a script cannot enrol a passkey that outlives it
        .policy(
            if operation == "access" {
                format!("allow if user({user:?})")
            } else {
                format!("allow if user({user:?}), purpose({operation:?})")
            }
            .as_str(),
        )
        .map_err(|e| anyhow!("{e}"))?
        .build(&biscuit)
        .map_err(|e| anyhow!("{e}"))?;
    authorizer
        .authorize()
        .map_err(|e| anyhow!("refused: {e}"))?;
    Ok((user, device))
}

impl App {
    fn ceremony_put(&self, c: Ceremony) -> String {
        let id = session::random_id();
        let mut m = self.ceremonies.lock().unwrap();
        m.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(300));
        m.insert(id.clone(), (Instant::now(), c));
        id
    }
    fn ceremony_take(&self, id: &str) -> Option<Ceremony> {
        self.ceremonies.lock().unwrap().remove(id).map(|(_, c)| c)
    }

    /// Who this request is, if anyone: a device-signed biscuit or this box's
    /// own session cookie. Nothing else counts. A session ended from the
    /// Devices tab, or signed in by a device since removed, is nobody.
    fn identify(&self, headers: &HeaderMap, operation: &str) -> Option<String> {
        self.identify_how(headers, operation).map(|(u, _)| u)
    }

    /// As identify, and whether a device's token said so (true) or this
    /// box's session cookie (false). A bearer that is not a biscuit is not
    /// a token: the cookie beside it is what counts.
    /// Who this is, and whether by a device's own key. Never one of the
    /// boxes' own names (identity::RESERVED_NAMES): an entry published
    /// under one before the rule is nobody here, so no service behind the
    /// gate - the forge least of all - is told it.
    fn identify_how(&self, headers: &HeaderMap, operation: &str) -> Option<(String, bool)> {
        a_person(self.identify_any(headers, operation))
    }

    fn identify_any(&self, headers: &HeaderMap, operation: &str) -> Option<(String, bool)> {
        let agent = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if let Some(tok) = bearer(headers)
            && UnverifiedBiscuit::from_base64(tok).is_ok()
        {
            return match self.verify_biscuit_device(tok, operation, &self.directory.here(headers)) {
                Ok((u, device)) => {
                    self.signins.used(&u, &device, agent);
                    Some((u, true))
                }
                Err(e) => {
                    eprintln!("biscuit refused: {e:#}");
                    None
                }
            };
        }
        let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
        let (user, id, exp) = self.sessions.session(cookie)?;
        if id.is_empty() {
            return Some((user, false));
        }
        if !self.session_live(&user, &id) {
            return None;
        }
        self.signins.saw(&user, &id, exp, agent, None);
        Some((user, false))
    }

    /// The user of a session cookie that this box made, that has not run
    /// out, and that has not been ended since. Every handler asks this.
    fn signed_in(&self, cookie: Option<&str>) -> Option<String> {
        let (user, id, _) = self.sessions.session(cookie)?;
        (id.is_empty() || self.session_live(&user, &id)).then_some(user)
    }

    /// Not ended from the Devices tab, and if a device signed it in, that
    /// device is still the user's.
    fn session_live(&self, user: &str, id: &str) -> bool {
        if self.signins.is_ended(id) {
            return false;
        }
        if let Some(device) = self.signins.device_of(id) {
            let entry = self.entry(user).ok().flatten();
            match device_kept(entry.as_ref(), &device) {
                ident::Answer::Yes => {}
                // their entry, without the device: removed, so it ends
                ident::Answer::No => {
                    self.signins.end(id);
                    return false;
                }
                // no entry to go by (not read yet): not let in now, and
                // not ended either
                ident::Answer::Unknown => return false,
            }
        }
        true
    }
}

/// Whether a session a device signed in may go on: Yes while the
/// person's entry has the device, No once their entry is there without it
/// (removed), Unknown with no entry to go by - which never ends a session
pub(crate) fn device_kept(entry: Option<&identity::SignedEntry>, device: &str) -> ident::Answer {
    match entry {
        None => ident::Answer::Unknown,
        Some(e) => ident::Answer::of(e.entry.devices.iter().any(|d| d.fingerprint == device)),
    }
}

/// On the fleet's network or not (App::network_answer): a member; or a
/// guest who has not left and whom some friend vouches for as a member.
/// No only where every part is known.
pub(crate) fn on_network(
    member: ident::Answer,
    left: bool,
    guest_record: bool,
    friends: impl FnOnce() -> Vec<ident::Answer>,
) -> ident::Answer {
    use ident::Answer;
    member.or(|| {
        if left || !guest_record {
            return Answer::No;
        }
        friends().into_iter().fold(Answer::No, |a, f| a.or(|| f))
    })
}

/// Who may acknowledge the reaper's brake: the fleet's owner (the admin),
/// by a device of theirs or a browser session that showed its passkey in
/// the last five minutes. Being on the box (loopback) is not enough.
pub(crate) fn may_ack(
    user: Option<&str>,
    admin: Option<&str>,
    by_device: bool,
    passkey_fresh: bool,
) -> Result<(), StatusCode> {
    let Some(user) = user else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if admin != Some(user) || user == pages::DEMO_USER {
        return Err(StatusCode::FORBIDDEN);
    }
    if !by_device && !passkey_fresh {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

fn reap_brake_who(app: &App, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let who = app.identify_how(headers, "access");
    let session = app
        .sessions
        .session(headers.get("cookie").and_then(|v| v.to_str().ok()))
        .map(|s| s.1)
        .unwrap_or_default();
    may_ack(
        who.as_ref().map(|(u, _)| u.as_str()),
        app.admin.as_deref(),
        who.as_ref().is_some_and(|(_, d)| *d),
        app.passkey_fresh(&session),
    )
    .map_err(|c| {
        Box::new(
            (
                c,
                "the fleet's owner, with a device or a passkey shown just now",
            )
                .into_response(),
        )
    })
}

/// GET /_dd/network/reap-brake: the owner's view of the brake: on, why,
/// and the state's hash an acknowledgement names
async fn reap_brake_state(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Err(r) = reap_brake_who(&app, &headers) {
        return *r;
    }
    let on = app.reap_brake.refused();
    Json(serde_json::json!({
        "on": on.is_some(),
        "why": on.as_ref().map(|o| o.0.clone()),
        "state": on.map(|o| o.1),
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct ReapAck {
    state: String,
}

/// POST /_dd/network/reap-brake {state}: the owner says the state the
/// brake refused is right; the next pass within ten minutes goes by it,
/// if it is still that state (network.rs Brake::acknowledge)
async fn reap_brake_ack(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(a): Json<ReapAck>,
) -> Response {
    if let Err(r) = reap_brake_who(&app, &headers) {
        return *r;
    }
    match app.reap_brake.acknowledge(&a.state, session::now()) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(network::AckRefused::Off) => {
            (StatusCode::CONFLICT, "the brake is not on").into_response()
        }
        Err(network::AckRefused::NotThatState) => (
            StatusCode::CONFLICT,
            "not the state the brake refused: it has changed since; look again",
        )
            .into_response(),
        Err(network::AckRefused::Empty) => (
            StatusCode::CONFLICT,
            "a state that gives nobody access is never reaped from",
        )
            .into_response(),
    }
}

/// `user("david")` out of a block's datalog source, without a parser.
/// What a directory read asks a token for: the purpose it was minted with,
/// where that is one a read takes (its own, or an enrol link's, for the
/// enrol page watching its own entry); else what any token is good for.
pub(crate) fn read_operation(token: &str) -> &'static str {
    let source = UnverifiedBiscuit::from_base64(token)
        .ok()
        .and_then(|b| b.print_block_source(0).ok())
        .unwrap_or_default();
    if source.contains(&format!("purpose({:?})", identity::DIRECTORY_READ)) {
        identity::DIRECTORY_READ
    } else if source.contains("purpose(\"enrol\")") {
        "enrol"
    } else {
        "access"
    }
}

fn peek_user(source: &str) -> Option<String> {
    let i = source.find("user(\"")? + 6;
    let rest = &source[i..];
    let j = rest.find('"')?;
    Some(rest[..j].to_string())
}

pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// nginx auth_request lands here for every request to a protected service.
/// who, if a person: none of the boxes' own names
fn a_person(who: Option<(String, bool)>) -> Option<(String, bool)> {
    who.filter(|(u, _)| !identity::reserved_name(u))
}

async fn verify(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    // A demo door (demo-<service>.<domain>, modules/gate/public.nix) is on
    // the open internet so anyone can look around. It lets the demo in and
    // nobody else: nothing of a member's is served through it, and the
    // services' own names stay on the private network. Nobody at all is a
    // 403 there, not a 401: some doors let a 401 through as anonymous (the
    // forge's public repos), which on this name would be the whole internet.
    let demo_door = asked_host(&headers).starts_with("demo-");
    let Some((user, by_device)) = app.identify_how(&headers, "access") else {
        return if demo_door {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::UNAUTHORIZED
        }
        .into_response();
    };
    let role = app.role(&user);
    let allowed = match role {
        Some(pages::Role::Member) => !demo_door,
        // the demo looks and does not touch: reads only, and only where a
        // tile sends it. Decided here, where the name is certain; an nginx
        // `if` runs before the gate has answered and cannot know it
        // and never the fleet's metrics, whatever a tile says
        Some(pages::Role::Demo) => {
            let metrics = format!("metrics.{}", app.domain);
            let h = asked_host(&headers);
            h != metrics
                && h.strip_prefix("demo-") != Some(metrics.as_str())
                && app.demo_allows(&headers)
        }
        // a guest reaches the games page and nothing else, whatever the
        // service behind another door would have made of them
        Some(pages::Role::Guest) => !demo_door && app.guest_host(&headers),
        // suspended: reads their own files, to take them, and nothing else
        // (their libraries are theirs alone: library.rs)
        Some(pages::Role::Suspended) => {
            let h = asked_host(&headers);
            !demo_door
                && reads_only(&headers)
                && (h == format!("files.{}", app.domain) || h == format!("home.{}", app.domain))
        }
        None => false,
    };
    if !allowed {
        // 403, not 401: nginx sends a 401 to the login page, and this person
        // is logged in. They land on the home page, which says so.
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(role) = role else {
        return StatusCode::FORBIDDEN.into_response();
    };
    // a box's metrics are its owner's and the admin's; this box's Thanos
    // holds every box's, which is the admin's alone (or this box's owner's,
    // who is the admin on the box that gathers them)
    if is_metrics_host(&app.domain, &asked_host(&headers)) {
        // a joined box's own metrics (/joined/<its name>/api/v1/...) are
        // its member's, and nobody else's, the fleet's admin included: the
        // gates say whose it is (member_boxes.rs), nginx then asks for its
        // series alone (observe/joined-metrics.nix)
        let uri = headers
            .get("x-original-uri")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        let owned = if member_boxes::metrics_box(uri).is_some() && !app.is_admin(&user) {
            member_boxes::current(&app).await.1
        } else {
            vec![]
        };
        if !member_boxes::sees_metrics(&user, app.sees_box(&user), app.is_admin(&user), uri, &owned)
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    // A disk image goes from Backups in Settings only after the passkey, and
    // the box holds the page to that: a tab left signed in is not enough.
    // `dd image delete` signs with the device's own key and asks itself; any
    // other Authorization header leaves the cookie to say who this is, and
    // a cookie is a browser's.
    let this_session = app
        .sessions
        .session(headers.get("cookie").and_then(|v| v.to_str().ok()))
        .map(|s| s.1)
        .unwrap_or_default();
    if !by_device && deletes_image(&headers) && !app.passkey_fresh(&this_session) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut r = StatusCode::OK.into_response();
    let v = HeaderValue::from_str(&user).unwrap();
    r.headers_mut()
        .insert("X-Auth-Request-Preferred-Username", v.clone());
    r.headers_mut().insert("X-Auth-Request-User", v);
    // what kind of account, for a service that treats guests differently
    // (the games manager); nginx passes it on, and strips any a client sent
    r.headers_mut()
        .insert("X-DD-Role", HeaderValue::from_static(role.as_str()));
    r
}

/// The pages this gate serves, as origins: "https://<host>.<domain>" for each
/// of VERIFY_HOSTS, and home. Nothing else may ask for a passkey here.
/// every name a token may be for on this box: the configured extras, and
/// with a domain, the domain, home.<domain> and each of VERIFY_HOSTS
fn box_names(extra: &[String], domain: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = extra.iter().map(|n| n.to_lowercase()).collect();
    if let Some(d) = domain {
        names.push(d.to_lowercase());
        names.extend(
            own_hosts(d)
                .iter()
                .filter_map(|h| h.strip_prefix("https://"))
                .map(str::to_lowercase),
        );
    }
    names.sort();
    names.dedup();
    names
}

fn own_hosts(domain: &str) -> Vec<String> {
    let mut hosts: Vec<String> = std::env::var("VERIFY_HOSTS")
        .unwrap_or_default()
        .split([',', ' '])
        .filter(|h| !h.is_empty())
        .map(|h| format!("https://{h}.{domain}"))
        .collect();
    hosts.push(format!("https://home.{domain}"));
    hosts.dedup();
    hosts
}

/// A change to a disk image - anything but reading, under /images/ - asked
/// by a browser. The path is held as nginx will route it, not as it was
/// spelled: `//images/`, `/%69mages/` and `/x/../images/` are /images/ too.
/// a request that changes nothing: what a suspended member may still make
fn reads_only(headers: &HeaderMap) -> bool {
    matches!(
        headers
            .get("x-original-method")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("GET"),
        "GET" | "HEAD" | "OPTIONS" | "PROPFIND"
    )
}

fn deletes_image(headers: &HeaderMap) -> bool {
    let h = |k| headers.get(k).and_then(|v: &HeaderValue| v.to_str().ok());
    let reads = matches!(
        h("x-original-method").unwrap_or("GET"),
        "GET" | "HEAD" | "OPTIONS" | "PROPFIND"
    );
    !reads && h("x-original-uri").is_some_and(|u| routed_path(u).starts_with("/images/"))
}

/// The path of a request as nginx routes it: no query, percent-escapes
/// decoded, runs of slashes merged, `.` and `..` resolved.
fn routed_path(uri: &str) -> String {
    let raw = uri.split(['?', '#']).next().unwrap_or_default();
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                16,
            )
        {
            decoded.push(b);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8_lossy(&decoded);
    let mut out: Vec<&str> = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    let mut path = format!("/{}", out.join("/"));
    if text.ends_with('/') && path != "/" {
        path.push('/');
    }
    path
}

/// On a demo door (demo-<service>.<domain>), the gate's own pages and
/// calls answer the demo alone, as /verify does for everything behind
/// nginx: a member's session there is sent back to the service's own name,
/// and a stranger without one is started in the demo.
async fn demo_door(
    State(app): State<Arc<App>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let host = asked_host(req.headers());
    let Some(service) = host.strip_prefix("demo-") else {
        return next.run(req).await;
    };
    // the subrequest nginx makes for everything else decides for itself
    if req.uri().path() == "/verify" {
        return next.run(req).await;
    }
    let path = req.uri().path().to_string();
    match app.identify(req.headers(), "access") {
        Some(u) if u == pages::DEMO_USER => next.run(req).await,
        // the same address on the service's own name, query and all; the
        // name is one of ours, and the path and query came in as a request
        // line, so neither can carry the redirect anywhere else
        Some(_) if fleet_host(&app.domain, service) => {
            let to = req
                .uri()
                .path_and_query()
                .map_or(path.as_str(), |p| p.as_str());
            Redirect::to(&format!("https://{service}{to}")).into_response()
        }
        Some(_) => StatusCode::NOT_FOUND.into_response(),
        // the static files a page needs; everything else starts the demo
        None if path.starts_with("/_dd/static/") || path.starts_with("/_dd/web/") => {
            next.run(req).await
        }
        None => Redirect::to(&format!("https://home.{}/_dd/demo", app.domain)).into_response(),
    }
}

/// What only a process on this box may ask (/internal/*, this box's own
/// link): from loopback, and not passed on by nginx, which connects from
/// loopback too and marks what it forwards.
async fn this_box_only(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip());
    if !from_this_box(peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(req).await
}

fn from_this_box(peer: Option<std::net::IpAddr>, headers: &HeaderMap) -> bool {
    let proxied = ["x-original-uri", "x-forwarded-for", "x-real-ip"]
        .iter()
        .any(|h| headers.contains_key(*h));
    peer.is_some_and(|ip| ip.to_canonical().is_loopback()) && !proxied
}

fn with_challenge(v: impl Serialize, ceremony: String) -> Response {
    let mut j = serde_json::to_value(v).unwrap_or_default();
    j["ceremony"] = serde_json::Value::String(ceremony);
    Json(j).into_response()
}

async fn enrol_start(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "enrol") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let existing = app.passkeys(&user);
    let uid = user_uuid(&user);
    let exclude: Vec<CredentialID> = existing.iter().map(|p| p.cred_id().clone()).collect();
    match app
        .webauthn
        .start_passkey_registration(uid, &user, &user, Some(exclude))
    {
        Ok((ccr, state)) => {
            let id = app.ceremony_put(Ceremony::Enrol { user, state });
            with_challenge(ccr, id)
        }
        Err(e) => {
            eprintln!("enrol start: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// What the browser sends when it has finished making a passkey: the
/// credential, and nothing else. A key a page made from the passkey's PRF
/// is refused: any page of the domain can ask a passkey for that secret,
/// so nothing may ever be sealed to it (docs/dev/libraries.md).
#[derive(Deserialize)]
struct Enrolled {
    #[serde(flatten)]
    credential: RegisterPublicKeyCredential,
    #[serde(default)]
    library_key: Option<serde_json::Value>,
}

impl Enrolled {
    #[allow(clippy::result_large_err)]
    fn credential(self) -> std::result::Result<RegisterPublicKeyCredential, Response> {
        match self.library_key {
            None | Some(serde_json::Value::Null) => Ok(self.credential),
            Some(_) => Err((
                StatusCode::BAD_REQUEST,
                "a passkey holds no keys: sign in with it, and open your things through the app",
            )
                .into_response()),
        }
    }
}

async fn enrol_finish(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(body): Json<Enrolled>,
) -> Response {
    let reg = match body.credential() {
        Ok(r) => r,
        Err(r) => return r,
    };
    let Some(Ceremony::Enrol { user, state }) = headers
        .get("x-dd-ceremony")
        .and_then(|v| v.to_str().ok())
        .and_then(|id| app.ceremony_take(id))
    else {
        return (StatusCode::BAD_REQUEST, "no ceremony").into_response();
    };
    let passkey = match app.webauthn.finish_passkey_registration(&reg, &state) {
        Ok(p) => p,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("passkey rejected: {e}")).into_response();
        }
    };
    // Not stored here. The browser made it; only the person's root can put
    // it in the entry, and that key is with the terminal that printed the
    // link. It collects the credential with the same token.
    let Some(key) = bearer(&headers).map(pending_key) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let id = B64_URL.encode(passkey.cred_id().as_slice());
    let cred = match serde_json::to_value(&passkey) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("enrol: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let mut m = app.pending.lock().unwrap();
    m.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(600));
    m.insert(
        key,
        (
            Instant::now(),
            identity::Passkey {
                id: id.clone(),
                cred,
                added: identity::now(),
                library_key: None,
                // which domain this passkey answers for; an assertion
                // carries a hash of it, and every box can then tell one
                // made here from one made somewhere else
                rp_id: Some(app.domain.clone()),
            },
        ),
    );
    eprintln!("passkey {id} made for {user}; waiting for dd to sign it in");
    Json(serde_json::json!({ "id": id, "user": user })).into_response()
}

/// `dd enrol` polls this with its enrol token until the browser is done.
async fn enrol_result(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if app.identify(&headers, "enrol").is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(key) = bearer(&headers).map(pending_key) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.pending.lock().unwrap().remove(&key) {
        Some((_, pk)) => Json(pk).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// A stable per-user id for the authenticator: derived from the name, so
/// the same person enrolling twice is the same user to the passkey.
fn user_uuid(user: &str) -> Uuid {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(format!("dd-user:{user}").as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    Uuid::from_bytes(b)
}

/// An account from nothing, in a browser. The passkey the browser makes is
/// the root; the entry naming it is signed by that passkey's assertion over
/// the entry's hash. This box assembles the bytes and asks; it holds no key
/// that could sign them, and every box checks the result the same way.
/// A browser sign-up: the name, and that the person ticked "I agree to the
/// Terms of use and the Privacy policy". The age question is the page's
/// alone: the answer never comes here.
#[derive(Deserialize)]
struct JoinStart {
    username: String,
    #[serde(default)]
    agree: bool,
}

async fn join_start(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(q): Json<JoinStart>,
) -> Response {
    if !q.agree {
        return (
            StatusCode::BAD_REQUEST,
            "agree to the Terms of use and the Privacy policy first",
        )
            .into_response();
    }
    let user = q.username.trim().to_lowercase();
    if !valid_user(&user) {
        return (
            StatusCode::BAD_REQUEST,
            "a name is lowercase letters, digits, - _ or . (64 at most)",
        )
            .into_response();
    }
    // guest names are for the fleet's own probes and are dropped after
    // minutes; a person typing one would lose the account. A probe says so.
    if user == pages::DEMO_USER {
        return (
            StatusCode::BAD_REQUEST,
            "that name is the demo's; pick another",
        )
            .into_response();
    }
    if directory::is_guest(&user) && headers.get("x-dd-probe").is_none() {
        return (
            StatusCode::BAD_REQUEST,
            "names starting with guest are reserved; pick another",
        )
            .into_response();
    }
    // the boxes' own identities on the forge (nix/modules/forge/forgejo.nix),
    // which no person is
    if identity::reserved_name(&user) {
        return (
            StatusCode::BAD_REQUEST,
            "that name is the boxes'; pick another",
        )
            .into_response();
    }
    match app.entry(&user) {
        Ok(None) => {}
        Ok(Some(_)) => return (StatusCode::CONFLICT, "that name is taken").into_response(),
        Err(e) => {
            eprintln!("join start: {e:#}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    match app
        .webauthn
        .start_passkey_registration(user_uuid(&user), &user, &user, None)
    {
        Ok((ccr, state)) => {
            let id = app.ceremony_put(Ceremony::Join { user, state });
            with_challenge(ccr, id)
        }
        Err(e) => {
            eprintln!("join start: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn join_finish(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(body): Json<Enrolled>,
) -> Response {
    // An invite code is a secret that makes a member of whoever proves it
    // first, so it is typed only on the person's own device: the app's
    // sign-up, or `dd identity new --code`. A box's page never asks for one.
    if headers.contains_key("x-dd-grant") {
        return (
            StatusCode::BAD_REQUEST,
            "an invite code goes into the Commonty app, which makes your account on your device",
        )
            .into_response();
    }
    let reg = match body.credential() {
        Ok(r) => r,
        Err(r) => return r,
    };
    let Some(Ceremony::Join { user, state }) = headers
        .get("x-dd-ceremony")
        .and_then(|v| v.to_str().ok())
        .and_then(|id| app.ceremony_take(id))
    else {
        return (StatusCode::BAD_REQUEST, "no ceremony").into_response();
    };
    let passkey = match app.webauthn.finish_passkey_registration(&reg, &state) {
        Ok(p) => p,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("passkey rejected: {e}")).into_response();
        }
    };
    let id = B64_URL.encode(passkey.cred_id().as_slice());
    let cred = match serde_json::to_value(&passkey) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("join: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let now = identity::now();
    let root = identity::passkey_root(&id, &cred);
    let entry = identity::Entry {
        name: user,
        root,
        recovery: String::new(),
        recovered_from: vec![],
        devices: vec![],
        passkeys: vec![identity::Passkey {
            id: id.clone(),
            cred,
            added: now,
            library_key: None,
            rp_id: Some(app.domain.clone()),
        }],
        grant: None,
        libraries: vec![],
        // what they agreed to, signed by their passkey with the rest
        terms: Some(identity::TERMS.to_string()),
        removed_boxes: vec![],
        version: 1,
        updated: now,
    };
    let challenge = match identity::challenge(&entry) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("join: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let ceremony = app.ceremony_put(Ceremony::JoinSign {
        entry: Box::new(entry),
    });
    // the get() options, in the shape the browser and webauthn-rs both read
    Json(serde_json::json!({
        "ceremony": ceremony,
        "publicKey": {
            "challenge": B64_URL.encode(challenge),
            "timeout": 60000,
            "rpId": app.domain,
            "allowCredentials": [{ "type": "public-key", "id": id }],
            "userVerification": "preferred",
        }
    }))
    .into_response()
}

async fn join_sign(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(cred): Json<PublicKeyCredential>,
) -> Response {
    let Some(Ceremony::JoinSign { entry }) = headers
        .get("x-dd-ceremony")
        .and_then(|v| v.to_str().ok())
        .and_then(|id| app.ceremony_take(id))
    else {
        return (StatusCode::BAD_REQUEST, "no ceremony").into_response();
    };
    let entry = *entry;
    let assertion = identity::Assertion {
        authenticator_data: B64_URL.encode(&cred.response.authenticator_data),
        client_data_json: B64_URL.encode(&cred.response.client_data_json),
        signature: B64_URL.encode(&cred.response.signature),
    };
    let signature = match serde_json::to_vec(&assertion) {
        Ok(v) => B64.encode(v),
        Err(e) => {
            eprintln!("join: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let user = entry.name.clone();
    let signed = identity::SignedEntry {
        entry,
        signature,
        recovery_signature: None,
    };
    // Named by the member list already, it is an entry and goes to the
    // directory like any other. Otherwise it is held on this box until the
    // list names it - the same sign-in, the same waiting screen for someone
    // not yet in. The accept rule checks the assertion in both.
    let earned = app.earned(&user, &signed.entry.root);
    if earned {
        if let Err((status, why)) = app.directory.admit(signed).await {
            return (status, why).into_response();
        }
        app.directory.unhold(&user);
    } else if let Err((status, why)) = app.directory.hold(&signed) {
        return (status, why).into_response();
    }
    // the entry was just signed with the passkey made a moment ago
    if let Some(r) = app.no_new_session() {
        return r;
    }
    let (set, id, _) = app.sessions.issue_noted(&user);
    app.saw_passkey(&id);
    let mut r = Json(serde_json::json!({ "user": user })).into_response();
    r.headers_mut()
        .insert("set-cookie", HeaderValue::from_str(&set).unwrap());
    r
}

fn pending_key(token: &str) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(token.as_bytes()))
}

#[derive(Deserialize)]
struct LoginStart {
    username: String,
}

async fn login_start(State(app): State<Arc<App>>, Json(q): Json<LoginStart>) -> Response {
    let user = q.username.trim().to_lowercase();
    if !valid_user(&user) {
        return (StatusCode::BAD_REQUEST, "bad username").into_response();
    }
    let passkeys = app.passkeys(&user);
    if passkeys.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            "no passkey in that name's entry - `dd enrol` adds one",
        )
            .into_response();
    }
    match app.webauthn.start_passkey_authentication(&passkeys) {
        Ok((rcr, state)) => {
            let id = app.ceremony_put(Ceremony::Login { user, state });
            with_challenge(rcr, id)
        }
        Err(e) => {
            eprintln!("login start: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn login_finish(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(cred): Json<PublicKeyCredential>,
) -> Response {
    let Some(Ceremony::Login { user, state }) = headers
        .get("x-dd-ceremony")
        .and_then(|v| v.to_str().ok())
        .and_then(|id| app.ceremony_take(id))
    else {
        return (StatusCode::BAD_REQUEST, "no ceremony").into_response();
    };
    // The signature counter is not written back: the credential lives in
    // the signed entry, which this box cannot update. Clone detection by
    // counter is given up for that; the passkey's private key never leaves
    // the authenticator either way.
    if let Err(e) = app.webauthn.finish_passkey_authentication(&cred, &state) {
        return (StatusCode::UNAUTHORIZED, format!("refused: {e}")).into_response();
    }
    let agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    app.signins
        .used(&user, &format!("passkey:{}", cred.id), agent);
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    // shown to confirm something, in a session that is this person's: that
    // session, fresh. With sign-in at the id Worker it is the fleet
    // sign-in's own, which a new one here would split off from
    if app.id_origin.is_some()
        && let Some((was, id, _)) = app.sessions.session(cookie)
        && was == user
        && !id.is_empty()
        && app.session_live(&user, &id)
    {
        app.saw_passkey(&id);
        return StatusCode::OK.into_response();
    }
    // the session this one replaces in this browser is over
    if let Some((was, id, _)) = app.sessions.session(cookie)
        && was == user
        && !id.is_empty()
    {
        app.signins.end(&id);
    }
    if let Some(r) = app.no_new_session() {
        return r;
    }
    let (set, id, exp) = app.sessions.issue_noted(&user);
    app.signins.saw(&user, &id, exp, agent, None);
    // fresh for this session, the one this browser holds from now
    app.saw_passkey(&id);
    let mut r = StatusCode::OK.into_response();
    r.headers_mut()
        .insert("set-cookie", HeaderValue::from_str(&set).unwrap());
    r
}

/// The signed-in front door. A browser without a session goes to the
/// login page and comes back here.
async fn home_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        Some(user) if app.role(&user).is_none() => {
            Html(pages::waiting(&user, &app.home)).into_response()
        }
        Some(_) => page("home"),
        None => Redirect::to("/_dd/login?rd=/").into_response(),
    }
}

/// GET /_dd/demo: straight in for someone with no session (or the demo's
/// own); anyone signed in is asked first, since it signs them out, and a
/// link on another site must not do that to them.
async fn demo_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !app.demo() {
        return (StatusCode::NOT_FOUND, "no demo here").into_response();
    }
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        Some(u) if u != pages::DEMO_USER => Html(pages::confirm(
            "Open the demo",
            "The demo shows what a member sees. Opening it signs you out of your account here; sign back in after.",
            "/_dd/demo",
            "Open the demo",
        ))
        .into_response(),
        _ => demo(&app),
    }
}

/// POST /_dd/demo: the button, from one of our own pages
async fn demo_post(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !from_our_site(&app.domain, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !app.demo() {
        return (StatusCode::NOT_FOUND, "no demo here").into_response();
    }
    demo(&app)
}

/// A browser's POST from one of this fleet's own pages, not another site's:
/// the Origin is one of our names. Every browser sends one on a POST, so a
/// request without it is a program's (`dd`, a test), not a page's.
fn from_our_site(domain: &str, headers: &HeaderMap) -> bool {
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v.as_bytes() == b"cross-site")
    {
        return false;
    }
    match headers.get("origin").map(|v| v.to_str()) {
        None => true,
        Some(Ok(o)) => o
            .strip_prefix("https://")
            .is_some_and(|h| fleet_host(domain, h)),
        Some(Err(_)) => false,
    }
}

/// A look without a key: a session as the demo account, which every box
/// treats as a member with nothing of its own. Each service decides what
/// the demo may do there (nginx, by the username the verifier reports).
fn demo(app: &App) -> Response {
    let mut r = Redirect::to("/_dd/home").into_response();
    r.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&app.sessions.issue(pages::DEMO_USER)).unwrap(),
    );
    r
}

/// The pages' own stylesheet and scripts (pages::static_file).
async fn static_file(
    headers: HeaderMap,
    axum::extract::Path(file): axum::extract::Path<String>,
) -> Response {
    match pages::static_file(&file) {
        Some((body, ty, tag)) => {
            // a font is the same bytes until a release changes its name
            let cache = if ty.starts_with("font/") {
                "public, max-age=604800"
            } else {
                "no-cache"
            };
            tagged(
                &headers,
                ty,
                cache,
                tag,
                axum::body::Bytes::from_static(body),
            )
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A file, or 304 when the browser already holds these bytes: no-cache
/// means ask each time, and the asking is all it costs.
fn tagged(
    headers: &HeaderMap,
    ty: &str,
    cache: &str,
    tag: &str,
    body: axum::body::Bytes,
) -> Response {
    let held = headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        // nginx's gzip makes a tag weak (W/"…") on the way out
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().trim_start_matches("W/") == tag)
        });
    let head = [
        ("content-type", ty.to_string()),
        ("cache-control", cache.to_string()),
        ("etag", tag.to_string()),
    ];
    if held {
        (StatusCode::NOT_MODIFIED, head).into_response()
    } else {
        (head, body).into_response()
    }
}

/// the web bundle's files, read once: a release is a new gate
static WEB: std::sync::LazyLock<std::sync::Mutex<HashMap<String, (axum::body::Bytes, String)>>> =
    std::sync::LazyLock::new(Default::default);

/// The browser-side Rust, as wasm-bindgen laid it out: a .js and a .wasm.
async fn web_file(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    axum::extract::Path(file): axum::extract::Path<String>,
) -> Response {
    let Some(dir) = &app.web_dir else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if file.contains('/') || file.starts_with('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let ty = match file.rsplit('.').next() {
        Some("js") => "application/javascript",
        Some("wasm") => "application/wasm",
        // the PDF editor's faces, and the text reader's model
        Some("ttf") => "font/ttf",
        Some("gz") => "application/gzip",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let known = WEB.lock().unwrap().get(&file).cloned();
    let (b, tag) = match known {
        Some(k) => k,
        None => match std::fs::read(dir.join(&file)) {
            Ok(b) => {
                let tag = pages::tag_of(&b);
                let k = (axum::body::Bytes::from(b), tag);
                WEB.lock().unwrap().insert(file, k.clone());
                k
            }
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    tagged(&headers, ty, "no-cache", &tag, b)
}

/// The one thing every page needs to talk to a passkey: which domain the
/// credentials belong to. Public, and true of the box either way.
async fn page_config(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let mut cfg = serde_json::json!({ "rpId": app.domain.clone() });
    // the signed format this box takes (docs/dev/identity-state.md §1):
    // said only where the control plane judges v2; absent, v1
    if app.directory.takes_v2() {
        cfg["envelope"] = serde_json::json!(2);
    }
    // The demo has no passkey, because a passkey lives in one browser on
    // one device and the demo is one account every visitor shares. Its
    // library key comes from the box instead, the way the demo's photos
    // password already does. Nothing is given away: the library holds
    // nothing private, and anyone at all may be the demo.
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    if app.signed_in(cookie).as_deref() == Some(pages::DEMO_USER)
        && let Some((id, key)) = &app.demo_library
    {
        cfg["demoLibrary"] = serde_json::json!({ "id": id, "key": key });
    }
    Json(cfg).into_response()
}

/// Files: a member's library. Its key is only ever on the member's devices,
/// so a member's browser is handed to the app, which serves the page to it
/// (app/src/local.rs): no box serves a member code that holds a key. The
/// demo's library holds nothing private, and its page is served here.
async fn files_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    library_page(&app, &headers, "files", "/_dd/files")
}

/// Mail: its code holds the member's mail key, so the app serves it, as it
/// does Files; a browser is handed to the app (mail.rs keeps the mail)
async fn mail_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        Some(user)
            if (app.member(&user) || app.suspension(&user).is_some())
                && user != pages::DEMO_USER =>
        {
            use_app()
        }
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None if app_answers(&headers) => use_app(),
        None => Redirect::to("/_dd/login?rd=/_dd/mail").into_response(),
    }
}

/// Movies & TV: the library's Movies and Shows, the same way as Files.
async fn media_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    library_page(&app, &headers, "media", "/_dd/media")
}

/// Calendar: the library's Calendars, the same way as Files.
async fn calendar_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    library_page(&app, &headers, "calendar", "/_dd/calendar")
}

/// Contacts: the library's Contacts, the same way as Files.
async fn contacts_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    library_page(&app, &headers, "contacts", "/_dd/contacts")
}

fn library_page(app: &App, headers: &HeaderMap, name: &str, at: &str) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        // the demo reads the box's one library for it and nothing else, and
        // the page hides every control a reader has no use for
        Some(user) if user == pages::DEMO_USER && app.demo_library.is_some() => page(name),
        // a suspended member too, to take their files: the app reads them,
        // and the gate lets them read and nothing more (library.rs)
        Some(user)
            if (app.member(&user) || app.suspension(&user).is_some())
                && user != pages::DEMO_USER =>
        {
            use_app()
        }
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None if app_answers(headers) => use_app(),
        None => Redirect::to(&format!("/_dd/login?rd={at}")).into_response(),
    }
}

/// whether the member's app has taken a link from this browser before: the
/// note use-app.js leaves once it has. It lets nothing in; a signed-out
/// browser with it is handed to the app, which signs in for itself.
fn app_answers(headers: &HeaderMap) -> bool {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .any(|c| c.trim() == "dd_app=1")
}

/// The page that hands a browser to the member's app (use-app.js): for a
/// page whose code would hold a key, which the app serves instead.
fn use_app() -> Response {
    (
        [("cache-control", "no-cache")],
        Html(pages::page("use-app").unwrap_or_default()),
    )
        .into_response()
}

/// A signed-in page, as the file it is (pages::page): it fills itself in
/// from /_dd/me, in a browser and in the app alike.
pub(crate) fn page(name: &str) -> Response {
    match pages::page(name) {
        Some(html) => ([("cache-control", "no-cache")], Html(html)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The docs' index: the nav, and every page's title, headings and words.
/// Anyone may read it, as anyone may read the docs.
async fn docs_index(headers: HeaderMap) -> Response {
    let (index, tag) = docs::index();
    tagged(
        &headers,
        "application/json",
        "no-cache",
        tag,
        axum::body::Bytes::from(index.to_string()),
    )
}

/// One page of the docs, as the markdown it is in the repository
async fn docs_raw(
    headers: HeaderMap,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    match docs::raw(&path) {
        Some(b) => tagged(
            &headers,
            "text/markdown; charset=utf-8",
            "no-cache",
            &pages::tag_of(b),
            axum::body::Bytes::from_static(b),
        ),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Who is looking, their menu and their services: what every signed-in
/// page fills itself from. A browser's session or a device's token.
async fn me(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.role(&user) {
        Some(role) => {
            let mut v = pages::me_json(&user, &app.home, role, &app.domain);
            if role == pages::Role::Guest {
                v["guestOf"] = app.friends.guest(&user).map(|g| g.by).into();
            }
            // why, and until when they may take their things
            if let Some((why, until)) = app.suspension(&user) {
                v["suspended"] = serde_json::json!({ "reason": why, "until": until });
            }
            if let Some(k) = &app.tmdb
                && role != pages::Role::Guest
            {
                v["tmdb"] = k.as_str().into();
            }
            // the household's switch for the house network, above sign-out
            if let Some(ab) = &app.adblock
                && role == pages::Role::Member
                && ab.allows(&user)
                && let Some(menu) = v["menu"].as_array_mut()
            {
                let at = menu.len().saturating_sub(1);
                menu.insert(
                    at,
                    serde_json::json!([{
                        "label": "Ad blocking at home",
                        "url": "/_dd/adblock",
                        "toggle": "/_dd/adblock/state?brief",
                        "switch": "/_dd/adblock/switch",
                    }]),
                );
            }
            // who you are, never kept by the browser: sign-out leaves the
            // http cache alone (photos::forget)
            ([("cache-control", "no-store")], Json(v)).into_response()
        }
        None => StatusCode::FORBIDDEN.into_response(),
    }
}

/// Every place a member can open, for the assistant on each page and the
/// docs' drawing of it (sitemap.rs): members and the demo, each their own.
async fn site_map(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.role(&user) {
        Some(role @ (pages::Role::Member | pages::Role::Demo)) => (
            [("cache-control", "private, no-cache")],
            Json(sitemap::build(
                &app.home,
                role,
                &app.domain,
                docs::index().0,
            )),
        )
            .into_response(),
        _ => StatusCode::FORBIDDEN.into_response(),
    }
}

/// What is new, for the assistant on every page (releases.rs): a public
/// repository's release notes, so for anyone. A box told of no forge has
/// none to list.
async fn release_notes(State(app): State<Arc<App>>) -> Response {
    let Some(notes) = &app.releases else {
        return (
            [("cache-control", "no-cache")],
            Json(serde_json::json!({ "releases": [] })),
        )
            .into_response();
    };
    match notes.get().await {
        Some(v) => ([("cache-control", "no-cache")], Json(v)).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "the release notes could not be read just now",
        )
            .into_response(),
    }
}

/// What Monty answers a question about Monty Python from (joke.rs): for
/// members and the demo, like the site map
async fn joke_lookup(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !matches!(
        app.role(&user),
        Some(pages::Role::Member | pages::Role::Demo)
    ) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let words: String = q
        .get("q")
        .map(String::as_str)
        .unwrap_or("")
        .chars()
        .take(300)
        .collect();
    (
        [("cache-control", "private, no-cache")],
        Json(joke::answer(&words)),
    )
        .into_response()
}

/// Chat (servers/chat): the demo's, with its counted prompts and nothing
/// kept, is served here. A member's keeps their chats sealed in their
/// library, so the member's app serves it to their browser (use_app).
async fn chat_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        Some(user) if app.role(&user) == Some(pages::Role::Demo) => page("chat"),
        Some(user) if app.role(&user) == Some(pages::Role::Member) => use_app(),
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None if app_answers(&headers) => use_app(),
        None => Redirect::to("/_dd/login?rd=/").into_response(),
    }
}

/// Git (servers/forge): our pages over the forge, for anyone signed in. The
/// forge's API decides what each of them may see and do; the page is the
/// same file for all.
async fn git_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    signed_in_page(&app, &headers, "git")
}

/// Metrics (servers/observe): the fleet's figures, for members; the queries
/// behind it pass the same gate. Not the demo's, whatever a tile says.
async fn metrics_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        // the page is anyone's who owns a box - one of the release's, or
        // one they joined and enlisted - or the admin's; what it then shows
        // is each box's to say (metrics_sources)
        Some(u)
            if u == pages::DEMO_USER
                || !app.member(&u)
                || (!app.sees_box(&u)
                    && app.boxes_seen(&u).is_empty()
                    && !owns_joined(&app, &u).await) =>
        {
            StatusCode::FORBIDDEN.into_response()
        }
        _ => signed_in_page(&app, &headers, "metrics"),
    }
}

/// a box this person joined and enlisted: theirs, its metrics theirs
async fn owns_joined(app: &Arc<App>, user: &str) -> bool {
    member_boxes::current(app)
        .await
        .1
        .iter()
        .any(|o| o.user == user)
}

fn signed_in_page(app: &App, headers: &HeaderMap, name: &str) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        Some(_) => page(name),
        None => {
            let at = headers
                .get("x-original-uri")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("/");
            let rd: String = url::form_urlencoded::byte_serialize(at.as_bytes()).collect();
            Redirect::to(&format!("/_dd/login?rd={rd}")).into_response()
        }
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

/// Chat's web search. The model has no network at all; the page asks
/// here, gets titles, links and a line of each, and hands them to the model
/// with the question. Members only: the demo would make this box a search
/// proxy for anyone.
async fn chat_search(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    axum::extract::Query(sq): axum::extract::Query<SearchQuery>,
) -> Response {
    // a browser's session, or the device's token Chat comes with when the
    // app serves it (app/src/local.rs)
    match app.identify(&headers, "access") {
        Some(user) if app.role(&user) == Some(pages::Role::Member) => {}
        Some(_) => return StatusCode::FORBIDDEN.into_response(),
        None => return StatusCode::UNAUTHORIZED.into_response(),
    }
    let Some(base) = &app.search else {
        return (StatusCode::NOT_FOUND, "web search is not on this box").into_response();
    };
    let q = sq.q.trim();
    if q.is_empty() || q.chars().count() > 300 {
        return (
            StatusCode::BAD_REQUEST,
            "ask for something, in under 300 characters",
        )
            .into_response();
    }
    let got = async {
        let v: serde_json::Value = http()
            .get(format!("{base}/search"))
            .timeout(Duration::from_secs(12))
            .query(&[("q", q), ("format", "json"), ("safesearch", "1")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        anyhow::Ok(v)
    }
    .await;
    let v = match got {
        Ok(v) => v,
        Err(e) => {
            // never the error itself: its URL carries what was searched for
            let status = e.downcast_ref::<reqwest::Error>().and_then(|r| r.status());
            eprintln!("verify: search did not answer ({status:?})");
            return (StatusCode::BAD_GATEWAY, "the search did not answer").into_response();
        }
    };
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let results: Vec<serde_json::Value> = v["results"]
        .as_array()
        .map(|r| {
            r.iter()
                .filter(|x| {
                    x["url"]
                        .as_str()
                        .is_some_and(|u| u.starts_with("https://") || u.starts_with("http://"))
                })
                .take(6)
                .map(|x| {
                    serde_json::json!({
                        "title": clip(x["title"].as_str().unwrap_or(""), 200),
                        "url": clip(x["url"].as_str().unwrap_or(""), 500),
                        "content": clip(x["content"].as_str().unwrap_or(""), 400),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (
        [("cache-control", "no-store")],
        Json(serde_json::json!({ "results": results })),
    )
        .into_response()
}

/// Boxes, Backups, Devices, Network: the pages behind the bar's menu.
/// Each is a member's own view of the fleet; the demo gets none of them.
async fn member_page(app: &App, headers: &HeaderMap, name: &str, at: &str) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.signed_in(cookie) {
        Some(user) if app.member(&user) && user != pages::DEMO_USER => page(name),
        // a guest has devices and joins the network like anyone, to reach
        // the game servers they are invited to: Settings shows them their
        // Profile and Devices
        Some(user) if name == "settings" && app.guest(&user) => page(name),
        // someone who left sees where they stand
        Some(user) if app.directory.has_left(&user) => {
            if name == "settings" {
                page(name)
            } else {
                Redirect::to("/_dd/settings#leave").into_response()
            }
        }
        // an account still waiting may leave too: Settings shows it Leave alone
        Some(user)
            if name == "settings"
                && user != pages::DEMO_USER
                && app.entry(&user).ok().flatten().is_some() =>
        {
            page(name)
        }
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None => Redirect::to(&format!("/_dd/login?rd={at}")).into_response(),
    }
}

/// What every box is running and how its last backup went. Read from each
/// box's own prometheus when the page asks, never stored here.
async fn fleet_json(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match app.identify(&headers, "access") {
        Some(user) if app.member(&user) && user != pages::DEMO_USER => {
            Json(fleet::look(&app.fleet, app.thanos.as_deref(), app.control.as_deref()).await)
                .into_response()
        }
        _ => StatusCode::FORBIDDEN.into_response(),
    }
}

/// GET /_dd/status.json: whether each service works, now and each of the
/// last 90 days. Anyone may ask: it names services, never a box, and says
/// nothing of releases.
async fn status_json(State(app): State<Arc<App>>) -> Response {
    match app
        .status
        .status(app.thanos.as_deref(), &app.service_boxes)
        .await
    {
        Some(s) => ([("cache-control", "public, max-age=30")], Json(s)).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            [("cache-control", "no-store")],
            Json(serde_json::json!({ "services": [] })),
        )
            .into_response(),
    }
}

/// POST /_dd/csp: what a page's content policy would have blocked, as the
/// browser reports it. Logged, one line each, for whoever tunes the policy;
/// anyone may send one, so it is read small and never answered with more
/// than a status.
async fn csp_report(body: axum::body::Bytes) -> StatusCode {
    let v: serde_json::Value = match serde_json::from_slice(&body[..body.len().min(8192)]) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST,
    };
    let r = &v["csp-report"];
    let field = |k: &str| {
        r[k].as_str()
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .take(160)
            .collect::<String>()
    };
    // the page's path only: a query string can carry anything
    let page = field("document-uri");
    let page = page.split(['?', '#']).next().unwrap_or_default();
    eprintln!(
        "csp: {} blocked {} on {page}",
        field("violated-directive"),
        field("blocked-uri")
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
    );
    StatusCode::NO_CONTENT
}

/// the most a player error report is read to
const PLAYER_ERROR_BODY: usize = 4096;
/// and the most lines they make a minute, whoever sends them
const PLAYER_ERRORS_A_MINUTE: u32 = 30;
static PLAYER_ERRORS: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

/// POST /_dd/player-error: Movies & TV's player stopped, or kept failing to
/// buffer. One line in the gate's log, so the next "it stopped on my phone"
/// has a record: the player's own error, the browser, and the kind of film
/// (size, codecs, picture size, the last segment's bytes) - never its name,
/// its library or a path, which are the member's. Anyone may send one, so
/// each field is cut to a word or a number and the lines are rate limited.
async fn player_error(headers: HeaderMap, body: axum::body::Bytes) -> StatusCode {
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let ua = headers
        .get("user-agent")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let line = match player_line(host, ua, &body) {
        Ok(l) => l,
        Err(code) => return code,
    };
    {
        let mut seen = PLAYER_ERRORS.lock().unwrap_or_else(|e| e.into_inner());
        let (since, n) = seen.get_or_insert((Instant::now(), 0));
        if since.elapsed() > Duration::from_secs(60) {
            (*since, *n) = (Instant::now(), 0);
        }
        *n += 1;
        if *n > PLAYER_ERRORS_A_MINUTE {
            return StatusCode::TOO_MANY_REQUESTS;
        }
    }
    eprintln!("{line}");
    StatusCode::NO_CONTENT
}

/// The log line for a player error report, from only the fields it knows,
/// each cut to a word or a number.
fn player_line(host: &str, ua: &str, body: &[u8]) -> std::result::Result<String, StatusCode> {
    if body.len() > PLAYER_ERROR_BODY {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    // A field is kept only in the shape it has from the player, else "-":
    // a filtered word could still be a film's title.
    let shaped = |k: &str, ok: &dyn Fn(&str) -> bool| -> String {
        match v[k].as_str() {
            Some(s) if s.len() <= 40 && ok(s) => s.to_string(),
            _ => "-".into(),
        }
    };
    // hls.js's names and the browser's: camelCase ending in what they end in
    let named = |s: &str, ends: &[&str]| {
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphabetic())
            && ends.iter().any(|e| s.ends_with(e))
    };
    // a codec string: "avc1.640028", "mp4a.40.2", "ac-3"
    let codec = |s: &str| {
        let mut parts = s.split('.');
        let head = parts.next().unwrap_or_default();
        (2..=5).contains(&head.len())
            && head
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && parts.clone().count() <= 4
            && parts
                .all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
    };
    let num = |k: &str| -> String {
        v[k].as_f64()
            .filter(|n| n.is_finite() && (0.0..1e13).contains(n))
            .map_or("-".into(), |n| format!("{}", (n * 10.0).round() / 10.0))
    };
    let mb = |k: &str| -> String {
        v[k].as_f64()
            .filter(|n| n.is_finite() && (0.0..1e13).contains(n))
            .map_or("-".into(), |n| format!("{:.1}MB", n / 1e6))
    };
    // the kind of file from its ending, and only a kind we know
    let container = shaped("container", &|s| {
        matches!(
            s,
            "mkv"
                | "mp4"
                | "m4v"
                | "mov"
                | "avi"
                | "webm"
                | "ts"
                | "mpg"
                | "mpeg"
                | "wmv"
                | "flv"
                | "ogv"
        )
    });
    // the box's name as the browser asked for it, and nothing after
    let host: String = host
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        .take(80)
        .collect();
    Ok(format!(
        "player-error: {} {} {} {}/{} code={} fatal={} recovered={} {}; film {} {} {}/{} {}x{}; segment {} {}s, buffered {}s",
        if host.is_empty() { "-" } else { &host },
        ua_family(ua),
        shaped("kind", &|s| matches!(s, "hls" | "video")),
        shaped("type", &|s| named(s, &["Error"])),
        shaped("details", &|s| {
            named(s, &["Error", "TimeOut", "Timeout", "Exception", "Gap"])
        }),
        num("code"),
        v["fatal"].as_bool().unwrap_or(false),
        v["recovered"].as_bool().unwrap_or(false),
        shaped("error", &|s| named(s, &["Error"])),
        mb("size"),
        container,
        shaped("video", &codec),
        shaped("audio", &codec),
        num("width"),
        num("height"),
        mb("piece"),
        num("pieceSecs"),
        num("buffered"),
    ))
}

/// a browser and its system from a user agent, and nothing finer:
/// "Chrome/154 Android"
fn ua_family(ua: &str) -> String {
    let major = |name: &str| -> Option<String> {
        let at = ua.find(name)? + name.len();
        let v: String = ua[at..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        Some(if v.is_empty() {
            name.trim_end_matches('/').into()
        } else {
            format!("{name}{v}")
        })
    };
    let browser = major("Edg/")
        .or_else(|| major("OPR/"))
        .or_else(|| major("SamsungBrowser/"))
        .or_else(|| major("Firefox/"))
        .or_else(|| major("Chrome/"))
        .or_else(|| {
            ua.contains("Safari/")
                .then(|| major("Version/").map(|v| v.replace("Version/", "Safari/")))
                .flatten()
        })
        .unwrap_or_else(|| "other".into());
    let system = if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Mac OS X") {
        "Mac"
    } else if ua.contains("CrOS") {
        "ChromeOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "other"
    };
    format!("{browser} {system}")
}

/// Where a stranger gets the app. The only page here that asks for
/// nothing: an invited person has no way in until they have it.
async fn download_page(State(app): State<Arc<App>>) -> Response {
    let Some(url) = app.app_manifest.clone() else {
        return (StatusCode::NOT_FOUND, "nothing to download yet").into_response();
    };
    match app.signed_app().await {
        Some(a) => Html(pages::download(
            &app.domain,
            &a.from,
            &a.commit,
            &a.tag,
            &url,
            &a.files,
        ))
        .into_response(),
        None => (StatusCode::NOT_FOUND, "nothing signed to download yet").into_response(),
    }
}

/// Signing out: the session goes, and so does what the photo app keeps in
/// this browser - its list of the library and the pictures it unlocked. That
/// belongs to the photos host, so the way out passes through there
/// (photos_forget), and on to the front door: signing in again lands on the
/// home page, not on whichever service the person signed out from.
/// It is a POST from our own pages; a link to it (GET) is a page with the
/// button, so another site cannot sign anyone out.
async fn logout(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !from_our_site(&app.domain, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let home = format!("home.{}", app.domain);
    let back = fleet_host(&app.domain, &home).then(|| format!("https://{home}/"));
    // the demo's photos are the gate's own page, never the photo app, and
    // the photos host is on the private network where the demo cannot go
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let demo = app.signed_in(cookie).as_deref() == Some(pages::DEMO_USER);
    // over here, and not only in this browser's cookie jar
    if let Some((_, id, _)) = app.sessions.session(cookie)
        && !id.is_empty()
    {
        app.signins.end(&id);
    }
    let mut to = match (photos::origin(&app.home), back) {
        (_, Some(b)) if demo => b,
        (Some(p), Some(b)) => format!("{p}/_dd/photos/forget?then={b}"),
        _ => "/".to_string(),
    };
    // and the browser's key at the sign-in page, or the next name it opens
    // would sign it straight back in
    if let Some(id) = &app.id_origin {
        let then = if to.starts_with('/') {
            format!("https://{}{to}", asked_host(&headers))
        } else {
            to
        };
        to = format!("{id}/out?then={}", urlencode(&then));
    }
    let mut r = Redirect::to(&to).into_response();
    for c in app.sessions.clear_all() {
        r.headers_mut()
            .append("set-cookie", HeaderValue::from_str(&c).unwrap());
    }
    r
}

/// one of this fleet's own names, and nothing that could carry more
/// GET /_dd/metrics/sources: where the Metrics page asks, for this person.
/// `here`: this box's Thanos (every box's), for the admin and this box's
/// owner; `boxes`: each box they see that answers for itself, live.
async fn metrics_sources(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if user == pages::DEMO_USER || !app.member(&user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut boxes: Vec<serde_json::Value> = app
        .boxes_seen(&user)
        .into_iter()
        .filter(|(_, b)| !b.metrics.is_empty() && fleet_host(&app.domain, &b.metrics))
        .map(
            |(name, b)| serde_json::json!({ "box": name, "url": format!("https://{}", b.metrics) }),
        )
        .collect();
    // the boxes this person set up, on the box that gathers their metrics
    // (observe/joined-metrics.nix): theirs, and every one for the admin
    if app.thanos.is_some() {
        let metrics = format!("metrics.{}", app.domain);
        // the admin sees every joined box, a member their own
        let (st, owned) = member_boxes::current(&app).await;
        let names: Vec<String> = if app.is_admin(&user) {
            st["joined"]
                .as_object()
                .map(|j| j.keys().cloned().collect())
                .unwrap_or_default()
        } else {
            owned
                .into_iter()
                .filter(|o| o.user == user)
                .map(|o| o.name)
                .collect()
        };
        boxes.extend(names.into_iter().map(
            |n| serde_json::json!({ "box": n, "url": format!("https://{metrics}/joined/{n}") }),
        ));
    }
    let mut r =
        Json(serde_json::json!({ "here": app.sees_box(&user), "boxes": boxes })).into_response();
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r
}

/// metrics.<domain>, or a box's own metrics.<box>.<domain>
fn is_metrics_host(domain: &str, host: &str) -> bool {
    host.strip_suffix(&format!(".{domain}"))
        .is_some_and(|sub| sub == "metrics" || sub.starts_with("metrics."))
}

/// GET /_dd/back?to=: back to the page that sent someone through this box's
/// own sign-in - Metrics, which goes through each box once, or a page whose
/// assistant needed Chat's (guide.js). Only those pages' names: https, no
/// port, no credentials in the address - anything else is this box's front
/// page.
async fn back(State(app): State<Arc<App>>, Query(q): Query<HashMap<String, String>>) -> Response {
    let to = q.get("to").map(String::as_str).unwrap_or_default();
    match back_to(&app.domain, to) {
        Some(u) => Redirect::to(u.as_str()).into_response(),
        None => Redirect::to("/").into_response(),
    }
}

fn back_to(domain: &str, to: &str) -> Option<Url> {
    let pages = ["metrics", "home", "files", "git", "games"].map(|h| format!("{h}.{domain}"));
    Url::parse(to).ok().filter(|u| {
        u.scheme() == "https"
            && u.port().is_none()
            && u.username().is_empty()
            && u.password().is_none()
            && u.host_str()
                .is_some_and(|h| h == domain || pages.iter().any(|p| p == h))
    })
}

/// auth_request on a box that serves its own pages alone: its owner and
/// the admin, and nobody else, whatever else they are
async fn verify_box(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !app.sees_box(&user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut r = StatusCode::OK.into_response();
    let v = HeaderValue::from_str(&user).unwrap();
    r.headers_mut().insert("X-Auth-Request-User", v);
    r
}

pub(crate) fn fleet_host(domain: &str, host: &str) -> bool {
    host.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && (host == domain || host.ends_with(&format!(".{domain}")))
}

#[derive(Deserialize)]
struct Authorize {
    client_id: String,
    redirect_uri: String,
    state: Option<String>,
    nonce: Option<String>,
    response_type: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

/// Passwords sends a person here to sign in: a member with a session gets
/// a code for the one client this issuer knows; anyone else signs in with
/// a passkey first, and anyone not a member gets nothing.
async fn oidc_authorize(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<Authorize>,
) -> Response {
    let Some(issuer) = app.oidc.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if q.client_id != issuer.client_id
        || q.redirect_uri != issuer.redirect_uri
        || q.response_type.as_deref() != Some("code")
        || q.code_challenge.is_some() && q.code_challenge_method.as_deref() != Some("S256")
    {
        return (StatusCode::BAD_REQUEST, "unknown client or redirect").into_response();
    }
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let Some(user) = app.signed_in(cookie) else {
        // no session here yet: passkey first, then back to this exact url
        let here = headers
            .get("x-original-uri")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("/")
            .to_string();
        return Redirect::to(&format!("/_dd/login?rd={}", urlencode(&here))).into_response();
    };
    // members only: not the demo, not a guest
    if !app.member(&user) || user == pages::DEMO_USER {
        return Redirect::to("/_dd/home").into_response();
    }
    if let Some(r) = app.no_new_session() {
        return r;
    }
    let code = issuer.code(&user, q.nonce, q.code_challenge);
    let mut to = format!("{}?code={}", q.redirect_uri, urlencode(&code));
    if let Some(st) = q.state {
        to.push_str(&format!("&state={}", urlencode(&st)));
    }
    Redirect::to(&to).into_response()
}

#[derive(Deserialize)]
struct TokenReq {
    grant_type: String,
    code: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    code_verifier: Option<String>,
}

async fn oidc_token(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Form(f): Form<TokenReq>,
) -> Response {
    let Some(issuer) = app.oidc.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // client_secret_basic or _post, whichever the client picks
    let (id, secret) = match headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| B64.decode(b).ok())
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| {
            s.split_once(':')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        }) {
        Some(p) => p,
        None => (
            f.client_id.unwrap_or_default(),
            f.client_secret.unwrap_or_default(),
        ),
    };
    if f.grant_type != "authorization_code" || !issuer.client_ok(&id, &secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid_client"})),
        )
            .into_response();
    }
    if let Some(r) = app.no_new_session() {
        return r;
    }
    match issuer.redeem(&f.code, f.code_verifier.as_deref()) {
        Some(v) => Json(v).into_response(),
        None => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant"})),
        )
            .into_response(),
    }
}

async fn oidc_userinfo(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(issuer) = app.oidc.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match bearer(&headers).and_then(|t| issuer.userinfo(t)) {
        Some(v) => Json(v).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn oidc_discovery(State(app): State<Arc<App>>) -> Response {
    match app.oidc.as_ref() {
        Some(i) => Json(i.discovery()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn oidc_jwks(State(app): State<Arc<App>>) -> Response {
    match app.oidc.as_ref() {
        Some(i) => Json(i.jwks()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Bind, then serve forever. Returns once bound with the address (so a
/// test can bind port 0 and learn the port) and the task serving it.
pub async fn start(
    cfg: Config,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<Result<()>>)> {
    std::fs::create_dir_all(&cfg.dir)?;
    let release = match &cfg.release_pub {
        Some(k) => Some(identity::decode_public(k).map_err(|e| anyhow!("release key: {e}"))?),
        None => None,
    };
    let mut directory = directory::Directory::open(cfg.dir.clone(), release)?
        .with_members(cfg.members.clone())
        .with_member_list(cfg.member_list.clone())
        .with_readers(cfg.directory_readers.clone())
        .with_names(box_names(&cfg.names, cfg.domain.as_deref()));
    if let Some((control, submit, config)) = &cfg.control {
        // the boxes that vote: every one of their gates acknowledges a new
        // name before it is anyone's (the witness has no gate and no say)
        #[derive(serde::Deserialize)]
        struct Control {
            boxes: std::collections::BTreeMap<String, serde_json::Value>,
            validators: Vec<Named>,
        }
        #[derive(serde::Deserialize)]
        struct Named {
            name: String,
        }
        let c: Control = serde_json::from_slice(&std::fs::read(config)?)
            .with_context(|| format!("{}", config.display()))?;
        let voters: Vec<String> = c
            .validators
            .into_iter()
            .map(|v| v.name)
            .filter(|n| c.boxes.contains_key(n))
            .collect();
        anyhow::ensure!(!voters.is_empty(), "no voting box in {}", config.display());
        // this box: its hostname; a test running several gates names each
        let me = match std::env::var("VERIFY_BOX") {
            Ok(b) if !b.is_empty() => b,
            _ => std::fs::read_to_string("/proc/sys/kernel/hostname")?
                .trim()
                .to_string(),
        };
        eprintln!(
            "directory: the agreed log, acknowledged by {}",
            voters.join(", ")
        );
        directory = directory.agreed(control.clone(), submit.clone(), voters, me)?;
    }
    anyhow::ensure!(
        cfg.identity_from == ident::From::Replay || cfg.control.is_some(),
        "VERIFY_IDENTITY_FROM={:?} reads the control plane, and VERIFY_CONTROL is not set",
        cfg.identity_from
    );
    directory = directory.identity_from(
        cfg.identity_from,
        cfg.control.as_ref().map(|(c, _, _)| c.as_str()),
    );
    let directory = Arc::new(directory);
    if cfg.control.is_some() {
        tokio::spawn(directory.clone().follow());
    }
    if let Some(a) = &directory.ident {
        eprintln!("identity: {:?}, from the agreed state", cfg.identity_from);
        tokio::spawn(a.clone().follow());
    }
    if let Some(c) = &directory.computers {
        tokio::spawn(c.clone().follow());
    }
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    let addr = listener.local_addr()?;
    // a box with nothing else on it: no domain, no sessions, no secrets. It
    // serves entries and accepts the ones that verify, and that is all.
    let Some(domain) = cfg.domain else {
        let house = Arc::new(home::DirHouse {
            home: home::Home::new(
                std::env::var("VERIFY_HOUSE")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
                &[],
            ),
            directory: directory.clone(),
            members: cfg.members.clone(),
        });
        let leave = Arc::new(leaving::Ctx {
            d: directory.clone(),
            m: cfg.members.clone(),
            demo: None,
        });
        let router = Router::new()
            .route("/health", get(|| async { "ok" }))
            .merge(home::dir_router(house))
            .merge(leaving::router(leave.clone()))
            .merge(
                leaving::internal_router(leave)
                    .route_layer(axum::middleware::from_fn(this_box_only)),
            )
            .merge(directory::router(directory));
        eprintln!("directory listening on {addr}");
        let task =
            tokio::spawn(async move { axum::serve(listener, router).await.map_err(Into::into) });
        return Ok((addr, task));
    };
    let state_dir = cfg
        .dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cfg.dir.clone());
    // the browser login's scope: the whole domain, so one passkey login covers
    // every service on this box, and the cookie rides along to all of them.
    // But only this box's own pages may ask for it (VERIFY_HOSTS): a page of
    // any other host of the domain could be another box, relaying this
    // box's challenge to get a session here.
    let rp_origin = Url::parse(&format!("https://{domain}"))?;
    let mut webauthn = WebauthnBuilder::new(&domain, &rp_origin)?.rp_name("Commonty");
    for h in own_hosts(&domain) {
        webauthn = webauthn.append_allowed_origin(&Url::parse(&h)?);
    }
    let webauthn = webauthn.build()?;
    let oidc = match cfg.oidc {
        Some(o) => Some(oidc::Issuer::open(
            &state_dir,
            o.issuer,
            o.client_id,
            o.client_secret,
            o.redirect,
            domain.clone(),
        )?),
        None => None,
    };
    let leave = Arc::new(leaving::Ctx {
        d: directory.clone(),
        m: cfg.members.clone(),
        demo: cfg.demo_library.as_ref().map(|(id, _)| id.clone()),
    });
    let app = Arc::new(App {
        directory: directory.clone(),
        // with sign-in at the id Worker, a cookie for each name alone: one
        // sent to every name of the domain would reach every box's
        sessions: session::Sessions::open(&state_dir, &domain, cfg.id_origin.is_some())?,
        signins: signins::SignIns::open(Some(state_dir.join("signins.json"))),
        state_dir: state_dir.clone(),
        adding: Default::default(),
        home_net: home::Home::new(
            std::env::var("VERIFY_HOUSE")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            &cfg.house_peers,
        ),
        webauthn,
        ceremonies: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        oidc,
        home: cfg.home,
        members: cfg.members,
        domain: domain.clone(),
        web_dir: cfg.web_dir,
        photos: cfg.photos,
        demo_rate: Mutex::new(HashMap::new()),
        handoffs: Mutex::new(HashMap::new()),
        id_origin: cfg.id_origin.clone(),
        owner: cfg.owner.clone(),
        admin: cfg.admin.clone(),
        boxes: cfg.boxes.clone(),
        sso: Mutex::new(HashMap::new()),
        library: cfg.library,
        mail: cfg.mail,
        library_owners: Mutex::new(HashMap::new()),
        network: cfg.network,
        demo_library: cfg.demo_library,
        app_manifest: cfg.app_manifest,
        tmdb: cfg.tmdb,
        search: cfg.search,
        app_seen: Mutex::new(None),
        releases: cfg
            .member_list
            .as_deref()
            .and_then(releases::Notes::from_member_list),
        fleet: cfg.fleet,
        thanos: cfg.thanos,
        service_boxes: cfg.service_boxes,
        status: Default::default(),
        control: cfg.control.as_ref().map(|(c, _, _)| c.clone()),
        adblock: cfg.adblock,
        forge_events: cfg.forge_events.map(forge_events::ForgeEvents::start),
        demo_photos: Default::default(),
        storage: storage::Ledger::new(cfg.storage_ledger.clone()),
        fresh: Mutex::new(HashMap::new()),
        member_boxes: Mutex::new(member_boxes::Cache::default()),
        reap_brake: Arc::new(network::Brake::from_env(&state_dir)),
        friends: friends::Store::open(&state_dir)?,
        controllers: controller::Store::open(Some(state_dir.join("controller-backups.json"))),
    });
    // members' locked mail, taken from the mail Worker as it comes (mail.rs)
    if app.mail.is_some() {
        tokio::spawn(mail::run(app.clone()));
    }
    // the directory's reads ask who this is the way every route does
    let weak = Arc::downgrade(&app);
    directory.set_caller(Box::new(move |h| {
        let op = bearer(h).map(read_operation).unwrap_or("access");
        weak.upgrade()?.identify(h, op)
    }));
    // A held sign-up follows through when the member list names it: then it
    // is published like any entry, and the hold goes. Reading each one also
    // drops those past their time.
    {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                for name in a.directory.held_names() {
                    let Some(h) = a.directory.held(&name) else {
                        continue;
                    };
                    if !a.earned(&name, &h.entry.root) {
                        continue;
                    }
                    match a.directory.admit(h).await {
                        Ok(()) => a.directory.unhold(&name),
                        Err((_, why)) => eprintln!("directory: publishing held {name}: {why}"),
                    }
                }
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }
    // Devices of people who are no longer members come off the network:
    // only on what is known, never while who is a member is not (reap_guard)
    if app.network.is_some() {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                a.reap_network().await;
                tokio::time::sleep(network::reap_every()).await;
            }
        });
    }
    // the demo's Photos are listed at start and kept fresh here, so no
    // visitor ever waits on its sign-in
    if app
        .photos
        .as_ref()
        .is_some_and(|p| p.demo_password.is_some())
    {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                if let Ok((g, all)) = a.demo_photos.refresh(&a).await {
                    a.demo_photos.warm(g, all);
                }
                tokio::time::sleep(demo_photos::FRESH).await;
            }
        });
    }
    // someone who left: out at once, and their things gone after the window
    {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                leaving::tick(&a).await;
                tokio::time::sleep(leaving::tick_every()).await;
            }
        });
    }
    // what was trashed 30 days ago is gone for good; daily
    if app.library.is_some() {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                // a bucket not up yet (at boot) is tried again within the hour
                let mut next = 24 * 3600;
                if let Some(g) = &a.library {
                    match g.purge_trash(identity::now()).await {
                        Ok(0) => {}
                        Ok(n) => eprintln!("library: {n} object(s) out of the trash for good"),
                        Err(e) => {
                            eprintln!("library: emptying the trash: {e:#}");
                            next = 3600;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(next)).await;
            }
        });
        // a member's box's old disk, backed up before it was set up, waits
        // a week for the box to join; then it goes. Only on a read of the
        // agreed state: with none, nothing can say which boxes joined
        let a = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(600)).await;
            loop {
                if let (Some(g), Some((joined, now))) =
                    (&a.library, member_boxes::joined_tickets(&a).await)
                {
                    match g.pending().await {
                        Ok(p) => {
                            let first = p.iter().map(|(t, (_, at))| (t.clone(), *at)).collect();
                            for t in member_boxes::expired(&first, &joined, now) {
                                match g.drop_pending(&p[&t].0).await {
                                    Ok(n) => eprintln!(
                                        "library: a pending backup of {n} object(s) whose box never joined, to the images' trash"
                                    ),
                                    Err(e) => {
                                        eprintln!("library: dropping a pending backup: {e:#}")
                                    }
                                }
                            }
                        }
                        Err(e) => eprintln!("library: listing pending backups: {e:#}"),
                    }
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
    }
    if cfg.box_only {
        let house = Arc::new(home::DirHouse {
            home: home::Home::new(
                std::env::var("VERIFY_HOUSE")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
                &[],
            ),
            directory: directory.clone(),
            members: app.members.clone(),
        });
        let router = Router::new()
            .route("/verify", get(verify_box))
            .route("/health", get(|| async { "ok" }))
            .route(
                "/_dd/login",
                get(|State(a): State<Arc<App>>, Query(q): Query<HashMap<String, String>>| async move {
                    let rd = q.get("rd").map(String::as_str).unwrap_or("/");
                    match a.id_origin {
                        Some(_) => Redirect::to(&format!("/_dd/sso/start?rd={}", urlencode(&sso::local_path(rd)))).into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }),
            )
            .route("/_dd/sso/start", get(sso::start))
            .route("/_dd/sso/creds", get(sso::creds))
            .route("/_dd/sso", post(sso::finish))
            .route("/_dd/back", get(back))
            .route("/_dd/logout", post(logout))
            .with_state(app)
            .merge(home::dir_router(house))
            .merge(leaving::router(leave.clone()))
            .merge(leaving::internal_router(leave).route_layer(axum::middleware::from_fn(this_box_only)))
            .merge(directory::router(directory));
        eprintln!("verify (this box's pages) listening on {addr}");
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .map_err(Into::into)
        });
        return Ok((addr, task));
    }
    let router = Router::new()
        .route("/verify", get(verify))
        .route("/_dd/back", get(back))
        .route("/_dd/metrics/sources", get(metrics_sources))
        .route("/health", get(|| async { "ok" }))
        // the browser side, served under /_dd/ on every vhost
        .route(
            "/_dd/login",
            get(
                |State(a): State<Arc<App>>, Query(q): Query<HashMap<String, String>>| async move {
                    // sign-in is the id Worker's: there and back, with a challenge
                    if a.id_origin.is_some() {
                        let rd = q.get("rd").map(String::as_str).unwrap_or("/");
                        return Redirect::to(&format!(
                            "/_dd/sso/start?rd={}",
                            urlencode(&sso::local_path(rd))
                        ))
                        .into_response();
                    }
                    Html(pages::login(&a.domain)).into_response()
                },
            ),
        )
        .route("/_dd/sso/start", get(sso::start))
        .route("/_dd/sso/creds", get(sso::creds))
        .route("/_dd/sso", post(sso::finish))
        .route("/_dd/login/start", post(login_start))
        .route("/_dd/login/finish", post(login_finish))
        .route(
            "/_dd/logout",
            get(|| async {
                Html(pages::confirm(
                    "Sign out",
                    "This ends your session in this browser.",
                    "/_dd/logout",
                    "Sign out",
                ))
            })
            .post(logout),
        )
        .route("/_dd/home", get(home_page))
        .route("/_dd/enrol", get(|| async { Html(pages::enrol()) }))
        .route(
            "/_dd/join",
            get(|State(a): State<Arc<App>>| async move { Html(pages::join(&a.domain)) }),
        )
        .route("/_dd/join/start", post(join_start))
        .route("/_dd/join/finish", post(join_finish))
        .route("/_dd/join/sign", post(join_sign))
        .route("/_dd/leave", get(leaving::mine))
        .route("/_dd/leave/start", post(leaving::start))
        .route("/_dd/leave/sign", post(leaving::sign))
        .route("/_dd/demo", get(demo_page).post(demo_post))
        .route("/_dd/web/{file}", get(web_file))
        .route("/_dd/static/{file}", get(static_file))
        // what any page needs before it can ask a passkey for anything
        .route("/_dd/config", get(page_config))
        .route("/_dd/files", get(files_page))
        .route("/_dd/mail", get(mail_page))
        .route("/_dd/calendar", get(calendar_page))
        .route("/_dd/contacts", get(contacts_page))
        .route("/_dd/media", get(media_page))
        // the boxes are on Settings' Network tab now
        .route(
            "/_dd/boxes",
            get(|| async { Redirect::to("/_dd/settings#network") }),
        )
        // Backups is a tab of Settings now
        .route(
            "/_dd/backups",
            get(|| async { Redirect::to("/_dd/settings#backups") }),
        )
        // and Devices
        .route(
            "/_dd/devices",
            get(|| async { Redirect::to("/_dd/settings#devices") }),
        )
        // Network is a tab of Settings now
        .route(
            "/_dd/network",
            get(|| async { Redirect::to("/_dd/settings#network") }),
        )
        .route("/_dd/download", get(download_page))
        .route("/_dd/landing", get(|| async { page("landing") }))
        .route("/_dd/docs/page", get(|| async { page("docs") }))
        .route("/_dd/docs/page/{*rest}", get(|| async { page("docs") }))
        .route("/_dd/docs/index.json", get(docs_index))
        .route("/_dd/docs/raw/{*path}", get(docs_raw))
        .route("/_dd/fleet.json", get(fleet_json))
        // anyone's: whether the services work (status.rs)
        .route("/_dd/status", get(|| async { page("status") }))
        .route("/_dd/status.json", get(status_json))
        .route("/_dd/me", get(me))
        .route("/_dd/sitemap.json", get(site_map))
        .route("/_dd/joke", get(joke_lookup))
        .route("/_dd/releases.json", get(release_notes))
        .route("/_dd/guide-keep", get(|| async { page("guide-keep") }))
        // Ente comes with the app: a browser here is handed to it
        .route("/_dd/photos", get(|| async { use_app() }))
        .route("/_dd/photos/move", get(photos::move_page))
        .route("/_dd/photos/config", post(photos::config))
        .route(
            "/_dd/settings",
            get(|State(a): State<Arc<App>>, h: HeaderMap| async move {
                member_page(&a, &h, "settings", "/_dd/settings").await
            }),
        )
        .route("/_dd/add/start", post(adddevice::start))
        .route("/_dd/add/status", get(adddevice::status))
        .route("/_dd/add/cancel", post(adddevice::cancel))
        .route("/_dd/add/offer", post(adddevice::offer))
        .route("/_dd/add/theirs", post(adddevice::theirs))
        .route("/_dd/add/wait", get(adddevice::wait))
        .route("/_dd/add/reveal", post(adddevice::reveal))
        // where the QR code goes: open to anyone, it only shows the code
        .route("/_dd/add", get(|| async { page("add") }))
        .route("/_dd/house", get(home::list))
        .route("/_dd/house/wifi", post(home::change))
        .route("/_dd/house/wifi/relay", post(home::relay))
        .route("/_dd/signins", get(signins::http::list))
        .route("/_dd/signins/end", post(signins::http::end))
        .route(
            "/_dd/devices/names",
            get(signins::http::names).put(signins::http::put_names),
        )
        .route(
            "/_dd/controllers",
            get(signins::http::controllers).put(signins::http::put_controllers),
        )
        // Storage is a tab of Settings now
        .route(
            "/_dd/storage",
            get(|| async { Redirect::to("/_dd/settings#storage") }),
        )
        .route("/_dd/storage/mine", get(storage::mine))
        .route("/_dd/photos/demo", get(demo_photos::page))
        .route("/_dd/photos/demo/list", get(demo_photos::list))
        .route("/_dd/photos/demo/thumb/{id}", get(demo_photos::thumb))
        .route("/_dd/photos/demo/photo/{id}", get(demo_photos::photo))
        .route("/_dd/csp", post(csp_report))
        .route("/_dd/player-error", post(player_error))
        .route("/_dd/photos/museum/{op}", post(photos::museum_verify))
        .route("/_dd/photos/forget", get(photos::forget))
        .route("/_dd/photos/handoff", post(photos::handoff_open))
        .route(
            "/_dd/photos/handoff/{id}",
            axum::routing::put(photos::handoff_fill).get(photos::handoff_take),
        )
        .route("/_dd/app/signin", post(photos::app_signin))
        // the network's door: a join key for an admitted device
        .route("/_dd/network/join", post(network::join))
        // the owner's word on the state the reaper's brake refused
        .route(
            "/_dd/network/reap-brake",
            get(reap_brake_state).post(reap_brake_ack),
        )
        // a box's own: signed by its TPM, answered sealed to it
        .route("/_dd/network/box", post(network::box_join))
        .route("/_dd/network/controller", post(network::controller_join))
        .route("/_dd/network/computer", post(network::computer_join))
        // a home controller's own requests, signed by its key (controller.rs)
        .route(
            "/_dd/controller/directory/{name}",
            get(controller::directory),
        )
        .route("/_dd/controller/standing/{name}", get(controller::standing))
        .route("/_dd/controller/backup", post(controller::backup))
        .route("/_dd/controller/backup/{label}", get(controller::restore))
        .route("/_dd/controller-name", get(controller::name_page))
        .route(
            "/_dd/controller-name/{label}",
            get(controller::take_name)
                .post(controller::leave_name)
                .put(controller::ask_name),
        )
        .route(
            "/_dd/controller-name/{label}/ask",
            get(controller::asked_name),
        )
        .route("/_dd/chat", get(chat_page))
        .route("/_dd/git", get(git_page))
        // anyone's: the public repositories, read-only, on the forge's
        // public name (nix/modules/forge/door.nix); the forge decides what
        // nobody may see
        .route("/_dd/git/public", get(|| async { page("git-public") }))
        .route("/_dd/git/events", get(forge_events::events))
        .route("/_dd/metrics", get(metrics_page))
        // anyone's: a Send decrypts in the browser with the key in its link
        .route(
            "/_dd/send",
            get(|| async {
                (
                    [("cache-control", "no-cache")],
                    Html(pages::page("send").unwrap_or_default()),
                )
            }),
        )
        // anyone's: Photos' and Passwords' sites say to open them in the app
        .route("/_dd/use-app", get(|| async { use_app() }))
        .route("/_dd/chat/search", get(chat_search))
        .route("/_dd/adblock", get(adblock::page))
        .route("/_dd/adblock/state", get(adblock::state))
        .route("/_dd/adblock/switch", post(adblock::switch))
        .route("/_dd/adblock/allow", post(adblock::allow))
        .route(
            "/_dd/adblock/allow/{domain}",
            axum::routing::delete(adblock::unallow),
        )
        .route("/_dd/friends", get(friends::page))
        .route("/_dd/friends/list", get(friends::list))
        .route("/_dd/friends/link", post(friends::make_link))
        .route(
            "/_dd/friends/link/{id}",
            axum::routing::delete(friends::cancel_link),
        )
        .route(
            "/_dd/friends/remove/{name}",
            axum::routing::delete(friends::unfriend),
        )
        .route(
            "/_dd/friend/{secret}",
            get(friends::link_page).post(friends::accept),
        )
        .route("/_dd/friend/{secret}/about", get(friends::about))
        // the encrypted libraries' gate: WebDAV over each library's prefix
        // a member's disk images, in the bucket (library.rs); nginx sends
        // files.<domain>/images/ here
        .route("/_dd/boxes/mine", get(member_boxes::mine))
        .route("/_dd/boxes/{kind}", put(member_boxes::put_item))
        .route("/_dd/network/joined", get(member_boxes::joined_mine))
        .route("/_dd/images", axum::routing::any(library::images_root))
        .route("/_dd/images/", axum::routing::any(library::images_root))
        .route(
            "/_dd/images/{*path}",
            axum::routing::any(library::images_path),
        )
        // a member's locked mail (mail.rs): their own objects, mail for
        // another member, and the pipe their app sends through
        .route("/_dd/mail/list/{kind}", get(mail::list))
        .route(
            "/_dd/mail/o/{kind}/{id}",
            axum::routing::any(mail::object).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route(
            "/_dd/mail/deliver/{to}/{id}",
            axum::routing::post(mail::deliver)
                .layer(axum::extract::DefaultBodyLimit::max(mail::MAX)),
        )
        .route("/_dd/mail/pipe", get(mail::pipe))
        .route("/_dd/dav/{lib}", axum::routing::any(library::handle_root))
        .route("/_dd/dav/{lib}/", axum::routing::any(library::handle_root))
        .route(
            "/_dd/dav/{lib}/{*path}",
            axum::routing::any(library::handle),
        )
        // what Movies & TV knows about a library, sealed, beside it
        .route(
            "/_dd/library/{lib}/notes/{kind}",
            get(library::notes::get).put(library::notes::put),
        )
        .route("/_dd/enrol/start", post(enrol_start))
        .route("/_dd/enrol/finish", post(enrol_finish))
        .route("/_dd/enrol/result", get(enrol_result))
        // the per-box issuer Passwords signs people in through
        .route(
            "/_dd/oidc/.well-known/openid-configuration",
            get(oidc_discovery),
        )
        .route("/_dd/oidc/authorize", get(oidc_authorize))
        .route("/_dd/oidc/token", post(oidc_token))
        .route("/_dd/oidc/userinfo", get(oidc_userinfo))
        .route("/_dd/oidc/jwks", get(oidc_jwks))
        // for the processes on this box alone: the storage ledger, the games
        // manager, and this box's link (house peers ask a directory box's
        // own, home::dir_router)
        .merge(
            Router::new()
                .merge(leaving::internal_router(leave.clone()))
                .route("/internal/storage/members", get(storage::members))
                .route("/internal/games/access", post(friends::game_access))
                .route("/internal/games/friends/{user}", get(friends::game_friends))
                .route("/_dd/house/here", get(home::here_route))
                .route_layer(axum::middleware::from_fn(this_box_only)),
        )
        .layer(axum::middleware::from_fn_with_state(app.clone(), demo_door))
        .with_state(app)
        .merge(leaving::router(leave))
        .merge(directory::router(directory));
    eprintln!("verify listening on {addr}");
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .map_err(Into::into)
    });
    Ok((addr, task))
}

#[cfg(test)]
mod tests {

    /// an entry published under one of the boxes' own names before the
    /// accept rule refused them is nobody to the gate, so no service -
    /// the forge least of all - is told the name
    #[test]
    fn the_boxes_own_names_are_nobody() {
        for name in identity::RESERVED_NAMES {
            assert_eq!(super::a_person(Some(((*name).into(), false))), None);
            assert_eq!(super::a_person(Some(((*name).into(), true))), None);
        }
        assert_eq!(
            super::a_person(Some(("sarah".into(), false))),
            Some(("sarah".into(), false))
        );
        assert_eq!(super::a_person(None), None);
    }

    /// A session a device signed in ends when the person's entry is known
    /// to be without the device, never because there is no entry to go by
    /// (the agreed state not read yet, after a restart)
    #[test]
    fn a_session_ends_only_on_a_device_known_removed() {
        use ident::Answer;
        let k = identity::generate();
        let p = identity::encode_public(&k.verifying_key());
        let fp = identity::fingerprint(&p);
        let e = |devices: Vec<identity::Device>| {
            identity::sign(
                identity::Entry {
                    name: "david".into(),
                    root: p.clone(),
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
                },
                &k,
            )
            .unwrap()
        };
        let with = e(vec![identity::Device {
            fingerprint: fp.clone(),
            public_key: p.clone(),
            added: 1,
        }]);
        assert_eq!(device_kept(Some(&with), &fp), Answer::Yes);
        assert_eq!(device_kept(Some(&e(vec![])), &fp), Answer::No);
        assert_eq!(device_kept(None, &fp), Answer::Unknown);
    }

    /// The brake is the fleet owner's to lift, by a device of theirs or a
    /// passkey shown just now; being on the box is nothing
    #[test]
    fn only_the_owner_lifts_the_brake() {
        let admin = Some("david");
        assert_eq!(
            may_ack(None, admin, false, false),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            may_ack(Some("kim"), admin, true, true),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            may_ack(Some("david"), None, true, true),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            may_ack(Some(pages::DEMO_USER), Some(pages::DEMO_USER), true, true),
            Err(StatusCode::FORBIDDEN)
        );
        // a browser of theirs that has not shown the passkey just now
        assert_eq!(
            may_ack(Some("david"), admin, false, false),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(may_ack(Some("david"), admin, false, true), Ok(()));
        assert_eq!(may_ack(Some("david"), admin, true, false), Ok(()));
    }

    /// and over http: no sign-in (a process on the box) or anyone but the
    /// owner gets nothing; the owner's tab without a fresh passkey neither
    #[tokio::test]
    async fn the_brake_is_not_lifted_from_the_box() {
        let (url, s) = gate("reap-brake", None, vec![]).await;
        let http = client();
        let post = |c: Option<String>| {
            let mut r = http
                .post(format!("{url}/_dd/network/reap-brake"))
                .json(&serde_json::json!({"state": "x"}));
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.send().await.unwrap().status().as_u16() }
        };
        assert_eq!(post(None).await, 401);
        assert_eq!(post(Some(cookie(&s.issue("stranger")))).await, 403);
        // tom is this gate's admin; his tab has not shown a passkey
        assert_eq!(post(Some(cookie(&s.issue("tom")))).await, 403);
    }

    /// Every way the gate hands out something new - a session, a code or
    /// token, a network key, a link, a device - refuses while who is who is
    /// not known (App::no_new_session). Each place one is made is found
    /// here, and each must ask first; a new one that does not fails this.
    #[test]
    fn every_issuer_asks_first() {
        let files = [
            ("lib.rs", include_str!("lib.rs")),
            ("sso.rs", include_str!("sso.rs")),
            ("photos.rs", include_str!("photos.rs")),
            ("friends.rs", include_str!("friends.rs")),
            ("adddevice.rs", include_str!("adddevice.rs")),
            ("network.rs", include_str!("network.rs")),
            ("oidc.rs", include_str!("oidc.rs")),
            ("signins.rs", include_str!("signins.rs")),
            ("controller.rs", include_str!("controller.rs")),
            ("home.rs", include_str!("home.rs")),
            ("member_boxes.rs", include_str!("member_boxes.rs")),
        ];
        // what makes something new, by file
        let makes: &[(&str, &str)] = &[
            ("*", "sessions.issue"),
            ("*", "issuer.code("),
            ("*", "issuer.redeem("),
            ("*", "friends.make_link("),
            ("*", "friends.accept("),
            ("*", "door.join_key("),
            ("*", "door.mint("),
            ("*", "door.mint_id("),
            ("photos.rs", "slots.insert("),
            ("adddevice.rs", "m.insert("),
            ("adddevice.rs", "p.offer = Some("),
            ("adddevice.rs", "p.revealed = Some("),
        ];
        // made without asking, and why that is right
        let exempt: &[(&str, &str, &str)] = &[
            (
                "lib.rs",
                "demo",
                "the demo's session: the demo is the release's, not anyone's identity",
            ),
            (
                "network.rs",
                "box_key",
                "a box's key: by the release's box list and its TPM, not who is who",
            ),
            (
                "network.rs",
                "computer_key",
                "its one caller, computer_join, asks first",
            ),
        ];
        let mut found = std::collections::BTreeSet::new();
        for (file, src) in files {
            let code = src.split("#[cfg(test)]\nmod tests").next().unwrap();
            for (only, needle) in makes {
                if *only != "*" && *only != file {
                    continue;
                }
                for (at, _) in code.match_indices(needle) {
                    // the function it is in: the nearest `fn ` before it
                    let start = code[..at]
                        .rmatch_indices("fn ")
                        .find(|(i, _)| {
                            let line = &code[code[..*i].rfind('\n').map_or(0, |n| n + 1)..*i];
                            line.trim_start()
                                .trim_start_matches("pub(crate) ")
                                .trim_start_matches("pub ")
                                .trim_start_matches("async ")
                                .is_empty()
                        })
                        .map(|(i, _)| i)
                        .unwrap();
                    let name: String = code[start + 3..]
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    found.insert((file, name.clone()));
                    if exempt.iter().any(|(f, n, _)| *f == file && *n == name) {
                        continue;
                    }
                    assert!(
                        code[start..at].contains("no_new_session()"),
                        "{file}: {name} makes something new ({needle}) without asking no_new_session() first"
                    );
                }
            }
        }
        // the ones there are, so a change here is looked at
        let want: std::collections::BTreeSet<(&str, String)> = [
            ("lib.rs", "demo"),
            ("lib.rs", "oidc_authorize"),
            ("lib.rs", "oidc_token"),
            ("lib.rs", "join_sign"),
            ("lib.rs", "login_finish"),
            ("sso.rs", "finish"),
            ("photos.rs", "app_signin"),
            ("photos.rs", "handoff_open"),
            ("friends.rs", "make_link"),
            ("friends.rs", "accept"),
            ("adddevice.rs", "start"),
            ("adddevice.rs", "offer"),
            ("adddevice.rs", "reveal"),
            ("network.rs", "join"),
            ("network.rs", "controller_join"),
            ("network.rs", "box_key"),
            ("network.rs", "computer_key"),
        ]
        .into_iter()
        .map(|(f, n)| (f, n.to_string()))
        .collect();
        assert_eq!(found, want);
        // computer_key's caller asks
        let net = include_str!("network.rs");
        let cj = &net[net.find("async fn computer_join(").unwrap()..];
        let cj = &cj[..cj.find("computer_key(").unwrap()];
        assert!(cj.contains("no_new_session()"));
    }

    /// Who belongs on the network: No only on what is known
    #[test]
    fn on_the_network_is_no_only_on_what_is_known() {
        use ident::Answer::{self, *};
        let none = Vec::<Answer>::new;
        // a member, whatever else
        assert_eq!(on_network(Yes, false, false, none), Yes);
        // not known to be a member: not known, unless a friend vouches
        assert_eq!(on_network(Unknown, false, false, none), Unknown);
        assert_eq!(on_network(Unknown, false, true, || vec![Yes]), Yes);
        // known no member, with no guest record: off
        assert_eq!(on_network(No, false, false, none), No);
        // a guest of theirs left: off; a guest whose friend is a member: on
        assert_eq!(on_network(No, true, true, || vec![Yes]), No);
        assert_eq!(on_network(No, false, true, || vec![No, Yes]), Yes);
        // a guest whose friends are all known no member: off; one not known
        // yet keeps them on
        assert_eq!(on_network(No, false, true, || vec![No, No]), No);
        assert_eq!(on_network(No, false, true, || vec![No, Unknown]), Unknown);
        assert_eq!(on_network(No, false, true, none), No);
    }

    #[test]
    fn a_suspension_and_its_reason_come_with_the_list() {
        let m = Members::parse(
            r#"{"members": [], "revoked": ["abc"], "removed": {"abc": 7}, "suspended": {"abc": "spam"}}"#,
        )
        .unwrap();
        assert_eq!(m.suspended.get("abc").map(String::as_str), Some("spam"));
        assert_eq!(m.removed.get("abc"), Some(&7));
    }

    use super::*;

    #[test]
    fn the_apps_note_is_read_by_exact_name_and_value() {
        let h = |c: &str| {
            let mut m = HeaderMap::new();
            m.insert("cookie", c.parse().unwrap());
            m
        };
        assert!(app_answers(&h("a=b; dd_app=1")));
        assert!(!app_answers(&h("dd_app=0")));
        assert!(!app_answers(&h("xdd_app=1")));
        assert!(!app_answers(&HeaderMap::new()));
    }

    const PHONE: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Mobile Safari/537.36";

    /// What the phone's player would have said: one line, with the error,
    /// the browser and the film's kind.
    #[test]
    fn a_player_error_is_one_line() {
        let body = serde_json::json!({
            "kind": "hls", "type": "mediaError", "details": "bufferFullError",
            "fatal": true, "recovered": true, "error": "QuotaExceededError",
            "size": 1_288_000_000u64, "container": "mkv",
            "video": "avc1.640028", "audio": "ac-3", "width": 1920, "height": 818,
            "piece": 11_200_000, "pieceSecs": 10.417, "buffered": 20.8,
        });
        let line = player_line("files.example.org", PHONE, body.to_string().as_bytes()).unwrap();
        assert_eq!(
            line,
            "player-error: files.example.org Chrome/154 Android hls mediaError/bufferFullError code=- fatal=true recovered=true QuotaExceededError; film 1288.0MB mkv avc1.640028/ac-3 1920x818; segment 11.2MB 10.4s, buffered 20.8s"
        );
        assert!(!line.contains('\n'));
    }

    /// Whatever the page sends, no name, path, library or url reaches the
    /// log: every field is a word or a number, and unknown ones are dropped.
    #[test]
    fn a_player_error_logs_nothing_private() {
        let body = serde_json::json!({
            "kind": "hls/../Movies/Secret Film (2004).mkv",
            "details": "https://s3.example.org/libraries/0123abcd/enc?X-Amz-Signature=f00",
            "error": "name\nplayer-error: forged",
            "container": "Secret Film.mkv",
            "path": "Movies/Secret Film (2004).mkv",
            "library": "0123abcd",
            "name": "Secret Film",
            "size": "Secret",
            "width": -5,
        });
        let line = player_line(
            "files.example.org/Movies\n",
            "Secret Agent",
            body.to_string().as_bytes(),
        )
        .unwrap();
        for private in [
            "Film",
            "2004",
            "Movies",
            "0123abcd",
            "Signature",
            "\n",
            "forged",
            "Secret",
            "Agent",
        ] {
            assert!(!line.contains(private), "{private:?} in {line}");
        }
        assert!(line.contains("film - - -/- -x-"), "{line}");
        assert!(line.contains("other other"), "{line}");
    }

    #[test]
    fn a_player_error_is_read_small() {
        let big = format!("{{\"details\":\"{}\"}}", "a".repeat(PLAYER_ERROR_BODY));
        assert_eq!(
            player_line("h", PHONE, big.as_bytes()),
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
        assert_eq!(
            player_line("h", PHONE, b"not json"),
            Err(StatusCode::BAD_REQUEST)
        );
        // a long word is cut short
        let long = format!("{{\"details\":\"{}\"}}", "a".repeat(500));
        assert!(
            !player_line("h", PHONE, long.as_bytes())
                .unwrap()
                .contains(&"a".repeat(41))
        );
    }

    #[test]
    fn browsers_are_named_by_family() {
        assert_eq!(ua_family(PHONE), "Chrome/154 Android");
        assert_eq!(
            ua_family(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"
            ),
            "Safari/18 iOS"
        );
        assert_eq!(
            ua_family("Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0"),
            "Firefox/140 Linux"
        );
        assert_eq!(ua_family(""), "other other");
    }

    #[test]
    fn peeks_the_user() {
        assert_eq!(
            peek_user("user(\"sarah\");\ndevice(\"ab\");\n").as_deref(),
            Some("sarah")
        );
        assert_eq!(peek_user("device(\"ab\");\n"), None);
    }
    #[test]
    fn the_demos_leash_follows_the_demos_door_not_the_members() {
        let svc =
            |name: &str, url: &str, demo_url: Option<&str>, demo: Option<&str>| pages::Service {
                name: name.into(),
                url: url.into(),
                icon: String::new(),
                color: String::new(),
                blurb: String::new(),
                demo: demo.map(str::to_string),
                demo_url: demo_url.map(str::to_string),
                menu_only: false,
            };
        let home = [
            // members use their own copy here; the demo is sent to a door
            // of its own, and "full" is a permission on that door alone
            svc(
                "Games",
                "https://files.x/_dd/games",
                Some("https://games.x/demo"),
                Some("full"),
            ),
            svc("Files", "https://files.x/_dd/files", None, None),
            svc("Chat", "https://llm.x/", None, Some("rate:10")),
        ];
        assert_eq!(demo_allowance(&home, "games.x").as_deref(), Some("full"));
        assert_eq!(demo_allowance(&home, "files.x"), None);
        assert_eq!(demo_allowance(&home, "llm.x").as_deref(), Some("rate:10"));
        assert_eq!(demo_allowance(&home, "git.x"), None);
    }

    #[test]
    fn only_deleting_an_image_wants_the_passkey() {
        let h = |method: &str, uri: &str| {
            let mut m = HeaderMap::new();
            m.insert("x-original-method", method.parse().unwrap());
            m.insert("x-original-uri", uri.parse().unwrap());
            m
        };
        assert!(deletes_image(&h("DELETE", "/images/data/ab")));
        assert!(!deletes_image(&h("GET", "/images/data/ab")));
        assert!(!deletes_image(&h("PROPFIND", "/images/")));
        assert!(!deletes_image(&h("DELETE", "/_dd/dav/x")));
        // every other change too, and however the path is spelled
        for m in ["PUT", "MOVE", "COPY", "MKCOL", "PATCH"] {
            assert!(deletes_image(&h(m, "/images/data/ab")), "{m}");
        }
        for u in [
            "//images/data/ab",
            "/%69mages/data/ab",
            "/x/../images/ab",
            "/./images/ab?x=1",
        ] {
            assert!(deletes_image(&h("DELETE", u)), "{u}");
        }
    }

    #[test]
    fn back_goes_only_to_our_own_pages() {
        for ok in [
            "https://metrics.x/boxes",
            "https://home.x/_dd/settings#devices",
            "https://git.x/a/b",
            "https://x/docs/guide",
        ] {
            assert!(back_to("x", ok).is_some(), "{ok}");
        }
        for not in [
            "http://home.x/",
            "https://home.x:8443/",
            "https://a@home.x/",
            "https://evil.example/",
            "https://home.x.evil.example/",
            "https://vault.x/",
            "javascript:alert(1)",
            "/_dd/home",
        ] {
            assert!(back_to("x", not).is_none(), "{not}");
        }
    }

    const DOMAIN: &str = "commonty.test";

    /// A full gate on a free port of loopback, its state in a folder of its
    /// own, asked over http as nginx and the pages ask it. Its sessions are
    /// made with the secret it keeps there. No member list: anyone signed
    /// in is a member, unless `members` says who is.
    async fn gate(
        what: &str,
        members: Option<Members>,
        home: Vec<pages::Service>,
    ) -> (String, session::Sessions) {
        gate_with(what, members, home, vec![]).await
    }

    /// the same, with the keys that may read single entries
    async fn gate_with(
        what: &str,
        members: Option<Members>,
        home: Vec<pages::Service>,
        directory_readers: Vec<String>,
    ) -> (String, session::Sessions) {
        gate_full(what, members, home, directory_readers, None).await
    }

    /// the same, holding the libraries bucket
    async fn gate_full(
        what: &str,
        members: Option<Members>,
        home: Vec<pages::Service>,
        directory_readers: Vec<String>,
        library: Option<library::Gate>,
    ) -> (String, session::Sessions) {
        gate_mail(what, members, home, directory_readers, library, None).await
    }

    /// the same, holding the mail bucket too (mail.rs)
    async fn gate_mail(
        what: &str,
        members: Option<Members>,
        home: Vec<pages::Service>,
        directory_readers: Vec<String>,
        library: Option<library::Gate>,
        mail: Option<mail::Mail>,
    ) -> (String, session::Sessions) {
        gate_reading(
            what,
            members,
            home,
            directory_readers,
            library,
            mail,
            None,
            Default::default(),
        )
        .await
    }

    /// the same, reading thanos at this address for the uptime page, and
    /// believing these boxes on each service (status.rs)
    #[allow(clippy::too_many_arguments)]
    async fn gate_reading(
        what: &str,
        members: Option<Members>,
        home: Vec<pages::Service>,
        directory_readers: Vec<String>,
        library: Option<library::Gate>,
        mail: Option<mail::Mail>,
        thanos: Option<String>,
        service_boxes: status::Speakers,
    ) -> (String, session::Sessions) {
        let d = std::env::temp_dir().join(format!("dd-gate-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (addr, _) = start(Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            dir: d.join("keys"),
            house_peers: vec![],
            directory_readers,
            names: vec!["127.0.0.1".into()],
            control: None,
            identity_from: Default::default(),
            domain: Some(DOMAIN.into()),
            oidc: None,
            home,
            members,
            release_pub: None,
            web_dir: None,
            photos: None,
            library,
            mail,
            network: None,
            demo_library: None,
            app_manifest: None,
            member_list: None,
            fleet: Default::default(),
            thanos,
            service_boxes,
            adblock: None,
            forge_events: None,
            storage_ledger: None,
            tmdb: None,
            search: None,
            id_origin: None,
            box_only: false,
            owner: None,
            // the fleet's admin, who sees every box's metrics
            admin: Some("tom".into()),
            boxes: Default::default(),
        })
        .await
        .unwrap();
        let sessions = session::Sessions::open(&d, DOMAIN, false).unwrap();
        (format!("http://{addr}"), sessions)
    }

    /// what a browser sends back of a Set-Cookie
    fn cookie(set: &str) -> String {
        set.split(';').next().unwrap().to_string()
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    fn demo_tile() -> pages::Service {
        pages::Service {
            name: "Files".into(),
            url: format!("https://files.{DOMAIN}/_dd/files"),
            icon: String::new(),
            color: String::new(),
            blurb: String::new(),
            demo: Some("read".into()),
            demo_url: None,
            menu_only: false,
        }
    }

    #[tokio::test]
    async fn a_bearer_that_is_not_a_token_does_not_skip_the_passkey() {
        let (url, s) = gate("image-bearer", None, vec![]).await;
        let tom = cookie(&s.issue("tom"));
        let ask = |method: &str, bearer: Option<&str>| {
            let mut r = client()
                .get(format!("{url}/verify"))
                .header("cookie", &tom)
                .header("x-original-method", method)
                .header("x-original-uri", "/images/data/ab");
            if let Some(b) = bearer {
                r = r.header("authorization", format!("Bearer {b}"));
            }
            async move { r.send().await.unwrap().status().as_u16() }
        };
        assert_eq!(ask("GET", None).await, 200);
        // a session, no passkey just now: no deleting
        assert_eq!(ask("DELETE", None).await, 403);
        // and a header that is no device's token changes nothing
        assert_eq!(ask("DELETE", Some("x")).await, 403);
        assert_eq!(ask("PUT", Some("not-a-biscuit")).await, 403);
    }

    /// A stand-in for thanos on a free port: every answer carries a box's
    /// name and address, as a real one could, and none of it may reach what
    /// the gate tells anyone.
    async fn thanos_stand_in() -> String {
        use axum::extract::Query;
        let now = session::now();
        let point = move |v: &str| serde_json::json!([now, v]);
        let app = Router::new()
            .route(
                "/api/v1/query",
                get(move || async move {
                    Json(serde_json::json!({"status": "success", "data": {"resultType": "vector", "result": [
                        {"metric": {"service": "photos", "box": "node1", "instance": "100.64.0.2:9100"}, "value": point("1")},
                        {"metric": {"service": "git", "box": "node1"}, "value": point("0")},
                        // a box not built with Git says it works: not asked
                        {"metric": {"service": "git", "box": "node2"}, "value": point("1")},
                        {"metric": {"service": "node1-secret"}, "value": point("1")}
                    ]}}))
                }),
            )
            .route(
                "/api/v1/query_range",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    let start: u64 = q["start"].parse().unwrap();
                    let end: u64 = q["end"].parse().unwrap();
                    let step: u64 = q["step"].parse().unwrap();
                    let values: Vec<_> = (0..)
                        .map(|i| start + i * step)
                        .take_while(|t| *t <= end)
                        .map(|t| serde_json::json!([t, "1"]))
                        .collect();
                    let down: Vec<_> = values
                        .iter()
                        .map(|v| serde_json::json!([v[0], "0"]))
                        .collect();
                    Json(serde_json::json!({"status": "success", "data": {"resultType": "matrix", "result": [
                        {"metric": {"service": "photos", "box": "node1", "site": "s-c844d9db"}, "values": values},
                        {"metric": {"service": "git", "box": "node1"}, "values": values},
                        {"metric": {"service": "photos", "box": "node2"}, "values": down}
                    ]}}))
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
        format!("http://{addr}")
    }

    /// The uptime page's answer is the same for anyone: services, never a
    /// box, an address, a place, how many boxes there are or a release.
    #[tokio::test]
    async fn the_status_is_services_alone_and_the_same_for_anyone() {
        let fake = thanos_stand_in().await;
        // node1 is built with Photos and Git, node2 with neither
        let speakers: status::Speakers = [("photos", "node1"), ("git", "node1")]
            .into_iter()
            .map(|(s, b)| (s.to_string(), vec![b.to_string()]))
            .collect();
        let (url, s) = gate_reading(
            "status",
            None,
            vec![demo_tile()],
            vec![],
            None,
            None,
            Some(fake),
            speakers,
        )
        .await;
        let http = client();
        let mut seen = vec![];
        for c in [
            None,
            Some(cookie(&s.issue("tom"))),
            Some(cookie(&s.issue("demo"))),
        ] {
            let mut r = http.get(format!("{url}/_dd/status.json"));
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            let r = r.send().await.unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.headers()["cache-control"], "public, max-age=30");
            let body = r.text().await.unwrap();
            for no in [
                "node",
                "box",
                "100.64",
                "instance",
                "s-c844",
                "site\":\"s",
                "release",
            ] {
                assert!(!body.contains(no), "{no} in {body}");
            }
            seen.push(body);
        }
        assert!(seen.iter().all(|b| *b == seen[0]));
        let v: serde_json::Value = serde_json::from_str(&seen[0]).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 3, "{v}");
        let services = v["services"].as_array().unwrap();
        let ids: Vec<_> = services.iter().map(|x| x["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["photos", "git"]);
        assert_eq!(services[0]["label"], "Photos");
        assert_eq!(services[0]["up"], true);
        assert_eq!(services[1]["up"], false);
        assert_eq!(services[0]["days"].as_array().unwrap().len(), 90);
        assert_eq!(services[0]["days"][89], 1.0);
        assert!(services[0]["since"].is_u64());

        // and the page, for anyone
        let page = http.get(format!("{url}/_dd/status")).send().await.unwrap();
        assert_eq!(page.status(), 200);
        let html = page.text().await.unwrap();
        for f in ["status.js", "status.css", "shell.js", "home.css"] {
            assert!(html.contains(&format!("/_dd/static/{f}")), "{f}");
        }
    }

    /// Where nothing is measured, the gate says so rather than guessing.
    #[tokio::test]
    async fn no_thanos_is_no_status_rather_than_a_guess() {
        let (url, _) = gate("status-none", None, vec![]).await;
        let r = client()
            .get(format!("{url}/_dd/status.json"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 503);
        assert_eq!(r.headers()["cache-control"], "no-store");
    }

    /// The bare name's front page is for anyone, signed in or not, and
    /// home's own front is still the way in.
    #[tokio::test]
    async fn the_front_page_is_for_anyone_and_home_still_signs_in() {
        let (url, s) = gate("landing", None, vec![]).await;
        let http = client();
        let tom = cookie(&s.issue("tom"));
        for c in [None, Some(tom.clone())] {
            let mut r = http.get(format!("{url}/_dd/landing"));
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            let r = r.send().await.unwrap();
            assert_eq!(r.status(), 200);
            assert!(
                r.headers()["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("text/html")
            );
            assert_eq!(r.headers()["cache-control"], "no-cache");
            let html = r.text().await.unwrap();
            assert!(html.contains("Keep it <span>yours.</span>"));
            // its own files, from this site, and no name of ours written in
            for f in ["landing.css", "landing.js", "footer.js", "home.css"] {
                assert!(html.contains(&format!("/_dd/static/{f}")), "{f}");
                let got = http
                    .get(format!("{url}/_dd/static/{f}"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(got.status(), 200, "{f}");
            }
            assert!(!html.contains("commonty.org"));
            assert!(!html.contains("<script>") && !html.contains("style=\""));
            assert!(html.contains("data-home=\"/_dd/demo\""));
            assert!(html.contains("href=\"#main\""));
        }
        let home = http.get(format!("{url}/_dd/home")).send().await.unwrap();
        assert_eq!(home.status(), 303);
        assert_eq!(home.headers()["location"], "/_dd/login?rd=/");
        let home = http
            .get(format!("{url}/_dd/home"))
            .header("cookie", tom)
            .send()
            .await
            .unwrap();
        assert_eq!(home.status(), 200);
    }

    /// The joke's parts are for members and the demo, like the site map
    #[tokio::test]
    async fn the_joke_is_a_members_and_the_demos() {
        let tom = Person::new("tom");
        let (url, s) = gate(
            "joke",
            Some(Members::list(vec![tom.id()])),
            vec![demo_tile()],
        )
        .await;
        let http = client();
        let ask = |c: Option<String>| {
            let mut r = http.get(format!("{url}/_dd/joke?q=black%20knight"));
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.send().await.unwrap() }
        };
        assert_eq!(ask(None).await.status(), 401);
        assert_eq!(ask(Some(cookie(&s.issue("stranger")))).await.status(), 403);
        tom.publish(&url).await;
        let r = ask(Some(cookie(&s.issue("tom")))).await;
        assert_eq!(r.status(), 200);
        let j: serde_json::Value = r.json().await.unwrap();
        assert!(j["found"].as_array().is_some_and(|f| !f.is_empty()), "{j}");
        assert_eq!(j["from"]["license"], "CC BY-SA 3.0");
    }

    /// The site map is for members and the demo, each their own; nobody
    /// signed out, nobody signed in who is neither
    #[tokio::test]
    async fn the_site_map_is_a_members_and_the_demos() {
        let tom = Person::new("tom");
        let (url, s) = gate(
            "sitemap",
            Some(Members::list(vec![tom.id()])),
            vec![demo_tile()],
        )
        .await;
        let http = client();
        let ask = |c: Option<String>| {
            let mut r = http.get(format!("{url}/_dd/sitemap.json"));
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.send().await.unwrap() }
        };
        assert_eq!(ask(None).await.status(), 401);
        assert_eq!(ask(Some(cookie(&s.issue("stranger")))).await.status(), 403);
        tom.publish(&url).await;
        let r = ask(Some(cookie(&s.issue("tom")))).await;
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["cache-control"], "private, no-cache");
        let m: serde_json::Value = r.json().await.unwrap();
        let ids: Vec<&str> = m["places"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["id"].as_str())
            .collect();
        assert!(
            ids.contains(&"settings-devices") && ids.contains(&"svc-files"),
            "{ids:?}"
        );
        assert!(
            !m.to_string().contains("\"tom\""),
            "the map names who asked"
        );
        let r = ask(Some(cookie(&s.issue(pages::DEMO_USER)))).await;
        assert_eq!(r.status(), 200);
        let m: serde_json::Value = r.json().await.unwrap();
        assert!(!m.to_string().contains("/_dd/settings"));
    }

    #[tokio::test]
    async fn the_demo_never_sees_metrics() {
        // even a tile that would let it in
        let mut m = demo_tile();
        m.name = "Metrics".into();
        m.url = format!("https://metrics.{DOMAIN}/");
        m.demo = Some("full".into());
        let (url, s) = gate("demo-metrics", None, vec![demo_tile(), m]).await;
        let http = client();
        let demo = cookie(&s.issue(pages::DEMO_USER));
        let tom = cookie(&s.issue("tom"));
        let ask = |c: &str, host: String| {
            let r = http
                .get(format!("{url}/verify"))
                .header("cookie", c)
                .header("x-original-host", host)
                .header("x-original-uri", "/api/v1/query?query=up");
            async move { r.send().await.unwrap().status().as_u16() }
        };
        assert_eq!(ask(&demo, format!("metrics.{DOMAIN}")).await, 403);
        assert_eq!(ask(&demo, format!("demo-metrics.{DOMAIN}")).await, 403);
        assert_eq!(ask(&tom, format!("metrics.{DOMAIN}")).await, 200);
        // its own door still lets it in
        assert_eq!(ask(&demo, format!("files.{DOMAIN}")).await, 200);
        let page = |c: String| {
            let r = http.get(format!("{url}/_dd/metrics")).header("cookie", c);
            async move { r.send().await.unwrap().status().as_u16() }
        };
        assert_eq!(page(demo).await, 403);
        assert_eq!(page(tom).await, 200);
    }

    #[tokio::test]
    async fn signing_out_and_the_demo_are_posts_from_our_own_pages() {
        let (url, s) = gate("post-only", None, vec![demo_tile()]).await;
        let http = client();
        let tom = cookie(&s.issue("tom"));
        let me = |c: String| {
            let r = http
                .get(format!("{url}/_dd/storage/mine"))
                .header("cookie", c);
            async move { r.send().await.unwrap().status().as_u16() }
        };
        assert_eq!(me(tom.clone()).await, 200);

        // a link (GET) is a page with the button, and changes nothing
        let r = http
            .get(format!("{url}/_dd/logout"))
            .header("cookie", &tom)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.headers().get("set-cookie").is_none());
        assert!(r.text().await.unwrap().contains("method=\"post\""));
        let r = http
            .get(format!("{url}/_dd/demo"))
            .header("cookie", &tom)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.headers().get("set-cookie").is_none());
        assert_eq!(me(tom.clone()).await, 200);

        // another site's form is refused, a page of ours is not
        for (origin, site) in [
            ("https://evil.example", "cross-site"),
            ("null", "cross-site"),
            (&*format!("https://{DOMAIN}.evil.example"), "cross-site"),
        ] {
            for path in ["/_dd/logout", "/_dd/demo"] {
                let r = http
                    .post(format!("{url}{path}"))
                    .header("cookie", &tom)
                    .header("origin", origin)
                    .header("sec-fetch-site", site)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(r.status(), 403, "{path} from {origin}");
                assert!(r.headers().get("set-cookie").is_none());
            }
        }
        // a same-site page still has to be one of our names
        let r = http
            .post(format!("{url}/_dd/demo"))
            .header("cookie", &tom)
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
        assert_eq!(me(tom.clone()).await, 200);

        let r = http
            .post(format!("{url}/_dd/demo"))
            .header("cookie", &tom)
            .header("origin", format!("https://games.{DOMAIN}"))
            .header("sec-fetch-site", "same-site")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        assert!(r.headers().get("set-cookie").is_some());
        let r = http
            .post(format!("{url}/_dd/logout"))
            .header("cookie", &tom)
            .header("origin", format!("https://home.{DOMAIN}"))
            .header("sec-fetch-site", "same-origin")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        assert!(r.headers().get("set-cookie").is_some());
        assert_eq!(me(tom).await, 401, "the session itself has ended");

        // no session at all: a link still starts the demo in one click
        let r = http.get(format!("{url}/_dd/demo")).send().await.unwrap();
        assert_eq!(r.status(), 303);
        assert!(r.headers().get("set-cookie").is_some());
    }

    #[tokio::test]
    async fn this_boxs_own_calls_answer_this_box_alone() {
        let (url, _) = gate("internal", None, vec![]).await;
        let http = client();
        let paths = [
            ("GET", "/internal/storage/members"),
            ("GET", "/internal/games/friends/tom"),
            ("POST", "/internal/games/access"),
            ("GET", "/_dd/house/here"),
        ];
        for (m, p) in paths {
            let req = |extra: Option<(&str, &str)>| {
                let mut r = http
                    .request(m.parse().unwrap(), format!("{url}{p}"))
                    .header("content-type", "application/json")
                    .body("[]");
                if let Some((k, v)) = extra {
                    r = r.header(k, v);
                }
                async move { r.send().await.unwrap().status().as_u16() }
            };
            // a process on this box
            assert_eq!(req(None).await, 200, "{p}");
            // the same, passed on by nginx
            for h in ["x-original-uri", "x-forwarded-for", "x-real-ip"] {
                assert_eq!(req(Some((h, "1"))).await, 404, "{p} via {h}");
            }
        }
        // and anything but loopback, wherever the gate listens
        let h = HeaderMap::new();
        assert!(from_this_box(Some("127.0.0.1".parse().unwrap()), &h));
        assert!(from_this_box(Some("::1".parse().unwrap()), &h));
        assert!(from_this_box(Some("::ffff:127.0.0.1".parse().unwrap()), &h));
        assert!(!from_this_box(Some("100.64.0.7".parse().unwrap()), &h));
        assert!(!from_this_box(Some("192.168.1.21".parse().unwrap()), &h));
        assert!(!from_this_box(None, &h));
    }

    #[tokio::test]
    async fn a_member_at_a_demo_door_keeps_the_query() {
        let (url, s) = gate("demo-door-query", None, vec![demo_tile()]).await;
        let tom = cookie(&s.issue("tom"));
        let r = client()
            .get(format!("{url}/_dd/files?path=%2Fa%20b&x=1"))
            .header("host", format!("demo-files.{DOMAIN}"))
            .header("cookie", &tom)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        assert_eq!(
            r.headers()["location"],
            format!("https://files.{DOMAIN}/_dd/files?path=%2Fa%20b&x=1").as_str()
        );
        // a name that is not ours goes nowhere
        let r = client()
            .get(format!("{url}/_dd/files?x=1"))
            .header("host", "demo-evil.example")
            .header("cookie", &tom)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404);
    }

    /// A device's token under one of the boxes' own names is nobody's, even
    /// with an entry on file that the accept rule would refuse today (one
    /// published before it): the directory's own way of telling who asks,
    /// and the gate's, both go through this
    #[test]
    fn a_device_token_for_the_boxes_own_names_is_nobody() {
        let on_file = |p: &Person| {
            let device = B64.encode(p.device.public().to_bytes());
            identity::sign(
                identity::Entry {
                    name: p.name.into(),
                    root: identity::encode_public(&p.root.verifying_key()),
                    recovery: identity::encode_public(&p.recovery.verifying_key()),
                    recovered_from: vec![],
                    devices: vec![identity::Device {
                        fingerprint: identity::fingerprint(&device),
                        public_key: device,
                        added: 1,
                    }],
                    passkeys: vec![],
                    grant: None,
                    libraries: vec![],
                    terms: Some(identity::TERMS.into()),
                    removed_boxes: vec![],
                    version: 1,
                    updated: identity::now(),
                },
                &p.root,
            )
            .unwrap()
        };
        let sarah = Person::new("sarah");
        let e = on_file(&sarah);
        let (who, _) =
            super::verify_device_token(|_| Ok(Some(e.clone())), &sarah.token(), "access", "x")
                .unwrap();
        assert_eq!(who, "sarah");
        for name in identity::RESERVED_NAMES {
            let p = Person::new(name);
            let e = on_file(&p);
            assert!(
                super::verify_device_token(|_| Ok(Some(e.clone())), &p.token(), "access", "x")
                    .is_err(),
                "{name}"
            );
        }
    }

    /// a person: a root, a paper key and one device
    struct Person {
        name: &'static str,
        root: identity::SigningKey,
        recovery: identity::SigningKey,
        device: biscuit_auth::KeyPair,
    }

    impl Person {
        fn new(name: &'static str) -> Self {
            Person {
                name,
                root: identity::generate(),
                recovery: identity::generate(),
                device: biscuit_auth::KeyPair::new_with_algorithm(Algorithm::Ed25519),
            }
        }
        async fn publish(&self, url: &str) {
            let device = B64.encode(self.device.public().to_bytes());
            let signed = identity::sign(
                identity::Entry {
                    name: self.name.into(),
                    root: identity::encode_public(&self.root.verifying_key()),
                    recovery: identity::encode_public(&self.recovery.verifying_key()),
                    recovered_from: vec![],
                    devices: vec![identity::Device {
                        fingerprint: identity::fingerprint(&device),
                        public_key: device,
                        added: 1,
                    }],
                    passkeys: vec![],
                    grant: None,
                    libraries: vec![],
                    terms: Some(identity::TERMS.into()),
                    removed_boxes: vec![],
                    version: 1,
                    updated: identity::now(),
                },
                &self.root,
            )
            .unwrap();
            let r = client()
                .put(format!("{url}/_dd/directory/{}", self.name))
                .json(&signed)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        }
        fn id(&self) -> String {
            identity::member_id(&identity::encode_public(&self.root.verifying_key()))
        }
        fn token(&self) -> String {
            Biscuit::builder()
                .fact(format!("user({:?})", self.name).as_str())
                .unwrap()
                .build(&self.device)
                .unwrap()
                .to_base64()
                .unwrap()
        }
        /// as account::read_token mints it: directory reads, at `host`
        fn read_token(&self, host: &str) -> String {
            let op = identity::DIRECTORY_READ;
            Biscuit::builder()
                .fact(format!("user({:?})", self.name).as_str())
                .unwrap()
                .fact(format!("audience({host:?})").as_str())
                .unwrap()
                .check(format!("check if here({host:?})").as_str())
                .unwrap()
                .fact(format!("purpose({op:?})").as_str())
                .unwrap()
                .check(format!("check if operation({op:?})").as_str())
                .unwrap()
                .build(&self.device)
                .unwrap()
                .to_base64()
                .unwrap()
        }
    }

    enum Ask {
        Nobody,
        Cookie(String),
        Bearer(String),
        Proof(String),
    }

    async fn read(url: &str, path: &str, ask: Ask) -> u16 {
        let r = client().get(format!("{url}/_dd/directory{path}"));
        let r = match ask {
            Ask::Nobody => r,
            Ask::Cookie(c) => r.header("cookie", c),
            Ask::Bearer(t) => r.bearer_auth(t),
            Ask::Proof(p) => r.header(identity::READ_PROOF_HEADER, p),
        };
        r.send().await.unwrap().status().as_u16()
    }

    #[tokio::test]
    async fn the_directory_is_read_by_members_owners_and_the_mail_key() {
        let mail = identity::generate();
        let mail_pub = identity::encode_public(&mail.verifying_key());
        let (sarah, tom) = (Person::new("sarah"), Person::new("tom"));
        // sarah is on the list; tom signed up and is not
        let (url, s) = gate_with(
            "dir-read",
            Some(Members::list(vec![sarah.id()])),
            vec![],
            vec![mail_pub],
        )
        .await;
        sarah.publish(&url).await;
        tom.publish(&url).await;
        let now = identity::now();

        // nobody: a 401 whether the name is there or not
        assert_eq!(read(&url, "", Ask::Nobody).await, 401);
        assert_eq!(read(&url, "/sarah", Ask::Nobody).await, 401);
        assert_eq!(read(&url, "/nobody", Ask::Nobody).await, 401);
        let stray = identity::read_proof(&identity::generate(), "sarah", now);
        assert_eq!(read(&url, "/sarah", Ask::Proof(stray)).await, 401);
        assert_eq!(
            read(&url, "", Ask::Cookie("dd_session=x".into())).await,
            401
        );

        // a member, by cookie or device token: the list and anyone's entry
        let hers = || Ask::Cookie(cookie(&s.issue("sarah")));
        assert_eq!(read(&url, "", hers()).await, 200);
        assert_eq!(read(&url, "/tom", hers()).await, 200);
        assert_eq!(read(&url, "/nobody", hers()).await, 404);
        assert_eq!(read(&url, "", Ask::Bearer(sarah.token())).await, 200);
        assert_eq!(read(&url, "/tom", Ask::Bearer(sarah.token())).await, 200);

        // not a member: their own entry and nothing else
        let his = || Ask::Cookie(cookie(&s.issue("tom")));
        assert_eq!(read(&url, "/tom", his()).await, 200);
        assert_eq!(read(&url, "/sarah", his()).await, 401);
        assert_eq!(read(&url, "", his()).await, 401);
        assert_eq!(read(&url, "/tom", Ask::Bearer(tom.token())).await, 200);
        assert_eq!(read(&url, "", Ask::Bearer(tom.token())).await, 401);

        // her own keys read her own entry with no device signed in: the
        // paper key and the root
        let paper = identity::read_proof(&sarah.recovery, "sarah", now);
        assert_eq!(read(&url, "/sarah", Ask::Proof(paper.clone())).await, 200);
        let root = identity::read_proof(&sarah.root, "sarah", now);
        assert_eq!(read(&url, "/sarah", Ask::Proof(root)).await, 200);
        let hers_for_tom = identity::read_proof(&sarah.recovery, "tom", now);
        assert_eq!(read(&url, "/tom", Ask::Proof(hers_for_tom)).await, 401);
        assert_eq!(read(&url, "", Ask::Proof(paper)).await, 401);

        // a token minted for directory reads at this host: reads here, and
        // nothing else anywhere
        let here = sarah.read_token("127.0.0.1");
        assert_eq!(read(&url, "", Ask::Bearer(here.clone())).await, 200);
        assert_eq!(read(&url, "/tom", Ask::Bearer(here.clone())).await, 200);
        let elsewhere = sarah.read_token("files.example.org");
        assert_eq!(read(&url, "", Ask::Bearer(elsewhere.clone())).await, 401);
        assert_eq!(read(&url, "/sarah", Ask::Bearer(elsewhere)).await, 401);
        // the audience is checked against this box's own names: a Host or
        // X-Original-Host a request makes up does not add one
        let made_up = sarah.read_token("evil.example");
        for h in ["host", "x-original-host"] {
            let r = client()
                .get(format!("{url}/_dd/directory"))
                .header(h, "evil.example")
                .bearer_auth(&made_up)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 401, "{h}: evil.example");
        }
        // a token for another of its names is refused on this one, and
        // taken when the request is asked under that name
        let home = sarah.read_token(&format!("home.{DOMAIN}"));
        assert_eq!(read(&url, "", Ask::Bearer(home.clone())).await, 401);
        let r = client()
            .get(format!("{url}/_dd/directory"))
            .header("x-original-host", format!("home.{DOMAIN}"))
            .bearer_auth(&home)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        for route in ["/_dd/me", "/verify", "/_dd/storage/mine"] {
            let r = client()
                .get(format!("{url}{route}"))
                .bearer_auth(&here)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 401, "{route}");
        }

        // the mail Worker's key: any one entry, never the list, never stale
        let m = identity::read_proof(&mail, "tom", now);
        assert_eq!(read(&url, "/tom", Ask::Proof(m.clone())).await, 200);
        assert_eq!(read(&url, "", Ask::Proof(m)).await, 401);
        let stale = identity::read_proof(&mail, "tom", now - 3600);
        assert_eq!(read(&url, "/tom", Ask::Proof(stale)).await, 401);
    }

    /// Until the release names the mail Worker's key, single entries stay
    /// open (the Worker reads them with nothing); the list is closed now.
    #[tokio::test]
    async fn without_a_reader_key_single_entries_stay_open_and_the_list_does_not() {
        let sarah = Person::new("sarah");
        let (url, _) = gate("dir-open", Some(Members::list(vec![sarah.id()])), vec![]).await;
        sarah.publish(&url).await;
        assert_eq!(read(&url, "/sarah", Ask::Nobody).await, 200);
        assert_eq!(read(&url, "/nobody", Ask::Nobody).await, 404);
        assert_eq!(read(&url, "", Ask::Nobody).await, 401);
        assert_eq!(read(&url, "", Ask::Bearer(sarah.token())).await, 200);
    }

    /// Settings on a page the app serves asks with the device's token:
    /// Storage, the house's boxes and adding a device took a browser's
    /// session alone, so the app's page was sent to sign in, which it
    /// cannot (and the box's sign-in page came back as a 502)
    #[tokio::test]
    async fn settings_are_answered_for_a_devices_token() {
        let sarah = Person::new("sarah");
        let (url, _) = gate(
            "device-settings",
            Some(Members::list(vec![sarah.id()])),
            vec![],
        )
        .await;
        sarah.publish(&url).await;
        let ask = |method: &'static str, path: &'static str, t: Option<String>| {
            let mut r = client().request(method.parse().unwrap(), format!("{url}{path}"));
            if let Some(t) = t {
                r = r.bearer_auth(t);
            }
            async move { r.send().await.unwrap().status().as_u16() }
        };
        for (method, path) in [
            ("GET", "/_dd/storage/mine"),
            ("GET", "/_dd/house"),
            ("POST", "/_dd/add/start"),
        ] {
            assert_eq!(ask(method, path, None).await, 401, "{path}");
            assert_eq!(ask(method, path, Some(sarah.token())).await, 200, "{path}");
        }
        // a token to read the directory with is not one for these
        let read = sarah.read_token("127.0.0.1");
        assert_eq!(ask("GET", "/_dd/storage/mine", Some(read)).await, 401);
    }

    #[tokio::test]
    async fn storage_tells_a_signed_in_stranger_403_not_401() {
        let (url, s) = gate("storage-403", Some(Members::list(vec![])), vec![]).await;
        let http = client();
        let mine = |c: Option<String>| {
            let mut r = http.get(format!("{url}/_dd/storage/mine"));
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.send().await.unwrap().status().as_u16() }
        };
        assert_eq!(mine(None).await, 401);
        assert_eq!(mine(Some(cookie(&s.issue("stranger")))).await, 403);
    }

    #[tokio::test]
    async fn a_members_home_controllers_are_theirs_alone_and_only_bytes_here() {
        let send = |method: &str, url: String, c: Option<String>, body: Vec<u8>| {
            let mut r = client().request(method.parse().unwrap(), url).body(body);
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.send().await.unwrap() }
        };
        let (url, s) = gate("controllers-stranger", Some(Members::list(vec![])), vec![]).await;
        let at = format!("{url}/_dd/controllers");
        assert_eq!(
            send("GET", at.clone(), None, vec![])
                .await
                .status()
                .as_u16(),
            401
        );
        let stranger = Some(cookie(&s.issue("stranger")));
        assert_eq!(
            send("PUT", at.clone(), stranger, b"x".to_vec())
                .await
                .status()
                .as_u16(),
            403
        );
        let (url, s) = gate("controllers", None, vec![]).await;
        let at = format!("{url}/_dd/controllers");
        let tom = || Some(cookie(&s.issue("tom")));
        assert_eq!(
            send("GET", at.clone(), tom(), vec![])
                .await
                .status()
                .as_u16(),
            404
        );
        let sealed = vec![7u8, 0, 255, 1];
        assert_eq!(
            send("PUT", at.clone(), tom(), sealed.clone())
                .await
                .status()
                .as_u16(),
            204
        );
        let back = send("GET", at.clone(), tom(), vec![]).await;
        assert_eq!(back.status().as_u16(), 200);
        assert_eq!(back.bytes().await.unwrap().to_vec(), sealed);
        // another member's is their own
        let ann = Some(cookie(&s.issue("ann")));
        assert_eq!(
            send("GET", at.clone(), ann, vec![]).await.status().as_u16(),
            404
        );
        assert_eq!(
            send("PUT", at, tom(), vec![0; 65 * 1024])
                .await
                .status()
                .as_u16(),
            413
        );
    }

    #[tokio::test]
    async fn a_home_controller_asks_with_its_key_and_its_members_word() {
        use controller::tests::{bucket, ctl, header, label_of};
        let (tom, ann, eve) = (Person::new("tom"), Person::new("ann"), Person::new("eve"));
        let members = || Some(Members::list(vec![tom.id(), ann.id()]));
        let (g, b) = bucket().await;
        let (url, s) = gate_full("controller", members(), vec![], vec![], Some(g)).await;
        for p in [&tom, &ann, &eve] {
            p.publish(&url).await;
        }
        let ctl = ctl();
        let label = &label_of(&ctl);
        let now = identity::now();
        let ask = |method: &str, path: &str, h: Option<String>| {
            let mut r = client().request(method.parse().unwrap(), format!("{url}{path}"));
            if let Some(h) = h {
                r = r.header(controller::HEADER, h);
            }
            async move { r.send().await.unwrap() }
        };
        let signed = |who: &Person, method: &str, path: &str, at: u64| {
            Some(header(
                who.name, &who.root, &ctl, label, DOMAIN, method, path, at,
            ))
        };
        let dir = "/_dd/controller/directory/ann";
        // one person's entry, never the list
        let r = ask("GET", dir, signed(&tom, "GET", dir, now)).await;
        assert_eq!(r.status(), 200);
        let e: identity::SignedEntry = r.json().await.unwrap();
        assert_eq!(e.entry.name, "ann");
        let list = "/_dd/controller/directory";
        assert_eq!(
            ask("GET", list, signed(&tom, "GET", list, now))
                .await
                .status(),
            404
        );
        let nobody = "/_dd/controller/directory/nobody";
        assert_eq!(
            ask("GET", nobody, signed(&tom, "GET", nobody, now))
                .await
                .status(),
            404
        );
        // refused: unsigned, stale, for another path, named by another root
        assert_eq!(ask("GET", dir, None).await.status(), 401);
        assert_eq!(ask("GET", dir, Some("x".into())).await.status(), 401);
        assert_eq!(
            ask("GET", dir, signed(&tom, "GET", dir, now - 400))
                .await
                .status(),
            401
        );
        let other = "/_dd/controller/directory/tom";
        assert_eq!(
            ask("GET", dir, signed(&tom, "GET", other, now))
                .await
                .status(),
            401
        );
        let forged = header("tom", &ann.root, &ctl, label, DOMAIN, "GET", dir, now);
        assert_eq!(ask("GET", dir, Some(forged)).await.status(), 401);
        // a stranger's word is good for nothing here
        assert_eq!(
            ask("GET", dir, signed(&eve, "GET", dir, now))
                .await
                .status(),
            403
        );

        // its backup: urls for its one object, ten minutes
        let up = "/_dd/controller/backup";
        let r = ask("POST", up, signed(&tom, "POST", up, now)).await;
        assert_eq!(r.status(), 200);
        let urls: serde_json::Value = r.json().await.unwrap();
        let object = format!("/b/controllers/{label}/backup?");
        for k in ["put", "get"] {
            let u = urls[k].as_str().unwrap();
            assert!(
                u.contains(&object) && u.contains("X-Amz-Expires=600"),
                "{u}"
            );
        }
        assert_eq!(
            ask("POST", up, signed(&tom, "POST", up, now))
                .await
                .status(),
            200
        );
        // the label is tom's controller's now: another's word for it is refused
        let theirs = header("ann", &ann.root, &ctl, label, DOMAIN, "POST", up, now);
        assert_eq!(ask("POST", up, Some(theirs)).await.status(), 409);
        // nor can a member claim it with a controller of their own: the
        // label is the key's, so their own root's word for it is no name
        let squat = identity::generate();
        let squatting = header("ann", &ann.root, &squat, label, DOMAIN, "POST", up, now);
        assert_eq!(ask("POST", up, Some(squatting)).await.status(), 401);
        assert_eq!(
            ask("POST", up, signed(&eve, "POST", up, now))
                .await
                .status(),
            403
        );

        // restoring: tom's, by session or device token
        let restore = format!("{url}/_dd/controller/backup/{label}");
        let get = |c: Option<String>, t: Option<String>, at: String| {
            let mut r = client().get(at);
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            if let Some(t) = t {
                r = r.bearer_auth(t);
            }
            async move { r.send().await.unwrap() }
        };
        let tom_c = || Some(cookie(&s.issue("tom")));
        assert_eq!(
            get(tom_c(), None, restore.clone()).await.status(),
            404,
            "nothing put yet"
        );
        b.lock()
            .unwrap()
            .insert(format!("controllers/{label}/backup"));
        let r = get(tom_c(), None, restore.clone()).await;
        assert_eq!(r.status(), 200);
        let v: serde_json::Value = r.json().await.unwrap();
        assert!(v["get"].as_str().unwrap().contains(&object));
        assert_eq!(
            get(None, Some(tom.token()), restore.clone()).await.status(),
            200
        );
        assert_eq!(get(None, None, restore.clone()).await.status(), 401);
        assert_eq!(
            get(Some(cookie(&s.issue("ann"))), None, restore.clone())
                .await
                .status(),
            404
        );
        assert_eq!(
            get(Some(cookie(&s.issue("eve"))), None, restore.clone())
                .await
                .status(),
            403
        );
        let unknown = format!("{url}/_dd/controller/backup/zyxwvutsrqponm76");
        assert_eq!(get(tom_c(), None, unknown).await.status(), 404);

        // a gate without the bucket has no backups at all
        let (bare, _) = gate("controller-bare", members(), vec![]).await;
        tom.publish(&bare).await;
        let r = client()
            .post(format!("{bare}{up}"))
            .header(controller::HEADER, signed(&tom, "POST", up, now).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404);
    }

    #[tokio::test]
    async fn a_passkeys_name_waits_for_its_own_member_once() {
        use controller::tests::{ctl, label_of};
        use ed25519_dalek::Signer as _;
        let (tom, ann) = (Person::new("tom"), Person::new("ann"));
        let (url, s) = gate(
            "controller-name",
            Some(Members::list(vec![tom.id(), ann.id()])),
            vec![],
        )
        .await;
        tom.publish(&url).await;
        ann.publish(&url).await;
        let key = identity::encode_public(&ctl().verifying_key());
        let label = label_of(&ctl());
        let at = format!("{url}/_dd/controller-name/{label}");
        let name = format!("{label}.home.{DOMAIN}");
        let said = |root: &identity::SigningKey| {
            B64.encode(
                root.sign(&identity::controller_name_message(&name, &key))
                    .to_bytes(),
            )
        };
        let page = client()
            .get(format!("{url}/_dd/controller-name"))
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), 200);
        assert!(
            page.text()
                .await
                .unwrap()
                .contains("/_dd/static/controller-name.js")
        );
        let home = format!("https://home.{DOMAIN}");
        let leave = |c: Option<String>, origin: Option<&str>, sig: String| {
            let mut r = client()
                .post(at.clone())
                .json(&serde_json::json!({ "sig": sig }));
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            if let Some(o) = origin {
                r = r.header("origin", o);
            }
            async move { r.send().await.unwrap().status().as_u16() }
        };
        let ask = |t: Option<String>, controller: &str| {
            let mut r = client()
                .put(at.clone())
                .json(&serde_json::json!({ "controller": controller }));
            if let Some(t) = t {
                r = r.bearer_auth(t);
            }
            async move { r.send().await.unwrap().status().as_u16() }
        };
        let asked = |c: Option<String>| {
            let mut r = client().get(format!("{at}/ask"));
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.send().await.unwrap() }
        };
        let tom_c = || Some(cookie(&s.issue("tom")));
        // nothing asked: a link alone gets no name signed or kept
        assert_eq!(asked(tom_c()).await.status(), 404);
        assert_eq!(leave(tom_c(), Some(&home), said(&tom.root)).await, 404);
        // only the member's app asks, and only for the label's own key
        assert_eq!(ask(None, &key).await, 401);
        assert_eq!(
            ask(tom_c(), &key).await,
            401,
            "a page's cookie is not the app"
        );
        let another = identity::encode_public(&identity::generate().verifying_key());
        assert_eq!(ask(Some(tom.token()), &another).await, 400);
        assert_eq!(ask(Some(tom.token()), &key).await, 204);
        // the page reads its own member's ask, nobody else's
        assert_eq!(asked(None).await.status(), 401);
        assert_eq!(asked(Some(cookie(&s.issue("ann")))).await.status(), 404);
        let r = asked(tom_c()).await;
        assert_eq!(r.status(), 200);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["controller"], key.as_str());

        assert_eq!(leave(None, None, said(&tom.root)).await, 401);
        assert_eq!(
            leave(tom_c(), Some("https://elsewhere.example"), said(&tom.root)).await,
            403
        );
        assert_eq!(leave(tom_c(), None, String::new()).await, 400);
        // not tom's root's word: not kept
        assert_eq!(leave(tom_c(), Some(&home), said(&ann.root)).await, 403);
        assert_eq!(leave(tom_c(), Some(&home), said(&tom.root)).await, 204);
        let take = |t: Option<String>, c: Option<String>| {
            let mut r = client().get(at.clone());
            if let Some(t) = t {
                r = r.bearer_auth(t);
            }
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.send().await.unwrap() }
        };
        assert_eq!(
            take(None, tom_c()).await.status(),
            401,
            "the app's token, not a page"
        );
        assert_eq!(
            take(Some(ann.token()), None).await.status(),
            404,
            "only their own"
        );
        let r = take(Some(tom.token()), None).await;
        assert_eq!(r.status(), 200);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["sig"], said(&tom.root));
        assert_eq!(take(Some(tom.token()), None).await.status(), 404, "once");
        // and the ask is used up with the answer
        assert_eq!(asked(tom_c()).await.status(), 404);
    }

    #[tokio::test]
    async fn a_librarys_notes_are_for_its_owner_and_readers_alone() {
        let lib = "0123456789abcdef0123456789abcdef";
        let at = |lib: &str, kind: &str| format!("/_dd/library/{lib}/notes/{kind}");
        let status = |method: &str, url: String, c: Option<String>| {
            let mut r = client().request(method.parse().unwrap(), url);
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.body("x").send().await.unwrap().status().as_u16() }
        };
        // a signed-up stranger is no member
        let (url, s) = gate("notes-stranger", Some(Members::list(vec![])), vec![]).await;
        let stranger = || Some(cookie(&s.issue("stranger")));
        let u = |p: String| format!("{url}{p}");
        assert_eq!(status("GET", u(at(lib, "progress")), None).await, 401);
        assert_eq!(status("GET", u(at(lib, "progress")), stranger()).await, 403);
        assert_eq!(status("PUT", u(at(lib, "shelf")), stranger()).await, 403);
        // a member whose library it is not
        let (url, s) = gate("notes", None, vec![]).await;
        let ask = |method: &str, path: String, who: Option<&str>| {
            status(
                method,
                format!("{url}{path}"),
                who.map(|w| cookie(&s.issue(w))),
            )
        };
        assert_eq!(ask("GET", at(lib, "progress"), Some("tom")).await, 403);
        assert_eq!(ask("PUT", at(lib, "progress"), Some("tom")).await, 403);
        assert_eq!(ask("GET", at("nothex", "shelf"), Some("tom")).await, 400);
        assert_eq!(ask("GET", at(lib, "names"), Some("tom")).await, 400);
        assert_eq!(ask("GET", at(lib, "..%2fshelf"), Some("tom")).await, 400);
    }

    #[test]
    fn usernames_are_filenames() {
        assert!(valid_user("david"));
        assert!(!valid_user("../etc"));
        assert!(!valid_user(".hidden"));
        assert!(!valid_user("Bad"));
    }

    #[tokio::test]
    async fn a_members_mail_is_theirs_alone() {
        let (tom, ann, sue, eve) = (
            Person::new("tom"),
            Person::new("ann"),
            Person::new("sue"),
            Person::new("eve"),
        );
        let mut members = Members::list(vec![tom.id(), ann.id()]);
        // sue was suspended just now: she may take hers, and add nothing
        members.revoked.push(sue.id());
        members.suspended.insert(sue.id(), "a test".into());
        members.removed.insert(sue.id(), identity::now());
        let (endpoint, _store) = mail::tests::fake_s3().await;
        let (url, s) = gate_mail(
            "mail",
            Some(members),
            vec![],
            vec![],
            None,
            Some(mail::tests::mail_at(&endpoint)),
        )
        .await;
        for p in [&tom, &ann, &sue, &eve] {
            p.publish(&url).await;
        }
        let as_ = |who: &str| Some(cookie(&s.issue(who)));
        let send = |method: &str, path: &str, c: Option<String>, body: Vec<u8>| {
            let mut r = client()
                .request(method.parse().unwrap(), format!("{url}{path}"))
                .body(body);
            if let Some(c) = c {
                r = r.header("cookie", c);
            }
            async move { r.send().await.unwrap() }
        };
        let id = "0mvd7o78x0n2r1k185a2v";
        let at = format!("/_dd/mail/o/in/{id}");
        assert_eq!(send("GET", &at, None, vec![]).await.status(), 401);
        assert_eq!(
            send("PUT", &at, as_("tom"), b"locked".to_vec())
                .await
                .status(),
            204
        );
        let r = send("GET", &at, as_("tom"), vec![]).await;
        assert_eq!(r.status(), 200);
        assert_eq!(r.bytes().await.unwrap().as_ref(), b"locked");
        // another member's are under their own name: not tom's
        assert_eq!(send("GET", &at, as_("ann"), vec![]).await.status(), 404);
        let list: serde_json::Value = send("GET", "/_dd/mail/list/in", as_("ann"), vec![])
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(list, serde_json::json!([]));
        let list: serde_json::Value = send("GET", "/_dd/mail/list/in", as_("tom"), vec![])
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(list[0]["id"], id);
        assert_eq!(send("DELETE", &at, as_("ann"), vec![]).await.status(), 204);
        assert_eq!(
            send("GET", &at, as_("tom"), vec![]).await.status(),
            200,
            "ann deleting hers leaves tom's"
        );
        // suspended: reads her own, writes nothing; a stranger nothing at all
        assert_eq!(
            send("GET", "/_dd/mail/list/in", as_("sue"), vec![])
                .await
                .status(),
            200
        );
        assert_eq!(
            send("PUT", &at, as_("sue"), b"x".to_vec()).await.status(),
            403
        );
        assert_eq!(
            send("GET", "/_dd/mail/list/in", as_("eve"), vec![])
                .await
                .status(),
            403
        );
        assert_eq!(
            send("PUT", "/_dd/mail/o/secrets/x", as_("tom"), b"x".to_vec())
                .await
                .status(),
            400
        );
        assert_eq!(
            send(
                "PUT",
                &format!("/_dd/mail/o/in/..%2Fann%2Fin%2F{id}"),
                as_("tom"),
                b"x".to_vec()
            )
            .await
            .status(),
            400
        );
        // to another member: only once they have Mail on, and never over an id
        let to_ann = format!("/_dd/mail/deliver/ann/{id}");
        assert_eq!(
            send("POST", &to_ann, as_("tom"), b"locked to ann".to_vec())
                .await
                .status(),
            404
        );
        assert_eq!(
            send("PUT", "/_dd/mail/o/key/doc", as_("ann"), b"{}".to_vec())
                .await
                .status(),
            204
        );
        assert_eq!(
            send("POST", &to_ann, as_("tom"), b"locked to ann".to_vec())
                .await
                .status(),
            204
        );
        assert_eq!(
            send("POST", &to_ann, as_("tom"), b"locked again!".to_vec())
                .await
                .status(),
            409
        );
        let r = send("GET", &at, as_("ann"), vec![]).await;
        assert_eq!(r.headers().get("x-dd-from").unwrap(), "tom");
        assert_eq!(r.bytes().await.unwrap().as_ref(), b"locked to ann");
        assert_eq!(
            send(
                "POST",
                "/_dd/mail/deliver/eve/0mvd7o78x0n2r1k185a2w",
                as_("tom"),
                b"locked x".to_vec()
            )
            .await
            .status(),
            404
        );
        assert_eq!(
            send("POST", &to_ann, as_("sue"), b"locked to ann".to_vec())
                .await
                .status(),
            403
        );
        // the pipe: the app's alone, never a page's
        let pipe = client()
            .get(format!(
                "{url}/_dd/mail/pipe?domain=example.com&host=mx.example.com"
            ))
            .header("cookie", as_("tom").unwrap())
            .header("origin", "https://evil.example")
            .header("connection", "Upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", B64.encode([7u8; 16]))
            .send()
            .await
            .unwrap();
        assert_eq!(pipe.status(), 403);
    }
}
