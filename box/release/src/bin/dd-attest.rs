//! A box's word on a release, before it is signed: evaluate the commit
//! here, walk the closure CI built for it, and vouch for it only if every
//! path in it is one of:
//!
//!   - signed by cache.nixos.org (checked against that key, not the
//!     signature list a narinfo claims),
//!   - content-addressed, so the path is its own proof,
//!   - built on this box by its own daemon,
//!   - or rebuilt here from this box's own evaluation and found the same,
//!     bit for bit.
//!
//! The answer is signed with the box's ssh host key (namespace
//! commonty-attest-v1), whose public half is already in fleet/boxes.json,
//! and `dd release publish` wants two boxes to agree with CI before it
//! signs anything. A runner, the forge, or the cache can each lie about a
//! path; they cannot make two boxes rebuild the lie.
//!
//! Runs as root (sudo, over the owner's ssh): the host key and --check.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;

const NAMESPACE: &str = "commonty-attest-v1";
const UPSTREAM: &str = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";
const CHECKED: &str = "/var/lib/dd-attest/checked";
const HOST_KEY: &str = "/etc/ssh/ssh_host_ed25519_key";

#[derive(Parser)]
struct Args {
    /// the repository, git+https://host/owner/name; the rev is pinned here
    #[arg(long)]
    flake: String,
    /// the commit, 40 hex
    #[arg(long)]
    rev: String,
    /// name=path: a box of the release and the toplevel CI built for it
    #[arg(long = "box", required = true)]
    boxes: Vec<String>,
}

fn main() -> Result<()> {
    let a = Args::parse();
    ensure!(
        a.rev.len() == 40 && a.rev.bytes().all(|b| b.is_ascii_hexdigit()),
        "rev: 40 hex"
    );
    ensure!(
        a.flake.starts_with("git+https://") && !a.flake.contains(['?', '#', ' ', '\'']),
        "flake: git+https://host/owner/name, nothing after it"
    );
    let attester = fs::read_to_string("/proc/sys/kernel/hostname")?
        .trim()
        .to_string();
    let _lock = lock()?;
    let mut checked = read_checked();

    let mut out = BTreeMap::new();
    for b in &a.boxes {
        let (name, want) = b.split_once('=').context("--box name=path")?;
        ensure!(
            name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "box name"
        );
        let attr = format!(
            "{}?rev={}#nixosConfigurations.{name}.config.system.build.toplevel",
            a.flake, a.rev
        );
        let drv = sh("nix", &["eval", "--raw", &format!("{attr}.drvPath")])?
            .trim()
            .to_string();
        let path = sh("nix", &["eval", "--raw", &format!("{attr}.outPath")])?
            .trim()
            .to_string();
        ensure!(
            path == want,
            "{name}: {} evaluates here to {path}, not {want}",
            &a.rev[..12]
        );
        eprintln!("== {name}: {path}");
        // present, however it gets here: substituted only where a trusted
        // key signed it, otherwise built by this box's daemon
        sh("nix-store", &["--realise", &drv])?;
        let closure = attest(&drv, &path, &mut checked)?;
        out.insert(
            name.to_string(),
            serde_json::json!({ "path": path, "closure": closure }),
        );
    }

    let statement = serde_json::to_string(&serde_json::json!({
        "v": 1,
        "attester": attester,
        "flake": a.flake,
        "rev": a.rev,
        "boxes": out,
    }))?;
    let signature = sign(&statement)?;
    println!(
        "{}",
        serde_json::json!({ "statement": statement, "signature": signature })
    );
    Ok(())
}

/// every path under `path` accounted for; the closure digest if so
fn attest(drv: &str, path: &str, checked: &mut BTreeSet<String>) -> Result<String> {
    let listing = sh("nix", &["path-info", "--json", "--recursive", path])?;
    let info: serde_json::Value = serde_json::from_str(&listing)?;
    let entries: Vec<(String, serde_json::Value)> = match &info {
        serde_json::Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|e| Some((e.get("path")?.as_str()?.to_string(), e.clone())))
            .collect(),
        _ => bail!("path-info is neither object nor array"),
    };
    let paths: Vec<&str> = entries.iter().map(|(p, _)| p.as_str()).collect();

    // signatures checked against cache.nixos.org's key alone; a
    // content-addressed path passes on its own
    let untrusted = untrusted(&paths)?;
    eprintln!(
        "   {} paths, {} not upstream's",
        paths.len(),
        untrusted.len()
    );

    let mut to_check: BTreeSet<String> = BTreeSet::new();
    for (p, e) in &entries {
        if !untrusted.contains(p) {
            continue;
        }
        let nar = e["narHash"].as_str().unwrap_or_default();
        if e["ultimate"].as_bool() == Some(true) || checked.contains(&format!("{p} {nar}")) {
            continue;
        }
        to_check.insert(p.clone());
    }

    if !to_check.is_empty() {
        // the derivation for each, from this box's evaluation: a deriver
        // recorded with the path came from wherever the path came from
        let ours: BTreeSet<String> = sh("nix-store", &["--query", "--requisites", drv])?
            .lines()
            .map(str::to_string)
            .collect();
        let mut drvs = BTreeSet::new();
        for p in &to_check {
            let d = sh("nix-store", &["--query", "--deriver", p])?
                .trim()
                .to_string();
            ensure!(
                ours.contains(&d),
                "{p}: neither upstream's nor built here, and not made by this commit's derivations"
            );
            let outputs = sh("nix-store", &["--query", "--outputs", &d])?;
            ensure!(
                outputs.lines().any(|o| o == p),
                "{p}: its recorded derivation does not make it"
            );
            drvs.insert(d);
        }
        eprintln!("   rebuilding {} to compare", drvs.len());
        let list: Vec<&str> = drvs.iter().map(String::as_str).collect();
        // every output first (--check needs them all), then the rebuild
        sh("nix-store", &[&["--realise"][..], &list].concat())?;
        sh(
            "nix-store",
            &[&["--realise", "--check"][..], &list].concat(),
        )
        .context("a path did not rebuild the same: nothing is vouched for")?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(CHECKED)?;
        for (p, e) in &entries {
            if to_check.contains(p) {
                let line = format!("{p} {}", e["narHash"].as_str().unwrap_or_default());
                writeln!(f, "{line}")?;
                checked.insert(line);
            }
        }
    }
    release::closure_digest(&listing).map_err(|e| anyhow::anyhow!("{e}"))
}

fn untrusted(paths: &[&str]) -> Result<BTreeSet<String>> {
    let mut args = vec![
        "store",
        "verify",
        "--no-contents",
        "--sigs-needed",
        "1",
        "--option",
        "trusted-public-keys",
        UPSTREAM,
    ];
    args.extend(paths);
    let o = Command::new("nix")
        .args(&args)
        .stdin(Stdio::null())
        .output()?;
    let err = String::from_utf8_lossy(&o.stderr);
    let set: BTreeSet<String> = err
        .lines()
        .filter(|l| l.contains("is untrusted"))
        .filter_map(|l| l.split('\'').nth(1).map(str::to_string))
        .collect();
    // a failure that is not about signatures is not a verdict
    ensure!(
        o.status.success() || !set.is_empty(),
        "nix store verify: {}",
        err.lines().last().unwrap_or_default()
    );
    Ok(set)
}

fn read_checked() -> BTreeSet<String> {
    fs::read_to_string(CHECKED)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// one attestation at a time: two would rebuild the same things twice
fn lock() -> Result<fs::File> {
    fs::create_dir_all(Path::new(CHECKED).parent().unwrap())?;
    let f = fs::File::create("/var/lib/dd-attest/lock")?;
    f.lock()?;
    Ok(f)
}

fn sign(statement: &str) -> Result<String> {
    let mut c = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-q", "-f", HOST_KEY, "-n", NAMESPACE])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("ssh-keygen")?;
    c.stdin.take().unwrap().write_all(statement.as_bytes())?;
    let o = c.wait_with_output()?;
    ensure!(o.status.success(), "ssh-keygen -Y sign failed");
    Ok(String::from_utf8(o.stdout)?)
}

fn sh(bin: &str, args: &[&str]) -> Result<String> {
    let o = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| bin.to_string())?;
    if !o.status.success() {
        bail!("{bin} {} failed", args.first().copied().unwrap_or_default());
    }
    Ok(String::from_utf8(o.stdout)?)
}
