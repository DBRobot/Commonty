//! What the boxes are doing, for the pages that show it. Every box writes
//! its facts to its own prometheus (the agent's release, the backup's last
//! good run, what the hardware is), which answers only on the box itself;
//! thanos on the observing box gathers them all with a `box` label. This
//! asks thanos, on this same box, the questions `dd release status` asks,
//! and hands the answers to a browser. No box is asked by another.

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

pub async fn look(fleet: &Fleet, thanos: Option<&str>) -> Vec<Status> {
    if let Ok(g) = LAST.lock()
        && let Some((at, v)) = g.as_ref()
        && at.elapsed() < KEEP
    {
        return v.clone();
    }
    let out = ask(fleet, thanos).await;
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
