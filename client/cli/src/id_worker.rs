//! The id Worker at Cloudflare (servers/system/id): sign-in, once for every
//! box, at id.<domain> (identity::session). Static pages and nothing else:
//! no secret, no store. Set up once with `dd id setup`, uploaded by
//! `dd release publish` from the release it ships with, beside the mail
//! Worker and with the same token.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::mail_worker::{api, call, token};

const SCRIPT: &str = "commonty-id";
const COMPATIBILITY: &str = "2026-05-01";

/// fleet/id-worker.json: where the Worker is. Nothing secret. Its being
/// there is what moves the boxes' sign-in onto it (dd.idWorker).
#[derive(Serialize, Deserialize)]
pub struct Setup {
    pub domain: String,
    pub account: String,
    pub zone: String,
    pub script: String,
    /// "try": nobody's sign-in changes until it says "on" (dd.idWorkerStage)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

fn read_setup(root: &Path) -> Result<Option<Setup>> {
    let p = root.join("fleet/id-worker.json");
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
            &format!("{flake}#id-worker"),
        ])
        .output()?;
    ensure!(
        out.status.success(),
        "building the id Worker: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().into())
}

fn kind(name: &str) -> &'static str {
    match name {
        "worker.js" => "application/javascript+module",
        n if n.ends_with(".woff2") => "application/octet-stream",
        _ => "text/plain",
    }
}

async fn upload(http: &reqwest::Client, token: &str, s: &Setup, dir: &Path) -> Result<()> {
    let metadata = serde_json::json!({
        "main_module": "worker.js",
        "compatibility_date": COMPATIBILITY,
        "bindings": [
            { "type": "plain_text", "name": "DOMAIN", "text": s.domain },
        ],
    });
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
        bail!("Cloudflare did not take the id Worker: {}", v["errors"]);
    }
    Ok(())
}

/// Upload the Worker the release at `ref` carries. Nothing to do before
/// `dd id setup` has been run.
pub async fn deploy(root: &Path, keys: &impl auth::KeyStore, r#ref: &str) -> Result<()> {
    let Some(s) = read_setup(root)? else {
        eprintln!("== no id Worker set up yet (`dd id setup`); nothing uploaded");
        return Ok(());
    };
    let dir = bundle(&format!("git+file://{}?ref={ref}", root.display()))?;
    upload(&reqwest::Client::new(), &token(keys)?, &s, &dir).await?;
    eprintln!("== the id Worker is {}", dir.display());
    Ok(())
}

/// Once: the first upload and the name id.<domain>, then
/// fleet/id-worker.json - committed and released, that lets the Worker be
/// tried; its stage "on" moves every box's sign-in onto it.
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
        stage: Some("try".into()),
    };
    let dir = bundle(&format!("path:{}", root.display()))?;
    upload(&http, &token, &s, &dir).await?;
    eprintln!("== uploaded the id Worker");
    call(
        &http,
        &token,
        reqwest::Method::PUT,
        &format!("/accounts/{account}/workers/domains"),
        Some(serde_json::json!({
            "hostname": format!("id.{domain}"),
            "service": SCRIPT,
            "environment": "production",
            "zone_id": zone_id,
        })),
    )
    .await?;
    eprintln!("== the Worker answers at https://id.{domain}");
    std::fs::write(
        root.join("fleet/id-worker.json"),
        serde_json::to_string_pretty(&s)? + "\n",
    )?;
    eprintln!("== wrote fleet/id-worker.json");
    eprintln!(
        "Left: commit fleet/id-worker.json and release; a sign-in is then there to try at https://home.{domain}/_dd/sso/start. Setting its stage to \"on\" and releasing moves every sign-in there, and ends sessions held today."
    );
    Ok(())
}
