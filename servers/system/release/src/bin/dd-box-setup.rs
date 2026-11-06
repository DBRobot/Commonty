//! The box installer's screen (nix/installer/setup.nix,
//! docs/dev/joining.md): its pages on loopback, for the kiosk browser
//! that fills the screen, and the steps they ask for - the firmware
//! checked and set, the network, the disk, its backup, the install. The
//! steps are the installer's own programs (dd-join-install, dd-box-backup);
//! this runs them and says how they are doing.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Parser;
use release::setup;
use serde_json::{Value, json};

#[derive(Parser)]
struct Args {
    /// where it answers: loopback only
    #[arg(long, env = "SETUP_LISTEN", default_value = "127.0.0.1:8090")]
    listen: String,
    /// its pages (index.html, setup.css, setup.js)
    #[arg(long, env = "SETUP_WEB", required_unless_present_any = ["ticket_id", "narrow_ticket"])]
    web: Option<PathBuf>,
    /// the shared stylesheet and fonts (servers/web-shared)
    #[arg(long, env = "SETUP_SHARED", required_unless_present_any = ["ticket_id", "narrow_ticket"])]
    shared: Option<PathBuf>,
    /// the drive's commonty/ folder: ticket, backup.key, wifi.json, version
    #[arg(long, env = "SETUP_DRIVE", default_value = "/boot/commonty")]
    drive: PathBuf,
    /// the firmware's settings, as the kernel offers them
    #[arg(
        long,
        env = "SETUP_FIRMWARE",
        default_value = "/sys/class/firmware-attributes"
    )]
    firmware: PathBuf,
    /// the screen's secret: a file made at boot that only root and the
    /// kiosk's user read; every request to /api/ carries it
    #[arg(
        long,
        env = "SETUP_TOKEN_FILE",
        default_value = "/run/dd-setup-token/token"
    )]
    token_file: PathBuf,
    /// print the id a join ticket's backup goes under, and exit
    #[arg(long)]
    ticket_id: Option<PathBuf>,
    /// print the join ticket in this file narrowed to a network key sealed
    /// to --recipient, good for five minutes, and exit
    #[arg(long, requires = "recipient")]
    narrow_ticket: Option<PathBuf>,
    #[arg(long)]
    recipient: Option<String>,
}

/// A step running: what it is, how far, the lines it said
#[derive(Default)]
struct Job {
    kind: String,
    state: String,
    done: f64,
    eta: Option<u64>,
    // its "== " lines: the install's stages
    stages: Vec<String>,
    // the rest it said, for when it stops: the last of it
    said: Vec<String>,
    started: Option<Instant>,
    // an install's build, and when its stage began
    build: setup::Build,
    building_since: Option<Instant>,
}

impl Job {
    fn install_progress(&mut self) {
        let share = self.build.share();
        // nix finds more to do as it goes: the bar never goes back
        self.done = self
            .done
            .max(setup::install_share(self.stages.len(), share));
        self.eta = self
            .building_since
            .filter(|_| self.stages.len() == 3)
            .and_then(|s| setup::eta(s.elapsed().as_secs(), share));
    }
}

struct App {
    args: Args,
    web: PathBuf,
    shared: PathBuf,
    token: String,
    job: Mutex<Job>,
    // the Wi-Fi the box is given: the drive's, or one typed here
    wifi: Mutex<Option<PathBuf>>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if let (Some(t), Some(r)) = (&args.narrow_ticket, &args.recipient) {
        let t = std::fs::read_to_string(t).context("the ticket")?;
        let until = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            + 300;
        let here = format!("installer-network:{}", r.trim());
        match setup::narrow_ticket(&t, &here, until) {
            Some(n) => println!("{n}"),
            None => bail!("not a ticket that can be narrowed"),
        }
        return Ok(());
    }
    if let Some(t) = &args.ticket_id {
        let t = std::fs::read_to_string(t).context("the ticket")?;
        match setup::ticket_id(&t) {
            Some(id) => println!("{id}"),
            None => bail!("not a join ticket"),
        }
        return Ok(());
    }
    let token = std::fs::read_to_string(&args.token_file)
        .context("the screen's secret")?
        .trim()
        .to_string();
    if token.len() < 32 {
        bail!("the screen's secret is too short");
    }
    if !args.listen.starts_with("127.0.0.1:") {
        bail!("it answers on loopback only");
    }
    let listener = TcpListener::bind(&args.listen).context("listening")?;
    let wifi = args.drive.join("wifi.json");
    let web = args.web.clone().context("--web")?;
    let shared = args.shared.clone().context("--shared")?;
    let app = Arc::new(App {
        web,
        shared,
        token,
        wifi: Mutex::new(wifi.exists().then_some(wifi)),
        args,
        job: Mutex::new(Job::default()),
    });
    for conn in listener.incoming().flatten() {
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = serve(&app, conn);
        });
    }
    Ok(())
}

fn serve(app: &Arc<App>, mut s: TcpStream) -> Result<()> {
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("/").to_string(),
    );
    let mut len = 0usize;
    let (mut host, mut origin, mut token) = (None, None, None);
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let v = v.trim().to_string();
            match k.to_ascii_lowercase().as_str() {
                "content-length" => len = v.parse().unwrap_or(0).min(1 << 16),
                "host" => host = Some(v),
                "origin" => origin = Some(v),
                "x-setup-token" => token = Some(v),
                _ => {}
            }
        }
    }
    if let Err(why) = allowed(
        app,
        &path,
        host.as_deref(),
        origin.as_deref(),
        token.as_deref(),
    ) {
        let out = json!({ "error": why }).to_string();
        write!(
            s,
            "HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
            out.len()
        )?;
        return Ok(());
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let (code, kind, out) = match route(app, &method, &path, &body) {
        Ok(Reply::Json(v)) => (200, "application/json", v.to_string().into_bytes()),
        Ok(Reply::File(b, t)) => (200, t, b),
        Ok(Reply::Missing) => (404, "text/plain", b"not here".to_vec()),
        Err(e) => (
            400,
            "application/json",
            json!({ "error": format!("{e:#}") })
                .to_string()
                .into_bytes(),
        ),
    };
    write!(
        s,
        "HTTP/1.1 {code} OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n",
        out.len()
    )?;
    s.write_all(&out)?;
    Ok(())
}

/// Only its own page drives it: addressed as itself (no other name that
/// resolves here, which a page elsewhere could use to reach it), from its
/// own origin when a browser says where from, and, for anything under
/// /api/, with the screen's secret, which only the page the kiosk opened
/// was given (in its address's fragment, never sent to anyone).
fn allowed(
    app: &App,
    path: &str,
    host: Option<&str>,
    origin: Option<&str>,
    token: Option<&str>,
) -> Result<(), &'static str> {
    let me = &app.args.listen;
    if host != Some(me.as_str()) {
        return Err("not addressed as this screen");
    }
    if origin.is_some_and(|o| o != format!("http://{me}")) {
        return Err("not from this screen's page");
    }
    if path.starts_with("/api/") {
        let given = token.unwrap_or("").as_bytes();
        let want = app.token.as_bytes();
        // the same time however much of it matches
        let same = given.len() == want.len()
            && given.iter().zip(want).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0;
        if !same {
            return Err("not this screen's page");
        }
    }
    Ok(())
}

enum Reply {
    Json(Value),
    File(Vec<u8>, &'static str),
    Missing,
}

fn file(dir: &Path, name: &str) -> Reply {
    if name.contains("..") || name.contains('/') {
        return Reply::Missing;
    }
    let t = match name.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("woff2") => "font/woff2",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    for d in [dir.to_path_buf(), dir.join("fonts")] {
        if let Ok(b) = std::fs::read(d.join(name)) {
            return Reply::File(b, t);
        }
    }
    Reply::Missing
}

fn sh(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| cmd.to_string())?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn route(app: &Arc<App>, method: &str, path: &str, body: &Value) -> Result<Reply> {
    let a = &app.args;
    Ok(match (method, path) {
        ("GET", "/") => file(&app.web, "index.html"),
        ("GET", p) if p.starts_with("/_dd/static/") => file(&app.shared, &p[12..]),
        ("GET", p) if p.starts_with("/s/") => file(&app.web, &p[3..]),
        ("GET", "/api/checks") => {
            let c = setup::checks(Path::new("/sys"), Path::new("/dev"));
            let p = setup::plan(
                &a.firmware,
                &setup::table(),
                &["tpm", "power", "virt", "setupmode"],
            );
            let product = |f: &str| {
                std::fs::read_to_string(format!("/sys/class/dmi/id/{f}"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default()
            };
            Reply::Json(json!({
                "checks": c,
                "missing": missing(),
                "firmware": p,
                "maker": product("sys_vendor"),
                "model": product("product_name"),
                "drive": {
                    "ticket": a.drive.join("ticket").exists(),
                    "backupKey": a.drive.join("backup.key").exists(),
                    "expired": drive_expired(a),
                    "version": std::fs::read_to_string(a.drive.join("version")).unwrap_or_default().trim(),
                },
            }))
        }
        ("POST", "/api/firmware") => {
            let p = setup::plan(
                &a.firmware,
                &setup::table(),
                &["tpm", "power", "virt", "setupmode"],
            );
            for c in &p.change {
                std::fs::write(&c.path, &c.value)
                    .with_context(|| format!("setting {} to {}", c.attribute, c.value))?;
            }
            let restart = p
                .driver
                .as_deref()
                .is_some_and(|d| setup::pending_reboot(&a.firmware, d))
                || !p.change.is_empty();
            Reply::Json(json!({ "changed": p.change, "restart": restart }))
        }
        ("POST", "/api/restart") => {
            sh("systemctl", &["reboot"])?;
            Reply::Json(json!({}))
        }
        ("POST", "/api/poweroff") => {
            sh("systemctl", &["poweroff"])?;
            Reply::Json(json!({}))
        }
        ("GET", "/api/network") => {
            let up =
                sh("nmcli", &["-t", "-f", "TYPE,STATE,CONNECTION", "device"]).unwrap_or_default();
            let wired = up.lines().any(|l| l.starts_with("ethernet:connected"));
            // the network's name, not its profile's (dd-setup)
            let wifi = up
                .lines()
                .any(|l| l.starts_with("wifi:connected"))
                .then(|| {
                    sh(
                        "nmcli",
                        &[
                            "-t",
                            "-f",
                            "ACTIVE,SSID",
                            "device",
                            "wifi",
                            "list",
                            "--rescan",
                            "no",
                        ],
                    )
                    .unwrap_or_default()
                    .lines()
                    .find_map(|l| l.strip_prefix("yes:").map(|s| s.replace("\\:", ":")))
                })
                .flatten();
            let drive = std::fs::read(a.drive.join("wifi.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .and_then(|v| v["ssid"].as_str().map(str::to_string));
            Reply::Json(json!({ "wired": wired, "wifi": wifi, "drive": drive }))
        }
        ("GET", "/api/wifi") => {
            let _ = sh("nmcli", &["device", "wifi", "rescan"]);
            let list = sh(
                "nmcli",
                &["-t", "-f", "SSID,SIGNAL,SECURITY", "device", "wifi", "list"],
            )
            .unwrap_or_default();
            let nets: Vec<Value> = list
                .lines()
                .filter_map(|l| {
                    let mut f = l.rsplitn(3, ':');
                    let (sec, sig, ssid) = (f.next()?, f.next()?, f.next()?);
                    (!ssid.is_empty()).then(|| json!({ "ssid": ssid.replace("\\:", ":"), "signal": sig.parse::<u32>().unwrap_or(0), "open": sec.is_empty() }))
                })
                .collect();
            Reply::Json(json!({ "networks": nets }))
        }
        ("POST", "/api/wifi") => {
            // the drive's, or one typed here: joined to now, and kept for
            // the box, which the install seals to its TPM
            let (ssid, psk) = if body["drive"].as_bool() == Some(true) {
                let v: Value = serde_json::from_slice(&std::fs::read(a.drive.join("wifi.json"))?)?;
                (
                    v["ssid"].as_str().unwrap_or("").to_string(),
                    v["psk"].as_str().unwrap_or("").to_string(),
                )
            } else {
                (
                    body["ssid"].as_str().unwrap_or("").to_string(),
                    body["psk"].as_str().unwrap_or("").to_string(),
                )
            };
            if ssid.is_empty() {
                bail!("no network named");
            }
            // a profile with its password, in NetworkManager's keyfiles
            // (root's alone) only while it is brought up, with
            // no agent: a wrong password is said at once, where an agent was
            // asked for it again until the wait ran out. Never in argv
            let _ = sh("nmcli", &["connection", "delete", "dd-setup"]);
            let profile =
                PathBuf::from("/etc/NetworkManager/system-connections/dd-setup.nmconnection");
            write_private(
                &profile,
                wifi_profile(&ssid, &psk, wifi_key_mgmt(&ssid))?.as_bytes(),
            )?;
            if let Err(e) = sh(
                "nmcli",
                &["connection", "load", &profile.display().to_string()],
            ) {
                let _ = std::fs::remove_file(&profile);
                return Err(e);
            }
            let mut c = Command::new("nmcli");
            c.args(["--wait", "60", "connection", "up", "dd-setup"]);
            let out = c.output();
            // NetworkManager holds it now; the drive keeps no second copy
            let _ = std::fs::remove_file(&profile);
            let out = out?;
            if !out.status.success() {
                let _ = sh("nmcli", &["connection", "delete", "dd-setup"]);
                let why = String::from_utf8_lossy(&out.stderr);
                if why.contains("Secrets were required") {
                    bail!("could not join {ssid}: the password is not right");
                }
                bail!(
                    "could not join {ssid}: {}",
                    why.lines()
                        .last()
                        .unwrap_or("")
                        .trim()
                        .trim_start_matches("Error: ")
                );
            }
            let keep = PathBuf::from("/run/dd-setup/wifi.json");
            write_private(
                &keep,
                json!({ "ssid": ssid, "psk": psk }).to_string().as_bytes(),
            )?;
            *app.wifi.lock().unwrap() = Some(keep);
            Reply::Json(json!({ "joined": ssid }))
        }
        ("GET", "/api/disks") => Reply::Json(disks()?),
        ("POST", "/api/backup") => {
            let disk = body["disk"].as_str().context("which disk")?.to_string();
            check_disk(&disk)?;
            start(
                app,
                "backup",
                "dd-box-backup",
                vec![
                    ("DISK", disk),
                    drive_env(a, "TICKET", "ticket"),
                    drive_env(a, "BACKUP_KEY", "backup.key"),
                ],
            )?;
            Reply::Json(json!({}))
        }
        ("POST", "/api/install") => {
            let disk = body["disk"].as_str().context("which disk")?.to_string();
            check_disk(&disk)?;
            let lacks = missing();
            if !lacks.is_empty() {
                bail!("this machine needs {}", lacks.join(", "));
            }
            if drive_expired(a) {
                bail!("this drive's pass has run out: make a new drive in the app");
            }
            let mut env = vec![("DISK", disk), drive_env(a, "TICKET", "ticket")];
            if let Some(w) = app.wifi.lock().unwrap().clone() {
                env.push(("WIFI", w.display().to_string()));
            }
            env.push(("BUILD_LOG_FORMAT", "internal-json".to_string()));
            start(app, "install", "dd-join-install", env)?;
            Reply::Json(json!({}))
        }
        ("GET", "/api/job") => {
            let j = app.job.lock().unwrap();
            Reply::Json(json!({
                "kind": j.kind,
                "state": j.state,
                "done": j.done,
                "eta": j.eta,
                "stages": j.stages,
                "build": j.build,
                "said": j.said.iter().rev().take(6).rev().collect::<Vec<_>>(),
                "seconds": j.started.map(|s| s.elapsed().as_secs()),
            }))
        }
        _ => Reply::Missing,
    })
}

/// What this machine lacks of the hardware minimum
fn missing() -> Vec<String> {
    let threads = std::thread::available_parallelism().map_or(0, |n| n.get() as u64);
    let total = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| setup::mem_total(&m))
        .unwrap_or(0);
    // as the join judges it: what is installed, where that is more
    let raw: Vec<Vec<u8>> = std::fs::read_dir("/sys/firmware/dmi/entries")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("17-"))
        .filter_map(|e| std::fs::read(e.path().join("raw")).ok())
        .collect();
    let installed = setup::mem_installed(raw.iter().map(Vec::as_slice));
    setup::missing(threads, total.max(installed))
}

/// The drive's ticket has run out: a box joining with it could never be its
/// member's (the judge refuses to enlist it), so no disk is wiped for it
fn drive_expired(a: &Args) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    std::fs::read_to_string(a.drive.join("ticket"))
        .ok()
        .and_then(|t| setup::ticket_expires(&t))
        .is_some_and(|until| until <= now)
}

fn drive_env(a: &Args, k: &'static str, f: &str) -> (&'static str, String) {
    (k, a.drive.join(f).display().to_string())
}

// NetworkManager's keyfile for the one network: the name as its bytes, so
// nothing in it is read as the file's syntax; the password printable, as
// WPA's are, its backslashes and a leading space escaped as the file has them
fn wifi_profile(ssid: &str, psk: &str, mgmt: &str) -> Result<String> {
    if !psk.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        bail!("a Wi-Fi password is letters, digits, spaces and punctuation");
    }
    let uuid = std::fs::read_to_string("/proc/sys/kernel/random/uuid")?;
    let name: String = ssid.bytes().map(|b| format!("{b};")).collect();
    let mut f = format!(
        "[connection]\nid=dd-setup\nuuid={}\ntype=wifi\nautoconnect=false\nauth-retries=1\n\n\
         [wifi]\nmode=infrastructure\nssid={name}\n\n",
        uuid.trim()
    );
    if !psk.is_empty() {
        let mut p = psk.replace('\\', "\\\\");
        if p.starts_with(' ') {
            p.replace_range(..1, "\\s");
        }
        f.push_str(&format!("[wifi-security]\nkey-mgmt={mgmt}\npsk={p}\n\n"));
    }
    f.push_str("[ipv4]\nmethod=auto\n\n[ipv6]\nmethod=auto\n");
    Ok(f)
}

// WPA3-only networks take sae; WPA2 and mixed ones wpa-psk
fn wifi_key_mgmt(ssid: &str) -> &'static str {
    let list = sh(
        "nmcli",
        &["-t", "-f", "SSID,SECURITY", "device", "wifi", "list"],
    )
    .unwrap_or_default();
    let sec = list.lines().find_map(|l| {
        let (s, sec) = l.rsplit_once(':')?;
        (s.replace("\\:", ":") == ssid).then(|| sec.to_string())
    });
    match sec {
        Some(s) if s.contains("WPA3") && !s.contains("WPA2") && !s.contains("WPA1") => "sae",
        _ => "wpa-psk",
    }
}

fn write_private(p: &Path, b: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(p)?;
    f.write_all(b)?;
    Ok(())
}

/// The disks it could install to: internal, not removable, not on USB,
/// nothing of them mounted (the same as dd-join-install's own choice)
fn disks() -> Result<Value> {
    let v: Value = serde_json::from_str(&sh(
        "lsblk",
        &[
            "-J",
            "-b",
            "-p",
            "-o",
            "NAME,TYPE,RM,TRAN,SIZE,MODEL,MOUNTPOINTS,FSTYPE,FSUSED",
        ],
    )?)?;
    let mounted = |d: &Value| -> bool {
        let here = d["mountpoints"]
            .as_array()
            .is_some_and(|m| m.iter().any(|x| x.is_string()));
        here || d["children"].as_array().is_some_and(|c| {
            c.iter().any(|p| {
                p["mountpoints"]
                    .as_array()
                    .is_some_and(|m| m.iter().any(|x| x.is_string()))
            })
        })
    };
    let list: Vec<Value> = v["blockdevices"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| {
            d["type"] == "disk"
                && d["rm"] != true
                && d["tran"].as_str() != Some("usb")
                && !mounted(d)
        })
        .map(|d| {
            let used: u64 = d["children"]
                .as_array()
                .map(|c| c.iter().filter_map(|p| p["fsused"].as_str().and_then(|s| s.parse::<u64>().ok()).or(p["fsused"].as_u64())).sum())
                .unwrap_or(0);
            let parts: Vec<Value> = d["children"]
                .as_array()
                .map(|c| c.iter().map(|p| json!({ "name": p["name"], "fs": p["fstype"] })).collect())
                .unwrap_or_default();
            let small = d["size"].as_u64().is_some_and(|s| s < setup::MIN_SYSTEM_DISK);
            json!({ "name": d["name"], "size": d["size"], "small": small, "model": d["model"], "used": used, "parts": parts })
        })
        .collect();
    Ok(json!({ "disks": list }))
}

fn check_disk(disk: &str) -> Result<()> {
    let list = disks()?;
    let ok = list["disks"]
        .as_array()
        .is_some_and(|l| l.iter().any(|d| d["name"] == disk && d["small"] != true));
    if !ok {
        bail!("{disk} is not a disk this machine can be installed to");
    }
    Ok(())
}

/// Runs a step's program, one at a time, keeping what it says: restic's
/// json for a backup's numbers, "== " lines for an install's stages
fn start(app: &Arc<App>, kind: &str, prog: &str, env: Vec<(&'static str, String)>) -> Result<()> {
    {
        let mut j = app.job.lock().unwrap();
        if j.state == "running" {
            bail!("{} is still running", j.kind);
        }
        *j = Job {
            kind: kind.into(),
            state: "running".into(),
            started: Some(Instant::now()),
            ..Job::default()
        };
    }
    let mut c = Command::new(prog);
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| prog.to_string())?;
    let out = child.stdout.take().context("stdout")?;
    let err = child.stderr.take().context("stderr")?;
    let (app, mine) = (app.clone(), app.clone());
    let errs = std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            let mut j = mine.job.lock().unwrap();
            if let Some(said) = j.build.line(&l) {
                if j.said.len() >= 200 {
                    j.said.remove(0);
                }
                j.said.push(said);
            } else if j.kind == "install" {
                j.install_progress();
            }
        }
    });
    let prog = prog.to_string();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            let mut j = app.job.lock().unwrap();
            if let Some((d, eta)) = setup::restic_progress(&l) {
                j.done = d;
                j.eta = eta;
            } else if let Some(s) = l.strip_prefix("== ") {
                j.stages.push(s.to_string());
                if j.stages.len() == 3 {
                    j.building_since = Some(Instant::now());
                }
                j.install_progress();
            }
        }
        let _ = errs.join();
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        // the disk free again before the page can offer it again, however
        // the run ended
        if !ok && app.job.lock().unwrap().kind == "install" {
            let _ = Command::new(&prog).arg("--tidy").status();
        }
        let mut j = app.job.lock().unwrap();
        j.state = if ok { "done" } else { "failed" }.into();
        if ok {
            j.done = 1.0;
            j.eta = Some(0);
        } else {
            j.eta = None;
        }
    });
    Ok(())
}
