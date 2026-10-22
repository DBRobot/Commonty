//! `dd release`: the fleet moves when this machine says so.
//!
//! `publish` takes the store paths CI built for one commit, checks each
//! against this machine's own evaluation of it, has two boxes vouch for
//! every closure, and pushes one signed file naming the commit, a counter
//! and every box's store path, nar hash and closure digest. Only when CI's
//! paths are missing does it build every box itself (on a box, with
//! DD_BUILD_STORE) and put the closures in the cache. The agent on each
//! box does the rest. CI never signs: the key stays on this machine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use auth::KeyStore as _;
use clap::Subcommand;
use release::ATTEST_NEED;

use crate::build_order;

/// keyring account holding the release key, base64
const ACCOUNT: &str = "release-key";
pub const DEFAULT_URL: &str = identity::fleet::RELEASES;
/// over the fleet's network: the owner's devices reach 3900 on the owner's boxes
const DEFAULT_CACHE: &str = identity::fleet::CACHE;

#[derive(Subcommand)]
pub enum ReleaseCmd {
    /// Make the release key here and write its public half to
    /// fleet/release.pub, which every box is built with.
    Init,
    /// Sign and publish the release of a ref: the paths CI built for it,
    /// checked against this machine's evaluation and vouched for by two
    /// boxes. Builds every box (and fills the cache) only when CI's paths
    /// are missing.
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
        /// check and sign, print the release, push nothing. No box is asked
        /// to vouch: the release printed is not vouched for
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
    /// Sign the app at a tag: two boxes build every download from that
    /// commit and vouch for the bytes (dd-attest), this machine fetches
    /// them from a box and checks them against what was vouched for,
    /// signs the apk with the Android key, puts the files on the forge's
    /// release of the tag, and publishes a manifest signed with the
    /// release key beside the box releases. The download page offers only
    /// what a signed manifest names.
    App {
        /// the release tag, e.g. v0.1.0
        #[arg(long)]
        tag: String,
        /// sign and print the manifest, upload and push nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Make the key Android apps are signed with, once, on this machine
    /// beside the release key. `app` signs the apk with it; a phone
    /// installs only a signed apk, and takes an update only from the same
    /// key, so it is made once and kept.
    AndroidInit,
    /// The next app, when its downloads would come out different from the
    /// last tag's: tag it on the forge and sign it as `app` does.
    /// `publish` starts this on its own, in the background; it never holds
    /// a release of the boxes up.
    AppNext {
        /// the branch the release came from
        #[arg(long, default_value = "main")]
        r#ref: String,
    },
    /// Whether the app at a branch still builds the same on two boxes: run
    /// nightly, it catches a change that makes the downloads differ from
    /// build to build long before a release would stop on it. Does nothing
    /// when nothing the app is made of changed since the last check passed.
    AppCheck {
        #[arg(long, default_value = "main")]
        r#ref: String,
        /// set up the nightly run on this machine (a user timer) and stop
        #[arg(long)]
        install: bool,
    },
    /// The release the boxes see.
    Show {
        #[arg(long, default_value = DEFAULT_URL)]
        url: String,
    },
    /// What every box says it runs, from each box's own metrics over the
    /// fleet's network, next to what the current release says it should.
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
            let (signed, check) =
                fetch(&repo_root(repo)?, &url)?.context("no release published yet")?;
            print(&signed, check.as_ref().err());
            if let Err(e) = check {
                bail!("the published release does not verify against fleet/release.pub: {e}");
            }
        }
        ReleaseCmd::Status { url } => status(repo, &url)?,
        ReleaseCmd::App { tag, dry_run } => app(repo, keys, &tag, dry_run)?,
        ReleaseCmd::AndroidInit => android_init(repo, keys)?,
        ReleaseCmd::AppNext { r#ref } => app_next(repo, keys, &r#ref)?,
        ReleaseCmd::AppCheck { r#ref, install } => {
            if install {
                app_check_install(repo)?
            } else {
                app_check(repo, keys, &r#ref)?
            }
        }
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

    // the member list at the commit released, every root on it under the
    // name it was confirmed as: a client anchors no root without one
    let has_list = !git(
        &root,
        &["ls-tree", "--name-only", &rev, "fleet/members.json"],
    )?
    .is_empty();
    let listed = match has_list {
        true => crate::member::parse(
            git(&root, &["show", &format!("{rev}:fleet/members.json")])?.as_bytes(),
        )?,
        false => crate::member::Members::default(),
    };
    named_all(&listed)?;

    let boxes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/boxes.json"))?)?;
    let mut names: Vec<String> = boxes
        .as_object()
        .context("boxes.json is not an object")?
        .keys()
        .cloned()
        .collect();
    // and the devices' systems (fleet/devices.json): signed beside the
    // boxes', vouched for by the same boxes
    names.extend(device_systems(&root)?);

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
    // Their signed answers go in the release: each box checks them itself
    // before it switches (dd-agent), so the release key alone moves no box.
    // DD_ATTEST=off is for bootstrap alone, and the release says so under
    // the key; a box that has taken an attested release refuses it.
    let mut attestations = Vec::new();
    let mut bootstrap = false;
    if dry_run {
        // a box's word is an ssh, a rebuild, minutes per box; a look at
        // what would be signed is not worth that
        eprintln!("== not vouched (dry run): no box was asked");
    } else if std::env::var("DD_ATTEST").as_deref() == Ok("off") {
        eprintln!(
            "== DD_ATTEST=off: no box was asked to vouch; the release is marked bootstrap, \
             and a box that has taken an attested release will refuse it"
        );
        bootstrap = true;
    } else {
        attestations = attest(&boxes, url, &rev, &built)?;
    }
    // Staging (docs/dev/rollout.md): what is never tried first on a real
    // box - a planned handover, a box joining with its TPM - on VMs on one
    // box, before anything is signed. DD_STAGE=off skips it, for a fleet
    // whose boxes have no KVM.
    if !dry_run && std::env::var("DD_STAGE").as_deref() != Ok("off") {
        stage(&boxes, url, &rev)?;
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
            let current = release::verify_json(&raw, &trusted_key(&root)?)
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
        eprintln!(
            "== the old form: no closure digest and no attestations, so the agents running now accept it"
        );
        for b in built.values_mut() {
            b.closure = None;
        }
        attestations.clear();
        bootstrap = false;
    }
    let signed = release::sign_as(
        release::Payload {
            counter,
            rev: rev.clone(),
            issued: release::now(),
            boxes: built,
            attestations,
            bootstrap,
        },
        &key,
        form,
    )?;
    let body = serde_json::to_string_pretty(&signed)?;
    // the member list, signed for clients: they hold other members' entries
    // to it, and the counter keeps an old one from coming back. A removal
    // only holds against a client built to refuse every list before it
    let published = url.replace("current.json", "members.json");
    let before = ureq_get(&published)
        .ok()
        .and_then(|raw| serde_json::from_str::<identity::SignedMemberList>(&raw).ok())
        .filter(|s| trusted_key(&root).is_ok_and(|k| identity::verify_member_list(s, &k).is_ok()))
        // the list the last release published, not an older one a forge
        // could serve beside it to hide who this release drops
        .filter(|s| s.list.counter + 1 == counter)
        .map(|s| s.list);
    if before.is_none() {
        eprintln!(
            "== {published} could not be read, is not signed by the release key, or is not the last release's: none was published yet, or the forge is not answering. What this release drops cannot be told, so it is taken to drop someone"
        );
    }
    let floor = floor_in(&git(
        &root,
        &["show", &format!("{rev}:client/identity/src/fleet.rs")],
    )?)?;
    floor_holds(&listed, before.as_ref(), floor, counter)?;
    let members = serde_json::to_string_pretty(&identity::sign_member_list(
        identity::MemberList {
            counter,
            members: listed.members,
            revoked: listed.revoked,
            names: listed.names,
        },
        &key,
    )?)?;
    if dry_run {
        println!("{body}");
        eprintln!("== dry run: not vouched, not pushed");
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
            ("members.json".into(), members),
        ],
        &format!("release {counter}: {}", &rev[..12]),
    )?;
    print(&signed, None);
    // the DNS Worker this release carries, with the names each box now
    // serves: up before the boxes are told, so a box that switches and asks
    // for a new name finds it there
    let dns = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(crate::dns_worker::deploy(&root, keys, r#ref))
    });
    if let Err(e) = dns {
        eprintln!(
            "== the DNS Worker did not go up: {e:#}\n   `dd dns deploy --ref {ref}` when it can"
        );
    }
    // the id Worker: sign-in, up before the boxes are told, so a box that
    // switches to it finds it there
    let id = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(crate::id_worker::deploy(&root, keys, r#ref))
    });
    if let Err(e) = id {
        eprintln!(
            "== the id Worker did not go up: {e:#}\n   `dd id deploy --ref {ref}` when it can"
        );
    }
    nudge(&boxes);
    // the control plane's relay this release carries
    let relayed = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(crate::control_worker::deploy(&root, keys, r#ref))
    });
    if let Err(e) = relayed {
        eprintln!(
            "== the control relay did not go up: {e:#}\n   `dd control deploy --ref {ref}` when it can"
        );
    }
    // the mail Worker this release carries (the gateway's closure holds the
    // same files, which the boxes vouched for and check Cloudflare against)
    let deployed = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(crate::mail_worker::deploy(&root, keys, r#ref))
    });
    if let Err(e) = deployed {
        eprintln!(
            "== the mail Worker did not go up: {e:#}\n   `dd mail deploy --ref {ref}` when it can"
        );
    }
    // the forge's page for it, with what it brings, by kind
    if let Err(e) = forge_release(&root, url, &rev, counter, keys) {
        eprintln!("== the forge's release page was not made: {e:#}");
    }
    app_in_background(&root, r#ref);
    Ok(())
}

/// The app follows the release on its own time: the boxes' builds of it
/// take the better part of an hour, and the release does not wait for them. A process
/// of its own, in a session of its own, so the end of this one (or of the
/// terminal that ran it) does not take it along - and holding nothing this
/// one had open.
fn app_in_background(root: &Path, r#ref: &str) {
    let log = dirs_cache().join("app-release.log");
    let started = (|| -> Result<()> {
        std::fs::create_dir_all(log.parent().context("no cache dir")?)?;
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(["release", "app-next", "--ref", r#ref])
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out);
        detach(&mut cmd);
        cmd.spawn()?;
        Ok(())
    })();
    match started {
        Ok(()) => eprintln!(
            "== the app: checked and, if it changed, built and signed in the background ({})",
            log.display()
        ),
        Err(e) => {
            eprintln!("== the app was not started: {e:#}\n   `dd release app-next` when it can")
        }
    }
}

/// A session of its own, and nothing of ours but its stdio: whatever else
/// is open here - the release lock a ship script holds, for one
/// (docs/dev/ci.md) - closes as the child starts. Release 196's app run kept
/// that lock for its whole wait on GitHub.
fn detach(cmd: &mut Command) {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        cmd.pre_exec(|| {
            libc::setsid();
            let max = match libc::sysconf(libc::_SC_OPEN_MAX) {
                n if n > 0 => n.min(65536) as i32,
                _ => 1024,
            };
            // marked, not closed: std's own pipe for reporting a failed
            // exec is among them, and it closes at exec the same way
            for fd in 3..max {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = cmd;
}

fn dirs_cache() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("dd")
}

/// The app's downloads, as the flake names them (legacyPackages.appRelease)
const APP_BUILDS: &[&str] = &[
    "linux",
    "windows",
    "dd-windows",
    "android",
    "home-controller",
    "box-installer",
];

/// Where each download of `version` lands when built from `rev`: what two
/// boxes are asked to build, and, at a fixed version, what says whether
/// anything the app is made of changed between two commits.
fn app_paths(root: &Path, rev: &str, version: &str) -> Result<BTreeMap<String, String>> {
    let names = APP_BUILDS
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let out = sh(
        "nix",
        &[
            "eval",
            "--json",
            &format!(
                "git+file://{}?rev={rev}#legacyPackages.x86_64-linux.appRelease",
                root.display()
            ),
            "--apply",
            &format!(
                "r: let a = r \"{version}\"; in builtins.listToAttrs (map (n: {{ name = n; value = a.${{n}}.outPath; }}) [ {names} ])"
            ),
        ],
    )?;
    Ok(serde_json::from_str(&out)?)
}

/// What the release at rev builds each box with (nix/lib/fleet-placement.nix
/// built, the flake's fleetBuilt): what a box can be asked to build by
fn fleet_built(root: &Path, rev: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let out = sh(
        "nix",
        &[
            "eval",
            "--json",
            &format!(
                "git+file://{}?rev={rev}#legacyPackages.x86_64-linux.fleetBuilt",
                root.display()
            ),
        ],
    )?;
    Ok(serde_json::from_str(&out)?)
}

/// What each download needs of the box that builds it (`passthru.needs`)
fn app_needs(root: &Path, rev: &str) -> Result<BTreeMap<String, build_order::Need>> {
    let names = APP_BUILDS
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let out = sh(
        "nix",
        &[
            "eval",
            "--json",
            &format!(
                "git+file://{}?rev={rev}#legacyPackages.x86_64-linux.appRelease",
                root.display()
            ),
            "--apply",
            &format!(
                "r: let a = r \"0.0.0\"; in builtins.listToAttrs (map (n: {{ name = n; value = a.${{n}}.needs or {{ }}; }}) [ {names} ])"
            ),
        ],
    )?;
    let v: BTreeMap<String, serde_json::Value> = serde_json::from_str(&out)?;
    Ok(v.iter()
        .map(|(k, n)| (k.clone(), build_order::Need::from_json(n)))
        .collect())
}

/// a command on a box of this machine's owner, over ssh: its output
fn on_box(addr: &str, cmd: &str) -> Result<String> {
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(format!("admin@{addr}"))
        .arg(cmd)
        .stdin(std::process::Stdio::null())
        .output()?;
    ensure!(
        out.status.success(),
        "on {addr}: {cmd}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?)
}

/// How far the move to v2 envelopes has got (docs/dev/identity-state.md):
/// the agreed identity state, read on a box of this machine's owner, as
/// (members still signed the old way, members on v2). Only for the person
/// whose box it is: it asks over ssh, as every release step does.
pub(crate) fn signatures(root: &Path, keys: &impl auth::KeyStore) -> Result<(Vec<String>, usize)> {
    let boxes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/boxes.json"))?)?;
    let owner = keys
        .get(crate::USER)?
        .context("no member name here (`dd login`)")?;
    let owners = box_owners(root, &boxes);
    let home = own_box(&boxes, &owners, &owner)?;
    let state: serde_json::Value =
        serde_json::from_str(&on_box(&home, "curl -sf http://127.0.0.1:4195/identity")?)?;
    Ok(count_signatures(&state))
}

/// the names whose agreed item is still v1, and how many are v2
pub(crate) fn count_signatures(state: &serde_json::Value) -> (Vec<String>, usize) {
    let mut old = Vec::new();
    let mut new = 0;
    for (name, m) in state["members"].as_object().into_iter().flatten() {
        // the item as agreed, verbatim: a v2 envelope says `"v": 2`
        let item = match &m["item"] {
            serde_json::Value::String(s) => serde_json::from_str(s).unwrap_or_default(),
            v => v.clone(),
        };
        if item.get("v").and_then(|v| v.as_u64()) == Some(2) {
            new += 1;
        } else {
            old.push(name.clone());
        }
    }
    old.sort();
    (old, new)
}

/// the same, asked again for a few minutes when it fails: a release
/// switching the box restarts what answers there for a few seconds, and a
/// wait of hours should not end on that
fn on_box_patient(addr: &str, cmd: &str) -> Result<String> {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        match on_box(addr, cmd) {
            Ok(out) => return Ok(out),
            Err(e) if std::time::Instant::now() < until => {
                eprintln!("   {e:#}; asking again in 10 s");
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Each box's owner, from its own file (fleet/boxes/<box>/box.json)
fn box_owners(root: &Path, boxes: &serde_json::Value) -> BTreeMap<String, String> {
    boxes
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, _)| {
            let f = std::fs::read(root.join("fleet/boxes").join(name).join("box.json")).ok()?;
            let v: serde_json::Value = serde_json::from_slice(&f).ok()?;
            Some((name.clone(), v["owner"].as_str()?.to_string()))
        })
        .collect()
}

/// One of the owner's own boxes, reachable from here: what the agreed log
/// is read through and the bytes are fetched from. Neither is trusted for
/// it - the results carry their builders' signatures, the bytes are checked
/// against the hash the builders agreed on.
fn own_box(
    boxes: &serde_json::Value,
    owners: &BTreeMap<String, String>,
    owner: &str,
) -> Result<String> {
    for (name, b) in boxes.as_object().into_iter().flatten() {
        if owners.get(name).map(String::as_str) == Some(owner)
            && let Some(t) = b["net"].as_str()
            // an address, and nothing ssh could take for an option
            && t.parse::<std::net::IpAddr>().is_ok()
            && on_box(t, "true").is_ok()
        {
            return Ok(t.to_string());
        }
    }
    bail!("none of {owner}'s boxes answers over ssh")
}

/// X.Y.Z, numbers only: the version goes into nix expressions and a box's
/// shell, so nothing else may get that far
fn version_of(tag: &str) -> Option<&str> {
    let v = tag.strip_prefix('v')?;
    let ok = v.split('.').count() == 3
        && v.split('.')
            .all(|n| !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()));
    ok.then_some(v)
}

/// v0.1.21 -> v0.1.22
fn next_tag(prev: &str) -> Option<String> {
    let v = version_of(prev)?;
    let (head, patch) = v.rsplit_once('.')?;
    Some(format!("v{head}.{}", patch.parse::<u64>().ok()? + 1))
}

/// the keystore's names for the Android signing key and its certificate,
/// both base64 DER (PKCS#8, X.509)
const ANDROID_KEY: &str = "android-key";
const ANDROID_CERT: &str = "android-cert";
/// the apk among the release's files: only ever the signed one
const SIGNED_APK: &str = "commonty.apk";

/// a tool from the flake's own nixpkgs, as every other this command runs
fn pinned(root: &Path, pkg: &str, bin: &str) -> Command {
    let mut c = Command::new("nix");
    c.current_dir(root).args([
        "shell",
        "--inputs-from",
        ".",
        &format!("nixpkgs#{pkg}"),
        "-c",
        bin,
    ]);
    c
}

fn run_ok(mut c: Command, what: &str) -> Result<()> {
    let out = c.output().with_context(|| format!("running {what}"))?;
    ensure!(
        out.status.success(),
        "{what}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

fn android_init(repo: &str, keys: &auth::Store) -> Result<()> {
    use base64::Engine as _;
    if keys.get(ANDROID_KEY)?.is_some() {
        bail!(
            "there is an Android key here already: a new one would leave every phone unable to update"
        );
    }
    let root = repo_root(repo)?;
    let dir = private_dir("dd-android")?;
    let made = (|| -> Result<(Vec<u8>, Vec<u8>)> {
        let pem = dir.join("key.pem");
        let mut c = pinned(&root, "openssl", "openssl");
        c.args([
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:4096",
            "-out",
        ])
        .arg(&pem);
        run_ok(c, "openssl genpkey")?;
        let mut c = pinned(&root, "openssl", "openssl");
        c.args(["pkcs8", "-topk8", "-nocrypt", "-outform", "DER", "-in"])
            .arg(&pem)
            .arg("-out")
            .arg(dir.join("key.der"));
        run_ok(c, "openssl pkcs8")?;
        // a hundred years: an app's signing certificate is not renewed,
        // only replaced, and a replaced one is a new app to every phone
        let mut c = pinned(&root, "openssl", "openssl");
        c.args([
            "req",
            "-new",
            "-x509",
            "-days",
            "36500",
            "-subj",
            "/CN=Commonty",
            "-outform",
            "DER",
            "-key",
        ])
        .arg(&pem)
        .arg("-out")
        .arg(dir.join("cert.der"));
        run_ok(c, "openssl req")?;
        Ok((
            std::fs::read(dir.join("key.der"))?,
            std::fs::read(dir.join("cert.der"))?,
        ))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    let (key, cert) = made?;
    let b64 = base64::engine::general_purpose::STANDARD;
    keys.set(ANDROID_KEY, &b64.encode(&key))?;
    keys.set(ANDROID_CERT, &b64.encode(&cert))?;
    use sha2::Digest as _;
    let fp = sha2::Sha256::digest(&cert)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    println!("Android key made, in this machine's keystore beside the release key");
    println!("certificate SHA-256: {fp}");
    println!("keep it as you keep the release key: without it, phones cannot update the app");
    Ok(())
}

fn android_key(keys: &auth::Store) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    match (keys.get(ANDROID_KEY)?, keys.get(ANDROID_CERT)?) {
        (Some(k), Some(c)) => Ok(Some((b64.decode(k.trim())?, b64.decode(c.trim())?))),
        _ => Ok(None),
    }
}

/// the attested apk, signed with the Android key (v2 and v3 schemes); the key
/// touches the disk only inside `dir`, which `app` removes
fn sign_apk(root: &Path, dir: &Path, apk: &Path, key: &[u8], cert: &[u8]) -> Result<PathBuf> {
    let k = dir.join("android.key.der");
    let c = dir.join("android.cert.der");
    for (path, bytes) in [(&k, key), (&c, cert)] {
        use std::io::Write as _;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        // readable by this user alone; on Windows the profile's own
        // permissions already say that
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        o.open(path)?.write_all(bytes)?;
    }
    let out = dir.join(SIGNED_APK);
    let mut s = pinned(root, "apksigner", "apksigner");
    s.args(["sign", "--key"])
        .arg(&k)
        .arg("--cert")
        .arg(&c)
        .arg("--out")
        .arg(&out)
        // apksigner aligns as it signs
        .arg(apk);
    let signed = run_ok(s, "apksigner sign");
    let _ = std::fs::remove_file(&k);
    signed?;
    let mut v = pinned(root, "apksigner", "apksigner");
    v.arg("verify").arg(&out);
    run_ok(v, "apksigner verify")?;
    Ok(out)
}

/// What each build holds, by the name the downloads page asks for
fn app_files(build: &str, version: &str) -> Vec<String> {
    match build {
        "linux" => vec![
            format!("commonty_{version}_amd64.deb"),
            format!("commonty_{version}_amd64.AppImage"),
        ],
        "windows" => vec!["commonty-setup.exe".into()],
        "dd-windows" => vec!["dd.exe".into()],
        "android" => vec!["commonty-unsigned.apk".into()],
        // the home controller's card, which the app's installer writes
        "home-controller" => vec![format!("commonty-home-controller-{version}.img.zst")],
        // the box installer's drive, which the app's installer window writes
        "box-installer" => vec![format!("commonty-box-installer-{version}.img.zst")],
        _ => vec![],
    }
}

/// the forge's release of `tag`, made if it is not there yet: its id
fn forge_app_release(api: &str, token: &str, tag: &str, rev: &str) -> Result<u64> {
    let id = |b: Vec<u8>| -> Result<u64> {
        serde_json::from_slice::<serde_json::Value>(&b)?["id"]
            .as_u64()
            .context("the forge's release has no id")
    };
    if let Some(b) = forge_get(&format!("{api}/releases/tags/{tag}"), token)? {
        return id(b);
    }
    let req = serde_json::json!({
        "tag_name": tag,
        "target_commitish": rev,
        "name": format!("Commonty {tag}"),
        "body": "The app, for every platform it is built for. What the downloads page offers is what the release key signed (app.json on the releases branch); these files are the same bytes.",
    });
    id(forge_call(
        "POST",
        &format!("{api}/releases"),
        token,
        Some(&req.to_string()),
    )?
    .context("the forge refused the release")?)
}

/// one file onto the forge's release, in place of one by that name
fn forge_upload(api: &str, token: &str, release: u64, file: &Path, name: &str) -> Result<()> {
    use std::io::Write as _;
    let assets = format!("{api}/releases/{release}/assets");
    if let Some(b) = forge_get(&assets, token)? {
        for a in serde_json::from_slice::<Vec<serde_json::Value>>(&b)? {
            if a["name"] == name
                && let Some(aid) = a["id"].as_u64()
            {
                forge_call("DELETE", &format!("{assets}/{aid}"), token, None)?
                    .with_context(|| format!("the forge kept the old {name}"))?;
            }
        }
    }
    let mut c = Command::new("curl")
        .args(["-sf", "-X", "POST", "-H", "@-", "-F"])
        .arg(format!("attachment=@{};filename={name}", file.display()))
        .arg(format!("{assets}?name={name}"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()?;
    c.stdin
        .take()
        .context("curl's stdin")?
        .write_all(format!("Authorization: Bearer {token}\n").as_bytes())?;
    ensure!(c.wait()?.success(), "the forge refused {name}");
    Ok(())
}

/// what app-next does with the newest tag
#[derive(Debug, PartialEq)]
enum NextStep {
    /// it is what the downloads page offers, and nothing changed since
    Nothing,
    /// nothing changed since it, but it was never signed: finish it
    Resume,
    /// the app changed: a new tag
    New,
    /// nothing is done, for this reason
    Refuse(String),
}

fn version_key(tag: &str) -> Option<[u64; 3]> {
    let mut n = version_of(tag)?.split('.').map(|n| n.parse().ok());
    Some([n.next()??, n.next()??, n.next()??])
}

/// The tag app.json offers, once the release key's signature on it checks
/// out; None only when there is no app.json. Anything that does not read
/// or verify is an error, and no step is taken on it.
fn signed_app_tag(raw: Option<&str>, key: &ed25519_dalek::VerifyingKey) -> Result<Option<String>> {
    let Some(raw) = raw else { return Ok(None) };
    let p = release::verify_doc(raw, "app", key)
        .map_err(|e| anyhow::anyhow!("app.json on the releases branch does not verify: {e}"))?;
    let tag = p["tag"]
        .as_str()
        .filter(|t| version_key(t).is_some())
        .context("app.json on the releases branch names no vX.Y.Z tag")?;
    Ok(Some(tag.into()))
}

/// the same, read from the forge's releases branch
fn read_signed_app_tag(root: &Path, key: &ed25519_dalek::VerifyingKey) -> Result<Option<String>> {
    // an empty answer is no branch; a failed one is not taken as that
    if git(root, &["ls-remote", "--heads", "forge", "releases"])?.is_empty() {
        return Ok(None);
    }
    git(root, &["fetch", "-q", "forge", "releases"])?;
    let releases = "refs/remotes/forge/releases";
    if git(root, &["ls-tree", "--name-only", releases, "app.json"])?.is_empty() {
        return Ok(None);
    }
    let raw = git(root, &["show", &format!("{releases}:app.json")])?;
    signed_app_tag(Some(&raw), key)
}

/// never an app older than the one signed: that would roll the downloads back
fn may_sign(tag: &str, signed: Option<&str>) -> Result<()> {
    let mine = version_key(tag).with_context(|| format!("{tag} is not vX.Y.Z"))?;
    if let Some(s) = signed {
        let theirs = version_key(s).with_context(|| format!("the signed app {s} is not vX.Y.Z"))?;
        ensure!(
            mine >= theirs,
            "{s} is signed already; {tag} is older and is not signed over it"
        );
    }
    Ok(())
}

/// the order on the releases branch, when the release key signed it for
/// exactly this run (build_order::resumable)
#[allow(clippy::too_many_arguments)]
fn resume_order(
    raw: &str,
    key: &ed25519_dalek::VerifyingKey,
    tag: &str,
    flake: &str,
    rev: &str,
    version: &str,
    want: &BTreeMap<String, String>,
    now: u64,
) -> Option<(String, Vec<build_order::Job>)> {
    let payload = match release::verify_doc(raw, "build-order", key) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("   the order on the releases branch does not verify ({e}); ordering anew");
            return None;
        }
    };
    build_order::resumable(&payload, tag, flake, rev, version, want, now)
}

/// `signed`: the tag app.json offers, verified; `prev`: the newest tag on
/// the ref; `on_ref`: prev's commit is in the ref's history and no tag is
/// newer
fn next_step(changed: bool, signed: Option<&str>, prev: &str, on_ref: bool) -> NextStep {
    let Some(p) = version_key(prev) else {
        return NextStep::Refuse(format!("{prev} is not vX.Y.Z"));
    };
    if !on_ref {
        return NextStep::Refuse(format!(
            "{prev} is not the newest tag, in the history being released"
        ));
    }
    let s = match signed.map(|s| (s, version_key(s))) {
        None => None,
        Some((s, Some(v))) => Some((s, v)),
        Some((s, None)) => return NextStep::Refuse(format!("the signed app {s} is not vX.Y.Z")),
    };
    let next = [p[0], p[1], p[2] + 1];
    match s {
        Some((s, v)) if changed && v >= next => {
            NextStep::Refuse(format!("{s} is signed already, newer than {prev}"))
        }
        _ if changed => NextStep::New,
        Some((s, _)) if s == prev => NextStep::Nothing,
        Some((s, v)) if v > p => NextStep::Refuse(format!(
            "{s} is signed already, newer than {prev}; an older app is never signed over it"
        )),
        _ => NextStep::Resume,
    }
}

fn app_next(repo: &str, keys: &auth::Store, r#ref: &str) -> Result<()> {
    let root = repo_root(repo)?;
    let stamp = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default()
    };
    git(&root, &["fetch", "-q", "--tags", "forge"])?;
    let rev = git(&root, &["rev-parse", &format!("refs/remotes/forge/{ref}")])?;
    let prev = git(
        &root,
        &["describe", "--tags", "--abbrev=0", "--match", "v*", &rev],
    )
    .context("no app tag before this one")?;
    let prev_rev = git(&root, &["rev-parse", &format!("{prev}^{{commit}}")])?;
    // the same version on both sides, so only what the app is made of can
    // differ; a tag from before the flake built the app always differs
    let now = app_paths(&root, &rev, "0.0.0")?;
    let changed = app_paths(&root, &prev_rev, "0.0.0").ok().as_ref() != Some(&now);
    // what the downloads page offers: the tag in app.json on the releases
    // branch, as the release key signed it. A tag pushed and never signed -
    // the laptop that was waiting on its builders went away - is still to
    // finish, not nothing to do. Unreadable or unverified: nothing is done
    let vk = load(keys)?.verifying_key();
    let offered = read_signed_app_tag(&root, &vk)
        .context("the app: nothing done, the signed app.json could not be read")?;
    // and the newest tag there is anywhere, not one passed by a later tag
    let newest = git(&root, &["tag", "-l", "v*"])?
        .lines()
        .filter_map(version_key)
        .max();
    let on_ref = git(&root, &["merge-base", "--is-ancestor", &prev_rev, &rev]).is_ok()
        && newest <= version_key(&prev);
    match next_step(changed, offered.as_deref(), &prev, on_ref) {
        NextStep::Nothing => {
            println!(
                "[{}] the app: nothing it is made of changed since {prev}",
                stamp()
            );
            return Ok(());
        }
        NextStep::Resume => {
            println!(
                "[{}] the app: {prev} was tagged and never signed; finishing it",
                stamp()
            );
            app(repo, keys, &prev, false)?;
            println!(
                "[{}] the app: {prev} is what the downloads page offers",
                stamp()
            );
            return Ok(());
        }
        NextStep::New => {}
        NextStep::Refuse(why) => bail!("the app: nothing done: {why}"),
    }
    let tag = next_tag(&prev).with_context(|| format!("{prev} is not vX.Y.Z"))?;
    println!(
        "[{}] the app: it changed since {prev}; {tag} at {}",
        stamp(),
        &rev[..12]
    );
    // a tag someone else made first is theirs to finish
    git(&root, &["tag", &tag, &rev])?;
    if let Err(e) = git(&root, &["push", "-q", "forge", &tag]) {
        let _ = git(&root, &["tag", "-d", &tag]);
        bail!("{tag} was not pushed (made elsewhere first?): {e:#}");
    }
    app(repo, keys, &tag, false)?;
    println!(
        "[{}] the app: {tag} is what the downloads page offers",
        stamp()
    );
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
            let net = b["net"].as_str().unwrap_or_default().to_string();
            let c = Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
                .arg(format!("admin@{net}"))
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

/// The staging VMs, on the first box (by name) that answers: its store
/// builds `checks.<system>.rollout-stage` at this commit. A box that cannot
/// be reached is passed over; a stage that fails stops the publish.
fn stage(boxes: &serde_json::Value, url: &str, rev: &str) -> Result<()> {
    let boxes = boxes.as_object().context("boxes.json is not an object")?;
    let base = url
        .split("/raw/")
        .next()
        .filter(|b| b.starts_with("https://"))
        .context("the release url is not a forge raw url")?;
    let check = format!("git+{base}?rev={rev}#checks.x86_64-linux.rollout-stage");
    for (name, b) in boxes {
        let net = b["net"].as_str().unwrap_or_default();
        eprintln!("== staging on {name}: {check}");
        let status = Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
            .args(["-o", "ServerAliveInterval=30"])
            .arg(format!("admin@{net}"))
            .args(["nix", "build", "--no-link", "-L", &check])
            .stdin(std::process::Stdio::null())
            .status()
            .context("ssh")?;
        match status.code() {
            Some(0) => {
                eprintln!("   staged on {name}");
                return Ok(());
            }
            // ssh itself: the box is not there; another may be
            Some(255) => eprintln!("   {name} did not answer"),
            _ => bail!("staging failed on {name}; nothing is signed (its log is above)"),
        }
    }
    bail!("no box answered to stage the release; DD_STAGE=off to publish without")
}

/// Ask every box, at once, to vouch for the release; each answer is
/// checked against the box's ssh host key in boxes.json, and every box of
/// the release needs ATTEST_NEED of them agreeing on its path and closure.
/// The answers that check out, to go in the release as they were signed.
fn attest(
    boxes: &serde_json::Value,
    url: &str,
    rev: &str,
    built: &BTreeMap<String, release::BoxRelease>,
) -> Result<Vec<release::Attestation>> {
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
        let net = b["net"].as_str().unwrap_or_default().to_string();
        let mut c = Command::new("ssh");
        c.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=30",
        ])
        .arg(format!("admin@{net}"))
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
    let mut signed = Vec::new();
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
            Ok((said, attestation)) => {
                signed.push(attestation);
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
    Ok(signed)
}

/// what one box vouched for: box name -> (path, closure)
type Said = BTreeMap<String, (String, String)>;

/// one box's signed answer, checked: what it says, and the answer as it
/// was signed
fn vouched(
    attester: &str,
    host_key: &str,
    answer: &str,
    flake: &str,
    rev: &str,
) -> Result<(Said, release::Attestation)> {
    let (st, attestation) = statement(attester, host_key, answer, flake, rev)?;
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
    Ok((out, attestation))
}

/// one box's answer, taken only if its host key signed it, in the attest
/// namespace, about this commit of this repository
fn statement(
    attester: &str,
    host_key: &str,
    answer: &str,
    flake: &str,
    rev: &str,
) -> Result<(serde_json::Value, release::Attestation)> {
    let v: serde_json::Value =
        serde_json::from_str(answer.lines().last().unwrap_or_default()).context("the answer")?;
    let statement = v["statement"].as_str().context("no statement")?;
    let signature = v["signature"].as_str().context("no signature")?;
    host_signed(attester, host_key, statement, signature)?;
    let st: serde_json::Value = serde_json::from_str(statement)?;
    ensure!(
        st["v"] == 1 && st["attester"] == attester && st["rev"] == rev && st["flake"] == flake,
        "the statement is about something else"
    );
    Ok((
        st,
        release::Attestation {
            attester: attester.to_string(),
            statement: statement.to_string(),
            signature: signature.to_string(),
        },
    ))
}

/// A new directory only this user can enter, at a name nobody else chose:
/// what is checked or signed in it cannot be swapped by another user of
/// the machine, as it could in a shared /tmp at a name known in advance
fn private_dir(prefix: &str) -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    private_dir_in(&base, prefix)
}

/// The same, on the disk: the runtime dir is a small RAM disk, and a
/// download can be a card image of gigabytes, held twice while unpacked
fn private_disk_dir(prefix: &str) -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&base)?;
    private_dir_in(&base, prefix)
}

fn private_dir_in(base: &Path, prefix: &str) -> Result<PathBuf> {
    use sha2::Digest as _;
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    for _ in 0..8 {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut seed = [0u8; 16];
        getrandom::fill(&mut seed).map_err(|e| anyhow::anyhow!("randomness: {e}"))?;
        let tag = sha2::Sha256::digest([&seed[..], &n.to_le_bytes()].concat());
        let dir = base.join(format!(
            "{prefix}-{:x}",
            u128::from_le_bytes(tag[..16].try_into()?)
        ));
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        match b.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    bail!("no fresh directory for {prefix}")
}

/// `statement` signed by `attester`'s ssh host key, in the attest namespace
/// (the same check every box makes of the release: release::ssh_signed)
fn host_signed(attester: &str, host_key: &str, statement: &str, signature: &str) -> Result<()> {
    let dir = private_dir("dd-attest")?;
    let checked = release::ssh_signed(
        &dir,
        &release::Attestation {
            attester: attester.to_string(),
            statement: statement.to_string(),
            signature: signature.to_string(),
        },
        host_key,
    );
    let _ = std::fs::remove_dir_all(&dir);
    checked.map_err(|_| anyhow::anyhow!("the signature is not {attester}'s host key"))
}

/// A file of an unpacked download, and only that: the download a real
/// directory, the file a plain file in it. A link would have this machine
/// read whatever it points at - a key, say - and publish it, and two boxes
/// agreeing on a link is no help.
fn plain_file(dir: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        !name.is_empty() && !name.contains('/') && name != "." && name != "..",
        "{name}: not a file name"
    );
    ensure!(
        std::fs::symlink_metadata(dir)?.file_type().is_dir(),
        "{} is not a directory",
        dir.display()
    );
    let file = dir.join(name);
    ensure!(
        std::fs::symlink_metadata(&file)
            .with_context(|| format!("no {name}"))?
            .file_type()
            .is_file(),
        "{name} is not a plain file"
    );
    Ok(file)
}

/// A vouched-for output from a box, as its NAR, checked against the hash
/// the boxes agreed on before a byte of it is unpacked into `dir`
fn fetch_vouched(addr: &str, path: &str, nar_hash: &str, dir: &Path) -> Result<()> {
    use sha2::Digest as _;
    let nar = dir.with_extension("nar");
    let mut c = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(format!("admin@{addr}"))
        .args(["nix-store", "--dump", path])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut from = c.stdout.take().context("ssh's stdout")?;
    // read back for the unpacking through this same handle, never by name
    let mut to = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&nar)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = std::io::Read::read(&mut from, &mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        std::io::Write::write_all(&mut to, &buf[..n])?;
    }
    ensure!(c.wait()?.success(), "{addr} did not hand over {path}");
    let got = format!("sha256:{:x}", h.finalize());
    let want = sh(
        "nix",
        &[
            "hash",
            "convert",
            "--hash-algo",
            "sha256",
            "--to",
            "base16",
            nar_hash,
        ],
    )?;
    ensure!(
        got == format!("sha256:{}", want.trim().trim_start_matches("sha256:")),
        "{path} from {addr} is not the bytes the boxes vouched for"
    );
    let st = Command::new("nix-store")
        .arg("--restore")
        .arg(dir)
        .stdin({
            std::io::Seek::rewind(&mut to)?;
            to
        })
        .status()?;
    let _ = std::fs::remove_file(&nar);
    ensure!(st.success(), "unpacking {path}");
    Ok(())
}

/// Each box answers for itself: its prometheus holds what its agent last
/// wrote. Nothing aggregates; a box that is off says nothing.
fn status(repo: &str, url: &str) -> Result<()> {
    let root = repo_root(repo)?;
    let boxes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/boxes.json"))?)?;
    let current = match fetch(&root, url)? {
        Some((s, Ok(()))) => {
            println!(
                "release {}  commit {}",
                s.payload.counter,
                &s.payload.rev[..12.min(s.payload.rev.len())]
            );
            Some(s)
        }
        Some((s, Err(e))) => {
            println!(
                "release {} as published DOES NOT VERIFY against fleet/release.pub: {e}\n\
                 the boxes refuse it; \"current\" and \"behind\" below are against it",
                s.payload.counter
            );
            Some(s)
        }
        None => {
            println!("no release published yet");
            None
        }
    };
    for (name, b) in boxes.as_object().context("boxes.json is not an object")? {
        let net = b["net"].as_str().unwrap_or("-");
        // a box's prometheus answers on its own loopback only; the owner
        // reaches it the way he reaches the box
        let q = |expr: &str| -> Option<serde_json::Value> {
            let out = Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
                .arg(format!("admin@{net}"))
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
    // the signing key in a file only for the length of the copy, in a
    // directory only this user can enter, at a name nobody chose in advance
    let key_dir = private_dir("dd-cache-key")?;
    let key_file = key_dir.join("key");
    {
        use std::io::Write as _;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
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
    let _ = std::fs::remove_dir_all(&key_dir);
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
    let Some(api) = forge_api(url) else {
        return Ok(None);
    };
    let tree = git(root, &["rev-parse", &format!("{rev}^{{tree}}")])?;
    let Some(token) = forge_token(keys)? else {
        return Ok(None);
    };
    for cand in [rev.to_owned(), format!("{rev}^2"), format!("{rev}^1")] {
        let Ok(sha) = git(root, &["rev-parse", "--verify", "--quiet", &cand]) else {
            continue;
        };
        if git(root, &["rev-parse", &format!("{sha}^{{tree}}")])? != tree {
            continue;
        }
        // every page: a run posts well over fifty statuses, and the paths
        // were once near enough the end of the first page to fall off it
        let mut statuses: Vec<serde_json::Value> = Vec::new();
        let mut read = true;
        for page in 1.. {
            let Some(body) = forge_get(
                &format!("{api}/commits/{sha}/statuses?limit=50&page={page}"),
                &token,
            )?
            else {
                read = false;
                break;
            };
            let got: Vec<serde_json::Value> = serde_json::from_slice(&body)?;
            let last = got.len() < 50;
            statuses.extend(got);
            if last {
                break;
            }
        }
        if !read {
            continue;
        }
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

/// https://forge/owner/repo/raw/... -> https://forge/api/v1/repos/owner/repo
fn forge_api(url: &str) -> Option<String> {
    let (repo_url, _) = url.split_once("/raw/")?;
    let slash = repo_url
        .find("://")
        .map(|i| i + 3)
        .and_then(|i| repo_url[i..].find('/').map(|j| i + j))?;
    Some(format!(
        "{}/api/v1/repos/{}",
        &repo_url[..slash],
        &repo_url[slash + 1..]
    ))
}

/// a ten-minute token for the forge, as this device; None without one
fn forge_token(keys: &auth::Store) -> Result<Option<String>> {
    let Some(kp) = auth::device::load(keys)? else {
        return Ok(None);
    };
    let Some(user) = keys.get(crate::USER)? else {
        return Ok(None);
    };
    Ok(Some(auth::device::mint(
        &kp,
        &user,
        std::time::Duration::from_secs(600),
    )?))
}

/// The forge's api: the answer's body, or None when it said no. The token
/// goes on curl's stdin, not its command line, which every process on this
/// machine can read while curl runs.
fn forge_call(method: &str, url: &str, token: &str, body: Option<&str>) -> Result<Option<Vec<u8>>> {
    use std::io::Write as _;
    let mut args = vec!["-sf", "-X", method, "-H", "@-"];
    if let Some(b) = body {
        args.extend(["-H", "content-type: application/json", "--data-binary", b]);
    }
    args.push(url);
    let mut c = Command::new("curl")
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    c.stdin
        .take()
        .context("curl's stdin")?
        .write_all(format!("Authorization: Bearer {token}\n").as_bytes())?;
    let out = c.wait_with_output()?;
    Ok(out.status.success().then_some(out.stdout))
}

fn forge_get(url: &str, token: &str) -> Result<Option<Vec<u8>>> {
    forge_call("GET", url, token, None)
}

/// The kinds a pull request's title starts with, in the order release notes
/// list them (.forgejo/checks/style.py holds titles to these)
const KINDS: &[(&str, &str)] = &[
    ("security", "Security"),
    ("fix", "Fixes"),
    ("feature", "Features"),
    ("service", "Services"),
    ("ci", "CI"),
    ("docs", "Docs"),
    ("refactor", "Refactors"),
    ("deps", "Dependencies"),
    ("chore", "Chores"),
];

/// A pull request's title as its kind and its words: `release: fix: x`
/// is ("fix", "x"); a title from before the kinds is ("", the title)
fn kind_of(title: &str) -> (&'static str, String) {
    let mut t = title.trim();
    for p in ["WIP: ", "release: "] {
        t = t.strip_prefix(p).unwrap_or(t);
    }
    for (k, _) in KINDS {
        if let Some(rest) = t.strip_prefix(&format!("{k}: ")) {
            return (k, rest.to_owned());
        }
    }
    ("", t.to_owned())
}

/// A release's name and notes from the pull requests it merged, newest
/// first as git lists them: (title, number)
fn release_notes(counter: u64, merged: &[(String, String)]) -> (String, String) {
    let all: Vec<(&str, String, &str)> = merged
        .iter()
        .map(|(t, n)| {
            let (k, words) = kind_of(t);
            (k, words, n.as_str())
        })
        .collect();
    let name = match all.as_slice() {
        [(_, words, _)] => format!("{counter} · {words}"),
        [] => format!("{counter}"),
        many => format!("{counter} · {} changes", many.len()),
    };
    let mut body = format!("Release {counter}, signed on the laptop and pulled by the boxes.\n");
    let heads = KINDS.iter().copied().chain([("", "Other")]);
    for (k, head) in heads {
        let these: Vec<_> = all.iter().filter(|(kk, _, _)| *kk == k).collect();
        if these.is_empty() {
            continue;
        }
        body.push_str(&format!("\n### {head}\n"));
        for (_, words, n) in these {
            body.push_str(&format!("- {words} (#{n})\n"));
        }
    }
    (name, body)
}

/// The forge's release page for this release, tagged r<counter> at the
/// commit, with notes from the pull requests merged since the last one
fn forge_release(
    root: &Path,
    url: &str,
    rev: &str,
    counter: u64,
    keys: &auth::Store,
) -> Result<()> {
    let api = forge_api(url).context("the release url is not the forge's")?;
    let token = forge_token(keys)?.context("no device key for the forge")?;
    let tag = format!("r{counter}");
    if forge_get(&format!("{api}/releases/tags/{tag}"), &token)?.is_some() {
        return Ok(());
    }
    // the release before this one, by number: some were made afterwards,
    // so the newest page is not the last release
    let since = forge_get(&format!("{api}/releases?limit=50"), &token)?
        .and_then(|b| serde_json::from_slice::<Vec<serde_json::Value>>(&b).ok())
        .and_then(|all| {
            all.iter()
                .filter_map(|r| {
                    r["tag_name"]
                        .as_str()?
                        .strip_prefix('r')?
                        .parse::<u64>()
                        .ok()
                })
                .filter(|n| *n < counter)
                .max()
        })
        .map(|n| format!("r{n}"))
        .and_then(|t| forge_get(&format!("{api}/tags/{t}"), &token).ok().flatten())
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v["commit"]["sha"].as_str().map(str::to_owned))
        .filter(|sha| git(root, &["merge-base", "--is-ancestor", sha, rev]).is_ok());
    let range = match &since {
        Some(sha) => format!("{sha}..{rev}"),
        None => format!("{rev}^..{rev}"),
    };
    let log = git(
        root,
        &["log", "--first-parent", "--merges", "--format=%s", &range],
    )?;
    let merged: Vec<(String, String)> = log
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("Merge pull request '")?;
            let (title, rest) = rest.rsplit_once("' (#")?;
            let (n, _) = rest.split_once(')')?;
            Some((title.to_owned(), n.to_owned()))
        })
        .collect();
    let (name, body) = release_notes(counter, &merged);
    let req = serde_json::json!({
        "tag_name": tag,
        "target_commitish": rev,
        "name": name,
        "body": body,
    });
    forge_call(
        "POST",
        &format!("{api}/releases"),
        &token,
        Some(&req.to_string()),
    )?
    .context("the forge refused the release")?;
    eprintln!("   the forge's release {tag}: {name}");
    Ok(())
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

/// `refused` is why the file did not verify; then the signer it names is
/// only what the file says, and is not printed as if it signed.
fn print(signed: &release::Signed, refused: Option<&String>) {
    let p = &signed.payload;
    let by = match refused {
        None => format!("signed by {}", identity::fingerprint(&signed.signer)),
        Some(e) => format!("DOES NOT VERIFY against fleet/release.pub ({e})"),
    };
    println!(
        "release {}  commit {}  {by}",
        p.counter,
        &p.rev[..12.min(p.rev.len())],
    );
    for (name, b) in &p.boxes {
        println!("  {name}  {}", b.path);
    }
    // what the file says; each box checks the signatures itself
    let by: Vec<&str> = p.attestations.iter().map(|a| a.attester.as_str()).collect();
    if p.bootstrap {
        println!("  bootstrap: published with no box asked to vouch");
    } else if !by.is_empty() {
        println!("  attested by {}", by.join(", "));
    }
}

/// The release key's public half, as every box is built with it.
fn trusted_key(root: &Path) -> Result<ed25519_dalek::VerifyingKey> {
    release::decode_public(std::fs::read_to_string(root.join("fleet/release.pub"))?.trim())
        .map_err(|e| anyhow::anyhow!("fleet/release.pub: {e}"))
}

/// The published release, and whether it verifies against
/// fleet/release.pub as a box would check it. One that does not is still
/// returned, to show what is there, with the reason.
fn fetch(
    root: &Path,
    url: &str,
) -> Result<Option<(release::Signed, std::result::Result<(), String>)>> {
    match ureq_get(url) {
        Ok(body) => Ok(Some(checked(&body, &trusted_key(root)?)?)),
        Err(NotFound) => Ok(None),
    }
}

fn checked(
    body: &str,
    trusted: &ed25519_dalek::VerifyingKey,
) -> Result<(release::Signed, std::result::Result<(), String>)> {
    match release::verify_json(body, trusted) {
        Ok(s) => Ok((s, Ok(()))),
        Err(e) => Ok((
            serde_json::from_str(body).context("release file is not json")?,
            Err(e.to_string()),
        )),
    }
}

struct NotFound;

/// every id on the member list has the name it was confirmed under
fn named_all(m: &crate::member::Members) -> Result<()> {
    let unnamed: Vec<&String> = m
        .members
        .iter()
        .filter(|id| !m.names.contains_key(*id))
        .collect();
    if !unnamed.is_empty() {
        bail!(
            "no name is listed for {}: `dd member add <their name> --confirm <id>` for each, commit, push, publish",
            unnamed
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

/// MEMBER_LIST_FLOOR as client/identity/src/fleet.rs has it
fn floor_in(fleet_rs: &str) -> Result<u64> {
    fleet_rs
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("pub const MEMBER_LIST_FLOOR: u64 = ")?
                .strip_suffix(';')
        })
        .context("client/identity/src/fleet.rs has no MEMBER_LIST_FLOOR")?
        .replace('_', "")
        .parse()
        .context("MEMBER_LIST_FLOOR is not a number")
}

/// The floor clients are built with, for the list this release signs: never
/// past it (they would take no list), and at it when this release drops a
/// root the last list let in (a removal, or a root a recovery superseded,
/// which a thief may hold) or revokes one it did not, so no client built
/// from here takes a list from before. With no last list to compare
/// (`before` None: unread or unsigned), every release is taken to drop one.
fn floor_holds(
    listed: &crate::member::Members,
    before: Option<&identity::MemberList>,
    floor: u64,
    counter: u64,
) -> Result<()> {
    ensure!(
        floor <= counter,
        "MEMBER_LIST_FLOOR is {floor}, past this release's {counter}: no client would take a list"
    );
    let dropped: Vec<&str> = match before {
        Some(b) => b
            .members
            .iter()
            .filter(|id| !listed.members.contains(id))
            .chain(listed.revoked.iter().filter(|id| !b.revoked.contains(id)))
            .map(|s| s.as_str())
            .collect(),
        None => vec!["(no last list to compare)"],
    };
    if !dropped.is_empty() && floor < counter {
        bail!(
            "this release drops or revokes {}: set MEMBER_LIST_FLOOR to {counter} in client/identity/src/fleet.rs, commit, push and publish again, so no client built from it takes an older list",
            dropped.join(", ")
        );
    }
    Ok(())
}

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
    // git takes an empty directory; a fresh private one, not a name anyone
    // could have put something at first
    let wt = private_dir("dd-release")?;
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
    let _ = std::fs::remove_dir_all(&wt);
    result
}

/// what the builders of an order agreed on, and who they were
struct Built {
    /// build -> (closure, nar hash)
    agreed: BTreeMap<String, (String, String)>,
    /// build -> (builders, how far apart)
    by: BTreeMap<String, (Vec<String>, u8)>,
}

/// The builders' answers to order `id`, read from the log from `since`:
/// what they agreed on, or None when someone stayed silent past
/// BUILD_WAIT_SECS (and is then in `exclude`, passed over next time)
#[allow(clippy::too_many_arguments)]
fn wait_built(
    boxes: &serde_json::Value,
    read: &dyn Fn(u64) -> Result<(Vec<serde_json::Value>, u64)>,
    id: &str,
    flake: &str,
    rev: &str,
    jobs: &[build_order::Job],
    mut since: u64,
    exclude: &mut Vec<String>,
) -> Result<Option<BTreeMap<String, (String, String)>>> {
    let verify = |attester: &str, statement: &str, signature: &str| -> Result<()> {
        let hk = boxes[attester]["hostKey"]
            .as_str()
            .with_context(|| format!("no hostKey for {attester}"))?;
        host_signed(attester, hk, statement, signature)
    };
    let mut said = BTreeMap::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(BUILD_WAIT_SECS);
    loop {
        let (items, next) = read(since)?;
        since = next;
        for item in &items {
            match build_order::read_item(item, id, flake, rev, jobs, &verify) {
                Ok(Some((job, from, s))) => {
                    eprintln!("   {job}: {from} answered");
                    said.insert((job, from), s);
                }
                Ok(None) => {}
                Err(e) => eprintln!("   a result refused: {e:#}"),
            }
        }
        match build_order::standing(jobs, &said)? {
            build_order::Standing::Agreed(agreed) => return Ok(Some(agreed)),
            build_order::Standing::Waiting(w) if std::time::Instant::now() >= deadline => {
                // whoever has not answered is passed over next time
                for line in &w {
                    if let Some((_, b)) = line.rsplit_once(" on ")
                        && !exclude.iter().any(|x| x == b)
                    {
                        exclude.push(b.to_string());
                    }
                }
                eprintln!("   no answer in time from: {}", w.join(", "));
                return Ok(None);
            }
            build_order::Standing::Waiting(_) => {
                std::thread::sleep(std::time::Duration::from_secs(120))
            }
        }
    }
}

/// The order on the releases branch, signed by this release key, when it
/// is the one for exactly these downloads of this tag at this commit
#[allow(clippy::too_many_arguments)]
fn standing_order(
    root: &Path,
    file: &str,
    key: &ed25519_dalek::SigningKey,
    tag: &str,
    flake: &str,
    rev: &str,
    version: &str,
    want: &BTreeMap<String, String>,
) -> Option<(String, Vec<build_order::Job>)> {
    git(root, &["fetch", "-q", "forge", "releases"]).ok()?;
    let raw = git(
        root,
        &["show", &format!("refs/remotes/forge/releases:{file}")],
    )
    .ok()?;
    resume_order(
        &raw,
        &key.verifying_key(),
        tag,
        flake,
        rev,
        version,
        want,
        release::now(),
    )
}

/// how long builders have to answer before the order goes to others
const BUILD_WAIT_SECS: u64 = 4 * 3600;

/// The app's builds, each by two boxes chosen apart, through a signed build
/// order and the agreed log; a builder that stays silent is passed over and
/// the order made again, once.
#[allow(clippy::too_many_arguments)]
fn order_builds(
    root: &Path,
    key: &ed25519_dalek::SigningKey,
    boxes: &serde_json::Value,
    owners: &BTreeMap<String, String>,
    home: &str,
    flake: &str,
    rev: &str,
    tag: &str,
    version: &str,
    want: &BTreeMap<String, String>,
    needs: &BTreeMap<String, build_order::Need>,
    // where the order goes on the releases branch: a release's, or a check's
    // (each box reads both; a check never takes the place of a release)
    file: &str,
) -> Result<Built> {
    let read = |since: u64| -> Result<(Vec<serde_json::Value>, u64)> {
        let v: serde_json::Value = serde_json::from_str(&on_box_patient(
            home,
            &format!("curl -sf http://127.0.0.1:4195/built?since={since}"),
        )?)?;
        Ok((
            v["items"].as_array().cloned().unwrap_or_default(),
            v["next"].as_u64().unwrap_or(since),
        ))
    };
    // what each box is built with, by the release being built
    let built_with = fleet_built(root, rev)?;
    let mut exclude: Vec<String> = Vec::new();
    for round in 1..=2 {
        let state: serde_json::Value = serde_json::from_str(&on_box_patient(
            home,
            "curl -sf http://127.0.0.1:4195/state",
        )?)?;
        let fleet = build_order::candidates(boxes, owners, &built_with, &state);
        // the order already out for these very downloads, waited on again
        let resumed = if round == 1 {
            standing_order(root, file, key, tag, flake, rev, version, want)
        } else {
            None
        };
        if let Some((id, jobs)) = resumed {
            let by = jobs
                .iter()
                .map(|j| {
                    let c = |n: &str| fleet.iter().find(|c| c.name == n);
                    let apart = match (j.builders.first(), j.builders.get(1)) {
                        (Some(a), Some(b)) => c(a)
                            .zip(c(b))
                            .map(|(a, b)| build_order::separation(a, b))
                            .unwrap_or(0),
                        _ => 0,
                    };
                    (j.name.clone(), (j.builders.clone(), apart))
                })
                .collect();
            eprintln!("== build order {id}: already out; reading what the builders said");
            match wait_built(boxes, &read, &id, flake, rev, &jobs, 0, &mut exclude)? {
                Some(agreed) => return Ok(Built { agreed, by }),
                None => continue,
            }
        }
        let mut jobs = Vec::new();
        let mut by = BTreeMap::new();
        for (name, path) in want {
            let need = needs.get(name).cloned().unwrap_or_default();
            let (builders, apart) =
                build_order::pick(path, &need, &fleet, build_order::BUILDERS, &exclude)?;
            eprintln!("   {name}: {} (apart {apart} of 3)", builders.join(", "));
            by.insert(name.clone(), (builders.clone(), apart));
            jobs.push(build_order::Job {
                name: name.clone(),
                attr: build_order::APP_ATTR.into(),
                version: Some(version.into()),
                part: Some(name.clone()),
                path: path.clone(),
                builders,
            });
        }
        let id = format!("{tag}-{round}-{}", release::now());
        // the log from here on: an answer to this order cannot come before it
        let (_, since) = read(0)?;
        let order = release::sign_doc(
            "build-order",
            build_order::order_payload(&id, flake, rev, &jobs),
            key,
        )?;
        to_releases(
            root,
            &[(file.into(), serde_json::to_string_pretty(&order)?)],
            &format!("build order {id}"),
        )?;
        eprintln!("== build order {id}: waiting for the builders");
        if let Some(agreed) = wait_built(boxes, &read, &id, flake, rev, &jobs, since, &mut exclude)?
        {
            return Ok(Built { agreed, by });
        }
    }
    bail!(
        "the builders did not answer; passed over: {}",
        exclude.join(", ")
    )
}

/// realise one download on a box, from the commit: built there or
/// substituted, whatever that box's own nix allows
fn realise_cmd(flake: &str, rev: &str, version: &str, build: &str) -> String {
    format!(
        "nix-store --realise \"$(nix eval --raw '{flake}?rev={rev}#legacyPackages.x86_64-linux.appRelease' --apply 'r: (r \"{version}\").{build}.drvPath')\" >/dev/null"
    )
}

/// what the last nightly check of the app found: the paths it checked, and
/// whether two boxes agreed on them
fn app_check_state() -> PathBuf {
    dirs_cache().join("app-check.json")
}

fn app_check(repo: &str, keys: &auth::Store, r#ref: &str) -> Result<()> {
    let root = repo_root(repo)?;
    let key = load(keys)?;
    // run on waking, the network can be a minute behind the timer
    let mut fetched = git(&root, &["fetch", "-q", "forge"]);
    for _ in 0..5 {
        if fetched.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(30));
        fetched = git(&root, &["fetch", "-q", "forge"]);
    }
    if let Err(e) = fetched {
        check_notice(
            "Commonty: the nightly app check did not run",
            &format!("the forge could not be reached: {e:#}"),
        );
        return Err(e.context("the forge could not be reached; nothing was checked"));
    }
    let rev = git(&root, &["rev-parse", &format!("refs/remotes/forge/{ref}")])?;
    // a real version's shape: Android refuses to package 0.0.0
    let version = CHECK_VERSION;
    let want = app_paths(&root, &rev, version)?;
    let last: Option<serde_json::Value> = std::fs::read(app_check_state())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if last
        .as_ref()
        .is_some_and(|l| l["ok"] == true && l["paths"] == serde_json::json!(want))
    {
        println!("the app: nothing it is made of changed since the last check passed");
        return Ok(());
    }
    let needs = app_needs(&root, &rev)?;
    let boxes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/boxes.json"))?)?;
    let repo_url = DEFAULT_URL
        .split("/raw/")
        .next()
        .context("the release url is not a forge raw url")?;
    let owner = keys
        .get(crate::USER)?
        .context("no member name here (`dd login`)")?;
    let owners = box_owners(&root, &boxes);
    let home = own_box(&boxes, &owners, &owner)?;
    eprintln!(
        "== the app at {} ({}): two boxes build it",
        &rev[..12],
        r#ref
    );
    let result = order_builds(
        &root,
        &key,
        &boxes,
        &owners,
        &home,
        &format!("git+{repo_url}"),
        &rev,
        "check",
        version,
        &want,
        &needs,
        "build-check.json",
    );
    let ok = result.is_ok();
    std::fs::create_dir_all(dirs_cache())?;
    std::fs::write(
        app_check_state(),
        serde_json::to_vec_pretty(&serde_json::json!({
            "rev": rev,
            "paths": want,
            "ok": ok,
            "at": release::now(),
            "why": result.as_ref().err().map(|e| format!("{e:#}")),
        }))?,
    )?;
    match result {
        Ok(_) => {
            println!(
                "the app at {}: built the same on two boxes; the next release will sign",
                &rev[..12]
            );
            Ok(())
        }
        Err(e) => {
            // said where it is seen: the desktop, and the timer's failure;
            // and said for what it is - two builds that differ is not the
            // same news as one that did not build
            let why = format!("{e:#}");
            let title = if why.contains("different bytes") {
                "Commonty: the app no longer builds the same on two boxes"
            } else if why.contains("could not build") {
                "Commonty: a box could not build the app"
            } else {
                "Commonty: the nightly app check failed"
            };
            check_notice(title, &why);
            Err(e.context(format!(
                "the app at {}: a release would stop here",
                &rev[..12]
            )))
        }
    }
}

/// what the nightly check builds as: any version with a real shape
const CHECK_VERSION: &str = "0.0.1";

/// the desktop's notice, where the check's news is seen
fn check_notice(title: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["--urgency=critical", title, body])
        .status();
}

/// The nightly run: a user timer on this machine, which holds the release
/// key the check order is signed with
fn app_check_install(repo: &str) -> Result<()> {
    let root = repo_root(repo)?;
    // the dd on PATH by the name it has there (a profile's link), so the
    // timer runs whatever dd this machine has then, not this build of it
    let dd = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("dd"))
        .find(|p| p.is_file())
        .map_or_else(std::env::current_exe, Ok)?;
    let home = std::env::var("HOME").context("HOME")?;
    let units = PathBuf::from(&home).join(".config/systemd/user");
    std::fs::create_dir_all(&units)?;
    std::fs::write(
        units.join("dd-app-check.service"),
        format!(
            "[Unit]\nDescription=Does the Commonty app still build the same on two boxes\n\n\
             [Service]\nType=oneshot\nWorkingDirectory={}\nExecStart={} release app-check\n",
            root.display(),
            dd.display()
        ),
    )?;
    std::fs::write(
        units.join("dd-app-check.timer"),
        "[Unit]\nDescription=Nightly: does the Commonty app still build the same twice\n\n\
         [Timer]\nOnCalendar=*-*-* 03:00\nPersistent=true\nRandomizedDelaySec=30m\n\n\
         [Install]\nWantedBy=timers.target\n",
    )?;
    run_ok(
        {
            let mut c = Command::new("systemctl");
            c.args(["--user", "daemon-reload"]);
            c
        },
        "systemctl --user daemon-reload",
    )?;
    run_ok(
        {
            let mut c = Command::new("systemctl");
            c.args(["--user", "enable", "--now", "dd-app-check.timer"]);
            c
        },
        "enabling the timer",
    )?;
    println!(
        "nightly at 03:00 (or on waking, if asleep then): `journalctl --user -u dd-app-check` for what it found"
    );
    Ok(())
}

fn app(repo: &str, keys: &auth::Store, tag: &str, dry_run: bool) -> Result<()> {
    use sha2::Digest as _;
    let root = repo_root(repo)?;
    let key = load(keys)?;
    let version = version_of(tag).with_context(|| format!("{tag} is not vX.Y.Z"))?;
    let rev = git(&root, &["rev-parse", &format!("{tag}^{{commit}}")])
        .with_context(|| format!("no tag {tag} here; fetch the forge's tags"))?;
    let boxes: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/boxes.json"))?)?;
    // what this commit's downloads must be, worked out here; the builders
    // must land on exactly these
    let want = app_paths(&root, &rev, version)?;
    let needs = app_needs(&root, &rev)?;
    let repo_url = DEFAULT_URL
        .split("/raw/")
        .next()
        .context("the release url is not a forge raw url")?;
    let flake = format!("git+{repo_url}");
    let owner = keys
        .get(crate::USER)?
        .context("no member name here (`dd login`)")?;
    let owners = box_owners(&root, &boxes);
    let home = own_box(&boxes, &owners, &owner)?;
    // never an app older than the one signed, by hand or not; nor on an
    // app.json that does not verify
    may_sign(
        tag,
        read_signed_app_tag(&root, &key.verifying_key())?.as_deref(),
    )?;
    let built = order_builds(
        &root,
        &key,
        &boxes,
        &owners,
        &home,
        &flake,
        &rev,
        tag,
        version,
        &want,
        &needs,
        "build-order.json",
    )?;
    // the domain the downloads are served from: the forge's, less git.
    let site = repo_url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .and_then(|h| h.strip_prefix("git."))
        .context("the forge is not at git.<domain>")?;
    let dir = private_disk_dir("dd-app")?;
    let result = (|| -> Result<(serde_json::Value, Vec<(PathBuf, String)>)> {
        let mut files = serde_json::Map::new();
        let mut upload = Vec::new();
        for (build, (_, nar)) in &built.agreed {
            let out = dir.join(build);
            let path = &want[build];
            eprintln!("== {build}: fetching it through {home}");
            // present on the owner's box however it gets there: the hash
            // the builders agreed on is what the bytes are held to
            on_box(&home, &realise_cmd(&flake, &rev, version, build))?;
            fetch_vouched(&home, path, nar, &out)?;
            for name in app_files(build, version) {
                let mut file = plain_file(&out, &name)
                    .with_context(|| format!("{build}: nothing is published"))?;
                let mut name = name;
                // The apk is built unsigned, and no phone installs that.
                // Signed here, with the Android key beside the release key;
                // without the key, left out, so the downloads page shows
                // Android as not ready rather than a file that cannot install.
                if build == "android" {
                    match android_key(keys)? {
                        Some((k, c)) => {
                            file = sign_apk(&root, &dir, &file, &k, &c)?;
                            name = SIGNED_APK.into();
                        }
                        None => {
                            eprintln!(
                                "   the apk: left out - no Android key here (`dd release android-init`), and a phone installs only a signed apk"
                            );
                            continue;
                        }
                    }
                }
                let bytes = std::fs::read(&file)?;
                let sha = format!("{:x}", sha2::Sha256::digest(&bytes));
                eprintln!("   {name}  {sha}");
                files.insert(
                    name.clone(),
                    serde_json::json!({
                        "sha256": sha,
                        "size": bytes.len(),
                        "url": format!("https://{site}/download/{tag}/{name}"),
                    }),
                );
                upload.push((file, name));
            }
        }
        Ok((
            serde_json::json!({
                "tag": tag,
                "commit": rev,
                "from": repo_url,
                "issued": release::now(),
                "files": files,
                // who built each, and how far apart they were (3: owner,
                // house and region all different)
                "built": built.by.iter().map(|(n, (who, apart))| {
                    (n.clone(), serde_json::json!({ "by": who, "apart": apart }))
                }).collect::<serde_json::Map<_, _>>(),
            }),
            upload,
        ))
    })();
    let published = result.and_then(|(manifest, upload)| {
        if dry_run {
            return Ok(manifest);
        }
        // the files first: a manifest naming files the forge does not have
        // would send people to nothing
        let api = forge_api(DEFAULT_URL).context("the release url is not the forge's")?;
        for (file, name) in &upload {
            // a token per file: ten minutes each, and the biggest is slow
            let token = forge_token(keys)?.context("no device key for the forge")?;
            let id = forge_app_release(&api, &token, tag, &rev)?;
            eprintln!("   uploading {name}");
            forge_upload(&api, &token, id, file, name)?;
        }
        Ok(manifest)
    });
    let _ = std::fs::remove_dir_all(&dir);
    let signed = release::sign_doc("app", published?, &key)?;
    let body = serde_json::to_string_pretty(&signed)?;
    if dry_run {
        println!("{body}");
        return Ok(());
    }
    // asked again: a newer one could have been signed while this one built
    may_sign(
        tag,
        read_signed_app_tag(&root, &key.verifying_key())?.as_deref(),
    )?;
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

pub(crate) fn repo_root(repo: &str) -> Result<PathBuf> {
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

/// The systems in fleet/devices.json: built and signed like a box's, run by
/// no box. Its "_" is the file's own note.
fn device_systems(root: &Path) -> Result<Vec<String>> {
    let path = root.join("fleet/devices.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let devices: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    Ok(devices
        .as_object()
        .context("devices.json is not an object")?
        .keys()
        .filter(|k| *k != "_")
        .cloned()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_still_on_v1_are_counted_by_their_agreed_item() {
        let state = serde_json::json!({"members": {
            "ada": {"item": {"v": 2, "kind": "entry", "body": "e30=", "sig": "x"}},
            "bob": {"item": {"entry": {"name": "bob"}, "signature": "x"}},
            "cy": {"item": "{\"v\":2,\"kind\":\"entry\"}"},
        }});
        let (old, new) = count_signatures(&state);
        assert_eq!(old, vec!["bob".to_string()]);
        assert_eq!(new, 2);
    }
    use release::ATTEST_NAMESPACE;

    #[test]
    fn a_tag_never_signed_is_finished_not_skipped() {
        // the app unchanged and the tag offered: nothing to do
        assert_eq!(
            next_step(false, Some("v0.1.49"), "v0.1.49", true),
            NextStep::Nothing
        );
        // the newest tag, on the ref, pushed but never signed: finish it
        assert_eq!(
            next_step(false, Some("v0.1.43"), "v0.1.49", true),
            NextStep::Resume
        );
        assert_eq!(
            next_step(false, Some("v0.1.9"), "v0.1.10", true),
            NextStep::Resume
        );
        assert_eq!(next_step(false, None, "v0.1.49", true), NextStep::Resume);
        // the app changed: a new tag, newer than what is signed
        assert_eq!(
            next_step(true, Some("v0.1.49"), "v0.1.49", true),
            NextStep::New
        );
        assert_eq!(
            next_step(true, Some("v0.1.43"), "v0.1.49", true),
            NextStep::New
        );
    }

    #[test]
    fn an_older_app_is_never_signed_over_a_newer_one() {
        let refused = |s: NextStep| matches!(s, NextStep::Refuse(_));
        // a newer app is signed than the newest tag here: no rollback
        assert!(refused(next_step(false, Some("v0.1.50"), "v0.1.49", true)));
        assert!(refused(next_step(false, Some("v0.1.10"), "v0.1.9", true)));
        // a new tag would be v0.1.50, and v0.1.50 or later is signed
        assert!(refused(next_step(true, Some("v0.1.50"), "v0.1.49", true)));
        assert!(refused(next_step(true, Some("v0.2.0"), "v0.1.49", true)));
        // a tag that is not on the ref, or not vX.Y.Z
        assert!(refused(next_step(false, Some("v0.1.43"), "v0.1.49", false)));
        assert!(refused(next_step(false, Some("v0.1.43"), "latest", true)));
        // the last word before anything is signed, by hand too
        assert!(may_sign("v0.1.48", Some("v0.1.49")).is_err());
        assert!(may_sign("v0.1.9", Some("v0.1.10")).is_err());
        assert!(may_sign("v0.1.49", Some("v0.1.49")).is_ok());
        assert!(may_sign("v0.1.50", Some("v0.1.49")).is_ok());
        assert!(may_sign("v0.1.50", None).is_ok());
        assert!(may_sign("v0.1.50", Some("junk")).is_err());
    }

    fn test_key(seed: u8) -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn an_app_json_that_does_not_verify_takes_no_step() {
        let key = test_key(7);
        let vk = key.verifying_key();
        let good = |tag: &str, k: &ed25519_dalek::SigningKey| {
            let d = release::sign_doc("app", serde_json::json!({ "tag": tag }), k).unwrap();
            serde_json::to_string(&d).unwrap()
        };
        assert_eq!(
            signed_app_tag(Some(&good("v0.1.49", &key)), &vk).unwrap(),
            Some("v0.1.49".into())
        );
        // nothing ever signed: no app.json
        assert_eq!(signed_app_tag(None, &vk).unwrap(), None);
        // unreadable, signed by another key, changed after signing, unsigned,
        // another kind, or no vX.Y.Z tag: an error, and no step is taken
        assert!(signed_app_tag(Some("{not json"), &vk).is_err());
        assert!(signed_app_tag(Some(&good("v0.1.49", &test_key(8))), &vk).is_err());
        let tampered = good("v0.1.49", &key).replace("v0.1.49", "v0.1.40");
        assert!(signed_app_tag(Some(&tampered), &vk).is_err());
        assert!(signed_app_tag(Some(r#"{"payload":{"tag":"v0.1.49"}}"#), &vk).is_err());
        let order = release::sign_doc("build-order", serde_json::json!({ "tag": "v0.1.49" }), &key);
        assert!(
            signed_app_tag(Some(&serde_json::to_string(&order.unwrap()).unwrap()), &vk).is_err()
        );
        assert!(signed_app_tag(Some(&good("latest", &key)), &vk).is_err());
    }

    #[test]
    fn an_order_is_waited_on_again_only_when_signed_for_this_very_run() {
        let key = test_key(7);
        let vk = key.verifying_key();
        let want: BTreeMap<String, String> = [("linux".to_string(), "/nix/store/a".to_string())]
            .into_iter()
            .collect();
        let now = 1_000_000;
        let job = |path: &str| build_order::Job {
            name: "linux".into(),
            attr: build_order::APP_ATTR.into(),
            version: Some("0.1.49".into()),
            part: Some("linux".into()),
            path: path.into(),
            builders: vec!["node1".into(), "node2".into()],
        };
        let signed = |k: &ed25519_dalek::SigningKey, rev: &str, path: &str, issued: u64| {
            let mut p =
                build_order::order_payload("v0.1.49-1-5", "git+https://x", rev, &[job(path)]);
            p["issued"] = issued.into();
            serde_json::to_string(&release::sign_doc("build-order", p, k).unwrap()).unwrap()
        };
        let resume = |raw: &str| {
            resume_order(
                raw,
                &vk,
                "v0.1.49",
                "git+https://x",
                "r1",
                "0.1.49",
                &want,
                now,
            )
        };
        let (id, jobs) = resume(&signed(&key, "r1", "/nix/store/a", now - 60)).unwrap();
        assert_eq!(id, "v0.1.49-1-5");
        assert_eq!(jobs, vec![job("/nix/store/a")]);
        // forged or unsigned: ordered anew
        assert!(resume(&signed(&test_key(8), "r1", "/nix/store/a", now - 60)).is_none());
        let tampered =
            signed(&key, "r1", "/nix/store/a", now - 60).replace("/nix/store/a", "/nix/store/z");
        assert!(resume(&tampered).is_none());
        let bare = serde_json::json!({ "payload": build_order::order_payload("v0.1.49-1-5", "git+https://x", "r1", &[job("/nix/store/a")]) });
        assert!(resume(&bare.to_string()).is_none());
        assert!(resume("{not json").is_none());
        // another commit, other downloads: ordered anew
        assert!(resume(&signed(&key, "r2", "/nix/store/a", now - 60)).is_none());
        assert!(resume(&signed(&key, "r1", "/nix/store/b", now - 60)).is_none());
        // too old, or from the future
        assert!(
            resume(&signed(
                &key,
                "r1",
                "/nix/store/a",
                now - build_order::ORDER_MAX_AGE_SECS - 1
            ))
            .is_none()
        );
        assert!(resume(&signed(&key, "r1", "/nix/store/a", now + 3600)).is_none());
    }

    #[test]
    fn a_member_list_is_published_named_and_with_its_floor() {
        let mut m = crate::member::Members {
            members: vec!["a".into()],
            ..Default::default()
        };
        assert!(named_all(&m).is_err());
        m.names.insert("a".into(), identity::name_tag("a", "amy"));
        named_all(&m).unwrap();

        assert_eq!(
            floor_in("/// the floor\npub const MEMBER_LIST_FLOOR: u64 = 0;\n").unwrap(),
            0
        );
        assert_eq!(
            floor_in("pub const MEMBER_LIST_FLOOR: u64 = 1_204;").unwrap(),
            1204
        );
        let r = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let now = |members: &[&str], revoked: &[&str]| crate::member::Members {
            members: r(members),
            revoked: r(revoked),
            ..Default::default()
        };
        let was = |members: &[&str], revoked: &[&str]| identity::MemberList {
            members: r(members),
            revoked: r(revoked),
            ..Default::default()
        };
        // nothing dropped: any floor up to the release
        floor_holds(&now(&["a", "b"], &["x"]), Some(&was(&["a"], &["x"])), 0, 9).unwrap();
        // a removal wants the floor at this release, and no further
        let removed = now(&["a"], &["x", "y"]);
        assert!(floor_holds(&removed, Some(&was(&["a"], &["x"])), 8, 9).is_err());
        floor_holds(&removed, Some(&was(&["a"], &["x"])), 9, 9).unwrap();
        // so does a root a recovery superseded, off the list, not revoked
        let recovered = now(&["a2"], &[]);
        let e = floor_holds(&recovered, Some(&was(&["a"], &[])), 8, 9).unwrap_err();
        assert!(e.to_string().contains("drops or revokes a"), "{e}");
        floor_holds(&recovered, Some(&was(&["a"], &[])), 9, 9).unwrap();
        // and no last list to compare: taken as a drop
        assert!(floor_holds(&now(&["a"], &[]), None, 8, 9).is_err());
        floor_holds(&now(&["a"], &[]), None, 9, 9).unwrap();
        assert!(floor_holds(&now(&[], &[]), None, 10, 9).is_err());
    }

    // the lock a ship script holds is an fd it opened without close-on-exec;
    // the app's background run must not carry it off
    #[cfg(target_os = "linux")]
    #[test]
    fn the_app_run_holds_nothing_it_was_handed() {
        use std::os::fd::AsRawFd as _;
        let dir = std::env::temp_dir().join(format!("dd-detach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let held = std::fs::File::create(dir.join("lock")).unwrap();
        let fd = held.as_raw_fd();
        unsafe {
            let f = libc::fcntl(fd, libc::F_GETFD);
            libc::fcntl(fd, libc::F_SETFD, f & !libc::FD_CLOEXEC);
        }
        let probe = format!("test -e /proc/self/fd/{fd} && echo held || echo free");
        let plain = Command::new("sh").args(["-c", &probe]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&plain.stdout).trim(),
            "held",
            "the probe sees a handed fd"
        );
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &probe]);
        detach(&mut cmd);
        let out = cmd.output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "free");
        drop(held);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // the throwaway worktree is a fresh private directory, and is gone after
    #[test]
    fn the_releases_branch_is_written_from_a_private_worktree() {
        let t = private_dir("dd-test-releases").unwrap();
        let (forge, root) = (t.join("forge.git"), t.join("repo"));
        git(&t, &["init", "-q", "--bare", forge.to_str().unwrap()]).unwrap();
        git(&t, &["init", "-q", root.to_str().unwrap()]).unwrap();
        git(&root, &["remote", "add", "forge", forge.to_str().unwrap()]).unwrap();
        // the worktree shares this config: who commits, and no signing
        for (k, v) in [
            ("user.name", "t"),
            ("user.email", "t@t"),
            ("commit.gpgsign", "false"),
        ] {
            git(&root, &["config", k, v]).unwrap();
        }
        let before = |base: &Path| {
            std::fs::read_dir(base)
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .filter(|e| e.file_name().to_string_lossy().starts_with("dd-release-"))
                        .count()
                })
                .unwrap_or(0)
        };
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let left = before(&base);
        // the first one starts the branch, the second adds to it
        to_releases(&root, &[("current.json".into(), "1".into())], "release 1").unwrap();
        to_releases(&root, &[("current.json".into(), "2".into())], "release 2").unwrap();
        assert_eq!(
            git(&forge, &["show", "releases:current.json"]).unwrap(),
            "2"
        );
        assert_eq!(
            git(&forge, &["rev-list", "--count", "releases"]).unwrap(),
            "2"
        );
        assert_eq!(before(&base), left, "no worktree left behind");
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn show_and_status_say_when_the_release_does_not_verify() {
        let key = release::generate();
        let payload = release::Payload {
            counter: 7,
            rev: "0123456789abcdef".into(),
            issued: 1,
            boxes: BTreeMap::new(),
            attestations: vec![],
            bootstrap: false,
        };
        let body = serde_json::to_string(&release::sign(payload, &key).unwrap()).unwrap();
        let (s, check) = checked(&body, &key.verifying_key()).unwrap();
        assert_eq!(s.payload.counter, 7);
        assert!(check.is_ok());
        // the counter changed after signing
        let forged = body.replace("\"counter\":7", "\"counter\":99");
        assert_ne!(forged, body);
        let (s, check) = checked(&forged, &key.verifying_key()).unwrap();
        assert_eq!(s.payload.counter, 99, "shown as it is");
        assert!(check.is_err());
        // signed, but by a key that is not the release key
        let (_, check) = checked(&body, &release::generate().verifying_key()).unwrap();
        assert!(check.is_err());
        assert!(checked("not json", &key.verifying_key()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn secrets_and_checks_go_where_no_one_else_can_reach() {
        use std::os::unix::fs::PermissionsExt as _;
        let a = private_dir("dd-test").unwrap();
        let b = private_dir("dd-test").unwrap();
        assert_ne!(a, b, "a fresh name every time");
        let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "this user's alone");
        std::fs::remove_dir(&a).unwrap();
        std::fs::remove_dir(&b).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_download_is_published_only_as_plain_files() {
        let t = std::env::temp_dir().join(format!("dd-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let out = t.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(t.join("secret"), "key").unwrap();
        std::fs::write(out.join("dd.exe"), "MZ").unwrap();
        assert!(plain_file(&out, "dd.exe").is_ok());
        // a link to anything on this machine is refused
        std::os::unix::fs::symlink(t.join("secret"), out.join("commonty.apk")).unwrap();
        assert!(plain_file(&out, "commonty.apk").is_err());
        // and so is a download that is itself a link to a directory
        std::os::unix::fs::symlink(&out, t.join("linked")).unwrap();
        assert!(plain_file(&t.join("linked"), "dd.exe").is_err());
        assert!(plain_file(&out, "../secret").is_err());
        assert!(plain_file(&out, "missing").is_err());
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn the_app_is_named_as_the_downloads_page_asks() {
        assert_eq!(next_tag("v0.1.21").as_deref(), Some("v0.1.22"));
        assert_eq!(next_tag("v1.2.9").as_deref(), Some("v1.2.10"));
        assert_eq!(next_tag("0.1.2"), None);
        // a tag carries nothing into a box's shell or a nix expression
        for bad in [
            "v1.2.3;id",
            "v1;id.2.3",
            "v1.2",
            "v1.2.3.4",
            "v1..3",
            "v1.2.x",
            "v1.2.3 ",
        ] {
            assert_eq!(version_of(bad), None, "{bad}");
            assert_eq!(next_tag(bad), None, "{bad}");
        }
        assert_eq!(version_of("v10.0.7"), Some("10.0.7"));
        assert_eq!(app_files("linux", "0.1.22")[0], "commonty_0.1.22_amd64.deb");
        assert_eq!(app_files("windows", "0.1.22"), ["commonty-setup.exe"]);
        // every build has files; a name the flake does not make has none
        assert!(APP_BUILDS.iter().all(|b| !app_files(b, "1.0.0").is_empty()));
        assert!(app_files("macos", "1.0.0").is_empty());
    }

    #[test]
    fn release_notes_group_pull_requests_by_kind() {
        assert_eq!(
            kind_of("release: fix: awk on the path"),
            ("fix", "awk on the path".into())
        );
        assert_eq!(
            kind_of("WIP: release: ci: one gate"),
            ("ci", "one gate".into())
        );
        assert_eq!(kind_of("Boxes as data"), ("", "Boxes as data".into()));
        let (name, body) = release_notes(
            186,
            &[
                ("release: feature: docs at /docs".into(), "217".into()),
                ("release: fix: awk on the path".into(), "220".into()),
                ("Boxes as data".into(), "213".into()),
            ],
        );
        assert_eq!(name, "186 · 3 changes");
        let fixes = body.find("### Fixes").unwrap();
        let features = body.find("### Features").unwrap();
        let other = body.find("### Other").unwrap();
        assert!(fixes < features && features < other, "{body}");
        assert!(body.contains("- docs at /docs (#217)"), "{body}");
        let (one, _) = release_notes(187, &[("release: docs: the ci page".into(), "230".into())]);
        assert_eq!(one, "187 · the ci page");
    }

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

        let (said, attestation) = vouched(
            "node1",
            &pub1,
            &answer(&statement, ATTEST_NAMESPACE),
            flake,
            &rev,
        )
        .unwrap();
        assert_eq!(said["node1"], ("/nix/store/x-node1".into(), "c1".into()));
        // what goes in the release is the answer as signed, and a box checking
        // it against the same host key takes it as node1's word on node1
        assert_eq!(attestation.statement, statement);
        let mut boxes = BTreeMap::new();
        boxes.insert(
            "node1".to_string(),
            release::BoxRelease {
                path: "/nix/store/x-node1".into(),
                nar_hash: "sha256-x".into(),
                closure: Some("c1".into()),
            },
        );
        let payload = release::Payload {
            counter: 1,
            rev: rev.clone(),
            issued: 1,
            boxes,
            attestations: vec![attestation],
            bootstrap: false,
        };
        let fleet: BTreeMap<String, String> = [("node1".to_string(), pub1.clone())].into();
        let work = private_dir("dd-vouch-check").unwrap();
        let (by, not) = release::vouchers(&payload, "node1", &fleet, |a, k| {
            release::ssh_signed(&work, a, k).is_ok()
        });
        assert!(by.contains("node1") && not.is_empty(), "{not:?}");
        let _ = std::fs::remove_dir_all(&work);

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
