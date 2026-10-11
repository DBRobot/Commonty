//! Leaving Commonty from a device that holds the root (`dd leave`), and
//! Photos' share of the deleting after 90 days (`dd forget-photos`, on the
//! Photos box). docs/dev/leaving.md.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// The statement, signed here, to every directory given; the first that
/// takes it puts it in the fleet's log.
pub async fn leave(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
    back: bool,
) -> Result<()> {
    let s = identity::sign_leave(
        identity::Leave {
            name: name.to_string(),
            root: identity::encode_public(&root.verifying_key()),
            at: identity::now(),
            back,
        },
        root,
    )?;
    let http = crate::who::http()?;
    let mut why = Vec::new();
    for d in dirs {
        let base = d.trim_end_matches('/');
        let base = base.strip_suffix("/_dd/directory").unwrap_or(base);
        match http
            .put(format!("{base}/_dd/leaving/{name}"))
            .json(&s)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => {
                let st = r.status();
                why.push(format!("{d}: {st} {}", r.text().await.unwrap_or_default()))
            }
            Err(e) => why.push(format!("{d}: {e}")),
        }
    }
    bail!("no directory took it:\n  {}", why.join("\n  "))
}

pub struct Photos {
    pub gate: String,
    pub ente_origin: String,
    pub photos_suffix: String,
    /// the ledger's photo account, an admin of museum (storage/ledger.nix)
    pub password_file: std::path::PathBuf,
}

/// Each member whose window is out: their Photos account deleted through
/// museum's admin api, then reported done to this box's gate. Museum
/// itself purges what the account held over the following days.
pub async fn forget_photos(p: Photos) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let gate = p.gate.trim_end_matches('/');
    let list: Value = http
        .get(format!("{gate}/internal/leaving"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("the gate's list of who left")?;
    let due: Vec<(String, String)> = list["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["due"].as_bool() == Some(true))
        .filter(|l| {
            !l["done"]
                .as_array()
                .is_some_and(|d| d.iter().any(|s| s == "photos"))
        })
        .filter_map(|l| {
            Some((
                l["name"].as_str()?.to_string(),
                l["id"].as_str()?.to_string(),
            ))
        })
        .collect();
    if due.is_empty() {
        return Ok(());
    }
    let password = std::fs::read_to_string(&p.password_file)
        .context("the ledger's photo password")?
        .trim()
        .to_string();
    let me = format!("storage+ledger{}", p.photos_suffix);
    // museum takes an admin's delete only from a token under ten minutes old
    let photos = ente::gallery::Gallery::sign_in(&p.ente_origin, &me, &password)
        .await
        .context("signing in to Photos as the ledger")?;
    let origin = p.ente_origin.trim_end_matches('/');
    for (name, id) in due {
        let email = format!("{name}{}", p.photos_suffix);
        let r = http
            .delete(format!(
                "{origin}/admin/user/delete?email={}",
                crate::storage_ledger::urlencode(&email)
            ))
            .header("X-Auth-Token", photos.token())
            .send()
            .await?;
        // never had an account, or deleted on an earlier run
        let gone = r.status().is_success() || r.status() == reqwest::StatusCode::NOT_FOUND;
        if !gone {
            eprintln!("member {id}: museum refused the delete: {}", r.status());
            continue;
        }
        eprintln!("member {id}: their Photos account is deleted");
        http.post(format!("{gate}/internal/leaving/done"))
            .json(&serde_json::json!({ "name": name, "service": "photos" }))
            .send()
            .await?
            .error_for_status()?;
    }
    Ok(())
}
