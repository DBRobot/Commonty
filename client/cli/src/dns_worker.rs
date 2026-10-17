//! The DNS Worker at Cloudflare (servers/system/dns): the only holder of
//! the zone's DNS token. It keeps the fleet's names as the release has them,
//! every five minutes, and writes a box's certificate challenges for the
//! names that box serves. Set up once with `dd dns setup`, uploaded by
//! `dd release publish` from the release it ships with, beside the mail
//! Worker and with the same token. The DNS token goes straight to the
//! Worker as a secret and is kept nowhere.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::mail_worker::{api, call, token};

const SCRIPT: &str = "commonty-dns";
const COMPATIBILITY: &str = "2026-05-01";

/// fleet/dns-worker.json: where the Worker is. Nothing secret. Its being
/// there is what moves the zone onto it (dd.dnsWorker, nix/modules/box/domain.nix):
/// the boxes' certificates, and the names the gateway kept before.
#[derive(Serialize, Deserialize)]
pub struct Setup {
    pub domain: String,
    pub account: String,
    pub zone: String,
    pub script: String,
}

fn read_setup(root: &Path) -> Result<Option<Setup>> {
    let p = root.join("fleet/dns-worker.json");
    if !p.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&std::fs::read(p)?)?))
}

fn bundle(flake: &str) -> Result<std::path::PathBuf> {
    let out = std::process::Command::new("nix")
        .args([
            "build",
            "--no-link",
            "--print-out-paths",
            &format!("{flake}#dns-worker"),
        ])
        .output()?;
    ensure!(
        out.status.success(),
        "building the DNS Worker: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().into())
}

/// the control plane's relay, whose witness has the agreed state: which box
/// holds each lease, which the names follow (fleet/control-worker.json)
fn control_script(root: &Path) -> Result<Option<String>> {
    let p = root.join("fleet/control-worker.json");
    if !p.exists() {
        return Ok(None);
    }
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(p)?)?;
    Ok(v["script"].as_str().map(str::to_string))
}

async fn upload(
    http: &reqwest::Client,
    token: &str,
    s: &Setup,
    dir: &Path,
    control: Option<&str>,
) -> Result<()> {
    let mut bindings = vec![
        serde_json::json!({ "type": "plain_text", "name": "DOMAIN", "text": s.domain }),
        serde_json::json!({ "type": "plain_text", "name": "ZONE", "text": s.zone }),
        serde_json::json!({ "type": "plain_text", "name": "ACCOUNT", "text": s.account }),
    ];
    if let Some(script) = control {
        bindings.push(serde_json::json!({
            "type": "durable_object_namespace",
            "name": "CONTROL",
            "class_name": "Relay",
            "script_name": script,
        }));
    }
    let metadata = serde_json::json!({
        "main_module": "worker.js",
        "compatibility_date": COMPATIBILITY,
        "bindings": bindings,
        // DNS_TOKEN and TUNNEL_TOKEN were set once and are kept
        "keep_bindings": ["secret_text"],
    });
    let mut form = reqwest::multipart::Form::new().part(
        "metadata",
        reqwest::multipart::Part::text(metadata.to_string()).mime_str("application/json")?,
    );
    // every module the Worker's package holds: worker.js imports
    // controller.js, which imports the mail crate's dd_mail.js and its wasm.
    // Two names by hand left controller.js out, and Cloudflare refused every
    // deploy from release 271 on ("No such module"), keeping a Worker that
    // knew none of the names added since
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for n in &names {
        let Some(mime) = module_mime(n) else { continue };
        form = form.part(
            n.clone(),
            reqwest::multipart::Part::bytes(std::fs::read(dir.join(n))?)
                .file_name(n.clone())
                .mime_str(mime)?,
        );
    }
    let v: serde_json::Value = http
        .put(format!(
            "{}/accounts/{}/workers/scripts/{}",
            api(),
            s.account,
            s.script
        ))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await?
        .json()
        .await?;
    if v["success"] != serde_json::Value::Bool(true) {
        bail!("Cloudflare did not take the DNS Worker: {}", v["errors"]);
    }
    // the names are put in step every minute, from what was uploaded and
    // who holds each lease
    call(
        http,
        token,
        reqwest::Method::PUT,
        &format!(
            "/accounts/{}/workers/scripts/{}/schedules",
            s.account, s.script
        ),
        Some(serde_json::json!([{ "cron": "* * * * *" }])),
    )
    .await?;
    Ok(())
}

/// Upload the Worker the release at `ref` carries. Nothing to do before
/// `dd dns setup` has been run.
/// How Cloudflare takes a module of the Worker's package, by its name; none
/// for a file that is not one
fn module_mime(name: &str) -> Option<&'static str> {
    if name.ends_with(".js") {
        Some("application/javascript+module")
    } else if name.ends_with(".wasm") {
        Some("application/wasm")
    } else {
        None
    }
}

pub async fn deploy(root: &Path, keys: &impl auth::KeyStore, r#ref: &str) -> Result<()> {
    let Some(s) = read_setup(root)? else {
        eprintln!("== no DNS Worker set up yet (`dd dns setup`); nothing uploaded");
        return Ok(());
    };
    let dir = bundle(&format!("git+file://{}?ref={ref}", root.display()))?;
    let control = control_script(root)?;
    upload(
        &reqwest::Client::new(),
        &token(keys)?,
        &s,
        &dir,
        control.as_deref(),
    )
    .await?;
    eprintln!("== the DNS Worker is {}", dir.display());
    Ok(())
}

/// Once: the first upload, the DNS token as its secret, the name
/// dns.<domain>, and then fleet/dns-worker.json - committed and released,
/// that moves the zone onto the Worker and its token off the gateway.
pub async fn setup(root: &Path, keys: &impl auth::KeyStore, domain: &str) -> Result<()> {
    let token = token(keys)?;
    let http = reqwest::Client::new();
    let z = call(
        &http,
        &token,
        reqwest::Method::GET,
        &format!("/zones?name={domain}"),
        None,
    )
    .await?;
    let zone = &z["result"][0];
    let zone_id = zone["id"]
        .as_str()
        .context("no such zone for this token")?
        .to_string();
    let account = zone["account"]["id"]
        .as_str()
        .context("zone without account")?
        .to_string();
    let s = Setup {
        domain: domain.into(),
        account: account.clone(),
        zone: zone_id.clone(),
        script: SCRIPT.into(),
    };

    let dir = bundle(&format!("path:{}", root.display()))?;
    let control = control_script(root)?;
    upload(&http, &token, &s, &dir, control.as_deref()).await?;
    eprintln!("== uploaded the DNS Worker");

    eprintln!(
        "The DNS token now: Zone > DNS Edit, this zone only. It goes straight to the\n\
         Worker and is kept nowhere else; no box holds it."
    );
    // or from the environment, where nothing is kept
    let dns = match std::env::var("CF_DNS_TOKEN") {
        Ok(t) => t,
        Err(_) => rpassword::prompt_password("DNS token: ")?,
    };
    ensure!(!dns.trim().is_empty(), "no token given");
    call(
        &http,
        &token,
        reqwest::Method::PUT,
        &format!("/accounts/{account}/workers/scripts/{SCRIPT}/secrets"),
        Some(serde_json::json!({ "name": "DNS_TOKEN", "text": dns.trim(), "type": "secret_text" })),
    )
    .await?;
    eprintln!("== the Worker has the DNS token");
    call(
        &http,
        &token,
        reqwest::Method::PUT,
        &format!("/accounts/{account}/workers/domains"),
        Some(serde_json::json!({
            "hostname": format!("dns.{domain}"),
            "service": SCRIPT,
            "environment": "production",
            "zone_id": zone_id,
        })),
    )
    .await?;
    eprintln!("== the Worker answers at https://dns.{domain}");
    std::fs::write(
        root.join("fleet/dns-worker.json"),
        serde_json::to_string_pretty(&s)? + "\n",
    )?;
    eprintln!("== wrote fleet/dns-worker.json");
    eprintln!(
        "Left: commit fleet/dns-worker.json and release; each box then asks for a\n\
         certificate of its own names, and the gateway no longer holds the zone's\n\
         token: once it has switched, delete that token at Cloudflare and\n\
         cloudflare-token from its sops file."
    );
    Ok(())
}

/// Once, for failover: the token the Worker makes each box's own tunnel
/// with (Account > Cloudflare Tunnel > Edit, this account only). It goes
/// straight to the Worker as a secret and is kept nowhere else; no box
/// holds it. Each box then asks for its tunnel with a secret it makes
/// (nix/modules/gate/door-ask.sh).
pub async fn tunnels(root: &Path, keys: &impl auth::KeyStore) -> Result<()> {
    let Some(s) = read_setup(root)? else {
        bail!("no DNS Worker yet: `dd dns setup` first");
    };
    eprintln!(
        "The tunnel token now: Account > Cloudflare Tunnel > Edit, this account only.\n\
         It goes straight to the Worker and is kept nowhere else; no box holds it."
    );
    let t = match std::env::var("CF_TUNNEL_TOKEN") {
        Ok(t) => t,
        Err(_) => rpassword::prompt_password("Tunnel token: ")?,
    };
    ensure!(!t.trim().is_empty(), "no token given");
    call(
        &reqwest::Client::new(),
        &token(keys)?,
        reqwest::Method::PUT,
        &format!(
            "/accounts/{}/workers/scripts/{}/secrets",
            s.account, s.script
        ),
        Some(
            serde_json::json!({ "name": "TUNNEL_TOKEN", "text": t.trim(), "type": "secret_text" }),
        ),
    )
    .await?;
    eprintln!(
        "== the Worker has the tunnel token; each box asks for its own tunnel at its next start"
    );
    Ok(())
}

#[cfg(test)]
mod module_tests {
    use super::module_mime;

    #[test]
    fn every_module_the_worker_imports_goes_up() {
        for n in ["worker.js", "fleet.js", "controller.js", "dd_mail.js"] {
            assert_eq!(module_mime(n), Some("application/javascript+module"), "{n}");
        }
        assert_eq!(module_mime("dd_mail_bg.wasm"), Some("application/wasm"));
        assert_eq!(module_mime("README"), None);
    }
}
