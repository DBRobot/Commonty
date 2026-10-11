//! The control plane's relay at Cloudflare (servers/system/control/worker): set up once
//! with `dd control setup`, uploaded by `dd release publish` from the release
//! it ships with, beside the mail Worker and with the same token. The
//! witness's key is made here, handed straight to the Worker as a secret and
//! kept nowhere; only its public half goes in fleet/control.json.
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::mail_worker::{api, call, token};

const SCRIPT: &str = "commonty-control";
const COMPATIBILITY: &str = "2026-05-01";
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// fleet/control-worker.json: where the relay is. Nothing secret.
#[derive(Serialize, Deserialize)]
pub struct Setup {
    pub domain: String,
    pub account: String,
    pub zone: String,
    pub script: String,
}

fn read_setup(root: &Path) -> Result<Option<Setup>> {
    let p = root.join("fleet/control-worker.json");
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
            &format!("{flake}#control-worker"),
        ])
        .output()?;
    ensure!(
        out.status.success(),
        "building the relay Worker: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().into())
}

fn kind(name: &str) -> &'static str {
    match name {
        "worker.js" | "wasm_exec.js" | "config.js" => "application/javascript+module",
        n if n.ends_with(".wasm") => "application/wasm",
        _ => "text/plain",
    }
}

async fn upload(
    http: &reqwest::Client,
    token: &str,
    s: &Setup,
    dir: &Path,
    first: bool,
) -> Result<()> {
    let mut metadata = serde_json::json!({
        "main_module": "worker.js",
        "compatibility_date": COMPATIBILITY,
        "bindings": [
            { "type": "durable_object_namespace", "name": "RELAY", "class_name": "Relay" },
        ],
        // WITNESS_KEY was set once and is kept
        "keep_bindings": ["secret_text"],
    });
    if first {
        metadata["migrations"] =
            serde_json::json!({ "new_tag": "v1", "new_sqlite_classes": ["Relay"] });
    }
    let mut form = reqwest::multipart::Form::new().part(
        "metadata",
        reqwest::multipart::Part::text(metadata.to_string()).mime_str("application/json")?,
    );
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<_>>()?;
    names.sort();
    for n in &names {
        form = form.part(
            n.clone(),
            reqwest::multipart::Part::bytes(std::fs::read(dir.join(n))?)
                .file_name(n.clone())
                .mime_str(kind(n))?,
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
        bail!("Cloudflare did not take the relay: {}", v["errors"]);
    }
    Ok(())
}

/// Upload the relay the release at `ref` carries. Nothing to do before
/// `dd control setup` has been run.
pub async fn deploy(root: &Path, keys: &impl auth::KeyStore, r#ref: &str) -> Result<()> {
    let Some(s) = read_setup(root)? else {
        eprintln!("== no control relay set up yet (`dd control setup`); nothing uploaded");
        return Ok(());
    };
    let dir = bundle(&format!("git+file://{}?ref={ref}", root.display()))?;
    upload(&reqwest::Client::new(), &token(keys)?, &s, &dir, false).await?;
    eprintln!("== the control relay is {}", dir.display());
    Ok(())
}

/// Once: the voters in fleet/control.json (the boxes' validator keys as
/// given, then the witness made here), the first upload with its Durable
/// Object, the witness's key as a secret, and the name control.<domain>.
pub async fn setup(
    root: &Path,
    keys: &impl auth::KeyStore,
    domain: &str,
    validators: &[String],
) -> Result<()> {
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

    // the voters: each box's key as its `dd-control -print-validator-key`
    // gave it, then the witness
    let path = root.join("fleet/control.json");
    let mut ctl: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let mut list = Vec::new();
    for (i, v) in validators.iter().enumerate() {
        let (name, key) = v.split_once('=').context("a validator is name=key")?;
        let raw = B64.decode(key).context("a validator key is base64")?;
        ensure!(raw.len() == 32, "{name}'s key is not an ed25519 public key");
        list.push(serde_json::json!({ "id": i + 1, "name": name, "key": key }));
    }
    let seed = {
        let mut s = [0u8; 32];
        getrandom::fill(&mut s).map_err(|e| anyhow::anyhow!("{e}"))?;
        s
    };
    let witness = ed25519_dalek::SigningKey::from_bytes(&seed);
    list.push(serde_json::json!({
        "id": validators.len() + 1,
        "name": "cloudflare",
        "key": B64.encode(witness.verifying_key().to_bytes()),
    }));
    ctl["validators"] = serde_json::Value::Array(list);
    std::fs::write(&path, serde_json::to_string_pretty(&ctl)? + "\n")?;
    let s = Setup {
        domain: domain.into(),
        account: account.clone(),
        zone: zone_id.clone(),
        script: SCRIPT.into(),
    };
    std::fs::write(
        root.join("fleet/control-worker.json"),
        serde_json::to_string_pretty(&s)? + "\n",
    )?;
    eprintln!("== wrote fleet/control.json and fleet/control-worker.json");

    // the bundle as this checkout has it, with the new voters
    let dir = bundle(&format!("path:{}", root.display()))?;
    upload(&http, &token, &s, &dir, true).await?;
    eprintln!("== uploaded the relay");
    call(
        &http,
        &token,
        reqwest::Method::PUT,
        &format!("/accounts/{account}/workers/scripts/{SCRIPT}/secrets"),
        Some(serde_json::json!({ "name": "WITNESS_KEY", "text": B64.encode(seed), "type": "secret_text" })),
    )
    .await?;
    eprintln!("== the witness has its key; it is kept nowhere else");
    call(
        &http,
        &token,
        reqwest::Method::PUT,
        &format!("/accounts/{account}/workers/domains"),
        Some(serde_json::json!({
            "hostname": format!("control.{domain}"),
            "service": SCRIPT,
            "environment": "production",
            "zone_id": zone_id,
        })),
    )
    .await?;
    eprintln!("== the relay answers at wss://control.{domain}/relay");
    eprintln!("Left: commit fleet/control.json and fleet/control-worker.json, and release.");
    Ok(())
}
