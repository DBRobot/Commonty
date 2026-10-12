//! dd-door, on the home controller. Every setting is a flag or DOOR_*:
//!
//!   DOOR_STATE_DIR      /var/lib/commonty-door   state.json, controller.key, registration.json
//!   DOOR_PAGES_DIR      (required)               the pages, laid out as the tree (servers/...)
//!   DOOR_LISTEN         0.0.0.0:443              the door; the firewall keeps it to commonty0
//!   DOOR_PAIR_LISTEN    0.0.0.0:8080             pairing, until /pair/finish
//!   DOOR_PAIR_KEY       /boot/commonty/pair      the installer's key; removed when paired
//!   DOOR_TLS_CERT       <state>/tls/cert.pem     PEM, read again when it changes
//!   DOOR_TLS_KEY        <state>/tls/key.pem
//!   DOOR_PLAIN_HTTP     false                    no TLS (tests)
//!   DOOR_NAME           from pairing             the door's name, for a test
//!   DOOR_HA             127.0.0.1:8123           Home Assistant
//!   DOOR_USERS_FILE     /var/lib/hass/commonty-users.yaml
//!   DOOR_RESTART        systemctl restart home-assistant
//!   DOOR_NETWORK_KEY    /var/lib/commonty-door/network-key   for the network's unit
//!   DOOR_HA_GROUP       system-admin             the Home Assistant group a person is made in
//!   DOOR_CT_EVERY       3600                     seconds between Certificate Transparency looks
//!   DOOR_AGENT_STATE    /var/lib/dd-agent        the release it runs; its hold on updates
//!   DOOR_MEMBERS        (required)               the release's member list (fleet/members.json)
//!   DOOR_GATE           https://files.<domain>   the gate: entries, and where backups go
//!   DOOR_HA_BACKUPS     /var/lib/hass/backups    Home Assistant's backups directory
//!   DOOR_BACKUP_FIRST   3600                     seconds to the first nightly backup
//!   DOOR_BACKUP_RETRY   300                      seconds before a failed backup is tried again
//!
//! Two more ways to run it, for the units around it (nix/modules/smarthome/door.nix):
//!
//!   dd-door register                   once joined: the name at the DNS Worker, and
//!                                      the controller's address there; prints the name
//!   dd-door acme-hook <op> <fqdn> <v>  lego's exec provider: one challenge, signed by
//!                                      the controller's own key
//!
//!   DOOR_DNS_WORKER     https://dns.<domain>
//!   DOOR_ADDRESS_FILE   /run/commonty-net-address   the controller's address on the network

use anyhow::{Context, Result};
use clap::Parser;
use smarthome_door::{App, Config, Parts, ct, gate, ha, pages, pair, tls};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "dd-door",
    about = "the door in front of Home Assistant on a home controller"
)]
struct Args {
    #[arg(long, env = "DOOR_STATE_DIR", default_value = "/var/lib/commonty-door")]
    state_dir: PathBuf,
    #[arg(long, env = "DOOR_PAGES_DIR")]
    pages_dir: PathBuf,
    #[arg(long, env = "DOOR_LISTEN", default_value = "0.0.0.0:443")]
    listen: SocketAddr,
    #[arg(long, env = "DOOR_PAIR_LISTEN", default_value = "0.0.0.0:8080")]
    pair_listen: SocketAddr,
    #[arg(long, env = "DOOR_PAIR_KEY", default_value = "/boot/commonty/pair")]
    pair_key: PathBuf,
    #[arg(long, env = "DOOR_TLS_CERT")]
    tls_cert: Option<PathBuf>,
    #[arg(long, env = "DOOR_TLS_KEY")]
    tls_key: Option<PathBuf>,
    #[arg(long, env = "DOOR_PLAIN_HTTP")]
    plain_http: bool,
    #[arg(long, env = "DOOR_NAME")]
    name: Option<String>,
    #[arg(long, env = "DOOR_HA", default_value = "127.0.0.1:8123")]
    ha: SocketAddr,
    #[arg(
        long,
        env = "DOOR_USERS_FILE",
        default_value = "/var/lib/hass/commonty-users.yaml"
    )]
    users_file: PathBuf,
    #[arg(
        long,
        env = "DOOR_RESTART",
        default_value = "systemctl restart home-assistant"
    )]
    restart: String,
    #[arg(
        long,
        env = "DOOR_NETWORK_KEY",
        default_value = "/var/lib/commonty-door/network-key"
    )]
    network_key: PathBuf,
    #[arg(long, env = "DOOR_HA_GROUP", default_value = "system-admin")]
    ha_group: String,
    #[arg(long, env = "DOOR_CT_EVERY", default_value_t = 3600)]
    ct_every: u64,
    #[arg(long, env = "DOOR_BACKUP_FIRST", default_value_t = 3600)]
    backup_first: u64,
    #[arg(long, env = "DOOR_BACKUP_RETRY", default_value_t = 300)]
    backup_retry: u64,
    #[arg(long, env = "DOOR_AGENT_STATE", default_value = "/var/lib/dd-agent")]
    agent_state: PathBuf,
    /// the release's member list (fleet/members.json)
    #[arg(long, env = "DOOR_MEMBERS")]
    members: PathBuf,
    #[arg(long, env = "DOOR_GATE")]
    gate: Option<String>,
    #[arg(long, env = "DOOR_HA_BACKUPS", default_value = "/var/lib/hass/backups")]
    ha_backups: PathBuf,
}

fn env_path(k: &str, default: &str) -> PathBuf {
    PathBuf::from(std::env::var(k).unwrap_or_else(|_| default.to_string()))
}

fn registration(state: &std::path::Path) -> Result<pair::Registration> {
    let f = state.join("registration.json");
    serde_json::from_slice(
        &std::fs::read(&f).with_context(|| format!("{} (not paired yet)", f.display()))?,
    )
    .context("registration.json")
}

async fn register() -> Result<()> {
    let state = env_path("DOOR_STATE_DIR", "/var/lib/commonty-door");
    let dns = std::env::var("DOOR_DNS_WORKER").context("DOOR_DNS_WORKER")?;
    let reg = registration(&state)?;
    let key = smarthome_door::state::controller_key(&state)?;
    let address =
        std::fs::read_to_string(env_path("DOOR_ADDRESS_FILE", "/run/commonty-net-address"))
            .context("the controller's address on the network (not joined yet)")?;
    let name = smarthome_door::name::register(&dns, &reg, &key, address.trim()).await?;
    println!("{name}");
    Ok(())
}

async fn acme_hook(args: Vec<String>) -> Result<()> {
    let [op, fqdn, value] = args.as_slice() else {
        anyhow::bail!("acme-hook <present|cleanup> <fqdn> <value>");
    };
    let state = env_path("DOOR_STATE_DIR", "/var/lib/commonty-door");
    let dns = std::env::var("DOOR_DNS_WORKER").context("DOOR_DNS_WORKER")?;
    let reg = registration(&state)?;
    let key = smarthome_door::state::controller_key(&state)?;
    smarthome_door::name::challenge(&dns, &reg, &key, op, fqdn, value).await
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut argv = std::env::args().skip(1);
    match argv.next().as_deref() {
        Some("register") => return register().await,
        Some("acme-hook") => return acme_hook(argv.collect()).await,
        _ => {}
    }
    let a = Args::parse();
    let tls = (!a.plain_http).then(|| {
        (
            a.tls_cert
                .unwrap_or_else(|| a.state_dir.join("tls/cert.pem")),
            a.tls_key.unwrap_or_else(|| a.state_dir.join("tls/key.pem")),
        )
    });
    let cfg = Config {
        state_dir: a.state_dir.clone(),
        pages_dir: a.pages_dir.clone(),
        listen: a.listen,
        pair_listen: a.pair_listen,
        pair_key: a.pair_key,
        tls,
        name: a.name,
        ha: a.ha,
        users_file: a.users_file,
        restart: a.restart.split_whitespace().map(str::to_string).collect(),
        network_key_file: a.network_key.clone(),
        ha_group: a.ha_group,
        ct_every: Duration::from_secs(a.ct_every.max(60)),
        agent_state: a.agent_state,
        members: Some(smarthome_door::members::Members::parse(
            &std::fs::read_to_string(&a.members).context("the member list")?,
        )?),
        gate: a.gate.clone(),
        ha_backups: a.ha_backups,
        backup_first: Duration::from_secs(a.backup_first),
        backup_every: Duration::from_secs(24 * 3600),
        backup_retry: Duration::from_secs(a.backup_retry.max(5)),
    };
    let assets = pages::Assets::load(&cfg.pages_dir).context("the pages")?;
    let http = reqwest::Client::builder()
        .user_agent("commonty-door")
        .build()?;
    let gate = Arc::new(gate::Gate {
        http: http.clone(),
        base: a.gate,
        registration: a.state_dir.join("registration.json"),
        key: smarthome_door::state::controller_key(&a.state_dir)?,
    });
    let parts = Parts {
        ha: Arc::new(ha::HomeAssistant::new(cfg.ha, cfg.restart.clone())),
        ct: Arc::new(ct::CrtSh(http.clone())),
        hooks: Arc::new(pair::FileHooks {
            network_key_file: a.network_key,
            registration_file: a.state_dir.join("registration.json"),
        }),
        directory: gate.clone(),
        upload: gate,
    };
    let app = App::new(cfg, assets, parts)?;
    eprintln!("door: controller {}", app.controller());

    let finished = app.with_store(|s| s.state.finished);
    // without a key on the card it does not pair, but its page says how to add one
    if !finished {
        let l = tokio::net::TcpListener::bind(app.cfg.pair_listen).await?;
        eprintln!("door: pairing on {}", app.cfg.pair_listen);
        let a2 = app.clone();
        tokio::spawn(async move {
            let done = a2.clone();
            let r = axum::serve(l, smarthome_door::pair_router(a2))
                .with_graceful_shutdown(async move { done.pairing_done.notified().await })
                .await;
            if let Err(e) = r {
                eprintln!("door: pairing: {e}");
            }
            eprintln!("door: pairing over");
        });
    }
    tokio::spawn(smarthome_door::reconcile_loop(app.clone()));
    tokio::spawn(ct::watch(app.clone()));
    tokio::spawn(smarthome_door::backup::schedule(app.clone()));

    let tcp = tokio::net::TcpListener::bind(app.cfg.listen).await?;
    let router = smarthome_door::router(app.clone());
    match app.cfg.tls.clone() {
        Some((cert, key)) => {
            // a door that is not paired has no certificate yet; it pairs
            // first, and opens here once its certificate is in place
            while !(cert.exists() && key.exists()) {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            eprintln!("door: on {} (tls)", app.cfg.listen);
            axum::serve(tls::Listener::new(tcp, tls::config(cert, key)?)?, router).await?;
        }
        None => {
            eprintln!("door: on {} (plain http)", app.cfg.listen);
            axum::serve(tcp, router).await?;
        }
    }
    Ok(())
}
