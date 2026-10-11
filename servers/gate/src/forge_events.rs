//! The forge's changes as they happen, for the Git pages (servers/forge/web).
//! A trigger in Forgejo's database (modules/forge/live.nix) announces every
//! run, job and commit status that changes, as ids and a state and nothing
//! else. The gate listens with a login that may read no table, and hands
//! each announcement to the pages watching that repository over Server-Sent
//! Events. Nothing is asked while nothing happens.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::App;

/// the channel the trigger announces on
const CHANNEL: &str = "dd_forge";

#[derive(Clone)]
pub struct ForgeEvents {
    tx: broadcast::Sender<String>,
}

impl ForgeEvents {
    /// listen from now on, reconnecting whenever the database goes away
    pub fn start(conn: String) -> Self {
        let (tx, _) = broadcast::channel(512);
        let out = tx.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = listen(&conn, &out).await {
                    eprintln!("forge events: {e}; again in 5 s");
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        ForgeEvents { tx }
    }
}

async fn listen(conn: &str, out: &broadcast::Sender<String>) -> anyhow::Result<()> {
    let (client, mut connection) = tokio_postgres::connect(conn, tokio_postgres::NoTls).await?;
    let mut messages = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
    // the client asks once, then only has to stay alive while the
    // connection is read below
    let (said, heard) = tokio::sync::oneshot::channel();
    let keep = tokio::spawn(async move {
        let r = client.batch_execute(&format!("LISTEN {CHANNEL}")).await;
        let _ = said.send(r);
        std::future::pending::<()>().await;
        drop(client);
    });
    let mut heard = Some(heard);
    let result = loop {
        tokio::select! {
            r = async { heard.as_mut().unwrap().await }, if heard.is_some() => {
                heard = None;
                if let Ok(Err(e)) = r {
                    break Err(e.into());
                }
            }
            m = messages.next() => match m {
                Some(Ok(tokio_postgres::AsyncMessage::Notification(n))) => {
                    // nobody watching is fine: the send has no one to reach
                    let _ = out.send(n.payload().to_string());
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            },
        }
    };
    keep.abort();
    result
}

/// An announcement, as a page watching `repo` gets it, or nothing: another
/// repository's, or one the trigger did not say is open to every member.
/// A repository private on the forge, or whose owner is, is the forge's to
/// show to whom it chooses; the gate shows its runs and commits to nobody.
fn forward(payload: &str, repo: i64) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(payload).ok()?;
    if v["repo"].as_i64() != Some(repo) || v["private"].as_bool() != Some(false) {
        return None;
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("private");
    }
    // sent on compact: Postgres writes its json with spaces
    Some(v.to_string())
}

#[derive(Deserialize)]
pub(crate) struct Watch {
    /// the repository's id (the pages have it from the forge's api)
    repo: i64,
}

/// GET /_dd/git/events?repo=<id>: that repository's changes, as they happen
pub(crate) async fn events(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(w): Query<Watch>,
) -> Response {
    let Some(ev) = app.forge_events.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match app.identify(&headers, "access") {
        Some(u) if app.member(&u) && u != crate::pages::DEMO_USER => {}
        Some(_) => return StatusCode::FORBIDDEN.into_response(),
        None => return StatusCode::UNAUTHORIZED.into_response(),
    }
    let repo = w.repo;
    let stream = tokio_stream::wrappers::BroadcastStream::new(ev.tx.subscribe()).filter_map(
        move |m| async move {
            let data = forward(&m.ok()?, repo)?;
            Some(Ok::<_, Infallible>(Event::default().data(data)))
        },
    );
    let mut r = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
        .into_response();
    // nginx hands each event on as it comes instead of gathering a buffer
    r.headers_mut().insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    r.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    r
}

#[cfg(test)]
mod tests {
    use super::forward;

    #[test]
    fn only_open_repositories_reach_the_page() {
        let open = r#"{"kind" : "run", "repo" : 7, "run" : 3, "status" : 1, "private" : false}"#;
        assert_eq!(
            forward(open, 7).as_deref(),
            Some(r#"{"kind":"run","repo":7,"run":3,"status":1}"#)
        );
        // another repository's
        assert_eq!(forward(open, 8), None);
        // private, or not said: nothing
        let private =
            r#"{"kind": "status", "repo": 7, "sha": "ab", "state": "success", "private": true}"#;
        assert_eq!(forward(private, 7), None);
        let unsaid = r#"{"kind": "job", "repo": 7, "run": 3, "job": 9, "status": 1}"#;
        assert_eq!(forward(unsaid, 7), None);
        let null = r#"{"kind": "job", "repo": 7, "private": null}"#;
        assert_eq!(forward(null, 7), None);
    }
}
