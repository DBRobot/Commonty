use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use games::{Catalogue, Config, Manager, Systemd, pages};

fn env(k: &str) -> Result<String> {
    std::env::var(k).with_context(|| format!("{k} is not set"))
}
fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

/// Who the gate says is asking; nginx sets it from the verifier's answer.
fn user(h: &HeaderMap) -> Option<String> {
    let u = h.get("x-dd-user")?.to_str().ok()?.trim().to_string();
    (!u.is_empty()).then_some(u)
}

/// A guest, as the gate says: plays where invited, hosts nothing. No word
/// from the gate is taken as the least: a guest.
fn guest(h: &HeaderMap) -> bool {
    !matches!(
        h.get("x-dd-role").and_then(|v| v.to_str().ok()),
        Some("member" | "demo")
    )
}

#[derive(serde::Deserialize)]
struct IndexQuery {
    n: Option<String>,
    q: Option<String>,
}

struct App {
    m: Arc<Manager>,
    covers: std::path::PathBuf,
    /// the gate on this box, for the owner's friends (friends.rs there)
    gate: String,
    http: reqwest::Client,
}

impl App {
    /// The owner's friends as the gate has them now: who may be invited.
    /// Nothing, if the gate cannot say - an invite is never taken on trust.
    async fn friends(&self, owner: &str) -> Vec<String> {
        let url = format!("{}/internal/games/friends/{owner}", self.gate);
        match self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(e) => {
                eprintln!("games: the gate did not answer for {owner}'s friends: {e}");
                Vec::new()
            }
        }
    }
}

async fn index(State(a): State<Arc<App>>, h: HeaderMap, Query(q): Query<IndexQuery>) -> Response {
    match user(&h) {
        Some(u) => Html(pages::library(
            &a.m,
            &u,
            guest(&h),
            q.q.as_deref().unwrap_or(""),
            None,
            q.n.as_deref(),
        ))
        .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// the library with one game open over it
async fn game(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<IndexQuery>,
) -> Response {
    let Some(u) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !a.m.cfg.catalogue.contains_key(&id) || guest(&h) {
        return StatusCode::NOT_FOUND.into_response();
    }
    Html(pages::library(
        &a.m,
        &u,
        false,
        q.q.as_deref().unwrap_or(""),
        Some(&id),
        q.n.as_deref(),
    ))
    .into_response()
}

/// one server's own page: status, address, log, controls
async fn server(State(a): State<Arc<App>>, h: HeaderMap, Path(id): Path<String>) -> Response {
    let Some(u) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // yours or one you are invited to; anyone else's is not there at all
    match a.m.instance(&id).filter(|i| a.m.may_see(&u, i)) {
        Some(i) => {
            let friends = if i.owner == u && !guest(&h) {
                a.friends(&u).await
            } else {
                Vec::new()
            };
            Html(pages::server(&a.m, &u, &i, &friends)).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The owner ticks who plays. Each name is checked against the gate's
/// list of the owner's friends, asked now; the form's word is not enough.
async fn players(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Response {
    let Some(u) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if guest(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let want: Vec<String> = form
        .keys()
        .filter_map(|k| k.strip_prefix("p:"))
        .map(str::to_string)
        .collect();
    let friends = a.friends(&u).await;
    back(
        &format!("/server/{id}#players"),
        a.m.set_players(&u, &id, &want, &friends).map(|_| ()),
    )
}

/// a server's state and log, for the page to refresh itself with
async fn server_state(State(a): State<Arc<App>>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if user(&h).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(i) = a.m.instance(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // the log is the owner's: a game server writes passwords, addresses and
    // whatever a player types into it
    if user(&h).is_none_or(|u| u != i.owner) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let st = a.m.state(&i);
    axum::Json(serde_json::json!({
        "state": pages::state_name(st),
        "label": st.label(),
        "log": a.m.log_tail(&id, 80),
    }))
    .into_response()
}

/// the stylesheet and the script, from the crate
async fn static_file(Path(name): Path<String>) -> Response {
    let (body, ty): (&str, &str) = match name.as_str() {
        "games.css" => (include_str!("../web/games.css"), "text/css; charset=utf-8"),
        "games.js" => (
            include_str!("../web/games.js"),
            "text/javascript; charset=utf-8",
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, ty),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn cover(State(a): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let Some(g) = a.m.cfg.catalogue.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(game) = g.game.filter(|_| g.cover) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match std::fs::read(a.covers.join(format!("{game}.jpg"))) {
        Ok(b) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "public, max-age=604800"),
            ],
            b,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Back to a page, with what went wrong on it if something did.
fn back(to: &str, r: Result<()>) -> Response {
    match r {
        Ok(()) => Redirect::to(to).into_response(),
        Err(e) => {
            let msg: String = format!("{e:#}")
                .bytes()
                .map(|b| match b {
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                        (b as char).to_string()
                    }
                    _ => format!("%{b:02X}"),
                })
                .collect();
            let sep = if to.contains('?') { '&' } else { '?' };
            Redirect::to(&format!("{to}{sep}n={msg}")).into_response()
        }
    }
}

async fn create(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(game): Path<String>,
    Form(settings): Form<BTreeMap<String, String>>,
) -> Response {
    let Some(u) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if guest(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match a.m.create(&u, &game, &settings) {
        Ok(i) => Redirect::to(&format!("/server/{}", i.id)).into_response(),
        Err(e) => back(&format!("/game/{game}"), Err(e)),
    }
}

async fn configure(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Form(settings): Form<BTreeMap<String, String>>,
) -> Response {
    let Some(u) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    back(&format!("/server/{id}"), a.m.configure(&u, &id, &settings))
}

async fn keep(State(a): State<Arc<App>>, h: HeaderMap, Path(id): Path<String>) -> Response {
    let Some(u) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match a.m.keep_world(&u, &id) {
        Ok(_) => Redirect::to("/").into_response(),
        Err(e) => back(&format!("/server/{id}"), Err(e)),
    }
}

async fn restore_world(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let Some(u) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if guest(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match a.m.restore_world(&u, &name) {
        Ok(i) => Redirect::to(&format!("/server/{}", i.id)).into_response(),
        Err(e) => back("/", Err(e)),
    }
}

async fn delete_world(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let Some(u) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    back("/", a.m.delete_world(&u, &name))
}

macro_rules! act {
    ($name:ident, $call:ident, $to:expr) => {
        async fn $name(
            State(a): State<Arc<App>>,
            h: HeaderMap,
            Path(id): Path<String>,
        ) -> Response {
            let Some(u) = user(&h) else {
                return StatusCode::UNAUTHORIZED.into_response();
            };
            let to = $to(&id);
            back(&to, a.m.$call(&u, &id))
        }
    };
}
act!(start, start, |id: &str| format!("/server/{id}"));
act!(stop, stop, |id: &str| format!("/server/{id}"));
act!(delete, delete, |_id: &str| "/".to_string());

#[tokio::main]
async fn main() -> Result<()> {
    let cat: Catalogue = serde_json::from_slice(&std::fs::read(env("DD_GAMES_CATALOGUE")?)?)
        .context("DD_GAMES_CATALOGUE")?;
    let catalogue = cat.games.into_iter().map(|g| (g.id.clone(), g)).collect();
    let m = Manager::new(
        Config {
            dir: env_or("DD_GAMES_DIR", "/var/lib/dd-games").into(),
            catalogue,
            port_base: env_or("DD_GAMES_PORT_BASE", "27000").parse()?,
            port_count: env_or("DD_GAMES_PORT_COUNT", "200").parse()?,
            per_member: env_or("DD_GAMES_PER_MEMBER", "1").parse()?,
            memory_budget: env_or("DD_GAMES_MEMORY_MIB", "16384").parse()?,
            cores: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(2),
            address: env("DD_GAMES_ADDRESS")?,
            home: env_or("DD_GAMES_HOME", "/"),
            access: std::env::var("DD_GAMES_ACCESS").ok().map(Into::into),
            address_file: std::env::var("DD_GAMES_ADDRESS_FILE").ok().map(Into::into),
        },
        Box::new(Systemd),
    )?;
    let back_up = m.restore();
    eprintln!("games: {back_up} server(s) brought back up");
    // what the firewall reads: rewritten as servers start and stop, and
    // every few seconds besides, so a missed change heals itself
    let writer = m.clone();
    tokio::spawn(async move {
        loop {
            writer.write_access();
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
    let app = Arc::new(App {
        m,
        covers: env_or("DD_GAMES_COVERS", "/var/lib/dd-games/covers").into(),
        gate: env_or("DD_GAMES_GATE", "http://127.0.0.1:4181"),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
    });
    let router = Router::new()
        .route("/", get(index))
        .route("/health", get(|| async { "ok" }))
        .route("/game/{id}", get(game))
        .route("/cover/{id}", get(cover))
        .route("/static/{name}", get(static_file))
        .route("/server/{id}", get(server))
        .route("/server/{id}/state", get(server_state))
        .route("/players/{id}", post(players))
        .route("/create/{game}", post(create))
        .route("/start/{id}", post(start))
        .route("/configure/{id}", post(configure))
        .route("/stop/{id}", post(stop))
        .route("/delete/{id}", post(delete))
        .route("/keep/{id}", post(keep))
        .route("/worlds/{name}/start", post(restore_world))
        .route("/worlds/{name}/delete", post(delete_world))
        .with_state(app);
    // A path is a unix socket, and that is what a box serves on: the name
    // above comes from a header, honestly set by nginx from the verifier's
    // answer and settable by anything else that can reach a port. Only what
    // systemd puts in this service's group can open the socket.
    let bind = env_or("DD_GAMES_BIND", "127.0.0.1:4182");
    eprintln!("games listening on {bind}");
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
