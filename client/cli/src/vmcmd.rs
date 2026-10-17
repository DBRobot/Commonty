//! `dd vm`: your own computers on the fleet (docs/dev/instances.md), through
//! the same API as their page, compute.<domain>, with this device's token.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::{Value, json};

#[derive(Subcommand)]
pub enum VmCmd {
    /// Your computers and configs, and what they take of your limits
    List,
    /// Save a config: what computers are made from
    Config {
        name: String,
        /// MiB
        #[arg(long, default_value_t = 4096)]
        memory: u64,
        #[arg(long, default_value_t = 2)]
        threads: u32,
        /// GiB
        #[arg(long, default_value_t = 32)]
        disk: u64,
        /// a NixOS module: the computer's packages and services
        #[arg(long)]
        nix: Option<PathBuf>,
        /// an SSH public key file for your account (repeat for more)
        #[arg(long = "key")]
        keys: Vec<PathBuf>,
        /// a port to open besides SSH (repeat for more)
        #[arg(long = "port")]
        ports: Vec<u16>,
        /// a friend who may reach it too (repeat for more)
        #[arg(long = "let-in")]
        players: Vec<String>,
        /// may run on anyone's box, not only yours
        #[arg(long)]
        anywhere: bool,
    },
    /// Delete a config; computers made from it keep running
    Forget { name: String },
    /// A new computer from a saved config
    Make {
        config: String,
        name: String,
        /// yes to running on someone else's box, without being asked
        #[arg(long)]
        yes: bool,
    },
    /// Start it; it stays up across reboots until stopped
    Start { name: String },
    /// Shut it down cleanly; /home and its system are kept
    Stop { name: String },
    /// Its config again: packages and keys now, size and ports at its next start
    Apply {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Its system back to the first disk; /home is kept (stopped only)
    Reset { name: String },
    /// Gone, with both its disks (stopped only)
    Delete { name: String },
    /// What its last apply said
    Log { name: String },
    /// SSH into it
    Ssh {
        name: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

struct Api {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Api {
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<String> {
        let mut r = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r
            .send()
            .await
            .with_context(|| format!("asking {}", self.base))?;
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
        Ok(serde_json::from_str(
            &self.send(reqwest::Method::GET, "/api/state", None).await?,
        )?)
    }

    /// a computer by its name, as its owner calls it
    async fn find(&self, name: &str) -> Result<Value> {
        self.state().await?["instances"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["name"] == name).cloned())
            .with_context(|| format!("you have no computer called {name}"))
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

async fn verb(api: &Api, name: &str, verb: &str, said: &str) -> Result<()> {
    let i = api.find(name).await?;
    let id = i["id"].as_str().unwrap_or("");
    api.send(
        reqwest::Method::POST,
        &format!("/api/instances/{id}/{verb}"),
        Some(json!({})),
    )
    .await?;
    println!("{name}: {said}");
    Ok(())
}

pub async fn run(cmd: VmCmd, keys: &auth::Store, dirs: &[String]) -> Result<()> {
    let (_, _, token) = media::gate::Opener::load(keys)?;
    let door = media::gate::door_base(dirs)?;
    let base = match door.split_once("://home.") {
        Some((scheme, rest)) => format!("{scheme}://compute.{rest}"),
        None => door,
    };
    let api = Api {
        base,
        token,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?,
    };
    use reqwest::Method as M;
    match cmd {
        VmCmd::List => {
            let s = api.state().await?;
            println!(
                "using {} of {} and {} of {} threads",
                gib(s["used"]["memory"].as_u64().unwrap_or(0)),
                gib(s["limits"]["memory"].as_u64().unwrap_or(0)),
                s["used"]["threads"],
                s["limits"]["threads"]
            );
            for i in s["instances"].as_array().into_iter().flatten() {
                let port = i["address"]
                    .as_str()
                    .unwrap_or("")
                    .rsplit(':')
                    .next()
                    .unwrap_or("");
                println!(
                    "{:<20} {:<9} {} · {} threads · {} GiB  ssh -p {port} {}@{}",
                    i["name"].as_str().unwrap_or(""),
                    i["state"].as_str().unwrap_or(""),
                    gib(i["memory"].as_u64().unwrap_or(0)),
                    i["threads"],
                    i["disk"],
                    i["owner"].as_str().unwrap_or(""),
                    i["address"]
                        .as_str()
                        .unwrap_or("")
                        .split(':')
                        .next()
                        .unwrap_or(""),
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
        VmCmd::Config {
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
            let nix = match nix {
                Some(p) => std::fs::read_to_string(&p)
                    .with_context(|| format!("reading {}", p.display()))?,
                None => String::new(),
            };
            let mut ks = Vec::new();
            for p in keys {
                let k = std::fs::read_to_string(&p)
                    .with_context(|| format!("reading {}", p.display()))?;
                ks.extend(
                    k.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string),
                );
            }
            let body = json!({
                "name": name, "memory": memory, "threads": threads, "disk": disk,
                "nix": nix, "keys": ks, "ports": ports, "players": players,
                "consent": if anywhere { "any" } else { "own" },
            });
            api.send(M::PUT, &format!("/api/configs/{name}"), Some(body))
                .await?;
            println!("config {name} saved");
        }
        VmCmd::Forget { name } => {
            api.send(M::DELETE, &format!("/api/configs/{name}"), None)
                .await?;
            println!("config {name} deleted");
        }
        VmCmd::Make { config, name, yes } => {
            let s = api.state().await?;
            let anywhere = s["configs"]
                .as_array()
                .and_then(|a| a.iter().find(|c| c["name"] == config.as_str()))
                .is_some_and(|c| c["consent"] == "any");
            let yes = if anywhere && !yes { agree()? } else { yes };
            if anywhere && !yes {
                bail!("not made");
            }
            let made: Value = serde_json::from_str(
                &api.send(
                    M::POST,
                    "/api/instances",
                    Some(json!({"config": config, "name": name, "yes": yes})),
                )
                .await?,
            )?;
            println!(
                "{name} is starting: dd vm ssh {name}  ({})",
                made["address"].as_str().unwrap_or("")
            );
        }
        VmCmd::Start { name } => verb(&api, &name, "start", "starting").await?,
        VmCmd::Stop { name } => verb(&api, &name, "stop", "stopping").await?,
        VmCmd::Reset { name } => {
            verb(
                &api,
                &name,
                "reset",
                "reset: its system starts fresh, /home is kept",
            )
            .await?
        }
        VmCmd::Apply { name, yes } => {
            let i = api.find(&name).await?;
            let id = i["id"].as_str().unwrap_or("");
            let s = api.state().await?;
            let widens = i["consent"] != "any"
                && s["configs"]
                    .as_array()
                    .and_then(|a| a.iter().find(|c| c["name"] == i["config"]))
                    .is_some_and(|c| c["consent"] == "any");
            let yes = if widens && !yes { agree()? } else { yes };
            api.send(
                M::POST,
                &format!("/api/instances/{id}/apply"),
                Some(json!({"yes": yes})),
            )
            .await?;
            println!("{name}: applied; its packages rebuild now, its size at its next start");
        }
        VmCmd::Delete { name } => {
            let i = api.find(&name).await?;
            api.send(
                M::DELETE,
                &format!("/api/instances/{}", i["id"].as_str().unwrap_or("")),
                None,
            )
            .await?;
            println!("{name} deleted");
        }
        VmCmd::Log { name } => {
            let i = api.find(&name).await?;
            print!(
                "{}",
                api.send(
                    M::GET,
                    &format!("/api/instances/{}/log", i["id"].as_str().unwrap_or("")),
                    None
                )
                .await?
            );
        }
        VmCmd::Ssh { name, args } => {
            let i = api.find(&name).await?;
            let addr = i["address"].as_str().unwrap_or("");
            let (host, port) = addr.rsplit_once(':').context("it has no address")?;
            let user = i["owner"].as_str().unwrap_or("");
            // the box's word, and a box is not trusted: nothing it says may
            // reach ssh as an option
            let port: u16 = port.parse().context("its port is not a port")?;
            let plain = |s: &str, extra: &[u8]| {
                !s.is_empty()
                    && !s.starts_with('-')
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
            };
            if !plain(user, b"-_") || !plain(host, b".:-") {
                bail!("the box gave an address ssh shouldn't be handed: {user}@{host}");
            }
            let status = std::process::Command::new("ssh")
                .args(["-p", &port.to_string(), "--", &format!("{user}@{host}")])
                // what to run there: after the destination, a command, never options
                .args(args)
                .status()
                .context("running ssh")?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }
    Ok(())
}
