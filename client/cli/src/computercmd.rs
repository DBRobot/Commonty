//! `dd computer`: your remote computers (docs/dev/remote-computers.md).
//!
//! What a computer is, and whether it runs, is agreed state: each change is
//! an item this device signs with your root key and puts at the gate
//! (PUT /_dd/computers/{kind}), which the control plane judges. What is
//! agreed, and where each runs, comes from their page's API,
//! compute.<domain>/api/state, with this device's token.
//!
//! Its SSH key is made here, on your device: the private half stays in
//! ~/.config/commonty/computers/<name>.device-key, only the public half goes
//! into the signed item, and the computer's sshd lets in exactly the keys
//! its item names.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::{Value, json};

#[derive(Subcommand)]
pub enum ComputerCmd {
    /// Your remote computers and configs, and what they take of your limits
    List,
    /// Save a config: what remote computers are made from
    Config {
        name: String,
        /// MiB
        #[arg(long, default_value_t = 4096)]
        memory: u64,
        #[arg(long, default_value_t = 2)]
        threads: u64,
        /// GiB
        #[arg(long, default_value_t = 32)]
        disk: u64,
        /// a NixOS module: the computer's packages and services
        #[arg(long)]
        nix: Option<PathBuf>,
        /// an SSH public key file to let in (repeat for more)
        #[arg(long = "key")]
        keys: Vec<PathBuf>,
        /// a port to open besides SSH (repeat for more)
        #[arg(long = "port")]
        ports: Vec<u64>,
        /// a friend who may reach it too (repeat for more)
        #[arg(long = "let-in")]
        players: Vec<String>,
        /// may run on anyone's box, not only yours
        #[arg(long)]
        anywhere: bool,
    },
    /// Delete a config; remote computers made from it keep running
    Forget { name: String },
    /// A new remote computer from a saved config, started; named as the
    /// config unless a name is given
    Make {
        config: String,
        name: Option<String>,
        /// yes to running on someone else's box, without being asked
        #[arg(long)]
        yes: bool,
    },
    /// Start it; it stays up across reboots until stopped
    Start { name: String },
    /// Shut it down cleanly; /home is kept, and its system unless a new release changes its base
    Stop { name: String },
    /// Its config again: packages and keys now, size and ports at its next start
    Apply {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Gone, with both its disks
    Delete { name: String },
    /// SSH into it, with the key this device made for it
    Ssh {
        name: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

struct Api {
    door: String,
    compute: String,
    token: String,
    root: Option<ed25519_dalek::SigningKey>,
    http: reqwest::Client,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// a version that grows from one save to the next: the time, in ms
fn version() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Api {
    async fn get(&self, path: &str) -> Result<String> {
        let resp = self
            .http
            .get(format!("{}{path}", self.compute))
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("asking {}", self.compute))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let why = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("the box said {status}"));
            bail!("{why}");
        }
        Ok(text)
    }

    async fn state(&self) -> Result<Value> {
        Ok(serde_json::from_str(&self.get("/api/state").await?)?)
    }

    /// a computer by its name, as its owner calls it
    async fn find(&self, name: &str) -> Result<Value> {
        self.state().await?["computers"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["name"] == name).cloned())
            .with_context(|| format!("you have no remote computer called {name}"))
    }

    /// `body`, signed by your root as `kind`, put at the gate
    async fn put(&self, kind: &str, body: &Value) -> Result<()> {
        let root = self.root.as_ref().context(
            "no root key on this device: a remote computer is signed by your account's main key",
        )?;
        let env = identity::envelope::seal_json(kind, body, root)?;
        let resp = self
            .http
            .put(format!("{}/_dd/computers/{kind}", self.door))
            .bearer_auth(&self.token)
            .json(&env)
            .send()
            .await
            .with_context(|| format!("asking {}", self.door))?;
        let status = resp.status();
        if !status.is_success() {
            bail!(
                "the gate said {status}: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }
}

fn gib(mib: u64) -> String {
    format!("{:.1} GiB", mib as f64 / 1024.0)
}

/// the warning the page shows, asked here
fn agree() -> Result<bool> {
    eprintln!(
        "This runs on someone else's box. Whoever has that box in their house can read\n\
         this computer's disk and memory: your files, and anything you sign in to on it,\n\
         such as Claude."
    );
    eprint!("Run it on someone else's box? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

fn plain_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 32
        || name.starts_with('-')
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("a name is lowercase letters, digits and dashes, up to 32: {name}");
    }
    Ok(())
}

/// where this device keeps its computers' keys: a directory of this flow's
/// own (0700), never ~/.ssh, where a key from elsewhere could already be
fn keys_dir() -> Result<PathBuf> {
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(c) if !c.is_empty() => PathBuf::from(c),
        _ => PathBuf::from(std::env::var_os("HOME").context("no HOME")?).join(".config"),
    };
    Ok(config.join("commonty").join("computers"))
}

fn key_path(name: &str) -> Result<PathBuf> {
    key_path_in(&keys_dir()?, name)
}

fn key_path_in(dir: &std::path::Path, name: &str) -> Result<PathBuf> {
    plain_name(name)?;
    Ok(dir.join(format!("{name}.device-key")))
}

/// the comment only this flow writes on a key it made
fn marker(name: &str) -> String {
    format!("commonty computer device key {name}")
}

/// This device's key for the computer called `name`: made the first time
/// by this flow, the private half kept in its own file (0600, in a 0700
/// directory). A file there that is not this user's, not 0600, or not one
/// this flow made is refused, never adopted. Its public line.
fn device_key(name: &str) -> Result<String> {
    device_key_in(&keys_dir()?, name)
}

// this user's own file, readable by no one else (on Windows the profile
// directory's ACL is what keeps it private)
#[cfg(unix)]
fn own_private(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    // SAFETY: geteuid has no preconditions
    m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o777 == 0o600
}

#[cfg(not(unix))]
fn own_private(_: &std::fs::Metadata) -> bool {
    true
}

fn device_key_in(dir: &std::path::Path, name: &str) -> Result<String> {
    let path = key_path_in(dir, name)?;
    match std::fs::symlink_metadata(&path) {
        Ok(m) => {
            if !m.file_type().is_file() || !own_private(&m) {
                bail!(
                    "{} is not a key this device made for {name} (not your own file, or not 0600); move it away",
                    path.display()
                );
            }
            let k = ssh_key::PrivateKey::from_openssh(std::fs::read_to_string(&path)?)
                .with_context(|| format!("reading {}", path.display()))?;
            if k.comment() != marker(name) {
                bail!(
                    "{} is not a key this device made for {name}; move it away",
                    path.display()
                );
            }
            return Ok(k.public_key().to_openssh()?);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| anyhow::anyhow!("randomness: {e}"))?;
    let mut key = ssh_key::PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&seed));
    key.set_comment(marker(name));
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let tmp = dir.join(format!(".{name}.device-key.tmp"));
    {
        let _ = std::fs::remove_file(&tmp);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(key.to_openssh(ssh_key::LineEnding::LF)?.as_bytes())?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(key.public_key().to_openssh()?)
}

fn read_keys(files: &[PathBuf]) -> Result<Vec<String>> {
    let mut ks = Vec::new();
    for p in files {
        let k = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        ks.extend(
            k.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_string),
        );
    }
    Ok(ks)
}

fn str_list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

/// the Default config every member has, which is not an item
fn default_config(user: &str) -> Value {
    json!({"owner": user, "name": "default", "memory": 4096, "threads": 2, "disk": 32,
        "nix": "", "ports": [], "players": [], "keys": [], "consent": "any"})
}

/// a computer's item from a config, as it would be signed now
fn computer_body(
    user: &str,
    id: &str,
    name: &str,
    cfg: &Value,
    keys: Vec<Value>,
    anywhere: bool,
) -> Value {
    json!({
        "owner": user, "id": id, "name": name,
        "config": cfg["name"], "memory": cfg["memory"], "threads": cfg["threads"],
        "disk": cfg["disk"], "nix": cfg["nix"], "ports": str_list(&cfg["ports"]),
        "players": str_list(&cfg["players"]), "keys": keys,
        "consent": cfg["consent"], "anywhere": anywhere, "version": version(),
    })
}

fn new_id() -> Result<String> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("randomness: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// its config's keys, and this device's own for it
fn keys_for(name: &str, cfg: &Value) -> Result<Vec<Value>> {
    let mine = device_key(name)?;
    let mut ks: Vec<Value> = str_list(&cfg["keys"])
        .into_iter()
        .filter(|k| k.as_str() != Some(mine.as_str()))
        .collect();
    ks.insert(0, json!(mine));
    ks.truncate(8);
    Ok(ks)
}

fn config_of(s: &Value, name: &str) -> Result<Value> {
    if let Some(c) = s["configs"]
        .as_array()
        .and_then(|a| a.iter().find(|c| c["name"] == name).cloned())
    {
        return Ok(c);
    }
    if name == "default" {
        return Ok(default_config(s["user"].as_str().unwrap_or("")));
    }
    bail!("you have no config called {name}")
}

async fn power(api: &Api, name: &str, up: bool) -> Result<()> {
    let i = api.find(name).await?;
    api.put(
        "computer-power",
        &json!({"id": i["id"], "up": up, "at": now_secs()}),
    )
    .await?;
    println!("{name}: {}", if up { "starting" } else { "stopping" });
    Ok(())
}

pub async fn run(cmd: ComputerCmd, keys: &auth::Store, dirs: &[String]) -> Result<()> {
    let (_, _, token) = media::gate::Opener::load(keys)?;
    let door = media::gate::door_base(dirs)?;
    let compute = match door.split_once("://home.") {
        Some((scheme, rest)) => format!("{scheme}://compute.{rest}"),
        None => door.clone(),
    };
    let api = Api {
        door,
        compute,
        token,
        root: crate::who::load_root(keys)?,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
    };
    match cmd {
        ComputerCmd::List => {
            let s = api.state().await?;
            println!(
                "using {} of {}, {} of {} threads, {} of {} GiB",
                gib(s["used"]["memory"].as_u64().unwrap_or(0)),
                gib(s["limits"]["memory"].as_u64().unwrap_or(0)),
                s["used"]["threads"],
                s["limits"]["threads"],
                s["used"]["disk"],
                s["limits"]["disk"],
            );
            for i in s["computers"].as_array().into_iter().flatten() {
                let at = match (i["box"].as_str(), i["address"].as_str()) {
                    (Some(b), Some(a)) => format!(
                        "on {b}, ssh -p {} {}@{a}",
                        i["port"],
                        i["owner"].as_str().unwrap_or("")
                    ),
                    (Some(b), None) => format!("on {b}"),
                    _ => i["waiting"]
                        .as_str()
                        .unwrap_or("waiting for a box")
                        .to_string(),
                };
                println!(
                    "{:<20} {:<7} {} · {} threads · {} GiB  {at}",
                    i["name"].as_str().unwrap_or(""),
                    if i["up"] == true { "up" } else { "stopped" },
                    gib(i["memory"].as_u64().unwrap_or(0)),
                    i["threads"],
                    i["disk"],
                );
            }
            for c in s["configs"].as_array().into_iter().flatten() {
                println!(
                    "config {:<13} {} · {} threads · {} GiB · {}",
                    c["name"].as_str().unwrap_or(""),
                    gib(c["memory"].as_u64().unwrap_or(0)),
                    c["threads"],
                    c["disk"],
                    if c["consent"] == "any" {
                        "anyone's box"
                    } else {
                        "my boxes"
                    }
                );
            }
        }
        ComputerCmd::Config {
            name,
            memory,
            threads,
            disk,
            nix,
            keys,
            ports,
            players,
            anywhere,
        } => {
            plain_name(&name)?;
            let s = api.state().await?;
            let user = s["user"].as_str().context("who you are")?.to_string();
            let nix = match nix {
                Some(p) => std::fs::read_to_string(&p)
                    .with_context(|| format!("reading {}", p.display()))?,
                None => String::new(),
            };
            let body = json!({
                "owner": user, "name": name, "memory": memory, "threads": threads, "disk": disk,
                "nix": nix, "ports": ports, "players": players, "keys": read_keys(&keys)?,
                "consent": if anywhere { "any" } else { "own" }, "version": version(),
            });
            api.put("computer-config", &body).await?;
            println!("config {name} sent; `dd computer list` shows it once it is agreed");
        }
        ComputerCmd::Forget { name } => {
            let s = api.state().await?;
            let mut c = config_of(&s, &name)?;
            if name == "default"
                && !s["configs"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|c| c["name"] == "default"))
            {
                bail!("the Default config is everyone's; save one called default to change it");
            }
            c["gone"] = json!(true);
            c["version"] = json!(version());
            api.put("computer-config", &c).await?;
            println!("config {name} deleted");
        }
        ComputerCmd::Make { config, name, yes } => {
            let name = name.unwrap_or_else(|| config.clone());
            plain_name(&name)?;
            let s = api.state().await?;
            let user = s["user"].as_str().context("who you are")?.to_string();
            if s["computers"]
                .as_array()
                .is_some_and(|a| a.iter().any(|c| c["name"] == name.as_str()))
            {
                bail!("you have a remote computer called {name}");
            }
            let cfg = config_of(&s, &config)?;
            let anywhere = cfg["consent"] == "any";
            let yes = if anywhere && !yes { agree()? } else { yes };
            if anywhere && !yes {
                bail!("not made");
            }
            let id = new_id()?;
            let body = computer_body(&user, &id, &name, &cfg, keys_for(&name, &cfg)?, anywhere);
            api.put("computer", &body).await?;
            println!(
                "{name} sent: once agreed it is placed on a box and starts.\n\
                 `dd computer list` says where; then `dd computer ssh {name}`"
            );
        }
        ComputerCmd::Start { name } => power(&api, &name, true).await?,
        ComputerCmd::Stop { name } => power(&api, &name, false).await?,
        ComputerCmd::Apply { name, yes } => {
            let s = api.state().await?;
            let user = s["user"].as_str().context("who you are")?.to_string();
            let i = s["computers"]
                .as_array()
                .and_then(|a| a.iter().find(|c| c["name"] == name.as_str()).cloned())
                .with_context(|| format!("you have no remote computer called {name}"))?;
            let cfg = config_of(&s, i["config"].as_str().unwrap_or(""))?;
            let anywhere = cfg["consent"] == "any";
            let widens = anywhere && i["consent"] != "any";
            let yes = if widens && !yes {
                agree()?
            } else {
                yes || !widens
            };
            if anywhere && !yes {
                bail!("not applied");
            }
            let id = i["id"].as_str().context("its id")?;
            let body = computer_body(&user, id, &name, &cfg, keys_for(&name, &cfg)?, anywhere);
            api.put("computer", &body).await?;
            println!(
                "{name}: applied; its packages and keys reach it now, its size at its next start"
            );
        }
        ComputerCmd::Delete { name } => {
            let s = api.state().await?;
            let i = api.find(&name).await?;
            api.put(
                "computer-delete",
                &json!({"owner": s["user"], "id": i["id"], "at": now_secs()}),
            )
            .await?;
            println!("{name} deleted");
        }
        ComputerCmd::Ssh { name, args } => {
            let i = api.find(&name).await?;
            let status = ssh(&i, &name)?
                // what to run there: after the destination, a command, never options
                .args(args)
                .status()
                .context("running ssh")?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }
    Ok(())
}

/// ssh to the computer, with this device's key for it; the command after
fn ssh(i: &Value, name: &str) -> Result<std::process::Command> {
    let key = key_path(name)?;
    if !key.exists() {
        bail!(
            "this device has no key for {name}: `dd computer apply {name}` makes one and lets it in"
        );
    }
    let host = i["address"].as_str().context("it runs nowhere yet")?;
    let user = i["owner"].as_str().unwrap_or("");
    // the box's word, and a box is not trusted: nothing it says may
    // reach ssh as an option
    let port = i["port"]
        .as_u64()
        .filter(|p| (1..=65535).contains(p))
        .context("its port is not a port")?;
    let plain = |s: &str, extra: &[u8]| {
        !s.is_empty()
            && !s.starts_with('-')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
    };
    if !plain(user, b"-_") || !plain(host, b".:") {
        bail!("the box gave an address ssh shouldn't be handed: {user}@{host}");
    }
    let mut c = std::process::Command::new("ssh");
    c.arg("-i")
        .arg(&key)
        .args(["-o", "IdentitiesOnly=yes", "-p", &port.to_string(), "--"])
        .arg(format!("{user}@{host}"));
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("computer-keys-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// a key made here is kept and found again, 0600 in a 0700 directory
    #[test]
    fn a_device_key_is_made_once() {
        let d = tmp("once");
        let a = device_key_in(&d, "dev").unwrap();
        assert_eq!(device_key_in(&d, "dev").unwrap(), a);
        let m = std::fs::metadata(key_path_in(&d, "dev").unwrap()).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(&d).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// a key already there that this flow did not make, or one anyone else
    /// could read, is refused; a legacy one elsewhere is never looked at
    #[test]
    fn a_foreign_or_legacy_key_is_not_adopted() {
        let d = tmp("foreign");
        std::fs::create_dir_all(&d).unwrap();
        let mut seed = [7u8; 32];
        seed[0] = 1;
        let mut other =
            ssh_key::PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&seed));
        other.set_comment("commonty-dev");
        let pem = other.to_openssh(ssh_key::LineEnding::LF).unwrap();
        let p = key_path_in(&d, "dev").unwrap();
        std::fs::write(&p, pem.as_bytes()).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        // no marker: refused
        assert!(device_key_in(&d, "dev").is_err());
        // the marker, but readable by others: refused
        other.set_comment(marker("dev"));
        std::fs::write(
            &p,
            other
                .to_openssh(ssh_key::LineEnding::LF)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(device_key_in(&d, "dev").is_err());
        // a legacy key beside, as ~/.ssh/commonty-<name> was: a fresh key is made
        let e = tmp("legacy");
        std::fs::create_dir_all(e.join(".ssh")).unwrap();
        std::fs::write(e.join(".ssh/commonty-dev"), pem.as_bytes()).unwrap();
        let fresh = device_key_in(&e.join("keys"), "dev").unwrap();
        assert_ne!(fresh, other.public_key().to_openssh().unwrap());
        assert!(fresh.ends_with(&marker("dev")));
        std::fs::remove_dir_all(&d).unwrap();
        std::fs::remove_dir_all(&e).unwrap();
    }
}
