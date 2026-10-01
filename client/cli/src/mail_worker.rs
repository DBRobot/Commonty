//! The mail Worker at Cloudflare (client/mail): set up once with `dd mail
//! setup`, and uploaded by `dd release publish` from the release it ships
//! with. Its token lives on this machine's keyring and nowhere else; the
//! Email Routing token it uses is handed straight to Cloudflare as a Worker
//! secret and kept nowhere.

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::Path;

const API: &str = "https://api.cloudflare.com/client/v4";

/// Cloudflare's API, or a stand-in for a test (DD_CF_API)
fn api() -> String {
    std::env::var("DD_CF_API").unwrap_or_else(|_| API.to_string())
}
/// keyring entry: Workers Scripts Edit, Workers KV Edit, Workers Routes
/// Edit and Zone Read, for this zone's account
const TOKEN: &str = "cloudflare-workers";
const SCRIPT: &str = "commonty-mail";
const KV_TITLE: &str = "commonty-mail-pinned";
const COMPATIBILITY: &str = "2026-09-01";

/// fleet/mail.json: where the Worker is. Nothing secret.
#[derive(Serialize, Deserialize)]
pub struct Setup {
    pub domain: String,
    pub account: String,
    pub zone: String,
    pub kv: String,
    pub script: String,
}

fn token(keys: &impl auth::KeyStore) -> Result<String> {
    // the name wrangler reads it under, for a one-off
    if let Ok(t) = std::env::var("CLOUDFLARE_API_TOKEN") {
        return Ok(t);
    }
    if let Some(t) = keys.get(TOKEN)? {
        return Ok(t.as_str().to_string());
    }
    eprintln!(
        "A Cloudflare API token for the mail Worker is needed once. Make one with:\n  \
         Account: Workers Scripts Edit, Workers KV Storage Edit\n  \
         Zone: Workers Routes Edit, Zone Read (this zone)\n\
         It stays on this machine's keyring."
    );
    let t = rpassword::prompt_password("Workers token: ")?;
    ensure!(!t.trim().is_empty(), "no token given");
    keys.set(TOKEN, t.trim())?;
    Ok(t.trim().to_string())
}

async fn call(
    http: &reqwest::Client,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let mut req = http
        .request(method, format!("{}{path}", api()))
        .bearer_auth(token);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let v: serde_json::Value = req.send().await?.json().await?;
    if v["success"] != serde_json::Value::Bool(true) {
        bail!("Cloudflare said no to {path}: {}", v["errors"]);
    }
    Ok(v)
}

fn read_setup(root: &Path) -> Result<Option<Setup>> {
    let p = root.join("fleet/mail.json");
    if !p.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&std::fs::read(p)?)?))
}

/// The Worker's files as the release at `ref` builds them.
fn bundle(root: &Path, r#ref: &str) -> Result<std::path::PathBuf> {
    let flake = format!("git+file://{}?ref={ref}#mail-worker", root.display());
    let out = std::process::Command::new("nix")
        .args(["build", "--no-link", "--print-out-paths", &flake])
        .output()?;
    ensure!(
        out.status.success(),
        "building the mail Worker from {ref}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().into())
}

/// How each file goes up: the Worker's own modules, the pages' scripts and
/// styles as text it serves, the fonts as bytes, the checks as wasm.
fn kind(name: &str) -> &'static str {
    match name {
        "worker.js" | "dd_mail.js" | "fleet.js" => "application/javascript+module",
        n if n.ends_with(".wasm") => "application/wasm",
        n if n.ends_with(".woff2") => "application/octet-stream",
        _ => "text/plain",
    }
}

async fn upload(http: &reqwest::Client, token: &str, s: &Setup, dir: &Path) -> Result<usize> {
    let metadata = serde_json::json!({
        "main_module": "worker.js",
        "compatibility_date": COMPATIBILITY,
        "bindings": [
            { "type": "plain_text", "name": "DOMAIN", "text": s.domain },
            { "type": "kv_namespace", "name": "PINNED", "namespace_id": s.kv },
        ],
        // CF_TOKEN and SESSION_KEY were set once and are kept
        "keep_bindings": ["secret_text"],
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
        let bytes = std::fs::read(dir.join(n))?;
        form = form.part(
            n.clone(),
            reqwest::multipart::Part::bytes(bytes)
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
        bail!("Cloudflare did not take the Worker: {}", v["errors"]);
    }
    Ok(names.len())
}

/// Upload the Worker the release at `ref` carries. Nothing to do before
/// `dd mail setup` has been run.
pub async fn deploy(root: &Path, keys: &impl auth::KeyStore, r#ref: &str) -> Result<()> {
    let Some(s) = read_setup(root)? else {
        eprintln!("== no mail Worker set up yet (`dd mail setup`); nothing uploaded");
        return Ok(());
    };
    let dir = bundle(root, r#ref)?;
    let token = token(keys)?;
    let n = upload(&reqwest::Client::new(), &token, &s, &dir).await?;
    eprintln!("== the mail Worker is {} ({n} files)", dir.display());
    Ok(())
}

/// Once: the KV namespace, fleet/mail.json, the first upload, the two
/// secrets and the name mail.<domain>.
pub async fn setup(
    root: &Path,
    keys: &impl auth::KeyStore,
    domain: &str,
    r#ref: &str,
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

    let list = call(
        &http,
        &token,
        reqwest::Method::GET,
        &format!("/accounts/{account}/storage/kv/namespaces?per_page=100"),
        None,
    )
    .await?;
    let found = list["result"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|n| n["title"] == KV_TITLE)
        .and_then(|n| n["id"].as_str().map(str::to_string));
    let kv = match found {
        Some(id) => id,
        None => call(
            &http,
            &token,
            reqwest::Method::POST,
            &format!("/accounts/{account}/storage/kv/namespaces"),
            Some(serde_json::json!({ "title": KV_TITLE })),
        )
        .await?["result"]["id"]
            .as_str()
            .context("no id for the new namespace")?
            .to_string(),
    };
    let s = Setup {
        domain: domain.to_string(),
        account: account.clone(),
        zone: zone_id.clone(),
        kv,
        script: SCRIPT.to_string(),
    };
    std::fs::write(
        root.join("fleet/mail.json"),
        serde_json::to_string_pretty(&s)? + "\n",
    )?;
    eprintln!("== wrote fleet/mail.json");

    let dir = bundle(root, r#ref)?;
    upload(&http, &token, &s, &dir).await?;
    eprintln!("== uploaded the Worker");

    let secret = |name: &str, text: String| serde_json::json!({ "name": name, "text": text, "type": "secret_text" });
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    let session = base64::engine::general_purpose::STANDARD.encode(raw);
    let path = format!("/accounts/{account}/workers/scripts/{SCRIPT}/secrets");
    call(
        &http,
        &token,
        reqwest::Method::PUT,
        &path,
        Some(secret("SESSION_KEY", session)),
    )
    .await?;
    eprintln!(
        "The Email Routing token now: Zone > Email Routing Rules Edit (this zone) and\n\
         Account > Email Routing Addresses Edit. It goes straight to the Worker and is\n\
         kept nowhere else."
    );
    let routing = rpassword::prompt_password("Email Routing token: ")?;
    ensure!(!routing.trim().is_empty(), "no token given");
    call(
        &http,
        &token,
        reqwest::Method::PUT,
        &path,
        Some(secret("CF_TOKEN", routing.trim().to_string())),
    )
    .await?;
    eprintln!("== the Worker has its secrets");

    call(
        &http,
        &token,
        reqwest::Method::PUT,
        &format!("/accounts/{account}/workers/domains"),
        Some(serde_json::json!({
            "hostname": format!("mail.{domain}"),
            "service": SCRIPT,
            "environment": "production",
            "zone_id": zone_id,
        })),
    )
    .await?;
    eprintln!("== the Worker answers at https://mail.{domain}");
    eprintln!(
        "Left for you: commit fleet/mail.json; take Email Routing off the boxes' DNS token;\n\
         and for the boxes' check, a token with Workers Scripts Read only, into sops as\n\
         cloudflare-workers-read for the gateway box."
    );
    Ok(())
}
