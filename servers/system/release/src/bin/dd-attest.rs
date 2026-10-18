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
    #[arg(long, required_unless_present = "order")]
    flake: Option<String>,
    /// the commit, 40 hex
    #[arg(long, required_unless_present = "order")]
    rev: Option<String>,
    /// name=path: a box of the release and the toplevel CI built for it
    #[arg(long = "box", required_unless_present = "order")]
    boxes: Vec<String>,
    /// Instead: the build order at this url (signed by the release key).
    /// Every job that names this box is built, accounted for by the same
    /// rules, and its signed result handed to dd-control for the agreed
    /// log. Run on a timer; a job is done once.
    /// Given more than once, each is read: a release's order and a check's.
    #[arg(long, conflicts_with_all = ["flake", "rev", "boxes"])]
    order: Vec<String>,
    /// the release key's public half, base64
    #[arg(long, default_value = "/etc/dd/release.pub")]
    key_file: String,
    /// where dd-control takes build results
    #[arg(long, default_value = "/run/dd-control/built.sock")]
    submit: String,
}

fn hostname() -> Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/hostname")?
        .trim()
        .to_string())
}

fn check_flake(flake: &str, rev: &str) -> Result<()> {
    ensure!(
        rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()),
        "rev: 40 hex"
    );
    ensure!(
        flake.starts_with("git+https://") && !flake.contains(['?', '#', ' ', '\'', '"']),
        "flake: git+https://host/owner/name, nothing after it"
    );
    Ok(())
}

fn main() -> Result<()> {
    let a = Args::parse();
    if !a.order.is_empty() {
        // each on its own: one that cannot be read does not hold up another.
        // In the order given, a release's first: a later one (the nightly
        // check) gives way to a new order at an earlier url, as it does to
        // its own being replaced
        let mut failed = 0;
        let mut ahead: Vec<(&str, Option<String>)> = Vec::new();
        for url in &a.order {
            if let Err(e) = order(url, &a.key_file, &a.submit, &ahead) {
                eprintln!("{url}: {e:#}");
                failed += 1;
            }
            let id = trusted_key(&a.key_file)
                .ok()
                .and_then(|k| order_id(url, &k));
            ahead.push((url, id));
        }
        ensure!(failed == 0, "{failed} order(s) not done");
        return Ok(());
    }
    let (flake, rev) = (a.flake.unwrap_or_default(), a.rev.unwrap_or_default());
    check_flake(&flake, &rev)?;
    let attester = hostname()?;
    let _lock = lock()?;
    let mut checked = read_checked();

    let mut out = BTreeMap::new();
    for b in &a.boxes {
        let (name, want) = b.split_once('=').context("--box name=path")?;
        ensure!(
            name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "box name"
        );
        let attr =
            format!("{flake}?rev={rev}#nixosConfigurations.{name}.config.system.build.toplevel");
        let drv = sh("nix", &["eval", "--raw", &format!("{attr}.drvPath")])?
            .trim()
            .to_string();
        let path = sh("nix", &["eval", "--raw", &format!("{attr}.outPath")])?
            .trim()
            .to_string();
        ensure!(
            path == want,
            "{name}: {} evaluates here to {path}, not {want}",
            &rev[..12]
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
        "flake": flake,
        "rev": rev,
        "boxes": out,
    }))?;
    let signature = sign(&statement)?;
    println!(
        "{}",
        serde_json::json!({ "statement": statement, "signature": signature })
    );
    Ok(())
}

/// orders older than this are not acted on: a build order is for one release
const ORDER_TTL_SECS: u64 = 7 * 24 * 3600;
/// what this box has finished: "<order id> <job>", one a line
const DONE: &str = "/var/lib/dd-attest/done";

/// a name in an attribute path, a job's name, a part: nothing nix or a
/// shell would read as more than a name
fn plain(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && !s.starts_with('.')
        && !s.ends_with('.')
}

fn version_ok(v: &str) -> bool {
    v.split('.').count() == 3
        && v.split('.')
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// One job of an order, as the order states it.
struct Job {
    name: String,
    attr: String,
    version: Option<String>,
    part: Option<String>,
    path: String,
}

fn jobs_for(order: &serde_json::Value, me: &str) -> Result<Vec<Job>> {
    let mut out = Vec::new();
    for j in order["jobs"].as_array().context("an order has jobs")? {
        let named = j["builders"]
            .as_array()
            .is_some_and(|b| b.iter().any(|x| x.as_str() == Some(me)));
        if !named {
            continue;
        }
        let field = |k: &str| j[k].as_str().map(str::to_string);
        let job = Job {
            name: field("name").context("a job has a name")?,
            attr: field("attr").context("a job has an attr")?,
            version: field("version"),
            part: field("part"),
            path: field("path").context("a job has a path")?,
        };
        ensure!(
            plain(&job.name) && plain(&job.attr),
            "job {}: a plain name and attr",
            job.name
        );
        ensure!(
            job.version.as_deref().is_none_or(version_ok),
            "job {}: version X.Y.Z",
            job.name
        );
        ensure!(
            job.part.as_deref().is_none_or(plain),
            "job {}: a plain part",
            job.name
        );
        ensure!(
            job.path.strip_prefix("/nix/store/").is_some_and(plain),
            "job {}: a store path",
            job.name
        );
        out.push(job);
    }
    Ok(out)
}

/// The installable a job names, and the --apply that reaches its output:
/// `<attr>` itself, or `<attr> "<version>"` and then `.part`
fn eval_job(flake: &str, rev: &str, job: &Job, field: &str) -> Result<String> {
    let at = format!("{flake}?rev={rev}#{}", job.attr);
    let apply = match (&job.version, &job.part) {
        (Some(v), Some(p)) => format!("r: (r \"{v}\").{p}.{field}"),
        (Some(v), None) => format!("r: (r \"{v}\").{field}"),
        (None, Some(p)) => format!("r: r.{p}.{field}"),
        (None, None) => format!("r: r.{field}"),
    };
    Ok(sh("nix", &["eval", "--raw", &at, "--apply", &apply])?
        .trim()
        .to_string())
}

fn trusted_key(key_file: &str) -> Result<ed25519_dalek::VerifyingKey> {
    release::decode_public(fs::read_to_string(key_file)?.trim())
        .map_err(|e| anyhow::anyhow!("{key_file}: {e}"))
}

/// `ahead`: the orders before this one, each with the id it had once done
fn order(url: &str, key_file: &str, submit: &str, ahead: &[(&str, Option<String>)]) -> Result<()> {
    let me = hostname()?;
    let trusted = trusted_key(key_file)?;
    // No order, or none to be had right now (the forge down, or switching
    // under us): nothing to do, and nothing wrong. The next run looks again.
    let raw = match ureq::get(url).call() {
        Ok(mut r) => r.body_mut().read_to_string()?,
        Err(e) => {
            eprintln!("{url}: no order read ({e}); looking again on the next run");
            return Ok(());
        }
    };
    let o = release::verify_doc(&raw, "build-order", &trusted)
        .map_err(|e| anyhow::anyhow!("the build order does not verify: {e}"))?;
    let id = o["id"].as_str().context("an order has an id")?.to_string();
    ensure!(plain(&id), "order id");
    let issued = o["issued"].as_u64().unwrap_or_default();
    if release::now().saturating_sub(issued) > ORDER_TTL_SECS {
        return Ok(());
    }
    let flake = o["flake"].as_str().context("an order names a flake")?;
    let rev = o["rev"].as_str().context("an order names a commit")?;
    check_flake(flake, rev)?;
    let jobs = jobs_for(&o, &me)?;
    let done = fs::read_to_string(DONE).unwrap_or_default();
    let todo: Vec<Job> = jobs
        .into_iter()
        .filter(|j| !done.lines().any(|l| l == format!("{id} {}", j.name)))
        .collect();
    if todo.is_empty() {
        return Ok(());
    }
    // No lock here: a build order is the app's downloads, an hour's build
    // at times, and holding the release's lock through it kept a box
    // release waiting behind it (release 231). One order at a time is the
    // dd-build unit's, which systemd never runs twice; the checked list is
    // only appended to, a line at a time, and only read whole.
    let mut checked = read_checked();
    // a newer order replaces this one: building on for the old one kept the
    // new one waiting an hour, so a job not started is left and one under
    // way is stopped, said nowhere and not marked done
    let gone = || {
        superseded(url, &trusted, &id)
            || ahead
                .iter()
                .any(|(u, was)| order_id(u, &trusted).is_some_and(|now| Some(&now) != was.as_ref()))
    };
    for job in todo {
        if gone() {
            eprintln!(
                "== order {id}: replaced, or a newer one goes first; the next run takes that"
            );
            return Ok(());
        }
        eprintln!("== order {id}: {}", job.name);
        // Built, or the reason it was not: either way the laptop hears it
        // in the log, rather than waiting out a box that will never answer
        let mut said = serde_json::json!({
            "v": 2,
            "kind": "built",
            "attester": me,
            "flake": flake,
            "rev": rev,
            "order": id,
            "job": job.name,
            "path": job.path,
        });
        let mut replaced = false;
        let built = (|| -> Result<(String, String)> {
            let drv = eval_job(flake, rev, &job, "drvPath")?;
            let path = eval_job(flake, rev, &job, "outPath")?;
            ensure!(
                path == job.path,
                "{} evaluates here to {path}, not {}",
                &rev[..12],
                job.path
            );
            if !realise_unless(&drv, &gone)? {
                replaced = true;
                bail!("replaced");
            }
            let closure = attest(&drv, &path, &mut checked)?;
            let nar = release::nar_hash_from_path_info(
                &sh("nix", &["path-info", "--json", &path])?,
                &path,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok((closure, nar))
        })();
        if replaced {
            eprintln!(
                "== order {id}: replaced, or a newer one goes first, during {}; the next run takes that",
                job.name
            );
            return Ok(());
        }
        match built {
            Ok((closure, nar)) => {
                said["closure"] = closure.into();
                said["narHash"] = nar.into();
            }
            Err(e) => {
                eprintln!("   {}: {e:#}", job.name);
                said["error"] = format!("{e:#}")
                    .chars()
                    .take(2000)
                    .collect::<String>()
                    .into();
            }
        }
        let statement = serde_json::to_string(&said)?;
        let signature = sign(&statement)?;
        post(
            submit,
            &serde_json::to_string(&serde_json::json!({
                "statement": statement,
                "signature": signature,
            }))?,
        )?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(DONE)?;
        writeln!(f, "{id} {}", job.name)?;
    }
    Ok(())
}

/// Whether the order at `url` is now another, newer one. Not being able to
/// read it says nothing: the build goes on.
fn superseded(url: &str, trusted: &ed25519_dalek::VerifyingKey, id: &str) -> bool {
    order_id(url, trusted).is_some_and(|now| now != id)
}

/// the id of the signed order at `url` now, if it can be read
fn order_id(url: &str, trusted: &ed25519_dalek::VerifyingKey) -> Option<String> {
    let mut r = ureq::get(url).call().ok()?;
    let raw = r.body_mut().read_to_string().ok()?;
    let o = release::verify_doc(&raw, "build-order", trusted).ok()?;
    o["id"].as_str().map(str::to_string)
}

/// `nix-store --realise`, asking `gone` every minute while it builds: true
/// once built, false if it was stopped because `gone` said so
fn realise_unless(drv: &str, gone: &dyn Fn() -> bool) -> Result<bool> {
    let mut c = Command::new("nix-store")
        .args(["--realise", drv])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("nix-store")?;
    loop {
        for _ in 0..60 {
            if let Some(st) = c.try_wait()? {
                ensure!(st.success(), "nix-store --realise failed");
                return Ok(true);
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        if gone() {
            // the daemon stops a build its last client has left
            let _ = c.kill();
            let _ = c.wait();
            return Ok(false);
        }
    }
}

/// one result to dd-control over its socket: a plain HTTP/1.0 POST
fn post(sock: &str, body: &str) -> Result<()> {
    use std::io::Read as _;
    let mut c = std::os::unix::net::UnixStream::connect(sock)
        .with_context(|| format!("dd-control at {sock}"))?;
    write!(
        c,
        "POST /built HTTP/1.0\r\nHost: dd-control\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )?;
    let mut answer = String::new();
    c.read_to_string(&mut answer)?;
    let status = answer.split_whitespace().nth(1).unwrap_or_default();
    ensure!(
        status == "202",
        "dd-control said {}",
        answer.lines().next().unwrap_or_default()
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
        // every mismatch named at once, not only the first
        sh(
            "nix-store",
            &[&["--realise", "--check", "--keep-going"][..], &list].concat(),
        )
        .context("a path did not rebuild the same: nothing is vouched for")?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(CHECKED)?;
        for (p, e) in &entries {
            if to_check.contains(p) {
                let line = format!("{p} {}", e["narHash"].as_str().unwrap_or_default());
                // one write a line: a release's attestation and a build
                // order may append at once, and their lines must not mix
                f.write_all(format!("{line}\n").as_bytes())?;
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

/// one release's attestation at a time: two would rebuild the same
/// things twice. A build order does not take it (orders)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn order() -> serde_json::Value {
        serde_json::json!({
            "id": "o1",
            "jobs": [
                { "name": "app-linux", "attr": "legacyPackages.x86_64-linux.appRelease",
                  "version": "1.2.3", "part": "linux",
                  "path": "/nix/store/aaaa-commonty-linux-1.2.3", "builders": ["node1", "node3"] },
                { "name": "app-android", "attr": "legacyPackages.x86_64-linux.appRelease",
                  "version": "1.2.3", "part": "android",
                  "path": "/nix/store/bbbb-commonty-android-1.2.3", "builders": ["node2", "node3"] },
            ],
        })
    }

    #[test]
    fn a_box_takes_only_the_jobs_that_name_it() {
        let names = |me| {
            jobs_for(&order(), me)
                .unwrap()
                .into_iter()
                .map(|j| j.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("node1"), ["app-linux"]);
        assert_eq!(names("node3"), ["app-linux", "app-android"]);
        assert!(names("node9").is_empty());
    }

    #[test]
    fn an_order_cannot_smuggle_nix_or_shell() {
        for (k, v) in [
            ("attr", "x; rm -rf /"),
            ("attr", r"a.b\"),
            ("part", "linux.drvPath)"),
            ("version", r#"1.2.3" + builtins.exec"#),
            ("path", "/nix/store/a/../../etc"),
            ("path", "/etc/passwd"),
            ("name", "a b"),
        ] {
            let mut o = order();
            o["jobs"][0][k] = v.into();
            assert!(jobs_for(&o, "node1").is_err(), "{k}={v} was taken");
        }
    }
}
