//! Members' own computers (docs/dev/instances.md).
//!
//! A member saves configs - size, packages as a Nix file, who may reach it,
//! where it may run - and makes instances from them. An instance is a record
//! on this box and a unit, `dd-computer@<id>`, that boots it (nix/modules/vms).
//! This service keeps both: it checks a member's instances together stay
//! within the policy's totals, hands each its ports, writes what the guest
//! reads from the record, and on its own start brings back every instance
//! whose record says it should be up.
//!
//! Who is asking comes from the gate: nginx sets `x-dd-user` from the
//! verifier's answer.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What every member's instances, together, may take; and the bounds of one.
#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    /// MiB, every instance a member has, running or not
    pub memory: u64,
    pub threads: u32,
    /// GiB, one instance's disk (its system's and its home's each)
    pub disk: u64,
    pub port_base: u16,
    pub port_count: u16,
    /// MiB and threads this box can give one computer (computer-facts.sh says the
    /// same to placement): a small box runs small ones
    pub box_memory: u64,
    pub box_threads: u32,
}

pub const MIN_MEMORY: u64 = 512;
pub const MIN_DISK: u64 = 8;
pub const MAX_NIX: usize = 64 * 1024;
pub const MAX_FORWARDS: usize = 8;
/// the config every member has from the start
pub const DEFAULT: &str = "default";
const DEFAULT_NIX: &str = "{ pkgs, ... }:
{
  # a general-purpose machine: build tools, editors, languages' basics
  environment.systemPackages = with pkgs; [
    gcc gnumake pkg-config python3 nodejs ripgrep fd tmux unzip wget
  ];
}
";

/// Where it may run. `Own`: only on a box of the owner's. `Any`: anyone's,
/// which its owner said yes to, knowing that box's owner can read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Consent {
    #[default]
    Own,
    Any,
}

/// A saved config: what instances are made from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// the address names it (PUT /api/configs/<name>); the body needn't
    #[serde(default)]
    pub name: String,
    /// MiB
    pub memory: u64,
    pub threads: u32,
    /// GiB
    pub disk: u64,
    /// a NixOS module for the guest: its packages, services, shells
    #[serde(default)]
    pub nix: String,
    /// SSH public keys, one per entry, for the owner's account
    #[serde(default)]
    pub keys: Vec<String>,
    /// guest ports opened besides SSH
    #[serde(default)]
    pub ports: Vec<u16>,
    /// members who may reach it too (the owner's friends; the gate checks)
    #[serde(default)]
    pub players: Vec<String>,
    #[serde(default)]
    pub consent: Consent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Forward {
    pub guest: u16,
    pub host: u16,
}

/// One instance, as this box keeps it. The unit reads it (computers.nix, dd-computer-run):
/// `memory`, `threads`, `disk`, `port`, `forwards`, `owner`, `keys`, `nix`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub id: String,
    pub owner: String,
    pub name: String,
    /// the config it was made from, and last applied
    pub config: String,
    pub memory: u64,
    pub threads: u32,
    pub disk: u64,
    /// SSH, on this box's address on the Commonty network
    pub port: u16,
    #[serde(default)]
    pub forwards: Vec<Forward>,
    #[serde(default)]
    pub nix: String,
    #[serde(default)]
    pub keys: Vec<String>,
    /// its own key's public half; the private half is beside its disk
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub players: Vec<String>,
    #[serde(default)]
    pub consent: Consent,
    /// when the owner said yes to anyone's box (ms); none for `Own`
    #[serde(default)]
    pub anywhere: Option<u64>,
    pub up: bool,
    pub created: u64,
}

impl Record {
    /// every key let in: its own, and any its config adds
    pub fn all_keys(&self) -> Vec<String> {
        std::iter::once(self.key.clone())
            .filter(|k| !k.is_empty())
            .chain(self.keys.iter().cloned())
            .collect()
    }
}

/// What the page and the CLI are shown of an instance.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    #[serde(flatten)]
    pub record: Record,
    /// running, starting, stopped, failed
    pub state: &'static str,
    /// `<address>:<port>`, for ssh -p
    pub address: String,
    /// what came of its Nix file, as the guest says (untrusted: shown, never acted on)
    pub applied: Option<serde_json::Value>,
}

/// The units the records run as. systemd on a box; a stand-in in tests.
pub trait Units: Send + Sync {
    fn start(&self, id: &str) -> Result<()>;
    fn stop(&self, id: &str) -> Result<()>;
    /// systemd's ActiveState: active, activating, inactive, failed, ...
    fn state(&self, id: &str) -> String;
}

pub struct Systemd;

impl Systemd {
    fn ctl(&self, verb: &str, id: &str) -> Result<String> {
        let out = std::process::Command::new("systemctl")
            .args([verb, "--no-block", &format!("dd-computer@{id}.service")])
            .output()
            .context("systemctl")?;
        if !out.status.success() {
            bail!(
                "systemctl {verb}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl Units for Systemd {
    fn start(&self, id: &str) -> Result<()> {
        self.ctl("start", id).map(|_| ())
    }
    fn stop(&self, id: &str) -> Result<()> {
        self.ctl("stop", id).map(|_| ())
    }
    fn state(&self, id: &str) -> String {
        std::process::Command::new("systemctl")
            .args([
                "show",
                "-p",
                "ActiveState",
                "--value",
                &format!("dd-computer@{id}.service"),
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }
}

pub struct Manager {
    pub dir: PathBuf,
    pub limits: Limits,
    /// whose box this is: "own" instances run only for them
    pub box_owner: String,
    /// this box on the Commonty network
    pub address: String,
    /// the storage ledger's count (storage/view.nix), for the allowance;
    /// none on a box without it
    pub ledger: Option<PathBuf>,
    units: Box<dyn Units>,
    lock: Mutex<()>,
}

/// a member's name or a config's: lowercase, digits, dashes
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 32
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !n.starts_with('-')
}

fn valid_key(k: &str) -> bool {
    let k = k.trim();
    !k.contains(['\n', '\r'])
        && k.len() < 16 * 1024
        && [
            "ssh-ed25519 ",
            "ssh-rsa ",
            "ecdsa-sha2-",
            "sk-ssh-ed25519@openssh.com ",
            "sk-ecdsa-sha2-",
        ]
        .iter()
        .any(|p| k.starts_with(p))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn random_id() -> Result<String> {
    let mut b = [0u8; 8];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// A file a guest wrote, from its report share: the guest's root can put
/// anything there, a link to another member's files, a pipe, a huge file.
/// Read only a plain file, not followed, at most `max` bytes.
fn read_guest(path: &Path, max: u64) -> Option<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut b = Vec::new();
    f.take(max).read_to_end(&mut b).ok()?;
    Some(b)
}

/// An SSH key pair for one remote computer: the public line for its
/// authorized keys, and the private key in OpenSSH's own format, which ssh
/// reads as it is (no passphrase: it is kept on the box, beside the disk
/// it opens, and handed only to the owner).
pub fn ssh_key(comment: &str) -> Result<(String, String)> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut seed)?;
    let pk = ed25519_dalek::SigningKey::from_bytes(&seed)
        .verifying_key()
        .to_bytes();
    // RFC 4251 strings: a length, then the bytes
    fn string(out: &mut Vec<u8>, b: &[u8]) {
        out.extend_from_slice(&(b.len() as u32).to_be_bytes());
        out.extend_from_slice(b);
    }
    let mut public = Vec::new();
    string(&mut public, b"ssh-ed25519");
    string(&mut public, &pk);
    let mut check = [0u8; 4];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut check)?;
    let mut private = Vec::new();
    private.extend_from_slice(&check);
    private.extend_from_slice(&check);
    string(&mut private, b"ssh-ed25519");
    string(&mut private, &pk);
    let mut both = seed.to_vec();
    both.extend_from_slice(&pk);
    string(&mut private, &both);
    string(&mut private, comment.as_bytes());
    let mut pad = 1u8;
    while private.len() % 8 != 0 {
        private.push(pad);
        pad += 1;
    }
    let mut blob = b"openssh-key-v1\0".to_vec();
    string(&mut blob, b"none");
    string(&mut blob, b"none");
    string(&mut blob, b"");
    blob.extend_from_slice(&1u32.to_be_bytes());
    string(&mut blob, &public);
    string(&mut blob, &private);
    let body = b64.encode(&blob);
    let mut pem = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for line in body.as_bytes().chunks(70) {
        pem.push_str(std::str::from_utf8(line).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    Ok((
        format!("ssh-ed25519 {} {comment}", b64.encode(&public)),
        pem,
    ))
}

/// written beside, then moved: a reader never sees half a file
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("moving {}", path.display()))?;
    Ok(())
}

impl Manager {
    pub fn new(
        dir: PathBuf,
        limits: Limits,
        box_owner: String,
        address: String,
        units: Box<dyn Units>,
    ) -> Result<Self> {
        for d in ["records", "instances", "configs"] {
            std::fs::create_dir_all(dir.join(d))?;
        }
        Ok(Manager {
            dir,
            limits,
            box_owner,
            address,
            ledger: None,
            units,
            lock: Mutex::new(()),
        })
    }

    fn configs_path(&self, owner: &str) -> PathBuf {
        self.dir.join("configs").join(format!("{owner}.json"))
    }
    fn record_path(&self, id: &str) -> PathBuf {
        self.dir.join("records").join(format!("{id}.json"))
    }
    fn instance_dir(&self, id: &str) -> PathBuf {
        self.dir.join("instances").join(id)
    }

    /// what the member saved
    fn stored(&self, owner: &str) -> BTreeMap<String, Config> {
        std::fs::read(self.configs_path(owner))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Their configs, and the default one unless they saved their own by
    /// that name: something to make a remote computer from on day one.
    pub fn configs(&self, owner: &str) -> BTreeMap<String, Config> {
        let mut all = self.stored(owner);
        all.entry(DEFAULT.to_string()).or_insert_with(|| Config {
            name: DEFAULT.to_string(),
            memory: 4096.min(self.limits.memory),
            threads: 2.min(self.limits.threads),
            disk: 32.min(self.limits.disk),
            nix: DEFAULT_NIX.to_string(),
            keys: vec![],
            ports: vec![],
            players: vec![],
            // the box's own owner can keep it home; anyone else has no box here
            consent: if owner == self.box_owner {
                Consent::Own
            } else {
                Consent::Any
            },
        });
        all
    }

    fn records(&self) -> Vec<Record> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.dir.join("records")) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json")
                && let Ok(r) = std::fs::read(&p)
                    .map_err(anyhow::Error::from)
                    .and_then(|b| Ok(serde_json::from_slice::<Record>(&b)?))
            {
                out.push(r);
            }
        }
        out.sort_by_key(|r| r.created);
        out
    }

    fn record(&self, owner: &str, id: &str) -> Result<Record> {
        if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("no such remote computer");
        }
        let r: Record = std::fs::read(self.record_path(id))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .context("no such remote computer")?;
        if r.owner != owner {
            bail!("no such remote computer");
        }
        Ok(r)
    }

    fn put(&self, r: &Record) -> Result<()> {
        write_atomic(&self.record_path(&r.id), &serde_json::to_vec_pretty(r)?)
    }

    /// A config as the owner may save it: every bound checked here, so a
    /// saved config can always make an instance on its own terms.
    pub fn check(&self, owner: &str, c: &Config) -> Result<()> {
        if !valid_name(&c.name) {
            bail!("a name is lowercase letters, digits and dashes, up to 32");
        }
        if c.memory < MIN_MEMORY || c.memory > self.limits.memory {
            bail!("memory is {MIN_MEMORY} to {} MiB", self.limits.memory);
        }
        if c.threads < 1 || c.threads > self.limits.threads {
            bail!("threads are 1 to {}", self.limits.threads);
        }
        if c.disk < MIN_DISK || c.disk > self.limits.disk {
            bail!("a disk is {MIN_DISK} to {} GiB", self.limits.disk);
        }
        if c.nix.len() > MAX_NIX {
            bail!("the Nix file is over 64 KiB");
        }
        if let Some(k) = c.keys.iter().find(|k| !valid_key(k)) {
            bail!(
                "not an SSH public key: {}",
                k.chars().take(40).collect::<String>()
            );
        }
        if c.ports.len() > MAX_FORWARDS {
            bail!("at most {MAX_FORWARDS} ports besides SSH");
        }
        if c.ports.iter().any(|&p| p == 0 || p == 22) {
            bail!("a port is 1 to 65535, and SSH is always there");
        }
        if let Some(p) = c.players.iter().find(|p| !valid_name(p) || *p == owner) {
            bail!("not someone to let in: {p}");
        }
        Ok(())
    }

    pub fn save_config(&self, owner: &str, c: Config) -> Result<()> {
        self.check(owner, &c)?;
        let _g = self.lock.lock().unwrap();
        let mut all = self.stored(owner);
        all.insert(c.name.clone(), c);
        write_atomic(&self.configs_path(owner), &serde_json::to_vec_pretty(&all)?)
    }

    pub fn delete_config(&self, owner: &str, name: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut all = self.stored(owner);
        if all.remove(name).is_none() {
            bail!(if name == DEFAULT {
                "the default config is always there; change it, and yours takes its place"
            } else {
                "no such config"
            });
        }
        write_atomic(&self.configs_path(owner), &serde_json::to_vec_pretty(&all)?)
    }

    /// What the owner's instances take together, leaving one out (the one
    /// being changed).
    fn used(&self, owner: &str, except: Option<&str>) -> (u64, u32) {
        self.records()
            .iter()
            .filter(|r| r.owner == owner && Some(r.id.as_str()) != except)
            .fold((0, 0), |(m, t), r| (m + r.memory, t + r.threads))
    }

    pub fn usage(&self, owner: &str) -> (u64, u32) {
        self.used(owner, None)
    }

    fn fits(&self, owner: &str, except: Option<&str>, c: &Config) -> Result<()> {
        if c.memory > self.limits.box_memory || c.threads > self.limits.box_threads {
            bail!(
                "this box can take a remote computer of up to {} MiB and {} threads",
                self.limits.box_memory,
                self.limits.box_threads
            );
        }
        let (m, t) = self.used(owner, except);
        if m + c.memory > self.limits.memory {
            bail!(
                "your remote computers would take {} MiB together; the limit is {} MiB",
                m + c.memory,
                self.limits.memory
            );
        }
        if t + c.threads > self.limits.threads {
            bail!(
                "your remote computers would take {} threads together; the limit is {}",
                t + c.threads,
                self.limits.threads
            );
        }
        Ok(())
    }

    /// Over their allowance, as the ledger last counted every service: no
    /// new computer, no bigger disk. One running isn't stopped. No ledger
    /// here, or no count yet: nothing to hold to.
    fn within_allowance(&self, owner: &str) -> Result<()> {
        let Some(path) = &self.ledger else {
            return Ok(());
        };
        let Some(v) = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            return Ok(());
        };
        let budget = v["budget"].as_u64().unwrap_or(u64::MAX);
        let used: u64 = v["members"][owner]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, n)| n.as_u64())
            .fold(0, u64::saturating_add);
        if used >= budget {
            bail!(
                "you use {} GB of your {} GB storage allowance: make room first",
                used / 1_000_000_000,
                budget / 1_000_000_000
            );
        }
        Ok(())
    }

    fn where_ok(&self, owner: &str, c: &Config, anywhere: Option<u64>) -> Result<()> {
        match c.consent {
            Consent::Own if owner != self.box_owner => {
                bail!("this box isn't yours: choose anyone's box, and say yes to its warning")
            }
            Consent::Any if anywhere.is_none() => {
                bail!("running on someone else's box needs your yes first")
            }
            _ => Ok(()),
        }
    }

    /// Host ports for SSH and the forwards: the lowest free in the pool.
    fn ports(&self, records: &[Record], n: usize) -> Result<Vec<u16>> {
        let taken: std::collections::HashSet<u16> = records
            .iter()
            .flat_map(|r| std::iter::once(r.port).chain(r.forwards.iter().map(|f| f.host)))
            .collect();
        let free: Vec<u16> = (self.limits.port_base
            ..self.limits.port_base.saturating_add(self.limits.port_count))
            .filter(|p| !taken.contains(p))
            .take(n)
            .collect();
        if free.len() < n {
            bail!("no ports left on this box");
        }
        Ok(free)
    }

    /// A new instance from a saved config. `yes`: the owner said yes to
    /// the warning about someone else's box, just now.
    /// `keys`: SSH keys for this one computer besides its config's (the
    /// default config has none of its own)
    pub fn make(
        &self,
        owner: &str,
        config: &str,
        name: &str,
        yes: bool,
        keys: &[String],
    ) -> Result<Record> {
        if !valid_name(name) {
            bail!("a name is lowercase letters, digits and dashes, up to 32");
        }
        let mut c = self
            .configs(owner)
            .remove(config)
            .context("no such config")?;
        c.keys.extend(
            keys.iter()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty()),
        );
        self.check(owner, &c)?;
        let anywhere = (c.consent == Consent::Any && yes).then(now_ms);
        self.where_ok(owner, &c, anywhere)?;
        self.within_allowance(owner)?;
        let _g = self.lock.lock().unwrap();
        let records = self.records();
        if records.iter().any(|r| r.owner == owner && r.name == name) {
            bail!("you have a remote computer called {name}");
        }
        self.fits(owner, None, &c)?;
        let ports = self.ports(&records, 1 + c.ports.len())?;
        let id = random_id()?;
        let key = self.new_key(&id, owner, name)?;
        let r = Record {
            id,
            owner: owner.to_string(),
            name: name.to_string(),
            config: c.name.clone(),
            memory: c.memory,
            threads: c.threads,
            disk: c.disk,
            port: ports[0],
            forwards: c
                .ports
                .iter()
                .zip(&ports[1..])
                .map(|(&guest, &host)| Forward { guest, host })
                .collect(),
            nix: c.nix,
            keys: c.keys,
            key,
            players: c.players,
            consent: c.consent,
            anywhere,
            up: true,
            created: now_ms(),
        };
        self.put(&r)?;
        self.units.start(&r.id)?;
        Ok(r)
    }

    /// The config, again, onto an instance made from it: its Nix file and
    /// keys at once (the guest rebuilds), its size and ports at its next start.
    pub fn apply(&self, owner: &str, id: &str, yes: bool) -> Result<Record> {
        let _g = self.lock.lock().unwrap();
        let mut r = self.record(owner, id)?;
        let c = self
            .configs(owner)
            .remove(&r.config)
            .context("its config is gone: save one by that name")?;
        self.check(owner, &c)?;
        let anywhere = match c.consent {
            Consent::Any if r.consent == Consent::Any => r.anywhere,
            Consent::Any if yes => Some(now_ms()),
            _ => None,
        };
        self.where_ok(owner, &c, anywhere)?;
        self.fits(owner, Some(id), &c)?;
        if c.disk < r.disk {
            bail!("a disk only grows: this one is {} GiB", r.disk);
        }
        if c.disk > r.disk {
            self.within_allowance(owner)?;
        }
        // its own ports count as taken: the ones it keeps are its own, and a
        // new one must not be its SSH
        let records = self.records();
        let mut forwards = Vec::new();
        let new = c
            .ports
            .iter()
            .filter(|g| !r.forwards.iter().any(|f| f.guest == **g))
            .count();
        let mut fresh = self.ports(&records, new)?.into_iter();
        for &guest in &c.ports {
            let host = r
                .forwards
                .iter()
                .find(|f| f.guest == guest)
                .map(|f| f.host)
                .or_else(|| fresh.next())
                .context("no ports left on this box")?;
            forwards.push(Forward { guest, host });
        }
        r.memory = c.memory;
        r.threads = c.threads;
        r.disk = c.disk;
        r.forwards = forwards;
        r.nix = c.nix;
        r.keys = c.keys;
        r.players = c.players;
        r.consent = c.consent;
        r.anywhere = anywhere;
        self.put(&r)?;
        self.write_share(&r)?;
        Ok(r)
    }

    /// what the guest reads (its config share), as the unit writes it at start
    fn write_share(&self, r: &Record) -> Result<()> {
        let d = self.instance_dir(&r.id).join("config");
        if !d.is_dir() {
            return Ok(()); // never started: the unit writes it
        }
        write_atomic(
            &d.join("owner.json"),
            &serde_json::to_vec(&serde_json::json!({"name": r.owner, "keys": r.all_keys()}))?,
        )?;
        let nix = if r.nix.trim().is_empty() {
            "{ ... }: { }\n"
        } else {
            &r.nix
        };
        write_atomic(&d.join("member.nix"), nix.as_bytes())
    }

    /// where a computer's own key is: beside its disks, out of the guest's
    /// shares, so it moves and is deleted with them
    fn key_path(&self, id: &str) -> PathBuf {
        self.instance_dir(id).join("ssh").join("key")
    }

    fn new_key(&self, id: &str, owner: &str, name: &str) -> Result<String> {
        let (public, private) = ssh_key(&format!("{owner}@{name}"))?;
        let path = self.key_path(id);
        std::fs::create_dir_all(path.parent().context("no parent")?)?;
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(private.as_bytes())?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(public)
    }

    /// Its private key, for its owner alone: what `dd computer ssh` and the
    /// page's download hand to ssh.
    pub fn private_key(&self, owner: &str, id: &str) -> Result<(String, String)> {
        let r = self.record(owner, id)?;
        let k = std::fs::read_to_string(self.key_path(id)).context("it has no key yet")?;
        Ok((r.name, k))
    }

    /// A new key, the old one no longer let in: for a key that went
    /// somewhere it shouldn't have.
    pub fn rekey(&self, owner: &str, id: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut r = self.record(owner, id)?;
        r.key = self.new_key(id, owner, &r.name)?;
        self.put(&r)?;
        self.write_share(&r)
    }

    pub fn set_up(&self, owner: &str, id: &str, up: bool) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut r = self.record(owner, id)?;
        r.up = up;
        self.put(&r)?;
        if up {
            self.units.start(id)
        } else {
            self.units.stop(id)
        }
    }

    fn stopped(&self, id: &str) -> bool {
        matches!(self.units.state(id).as_str(), "inactive" | "failed" | "")
    }

    /// Its system again from the release's first disk; /home is kept.
    pub fn reset(&self, owner: &str, id: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        self.record(owner, id)?;
        if !self.stopped(id) {
            bail!("stop it first");
        }
        let sys = self.instance_dir(id).join("system.qcow2");
        if sys.exists() {
            std::fs::remove_file(&sys)?;
        }
        Ok(())
    }

    /// Gone, with its disks. Only stopped, and only by its owner.
    pub fn delete(&self, owner: &str, id: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        self.record(owner, id)?;
        if !self.stopped(id) {
            bail!("stop it first");
        }
        let d = self.instance_dir(id);
        if d.exists() {
            std::fs::remove_dir_all(&d)?;
        }
        std::fs::remove_file(self.record_path(id))?;
        Ok(())
    }

    pub fn view(&self, r: Record) -> View {
        let state = match self.units.state(&r.id).as_str() {
            "active" | "reloading" => "running",
            "activating" => "starting",
            "deactivating" => "stopping",
            "failed" => "failed",
            _ => "stopped",
        };
        let applied = read_guest(&self.instance_dir(&r.id).join("report/apply.json"), 4096)
            .and_then(|b| serde_json::from_slice(&b).ok());
        View {
            address: format!("{}:{}", self.address, r.port),
            state,
            applied,
            record: r,
        }
    }

    pub fn instances(&self, owner: &str) -> Vec<View> {
        self.records()
            .into_iter()
            .filter(|r| r.owner == owner)
            .map(|r| self.view(r))
            .collect()
    }

    /// The guest's own words about its last apply, cut short; shown to its owner.
    pub fn apply_log(&self, owner: &str, id: &str) -> Result<String> {
        self.record(owner, id)?;
        let b =
            read_guest(&self.instance_dir(id).join("report/apply.log"), 8192).unwrap_or_default();
        Ok(String::from_utf8_lossy(&b).into_owned())
    }

    /// Every instance whose record says up, after a reboot or a restart of
    /// this service. Starting one already running is nothing.
    pub fn restore(&self) -> usize {
        let mut n = 0;
        for r in self.records().into_iter().filter(|r| r.up) {
            match self.units.start(&r.id) {
                Ok(()) => n += 1,
                Err(e) => eprintln!("vms: {}: {e:#}", r.id),
            }
        }
        n
    }

    /// What the firewall's filler reads (computers-access, games-access.sh): each
    /// running instance's owner, who else may reach it, and its host ports.
    pub fn access(&self) -> serde_json::Value {
        let servers: Vec<_> = self
            .records()
            .into_iter()
            .filter(|r| r.up)
            .map(|r| {
                let ports: Vec<u16> = std::iter::once(r.port)
                    .chain(r.forwards.iter().map(|f| f.host))
                    .collect();
                serde_json::json!({"owner": r.owner, "players": r.players, "ports": ports})
            })
            .collect();
        serde_json::json!({ "servers": servers })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as M};

    #[derive(Default, Clone)]
    struct Fake(Arc<M<BTreeMap<String, String>>>);
    impl Units for Fake {
        fn start(&self, id: &str) -> Result<()> {
            self.0.lock().unwrap().insert(id.into(), "active".into());
            Ok(())
        }
        fn stop(&self, id: &str) -> Result<()> {
            self.0.lock().unwrap().insert(id.into(), "inactive".into());
            Ok(())
        }
        fn state(&self, id: &str) -> String {
            self.0.lock().unwrap().get(id).cloned().unwrap_or_default()
        }
    }

    fn scratch(what: &str) -> (Manager, Fake) {
        let d = std::env::temp_dir().join(format!("dd-computers-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let f = Fake::default();
        let m = Manager::new(
            d,
            Limits {
                memory: 16384,
                threads: 8,
                disk: 200,
                port_base: 27200,
                port_count: 4,
                box_memory: 30000,
                box_threads: 7,
            },
            "david".into(),
            "100.64.0.2".into(),
            Box::new(f.clone()),
        )
        .unwrap();
        (m, f)
    }

    fn config(name: &str, memory: u64, threads: u32) -> Config {
        Config {
            name: name.into(),
            memory,
            threads,
            disk: 20,
            nix: String::new(),
            keys: vec!["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample tom@laptop".into()],
            ports: vec![],
            players: vec![],
            consent: Consent::Any,
        }
    }

    #[test]
    fn the_totals_count_every_instance_running_or_not() {
        let (m, _) = scratch("totals");
        m.save_config("tom", config("big", 12288, 6)).unwrap();
        let a = m.make("tom", "big", "a", true, &[]).unwrap();
        m.set_up("tom", &a.id, false).unwrap();
        let e = m
            .make("tom", "big", "b", true, &[])
            .unwrap_err()
            .to_string();
        assert!(e.contains("MiB together"), "{e}");
        m.save_config("tom", config("small", 4096, 2)).unwrap();
        m.make("tom", "small", "b", true, &[]).unwrap();
        let e = m
            .make("tom", "small", "c", true, &[])
            .unwrap_err()
            .to_string();
        assert!(e.contains("together"), "{e}");
    }

    #[test]
    fn someone_elses_box_needs_a_yes_and_own_needs_your_box() {
        let (m, _) = scratch("consent");
        m.save_config("tom", config("any", 1024, 1)).unwrap();
        assert!(m.make("tom", "any", "a", false, &[]).is_err());
        assert!(
            m.make("tom", "any", "a", true, &[])
                .unwrap()
                .anywhere
                .is_some()
        );
        let mut own = config("own", 1024, 1);
        own.consent = Consent::Own;
        m.save_config("tom", own.clone()).unwrap();
        assert!(m.make("tom", "own", "b", false, &[]).is_err());
        m.save_config("david", own).unwrap();
        assert!(
            m.make("david", "own", "b", false, &[])
                .unwrap()
                .anywhere
                .is_none()
        );
    }

    #[test]
    fn ports_are_handed_once_and_kept_across_an_apply() {
        let (m, _) = scratch("ports");
        let mut c = config("web", 1024, 1);
        c.ports = vec![8080];
        m.save_config("tom", c.clone()).unwrap();
        let a = m.make("tom", "web", "a", true, &[]).unwrap();
        assert_eq!(a.port, 27200);
        assert_eq!(
            a.forwards,
            vec![Forward {
                guest: 8080,
                host: 27201
            }]
        );
        c.ports = vec![8080, 3000];
        m.save_config("tom", c).unwrap();
        let a = m.apply("tom", &a.id, false).unwrap();
        assert_eq!(
            a.forwards[0],
            Forward {
                guest: 8080,
                host: 27201
            }
        );
        assert_eq!(a.forwards[1].host, 27202);
        // the pool has four: a second needs two more and only one is left
        m.save_config("ann", config("x", 1024, 1)).unwrap();
        let mut two = config("two", 1024, 1);
        two.ports = vec![80];
        m.save_config("ann", two).unwrap();
        assert!(m.make("ann", "two", "b", true, &[]).is_err());
        assert_eq!(m.make("ann", "x", "b", true, &[]).unwrap().port, 27203);
    }

    #[test]
    fn only_the_owner_and_only_stopped() {
        let (m, f) = scratch("owner");
        m.save_config("tom", config("c", 1024, 1)).unwrap();
        let a = m.make("tom", "c", "a", true, &[]).unwrap();
        assert!(m.set_up("ann", &a.id, false).is_err());
        assert!(m.delete("ann", &a.id).is_err());
        assert!(m.delete("tom", &a.id).is_err()); // running
        m.set_up("tom", &a.id, false).unwrap();
        assert_eq!(f.state(&a.id), "inactive");
        m.delete("tom", &a.id).unwrap();
        assert!(m.instances("tom").is_empty());
        assert!(m.record("tom", "../../etc/x").is_err());
    }

    #[test]
    fn a_guests_report_is_never_followed() {
        let (m, _) = scratch("report");
        m.save_config("tom", config("c", 1024, 1)).unwrap();
        let a = m.make("tom", "c", "a", true, &[]).unwrap();
        let rep = m.instance_dir(&a.id).join("report");
        std::fs::create_dir_all(&rep).unwrap();
        // another member's record, linked in by the guest
        std::os::unix::fs::symlink(m.record_path(&a.id), rep.join("apply.log")).unwrap();
        assert_eq!(m.apply_log("tom", &a.id).unwrap(), "");
        std::os::unix::fs::symlink(m.record_path(&a.id), rep.join("apply.json")).unwrap();
        assert!(m.view(a.clone()).applied.is_none());
        std::fs::remove_file(rep.join("apply.json")).unwrap();
        std::fs::write(rep.join("apply.json"), r#"{"ok": true}"#).unwrap();
        assert!(m.view(a).applied.is_some());
    }

    #[test]
    fn over_the_allowance_no_new_computer() {
        let (mut m, _) = scratch("allowance");
        let ledger = m.dir.join("ledger.json");
        std::fs::write(
            &ledger,
            r#"{"budget": 1000, "members": {"tom": {"photos": 600, "computers": 400}}}"#,
        )
        .unwrap();
        m.ledger = Some(ledger.clone());
        m.save_config("tom", config("c", 1024, 1)).unwrap();
        let e = m.make("tom", "c", "a", true, &[]).unwrap_err().to_string();
        assert!(e.contains("allowance"), "{e}");
        std::fs::write(
            &ledger,
            r#"{"budget": 1000, "members": {"tom": {"photos": 600}}}"#,
        )
        .unwrap();
        m.make("tom", "c", "a", true, &[]).unwrap();
    }

    #[test]
    fn a_small_box_takes_small_computers() {
        let (mut m, _) = scratch("small");
        m.limits.box_memory = 6144;
        m.limits.box_threads = 3;
        m.save_config("tom", config("big", 8192, 2)).unwrap();
        let e = m
            .make("tom", "big", "a", true, &[])
            .unwrap_err()
            .to_string();
        assert!(e.contains("this box can take"), "{e}");
        m.save_config("tom", config("small", 2048, 2)).unwrap();
        m.make("tom", "small", "a", true, &[]).unwrap();
    }

    #[test]
    fn everyone_has_the_default_config_and_each_computer_its_own_key() {
        let (m, _) = scratch("default");
        assert!(m.configs("ann").contains_key(DEFAULT));
        assert_eq!(m.configs("ann")[DEFAULT].consent, Consent::Any);
        assert_eq!(m.configs("david")[DEFAULT].consent, Consent::Own);
        let r = m.make("ann", DEFAULT, "a", true, &[]).unwrap();
        // its own key: the public half let in, the private half its owner's
        assert!(r.key.starts_with("ssh-ed25519 ") && r.key.ends_with(" ann@a"));
        assert_eq!(r.all_keys(), vec![r.key.clone()]);
        let (name, private) = m.private_key("ann", &r.id).unwrap();
        assert_eq!(name, "a");
        assert!(private.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(m.private_key("tom", &r.id).is_err());
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(m.key_path(&r.id)).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        m.rekey("ann", &r.id).unwrap();
        assert_ne!(m.record("ann", &r.id).unwrap().key, r.key);
        assert!(m.delete_config("ann", DEFAULT).is_err());
    }

    /// ssh itself reads the key and gets the same public half back, where
    /// there is an ssh-keygen to ask (not in a build's sandbox)
    #[test]
    fn ssh_reads_the_key() {
        let (public, private) = ssh_key("tom@dev").unwrap();
        let d = std::env::temp_dir().join(format!("dd-computers-key-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("key");
        std::fs::write(&f, &private).unwrap();
        std::fs::set_permissions(&f, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let Ok(out) = std::process::Command::new("ssh-keygen")
            .arg("-y")
            .arg("-f")
            .arg(&f)
            .output()
        else {
            return;
        };
        std::fs::remove_dir_all(&d).ok();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), public);
    }

    #[test]
    fn a_config_out_of_bounds_is_not_saved() {
        let (m, _) = scratch("bounds");
        assert!(m.save_config("tom", config("c", 256, 1)).is_err());
        assert!(m.save_config("tom", config("c", 1024, 9)).is_err());
        assert!(m.save_config("tom", config("Bad Name", 1024, 1)).is_err());
        let mut k = config("c", 1024, 1);
        k.keys = vec!["ssh-ed25519 AAAA\nssh-ed25519 BBBB".into()];
        assert!(m.save_config("tom", k).is_err());
        let mut p = config("c", 1024, 1);
        p.ports = vec![22];
        assert!(m.save_config("tom", p).is_err());
    }
}
