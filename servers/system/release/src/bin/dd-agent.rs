//! The agent: a box moves itself to the release, and to nothing else.
//!
//! Runs from a timer. Fetches the release file, checks it is signed by the
//! release key this box was installed with, refuses a counter lower than
//! the last one it applied, looks up its own entry, fetches that closure,
//! checks the nar hash and that two boxes of the fleet vouched for that
//! closure, switches, and proves the box is still a box: no
//! failed units, sshd listening, the verifier answering. If that fails
//! within the window the previous system comes back. Nothing here accepts
//! a push; the only input is a signed file and the only authority is the
//! key that signed it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Parser)]
struct Args {
    /// where the signed release file is
    #[arg(long, env = "DD_AGENT_URL")]
    url: String,
    /// file holding the release key's public half, base64
    #[arg(long, env = "DD_AGENT_KEY_FILE")]
    key_file: PathBuf,
    /// this box's name in the release; defaults to the hostname
    #[arg(long, env = "DD_AGENT_BOX")]
    r#box: Option<String>,
    /// a binary cache to fetch the closure from; none means it must
    /// already be in the store (the vm test shares one)
    #[arg(long, env = "DD_AGENT_CACHE")]
    cache: Option<String>,
    /// the last applied counter and the previous system live here
    #[arg(long, env = "DD_AGENT_STATE", default_value = "/var/lib/dd-agent")]
    state: PathBuf,
    /// where the node exporter reads textfile metrics
    #[arg(long, env = "DD_AGENT_FACTS", default_value = "/var/lib/dd-facts")]
    facts: PathBuf,
    /// how long after the switch the probe may keep failing before rollback
    #[arg(long, env = "DD_AGENT_PROBE_SECS", default_value = "600")]
    probe_secs: u64,
    /// the verifier's port, probed after a switch
    #[arg(long, env = "DD_AGENT_VERIFY_PORT", default_value = "4181")]
    verify_port: u16,
    /// whether sshd must be up after a switch: a box's is its way in; a home
    /// controller runs none
    #[arg(long, env = "DD_AGENT_SSH", default_value_t = true, action = clap::ArgAction::Set)]
    ssh: bool,
    /// host:port of the other boxes, whitespace separated. A release that
    /// takes away the reach this box had before it is rolled back: the
    /// switch may not strand the box, however healthy it looks from here.
    #[arg(long, env = "DD_AGENT_REACH", default_value = "")]
    reach: String,
    /// json file of the fleet's boxes, name -> ssh host key, as this box's
    /// system was built with them (fleet/boxes.json): whose word on a
    /// closure counts. None, or fewer than two, is a fleet that cannot vouch.
    #[arg(long, env = "DD_AGENT_ATTESTERS")]
    attesters: Option<PathBuf>,
}

/// Why this box may switch to a closure on the boxes' word, or may without it.
#[derive(Debug, PartialEq, Eq)]
enum Vouched {
    /// these boxes of the fleet vouched for exactly this closure
    By(Vec<String>),
    /// the fleet this box was built with has fewer boxes than must vouch
    FleetTooSmall,
    /// signed as published with no box asked, on a box that has never
    /// taken an attested release
    Bootstrap,
    /// a release from before attestations, on a box that has never taken one
    Before,
}

/// The rule. Two boxes of this box's fleet vouch for its closure, or it is
/// refused - except on a box that has never taken an attested release
/// (`since` is None), which still takes one from before them or one the
/// release key marks as bootstrap; once it has, neither will do again. A
/// release that carries attestations is held to them from the start.
fn judge(
    fleet: usize,
    by: &std::collections::BTreeSet<String>,
    carries: bool,
    bootstrap: bool,
    since: Option<u64>,
) -> std::result::Result<Vouched, String> {
    if fleet < release::ATTEST_NEED {
        return Ok(Vouched::FleetTooSmall);
    }
    if by.len() >= release::ATTEST_NEED {
        return Ok(Vouched::By(by.iter().cloned().collect()));
    }
    let short = format!(
        "{} of the {} boxes needed vouch for it{}",
        by.len(),
        release::ATTEST_NEED,
        if by.is_empty() {
            String::new()
        } else {
            format!(" ({})", by.iter().cloned().collect::<Vec<_>>().join(", "))
        }
    );
    match since {
        Some(n) => Err(format!(
            "{short}; this box has required that since release {n}"
        )),
        None if carries => Err(short),
        None if bootstrap => Ok(Vouched::Bootstrap),
        None => Ok(Vouched::Before),
    }
}

/// The fleet's boxes and their host keys, as this system was built with them.
fn read_attesters(path: Option<&Path>) -> Result<std::collections::BTreeMap<String, String>> {
    let Some(path) = path else {
        return Ok(Default::default());
    };
    let raw = fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("{}", path.display()))
}

/// The first release this box took on the boxes' word; from then on it
/// takes no other kind.
fn read_since(state: &Path) -> Option<u64> {
    fs::read_to_string(state.join("attested"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Who vouches for this box's closure, each attestation's signature
/// checked by ssh-keygen in a directory of the agent's own.
fn check_vouchers(
    args: &Args,
    payload: &release::Payload,
    name: &str,
    fleet: &std::collections::BTreeMap<String, String>,
) -> (std::collections::BTreeSet<String>, Vec<String>) {
    let dir = args.state.join("attest");
    let _ = fs::remove_dir_all(&dir);
    let made = {
        let mut b = fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&dir)
    };
    let out = release::vouchers(payload, name, fleet, |a, key| {
        made.is_ok() && release::ssh_signed(&dir, a, key).is_ok()
    });
    let _ = fs::remove_dir_all(&dir);
    out
}

/// What the boxes' word says about this release for this box, and the
/// state written once it is an attested one.
fn vouch(
    args: &Args,
    payload: &release::Payload,
    name: &str,
) -> Result<std::result::Result<Vouched, String>> {
    let fleet = read_attesters(args.attesters.as_deref())?;
    let (by, not) = if fleet.len() >= release::ATTEST_NEED {
        check_vouchers(args, payload, name, &fleet)
    } else {
        Default::default()
    };
    for n in &not {
        eprintln!("attestation: {n}");
    }
    let since = read_since(&args.state);
    let verdict = judge(
        fleet.len(),
        &by,
        !payload.attestations.is_empty(),
        payload.bootstrap,
        since,
    );
    if matches!(verdict, Ok(Vouched::By(_))) && since.is_none() {
        let tmp = args.state.join("attested.tmp");
        fs::write(&tmp, format!("{}\n", payload.counter))?;
        fs::rename(tmp, args.state.join("attested"))?;
    }
    Ok(verdict)
}

/// Can this box still reach the fleet? None of these is a health check of
/// the other box; the question is only whether this box's own network
/// still carries a connection off itself. A refusal counts: it came back
/// from the other box, so the way there works, whatever listens on the port.
fn reaches_any(reach: &str) -> bool {
    let targets: Vec<&str> = reach.split_whitespace().collect();
    if targets.is_empty() {
        return true; // nothing to reach: a fleet of one
    }
    targets.iter().any(|t| {
        use std::net::ToSocketAddrs as _;
        t.to_socket_addrs().ok().into_iter().flatten().any(|a| {
            match std::net::TcpStream::connect_timeout(&a, Duration::from_secs(4)) {
                Ok(_) => true,
                Err(e) => e.kind() == std::io::ErrorKind::ConnectionRefused,
            }
        })
    })
}

/// The same, given a while, and it has to hold: a box that has just
/// switched may need a moment for its network to settle, and the network
/// it is losing may take a moment to go. One answer proves nothing, so
/// two in a row, seconds apart, are asked for.
fn reaches_within(reach: &str, window: Duration) -> bool {
    let start = Instant::now();
    let mut held = 0;
    loop {
        if reaches_any(reach) {
            held += 1;
            if held >= 2 {
                return true;
            }
        } else {
            held = 0;
        }
        if start.elapsed() > window {
            return false;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

fn main() {
    let args = Args::parse();
    let outcome = run(&args);
    match &outcome {
        Ok(msg) => println!("{msg}"),
        Err(e) => eprintln!("error: {e:#}"),
    }
    if outcome.is_err() {
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<String> {
    fs::create_dir_all(&args.state)?;
    let name = match &args.r#box {
        Some(b) => b.clone(),
        None => hostname()?,
    };
    // the household held this home controller's updates, at its door
    // (servers/smarthome/door, updates.rs): nothing moves until they let go.
    // No box ever has the file.
    if args.state.join("hold").exists() {
        record(args, &name, read_counter(&args.state), "held");
        return Ok("updates are held".into());
    }
    let trusted = release::decode_public(
        fs::read_to_string(&args.key_file)
            .with_context(|| format!("release key {}", args.key_file.display()))?
            .trim(),
    )
    .map_err(|e| anyhow::anyhow!("release key: {e}"))?;

    // no file yet is not an error: a fleet before its first release
    let mut response = match ureq::get(&args.url).call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => {
            record(args, &name, read_counter(&args.state), "none");
            return Ok(format!("no release at {} yet", args.url));
        }
        Err(e) => return Err(e).with_context(|| format!("fetching {}", args.url)),
    };
    let body = response.body_mut().read_to_string()?;
    // checked against the bytes as published, unknown fields and all: a
    // newer publisher may add one, and this agent must still be able to
    // take the release that brings its own replacement
    let signed = match release::verify_json(&body, &trusted) {
        Ok(s) => s,
        Err(e) => {
            // what this box runs has not changed: the counter stays the one
            // it applied, and the refusal is the result
            record(args, &name, read_counter(&args.state), "refused");
            bail!("release refused: {e}");
        }
    };
    let counter = signed.payload.counter;

    let last = read_counter(&args.state);
    if counter < last {
        record(args, &name, last, "refused");
        bail!("release {counter} is older than the {last} this box already applied");
    }

    let Some(mine) = signed.payload.boxes.get(&name) else {
        record(args, &name, last, "absent");
        return Ok(format!("release {counter} has nothing for {name}"));
    };
    // A release this box has already tried and rolled back from is not
    // worth trying every five minutes for the rest of its life: each go
    // costs the probe window twice over in a system that does not work.
    // It is remembered by counter AND path, so a republished fix comes in.
    let tried = args.state.join("refused");
    let stamp = format!("{counter} {}", mine.path);
    if fs::read_to_string(&tried)
        .map(|t| t.lines().any(|l| l.trim() == stamp))
        .unwrap_or(false)
    {
        record(args, &name, last, "refused");
        return Ok(format!(
            "release {counter} was tried here and rolled back; not trying it again"
        ));
    }
    let current = running().context("/run/current-system")?;
    // The same counter is the same release: publish never reuses a number.
    // One that names something else for this box is not a re-publish, and
    // taking it would let one counter mean two systems.
    let applied = fs::read_to_string(args.state.join("applied")).ok();
    if !same_release(counter, last, applied.as_deref(), &current, &mine.path) {
        record(args, &name, last, "refused");
        bail!(
            "release {counter} names {} for {name}, but this box applied {counter} as something else",
            mine.path
        );
    }
    // Running it is not the same as having proved it. The profile is set
    // before the switch and the guard is a transient unit, so a box that
    // power-cycles inside the probe window comes back running the new
    // system with nothing having ever checked it - and this shortcut would
    // then write it down as good. A marker put down before switching says
    // "not proved yet", and it is this path, after a reboot, that owes the
    // probe.
    let pending = args.state.join("pending");
    if current == mine.path {
        // nothing to switch to, so nothing to refuse; but a release that
        // runs here on the boxes' word is where requiring it starts
        if let Ok(Ok(Vouched::By(by))) = vouch(args, &signed.payload, &name) {
            eprintln!("release {counter}: vouched for by {}", by.join(", "));
        }
        let owed = fs::read_to_string(&pending)
            .map(|t| t.trim() == stamp)
            .unwrap_or(false);
        if owed {
            // what was failing before the switch, not now: a unit that
            // failed after the reboot is this release's as much as one that
            // failed after a switch. An older agent's pending has no list;
            // then all there is to go on is now.
            let before = read_failed_before(&args.state).unwrap_or_else(failed_units);
            let ok = probe_until(args, Duration::from_secs(args.probe_secs), &before);
            if !ok {
                let mut seen = fs::read_to_string(&tried).unwrap_or_default();
                seen.push_str(&stamp);
                seen.push('\n');
                let _ = fs::write(&tried, seen);
                let previous = fs::read_to_string(args.state.join("previous")).unwrap_or_default();
                let previous = previous.trim();
                if !previous.is_empty() {
                    let _ = sh(
                        "nix-env",
                        &["-p", "/nix/var/nix/profiles/system", "--set", previous],
                    );
                    let _ = sh(
                        &format!("{previous}/bin/switch-to-configuration"),
                        &["switch"],
                    );
                }
                let _ = fs::remove_file(&pending);
                let _ = fs::remove_file(args.state.join("failed-before"));
                record(args, &name, last, "refused");
                bail!("release {counter} came up after a reboot and did not probe; rolled back");
            }
        }
        let _ = fs::remove_file(&pending);
        let _ = fs::remove_file(args.state.join("failed-before"));
        write_applied(&args.state, counter, &stamp)?;
        record(args, &name, counter, "ok");
        return Ok(format!("release {counter}: already running {}", mine.path));
    }

    // Two boxes of this fleet rebuilt exactly this closure and signed so
    // with their host keys, or nothing changes here. The release key alone
    // does not move a box. Refused before anything is fetched or switched:
    // the box stays on what it runs.
    match vouch(args, &signed.payload, &name)? {
        Ok(Vouched::By(by)) => eprintln!("release {counter}: vouched for by {}", by.join(", ")),
        Ok(Vouched::FleetTooSmall) => {
            eprintln!("release {counter}: a fleet of fewer than two boxes; no box can vouch")
        }
        Ok(Vouched::Bootstrap) => {
            eprintln!("release {counter}: bootstrap, published with no box asked to vouch")
        }
        Ok(Vouched::Before) => eprintln!(
            "release {counter}: carries no attestations; taken, as this box has not yet taken one that does"
        ),
        Err(why) => {
            record(args, &name, last, "refused");
            bail!("release {counter}: not switching to {}: {why}", mine.path);
        }
    }

    // fetch, then check the contents are what was signed: an input-addressed
    // store path says nothing about what is in it, the nar hash does
    if let Some(cache) = &args.cache {
        sh(
            "nix",
            &["copy", "--from", cache, "--no-check-sigs", &mine.path],
        )
        .context("fetching the closure")?;
    }
    let info = sh(
        "nix",
        &[
            "--extra-experimental-features",
            "nix-command",
            "path-info",
            "--json",
            &mine.path,
        ],
    )
    .context("nar hash")?;
    let got =
        release::nar_hash_from_path_info(&info, &mine.path).map_err(|e| anyhow::anyhow!("{e}"))?;
    if got != mine.nar_hash {
        record(args, &name, last, "refused");
        bail!(
            "{} has nar hash {got}, the release says {}",
            mine.path,
            mine.nar_hash
        );
    }
    // and everything underneath. A toplevel is a tree of symlinks: its own
    // nar pins the names of its dependencies, not their contents, and the
    // contents came from a bucket more than one machine can write to.
    if let Some(want) = &mine.closure {
        let rec = sh(
            "nix",
            &[
                "--extra-experimental-features",
                "nix-command",
                "path-info",
                "--json",
                "--recursive",
                &mine.path,
            ],
        )
        .context("reading the closure")?;
        let got = release::closure_digest(&rec).map_err(|e| anyhow::anyhow!("{e}"))?;
        if got != *want {
            record(args, &name, last, "refused");
            bail!(
                "the closure under {} is not the one that was signed",
                mine.path
            );
        }
    }

    // switch, with a way back: the previous system is kept by name, and a
    // transient unit brings it back if this process dies mid-way
    fs::write(args.state.join("previous"), &current)?;
    // what this box could reach, and what was already broken on it,
    // before the switch - both only mean anything as a comparison. The
    // failed units are kept: a reboot inside the window owes the probe
    // against this list, not against whatever is failing when it is back.
    let reached_before = reaches_any(&args.reach);
    let failed_before = failed_units();
    fs::write(args.state.join("failed-before"), failed_before.join("\n"))?;
    // before anything changes: if the box reboots from here, the next run
    // finds this and owes the probe it never got to do
    fs::write(&pending, &stamp)?;
    let _ = disarm();
    // the old system and the profile pointing at it, as the agent's own
    // rollback does: a guard that only switched would leave the profile on
    // the new release, and the next boot would come up on it
    let back = format!(
        "{current}/sw/bin/nix-env -p /nix/var/nix/profiles/system --set {current}; \
         exec {current}/bin/switch-to-configuration switch"
    );
    sh(
        "systemd-run",
        &[
            "--quiet",
            "--unit=dd-agent-guard",
            &format!("--on-active={}", guard_secs(args.probe_secs)),
            "/bin/sh",
            "-c",
            &back,
        ],
    )
    .context("arming the rollback guard")?;
    sh(
        "nix-env",
        &["-p", "/nix/var/nix/profiles/system", "--set", &mine.path],
    )?;
    let switch = || {
        sh(
            &format!("{}/bin/switch-to-configuration", mine.path),
            &["switch"],
        )
    };
    let mut switched = switch();
    // why, in the journal: release 244 rolled back on node1 with nothing to
    // say what the switch had refused. Then once more: what fails a switch
    // is as often the moment as the release (a user's systemd stopping just
    // then cost 244 and 246), and a second switch to the same system only
    // finishes what the first began
    if let Err(e) = &switched {
        eprintln!("release {counter}: the switch failed: {e:#}");
        std::thread::sleep(Duration::from_secs(10));
        switched = switch();
        match &switched {
            Ok(_) => eprintln!("release {counter}: switched on the second try"),
            Err(e) => eprintln!("release {counter}: the second switch failed too: {e:#}"),
        }
    }

    let probed =
        switched.is_ok() && probe_until(args, Duration::from_secs(args.probe_secs), &failed_before);
    // and it may not have cost this box its way off itself; asked while the
    // new system runs, since after the switch back the way is there again
    let stranded = probed
        && reached_before
        && !reaches_within(&args.reach, Duration::from_secs(args.probe_secs));
    let healthy = probed && !stranded;
    // No more firing from here; then, whatever the probes said, it is the
    // system running now that is written down. A guard that went off while
    // they ran has put the old one back, and a pass measured across that is
    // not this release's.
    let _ = sh("systemctl", &["stop", "dd-agent-guard.timer"]);
    let guarded = guard_fired(
        &sh(
            "systemctl",
            &[
                "show",
                "-P",
                "ExecMainStartTimestampMonotonic",
                "dd-agent-guard.service",
            ],
        )
        .unwrap_or_default(),
        running().ok().as_deref(),
        &mine.path,
    );
    let healthy = healthy && !guarded;
    if !healthy {
        // remember it before undoing, so a crash between the two does not
        // leave this box to meet the same release again in five minutes
        let mut seen = fs::read_to_string(&tried).unwrap_or_default();
        seen.push_str(&stamp);
        seen.push('\n');
        let _ = fs::write(&tried, seen);
        // a guard part-way through its switch finishes first; two switches
        // at once is neither system
        let waited = Instant::now();
        while guarded
            && waited.elapsed() < Duration::from_secs(600)
            && sh(
                "systemctl",
                &["show", "-P", "ActiveState", "dd-agent-guard.service"],
            )
            .is_ok_and(|s| matches!(s.trim(), "active" | "activating" | "deactivating"))
        {
            std::thread::sleep(Duration::from_secs(2));
        }
        let _ = sh(
            "nix-env",
            &["-p", "/nix/var/nix/profiles/system", "--set", &current],
        );
        let _ = sh(
            &format!("{current}/bin/switch-to-configuration"),
            &["switch"],
        );
        let _ = disarm();
        // back on the old system: nothing is owed for the new one
        let _ = fs::remove_file(&pending);
        let _ = fs::remove_file(args.state.join("failed-before"));
        record(args, &name, last, "rollback");
        bail!(
            "release {counter}: {} did not come up healthy{}; back on {current}",
            mine.path,
            if guarded {
                " (the guard switched back before it was proved)"
            } else if stranded {
                " (it could no longer reach the fleet)"
            } else {
                ""
            }
        );
    }
    let _ = disarm();
    // probed and good: nothing is owed any more
    let _ = fs::remove_file(&pending);
    let _ = fs::remove_file(args.state.join("failed-before"));
    write_applied(&args.state, counter, &stamp)?;
    record(args, &name, counter, "ok");
    Ok(format!("release {counter}: now running {}", mine.path))
}

/// The guard is a transient timer and its service; both go, or the timer
/// fires later and switches the box back under a healthy release, and the
/// next arming fails because the name is taken. A failed one stays loaded
/// until it is reset, and holds the name just the same.
fn disarm() -> Result<String> {
    let stopped = sh(
        "systemctl",
        &["stop", "dd-agent-guard.timer", "dd-agent-guard.service"],
    );
    let _ = sh(
        "systemctl",
        &[
            "reset-failed",
            "dd-agent-guard.timer",
            "dd-agent-guard.service",
        ],
    );
    stopped
}

/// How long after arming the guard fires. It has to outlast everything the
/// agent may spend deciding - the switch, up to the probe window, then up
/// to the probe window again for the reach - or it switches the box back
/// under a release that is about to pass. The margin is the switch's, and
/// each probe's last try past its window.
fn guard_secs(probe_secs: u64) -> u64 {
    2 * probe_secs + 300
}

/// Has the guard gone off? Its service has started (systemd's start
/// timestamp is 0 for a unit that never ran, or one it no longer knows),
/// or the box no longer runs the release. Either way the probes passed on
/// a box that was being, or had been, switched back.
fn guard_fired(started_usec: &str, running: Option<&str>, path: &str) -> bool {
    let started = started_usec.trim().parse::<u64>().unwrap_or(0) != 0;
    started || running != Some(path)
}

/// A counter equal to the one applied is the same release fetched again,
/// or a re-publish of it, and is fine only if it names the same system.
/// `applied` is the `counter path` this box wrote when it applied it; an
/// older agent wrote none, and then what runs now is what it applied.
fn same_release(counter: u64, last: u64, applied: Option<&str>, running: &str, path: &str) -> bool {
    if counter != last {
        return true;
    }
    match applied {
        Some(a) => a.trim() == format!("{counter} {path}"),
        None => running == path,
    }
}

fn running() -> Result<String> {
    Ok(fs::read_link("/run/current-system")?
        .to_string_lossy()
        .into_owned())
}

/// The units that were failing before the last switch, as written then;
/// None when nothing was written.
fn read_failed_before(state: &Path) -> Option<Vec<String>> {
    let t = fs::read_to_string(state.join("failed-before")).ok()?;
    Some(
        t.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Which units are failed right now, games aside: a member's game that
/// crashed is that game's trouble, not a release's.
fn failed_units() -> Vec<String> {
    sh("systemctl", &["--failed", "--no-legend", "--plain"])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with("dd-game@"))
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect()
}

/// The box is a box: nothing newly failed, ssh answers, the verifier answers.
///
/// `before` is what was already failing when the switch began. Something
/// broken beforehand is not this release's doing, and counting it would
/// make every later release roll back - including the one that fixes it.
fn probe(args: &Args, before: &[String]) -> Result<()> {
    let failed: Vec<String> = failed_units()
        .into_iter()
        .filter(|u| !before.contains(u))
        .collect();
    if !failed.is_empty() {
        bail!("failed units: {}", failed.join(" ").trim());
    }
    if args.ssh {
        let ssh = sh("systemctl", &["is-active", "sshd.service"])?;
        if ssh.trim() != "active" {
            bail!("sshd is {}", ssh.trim());
        }
    }
    std::net::TcpStream::connect_timeout(
        &([127, 0, 0, 1], args.verify_port).into(),
        Duration::from_secs(3),
    )
    .with_context(|| format!("verifier on {}", args.verify_port))?;
    Ok(())
}

fn probe_until(args: &Args, window: Duration, before: &[String]) -> bool {
    let start = Instant::now();
    loop {
        match probe(args, before) {
            Ok(()) => return true,
            Err(e) => {
                eprintln!("probe: {e:#}");
                if start.elapsed() > window {
                    return false;
                }
                std::thread::sleep(Duration::from_secs(10));
            }
        }
    }
}

fn read_counter(state: &Path) -> u64 {
    fs::read_to_string(state.join("counter"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// The counter, and what it was for this box: the second is what tells
/// a re-publish from another system under the same number.
fn write_applied(state: &Path, n: u64, stamp: &str) -> Result<()> {
    let tmp = state.join("applied.tmp");
    fs::write(&tmp, format!("{stamp}\n"))?;
    fs::rename(tmp, state.join("applied"))?;
    let tmp = state.join("counter.tmp");
    fs::write(&tmp, format!("{n}\n"))?;
    fs::rename(tmp, state.join("counter"))?;
    Ok(())
}

/// Samples for the box dashboard and `dd release status`: the counter this
/// box runs, what it runs, and how the last run ended. Best effort; a
/// metric must never fail a release.
fn record(args: &Args, name: &str, counter: u64, result: &str) {
    let _ = fs::create_dir_all(&args.facts);
    let now = release::now();
    let running = fs::read_link("/run/current-system")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let body = format!(
        "# HELP dd_agent_counter The release counter this box last applied.\n\
         # TYPE dd_agent_counter gauge\n\
         dd_agent_counter{{box=\"{name}\"}} {counter}\n\
         # HELP dd_agent_info What this box runs and how the agent's last run ended.\n\
         # TYPE dd_agent_info gauge\n\
         dd_agent_info{{box=\"{name}\",path=\"{running}\",result=\"{result}\"}} 1\n\
         # HELP dd_agent_last_run_seconds When the agent last ran.\n\
         # TYPE dd_agent_last_run_seconds gauge\n\
         dd_agent_last_run_seconds{{box=\"{name}\"}} {now}\n"
    );
    let tmp = args.facts.join("dd_agent.prom.tmp");
    if fs::write(&tmp, body).is_ok() {
        let _ = fs::rename(tmp, args.facts.join("dd_agent.prom"));
    }
}

fn hostname() -> Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/hostname")?
        .trim()
        .to_owned())
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

    #[test]
    fn the_guard_outlasts_the_whole_decision() {
        for probe in [20, 600, 3600] {
            // the probe window, then the reach window, each with a last try
            // past it, and the switch before both
            let decision = probe + 10 + probe + 5 + 60;
            assert!(guard_secs(probe) > decision, "probe {probe}");
        }
        assert_eq!(guard_secs(600), 1500);
    }

    #[test]
    fn a_pass_under_a_guard_that_fired_is_no_pass() {
        let new = "/nix/store/new";
        assert!(!guard_fired("0", Some(new), new));
        assert!(!guard_fired("", Some(new), new), "no such unit any more");
        assert!(guard_fired("123456789", Some(new), new), "the guard ran");
        assert!(guard_fired("0", Some("/nix/store/old"), new), "it is back");
        assert!(guard_fired("0", None, new));
    }

    #[test]
    fn one_counter_is_one_system() {
        let (a, b) = ("/nix/store/a", "/nix/store/b");
        // a newer counter is the rest of the agent's to judge
        assert!(same_release(3, 2, Some("2 /nix/store/a"), a, b));
        // the same release again, or published again unchanged
        assert!(same_release(2, 2, Some("2 /nix/store/a\n"), b, a));
        // the same number naming something else
        assert!(!same_release(2, 2, Some("2 /nix/store/a"), a, b));
        // an older agent left no record: what runs is what it applied
        assert!(same_release(2, 2, None, a, a));
        assert!(!same_release(2, 2, None, a, b));
    }

    #[test]
    fn the_owed_probe_judges_against_what_failed_before_the_switch() {
        let dir = std::env::temp_dir().join(format!("dd-agent-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_failed_before(&dir), None, "nothing written");
        fs::write(dir.join("failed-before"), "").unwrap();
        assert_eq!(
            read_failed_before(&dir),
            Some(vec![]),
            "nothing was failing"
        );
        fs::write(dir.join("failed-before"), "a.service\nb.timer").unwrap();
        assert_eq!(
            read_failed_before(&dir),
            Some(vec!["a.service".to_string(), "b.timer".to_string()])
        );
        write_applied(&dir, 7, "7 /nix/store/x").unwrap();
        assert_eq!(read_counter(&dir), 7);
        assert_eq!(
            fs::read_to_string(dir.join("applied")).unwrap().trim(),
            "7 /nix/store/x"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_boxes_vouch_or_nothing_moves() {
        use std::collections::BTreeSet;
        let none = BTreeSet::new();
        let one: BTreeSet<String> = ["node1".to_string()].into();
        let two: BTreeSet<String> = ["node1".to_string(), "node2".to_string()].into();
        // two boxes of the fleet: taken, before and after it is required
        assert_eq!(
            judge(2, &two, true, false, None),
            Ok(Vouched::By(vec!["node1".into(), "node2".into()]))
        );
        assert!(matches!(
            judge(3, &two, true, false, Some(5)),
            Ok(Vouched::By(_))
        ));
        // one is not two, whatever else the release says
        assert!(judge(2, &one, true, false, None).is_err());
        assert!(judge(2, &one, true, true, None).is_err());
        let why = judge(2, &one, true, false, Some(5)).unwrap_err();
        assert!(
            why.contains("1 of the 2") && why.contains("since release 5"),
            "{why}"
        );
        // once required: a release without them, bootstrap or not, is refused
        assert!(judge(2, &none, false, false, Some(5)).is_err());
        assert!(judge(2, &none, false, true, Some(5)).is_err());
        // until then: one from before them, or marked bootstrap, is taken
        assert_eq!(judge(2, &none, false, false, None), Ok(Vouched::Before));
        assert_eq!(judge(2, &none, false, true, None), Ok(Vouched::Bootstrap));
        // a fleet that cannot make two
        assert_eq!(
            judge(1, &none, false, false, Some(5)),
            Ok(Vouched::FleetTooSmall)
        );
        assert_eq!(
            judge(0, &none, true, false, None),
            Ok(Vouched::FleetTooSmall)
        );
    }

    fn args_in(state: &Path, attesters: Option<PathBuf>) -> Args {
        Args::parse_from(
            [
                "dd-agent".to_string(),
                "--url".into(),
                "http://127.0.0.1:9/current.json".into(),
                "--key-file".into(),
                "/nonexistent".into(),
                "--state".into(),
                state.display().to_string(),
            ]
            .into_iter()
            .chain(attesters.map(|a| format!("--attesters={}", a.display()))),
        )
    }

    fn payload(attestations: Vec<release::Attestation>) -> release::Payload {
        let mut boxes = std::collections::BTreeMap::new();
        boxes.insert(
            "node2".to_string(),
            release::BoxRelease {
                path: "/nix/store/new-node2".into(),
                nar_hash: "sha256-x".into(),
                closure: Some("c".into()),
            },
        );
        release::Payload {
            counter: 12,
            rev: "a".repeat(40),
            issued: 1,
            boxes,
            attestations,
            bootstrap: false,
        }
    }

    #[test]
    fn a_refusal_leaves_the_state_as_it_was_and_says_why() {
        let dir = std::env::temp_dir().join(format!("dd-agent-vouch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let fleet = dir.join("attesters.json");
        fs::write(
            &fleet,
            r#"{"node1": "ssh-ed25519 AAAA1", "node2": "ssh-ed25519 AAAA2"}"#,
        )
        .unwrap();
        let args = args_in(&dir, Some(fleet.clone()));
        // an attestation whose signature is nobody's: refused, nothing written
        let forged = release::Attestation {
            attester: "node1".into(),
            statement: serde_json::json!({
                "v": 1, "attester": "node1", "flake": "git+https://x/o/r", "rev": "a".repeat(40),
                "boxes": { "node2": { "path": "/nix/store/new-node2", "closure": "c" } },
            })
            .to_string(),
            signature: "not a signature".into(),
        };
        let p = payload(vec![
            forged.clone(),
            release::Attestation {
                attester: "node2".into(),
                ..forged
            },
        ]);
        let why = vouch(&args, &p, "node2").unwrap().unwrap_err();
        assert!(why.starts_with("0 of the 2"), "{why}");
        assert_eq!(read_since(&dir), None);
        assert!(!dir.join("attest").exists(), "its work directory is gone");
        // the same, signed by the two boxes' own host keys: taken, and
        // from here on required
        let keygen = |n: &str| {
            let k = dir.join(n);
            assert!(
                Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                    .arg(&k)
                    .status()
                    .unwrap()
                    .success()
            );
            let p = fs::read_to_string(k.with_extension("pub")).unwrap();
            (
                k,
                p.split_whitespace().take(2).collect::<Vec<_>>().join(" "),
            )
        };
        let sign = |key: &Path, text: &str| {
            use std::io::Write as _;
            let mut c = Command::new("ssh-keygen")
                .args(["-Y", "sign", "-q", "-f"])
                .arg(key)
                .args(["-n", release::ATTEST_NAMESPACE])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
            String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap()
        };
        let ((k1, pub1), (k2, pub2)) = (keygen("node1"), keygen("node2"));
        let real = dir.join("real.json");
        fs::write(
            &real,
            serde_json::json!({ "node1": pub1, "node2": pub2 }).to_string(),
        )
        .unwrap();
        let said = |who: &str, key: &Path| {
            let statement = p.attestations[0]
                .statement
                .replace("\"node1\"", &format!("\"{who}\""));
            release::Attestation {
                attester: who.into(),
                signature: sign(key, &statement),
                statement,
            }
        };
        let good = payload(vec![said("node1", &k1), said("node2", &k2)]);
        let both = vouch(&args_in(&dir, Some(real.clone())), &good, "node2").unwrap();
        assert_eq!(both, Ok(Vouched::By(vec!["node1".into(), "node2".into()])));
        assert_eq!(read_since(&dir), Some(12));
        // the box alone is not two boxes
        let alone = payload(vec![said("node2", &k2)]);
        assert!(
            vouch(&args_in(&dir, Some(real)), &alone, "node2")
                .unwrap()
                .is_err()
        );
        fs::remove_file(dir.join("attested")).unwrap();

        // a release from before them, on a box that never required them
        assert_eq!(
            vouch(&args, &payload(vec![]), "node2").unwrap(),
            Ok(Vouched::Before)
        );
        // once one was taken on the boxes' word, the same is refused
        fs::write(dir.join("attested"), "11\n").unwrap();
        assert!(vouch(&args, &payload(vec![]), "node2").unwrap().is_err());
        // a fleet file that does not parse stops the agent rather than
        // turning the check off
        fs::write(&fleet, "not json").unwrap();
        assert!(vouch(&args, &payload(vec![]), "node2").is_err());
        // no fleet file at all (the vm test's boxes): a fleet that cannot vouch
        assert_eq!(
            vouch(&args_in(&dir, None), &payload(vec![]), "node2").unwrap(),
            Ok(Vouched::FleetTooSmall)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_refusal_is_reach_and_silence_is_not() {
        let open = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(reaches_any(&open.local_addr().unwrap().to_string()));
        // a port nothing listens on: the box answered, so the way is there
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = closed.local_addr().unwrap().to_string();
        drop(closed);
        assert!(reaches_any(&addr));
        // documentation's own address: nothing answers, nothing refuses
        assert!(!reaches_any("192.0.2.1:9"));
        assert!(reaches_any(""));
    }
}
