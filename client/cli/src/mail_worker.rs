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
pub(crate) fn api() -> String {
    std::env::var("DD_CF_API").unwrap_or_else(|_| API.to_string())
}
/// keyring entry: Workers Scripts Edit, Workers KV Edit, Workers Routes
/// Edit and Zone Read, for this zone's account
const TOKEN: &str = "cloudflare-workers";
const SCRIPT: &str = "commonty-mail";
const KV_TITLE: &str = "commonty-mail-pinned";
// the same date the end-to-end check runs it at (nix/tests/mail-worker.nix)
const COMPATIBILITY: &str = "2026-05-01";
/// where receivers find the domain's DKIM key: cm1._domainkey.<domain>
pub(crate) const DKIM_SELECTOR: &str = "cm1";

/// fleet/mail.json: where the Worker is. Nothing secret.
#[derive(Serialize, Deserialize)]
pub struct Setup {
    pub domain: String,
    pub account: String,
    pub zone: String,
    pub kv: String,
    pub script: String,
    /// the public half of the Worker's directory read key (`dd mail
    /// reader-key`): boxes show single entries to it and to no stranger
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader: Option<String>,
    /// the public half of the domain's DKIM key (`dd mail dkim`), which the
    /// DNS Worker publishes at <selector>._domainkey; its secret half is
    /// the Worker's DKIM_KEY and nowhere else
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dkim: Option<Dkim>,
}

#[derive(Serialize, Deserialize)]
pub struct Dkim {
    pub selector: String,
    /// base64 SubjectPublicKeyInfo
    pub public: String,
}

pub(crate) fn token(keys: &impl auth::KeyStore) -> Result<String> {
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

pub(crate) async fn call(
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
        "worker.js" | "dd_mail.js" | "fleet.js" | "unlock.js" | "leaving.js" | "send.js"
        | "aliases.js" | "identity.js" | "inbox.js" | "dkim.js" | "mail-hpke.js"
        | "mail-mime.js" | "mail-ids.js" | "mail-object.js" => "application/javascript+module",
        n if n.ends_with(".wasm") => "application/wasm",
        n if n.ends_with(".woff2") => "application/octet-stream",
        _ => "text/plain",
    }
}

/// the Durable Object migration the script has had, so the Hold
/// (inbox.js) is made by the first upload that has it and never again
const HOLD_TAG: &str = "hold-v1";
async fn migration_tag(http: &reqwest::Client, token: &str, s: &Setup) -> Result<Option<String>> {
    let v = call(
        http,
        token,
        reqwest::Method::GET,
        &format!("/accounts/{}/workers/scripts", s.account),
        None,
    )
    .await?;
    Ok(v["result"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"].as_str() == Some(&s.script)))
        .and_then(|x| x["migration_tag"].as_str())
        .map(str::to_string))
}

async fn upload(
    http: &reqwest::Client,
    token: &str,
    s: &Setup,
    dir: &Path,
    control: Option<&str>,
    identity: &str,
) -> Result<usize> {
    let mut metadata = serde_json::json!({
        "main_module": "worker.js",
        "compatibility_date": COMPATIBILITY,
        "bindings": [
            { "type": "plain_text", "name": "DOMAIN", "text": s.domain },
            // who is a member: judged here, or read from the control plane's
            // agreed state (identity.js, dns_worker::identity_from)
            { "type": "plain_text", "name": "IDENTITY_FROM", "text": identity },
            // its own name: the zone's catch-all sends members' aliases here
            { "type": "plain_text", "name": "WORKER", "text": s.script },
            { "type": "kv_namespace", "name": "PINNED", "namespace_id": s.kv },
            // the boxes' mail (send.js): only to addresses Email Routing has
            // confirmed, and only from these two
            {
                "type": "send_email",
                "name": "SEND",
                "allowed_sender_addresses": [
                    format!("noreply@{}", s.domain),
                    format!("alerts@{}", s.domain),
                ],
            },
            // members' locked mail until a box takes it (inbox.js)
            { "type": "durable_object_namespace", "name": "HOLD", "class_name": "Hold" },
            // the DKIM key's name in DNS; the key itself is the secret DKIM_KEY
            { "type": "plain_text", "name": "DKIM_SELECTOR", "text": DKIM_SELECTOR },
        ],
        // CF_TOKEN and SESSION_KEY were set once and are kept
        "keep_bindings": ["secret_text"],
    });
    // the control relay, whose witness has the agreed state (/identity)
    if let Some(script) = control {
        metadata["bindings"]
            .as_array_mut()
            .expect("bindings")
            .push(serde_json::json!({
                "type": "durable_object_namespace",
                "name": "CONTROL",
                "class_name": "Relay",
                "script_name": script,
            }));
    }
    match migration_tag(http, token, s).await? {
        Some(t) if t == HOLD_TAG => {}
        old => {
            let mut m = serde_json::json!({ "new_tag": HOLD_TAG, "new_sqlite_classes": ["Hold"] });
            if let Some(t) = old {
                m["old_tag"] = serde_json::Value::String(t);
            }
            metadata["migrations"] = m;
        }
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

/// A fresh read key: the secret straight to the Worker as DIRECTORY_KEY,
/// kept nowhere else; the public half back, for fleet/mail.json.
async fn put_reader_key(http: &reqwest::Client, token: &str, account: &str) -> Result<String> {
    let key = identity::generate();
    call(
        http,
        token,
        reqwest::Method::PUT,
        &format!("/accounts/{account}/workers/scripts/{SCRIPT}/secrets"),
        Some(serde_json::json!({
            "name": "DIRECTORY_KEY",
            "text": identity::encode_secret(&key).as_str(),
            "type": "secret_text",
        })),
    )
    .await?;
    Ok(identity::encode_public(&key.verifying_key()))
}

/// `dd mail reader-key`: a new read key for the Worker that is set up
/// already. The old one stops at the release that names the new one.
pub async fn reader_key(root: &Path, keys: &impl auth::KeyStore) -> Result<()> {
    let Some(mut s) = read_setup(root)? else {
        bail!("no mail Worker set up yet - `dd mail setup`");
    };
    let token = token(keys)?;
    s.reader = Some(put_reader_key(&reqwest::Client::new(), &token, &s.account).await?);
    std::fs::write(
        root.join("fleet/mail.json"),
        serde_json::to_string_pretty(&s)? + "\n",
    )?;
    eprintln!(
        "== the Worker signs its directory reads with the new key; fleet/mail.json names it.\n\
         Commit it and publish a release: from then on the boxes show an entry to\n\
         members, its own keys and the Worker, and to nobody else."
    );
    Ok(())
}

/// `dd mail dkim`: the domain's DKIM key, made here. The secret goes
/// straight to the Worker as DKIM_KEY and is kept nowhere else; the public
/// half goes into fleet/mail.json, which the next release's DNS Worker
/// publishes. Made again, the old key stops verifying once that is out.
/// It also makes MAIL_SIGN, the key the Worker signs locked mail with; a
/// new one leaves the old ones believed (the Worker keeps their public
/// halves).
pub async fn dkim(root: &Path, keys: &impl auth::KeyStore) -> Result<()> {
    use rsa::pkcs8::{EncodePrivateKey as _, EncodePublicKey as _};
    let Some(mut s) = read_setup(root)? else {
        bail!("no mail Worker set up yet - `dd mail setup`");
    };
    let token = token(keys)?;
    eprintln!("making a 2048-bit RSA key ...");
    let key = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048)?;
    let secret = zeroize::Zeroizing::new(
        base64::engine::general_purpose::STANDARD.encode(key.to_pkcs8_der()?.as_bytes()),
    );
    let public = base64::engine::general_purpose::STANDARD
        .encode(key.to_public_key().to_public_key_der()?.as_bytes());
    call(
        &reqwest::Client::new(),
        &token,
        reqwest::Method::PUT,
        &format!("/accounts/{}/workers/scripts/{}/secrets", s.account, s.script),
        Some(serde_json::json!({ "name": "DKIM_KEY", "text": secret.as_str(), "type": "secret_text" })),
    )
    .await?;
    // and the key the Worker signs the mail it locks with (inbox.js): an
    // Ed25519 seed as PKCS#8, made here, to the Worker alone. Its public
    // half the Worker keeps itself and tells members' devices
    let mut seed = zeroize::Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut seed[..]).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut pkcs8 = zeroize::Zeroizing::new(vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ]);
    pkcs8.extend_from_slice(&seed[..]);
    let sign =
        zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(&pkcs8[..]));
    call(
        &reqwest::Client::new(),
        &token,
        reqwest::Method::PUT,
        &format!("/accounts/{}/workers/scripts/{}/secrets", s.account, s.script),
        Some(serde_json::json!({ "name": "MAIL_SIGN", "text": sign.as_str(), "type": "secret_text" })),
    )
    .await?;
    s.dkim = Some(Dkim {
        selector: DKIM_SELECTOR.into(),
        public,
    });
    std::fs::write(
        root.join("fleet/mail.json"),
        serde_json::to_string_pretty(&s)? + "\n",
    )?;
    eprintln!(
        "== the Worker signs members' mail with the new key; fleet/mail.json has its public half.\n\
         It also signs the mail it locks with a new key of its own, which members' devices learn from it.\n\
         Commit it and publish a release: the DNS Worker then publishes {DKIM_SELECTOR}._domainkey.{} and,\n\
         when the domain has none, a DMARC record that sends mail claiming the domain unsigned to spam.",
        s.domain
    );
    Ok(())
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
    let control = crate::dns_worker::control_script(root)?;
    let identity = crate::dns_worker::identity_from(root, control.as_deref())?;
    let n = upload(
        &reqwest::Client::new(),
        &token,
        &s,
        &dir,
        control.as_deref(),
        &identity,
    )
    .await?;
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
    // given in the environment for setup: kept on the keyring for publish
    if let Ok(t) = std::env::var("CLOUDFLARE_API_TOKEN") {
        keys.set(TOKEN, t.trim())?;
    }
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
        reader: None,
        dkim: None,
    };
    std::fs::write(
        root.join("fleet/mail.json"),
        serde_json::to_string_pretty(&s)? + "\n",
    )?;
    eprintln!("== wrote fleet/mail.json");

    let dir = bundle(root, r#ref)?;
    let control = crate::dns_worker::control_script(root)?;
    let identity = crate::dns_worker::identity_from(root, control.as_deref())?;
    upload(&http, &token, &s, &dir, control.as_deref(), &identity).await?;
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
    // or from the environment, where nothing is kept
    let routing = match std::env::var("CF_EMAIL_ROUTING_TOKEN") {
        Ok(t) => t,
        Err(_) => rpassword::prompt_password("Email Routing token: ")?,
    };
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
    let mut s = s;
    s.reader = Some(put_reader_key(&http, &token, &account).await?);
    std::fs::write(
        root.join("fleet/mail.json"),
        serde_json::to_string_pretty(&s)? + "\n",
    )?;
    eprintln!("== the Worker has its directory read key; fleet/mail.json names it");

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
