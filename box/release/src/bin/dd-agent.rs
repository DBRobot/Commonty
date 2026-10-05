//! The agent: a box moves itself to the release, and to nothing else.
//!
//! Runs from a timer. Fetches the release file, checks it is signed by the
//! release key this box was installed with, refuses a counter lower than
//! the last one it applied, looks up its own entry, fetches that closure,
//! checks the nar hash, switches, and proves the box is still a box: no
//! failed units, sshd listening, the verifier answering. If that fails
//! within the window the previous system comes back. Nothing here accepts
//! a push; the only input is a signed file and the only authority is the
//! key that signed it.
//!
//! bin/deploy, inverted: the same steps, started by the box.

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
    /// host:port of the other boxes, whitespace separated. A release that
    /// takes away the reach this box had before it is rolled back: the
    /// switch may not strand the box, however healthy it looks from here.
    #[arg(long, env = "DD_AGENT_REACH", default_value = "")]
    reach: String,
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
            record(args, &name, 0, "refused");
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
    let current = fs::read_link("/run/current-system")
        .context("/run/current-system")?
        .to_string_lossy()
        .into_owned();
    // Running it is not the same as having proved it. The profile is set
    // before the switch and the guard is a transient unit, so a box that
    // power-cycles inside the probe window comes back running the new
    // system with nothing having ever checked it - and this shortcut would
    // then write it down as good. A marker put down before switching says
    // "not proved yet", and it is this path, after a reboot, that owes the
    // probe.
    let pending = args.state.join("pending");
    if current == mine.path {
        let owed = fs::read_to_string(&pending)
            .map(|t| t.trim() == stamp)
            .unwrap_or(false);
        if owed {
            let ok = probe_until(args, Duration::from_secs(args.probe_secs), &failed_units());
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
                record(args, &name, last, "refused");
                bail!("release {counter} came up after a reboot and did not probe; rolled back");
            }
        }
        let _ = fs::remove_file(&pending);
        write_counter(&args.state, counter)?;
        record(args, &name, counter, "ok");
        return Ok(format!("release {counter}: already running {}", mine.path));
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
    // before anything changes: if the box reboots from here, the next run
    // finds this and owes the probe it never got to do
    fs::write(&pending, &stamp)?;
    let guard_secs = (args.probe_secs + 300).to_string();
    let _ = disarm();
    // what this box could reach, and what was already broken on it,
    // before the switch - both only mean anything as a comparison
    let reached_before = reaches_any(&args.reach);
    let failed_before = failed_units();
    sh(
        "systemd-run",
        &[
            "--quiet",
            "--unit=dd-agent-guard",
            &format!("--on-active={guard_secs}"),
            &format!("{current}/bin/switch-to-configuration"),
            "switch",
        ],
    )
    .context("arming the rollback guard")?;
    sh(
        "nix-env",
        &["-p", "/nix/var/nix/profiles/system", "--set", &mine.path],
    )?;
    let switched = sh(
        &format!("{}/bin/switch-to-configuration", mine.path),
        &["switch"],
    );

    let healthy = switched.is_ok()
        && probe_until(args, Duration::from_secs(args.probe_secs), &failed_before)
        // and it may not have cost this box its way off itself
        && (!reached_before || reaches_within(&args.reach, Duration::from_secs(args.probe_secs)));
    if !healthy {
        // remember it before undoing, so a crash between the two does not
        // leave this box to meet the same release again in five minutes
        let mut seen = fs::read_to_string(&tried).unwrap_or_default();
        seen.push_str(&stamp);
        seen.push('\n');
        let _ = fs::write(&tried, seen);
        let _ = sh(
            "nix-env",
            &["-p", "/nix/var/nix/profiles/system", "--set", &current],
        );
        let _ = sh(
            &format!("{current}/bin/switch-to-configuration"),
            &["switch"],
        );
        let _ = disarm();
        record(args, &name, last, "rollback");
        bail!(
            "release {counter}: {} did not come up healthy{}; back on {current}",
            mine.path,
            if reached_before && !reaches_any(&args.reach) {
                " (it could no longer reach the fleet)"
            } else {
                ""
            }
        );
    }
    let _ = disarm();
    // probed and good: nothing is owed any more
    let _ = fs::remove_file(&pending);
    write_counter(&args.state, counter)?;
    record(args, &name, counter, "ok");
    Ok(format!("release {counter}: now running {}", mine.path))
}

/// The guard is a transient timer and its service; both go, or the timer
/// fires later and switches the box back under a healthy release, and the
/// next arming fails because the name is taken.
fn disarm() -> Result<String> {
    sh(
        "systemctl",
        &["stop", "dd-agent-guard.timer", "dd-agent-guard.service"],
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
    let ssh = sh("systemctl", &["is-active", "sshd.service"])?;
    if ssh.trim() != "active" {
        bail!("sshd is {}", ssh.trim());
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

fn write_counter(state: &Path, n: u64) -> Result<()> {
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
    use super::reaches_any;

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
