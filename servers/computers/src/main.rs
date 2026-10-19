use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use computers::{Host, Limits, valid_name};

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}
fn num(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

struct App {
    host: Host,
    /// box name -> its address on the Commonty network, from the release
    boxes: BTreeMap<String, String>,
}

/// Who the gate says is asking (nginx sets x-dd-user from its answer), and
/// whether a member: guests and the demo have no computers
fn who(h: &HeaderMap) -> Result<(String, bool), StatusCode> {
    let u = h
        .get("x-dd-user")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !valid_name(&u) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let member = h.get("x-dd-role").and_then(|v| v.to_str().ok()) == Some("member");
    Ok((u, member))
}

fn unavailable(e: anyhow::Error) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": format!("{e:#}")})),
    )
        .into_response()
}

/// Everything the page draws: the member's configs and computers as agreed,
/// where the plan puts each and how to reach it, what is used of their
/// limits, and for those running here what their guest said back
async fn state(State(app): State<Arc<App>>, h: HeaderMap) -> Response {
    let (user, member) = match who(&h) {
        Ok(w) => w,
        Err(c) => return c.into_response(),
    };
    let a = app.clone();
    let got = tokio::task::spawn_blocking(move || -> Result<_> {
        let v = computers::view(&a.host.control)?;
        let (on, why) = computers::placed(&a.host.control)?;
        Ok((v, on, why, computers::flagged(&a.host.placed)))
    })
    .await;
    let (v, on, why, here) = match got {
        Ok(Ok(x)) => x,
        Ok(Err(e)) => return unavailable(e),
        Err(e) => return unavailable(e.into()),
    };
    let (mut mem, mut thr, mut disk) = (0u64, 0u64, 0u64);
    let mut list = Vec::new();
    for c in v.computers.values().filter(|c| c.owner == user) {
        mem += c.memory;
        thr += c.threads;
        disk += c.disk;
        let host = on.get(&c.id).cloned();
        let running_here = here.contains(&c.id);
        let mut j = serde_json::to_value(c).unwrap_or_default();
        j["box"] = serde_json::json!(host);
        j["address"] = serde_json::json!(host.as_ref().and_then(|b| app.boxes.get(b)));
        j["waiting"] = serde_json::json!(why.get(&c.id));
        j["here"] = serde_json::json!(running_here);
        list.push(j);
    }
    let configs: Vec<_> = v
        .configs
        .get(&user)
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    Json(serde_json::json!({
        "user": user,
        "member": member,
        "limits": app.host.limits,
        "used": {"memory": mem, "threads": thr, "disk": disk},
        "configs": configs,
        "computers": list,
    }))
    .into_response()
}

/// the page: files in web/, built in
async fn page(h: HeaderMap) -> Response {
    if let Err(c) = who(&h) {
        return c.into_response();
    }
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../web/index.html"),
    )
        .into_response()
}

async fn static_file(Path(name): Path<String>) -> Response {
    let (body, kind) = match name.as_str() {
        "computers.js" => (
            include_str!("../web/computers.js"),
            "text/javascript; charset=utf-8",
        ),
        "computers.css" => (
            include_str!("../web/computers.css"),
            "text/css; charset=utf-8",
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, kind),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

fn host() -> Host {
    Host {
        // a computer's own unit is handed its directory; its parent holds them all
        dir: std::env::var("STATE_DIRECTORY")
            .ok()
            .and_then(|d| std::path::Path::new(&d).parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| env_or("DD_COMPUTERS_DIR", "/var/lib/dd-computer").into()),
        placed: env_or("DD_COMPUTERS_PLACED", "/run/dd-placed").into(),
        control: env_or("DD_COMPUTERS_CONTROL", "127.0.0.1:4195"),
        limits: Limits {
            memory: num("DD_COMPUTERS_MEMORY_MIB", 16384),
            threads: num("DD_COMPUTERS_THREADS", 8),
            disk: num("DD_COMPUTERS_DISK_GIB", 200),
            port_base: num("DD_COMPUTERS_PORT_BASE", 27200),
            port_count: num("DD_COMPUTERS_PORT_COUNT", 200),
        },
    }
}

async fn serve(host: Host) -> Result<()> {
    let boxes = match std::env::var("DD_COMPUTERS_BOXES") {
        Ok(f) => serde_json::from_slice(&std::fs::read(&f).with_context(|| f.clone())?)?,
        Err(_) => BTreeMap::new(),
    };
    let app = Arc::new(App { host, boxes });
    // what the firewall's filler reads, rewritten every few seconds: only
    // what is agreed now opens a port, and nothing while that is unknown
    if let Ok(access) = std::env::var("DD_COMPUTERS_ACCESS") {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                let b = a.clone();
                let body = tokio::task::spawn_blocking(move || {
                    let v = computers::view(&b.host.control).unwrap_or_default();
                    serde_json::to_vec(&b.host.access(&v)).unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                if computers::write_atomic(std::path::Path::new(&access), &body, 0o640).is_err() {
                    eprintln!("computers: could not write {access}");
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }
    let router = Router::new()
        .route("/", get(page))
        .route("/health", get(|| async { "ok" }))
        .route("/static/{name}", get(static_file))
        .route("/api/state", get(state))
        .with_state(app);
    // the user comes from a header: a unix socket only nginx's group opens
    let bind = env_or("DD_COMPUTERS_BIND", "/run/dd-computers/dd-computers.sock");
    if !bind.starts_with('/') {
        bail!("{bind} is not a socket path: only a socket nginx's group opens will do");
    }
    let _ = std::fs::remove_file(&bind);
    let listener = tokio::net::UnixListener::bind(&bind)?;
    std::fs::set_permissions(&bind, std::os::unix::fs::PermissionsExt::from_mode(0o660))?;
    eprintln!("computers: listening on {bind}");
    axum::serve(listener, router).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let h = host();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["prepare", id] => {
            let c = h.prepare(id)?;
            eprintln!(
                "computer-{id}: {}'s {}, {} MiB, {} threads, {} GiB, port {}",
                c.owner, c.name, c.memory, c.threads, c.disk, c.port
            );
            Ok(())
        }
        ["share", id] => {
            // dd-control or its answer not there: nothing to do now, and the
            // timer asks again
            match h.share(id) {
                Ok(true) => eprintln!("computer-{id}: its share rewritten"),
                Ok(false) => {}
                Err(e) => eprintln!("computer-{id}: the agreed record not read: {e:#}"),
            }
            Ok(())
        }
        [] | ["serve"] => serve(h).await,
        _ => bail!("usage: dd-computers [serve | prepare <id> | share <id>]"),
    }
}
