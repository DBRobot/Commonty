//! Members' remote computers on a box (docs/dev/remote-computers.md).
//!
//! Nothing about a computer is decided here. Its record is agreed state,
//! judged by the control plane (state/computers) and served by this box's
//! dd-control at 127.0.0.1:4195/computers; the plan puts it on a box, and
//! dd-control holds its lease and starts `dd-computer@<id>` there. This
//! crate is the box's side of that:
//!
//! - `prepare <id>`: before the guest starts, the agreed record, checked
//!   again here (the policy's bounds, the pool, the owner's totals on this
//!   box), written where the unit reads it, and the guest's share (the
//!   owner's name, their SSH public keys, their Nix file);
//! - `share <id>`: a running computer's share, rewritten when the agreed
//!   record changes, so a saved Nix file or key reaches the guest;
//! - the page and its API behind compute.<domain>: what the agreed state
//!   says of the member's computers and where the plan puts each. Changing
//!   anything is a signed item the page or `dd computer` puts at the gate.
//!
//! Each computer has a user of its own (DynamicUser=, `dd-computer-<id>`),
//! which alone owns its directory: its QEMU, its prepare and its share run
//! as that user, and nothing here reads what a guest wrote.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What one member's computers may take together, and the pool their ports
/// come from (fleet/policy.json computers): held to here as the judge does
#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    pub memory: u64,
    pub threads: u64,
    pub disk: u64,
    pub port_base: u64,
    pub port_count: u64,
}

pub const MIN_MEMORY: u64 = 512;
pub const MIN_DISK: u64 = 8;
pub const MAX_NIX: usize = 44 * 1024;
pub const MAX_KEYS: usize = 8;
pub const MAX_KEY: usize = 2048;
pub const MAX_FORWARDS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Forward {
    pub guest: u64,
    pub host: u64,
}

/// One computer as /computers lists it
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Listed {
    pub owner: String,
    pub id: String,
    pub name: String,
    pub config: String,
    pub memory: u64,
    pub threads: u64,
    pub disk: u64,
    pub nix: String,
    pub players: Vec<String>,
    pub keys: Vec<String>,
    pub consent: String,
    pub anywhere: bool,
    pub port: u64,
    pub forwards: Vec<Forward>,
    pub asked: bool,
    pub up: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct View {
    pub computers: BTreeMap<String, Listed>,
    pub configs: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// a member's name, as the guest makes it a user and the box a file name
pub fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
}

const KEY_TYPES: [&str; 7] = [
    "ssh-ed25519",
    "ssh-rsa",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

/// One OpenSSH public key line, as the judge takes it (state/computers
/// ValidKey): a type, the key whose blob names that type, and a comment of
/// printable text at most. No options, nothing past one line: it becomes a
/// line of the guest's authorized keys.
pub fn valid_key(k: &str) -> bool {
    use base64::Engine as _;
    if k.len() > MAX_KEY {
        return false;
    }
    let mut parts = k.splitn(3, ' ');
    let (Some(kind), Some(b64)) = (parts.next(), parts.next()) else {
        return false;
    };
    if let Some(c) = parts.next()
        && (c.is_empty() || c.len() > 128 || !c.bytes().all(|b| (b' '..=b'~').contains(&b)))
    {
        return false;
    }
    if !KEY_TYPES.contains(&kind) {
        return false;
    }
    let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return false;
    };
    if blob.len() < 4 {
        return false;
    }
    let n = u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
    n == kind.len() && blob.len() > 4 + n && &blob[4..4 + n] == kind.as_bytes()
}

impl Limits {
    fn in_pool(&self, p: u64) -> bool {
        p >= self.port_base && p < self.port_base.saturating_add(self.port_count)
    }

    /// One computer's record within the policy's bounds, as the judge
    /// checked it: the box is not trusted to have been told the truth, nor
    /// is any other box
    pub fn check(&self, c: &Listed) -> Result<()> {
        if !valid_id(&c.id) || !valid_name(&c.owner) {
            bail!("not a computer's id or not an owner's name");
        }
        if !c.up {
            bail!("it is not agreed up");
        }
        if c.memory < MIN_MEMORY || c.memory > self.memory {
            bail!("memory is {MIN_MEMORY} to {} MiB", self.memory);
        }
        if c.threads < 1 || c.threads > self.threads {
            bail!("threads are 1 to {}", self.threads);
        }
        if c.disk < MIN_DISK || c.disk > self.disk {
            bail!("a disk is {MIN_DISK} to {} GiB", self.disk);
        }
        if c.nix.len() > MAX_NIX {
            bail!("the Nix file is too big");
        }
        if c.keys.len() > MAX_KEYS || !c.keys.iter().all(|k| valid_key(k)) {
            bail!("its SSH keys are not public keys, one to a line");
        }
        if !self.in_pool(c.port) || c.forwards.len() > MAX_FORWARDS {
            bail!("its ports are not the pool's");
        }
        let mut seen = vec![c.port];
        for f in &c.forwards {
            if !self.in_pool(f.host) || f.guest == 0 || f.guest == 22 || f.guest > 65535 {
                bail!("its ports are not the pool's");
            }
            if seen.contains(&f.host) {
                bail!("a port twice");
            }
            seen.push(f.host);
        }
        Ok(())
    }

    /// The owner's computers running here together, this one with them,
    /// within their totals; and no port of this one another here has
    pub fn fits_beside(&self, c: &Listed, here: &[&Listed]) -> Result<()> {
        let (mut mem, mut thr, mut disk) = (c.memory, c.threads, c.disk);
        let mine: Vec<u64> = std::iter::once(c.port)
            .chain(c.forwards.iter().map(|f| f.host))
            .collect();
        for o in here.iter().filter(|o| o.id != c.id) {
            if o.owner == c.owner {
                mem = mem.saturating_add(o.memory);
                thr = thr.saturating_add(o.threads);
                disk = disk.saturating_add(o.disk);
            }
            let theirs = std::iter::once(o.port).chain(o.forwards.iter().map(|f| f.host));
            if theirs.into_iter().any(|p| mine.contains(&p)) {
                bail!("a port another computer here has");
            }
        }
        if mem > self.memory || thr > self.threads || disk > self.disk {
            bail!(
                "{}'s computers here would take {mem} MiB, {thr} threads and {disk} GiB; the limit is {} MiB, {} and {} GiB",
                c.owner,
                self.memory,
                self.threads,
                self.disk
            );
        }
        Ok(())
    }
}

/// A GET to dd-control on this box's loopback: plain HTTP/1.0, bounded
pub fn get(addr: &str, path: &str) -> Result<Vec<u8>> {
    let mut s =
        std::net::TcpStream::connect(addr).with_context(|| format!("dd-control at {addr}"))?;
    s.set_read_timeout(Some(Duration::from_secs(20)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )?;
    let mut b = Vec::new();
    s.take(64 << 20).read_to_end(&mut b)?;
    let head_end = b
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("no answer from dd-control")?;
    let status = std::str::from_utf8(&b[..head_end])
        .ok()
        .and_then(|h| h.split(' ').nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    if status != 200 {
        bail!("dd-control said {status} to {path}");
    }
    Ok(b[head_end + 4..].to_vec())
}

pub fn view(control: &str) -> Result<View> {
    Ok(serde_json::from_slice(&get(control, "/computers")?)?)
}

/// computer id -> the box the plan gives it, from the plan every box makes
/// alike; and why the ones it places nowhere are not
pub fn placed(control: &str) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Plan {
        boxes: BTreeMap<String, Vec<serde_json::Value>>,
        unplaced: BTreeMap<String, String>,
    }
    let p: Plan = serde_json::from_slice(&get(control, "/plan")?)?;
    let mut on = BTreeMap::new();
    for (b, ds) in p.boxes {
        for d in ds {
            if let Some(id) = d["service"]
                .as_str()
                .and_then(|s| s.strip_prefix("computer-"))
            {
                on.insert(id.to_string(), b.clone());
            }
        }
    }
    let why = p
        .unplaced
        .into_iter()
        .filter_map(|(s, w)| s.strip_prefix("computer-").map(|id| (id.to_string(), w)))
        .collect();
    Ok((on, why))
}

/// The computers this box runs: their flags, which dd-control raises while
/// it holds each one's lease (/run/dd-placed/computer-<id>)
pub fn flagged(placed: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(placed)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix("computer-"))
                .filter(|id| valid_id(id))
                .map(str::to_string)
        })
        .collect();
    out.sort();
    out
}

/// written beside, then moved: a reader never sees half a file
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let tmp = path.with_extension("new");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .with_context(|| format!("writing {}", tmp.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("moving {}", path.display()))?;
    Ok(())
}

/// The box's side of the computers
pub struct Host {
    /// /var/lib/dd-computer: each computer's own directory, <id>/, its
    /// user's alone (StateDirectory=, DynamicUser=)
    pub dir: PathBuf,
    /// /run/dd-placed
    pub placed: PathBuf,
    /// dd-control, 127.0.0.1:4195
    pub control: String,
    pub limits: Limits,
}

impl Host {
    fn instance(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// What the guest reads, from the agreed record: its owner's name and
    /// SSH public keys, and their Nix file (an empty one is the empty
    /// module). Only what changed is written.
    pub fn write_share(&self, c: &Listed) -> Result<bool> {
        let share = self.instance(&c.id).join("config");
        std::fs::create_dir_all(&share)?;
        let owner = serde_json::to_vec(&serde_json::json!({"name": c.owner, "keys": c.keys}))?;
        let nix = if c.nix.trim().is_empty() {
            "{ ... }: { }\n".to_string()
        } else {
            c.nix.clone()
        };
        let mut changed = false;
        for (f, b) in [("owner.json", owner), ("member.nix", nix.into_bytes())] {
            let p = share.join(f);
            if std::fs::read(&p).ok().as_deref() != Some(&b[..]) {
                write_atomic(&p, &b, 0o600)?;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// Before `dd-computer@<id>` starts: the record as agreed now, checked,
    /// written for the unit (<id>/record.json) and its share. Refused
    /// unless this box holds its flag and the record is agreed up.
    pub fn prepare(&self, id: &str) -> Result<Listed> {
        if !valid_id(id) {
            bail!("not a computer's id: {id}");
        }
        // what tells the guest's unit this start was prepared: gone until it is
        let ready = self.ready(id);
        if let Err(e) = std::fs::remove_file(&ready)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e.into());
        }
        let here = flagged(&self.placed);
        if !here.iter().any(|h| h == id) {
            bail!("computer-{id} is not this box's to run (no flag)");
        }
        let v = view(&self.control)?;
        let c = v
            .computers
            .get(id)
            .with_context(|| format!("no agreed computer {id}"))?;
        if c.id != id {
            bail!("the agreed record of {id} names another id");
        }
        self.limits.check(c)?;
        let beside: Vec<&Listed> = here
            .iter()
            .filter(|h| *h != id)
            .filter_map(|h| v.computers.get(h))
            .filter(|o| o.up)
            .collect();
        self.limits.fits_beside(c, &beside)?;
        std::fs::create_dir_all(self.instance(id))?;
        self.write_share(c)?;
        let rec = serde_json::to_vec(&serde_json::json!({
            "owner": c.owner, "id": c.id, "name": c.name, "memory": c.memory,
            "threads": c.threads, "disk": c.disk, "port": c.port, "forwards": c.forwards,
            "consent": c.consent,
        }))?;
        write_atomic(&self.instance(id).join("record.json"), &rec, 0o600)?;
        write_atomic(&ready, b"", 0o600)?;
        Ok(c.clone())
    }

    /// prepared for this start (<id>/.prepared): the guest's unit takes it,
    /// and runs on nothing older
    pub fn ready(&self, id: &str) -> PathBuf {
        self.instance(id).join(".prepared")
    }

    /// One running computer's share, as agreed now: a Nix file or a key
    /// saved reaches its guest without a restart. One that is no longer
    /// agreed up, or not here, is left: dd-control stops it. Whether it
    /// was written.
    pub fn share(&self, id: &str) -> Result<bool> {
        if !valid_id(id) || !flagged(&self.placed).iter().any(|h| h == id) {
            return Ok(false);
        }
        let v = view(&self.control)?;
        let Some(c) = v.computers.get(id) else {
            return Ok(false);
        };
        if c.id != id || self.limits.check(c).is_err() || !self.instance(id).is_dir() {
            return Ok(false);
        }
        self.write_share(c)
    }

    /// What the firewall's filler reads (games-access.sh), in the games'
    /// form: a list of each computer running here, its owner, whom it lets
    /// in and its ports
    pub fn access(&self, v: &View) -> serde_json::Value {
        let mut servers = Vec::new();
        for id in flagged(&self.placed) {
            if let Some(c) = v.computers.get(&id).filter(|c| c.up && c.id == id) {
                let ports: Vec<u64> = std::iter::once(c.port)
                    .chain(c.forwards.iter().map(|f| f.host))
                    .filter(|p| self.limits.in_pool(*p))
                    .collect();
                servers.push(serde_json::json!({"id": id, "owner": c.owner, "players": c.players, "ports": ports}));
            }
        }
        serde_json::json!({ "servers": servers })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> Limits {
        Limits {
            memory: 16384,
            threads: 8,
            disk: 200,
            port_base: 27200,
            port_count: 200,
        }
    }

    fn key() -> String {
        use base64::Engine as _;
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[7; 32]);
        format!(
            "ssh-ed25519 {}",
            base64::engine::general_purpose::STANDARD.encode(blob)
        )
    }

    fn comp(id: &str, owner: &str, mem: u64, port: u64) -> Listed {
        Listed {
            owner: owner.into(),
            id: id.into(),
            memory: mem,
            threads: 2,
            disk: 32,
            port,
            up: true,
            keys: vec![key()],
            ..Default::default()
        }
    }

    #[test]
    fn keys_are_public_keys_on_one_line() {
        let k = key();
        assert!(valid_key(&k));
        assert!(valid_key(&format!("{k} ann@laptop")));
        assert!(!valid_key(&format!("command=\"sh\" {k}")));
        assert!(!valid_key(&format!("{k} a\nssh-ed25519 AAAA")));
        assert!(!valid_key(&k.replacen("ssh-ed25519", "ssh-rsa", 1)));
        assert!(!valid_key("ssh-ed25519 !!!"));
        assert!(!valid_key(&format!("{k} ")));
    }

    #[test]
    fn a_record_is_held_to_the_policy_here_too() {
        let l = lim();
        let ok = comp("0123456789abcdef", "ann", 4096, 27200);
        assert!(l.check(&ok).is_ok());
        for bad in [
            Listed {
                up: false,
                ..ok.clone()
            },
            Listed {
                memory: 32768,
                ..ok.clone()
            },
            Listed {
                threads: 9,
                ..ok.clone()
            },
            Listed {
                port: 22,
                ..ok.clone()
            },
            Listed {
                owner: "../root".into(),
                ..ok.clone()
            },
            Listed {
                id: "../../etc".into(),
                ..ok.clone()
            },
            Listed {
                keys: vec!["no".into()],
                ..ok.clone()
            },
            Listed {
                forwards: vec![Forward {
                    guest: 80,
                    host: 80,
                }],
                ..ok.clone()
            },
        ] {
            assert!(l.check(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_owners_computers_here_stay_within_their_totals() {
        let l = lim();
        let a = comp("0123456789abcdef", "ann", 12288, 27200);
        let b = comp("fedcba9876543210", "ann", 8192, 27201);
        let c = comp("00000000000000c1", "bob", 8192, 27202);
        assert!(l.fits_beside(&b, &[&a]).is_err());
        assert!(l.fits_beside(&c, &[&a]).is_ok());
        let clash = comp("00000000000000c2", "bob", 1024, 27200);
        assert!(l.fits_beside(&clash, &[&a]).is_err());
    }

    /// the firewall's filler reads a list, as the games' manager writes it
    #[test]
    fn access_is_a_list_of_what_runs_here() {
        let d = std::env::temp_dir().join(format!("computers-access-{}", std::process::id()));
        std::fs::create_dir_all(d.join("placed")).unwrap();
        std::fs::write(d.join("placed/computer-0123456789abcdef"), b"{}").unwrap();
        let h = Host {
            dir: d.clone(),
            placed: d.join("placed"),
            control: String::new(),
            limits: lim(),
        };
        let mut v = View::default();
        for c in [
            comp("0123456789abcdef", "ann", 1024, 27200),
            comp("fedcba9876543210", "bob", 1024, 27201),
        ] {
            v.computers.insert(c.id.clone(), c);
        }
        let a = h.access(&v);
        assert_eq!(
            a,
            serde_json::json!({"servers": [{"id": "0123456789abcdef", "owner": "ann", "players": [], "ports": [27200]}]})
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_share_is_written_once_per_change() {
        let d = std::env::temp_dir().join(format!("computers-share-{}", std::process::id()));
        let h = Host {
            dir: d.clone(),
            placed: d.join("placed"),
            control: String::new(),
            limits: lim(),
        };
        let mut c = comp("0123456789abcdef", "ann", 1024, 27200);
        assert!(h.write_share(&c).unwrap());
        assert!(!h.write_share(&c).unwrap());
        let nix = std::fs::read_to_string(d.join("0123456789abcdef/config/member.nix"));
        assert_eq!(nix.unwrap(), "{ ... }: { }\n");
        c.nix = "{ pkgs, ... }: { }".into();
        assert!(h.write_share(&c).unwrap());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
