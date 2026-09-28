//! Game servers a member starts for themselves.
//!
//! The release carries a catalogue of games and one guest machine
//! (modules/games.nix). That a particular server exists is not in any
//! release: it is a record in this box's state, made here at a member's
//! request, and the unit that runs it is an instance of a template
//! (`dd-game@<id>`). This service makes and removes those records, starts
//! and stops those units, and on its own start brings back every server
//! whose record says it should be up - so a reboot does not end a world, and
//! a deploy, which never touches the template's instances, does not either.
//!
//! Who is asking comes from the gate: nginx sets `x-dd-user` from the
//! verifier's answer. What the demo may do here (look) is the gate's rule,
//! not this service's.

pub mod pages;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::OpenOptionsExt as _;

/// One game the catalogue knows: a pelican egg, resolved (games/resolve.py).
/// The guest does what the egg's daemon would: install with steamcmd, fill
/// the startup line from the settings, rewrite the files it names, watch
/// for the ready line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// the dedicated server's steam app
    pub app: u64,
    /// the game's own steam app: what the cover is of
    #[serde(default)]
    pub game: Option<u64>,
    #[serde(default)]
    pub cover: bool,
    #[serde(default)]
    pub wine: bool,
    pub startup: String,
    #[serde(default)]
    pub ready: Option<String>,
    #[serde(default = "ctrl_c")]
    pub stop: String,
    #[serde(default)]
    pub files: serde_json::Value,
    pub install: String,
    #[serde(default)]
    pub settings: Vec<Setting>,
    /// variables that are ports: each gets one from the pool
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub port_defaults: BTreeMap<String, String>,
}
fn ctrl_c() -> String {
    "^C".into()
}

impl Game {
    /// MiB: nothing in an egg says; wine servers and the big builders want more
    pub fn memory(&self) -> u64 {
        if self.wine { 8192 } else { 6144 }
    }
    pub fn cores(&self) -> u32 {
        4
    }
    /// what a person fills in: our few by our names, then the egg's own
    pub fn visible_settings(&self) -> impl Iterator<Item = &Setting> {
        self.settings.iter().filter(|s| s.editable)
    }
}

/// One thing a person may set, by the egg's variable and our label for it
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setting {
    pub var: String,
    pub label: String,
    #[serde(default)]
    pub default: String,
    #[serde(default = "text")]
    pub kind: String,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub editable: bool,
    /// our short name for a common one: name, password, players, world
    #[serde(default)]
    pub ours: Option<String>,
    #[serde(default)]
    pub choices: Option<Vec<String>>,
}
fn text() -> String {
    "text".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalogue {
    pub games: Vec<Game>,
}

/// a port from the pool, forwarded both ways: game ports are udp and tcp
/// alike more often than not, and the guest listens on the same number
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Port {
    /// the egg's variable it fills (SERVER_PORT, QUERY_PORT, ...)
    pub var: String,
    pub port: u16,
}

/// One server: what the guest needs to be it (the runner and the guest read
/// this same file), whose it is, and whether it is meant to be up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Instance {
    pub id: String,
    pub game: String,
    pub owner: String,
    pub memory: u64,
    pub cores: u32,
    pub ports: Vec<Port>,
    /// the egg's variables as this server has them: settings and ports
    pub env: BTreeMap<String, String>,
    /// the recipe, so the guest needs nothing but this file
    pub recipe: Recipe,
    /// "running" or "stopped": what a reboot restores
    pub desired: String,
    pub created: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Recipe {
    pub app: u64,
    pub wine: bool,
    pub startup: String,
    pub ready: Option<String>,
    pub stop: String,
    pub files: serde_json::Value,
    pub install: String,
}

/// A world kept apart from its server: what the game wrote, and the
/// settings it ran with, under the owner's name
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct World {
    pub name: String,
    pub game: String,
    pub owner: String,
    pub env: BTreeMap<String, String>,
    pub kept: u64,
    pub files: usize,
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    for p in walk(from) {
        let rel = p.strip_prefix(from).unwrap_or(&p);
        let dest = to.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // the tree came from the guest: a link in it is the guest asking
        // the host to read whatever it points at
        if p.symlink_metadata().map(|m| m.is_file()).unwrap_or(false) {
            std::fs::copy(&p, &dest)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Starting,
    Updating,
    Running,
    Failed,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Stopped => "stopped",
            State::Starting => "starting",
            State::Updating => "downloading the game",
            State::Running => "running",
            State::Failed => "failed",
        }
    }
}

/// systemd, as far as this service needs it; the tests have their own.
pub trait Units: Send + Sync {
    fn start(&self, id: &str) -> Result<()>;
    fn stop(&self, id: &str) -> Result<()>;
    /// (active, failed)
    fn state(&self, id: &str) -> (bool, bool);
}

pub struct Systemd;

impl Systemd {
    fn ctl(args: &[&str]) -> Result<std::process::Output> {
        std::process::Command::new("systemctl")
            .args(args)
            .output()
            .context("systemctl")
    }
}

impl Units for Systemd {
    fn start(&self, id: &str) -> Result<()> {
        // --no-block: the guest takes a while; the page shows its progress
        let o = Self::ctl(&["start", "--no-block", &format!("dd-game@{id}.service")])?;
        anyhow::ensure!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        Ok(())
    }
    fn stop(&self, id: &str) -> Result<()> {
        let o = Self::ctl(&["stop", "--no-block", &format!("dd-game@{id}.service")])?;
        anyhow::ensure!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        Ok(())
    }
    fn state(&self, id: &str) -> (bool, bool) {
        let unit = format!("dd-game@{id}.service");
        let is = |what: &str| {
            Self::ctl(&[what, "--quiet", &unit])
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        (is("is-active"), is("is-failed"))
    }
}

pub struct Config {
    pub dir: PathBuf,
    pub catalogue: BTreeMap<String, Game>,
    /// first host port handed out; each instance port gets the next free one
    pub port_base: u16,
    pub port_count: u16,
    /// servers one member may have up at once
    pub per_member: usize,
    /// MiB every running guest together may have
    pub memory_budget: u64,
    /// the box's cpus: a guest gets no more
    pub cores: u32,
    /// the address players type, shown with the port
    pub address: String,
    pub home: String,
    /// where the firewall reads who may reach which ports (write_access)
    pub access: Option<PathBuf>,
    /// the box's address on the fleet's network, as the join keeper last
    /// wrote it; `address` until it has
    pub address_file: Option<PathBuf>,
}

pub struct Manager {
    pub cfg: Config,
    units: Box<dyn Units>,
    // creation allocates ports and an id: one at a time
    lock: Mutex<()>,
}

pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 24
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Manager {
    pub fn new(cfg: Config, units: Box<dyn Units>) -> Result<Arc<Self>> {
        std::fs::create_dir_all(cfg.dir.join("instances"))?;
        std::fs::create_dir_all(cfg.dir.join("records"))?;
        let m = Arc::new(Self {
            cfg,
            units,
            lock: Mutex::new(()),
        });
        m.take_records()?;
        Ok(m)
    }

    /// Servers made before the records moved off the share: their record
    /// is taken from the share once, as it stands, and kept here after.
    fn take_records(&self) -> Result<()> {
        let Ok(rd) = std::fs::read_dir(self.cfg.dir.join("instances")) else {
            return Ok(());
        };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) || self.record_file(&id).exists() {
                continue;
            }
            if let Ok(b) = std::fs::read(e.path().join("instance.json"))
                && let Ok(i) = serde_json::from_slice::<Instance>(&b)
                && i.id == id
            {
                eprintln!("games: {id}'s record moves off the share");
                self.save(&i)?;
            }
        }
        Ok(())
    }

    /// The record the host goes by: whose server, its ports, its memory.
    /// It lives outside the directory the guest shares, which only gets a
    /// copy to read; what the guest writes there changes nothing here.
    fn record_file(&self, id: &str) -> PathBuf {
        self.cfg.dir.join("records").join(format!("{id}.json"))
    }

    fn instance_dir(&self, id: &str) -> PathBuf {
        self.cfg.dir.join("instances").join(id)
    }

    pub fn instances(&self) -> Vec<Instance> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.cfg.dir.join("records")) else {
            return out;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json")
                && let Some(i) = self.instance(id)
            {
                out.push(i);
            }
        }
        out.sort_by_key(|i| i.created);
        out
    }

    pub fn instance(&self, id: &str) -> Option<Instance> {
        if !valid_id(id) {
            return None;
        }
        let i: Instance =
            serde_json::from_slice(&std::fs::read(self.record_file(id)).ok()?).ok()?;
        // every caller builds host paths from `i.id` - systemctl, save,
        // remove_dir_all - so it has to be the name it was found under
        (i.id == id).then_some(i)
    }

    fn save(&self, i: &Instance) -> Result<()> {
        let body = serde_json::to_vec_pretty(i)?;
        let r = self.record_file(&i.id);
        let tmp = r.with_extension("json.tmp");
        std::fs::write(&tmp, &body)?;
        std::fs::rename(tmp, &r)?;
        // the guest's copy: what it needs to be this server
        let d = self.instance_dir(&i.id);
        std::fs::create_dir_all(&d)?;
        // written into a directory the guest can write: a fresh file,
        // never through a link it left there, renamed over the old copy
        let tmp = d.join(".instance.json.tmp");
        let _ = std::fs::remove_file(&tmp);
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(Self::nofollow())
                .open(&tmp)?;
            f.write_all(&body)?;
        }
        std::fs::rename(tmp, d.join("instance.json"))?;
        Ok(())
    }

    /// O_NOFOLLOW, without pulling in libc for one constant
    fn nofollow() -> i32 {
        0o400000
    }

    /// A file the guest writes, read the way anything a guest writes has to
    /// be: no symlink followed, a regular file or nothing, and only the last
    /// `cap` bytes. The instance directory is a 9p share the guest can write,
    /// so every one of these is an untrusted file however ordinary it looks.
    fn tail_of(path: &std::path::Path, cap: u64) -> String {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let mut f = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(Self::nofollow())
            .open(path)
        {
            Ok(f) => f,
            Err(_) => return String::new(),
        };
        let Ok(meta) = f.metadata() else {
            return String::new();
        };
        if !meta.is_file() {
            return String::new();
        }
        let from = meta.len().saturating_sub(cap);
        if from > 0 && f.seek(SeekFrom::Start(from)).is_err() {
            return String::new();
        }
        let mut buf = Vec::new();
        if f.take(cap).read_to_end(&mut buf).is_err() {
            return String::new();
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// the game's own output, as the guest appends it beside the record
    pub fn log_tail(&self, id: &str, lines: usize) -> String {
        let text = Self::tail_of(&self.instance_dir(id).join("game.log"), 64 * 1024);
        let all: Vec<&str> = text.lines().collect();
        let from = all.len().saturating_sub(lines);
        all[from..].join("\n")
    }

    pub fn state(&self, i: &Instance) -> State {
        let (active, failed) = self.units.state(&i.id);
        if failed {
            return State::Failed;
        }
        if !active {
            // meant to be up and not yet: between states (a restart, the
            // moment after a start), not stopped
            return if i.desired == "running" {
                State::Starting
            } else {
                State::Stopped
            };
        }
        match Self::tail_of(&self.instance_dir(&i.id).join("status"), 64).trim() {
            "running" => State::Running,
            "updating" => State::Updating,
            _ => State::Starting,
        }
    }

    fn up(&self, i: &Instance) -> bool {
        self.units.state(&i.id).0
    }

    /// A new server of `game` for `owner`, started. Refused past the
    /// member's share or the box's memory.
    pub fn create(
        &self,
        owner: &str,
        game: &str,
        settings: &BTreeMap<String, String>,
    ) -> Result<Instance> {
        let _g = self.lock.lock().unwrap();
        let spec = self
            .cfg
            .catalogue
            .get(game)
            .with_context(|| format!("no such game: {game}"))?;
        let all = self.instances();
        let mine_up = all
            .iter()
            .filter(|i| i.owner == owner && i.desired == "running")
            .count();
        if mine_up >= self.cfg.per_member {
            bail!(
                "you have {mine_up} server{} up already; stop one first",
                if mine_up == 1 { "" } else { "s" }
            );
        }
        self.room_for(&all, spec.memory().min(self.cfg.memory_budget))?;

        let used: HashSet<u16> = all
            .iter()
            .flat_map(|i| i.ports.iter().map(|p| p.port))
            .collect();
        let mut free = (self.cfg.port_base..self.cfg.port_base.saturating_add(self.cfg.port_count))
            .filter(|p| !used.contains(p));
        let mut ports = Vec::new();
        for var in &spec.ports {
            // a port the game insists on (it tells clients that number) is
            // that number or nothing; the rest come from the pool
            let port = match spec
                .port_defaults
                .get(var)
                .and_then(|d| d.parse::<u16>().ok())
            {
                Some(fixed) => {
                    if used.contains(&fixed) {
                        bail!(
                            "{} needs port {fixed} and another server here has it; one of this game per box",
                            spec.name
                        );
                    }
                    fixed
                }
                None => free.next().context("this box has no game ports left")?,
            };
            ports.push(Port {
                var: var.clone(),
                port,
            });
        }

        // the egg's variables: its defaults, then what the person set where
        // they may, then the ports
        let mut env: BTreeMap<String, String> = spec
            .settings
            .iter()
            .map(|s| (s.var.clone(), s.default.clone()))
            .collect();
        for s in spec.visible_settings() {
            if let Some(v) = settings.get(&s.var) {
                let v = v.trim();
                if !v.is_empty() {
                    if let Some(c) = &s.choices
                        && !c.iter().any(|x| x == v)
                    {
                        bail!("{}: not one of the choices", s.label);
                    }
                    if s.kind == "number" && v.parse::<f64>().is_err() {
                        bail!("{}: not a number", s.label);
                    }
                    Self::settable(&s.label, v)?;
                    env.insert(s.var.clone(), v.to_string());
                }
            }
        }
        // any password the catalogue suggested is one everybody knows
        for s in &spec.settings {
            if s.kind == "password" && !s.default.is_empty() {
                let given = env.get(&s.var).map(String::as_str).unwrap_or("");
                if given.is_empty() || given == s.default {
                    let made = Self::made_up_secret();
                    if made.is_empty() {
                        bail!("{}: no randomness to make a password with", s.label);
                    }
                    env.insert(s.var.clone(), made);
                }
            }
        }
        for p in &ports {
            env.insert(p.var.clone(), p.port.to_string());
        }

        // The id is the game's name with a number after it, and it has to
        // fit valid_id. Four catalogue games are already 24 characters or
        // more, so the name has to give way to the number rather than the
        // search running past every integer looking for a fit.
        let taken: HashSet<String> = all.iter().map(|i| i.id.clone()).collect();
        let stem: String = game
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        let id = (1..=999)
            .map(|n| {
                let n = n.to_string();
                let keep = 24usize.saturating_sub(n.len()).min(stem.len());
                format!("{}{n}", &stem[..keep])
            })
            .find(|s| !taken.contains(s) && valid_id(s))
            .context("no free id for this game")?;

        // no more than the box has: the budget bounds memory, the cpu count
        // bounds cores (qemu warns past it, kvm crawls)
        let memory = spec.memory().min(self.cfg.memory_budget);
        let cores = spec.cores().min(self.cfg.cores.max(1));
        let i = Instance {
            id,
            game: game.to_string(),
            owner: owner.to_string(),
            memory,
            cores,
            ports,
            env,
            recipe: Recipe {
                app: spec.app,
                wine: spec.wine,
                startup: spec.startup.clone(),
                ready: spec.ready.clone(),
                stop: spec.stop.clone(),
                files: spec.files.clone(),
                install: spec.install.clone(),
            },
            desired: "running".into(),
            created: now(),
        };
        self.save(&i)?;
        self.units.start(&i.id)?;
        Ok(i)
    }

    fn room_for(&self, all: &[Instance], more: u64) -> Result<()> {
        let up: u64 = all
            .iter()
            .filter(|i| i.desired == "running")
            .map(|i| i.memory)
            .sum();
        if up + more > self.cfg.memory_budget {
            bail!("this box is full of games right now; try again when one stops");
        }
        Ok(())
    }

    fn owned(&self, owner: &str, id: &str) -> Result<Instance> {
        let i = self.instance(id).context("no such server")?;
        anyhow::ensure!(i.owner == owner, "that server is not yours");
        Ok(i)
    }

    pub fn start(&self, owner: &str, id: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut i = self.owned(owner, id)?;
        if i.desired != "running" {
            let all = self.instances();
            let mine_up = all
                .iter()
                .filter(|x| x.owner == owner && x.desired == "running")
                .count();
            anyhow::ensure!(
                mine_up < self.cfg.per_member,
                "you have a server up already; stop it first"
            );
            self.room_for(&all, i.memory)?;
        }
        i.desired = "running".into();
        self.save(&i)?;
        let r = self.units.start(&i.id);
        self.write_access();
        r
    }

    /// The settings a person may set, changed; they take effect when the
    /// server next starts (the guest fills its files at start). A running
    /// server is restarted for them, world kept.
    /// A secret for this instance alone.
    ///
    /// A catalogue default for a password is a published password: the egg
    /// is public and so is whatever it suggests. An empty one is left empty
    /// on purpose - for most games that is "no password", and filling it in
    /// would shut a member's friends out of their own server.
    fn made_up_secret() -> String {
        const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        let mut raw = [0u8; 20];
        if std::fs::File::open("/dev/urandom")
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut raw))
            .is_err()
        {
            // never silently fall back to something guessable
            return String::new();
        }
        raw.iter()
            .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
            .collect()
    }

    /// A setting's value, as it may safely be written into a config file
    /// and an environment. The startup line does not need this - it takes
    /// a shell reference to the variable, never the text - but a newline
    /// in a value would still split a line in a config file in two.
    fn settable(label: &str, v: &str) -> Result<()> {
        if let Some(c) = v.chars().find(|c| c.is_control()) {
            bail!("{label}: {c:?} cannot be used here");
        }
        Ok(())
    }

    pub fn configure(
        &self,
        owner: &str,
        id: &str,
        settings: &BTreeMap<String, String>,
    ) -> Result<()> {
        let mut i = self.owned(owner, id)?;
        let spec = self
            .cfg
            .catalogue
            .get(&i.game)
            .with_context(|| format!("no such game: {}", i.game))?;
        for s in spec.visible_settings() {
            if let Some(v) = settings.get(&s.var) {
                let v = v.trim();
                if v.is_empty() {
                    continue;
                }
                if let Some(c) = &s.choices
                    && !c.iter().any(|x| x == v)
                {
                    bail!("{}: not one of the choices", s.label);
                }
                if s.kind == "number" && v.parse::<f64>().is_err() {
                    bail!("{}: not a number", s.label);
                }
                Self::settable(&s.label, v)?;
                i.env.insert(s.var.clone(), v.to_string());
            }
        }
        self.save(&i)?;
        if i.desired == "running" {
            self.units.stop(&i.id)?;
            self.units.start(&i.id)?;
        }
        Ok(())
    }

    pub fn stop(&self, owner: &str, id: &str) -> Result<()> {
        let mut i = self.owned(owner, id)?;
        i.desired = "stopped".into();
        self.save(&i)?;
        let r = self.units.stop(&i.id);
        self.write_access();
        r
    }

    /// The server and its world, gone. Stopped first.
    pub fn delete(&self, owner: &str, id: &str) -> Result<()> {
        let i = self.owned(owner, id)?;
        anyhow::ensure!(
            !self.up(&i),
            "stop it first; deleting removes the world with it"
        );
        std::fs::remove_dir_all(self.instance_dir(&i.id))?;
        let _ = std::fs::remove_file(self.record_file(&i.id));
        let _ = std::fs::remove_file(self.players_file(&i.id));
        Ok(())
    }

    // Who else plays on a server. Kept beside the instances, not in them:
    // an instance's directory is shared with its guest, and a game server
    // must not be able to invite people to itself.
    fn players_file(&self, id: &str) -> PathBuf {
        self.cfg.dir.join("players").join(format!("{id}.json"))
    }

    /// the people the owner has invited to this server
    pub fn players(&self, id: &str) -> Vec<String> {
        if !valid_id(id) {
            return Vec::new();
        }
        std::fs::read(self.players_file(id))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// The owner says who plays. Only names the gate says are the owner's
    /// friends are kept (`friends`, asked by the caller just before).
    pub fn set_players(
        &self,
        owner: &str,
        id: &str,
        want: &[String],
        friends: &[String],
    ) -> Result<Vec<String>> {
        let i = self.owned(owner, id)?;
        let mut keep: Vec<String> = want
            .iter()
            .filter(|p| *p != owner && friends.contains(p))
            .cloned()
            .collect();
        keep.sort();
        keep.dedup();
        let f = self.players_file(&i.id);
        std::fs::create_dir_all(f.parent().context("players dir")?)?;
        let tmp = f.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&keep)?)?;
        std::fs::rename(&tmp, &f)?;
        self.write_access();
        Ok(keep)
    }

    /// The address players type: this box on the fleet's network, where
    /// the people invited reach it from their app.
    pub fn address(&self) -> String {
        self.cfg
            .address_file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty() && a.parse::<std::net::IpAddr>().is_ok())
            .unwrap_or_else(|| self.cfg.address.clone())
    }

    /// whose server it is, or one they are invited to
    pub fn may_see(&self, user: &str, i: &Instance) -> bool {
        i.owner == user || self.players(&i.id).iter().any(|p| p == user)
    }

    /// For the firewall: every server meant to be up, its ports, its owner
    /// and players. Names only - the gate turns them into addresses, and
    /// checks every one again, each time the firewall asks.
    pub fn access(&self) -> serde_json::Value {
        let servers: Vec<serde_json::Value> = self
            .instances()
            .into_iter()
            .filter(|i| i.desired == "running")
            .map(|i| {
                serde_json::json!({
                    "id": i.id,
                    "owner": i.owner,
                    "players": self.players(&i.id),
                    "ports": i.ports.iter().map(|p| p.port).collect::<Vec<_>>(),
                })
            })
            .collect();
        serde_json::json!({ "servers": servers })
    }

    /// Written aside and moved into place, so the firewall never reads half.
    pub fn write_access(&self) {
        let Some(path) = &self.cfg.access else {
            return;
        };
        let tmp = path.with_extension("tmp");
        let r = serde_json::to_vec(&self.access())
            .map_err(anyhow::Error::from)
            .and_then(|b| Ok(std::fs::write(&tmp, b)?))
            .and_then(|()| Ok(std::fs::rename(&tmp, path)?));
        if let Err(e) = r {
            eprintln!("games: access file: {e:#}");
        }
    }

    fn worlds_dir(&self) -> PathBuf {
        self.cfg.dir.join("worlds")
    }

    /// A world is everything the game wrote after its install: saves,
    /// its config, whatever it keeps, whichever game it is. Kept under the
    /// owner's name; the game files, which steam can give again, go.
    pub fn keep_world(&self, owner: &str, id: &str) -> Result<String> {
        let i = self.owned(owner, id)?;
        anyhow::ensure!(!self.up(&i), "stop it first");
        let server = self.instance_dir(&i.id).join("server");
        let marker = server.join(".installed");
        let since = std::fs::metadata(&marker)
            .and_then(|m| m.modified())
            .context("this server was never installed; nothing to keep")?;
        let name = format!("{}-{}", i.game, now());
        let dest = self.worlds_dir().join(owner).join(&name);
        std::fs::create_dir_all(&dest)?;
        let mut files = 0usize;
        for entry in walk(&server) {
            let rel = entry.strip_prefix(&server).unwrap_or(&entry);
            let top = rel
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_string());
            // steam's own dir and the install marker are not the world
            if matches!(
                top.as_deref(),
                Some("steamcmd") | Some(".steam") | Some(".installed")
            ) {
                continue;
            }
            // symlink_metadata, not metadata: the server tree is the
            // guest's, and a link in it would have the host copy whatever
            // it points at into the member's saved world
            let Ok(meta) = std::fs::symlink_metadata(&entry) else {
                continue;
            };
            if meta.is_file() && meta.modified().map(|m| m > since).unwrap_or(false) {
                let to = dest.join(rel);
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&entry, &to)?;
                files += 1;
            }
        }
        let meta = World {
            name: name.clone(),
            game: i.game.clone(),
            owner: owner.to_string(),
            env: i.env.clone(),
            kept: now(),
            files,
        };
        std::fs::write(dest.join("world.json"), serde_json::to_vec_pretty(&meta)?)?;
        std::fs::remove_dir_all(self.instance_dir(&i.id))?;
        let _ = std::fs::remove_file(self.record_file(&i.id));
        Ok(name)
    }

    /// The worlds one person has kept
    pub fn worlds(&self, owner: &str) -> Vec<World> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.worlds_dir().join(owner)) else {
            return out;
        };
        for e in rd.flatten() {
            if let Ok(b) = std::fs::read(e.path().join("world.json"))
                && let Ok(w) = serde_json::from_slice::<World>(&b)
            {
                out.push(w);
            }
        }
        out.sort_by_key(|w| std::cmp::Reverse(w.kept));
        out
    }

    /// A new server of the world's game, with the world's settings, and
    /// the world's files put back once the game is installed (the guest
    /// copies /instance/world over the install before it starts).
    pub fn restore_world(&self, owner: &str, name: &str) -> Result<Instance> {
        let src = self.worlds_dir().join(owner).join(name);
        let w: World = serde_json::from_slice(&std::fs::read(src.join("world.json"))?)
            .context("no such world")?;
        anyhow::ensure!(w.owner == owner, "not yours");
        let settings: BTreeMap<String, String> = w.env.clone();
        let i = self.create(owner, &w.game, &settings)?;
        let dest = self.instance_dir(&i.id).join("world");
        copy_tree(&src, &dest)?;
        let _ = std::fs::remove_file(dest.join("world.json"));
        std::fs::remove_dir_all(&src)?;
        Ok(i)
    }

    pub fn delete_world(&self, owner: &str, name: &str) -> Result<()> {
        let src = self.worlds_dir().join(owner).join(name);
        let w: World = serde_json::from_slice(&std::fs::read(src.join("world.json"))?)
            .context("no such world")?;
        anyhow::ensure!(w.owner == owner, "not yours");
        std::fs::remove_dir_all(&src)?;
        Ok(())
    }

    /// After a reboot, or this service's own restart: every server whose
    /// record says it should be up, is.
    pub fn restore(&self) -> usize {
        let mut n = 0;
        for i in self.instances() {
            if i.desired == "running" && !self.up(&i) {
                match self.units.start(&i.id) {
                    Ok(()) => n += 1,
                    Err(e) => eprintln!("games: could not bring {} back: {e:#}", i.id),
                }
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {

    /// Everything in an instance directory is on a share the guest writes,
    /// so every read of one is a read of something hostile.
    #[test]
    fn a_guest_cannot_make_the_host_read_forever() {
        let d = std::env::temp_dir().join(format!("dd-games-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();

        // a log far bigger than the cap: only the tail comes back
        let big = d.join("game.log");
        std::fs::write(&big, "x".repeat(300_000) + "\nlast line\n").unwrap();
        let got = Manager::tail_of(&big, 64 * 1024);
        assert!(got.len() <= 64 * 1024, "{}", got.len());
        assert!(got.ends_with("last line\n"), "the tail, not the head");

        // a symlink to something endless is not followed at all
        let link = d.join("status");
        std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
        assert_eq!(Manager::tail_of(&link, 64), "");

        let _ = std::fs::remove_dir_all(&d);
    }
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Fake {
        up: Mutex<HashSet<String>>,
    }
    impl Units for Arc<Fake> {
        fn start(&self, id: &str) -> Result<()> {
            self.up.lock().unwrap().insert(id.into());
            Ok(())
        }
        fn stop(&self, id: &str) -> Result<()> {
            self.up.lock().unwrap().remove(id);
            Ok(())
        }
        fn state(&self, id: &str) -> (bool, bool) {
            (self.up.lock().unwrap().contains(id), false)
        }
    }

    static N: AtomicUsize = AtomicUsize::new(0);
    fn manager(per_member: usize, budget: u64) -> (Arc<Manager>, Arc<Fake>) {
        let dir = std::env::temp_dir().join(format!(
            "dd-games-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let mut catalogue = BTreeMap::new();
        catalogue.insert(
            "valheim".to_string(),
            Game {
                id: "valheim".into(),
                name: "Valheim".into(),
                description: String::new(),
                app: 896660,
                game: Some(892970),
                cover: true,
                wine: false,
                startup: "./valheim_server.x86_64 -name \"{{SERVER_NAME}}\" -port {{SERVER_PORT}} -password \"{{PASSWORD}}\"".into(),
                ready: Some("Game server connected".into()),
                stop: "^C".into(),
                files: serde_json::json!({}),
                install: "steamcmd".into(),
                settings: vec![
                    Setting {
                        var: "SERVER_NAME".into(),
                        label: "Server name".into(),
                        default: "My Server".into(),
                        kind: "text".into(),
                        help: String::new(),
                        editable: true,
                        ours: Some("name".into()),
                        choices: None,
                    },
                    Setting {
                        var: "PASSWORD".into(),
                        label: "Password".into(),
                        default: "secret".into(),
                        kind: "text".into(),
                        help: String::new(),
                        editable: true,
                        ours: Some("password".into()),
                        choices: None,
                    },
                    Setting {
                        var: "SRCDS_APPID".into(),
                        label: "App".into(),
                        default: "896660".into(),
                        kind: "text".into(),
                        help: String::new(),
                        editable: false,
                        ours: None,
                        choices: None,
                    },
                ],
                ports: vec!["SERVER_PORT".into(), "QUERY_PORT".into()],
                port_defaults: BTreeMap::new(),
            },
        );
        let fake = Arc::new(Fake::default());
        let m = Manager::new(
            Config {
                dir,
                catalogue,
                port_base: 27000,
                port_count: 4,
                per_member,
                memory_budget: budget,
                cores: 8,
                address: "box.example".into(),
                home: "https://home.example/".into(),
                access: None,
                address_file: None,
            },
            Box::new(fake.clone()),
        )
        .unwrap();
        (m, fake)
    }

    #[test]
    fn only_the_owners_friends_are_invited_and_only_they_see_it() {
        let (m, _fake) = manager(1, 16384);
        let mut set = BTreeMap::new();
        set.insert("SRCDS_APPID".to_string(), "1".to_string());
        let i = m.create("tom", "valheim", &set).unwrap();
        let friends = vec!["sarah".to_string(), "lingyun".to_string()];
        // a name the gate did not vouch for, and the owner, are dropped
        let kept = m
            .set_players(
                "tom",
                &i.id,
                &["sarah".into(), "mallory".into(), "tom".into()],
                &friends,
            )
            .unwrap();
        assert_eq!(kept, ["sarah"]);
        assert!(m.may_see("tom", &i) && m.may_see("sarah", &i));
        assert!(!m.may_see("lingyun", &i) && !m.may_see("mallory", &i));
        // only the owner says who plays
        assert!(
            m.set_players("sarah", &i.id, &["lingyun".into()], &friends)
                .is_err()
        );
        // the firewall hears of it only while it is meant to be up: a new
        // server starts at once
        let a = m.access();
        assert_eq!(a["servers"][0]["owner"], "tom");
        assert_eq!(a["servers"][0]["players"][0], "sarah");
        assert_eq!(
            a["servers"][0]["ports"].as_array().unwrap().len(),
            i.ports.len()
        );
        // kept apart from the guest's share
        assert!(!m.instance_dir(&i.id).join("players.json").exists());
        m.stop("tom", &i.id).unwrap();
        assert!(m.access()["servers"].as_array().unwrap().is_empty());
        m.delete("tom", &i.id).unwrap();
        assert!(m.players(&i.id).is_empty());
    }

    #[test]
    fn a_member_starts_stops_and_deletes_their_own_server() {
        let (m, fake) = manager(1, 16384);
        let mut set = BTreeMap::new();
        set.insert("SERVER_NAME".to_string(), "Tom's".to_string());
        set.insert("SRCDS_APPID".to_string(), "1".to_string());
        let i = m.create("tom", "valheim", &set).unwrap();
        assert_eq!(i.id, "valheim1");
        assert_eq!(i.ports[0].port, 27000);
        assert_eq!(i.ports[1].port, 27001);
        assert_eq!(i.recipe.app, 896660);
        assert_eq!(i.env["SERVER_NAME"], "Tom's");
        assert_eq!(i.env["PASSWORD"], "secret", "the egg's default");
        assert_eq!(i.env["SRCDS_APPID"], "896660", "not a person's to set");
        assert_eq!(i.env["SERVER_PORT"], i.ports[0].port.to_string());
        assert!(fake.up.lock().unwrap().contains("valheim1"));
        // the record is what the runner and the guest read
        assert_eq!(m.instance("valheim1").unwrap(), i);

        // one at a time, and not someone else's
        assert!(m.create("tom", "valheim", &BTreeMap::new()).is_err());
        assert!(m.stop("eve", "valheim1").is_err());
        assert!(m.delete("tom", "valheim1").is_err(), "running: stop first");

        m.stop("tom", "valheim1").unwrap();
        assert_eq!(m.instance("valheim1").unwrap().desired, "stopped");
        assert_eq!(m.state(&m.instance("valheim1").unwrap()), State::Stopped);
        m.delete("tom", "valheim1").unwrap();
        assert!(m.instance("valheim1").is_none());
        assert!(m.create("tom", "nope", &BTreeMap::new()).is_err());
    }

    #[test]
    fn a_reboot_brings_back_what_should_be_up_and_only_that() {
        let (m, fake) = manager(2, 16384);
        let a = m.create("tom", "valheim", &BTreeMap::new()).unwrap();
        let b = m.create("tom", "valheim", &BTreeMap::new()).unwrap();
        m.stop("tom", &b.id).unwrap();
        // the box goes down: nothing is up, the records remain
        fake.up.lock().unwrap().clear();
        assert_eq!(m.restore(), 1);
        assert!(fake.up.lock().unwrap().contains(&a.id));
        assert!(!fake.up.lock().unwrap().contains(&b.id));
        assert_eq!(m.restore(), 0, "already up");
    }

    #[test]
    fn settings_change_and_a_running_server_restarts_for_them() {
        let (m, fake) = manager(1, 16384);
        let i = m.create("tom", "valheim", &BTreeMap::new()).unwrap();
        let mut set = BTreeMap::new();
        set.insert("SERVER_NAME".to_string(), "Renamed".to_string());
        set.insert("SRCDS_APPID".to_string(), "1".to_string());
        m.configure("tom", &i.id, &set).unwrap();
        let i = m.instance(&i.id).unwrap();
        assert_eq!(i.env["SERVER_NAME"], "Renamed");
        assert_eq!(i.env["SRCDS_APPID"], "896660", "not a person's to set");
        assert!(fake.up.lock().unwrap().contains(&i.id), "back up after");
        assert!(m.configure("ann", &i.id, &set).is_err(), "not hers");
    }

    #[test]
    fn a_world_outlives_its_server_and_starts_a_new_one() {
        let (m, fake) = manager(1, 16384);
        let i = m.create("tom", "valheim", &BTreeMap::new()).unwrap();
        let server = m.instance_dir(&i.id).join("server");
        std::fs::create_dir_all(server.join("steamcmd")).unwrap();
        std::fs::write(server.join(".installed"), "1").unwrap();
        std::fs::write(server.join("game.bin"), "install").unwrap();
        std::fs::write(server.join("steamcmd/steam.bin"), "tool").unwrap();
        // the marker is older than what the game writes after
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::open(server.join(".installed"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::File::open(server.join("game.bin"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::create_dir_all(server.join("saves")).unwrap();
        std::fs::write(server.join("saves/world.db"), "the world").unwrap();
        assert!(m.keep_world("tom", &i.id).is_err(), "not while up");
        m.stop("tom", &i.id).unwrap();
        let name = m.keep_world("tom", &i.id).unwrap();
        assert!(m.instance(&i.id).is_none(), "the server is gone");
        let worlds = m.worlds("tom");
        assert_eq!(worlds.len(), 1);
        assert_eq!(worlds[0].files, 1, "only what the game wrote");
        assert!(m.worlds("ann").is_empty());
        let j = m.restore_world("tom", &name).unwrap();
        assert!(fake.up.lock().unwrap().contains(&j.id));
        assert_eq!(
            std::fs::read_to_string(m.instance_dir(&j.id).join("world/saves/world.db")).unwrap(),
            "the world"
        );
        assert!(m.worlds("tom").is_empty(), "handed over");
    }

    #[test]
    fn the_box_has_so_much_memory_and_so_many_ports() {
        let (m, _) = manager(5, 12288);
        m.create("a", "valheim", &BTreeMap::new()).unwrap();
        m.create("b", "valheim", &BTreeMap::new()).unwrap();
        let e = m
            .create("c", "valheim", &BTreeMap::new())
            .unwrap_err()
            .to_string();
        assert!(e.contains("full"), "{e}");
        let (m, _) = manager(5, 1 << 20);
        m.create("a", "valheim", &BTreeMap::new()).unwrap();
        m.create("b", "valheim", &BTreeMap::new()).unwrap();
        let e = m
            .create("c", "valheim", &BTreeMap::new())
            .unwrap_err()
            .to_string();
        assert!(e.contains("ports"), "{e}");
    }
}
