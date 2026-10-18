use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use vms::{Config, Limits, Manager, Systemd};

fn env(k: &str) -> Result<String> {
    std::env::var(k).with_context(|| format!("{k} is not set"))
}
fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}
fn num<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

type App = Arc<Manager>;

/// Who the gate says is asking, and whether a member: guests and the demo
/// have no computers here. The demo may look.
fn who(h: &HeaderMap) -> Result<(String, bool), StatusCode> {
    let u = h
        .get("x-dd-user")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let member = h.get("x-dd-role").and_then(|v| v.to_str().ok()) == Some("member");
    Ok((u, member))
}

fn member(h: &HeaderMap) -> Result<String, (StatusCode, &'static str)> {
    match who(h).map_err(|c| (c, "who are you?"))? {
        (u, true) => Ok(u),
        _ => Err((StatusCode::FORBIDDEN, "remote computers are for members")),
    }
}

/// an error a member can act on, said plainly
fn refused(e: anyhow::Error) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({"error": format!("{e:#}")})),
    )
        .into_response()
}

fn done(r: Result<impl serde::Serialize>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => refused(e),
    }
}

/// Everything the page draws, in one answer: the configs, the instances,
/// the limits and what is used of them.
async fn state(State(m): State<App>, h: HeaderMap) -> Response {
    let (user, is_member) = match who(&h) {
        Ok(w) => w,
        Err(r) => return r.into_response(),
    };
    let (memory, threads) = m.usage(&user);
    Json(serde_json::json!({
        "user": user,
        "member": is_member,
        "boxOwner": m.box_owner,
        "limits": m.limits,
        "used": {"memory": memory, "threads": threads},
        "configs": m.configs(&user).into_values().collect::<Vec<_>>(),
        "instances": m.instances(&user),
    }))
    .into_response()
}

async fn save_config(
    State(m): State<App>,
    h: HeaderMap,
    Path(name): Path<String>,
    Json(mut c): Json<Config>,
) -> Response {
    let user = match member(&h) {
        Ok(u) => u,
        Err(r) => return r.into_response(),
    };
    c.name = name;
    done(m.save_config(&user, c))
}

async fn delete_config(State(m): State<App>, h: HeaderMap, Path(name): Path<String>) -> Response {
    let user = match member(&h) {
        Ok(u) => u,
        Err(r) => return r.into_response(),
    };
    done(m.delete_config(&user, &name))
}

#[derive(serde::Deserialize)]
struct Make {
    config: String,
    name: String,
    /// the owner's yes to running on someone else's box
    #[serde(default)]
    yes: bool,
    /// SSH keys for this computer besides its config's
    #[serde(default)]
    keys: Vec<String>,
}

async fn make(State(m): State<App>, h: HeaderMap, Json(b): Json<Make>) -> Response {
    let user = match member(&h) {
        Ok(u) => u,
        Err(r) => return r.into_response(),
    };
    done(
        m.make(&user, &b.config, &b.name, b.yes, &b.keys)
            .map(|r| m.view(r)),
    )
}

#[derive(serde::Deserialize, Default)]
struct Yes {
    #[serde(default)]
    yes: bool,
}

async fn act(
    State(m): State<App>,
    h: HeaderMap,
    Path((id, verb)): Path<(String, String)>,
    body: Option<Json<Yes>>,
) -> Response {
    let user = match member(&h) {
        Ok(u) => u,
        Err(r) => return r.into_response(),
    };
    let yes = body.map(|Json(b)| b.yes).unwrap_or_default();
    let r = match verb.as_str() {
        "start" => m.set_up(&user, &id, true),
        "stop" => m.set_up(&user, &id, false),
        "apply" => m.apply(&user, &id, yes).map(|_| ()),
        "reset" => m.reset(&user, &id),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    done(r)
}

async fn delete(State(m): State<App>, h: HeaderMap, Path(id): Path<String>) -> Response {
    let user = match member(&h) {
        Ok(u) => u,
        Err(r) => return r.into_response(),
    };
    done(m.delete(&user, &id))
}

async fn log(State(m): State<App>, h: HeaderMap, Path(id): Path<String>) -> Response {
    let user = match member(&h) {
        Ok(u) => u,
        Err(r) => return r.into_response(),
    };
    match m.apply_log(&user, &id) {
        Ok(s) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], s).into_response(),
        Err(e) => refused(e),
    }
}

/// the page: files in web/, built in
async fn page(h: HeaderMap) -> Response {
    if let Err(r) = who(&h) {
        return r.into_response();
    }
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../web/index.html"),
    )
        .into_response()
}

async fn static_file(Path(name): Path<String>) -> Response {
    let (body, kind) = match name.as_str() {
        "vms.js" => (
            include_str!("../web/vms.js"),
            "text/javascript; charset=utf-8",
        ),
        "vms.css" => (include_str!("../web/vms.css"), "text/css; charset=utf-8"),
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

/// this box's memory less what it keeps, in MiB, as vm-facts.sh counts it
fn box_memory(reserve: u64) -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| {
            m.lines()
                .find_map(|l| l.strip_prefix("MemTotal:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        })
        .map(|kib| (kib / 1024).saturating_sub(reserve))
        .unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut m = Manager::new(
        env_or("DD_VMS_DIR", "/var/lib/dd-vms").into(),
        Limits {
            memory: num("DD_VMS_MEMORY_MIB", 16384),
            threads: num("DD_VMS_THREADS", 8),
            disk: num("DD_VMS_DISK_GIB", 200),
            port_base: num("DD_VMS_PORT_BASE", 27200),
            port_count: num("DD_VMS_PORT_COUNT", 200),
            box_memory: box_memory(num("DD_VMS_RESERVE_MIB", 2048)),
            box_threads: std::thread::available_parallelism()
                .map(|n| (n.get() as u32).saturating_sub(1).max(1))
                .unwrap_or(1),
        },
        env("DD_VMS_BOX_OWNER")?,
        env("DD_VMS_ADDRESS")?,
        Box::new(Systemd),
    )?;
    m.ledger = std::env::var("DD_VMS_LEDGER").ok().map(Into::into);
    let m = Arc::new(m);
    eprintln!("vms: {} computer(s) brought back up", m.restore());
    // what the firewall's filler reads, rewritten every few seconds
    if let Ok(access) = std::env::var("DD_VMS_ACCESS") {
        let w = m.clone();
        tokio::spawn(async move {
            loop {
                let body = serde_json::to_vec(&w.access()).unwrap_or_default();
                let tmp = format!("{access}.tmp");
                if std::fs::write(&tmp, body)
                    .and_then(|_| std::fs::rename(&tmp, &access))
                    .is_err()
                {
                    eprintln!("vms: could not write {access}");
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
        .route(
            "/api/configs/{name}",
            put(save_config).delete(delete_config),
        )
        .route("/api/instances", post(make))
        .route("/api/instances/{id}", axum::routing::delete(delete))
        .route("/api/instances/{id}/log", get(log))
        .route("/api/instances/{id}/{verb}", post(act))
        .with_state(m);
    // as the games manager: a unix socket only nginx's group opens, since
    // the user comes from a header
    let bind = env_or("DD_VMS_BIND", "127.0.0.1:4183");
    eprintln!("vms listening on {bind}");
    if bind.starts_with('/') {
        let _ = std::fs::remove_file(&bind);
        let listener = tokio::net::UnixListener::bind(&bind)?;
        std::fs::set_permissions(&bind, std::os::unix::fs::PermissionsExt::from_mode(0o660))?;
        axum::serve(listener, router).await?;
    } else {
        let listener = tokio::net::TcpListener::bind(&bind).await?;
        axum::serve(listener, router).await?;
    }
    Ok(())
}
