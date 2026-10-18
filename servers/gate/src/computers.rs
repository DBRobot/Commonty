//! Members' remote computers (docs/dev/remote-computers.md): their signed
//! items, put here and handed to the control plane, which judges them
//! (state/computers). The same road in as the directory's PUTs: the one
//! limiter by visitor, a bounded body, the signature checked here so a
//! forgery never reaches the log, the name's and the key's allowance, and
//! the shared list of items already sent. Nothing is decided here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use crate::directory::Directory;

/// The kinds put here, and whether the body names its owner
pub const KINDS: [(&str, bool); 4] = [
    ("computer-config", true),
    ("computer", true),
    ("computer-delete", true),
    ("computer-power", false),
];

/// Whose each remote computer is, as the agreed state last said: what a
/// start or stop, which names only the computer, is checked against
pub struct Owners {
    url: String,
    http: reqwest::Client,
    // None until the first read: nothing known is not "nobody's"
    owners: Mutex<Option<(u64, HashMap<String, String>)>>,
}

#[derive(serde::Deserialize)]
struct View {
    changed: u64,
    computers: HashMap<String, Listed>,
}

#[derive(serde::Deserialize)]
struct Listed {
    owner: String,
}

impl Owners {
    /// `control` is dd-control's address on this box
    pub fn new(control: &str) -> Self {
        Self {
            url: format!("{}/computers", control.trim_end_matches('/')),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("an http client"),
            owners: Mutex::new(None),
        }
    }

    /// whose it is; Err while nothing has been read
    pub fn owner_of(&self, id: &str) -> Result<Option<String>, ()> {
        match &*self.owners.lock().unwrap() {
            None => Err(()),
            Some((_, m)) => Ok(m.get(id).cloned()),
        }
    }

    async fn refresh(&self) -> anyhow::Result<()> {
        let since = self.owners.lock().unwrap().as_ref().map(|(c, _)| *c);
        let mut req = self.http.get(&self.url);
        if let Some(c) = since {
            req = req.query(&[("since", c)]);
        }
        let res = req.send().await?;
        if res.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(());
        }
        let v: View = res.error_for_status()?.json().await?;
        let m = v
            .computers
            .into_iter()
            .map(|(id, c)| (id, c.owner))
            .collect();
        *self.owners.lock().unwrap() = Some((v.changed, m));
        Ok(())
    }

    /// Kept fresh for as long as the gate runs, like the identity state
    pub async fn follow(self: Arc<Self>) {
        let mut wait = Duration::from_secs(1);
        loop {
            match self.refresh().await {
                Ok(()) => wait = Duration::from_secs(1),
                Err(e) => {
                    eprintln!("computers: dd-control did not answer: {e:#}");
                    wait = (wait * 2).min(Duration::from_secs(30));
                }
            }
            tokio::time::sleep(wait).await;
        }
    }

    #[cfg(test)]
    pub fn set(&self, m: HashMap<String, String>) {
        *self.owners.lock().unwrap() = Some((1, m));
    }
}

/// PUT /_dd/computers/{kind}: one signed item of that kind
pub async fn put(
    State(d): State<Arc<Directory>>,
    Path(kind): Path<String>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    let body = match crate::directory::read_put(&d, &headers, body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    match d.submit_computer(&kind, &headers, &body).await {
        Ok(hash) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "submitted": true, "item": hash })),
        )
            .into_response(),
        Err((status, why)) => (status, why).into_response(),
    }
}
