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
mod demo_photos;
mod directory;
pub mod fleet;
mod forge_events;
mod friends;
pub mod library;
pub mod network;
mod oidc;
pub mod pages;
mod photos;
mod session;
mod storage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::AUTHORIZATION},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
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
    /// the encrypted libraries' gate (library.rs), on a box with the bucket
    library: Option<library::Gate>,
    network: Option<network::Door>,
    demo_library: Option<(String, String)>,
    app_manifest: Option<String>,
    tmdb: Option<String>,
    search: Option<String>,
    /// the app manifest as last read, and when: a good one for ten
    /// minutes, the lack of one for one
    app_seen: Mutex<Option<(Instant, Option<SignedApp>)>>,
    fleet: fleet::Fleet,
    thanos: Option<String>,
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
    JoinSign { entry: identity::Entry },
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
    /// the other boxes' directory urls
    pub peers: Vec<String>,
    pub sync_secs: u64,
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
    pub network: Option<network::Door>,
    /// The library the demo account reads: an id and its key, both in the
    /// open on purpose (see the option in modules/library/libraries.nix).
    pub demo_library: Option<(String, String)>,
    /// Where the signed app manifest is published (the releases branch,
    /// beside the box releases). The downloads page offers what it names,
    /// and nothing else. None: no page.
    pub app_manifest: Option<String>,
    /// Every box in the fleet, for the Boxes and Backups pages. Empty on a
    /// box that is not told.
    pub fleet: fleet::Fleet,
    /// Thanos on this box, which holds every box's facts (fleet.rs). None:
    /// this box does not gather them, and every box shows as unknown.
    pub thanos: Option<String>,
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
fn asked_host(headers: &HeaderMap) -> String {
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
}

impl Members {
    pub fn list(members: Vec<String>) -> Self {
        Self {
            members,
            revoked: vec![],
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
            },
        }
        Ok(match serde_json::from_str::<File>(json)? {
            File::List(members) => Self::list(members),
            File::Full { members, revoked } => Self { members, revoked },
        })
    }
}

pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect: String,
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
            let raw = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .ok()?
                .get(url)
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

    /// on the member list by root, and not shut out
    fn listed(&self, root: &str) -> bool {
        let id = identity::member_id(root);
        self.members
            .as_ref()
            .is_some_and(|m| m.members.contains(&id) && !m.revoked.contains(&id))
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

    /// they showed their passkey just now: for five minutes, what asks for
    /// that may go ahead
    fn saw_passkey(&self, user: &str) {
        let now = session::now();
        let mut f = self.fresh.lock().unwrap();
        f.retain(|_, until| *until > now);
        f.insert(user.to_string(), now + 300);
    }

    fn passkey_fresh(&self, user: &str) -> bool {
        self.fresh
            .lock()
            .unwrap()
            .get(user)
            .is_some_and(|until| *until > session::now())
    }

    fn member(&self, user: &str) -> bool {
        if user == pages::DEMO_USER {
            return self.demo();
        }
        let Some(m) = &self.members else {
            return true;
        };
        let Some(e) = self.entry(user).ok().flatten() else {
            return false;
        };
        let id = identity::member_id(&e.entry.root);
        if m.revoked.contains(&id) {
            return false;
        }
        if m.members.contains(&id) {
            return true;
        }
        // an invite the owner signed, redeemed by this root
        e.entry.grant.is_some()
            && self
                .directory
                .release()
                .is_some_and(|r| identity::verify_grant(&e.entry, r).is_ok())
    }

    /// Someone who came in through a friend link and is not a member: let
    /// in to the game servers they are invited to and nowhere else, and
    /// only while a member is still their friend. A guest made a member
    /// is a member.
    fn guest(&self, user: &str) -> bool {
        user != pages::DEMO_USER
            && !self.member(user)
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
        } else {
            None
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
        let unverified = UnverifiedBiscuit::from_base64(token).context("not a biscuit")?;
        // the user is named in the authority block; it has to be read before the
        // signature can be checked, because the key to check with depends on it
        let source = unverified
            .print_block_source(0)
            .context("unreadable authority block")?;
        let user = peek_user(&source).context("no user fact")?;
        anyhow::ensure!(valid_user(&user), "bad user in token");
        let signed = self
            .entry(&user)?
            .context("no identity published for that name")?;
        let mut verified: Option<Biscuit> = None;
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
                break;
            }
        }
        let biscuit = verified.ok_or_else(|| anyhow!("signature matches no device of {user}"))?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
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
        Ok(user)
    }
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
    /// own session cookie. Nothing else counts.
    fn identify(&self, headers: &HeaderMap, operation: &str) -> Option<String> {
        if let Some(tok) = bearer(headers)
            && UnverifiedBiscuit::from_base64(tok).is_ok()
        {
            return match self.verify_biscuit_for(tok, operation, &asked_host(headers)) {
                Ok(u) => Some(u),
                Err(e) => {
                    eprintln!("biscuit refused: {e:#}");
                    None
                }
            };
        }
        let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
        self.sessions.user(cookie)
    }
}

/// `user("david")` out of a block's datalog source, without a parser.
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
async fn verify(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    // A demo door (demo-<service>.<domain>, modules/gate/public.nix) is on
    // the open internet so anyone can look around. It lets the demo in and
    // nobody else: nothing of a member's is served through it, and the
    // services' own names stay on the private network. Nobody at all is a
    // 403 there, not a 401: some doors let a 401 through as anonymous (the
    // forge's public repos), which on this name would be the whole internet.
    let demo_door = asked_host(&headers).starts_with("demo-");
    let Some(user) = app.identify(&headers, "access") else {
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
        Some(pages::Role::Demo) => app.demo_allows(&headers),
        // a guest reaches the games page and nothing else, whatever the
        // service behind another door would have made of them
        Some(pages::Role::Guest) => !demo_door && app.guest_host(&headers),
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
    // A disk image goes from the Backups page only after the passkey, and
    // the box holds the page to that: a tab left signed in is not enough.
    // `dd image delete` signs with the device's own key and asks itself.
    if bearer(&headers).is_none() && deletes_image(&headers) && !app.passkey_fresh(&user) {
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

fn deletes_image(headers: &HeaderMap) -> bool {
    let h = |k| headers.get(k).and_then(|v: &HeaderValue| v.to_str().ok());
    h("x-original-method") == Some("DELETE")
        && h("x-original-uri").is_some_and(|u| u.starts_with("/images/"))
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
        Some(_) => Redirect::to(&format!("https://{service}{path}")).into_response(),
        // the static files a page needs; everything else starts the demo
        None if path.starts_with("/_dd/static/") || path.starts_with("/_dd/web/") => {
            next.run(req).await
        }
        None => Redirect::to(&format!("https://home.{}/_dd/demo", app.domain)).into_response(),
    }
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

/// what the browser sends when it has finished making a passkey: the
/// credential, and the public half of the key it derived from that
/// passkey's own PRF secret, so libraries can be sealed to it
#[derive(Deserialize)]
struct Enrolled {
    #[serde(flatten)]
    credential: RegisterPublicKeyCredential,
    /// base64 ed25519 public key; absent where the browser has no PRF
    #[serde(default)]
    library_key: Option<String>,
}

async fn enrol_finish(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(body): Json<Enrolled>,
) -> Response {
    let Enrolled {
        credential: reg,
        library_key,
    } = body;
    if let Some(k) = &library_key
        && identity::decode_public(k).is_err()
    {
        return (StatusCode::BAD_REQUEST, "that is not a public key").into_response();
    }
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
                library_key: library_key.clone(),
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
async fn join_start(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(q): Json<LoginStart>,
) -> Response {
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
    let Enrolled {
        credential: reg,
        library_key,
    } = body;
    if let Some(k) = &library_key
        && identity::decode_public(k).is_err()
    {
        return (StatusCode::BAD_REQUEST, "that is not a public key").into_response();
    }
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
    // an invite code typed on the join page: the browser proved it for this
    // root; the invite itself is looked up here, and checked at admission
    let grant = match grant_claim(&app, &headers) {
        Ok(g) => g,
        Err(r) => return r.into_response(),
    };
    let entry = identity::Entry {
        name: user,
        root,
        recovery: String::new(),
        devices: vec![],
        passkeys: vec![identity::Passkey {
            id: id.clone(),
            cred,
            added: now,
            library_key: library_key.clone(),
            rp_id: Some(app.domain.clone()),
        }],
        grant,
        libraries: vec![],
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
    let ceremony = app.ceremony_put(Ceremony::JoinSign { entry });
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

/// What the browser sends for a code: the invite's public key it derived,
/// and its proof for this root. Base64 json in the x-dd-grant header.
#[derive(Deserialize)]
struct GrantClaim {
    invite_public_key: String,
    redeemed: u64,
    proof: String,
}

fn grant_claim(
    app: &App,
    headers: &HeaderMap,
) -> std::result::Result<Option<identity::Grant>, (StatusCode, &'static str)> {
    let Some(h) = headers.get("x-dd-grant").and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let claim: GrantClaim = B64
        .decode(h)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or((StatusCode::BAD_REQUEST, "bad invite claim"))?;
    let invite = app.directory.invite(&claim.invite_public_key).ok_or((
        StatusCode::NOT_FOUND,
        "that code is not valid here, or it has expired",
    ))?;
    Ok(Some(identity::Grant {
        invite,
        redeemed: claim.redeemed,
        proof: claim.proof,
    }))
}

/// A person with an account and a code: their entry gets the grant, signed
/// by their passkey like any update. Only for a passkey root; a root held
/// by `dd` redeems there.
async fn redeem_start(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let Some(user) = app.sessions.user(cookie) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(signed) = app.entry(&user).ok().flatten() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !signed.entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        return (
            StatusCode::BAD_REQUEST,
            "this account's key is on a device: run `dd invite redeem` there",
        )
            .into_response();
    }
    let grant = match grant_claim(&app, &headers) {
        Ok(Some(g)) => g,
        Ok(None) => return (StatusCode::BAD_REQUEST, "no code").into_response(),
        Err(r) => return r.into_response(),
    };
    let mut entry = signed.entry;
    entry.grant = Some(grant);
    entry.version += 1;
    entry.updated = identity::now();
    let challenge = match identity::challenge(&entry) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("redeem: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let allow: Vec<serde_json::Value> = entry
        .passkeys
        .iter()
        .map(|p| serde_json::json!({ "type": "public-key", "id": p.id }))
        .collect();
    let ceremony = app.ceremony_put(Ceremony::JoinSign { entry });
    Json(serde_json::json!({
        "ceremony": ceremony,
        "publicKey": {
            "challenge": B64_URL.encode(challenge),
            "timeout": 60000,
            "rpId": app.domain,
            "allowCredentials": allow,
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
    // With a code, or already named by the member list, it is an entry and
    // goes to the directory like any other. Without either it is held on
    // this box until it follows through - the same page either way, the
    // same sign-in, the same waiting screen for someone not yet in.
    // The accept rule checks the assertion in both.
    let earned = signed.entry.grant.is_some() || app.listed(&signed.entry.root);
    if earned {
        if let Err((status, why)) = app.directory.admit(signed).await {
            return (status, why).into_response();
        }
        app.directory.unhold(&user);
    } else if let Err((status, why)) = app.directory.hold(&signed) {
        return (status, why).into_response();
    }
    // the entry was just signed with the passkey made a moment ago
    app.saw_passkey(&user);
    let mut r = Json(serde_json::json!({ "user": user })).into_response();
    r.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&app.sessions.issue(&user)).unwrap(),
    );
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
    app.saw_passkey(&user);
    let mut r = StatusCode::OK.into_response();
    r.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&app.sessions.issue(&user)).unwrap(),
    );
    r
}

/// The signed-in front door. A browser without a session goes to the
/// login page and comes back here.
async fn home_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
        Some(user) if app.role(&user).is_none() => {
            Html(pages::waiting(&user, &app.home)).into_response()
        }
        Some(_) => page("home"),
        None => Redirect::to("/_dd/login?rd=/").into_response(),
    }
}

/// A look without a key: a session as the demo account, which every box
/// treats as a member with nothing of its own. Each service decides what
/// the demo may do there (nginx, by the username the verifier reports).
async fn demo(State(app): State<Arc<App>>) -> Response {
    if !app.demo() {
        return (StatusCode::NOT_FOUND, "no demo here").into_response();
    }
    let mut r = Redirect::to("/_dd/home").into_response();
    r.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&app.sessions.issue(pages::DEMO_USER)).unwrap(),
    );
    r
}

/// The pages' own stylesheet and scripts (pages::static_file).
async fn static_file(axum::extract::Path(file): axum::extract::Path<String>) -> Response {
    match pages::static_file(&file) {
        Some((body, ty)) => {
            // a font is the same bytes until a release changes its name
            let cache = if ty.starts_with("font/") {
                "public, max-age=604800"
            } else {
                "no-cache"
            };
            ([("content-type", ty), ("cache-control", cache)], body).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The browser-side Rust, as wasm-bindgen laid it out: a .js and a .wasm.
async fn web_file(
    State(app): State<Arc<App>>,
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
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match std::fs::read(dir.join(&file)) {
        Ok(b) => ([("content-type", ty), ("cache-control", "no-cache")], b).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The one thing every page needs to talk to a passkey: which domain the
/// credentials belong to. Public, and true of the box either way.
async fn page_config(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let mut cfg = serde_json::json!({ "rpId": app.domain.clone() });
    // The demo has no passkey, because a passkey lives in one browser on
    // one device and the demo is one account every visitor shares. Its
    // library key comes from the box instead, the way the demo's photos
    // password already does. Nothing is given away: the library holds
    // nothing private, and anyone at all may be the demo.
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    if app.sessions.user(cookie).as_deref() == Some(pages::DEMO_USER)
        && let Some((id, key)) = &app.demo_library
    {
        cfg["demoLibrary"] = serde_json::json!({ "id": id, "key": key });
    }
    Json(cfg).into_response()
}

/// Files: a member's library, opened in the browser by their passkey. The
/// demo has no passkey and no library, so it does not come here.
async fn files_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
        // the demo comes here too when the box keeps a library for it: it
        // reads that one and nothing else, and the page hides every
        // control a reader has no use for
        Some(user) if user == pages::DEMO_USER && app.demo_library.is_some() => page("files"),
        Some(user) if app.member(&user) && user != pages::DEMO_USER => page("files"),
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None => Redirect::to("/_dd/login?rd=/_dd/files").into_response(),
    }
}

/// Movies & TV: the library's Movies and Shows, opened by the passkey.
async fn media_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
        Some(user) if user == pages::DEMO_USER && app.demo_library.is_some() => page("media"),
        Some(user) if app.member(&user) && user != pages::DEMO_USER => page("media"),
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None => Redirect::to("/_dd/login?rd=/_dd/media").into_response(),
    }
}

/// A signed-in page, as the file it is (pages::page): it fills itself in
/// from /_dd/me, in a browser and in the app alike.
pub(crate) fn page(name: &str) -> Response {
    match pages::page(name) {
        Some(html) => ([("cache-control", "no-cache")], Html(html)).into_response(),
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

/// Chat (box/chat): a member's, or the demo's with its counted prompts.
/// The page is the same for both; what differs is where it keeps chats.
async fn chat_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
        Some(user)
            if matches!(
                app.role(&user),
                Some(pages::Role::Member | pages::Role::Demo)
            ) =>
        {
            page("chat")
        }
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None => Redirect::to("/_dd/login?rd=/").into_response(),
    }
}

/// Git (box/forge): our pages over the forge, for anyone signed in. The
/// forge's API decides what each of them may see and do; the page is the
/// same file for all.
async fn git_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    signed_in_page(&app, &headers, "git")
}

/// Metrics (box/observe): the fleet's figures, for anyone signed in; the
/// queries behind it pass the same gate.
async fn metrics_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    signed_in_page(&app, &headers, "metrics")
}

fn signed_in_page(app: &App, headers: &HeaderMap, name: &str) -> Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
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
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
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
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .build()?;
        let v: serde_json::Value = client
            .get(format!("{base}/search"))
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
            eprintln!("verify: search: {e:#}");
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
    match app.sessions.user(cookie) {
        Some(user) if app.member(&user) && user != pages::DEMO_USER => page(name),
        // a guest has devices and joins the network like anyone, to reach
        // the game servers they are invited to
        Some(user) if matches!(name, "devices" | "network") && app.guest(&user) => page(name),
        Some(_) => Redirect::to("/_dd/home").into_response(),
        None => Redirect::to(&format!("/_dd/login?rd={at}")).into_response(),
    }
}

/// What every box is running and how its last backup went. Read from each
/// box's own prometheus when the page asks, never stored here.
async fn fleet_json(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match app.identify(&headers, "access") {
        Some(user) if app.member(&user) && user != pages::DEMO_USER => {
            Json(fleet::look(&app.fleet, app.thanos.as_deref()).await).into_response()
        }
        _ => StatusCode::FORBIDDEN.into_response(),
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

/// The member's own machines on the fleet's network.
async fn network_mine(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if !(app.member(&user) || app.guest(&user)) || user == pages::DEMO_USER {
        return StatusCode::FORBIDDEN.into_response();
    }
    match &app.network {
        // the control server is on one box; elsewhere the page says so
        None => Json(serde_json::json!({ "here": false, "machines": [] })).into_response(),
        Some(door) => match door.mine(&user).await {
            Ok(m) => Json(serde_json::json!({ "here": true, "machines": m })).into_response(),
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        },
    }
}

/// Signing out: the session goes, and so does what the photo app keeps in
/// this browser - its list of the library and the pictures it unlocked. That
/// belongs to the photos host, so the way out passes through there
/// (photos_forget), and on to the front door: signing in again lands on the
/// home page, not on whichever service the person signed out from.
async fn logout(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let home = format!("home.{}", app.domain);
    let back = fleet_host(&app.domain, &home).then(|| format!("https://{home}/"));
    // the demo's photos are the gate's own page, never the photo app, and
    // the photos host is on the private network where the demo cannot go
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    let demo = app.sessions.user(cookie).as_deref() == Some(pages::DEMO_USER);
    let to = match (photos::origin(&app.home), back) {
        (_, Some(b)) if demo => b,
        (Some(p), Some(b)) => format!("{p}/_dd/photos/forget?then={b}"),
        _ => "/".to_string(),
    };
    let mut r = Redirect::to(&to).into_response();
    r.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&app.sessions.clear()).unwrap(),
    );
    r
}

/// one of this fleet's own names, and nothing that could carry more
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
    let Some(user) = app.sessions.user(cookie) else {
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
    let directory = Arc::new(directory::Directory::open(
        cfg.dir.clone(),
        cfg.peers.clone(),
        release,
    )?);
    tokio::spawn(directory.clone().sync_forever(cfg.sync_secs));
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    let addr = listener.local_addr()?;
    // a box with nothing else on it: no domain, no sessions, no secrets. It
    // serves entries and accepts the ones that verify, and that is all.
    let Some(domain) = cfg.domain else {
        let router = Router::new()
            .route("/health", get(|| async { "ok" }))
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
    // every service on this box, and the cookie rides along to all of them
    let rp_origin = Url::parse(&format!("https://{domain}"))?;
    let webauthn = WebauthnBuilder::new(&domain, &rp_origin)?
        .rp_name("Commonty")
        .allow_subdomains(true)
        .build()?;
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
    let app = Arc::new(App {
        directory: directory.clone(),
        sessions: session::Sessions::open(&state_dir, &domain)?,
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
        library: cfg.library,
        network: cfg.network,
        demo_library: cfg.demo_library,
        app_manifest: cfg.app_manifest,
        tmdb: cfg.tmdb,
        search: cfg.search,
        app_seen: Mutex::new(None),
        fleet: cfg.fleet,
        thanos: cfg.thanos,
        adblock: cfg.adblock,
        forge_events: cfg.forge_events.map(forge_events::ForgeEvents::start),
        demo_photos: Default::default(),
        storage: storage::Ledger::new(cfg.storage_ledger.clone()),
        fresh: Mutex::new(HashMap::new()),
        friends: friends::Store::open(&state_dir)?,
    });
    // A held sign-up follows through when the member list names it: then it
    // is published like any entry, and the hold goes. (The other way, a code
    // redeemed on the waiting page, publishes it there and then.) Reading
    // each one also drops those past their time.
    {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                for name in a.directory.held_names() {
                    let Some(h) = a.directory.held(&name) else {
                        continue;
                    };
                    if !a.listed(&h.entry.root) {
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
    // Devices of people who are no longer members come off the network. An
    // empty member list is a broken release rather than everyone revoked,
    // and it would take every device off at once, so that is left alone.
    if app.network.is_some() {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                if let Some(door) = &a.network
                    && a.members.as_ref().is_some_and(|m| !m.members.is_empty())
                    && let Err(e) = door
                        .reap(|u| (a.member(u) || a.guest(u)) && u != pages::DEMO_USER)
                        .await
                {
                    eprintln!("network: reaping: {e:#}");
                }
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }
    // what was trashed three months ago is gone for good; daily
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
    }
    let router = Router::new()
        .route("/verify", get(verify))
        .route("/health", get(|| async { "ok" }))
        // the browser side, served under /_dd/ on every vhost
        .route(
            "/_dd/login",
            get(|State(a): State<Arc<App>>| async move { Html(pages::login(&a.domain)) }),
        )
        .route("/_dd/login/start", post(login_start))
        .route("/_dd/login/finish", post(login_finish))
        .route("/_dd/logout", get(logout))
        .route("/_dd/home", get(home_page))
        .route("/_dd/enrol", get(|| async { Html(pages::enrol()) }))
        .route(
            "/_dd/join",
            get(|State(a): State<Arc<App>>| async move { Html(pages::join(&a.domain)) }),
        )
        .route("/_dd/join/start", post(join_start))
        .route("/_dd/join/finish", post(join_finish))
        .route("/_dd/join/sign", post(join_sign))
        .route("/_dd/redeem/start", post(redeem_start))
        .route("/_dd/redeem/sign", post(join_sign))
        .route("/_dd/demo", get(demo))
        .route("/_dd/web/{file}", get(web_file))
        .route("/_dd/static/{file}", get(static_file))
        // what any page needs before it can ask a passkey for anything
        .route("/_dd/config", get(page_config))
        .route("/_dd/files", get(files_page))
        .route("/_dd/media", get(media_page))
        .route(
            "/_dd/boxes",
            get(|State(a): State<Arc<App>>, h: HeaderMap| async move {
                member_page(&a, &h, "boxes", "/_dd/boxes").await
            }),
        )
        // Backups is a tab of Settings now
        .route(
            "/_dd/backups",
            get(|| async { Redirect::to("/_dd/settings#backups") }),
        )
        .route(
            "/_dd/devices",
            get(|State(a): State<Arc<App>>, h: HeaderMap| async move {
                member_page(&a, &h, "devices", "/_dd/devices").await
            }),
        )
        .route(
            "/_dd/network",
            get(|State(a): State<Arc<App>>, h: HeaderMap| async move {
                member_page(&a, &h, "network", "/_dd/network").await
            }),
        )
        .route("/_dd/download", get(download_page))
        .route("/_dd/fleet.json", get(fleet_json))
        .route("/_dd/me", get(me))
        .route("/_dd/network/mine", get(network_mine))
        .route("/_dd/photos", get(photos::page))
        .route("/_dd/photos/config", post(photos::config))
        .route(
            "/_dd/settings",
            get(|State(a): State<Arc<App>>, h: HeaderMap| async move {
                member_page(&a, &h, "settings", "/_dd/settings").await
            }),
        )
        .route("/_dd/storage", get(storage::page))
        .route("/_dd/storage/mine", get(storage::mine))
        .route("/internal/storage/members", get(storage::members))
        .route("/_dd/photos/demo", get(demo_photos::page))
        .route("/_dd/photos/demo/list", get(demo_photos::list))
        .route("/_dd/photos/demo/thumb/{id}", get(demo_photos::thumb))
        .route("/_dd/photos/demo/photo/{id}", get(demo_photos::photo))
        .route("/_dd/csp", post(csp_report))
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
        .route("/_dd/chat", get(chat_page))
        .route("/_dd/git", get(git_page))
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
        .route("/internal/games/access", post(friends::game_access))
        .route("/internal/games/friends/{user}", get(friends::game_friends))
        // the encrypted libraries' gate: WebDAV over each library's prefix
        .route("/_dd/dav/{lib}", axum::routing::any(library::handle_root))
        .route("/_dd/dav/{lib}/", axum::routing::any(library::handle_root))
        .route(
            "/_dd/dav/{lib}/{*path}",
            axum::routing::any(library::handle),
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
        .layer(axum::middleware::from_fn_with_state(app.clone(), demo_door))
        .with_state(app)
        .merge(directory::router(directory));
    eprintln!("verify listening on {addr}");
    let task = tokio::spawn(async move { axum::serve(listener, router).await.map_err(Into::into) });
    Ok((addr, task))
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(!deletes_image(&h("PUT", "/images/locks/ab")));
        assert!(!deletes_image(&h("DELETE", "/_dd/dav/x")));
    }

    #[test]
    fn usernames_are_filenames() {
        assert!(valid_user("david"));
        assert!(!valid_user("../etc"));
        assert!(!valid_user(".hidden"));
        assert!(!valid_user("Bad"));
    }
}
