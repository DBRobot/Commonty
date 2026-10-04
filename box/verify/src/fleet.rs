//! What the boxes are doing, for the pages that show it. Every box puts its
//! release, how its agent's last run ended and its last good backup in its
//! signed heartbeat, and the control plane agrees on them; this reads the
//! agreed state from this box's own dd-control. Where there is no control
//! plane it asks thanos on this box, as before. No box is asked by another.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

/// One box, as its own prometheus describes it. Every field is optional:
/// a box that is off answers nothing and is still a box.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Status {
    pub name: String,
    /// false when its prometheus did not answer at all
    pub up: bool,
    pub release: Option<u64>,
    /// what the agent's last run came to: "ok", "rolled back", …
    pub result: Option<String>,
    /// the backup's own facts (modules/storage/backup.nix)
    pub backup: Option<Backup>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Backup {
    pub last_success: Option<u64>,
}

/// the boxes, from the release (the addresses are not used: nothing here
/// talks to another box)
pub type Fleet = BTreeMap<String, String>;

/// every sample of one metric for one box, as (labels, value)
async fn query(
    http: &reqwest::Client,
    thanos: &str,
    name: &str,
    metric: &str,
) -> Option<Vec<(BTreeMap<String, String>, f64)>> {
    let expr = format!("{metric}{{box=\"{name}\"}}");
    let r = http
        .get(format!("{thanos}/api/v1/query"))
        .timeout(Duration::from_secs(5))
        .query(&[("query", expr.as_str())])
        .send()
        .await
        .ok()?;
    let v: serde_json::Value = r.json().await.ok()?;
    Some(
        v["data"]["result"]
            .as_array()?
            .iter()
            .map(|s| {
                let labels = s["metric"]
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                let value = s["value"][1]
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                (labels, value)
            })
            .collect(),
    )
}

async fn one(http: &reqwest::Client, name: &str, thanos: &str) -> Status {
    let mut b = Status {
        name: name.to_string(),
        ..Default::default()
    };
    let num = |r: &Option<Vec<(BTreeMap<String, String>, f64)>>| {
        r.as_ref().and_then(|s| s.first()).map(|(_, v)| *v as u64)
    };
    // what the pages show (Network, the Git page): every question at once
    let (agent, counter, info, last) = tokio::join!(
        query(http, thanos, name, "dd_agent_info"),
        query(http, thanos, name, "dd_agent_counter"),
        query(http, thanos, name, "dd_box_info"),
        query(http, thanos, name, "dd_backup_last_success_seconds"),
    );
    // up: thanos has current facts from it; none, and it is off or unheard
    let any = |r: &Option<Vec<(BTreeMap<String, String>, f64)>>| {
        r.as_ref().is_some_and(|s| !s.is_empty())
    };
    b.up = any(&agent) || any(&info);
    if !b.up {
        return b;
    }
    if let Some(s) = &agent
        && let Some((l, _)) = s.first()
    {
        b.result = l.get("result").cloned();
    }
    b.release = num(&counter);
    let last = num(&last);
    if last.is_some() {
        b.backup = Some(Backup { last_success: last });
    }
    b
}

/// what the boxes said, for a few seconds: pages opened together (the
/// Git page's "Running now", Boxes, Backups) share one round of questions
static LAST: std::sync::Mutex<Option<(std::time::Instant, Vec<Status>)>> =
    std::sync::Mutex::new(None);
const KEEP: std::time::Duration = std::time::Duration::from_secs(15);

/// a box unheard this long is down (the control plane's own goneAfter is 90 s)
const GONE_MS: i64 = 120_000;

/// every box from the agreed state: its heartbeat's facts, and whether it
/// has been heard lately by agreed time
async fn from_control(fleet: &Fleet, control: &str) -> Option<Vec<Status>> {
    let s: serde_json::Value = crate::http()
        .get(format!("{}/state", control.trim_end_matches('/')))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let now = s["time"].as_i64()?;
    Some(
        fleet
            .keys()
            .map(|name| {
                let b = &s["boxes"][name];
                let f = &b["facts"];
                let seen = b["lastSeen"].as_i64().unwrap_or(0);
                Status {
                    name: name.clone(),
                    up: seen > 0 && now - seen <= GONE_MS,
                    release: f["release"].as_f64().map(|v| v as u64),
                    result: f["agent_ok"]
                        .as_f64()
                        .map(|v| if v >= 1.0 { "ok" } else { "refused" }.to_string()),
                    backup: f["backup_last"].as_f64().map(|v| Backup {
                        last_success: Some(v as u64),
                    }),
                }
            })
            .collect(),
    )
}

pub async fn look(fleet: &Fleet, thanos: Option<&str>, control: Option<&str>) -> Vec<Status> {
    if let Ok(g) = LAST.lock()
        && let Some((at, v)) = g.as_ref()
        && at.elapsed() < KEEP
    {
        return v.clone();
    }
    let agreed = match control {
        Some(c) => from_control(fleet, c).await,
        None => None,
    };
    let out = match agreed {
        Some(v) => v,
        None => ask(fleet, thanos).await,
    };
    if let Ok(mut g) = LAST.lock() {
        *g = Some((std::time::Instant::now(), out.clone()));
    }
    out
}

/// every box at once: one slow box does not hold up the page
async fn ask(fleet: &Fleet, thanos: Option<&str>) -> Vec<Status> {
    let http = crate::http();
    let mut set = tokio::task::JoinSet::new();
    for name in fleet.keys() {
        let (http, name, thanos) = (http.clone(), name.clone(), thanos.map(str::to_string));
        set.spawn(async move {
            match thanos {
                Some(t) => one(&http, &name, &t).await,
                // this box does not gather the fleet's facts: every box unknown
                None => Status {
                    name,
                    ..Default::default()
                },
            }
        });
    }
    let mut out: Vec<Status> = set.join_all().await;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
