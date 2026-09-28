//! What the boxes are doing, for the pages that show it. Every box runs
//! its own prometheus on the tailnet and every box writes its facts there
//! (the agent's release, the backup's last good run, what the hardware
//! is). Nothing new is stored: this asks each box the same questions `dd
//! release status` asks, and hands the answers to a browser.

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
    pub last_run: Option<u64>,
    pub model: Option<String>,
    pub kernel: Option<String>,
    pub cores: Option<u64>,
    pub memory: Option<u64>,
    /// the backup's own facts (modules/storage/backup.nix)
    pub backup: Option<Backup>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Backup {
    pub last_success: Option<u64>,
    pub snapshots: Option<u64>,
    pub oldest: Option<u64>,
    pub newest: Option<u64>,
    pub paths: Vec<String>,
}

/// the boxes and the address each answers on, from the release
pub type Fleet = BTreeMap<String, String>;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default()
}

/// every sample of one metric, as (labels, value)
async fn query(
    http: &reqwest::Client,
    addr: &str,
    expr: &str,
) -> Option<Vec<(BTreeMap<String, String>, f64)>> {
    let r = http
        .get(format!("http://{addr}:9090/api/v1/query"))
        .query(&[("query", expr)])
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

async fn one(http: &reqwest::Client, name: &str, addr: &str) -> Status {
    let mut b = Status {
        name: name.to_string(),
        ..Default::default()
    };
    let num = |r: &Option<Vec<(BTreeMap<String, String>, f64)>>| {
        r.as_ref().and_then(|s| s.first()).map(|(_, v)| *v as u64)
    };
    let Some(info) = query(http, addr, "dd_agent_info").await else {
        return b; // nothing answered: the box is off, or the tailnet is
    };
    b.up = true;
    if let Some((l, _)) = info.first() {
        b.result = l.get("result").cloned();
    }
    // the rest all at once: one round trip to a box, not ten
    let (counter, last_run, info, cores, memory, last, paths, snapshots, oldest, newest) = tokio::join!(
        query(http, addr, "dd_agent_counter"),
        query(http, addr, "dd_agent_last_run_seconds"),
        query(http, addr, "dd_box_info"),
        query(http, addr, "dd_box_cpu_cores"),
        query(http, addr, "dd_box_memory_bytes"),
        query(http, addr, "dd_backup_last_success_seconds"),
        query(http, addr, "dd_backup_path"),
        query(http, addr, "dd_backup_snapshots"),
        query(http, addr, "dd_backup_oldest_seconds"),
        query(http, addr, "dd_backup_newest_seconds"),
    );
    b.release = num(&counter);
    b.last_run = num(&last_run);
    if let Some(s) = info
        && let Some((l, _)) = s.first()
    {
        b.model = l.get("model").cloned();
        b.kernel = l.get("kernel").cloned();
    }
    b.cores = num(&cores);
    b.memory = num(&memory);
    let last = num(&last);
    let paths: Vec<String> = paths
        .map(|s| {
            s.iter()
                .filter_map(|(l, _)| l.get("path").cloned())
                .collect()
        })
        .unwrap_or_default();
    if last.is_some() || !paths.is_empty() {
        b.backup = Some(Backup {
            last_success: last,
            snapshots: num(&snapshots),
            oldest: num(&oldest),
            newest: num(&newest),
            paths,
        });
    }
    b
}

/// what the boxes said, for a few seconds: pages opened together (the
/// Git page's "Running now", Boxes, Backups) share one round of questions
static LAST: std::sync::Mutex<Option<(std::time::Instant, Vec<Status>)>> =
    std::sync::Mutex::new(None);
const KEEP: std::time::Duration = std::time::Duration::from_secs(15);

pub async fn look(fleet: &Fleet) -> Vec<Status> {
    if let Ok(g) = LAST.lock()
        && let Some((at, v)) = g.as_ref()
        && at.elapsed() < KEEP
    {
        return v.clone();
    }
    let out = ask(fleet).await;
    if let Ok(mut g) = LAST.lock() {
        *g = Some((std::time::Instant::now(), out.clone()));
    }
    out
}

/// every box at once: one slow box does not hold up the page
async fn ask(fleet: &Fleet) -> Vec<Status> {
    let http = client();
    let mut set = tokio::task::JoinSet::new();
    for (name, addr) in fleet {
        let (http, name, addr) = (http.clone(), name.clone(), addr.clone());
        set.spawn(async move { one(&http, &name, &addr).await });
    }
    let mut out: Vec<Status> = set.join_all().await;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
