//! `dd storage-ledger`: one allowance per member, across every service.
//! Counted on the box every ten minutes, by a unit of its own that holds a
//! Photos admin login the gate never sees: each member's Photos (museum's
//! admin api), their libraries (the gate lists the bucket), Code (the
//! forge's repository sizes) and Passwords (attachments). Written down for
//! the gate, which shows it and holds uploads to it, and each member's
//! Photos limit set to what the rest leaves of their allowance.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

pub struct Ledger {
    pub gate: String,
    pub ente_origin: String,
    pub photos_suffix: String,
    pub vault_suffix: String,
    pub password_file: PathBuf,
    pub budget: u64,
    pub out: PathBuf,
}

/// name -> bytes, from a query whose rows are "<key>\t<bytes>"
fn psql(db: &str, query: &str) -> Result<BTreeMap<String, u64>> {
    let o = std::process::Command::new("psql")
        .args(["-X", "-tA", "-F", "\t", "-d", db, "-c", query])
        .output()
        .with_context(|| format!("psql {db}"))?;
    if !o.status.success() {
        return Err(anyhow!(
            "psql {db}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('\t')?;
            Some((k.to_string(), v.trim().parse().ok()?))
        })
        .collect())
}

/// Photos' limit for a member: what the rest of their keeping leaves of the
/// allowance. Never nothing at all - museum wants a number, and one byte is
/// its "no more" - and never below what they already keep there, which
/// would say they must delete rather than that they may not add.
pub fn photos_limit(budget: u64, others: u64, photos: u64) -> u64 {
    budget.saturating_sub(others).max(photos).max(1)
}

pub async fn run(l: Ledger) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let members: BTreeMap<String, Value> = http
        .get(format!(
            "{}/internal/storage/members",
            l.gate.trim_end_matches('/')
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("the gate's list of members")?;
    let code = psql(
        "forgejo",
        "select owner_name, coalesce(sum(size + lfs_size), 0) from repository group by owner_name",
    )?;
    let passwords = psql(
        "vaultwarden",
        "select u.email, coalesce(sum(a.file_size), 0) from attachments a \
         join ciphers c on c.uuid = a.cipher_uuid join users u on u.uuid = c.user_uuid \
         group by u.email",
    )?;

    let password = std::fs::read_to_string(&l.password_file)
        .context("the ledger's photo password")?
        .trim()
        .to_string();
    let me = format!("storage+ledger{}", l.photos_suffix);
    let photos = ente::gallery::Gallery::sign_in(&l.ente_origin, &me, &password)
        .await
        .context("signing in to Photos as the ledger")?;
    let admin = |path: String| {
        http.get(format!("{}{path}", l.ente_origin.trim_end_matches('/')))
            .header("X-Auth-Token", photos.token())
    };

    let mut counted = serde_json::Map::new();
    for name in members.keys() {
        let libraries = members[name]["libraries"].as_u64().unwrap_or(0);
        let code = code.get(name).copied().unwrap_or(0);
        let passwords = passwords
            .get(&format!("{name}{}", l.vault_suffix))
            .copied()
            .unwrap_or(0);
        let email = format!("{name}{}", l.photos_suffix);
        let r = admin(format!("/admin/user?email={}", urlencode(&email)))
            .send()
            .await?;
        let mut used_photos = 0;
        if r.status().is_success() {
            let u: Value = r.json().await?;
            used_photos = u["details"]["usage"].as_u64().unwrap_or(0);
            // museum's user has no json name for its id: it goes out as "ID"
            let id = u["user"]["ID"]
                .as_i64()
                .or(u["user"]["id"].as_i64())
                .unwrap_or(0);
            let now = u["subscription"]["storage"].as_u64().unwrap_or(0);
            let want = photos_limit(l.budget, libraries + code + passwords, used_photos);
            if id != 0 && now != want {
                let r = http
                    .put(format!(
                        "{}/admin/user/subscription",
                        l.ente_origin.trim_end_matches('/')
                    ))
                    .header("X-Auth-Token", photos.token())
                    .json(&json!({
                        "userID": id,
                        "storage": want,
                        "transactionID": "dd-storage",
                        "productID": "free",
                        "expiryTime": 4102444800000000i64,
                        "paymentProvider": "",
                    }))
                    .send()
                    .await?;
                if r.status().is_success() {
                    eprintln!("{name}: photos limit {now} -> {want} bytes");
                } else {
                    eprintln!("{name}: museum refused the photos limit: {}", r.status());
                }
            }
        } else if r.status() != reqwest::StatusCode::NOT_FOUND {
            eprintln!("{name}: photos usage: {}", r.status());
        }
        counted.insert(
            name.clone(),
            json!({ "photos": used_photos, "libraries": libraries, "code": code, "passwords": passwords }),
        );
    }

    let doc = json!({
        "updated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "budget": l.budget,
        "members": counted,
    });
    // whole or not at all: the gate reads it while this writes
    let tmp = l.out.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&doc)?)?;
    std::fs::rename(&tmp, &l.out)?;
    eprintln!("counted {} members", counted.len());
    Ok(())
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::photos_limit;

    #[test]
    fn photos_gets_what_the_rest_leaves() {
        let gb = 1_000_000_000;
        assert_eq!(photos_limit(200 * gb, 2 * gb, 78 * gb), 198 * gb);
        // the rest already fills it: no more photos, and none taken away
        assert_eq!(photos_limit(200 * gb, 210 * gb, 5 * gb), 5 * gb);
        assert_eq!(photos_limit(200 * gb, 210 * gb, 0), 1);
    }
}
