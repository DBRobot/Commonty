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
    // Without a fresh fleet count, what members keep elsewhere is not known:
    // Photos' limits are left as they are rather than loosened to what an
    // empty count would allow. This box's own counts still go out.
    let fresh = view["updated"].as_u64().is_some_and(|t| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|n| n.as_secs().saturating_sub(t) < 3600)
            .unwrap_or(false)
    });
    if !fresh {
        eprintln!("no fresh fleet count: Photos' limits left as they are");
    }
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
            // suspended: what they keep, and no more
            let want = if members[name]["suspended"].as_bool() == Some(true) {
                used_photos.max(1)
            } else {
                photos_limit(l.budget, libraries + code + passwords, used_photos)
            };
            if fresh && id != 0 && now != want {
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
    let tmp = l.facts_out.with_extension("tmp");
    std::fs::write(&tmp, usage_facts(&counted))?;
    std::fs::rename(&tmp, &l.facts_out)?;
    eprintln!("counted {} members", counted.len());
    Ok(())
}

/// The heartbeat's lines, `usage/<service>/<member> <bytes>`: not a
/// Prometheus metric, so no member's figures reach the fleet's metrics
fn usage_facts(counted: &serde_json::Map<String, Value>) -> String {
    let mut facts = String::from("# what members keep here (dd storage-ledger)\n");
    for (name, c) in counted {
        for k in ["photos", "libraries"] {
            facts.push_str(&format!(
                "usage/{k}/{name} {}\n",
                c[k].as_u64().unwrap_or(0)
            ));
        }
    }
    facts
}

/// The fleet's count, from the control plane's agreed state: every box's
/// `usage/<service>/<member>` added up, for this box's gate. A service on
/// two boxes is counted on both, as it keeps things on both.
pub async fn view(
    control: &str,
    budget: u64,
    roles: &std::path::Path,
    out: &std::path::Path,
) -> Result<()> {
    // which box may count what: from the signed release's list of boxes,
    // never from what a box says of itself
    let roles: BTreeMap<String, Vec<String>> =
        serde_json::from_slice(&std::fs::read(roles).with_context(|| roles.display().to_string())?)
            .context("the boxes' roles")?;
    let asked = async {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?
            .get(format!("{}/state", control.trim_end_matches('/')))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await
            .context("the control plane's state")
    };
    // the control plane not answering (restarting under a switch) is next
    // time: the last count stands, and a failure here would fail the switch
    let s = match asked.await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("the control plane did not answer ({e:#}); counting next time");
            return Ok(());
        }
    };
    let doc = sum(&s, budget, &roles)?;
    // whole or not at all: the gate reads it while this writes
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&doc)?)?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}

/// What each kind of count is, and the role a box must have, in the
/// release's list, for its count of that kind to be believed. A box can only
/// add to the count - nothing it says lowers anyone's - and only for what it
/// runs: one that claims to count Photos without running Photos is not heard.
const KINDS: [(&str, &str); 4] = [
    ("photos", "photos"),
    ("libraries", "photos"),
    ("code", "forge"),
    ("passwords", "vault"),
];

fn sum(state: &Value, budget: u64, roles: &BTreeMap<String, Vec<String>>) -> Result<Value> {
    let mut members: BTreeMap<String, BTreeMap<&str, u64>> = BTreeMap::new();
    for (name, b) in state["boxes"].as_object().into_iter().flatten() {
        let has = |role: &str| roles.get(name).is_some_and(|r| r.iter().any(|x| x == role));
        for (k, v) in b["facts"].as_object().into_iter().flatten() {
            let mut parts = k.splitn(3, '/');
            let (Some("usage"), Some(svc), Some(member)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some((kind, _)) = KINDS.iter().find(|(x, role)| *x == svc && has(role)) else {
                continue;
            };
            // a count is a whole number of bytes; anything else counts nothing
            let n = match v.as_f64() {
                Some(f) if f.is_finite() && f > 0.0 => f.min(u64::MAX as f64) as u64,
                _ => 0,
            };
            let e = members
                .entry(member.to_string())
                .or_default()
                .entry(kind)
                .or_default();
            *e = e.saturating_add(n);
        }
    }
    let members: serde_json::Map<String, Value> = members
        .into_iter()
        .map(|(m, c)| {
            let mut o = serde_json::Map::new();
            for (k, _) in KINDS {
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
        let roles = std::collections::BTreeMap::from([
            (
                "node1".to_string(),
                vec!["photos".to_string(), "forge".to_string()],
            ),
            (
                "node2".to_string(),
                vec!["forge".to_string(), "vault".to_string()],
            ),
        ]);
        let v = super::sum(&state, 1000, &roles).unwrap();
        assert_eq!(v["budget"], 1000);
        assert_eq!(v["members"]["tom"]["photos"], 100);
        assert_eq!(v["members"]["tom"]["libraries"], 20);
        assert_eq!(v["members"]["tom"]["code"], 12);
        assert_eq!(v["members"]["tom"]["passwords"], 0);
        assert_eq!(v["members"]["sarah"]["passwords"], 3);
    }

    /// A box counts only what the release says it runs, and no number it
    /// sends is more than bytes: not negative, not a NaN, not past the top
    #[test]
    fn a_box_counts_only_what_it_runs() {
        let state = serde_json::json!({ "boxes": {
            "node2": { "facts": { "usage/photos/tom": 999.0, "usage/code/tom": -5.0 } },
            "node3": { "facts": { "usage/code/sarah": 1e30 } },
            "node4": { "facts": { "usage/code/sarah": 1e30 } }
        }});
        let roles = std::collections::BTreeMap::from([
            ("node2".to_string(), vec!["forge".to_string()]),
            ("node3".to_string(), vec!["forge".to_string()]),
            ("node4".to_string(), vec!["forge".to_string()]),
        ]);
        let v = super::sum(&state, 1000, &roles).unwrap();
        // node2 runs no Photos; its -5 counts nothing
        assert_eq!(v["members"]["tom"]["photos"], 0);
        assert_eq!(v["members"]["tom"]["code"], 0);
        // two huge counts add up to the top, not round past it
        assert_eq!(v["members"]["sarah"]["code"], u64::MAX);
    }
}

pub(crate) fn urlencode(s: &str) -> String {
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

    #[test]
    fn usage_facts_are_not_metrics() {
        let counted = serde_json::json!({ "tom": { "photos": 5, "libraries": 2 } });
        let f = super::usage_facts(counted.as_object().unwrap());
        assert!(f.contains("usage/photos/tom 5\n"), "{f}");
        assert!(f.contains("usage/libraries/tom 2\n"), "{f}");
        assert!(!f.contains("dd_usage") && !f.contains("member="), "{f}");
    }
}
