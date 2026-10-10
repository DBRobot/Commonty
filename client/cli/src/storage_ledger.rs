//! One allowance per member, across every service and every box.
//!
//! Each box says what its members keep in the services it runs, in its own
//! heartbeat (`usage/<service>/<member>`, dd-control): the forge and the
//! vault count themselves (nix/modules/storage/usage.nix), and the box with
//! Photos counts Photos and the libraries here (`dd storage-ledger`: it
//! holds a Photos admin login the gate never sees). Every box adds those up
//! from the control plane's agreed state (`dd storage-view`) for its gate,
//! which shows it and holds uploads to it; and the Photos box sets each
//! member's Photos limit to what the rest leaves of their allowance. No box
//! reads another, and where a service runs does not matter.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

pub struct Ledger {
    pub gate: String,
    pub ente_origin: String,
    pub password_file: PathBuf,
    pub photos_suffix: String,
    pub budget: u64,
    /// the fleet's count (storage-view's), for what each member keeps
    /// elsewhere
    pub view: PathBuf,
    /// where this box's own counts go, for its heartbeat
    pub facts_out: PathBuf,
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
    // what the fleet counted last, for what each keeps outside Photos and
    // the libraries (this box counts those two itself, just below)
    let view: Value = std::fs::read(&l.view)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
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
        let elsewhere = |k: &str| view["members"][name][k].as_u64().unwrap_or(0);
        let code = elsewhere("code");
        let passwords = elsewhere("passwords");
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

    // this box's own word, for its heartbeat: Photos and the libraries,
    // each member's, as the fleet adds them up
    let mut facts = String::from("# what members keep here (dd storage-ledger)\n");
    for (name, c) in &counted {
        for k in ["photos", "libraries"] {
            facts.push_str(&format!(
                "dd_usage{{service=\"{k}\",member=\"{name}\"}} {}\n",
                c[k].as_u64().unwrap_or(0)
            ));
        }
    }
    let tmp = l.facts_out.with_extension("tmp");
    std::fs::write(&tmp, facts)?;
    std::fs::rename(&tmp, &l.facts_out)?;
    eprintln!("counted {} members", counted.len());
    Ok(())
}

/// The fleet's count, from the control plane's agreed state: every box's
/// `usage/<service>/<member>` added up, for this box's gate. A service on
/// two boxes is counted on both, as it keeps things on both.
pub async fn view(control: &str, budget: u64, out: &std::path::Path) -> Result<()> {
    let s: Value = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?
        .get(format!("{}/state", control.trim_end_matches('/')))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("the control plane's state")?;
    let doc = sum(&s, budget)?;
    // whole or not at all: the gate reads it while this writes
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&doc)?)?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}

fn sum(state: &Value, budget: u64) -> Result<Value> {
    const KINDS: [&str; 4] = ["photos", "libraries", "code", "passwords"];
    let mut members: BTreeMap<String, BTreeMap<&str, u64>> = BTreeMap::new();
    for b in state["boxes"]
        .as_object()
        .into_iter()
        .flat_map(|o| o.values())
    {
        for (k, v) in b["facts"].as_object().into_iter().flatten() {
            let mut parts = k.splitn(3, '/');
            let (Some("usage"), Some(svc), Some(member)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some(kind) = KINDS.iter().find(|x| **x == svc) else {
                continue;
            };
            let n = v.as_f64().unwrap_or(0.0).max(0.0) as u64;
            *members
                .entry(member.to_string())
                .or_default()
                .entry(kind)
                .or_default() += n;
        }
    }
    let members: serde_json::Map<String, Value> = members
        .into_iter()
        .map(|(m, c)| {
            let mut o = serde_json::Map::new();
            for k in KINDS {
                o.insert(k.into(), json!(c.get(k).copied().unwrap_or(0)));
            }
            (m, Value::Object(o))
        })
        .collect();
    Ok(json!({
        "updated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "budget": budget,
        "members": members,
    }))
}

#[cfg(test)]
mod view_tests {
    /// Every box's counts added up per member, whatever box a service is
    /// on, and anything that is not a count left out
    #[test]
    fn the_fleet_adds_up() {
        let state = serde_json::json!({ "boxes": {
            "node1": { "facts": { "usage/photos/tom": 100.0, "usage/libraries/tom": 20.0, "usage/code/tom": 5.0, "cores": 8.0 } },
            "node2": { "facts": { "usage/code/tom": 7.0, "usage/passwords/sarah": 3.0, "usage/weird/tom": 9.0 } }
        }});
        let v = super::sum(&state, 1000).unwrap();
        assert_eq!(v["budget"], 1000);
        assert_eq!(v["members"]["tom"]["photos"], 100);
        assert_eq!(v["members"]["tom"]["libraries"], 20);
        assert_eq!(v["members"]["tom"]["code"], 12);
        assert_eq!(v["members"]["tom"]["passwords"], 0);
        assert_eq!(v["members"]["sarah"]["passwords"], 3);
    }
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
