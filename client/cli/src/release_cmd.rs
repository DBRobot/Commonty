//! `dd release`: the fleet moves when this machine says so.
//!
//! `publish` builds every box in fleet/boxes.json from one commit, puts the
//! closures where the boxes fetch from, and pushes one signed file naming
//! the commit, a counter and every box's store path and nar hash. The
//! agent on each box does the rest. CI never does this: a box's runner
//! would be signing what a box built.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use auth::KeyStore as _;
use clap::Subcommand;

/// keyring account holding the release key, base64
const ACCOUNT: &str = "release-key";
pub const DEFAULT_URL: &str =
    "https://git.commonty.org/david/commonty/raw/branch/releases/current.json";
/// the nix-cache bucket, through node1's garage over the tailnet
const DEFAULT_CACHE: &str =
    "s3://nix-cache?endpoint=100.95.31.105:3900&scheme=http&region=us-east-1";

#[derive(Subcommand)]
pub enum ReleaseCmd {
    /// Make the release key here and write its public half to
    /// fleet/release.pub, which every box is built with.
    Init,
    /// Build every box from a ref, push the closures to the cache, sign
    /// and publish the release.
    Publish {
        /// a local branch that matches the forge's
        #[arg(long, default_value = "main")]
        r#ref: String,
        /// where the closures go: the bucket every box fetches from
        #[arg(long, default_value = DEFAULT_CACHE)]
        cache: String,
        /// where boxes read the release from
        #[arg(long, default_value = DEFAULT_URL)]
        url: String,
        /// build and sign, print the release, push nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Sign a payload file with the release key. `publish` does this
    /// itself; the vm test drives it directly.
    Sign {
        payload: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Sign the app a tag built: fetch every file of the GitHub release,
    /// check GitHub's attestation that it was built from that tag in that
    /// repository, hash each one, and publish a manifest signed with the
    /// release key beside the box releases. The download page offers only
    /// what a signed manifest names.
    App {
        /// the release tag, e.g. v0.1.0
        #[arg(long)]
        tag: String,
        /// the GitHub repository it was built in
        #[arg(long, default_value = "DBRobot/Home-Server")]
        from: String,
        /// sign and print the manifest, push nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// The release the boxes see.
    Show {
        #[arg(long, default_value = DEFAULT_URL)]
        url: String,
    },
    /// What every box says it runs, from each box's own metrics over the
    /// tailnet, next to what the current release says it should.
    Status {
        #[arg(long, default_value = DEFAULT_URL)]
        url: String,
    },
}

pub fn run(cmd: ReleaseCmd, repo: &str, keys: &auth::Store) -> Result<()> {
    match cmd {
        ReleaseCmd::Init => {
            if keys.get(ACCOUNT)?.is_some() {
                bail!("there is a release key here already");
            }
            let k = release::generate();
            keys.set(ACCOUNT, &release::encode_secret(&k))?;
            let public = release::encode_public(&k.verifying_key());
            let root = repo_root(repo)?;
            let path = root.join("fleet/release.pub");
            std::fs::write(&path, format!("{public}\n"))?;
            println!("release key made; public half in {}", path.display());
            println!("{public}");
        }
        ReleaseCmd::Sign { payload, out } => {
            let key = load(keys)?;
            let payload: release::Payload =
                serde_json::from_slice(&std::fs::read(&payload).context("reading payload")?)?;
            let signed = release::sign(payload, &key)?;
            std::fs::write(&out, serde_json::to_vec_pretty(&signed)?)?;
            println!(
                "signed release {} into {}",
                signed.payload.counter,
                out.display()
            );
        }
        ReleaseCmd::Show { url } => {
            let signed = fetch(&url)?.context("no release published yet")?;
            print(&signed);
        }
        ReleaseCmd::Status { url } => status(repo, &url)?,
        ReleaseCmd::App { tag, from, dry_run } => app(repo, keys, &tag, &from, dry_run)?,
        ReleaseCmd::Publish {
            r#ref,
            cache,
            url,
            dry_run,
        } => publish(repo, &r#ref, &cache, &url, dry_run, keys)?,
    }
    Ok(())
}

fn publish(
    repo: &str,
    r#ref: &str,
    cache: &str,
    url: &str,
    dry_run: bool,
    keys: &auth::Store,
) -> Result<()> {
    let key = load(keys)?;
    let root = repo_root(repo)?;
    let rev = git(&root, &["rev-parse", &format!("refs/heads/{ref}")])
        .with_context(|| format!("no local branch {ref}"))?;
    let forge_rev =
        git(&root, &["rev-parse", &format!("refs/remotes/forge/{ref}")]).unwrap_or_default();
    ensure!(
        rev == forge_rev,
        "local {ref} ({}) is not what the forge has - push it first",
        &rev[..12]
    );

    let boxes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/boxes.json"))?)?;
    let names: Vec<String> = boxes
        .as_object()
        .context("boxes.json is not an object")?
        .keys()
        .cloned()
        .collect();

    let (key_id, key_secret, signing_key) = cache_writer(&root)?;

    // What CI built is what ships, but only once this machine has worked
    // out for itself what that should be. A commit status is written by
    // whoever holds a job token, a runner credential, or the forge: taking
    // the path from one and signing it is letting them choose what every
    // box runs. So each path is compared against an evaluation of the very
    // same ref, done here. Evaluation, not a build - seconds, no store
    // written - which settles "is this the closure this commit describes".
    // What it cannot settle is whether the bytes behind that path are the
    // ones the derivation makes; only building here, or a second builder
    // agreeing, would say that.
    let mut built = BTreeMap::new();
    let mut from_ci = false;
    if let Some(paths) = recorded_builds(&root, url, &rev, &names, keys)? {
        let mut ok = true;
        for (name, path) in &paths {
            let flake = format!(
                "git+file://{}?ref={ref}#nixosConfigurations.{name}.config.system.build.toplevel.outPath",
                root.display()
            );
            let want = sh("nix", &["eval", "--raw", &flake])
                .with_context(|| format!("evaluating {name} from {ref}"))?
                .trim()
                .to_owned();
            if want != *path {
                bail!(
                    "{name}: CI reported {path}, but {ref} evaluates to {want}.\n\
                     Nothing is signed. Whoever posted that status does not agree \
                     with the commit."
                );
            }
            let info = Command::new("nix")
                .args(["path-info", "--json", "--store", cache, path])
                .env("AWS_ACCESS_KEY_ID", &key_id)
                .env("AWS_SECRET_ACCESS_KEY", &key_secret)
                .output()?;
            if !info.status.success() {
                eprintln!("   {name}: {path} is not in the cache; building instead");
                ok = false;
                break;
            }
            let nar_hash =
                release::nar_hash_from_path_info(&String::from_utf8_lossy(&info.stdout), path)?;
            // every path underneath, too: the toplevel's own hash pins the
            // names of its dependencies and nothing about their contents
            let rec = Command::new("nix")
                .args(["path-info", "--json", "--recursive", "--store", cache, path])
                .env("AWS_ACCESS_KEY_ID", &key_id)
                .env("AWS_SECRET_ACCESS_KEY", &key_secret)
                .output()?;
            if !rec.status.success() {
                eprintln!("   {name}: cannot read the closure from the cache; building instead");
                ok = false;
                break;
            }
            let closure = release::closure_digest(&String::from_utf8_lossy(&rec.stdout))?;
            eprintln!("== {name} as CI built it\n   {path}");
            built.insert(
                name.clone(),
                release::BoxRelease {
                    path: path.clone(),
                    nar_hash,
                    closure: Some(closure),
                },
            );
        }
        from_ci = ok;
        if !ok {
            built.clear();
        }
    }

    // otherwise every box, from git, never from the working tree. The
    // build itself runs wherever DD_BUILD_STORE says (a box:
    // ssh-ng://admin@...), so the laptop signs what a box built and keeps
    // no store of its own; unset, it builds here.
    let store = std::env::var("DD_BUILD_STORE").ok();
    for name in names.iter().filter(|_| !from_ci) {
        eprintln!("== build {name} from {ref} ({})", &rev[..12]);
        let flake = format!(
            "git+file://{}?ref={ref}#nixosConfigurations.{name}.config.system.build.toplevel",
            root.display()
        );
        let mut args = vec!["build", "--no-link", "--print-out-paths"];
        if let Some(st) = &store {
            args.extend(["--eval-store", "auto", "--store", st]);
        }
        args.push(&flake);
        let path = sh("nix", &args)
            .with_context(|| format!("building {name}"))?
            .trim()
            .to_owned();
        let mut info_args = vec!["path-info", "--json"];
        if let Some(st) = &store {
            info_args.extend(["--store", st]);
        }
        info_args.push(&path);
        let info = sh("nix", &info_args)?;
        let nar_hash = release::nar_hash_from_path_info(&info, &path)?;
        let mut rec_args = vec!["path-info", "--json", "--recursive"];
        if let Some(st) = &store {
            rec_args.extend(["--store", st]);
        }
        rec_args.push(&path);
        let closure = release::closure_digest(&sh("nix", &rec_args)?)?;
        eprintln!("   {path}");
        built.insert(
            name.clone(),
            release::BoxRelease {
                path,
                nar_hash,
                closure: Some(closure),
            },
        );
    }

    // Two boxes vouch for every box's closure before anything is signed:
    // each evaluates the commit itself and accounts for every path, by
    // upstream's signature or by building it (dd-attest). CI, the forge
    // and the cache can each lie about a path; not past two rebuilds.
    // DD_ATTEST=off is for the one release that first carries dd-attest.
    if std::env::var("DD_ATTEST").as_deref() == Ok("off") {
        eprintln!("== DD_ATTEST=off: no box was asked to vouch for this release");
    } else {
        attest(&boxes, url, &rev, &built)?;
    }

    // The counter only ever goes up. A current release that cannot be
    // fetched is not "none yet" if the releases branch has history: the
    // forge was just unreachable, and a release 1 on top of a 37 would be
    // ignored by every box and a lie in the history.
    let has_history = std::process::Command::new("git")
        .args(["ls-remote", "--exit-code", "--heads", "forge", "releases"])
        .current_dir(&root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let counter = match ureq_get(url).ok() {
        Some(raw) => {
            // The forge serves this file and does not sign it. Taking the
            // counter from it unverified means whoever serves it chooses
            // the next one, and a u64::MAX would leave every box refusing
            // everything after it, for good, with nothing to publish over
            // the top. It is signed by the key whose public half is in the
            // repo, so check it here before believing a number out of it.
            let trusted = release::decode_public(
                std::fs::read_to_string(root.join("fleet/release.pub"))?.trim(),
            )
            .map_err(|e| anyhow::anyhow!("fleet/release.pub: {e}"))?;
            let current = release::verify_json(&raw, &trusted)
                .map_err(|e| anyhow::anyhow!("the published release does not verify: {e}"))?;
            current
                .payload
                .counter
                .checked_add(1)
                .context("the release counter is at its limit")?
        }
        None if has_history => {
            bail!(
                "no current release could be read, yet the releases branch exists: the forge is not answering; not starting over at 1"
            )
        }
        None => 1,
    };
    // DD_RELEASE_FORM=legacy for exactly one release: the one that carries
    // an agent able to read the newer form to boxes whose agent cannot. It
    // signs the old way and leaves out every field the old agent has never
    // heard of - which today is the closure digest.
    let form = match std::env::var("DD_RELEASE_FORM").as_deref() {
        Ok("legacy") => release::Form::Legacy,
        Ok("v2") | Err(_) => release::Form::V2,
        Ok(other) => bail!("DD_RELEASE_FORM={other}: legacy or v2"),
    };
    if form == release::Form::Legacy {
        eprintln!("== the old form: no closure digest, so the agents running now accept it");
        for b in built.values_mut() {
            b.closure = None;
        }
    }
    let signed = release::sign_as(
        release::Payload {
            counter,
            rev: rev.clone(),
            issued: release::now(),
            boxes: built,
        },
        &key,
        form,
    )?;
    let body = serde_json::to_string_pretty(&signed)?;
    if dry_run {
        println!("{body}");
        return Ok(());
    }

    if !from_ci {
        copy_to_cache(cache, &store, &signed, &key_id, &key_secret, &signing_key)?;
    }

    eprintln!("== publish release {counter}");
    to_releases(
        &root,
        &[
            ("current.json".into(), body.clone()),
            (format!("history/{counter}.json"), body.clone()),
        ],
        &format!("release {counter}: {}", &rev[..12]),
    )?;
    print(&signed);
    nudge(&boxes);
    Ok(())
}

/// Every box told to look now rather than on its next timer. The box does
/// all the deciding itself - it fetches the release, checks the key, the
/// counter and every hash - so this changes when it looks, not what it
/// accepts. A box that cannot be reached finds it on its timer as before.
fn nudge(boxes: &serde_json::Value) {
    let Some(boxes) = boxes.as_object() else {
        return;
    };
    let asks: Vec<_> = boxes
        .iter()
        .map(|(name, b)| {
            let tailnet = b["tailnet"].as_str().unwrap_or_default().to_string();
            let c = Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
                .arg(format!("admin@{tailnet}"))
                .args([
                    "sudo",
                    "systemctl",
                    "start",
                    "--no-block",
                    "dd-agent.service",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            (name.clone(), c)
        })
        .collect();
    for (name, r) in asks {
        match r {
            Ok(s) if s.success() => eprintln!("   {name}: told to look now"),
            _ => eprintln!("   {name}: not reached; it looks on its own timer"),
        }
    }
}

const ATTEST_NAMESPACE: &str = "commonty-attest-v1";
/// boxes that must agree with CI on each box's closure
const ATTEST_NEED: usize = 2;

/// Ask every box, at once, to vouch for the release; each answer is
/// checked against the box's ssh host key in boxes.json, and every box of
/// the release needs ATTEST_NEED of them agreeing on its path and closure.
fn attest(
    boxes: &serde_json::Value,
    url: &str,
    rev: &str,
    built: &BTreeMap<String, release::BoxRelease>,
) -> Result<()> {
    let boxes = boxes.as_object().context("boxes.json is not an object")?;
    ensure!(
        boxes.len() >= ATTEST_NEED,
        "{} box(es) cannot make {ATTEST_NEED} that agree; DD_ATTEST=off to publish without",
        boxes.len()
    );
    let base = url
        .split("/raw/")
        .next()
        .filter(|b| b.starts_with("https://"))
        .context("the release url is not a forge raw url")?;
    let flake = format!("git+{base}");
    let mut want: Vec<String> = Vec::new();
    for (name, b) in built {
        ensure!(
            b.closure.is_some(),
            "{name}: no closure digest to vouch for"
        );
        want.push(format!("{name}={}", b.path));
    }
    eprintln!("== asking {} box(es) to vouch", boxes.len());
    let mut asks = Vec::new();
    for (name, b) in boxes {
        let tailnet = b["tailnet"].as_str().unwrap_or_default().to_string();
        let mut c = Command::new("ssh");
        c.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=30",
        ])
        .arg(format!("admin@{tailnet}"))
        .args([
            "sudo",
            "/run/current-system/sw/bin/dd-attest",
            "--flake",
            &flake,
            "--rev",
            rev,
        ]);
        for w in &want {
            c.args(["--box", w]);
        }
        c.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped());
        asks.push((name.clone(), c.spawn()));
    }
    let mut agree: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (attester, child) in asks {
        let answer = child
            .map_err(anyhow::Error::from)
            .and_then(|c| Ok(c.wait_with_output()?))
            .and_then(|o| {
                ensure!(o.status.success(), "dd-attest failed");
                Ok(String::from_utf8(o.stdout)?)
            })
            .and_then(|out| {
                let host_key = boxes[&attester]["hostKey"]
                    .as_str()
                    .context("no hostKey in boxes.json")?;
                vouched(&attester, host_key, out.trim(), &flake, rev)
            });
        match answer {
            Ok(said) => {
                for (name, b) in built {
                    let same = said
                        .get(name)
                        .is_some_and(|(p, c)| *p == b.path && Some(c) == b.closure.as_ref());
                    if same {
                        agree
                            .entry(name.clone())
                            .or_default()
                            .push(attester.clone());
                    } else {
                        eprintln!("   {attester} does not agree on {name}");
                    }
                }
            }
            Err(e) => eprintln!("   {attester}: {e:#}"),
        }
    }
    for name in built.keys() {
        let who = agree.get(name).cloned().unwrap_or_default();
        ensure!(
            who.len() >= ATTEST_NEED,
            "{name}: {} box(es) vouched ({}), {ATTEST_NEED} needed. Nothing is signed.",
            who.len(),
            who.join(", ")
        );
        eprintln!("   {name}: vouched for by {}", who.join(", "));
    }
    Ok(())
}

/// one box's signed answer, checked: box name -> (path, closure)
fn vouched(
    attester: &str,
    host_key: &str,
    answer: &str,
    flake: &str,
    rev: &str,
) -> Result<BTreeMap<String, (String, String)>> {
    let v: serde_json::Value =
        serde_json::from_str(answer.lines().last().unwrap_or_default()).context("the answer")?;
    let statement = v["statement"].as_str().context("no statement")?;
    let signature = v["signature"].as_str().context("no signature")?;
    let dir = std::env::temp_dir().join(format!("dd-attest-{}-{attester}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let allowed = dir.join("allowed");
    let sig = dir.join("sig");
    std::fs::write(
        &allowed,
        format!("{attester} namespaces=\"{ATTEST_NAMESPACE}\" {host_key}\n"),
    )?;
    std::fs::write(&sig, signature)?;
    let mut c = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-f"])
        .arg(&allowed)
        .args(["-I", attester, "-n", ATTEST_NAMESPACE, "-s"])
        .arg(&sig)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::io::Write::write_all(&mut c.stdin.take().unwrap(), statement.as_bytes())?;
    let ok = c.wait()?.success();
    let _ = std::fs::remove_dir_all(&dir);
    ensure!(ok, "the signature is not {attester}'s host key");
    let st: serde_json::Value = serde_json::from_str(statement)?;
    ensure!(
        st["v"] == 1 && st["attester"] == attester && st["rev"] == rev && st["flake"] == flake,
        "the statement is about something else"
    );
    let mut out = BTreeMap::new();
    for (name, b) in st["boxes"].as_object().context("no boxes")? {
        out.insert(
            name.clone(),
            (
                b["path"].as_str().unwrap_or_default().to_string(),
                b["closure"].as_str().unwrap_or_default().to_string(),
            ),
        );
    }
    Ok(out)
}

/// Each box answers for itself: its prometheus holds what its agent last
/// wrote. Nothing aggregates; a box that is off says nothing.
fn status(repo: &str, url: &str) -> Result<()> {
    let root = repo_root(repo)?;
    let boxes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/boxes.json"))?)?;
    let current = fetch(url)?;
    match &current {
        Some(s) => println!(
            "release {}  commit {}",
            s.payload.counter,
            &s.payload.rev[..12.min(s.payload.rev.len())]
        ),
        None => println!("no release published yet"),
    }
    for (name, b) in boxes.as_object().context("boxes.json is not an object")? {
        let tailnet = b["tailnet"].as_str().unwrap_or("-");
        // a box's prometheus answers on its own loopback only; the owner
        // reaches it the way he reaches the box
        let q = |expr: &str| -> Option<serde_json::Value> {
            let out = Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
                .arg(format!("admin@{tailnet}"))
                .arg(format!(
                    "curl -sf -m 5 --get --data-urlencode 'query={expr}' http://127.0.0.1:9090/api/v1/query"
                ))
                .stdin(std::process::Stdio::null())
                .output()
                .ok()?;
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
            v["data"]["result"].as_array()?.first().cloned()
        };
        let counter = q("dd_agent_counter")
            .and_then(|r| r["value"][1].as_str().map(str::to_owned))
            .unwrap_or_else(|| "-".into());
        let info = q("dd_agent_info");
        let running = info
            .as_ref()
            .and_then(|r| r["metric"]["path"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "?".into());
        let result = info
            .as_ref()
            .and_then(|r| r["metric"]["result"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "unreachable".into());
        let want = current
            .as_ref()
            .and_then(|s| s.payload.boxes.get(name))
            .map(|b| b.path.as_str());
        let verdict = match want {
            Some(w) if w == running => "current",
            Some(_) => "behind",
            None => "-",
        };
        println!("{name}  counter {counter}  last run {result}  {verdict}");
        println!("  runs {running}");
    }
    Ok(())
}

/// The closures into the cache bucket, signed with the fleet's cache key.
fn copy_to_cache(
    cache: &str,
    store: &Option<String>,
    signed: &release::Signed,
    key_id: &str,
    key_secret: &str,
    signing_key: &str,
) -> Result<()> {
    eprintln!("== copy the closures to the cache");
    // the signing key in a file only for the length of the copy
    let key_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let key_file = PathBuf::from(key_dir).join(format!("dd-cache-key-{}", std::process::id()));
    {
        use std::io::Write as _;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        // the key is on disk for the length of one copy; where the
        // filesystem has modes, it is readable by nobody else
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let mut f = o.open(&key_file)?;
        f.write_all(signing_key.as_bytes())?;
    }
    let to = format!("{cache}&secret-key={}", key_file.display());
    let mut args = vec!["copy", "--to", to.as_str()];
    if let Some(st) = &store {
        args.extend(["--from", st.as_str()]);
    }
    args.extend(signed.payload.boxes.values().map(|b| b.path.as_str()));
    let status = Command::new("nix")
        .args(&args)
        .env("AWS_ACCESS_KEY_ID", key_id)
        .env("AWS_SECRET_ACCESS_KEY", key_secret)
        .status();
    let _ = std::fs::remove_file(&key_file);
    ensure!(
        status.context("running nix copy")?.success(),
        "copying to the cache"
    );

    Ok(())
}

/// The store paths CI recorded for this tree, if it recorded every box:
/// `build/<box>` statuses on the commit or a parent with the same tree.
fn recorded_builds(
    root: &Path,
    url: &str,
    rev: &str,
    names: &[String],
    keys: &auth::Store,
) -> Result<Option<BTreeMap<String, String>>> {
    // https://forge/owner/repo/raw/... -> https://forge/api/v1/repos/owner/repo
    let Some((repo_url, _)) = url.split_once("/raw/") else {
        return Ok(None);
    };
    let Some(slash) = repo_url
        .find("://")
        .map(|i| i + 3)
        .and_then(|i| repo_url[i..].find('/').map(|j| i + j))
    else {
        return Ok(None);
    };
    let api = format!(
        "{}/api/v1/repos/{}",
        &repo_url[..slash],
        &repo_url[slash + 1..]
    );
    let tree = git(root, &["rev-parse", &format!("{rev}^{{tree}}")])?;
    let token = match auth::device::load(keys)? {
        Some(kp) => match keys.get(crate::USER)? {
            Some(user) => auth::device::mint(&kp, &user, std::time::Duration::from_secs(600))?,
            None => return Ok(None),
        },
        None => return Ok(None),
    };
    for cand in [rev.to_owned(), format!("{rev}^2"), format!("{rev}^1")] {
        let Ok(sha) = git(root, &["rev-parse", "--verify", "--quiet", &cand]) else {
            continue;
        };
        if git(root, &["rev-parse", &format!("{sha}^{{tree}}")])? != tree {
            continue;
        }
        // the header on stdin, not the command line: an argument is readable
        // by every process on this machine while curl runs
        let out = {
            use std::io::Write as _;
            let mut c = Command::new("curl")
                .args([
                    "-sf",
                    "-H",
                    "@-",
                    &format!("{api}/commits/{sha}/statuses?limit=50"),
                ])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()?;
            c.stdin
                .take()
                .context("curl's stdin")?
                .write_all(format!("Authorization: Bearer {token}\n").as_bytes())?;
            c.wait_with_output()?
        };
        if !out.status.success() {
            continue;
        }
        let statuses: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)?;
        let mut paths = BTreeMap::new();
        // newest first; the first success per context wins
        for st in &statuses {
            let (Some(ctx), Some(state), Some(desc)) = (
                st["context"].as_str(),
                st["status"].as_str(),
                st["description"].as_str(),
            ) else {
                continue;
            };
            if state != "success" || !desc.starts_with("/nix/store/") {
                continue;
            }
            if let Some(name) = ctx.strip_prefix("build/") {
                paths
                    .entry(name.to_owned())
                    .or_insert_with(|| desc.to_owned());
            }
        }
        if names.iter().all(|n| paths.contains_key(n)) {
            paths.retain(|n, _| names.contains(n));
            return Ok(Some(paths));
        }
    }
    Ok(None)
}

/// The key that writes the cache bucket: in the private half of the fleet
/// file, readable by this machine's sops key and nobody else's.
fn cache_writer(root: &Path) -> Result<(String, String, String)> {
    let out = Command::new(std::env::current_exe()?)
        .args(["secret", "run", "--", "-d", "--output-type", "json"])
        .arg(root.join("secrets/fleet.yaml"))
        .output()?;
    ensure!(out.status.success(), "decrypting secrets/fleet.yaml");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let id = v["cache"]["keyId"]
        .as_str()
        .context("fleet.yaml: cache.keyId")?;
    let secret = v["cache"]["keySecret"]
        .as_str()
        .context("fleet.yaml: cache.keySecret")?;
    let signing = v["cache"]["signingKey"]
        .as_str()
        .context("fleet.yaml: cache.signingKey")?;
    Ok((id.to_owned(), secret.to_owned(), signing.to_owned()))
}

fn print(signed: &release::Signed) {
    let p = &signed.payload;
    println!(
        "release {}  commit {}  signed by {}",
        p.counter,
        &p.rev[..12.min(p.rev.len())],
        identity::fingerprint(&signed.signer)
    );
    for (name, b) in &p.boxes {
        println!("  {name}  {}", b.path);
    }
}

fn fetch(url: &str) -> Result<Option<release::Signed>> {
    match ureq_get(url) {
        Ok(body) => Ok(Some(
            serde_json::from_str(&body).context("release file is not json")?,
        )),
        Err(NotFound) => Ok(None),
    }
}

struct NotFound;

fn ureq_get(url: &str) -> std::result::Result<String, NotFound> {
    // curl is on every machine this runs on; no need for another http stack
    let out = Command::new("curl")
        .args(["-sf", "-L", url])
        .output()
        .map_err(|_| NotFound)?;
    if !out.status.success() {
        return Err(NotFound);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(crate) fn load(keys: &auth::Store) -> Result<ed25519_dalek::SigningKey> {
    let s = keys
        .get(ACCOUNT)?
        .context("no release key here - `dd release init`")?;
    release::decode_secret(&s).map_err(|e| anyhow::anyhow!("release key: {e}"))
}

/// Write files onto the releases branch and push it: the history of what
/// the release key has signed, never merged anywhere.
fn to_releases(root: &std::path::Path, files: &[(String, String)], message: &str) -> Result<()> {
    let wt = std::env::temp_dir().join(format!("dd-release-{}", std::process::id()));
    let have_branch = git(
        root,
        &["ls-remote", "--exit-code", "--heads", "forge", "releases"],
    )
    .is_ok();
    if have_branch {
        git(root, &["fetch", "-q", "forge", "releases"])?;
        git(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                wt.to_str().unwrap(),
                "forge/releases",
            ],
        )?;
    } else {
        git(
            root,
            &["worktree", "add", "-q", "--orphan", wt.to_str().unwrap()],
        )?;
    }
    let result = (|| -> Result<()> {
        for (name, body) in files {
            let p = wt.join(name);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&p, body)?;
            git(&wt, &["add", name])?;
        }
        git(&wt, &["commit", "-q", "-m", message])?;
        git(&wt, &["push", "-q", "forge", "HEAD:refs/heads/releases"])?;
        Ok(())
    })();
    let _ = git(
        root,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );
    result
}

/// The files of a GitHub release: name and download url.
fn github_assets(from: &str, tag: &str) -> Result<Vec<(String, String)>> {
    let body = ureq_get(&format!(
        "https://api.github.com/repos/{from}/releases/tags/{tag}"
    ))
    .map_err(|_| anyhow::anyhow!("no release {tag} in {from}"))?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let out: Vec<(String, String)> = v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some((
                a["name"].as_str()?.to_string(),
                a["browser_download_url"].as_str()?.to_string(),
            ))
        })
        .collect();
    if out.is_empty() {
        bail!("release {tag} in {from} has no files");
    }
    Ok(out)
}

fn app(repo: &str, keys: &auth::Store, tag: &str, from: &str, dry_run: bool) -> Result<()> {
    use sha2::Digest as _;
    let root = repo_root(repo)?;
    let key = load(keys)?;
    // The release key vouches for bytes it has seen, built from source it
    // can name. `gh attestation verify` checks GitHub's signed statement
    // that this file came out of a workflow run in `from` - and the run's
    // commit is then checked to be what `tag` points at here, so a file
    // built from some other commit, or uploaded by hand, is refused.
    let want = git(&root, &["rev-parse", &format!("{tag}^{{commit}}")])
        .with_context(|| format!("no tag {tag} here; fetch the forge's tags"))?;
    let want = want.trim().to_string();
    let dir = std::env::temp_dir().join(format!("dd-app-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let result = (|| -> Result<serde_json::Value> {
        let mut files = serde_json::Map::new();
        for (name, url) in github_assets(from, tag)? {
            let path = dir.join(&name);
            let st = Command::new("curl")
                .args(["-sfL", "-o"])
                .arg(&path)
                .arg(&url)
                .status()?;
            if !st.success() {
                bail!("downloading {name}");
            }
            // this repo's pinned gh, and only a signature made by the
            // release workflow counts - not any workflow in the repository
            let workflow = format!("{from}/.github/workflows/release.yml");
            let token = Command::new("gh")
                .args(["auth", "token"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            let mut gh = Command::new("nix");
            gh.current_dir(&root)
                .args([
                    "run",
                    ".#gh",
                    "--",
                    "attestation",
                    "verify",
                    "--format",
                    "json",
                ])
                .args(["--repo", from, "--signer-workflow", &workflow])
                .arg(&path);
            if let Some(t) = token {
                gh.env("GH_TOKEN", t);
            }
            let att = gh
                .output()
                .context("running gh: it checks the build's provenance")?;
            if !att.status.success() {
                bail!(
                    "{name}: no attestation from {from} verifies it:\n{}",
                    String::from_utf8_lossy(&att.stderr)
                );
            }
            let built_from = serde_json::from_slice::<serde_json::Value>(&att.stdout)?
                .as_array()
                .and_then(|a| a.first())
                .and_then(|a| {
                    a.pointer("/verificationResult/statement/predicate/buildDefinition/resolvedDependencies/0/digest/gitCommit")
                        .and_then(|c| c.as_str())
                        .map(str::to_string)
                })
                .context("the attestation names no commit")?;
            if built_from != want {
                bail!("{name} was built from {built_from}, and {tag} is {want}");
            }
            let bytes = std::fs::read(&path)?;
            let sha = format!("{:x}", sha2::Sha256::digest(&bytes));
            eprintln!("   {name}  {sha}");
            files.insert(
                name,
                serde_json::json!({ "sha256": sha, "size": bytes.len(), "url": url }),
            );
        }
        Ok(serde_json::json!({
            "tag": tag,
            "commit": want,
            "from": from,
            "issued": release::now(),
            "files": files,
        }))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    let signed = release::sign_doc("app", result?, &key)?;
    let body = serde_json::to_string_pretty(&signed)?;
    if dry_run {
        println!("{body}");
        return Ok(());
    }
    to_releases(
        &root,
        &[
            ("app.json".into(), body.clone()),
            (format!("app-history/{tag}.json"), body),
        ],
        &format!("app {tag}"),
    )?;
    println!("app {tag} signed; the download page offers it within ten minutes");
    Ok(())
}

fn repo_root(repo: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(
        git(Path::new(repo), &["rev-parse", "--show-toplevel"]).context("not in the repository")?,
    ))
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn sh(bin: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("running {bin}"))?;
    if !out.status.success() {
        bail!(
            "{bin} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keygen(dir: &Path, name: &str) -> (PathBuf, String) {
        let k = dir.join(name);
        let ok = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&k)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let public = std::fs::read_to_string(k.with_extension("pub")).unwrap();
        (
            k,
            public
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    fn sign(key: &Path, namespace: &str, statement: &str) -> String {
        let mut c = Command::new("ssh-keygen")
            .args(["-Y", "sign", "-q", "-f"])
            .arg(key)
            .args(["-n", namespace])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut c.stdin.take().unwrap(), statement.as_bytes()).unwrap();
        String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap()
    }

    /// what a box signs is taken only from that box's host key, in the
    /// attest namespace, about this commit
    #[test]
    fn a_box_answer_counts_only_as_itself() {
        let dir = std::env::temp_dir().join(format!("dd-vouch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (k1, pub1) = keygen(&dir, "node1");
        let (_, pub2) = keygen(&dir, "node2");
        let flake = "git+https://git.example/o/r";
        let rev = "a".repeat(40);
        let statement = serde_json::json!({
            "v": 1, "attester": "node1", "flake": flake, "rev": rev,
            "boxes": { "node1": { "path": "/nix/store/x-node1", "closure": "c1" } },
        })
        .to_string();
        let answer = |st: &str, ns: &str| {
            serde_json::json!({ "statement": st, "signature": sign(&k1, ns, st) }).to_string()
        };

        let said = vouched(
            "node1",
            &pub1,
            &answer(&statement, ATTEST_NAMESPACE),
            flake,
            &rev,
        )
        .unwrap();
        assert_eq!(said["node1"], ("/nix/store/x-node1".into(), "c1".into()));

        // node2's key does not stand behind node1's signature
        assert!(
            vouched(
                "node1",
                &pub2,
                &answer(&statement, ATTEST_NAMESPACE),
                flake,
                &rev
            )
            .is_err()
        );
        // a signature made for anything else is not an attestation
        assert!(vouched("node1", &pub1, &answer(&statement, "file"), flake, &rev).is_err());
        // the statement cannot be changed under its signature
        let good = answer(&statement, ATTEST_NAMESPACE);
        let forged = good.replace("c1", "c2");
        assert!(vouched("node1", &pub1, &forged, flake, &rev).is_err());
        // nor spent on another commit, or claimed by another box
        assert!(vouched("node1", &pub1, &good, flake, &"b".repeat(40)).is_err());
        assert!(vouched("node2", &pub1, &good, flake, &rev).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
