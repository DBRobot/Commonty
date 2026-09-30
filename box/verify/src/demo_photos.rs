//! Photos, for the demo: the demo account's photos as a gallery on the
//! gate's own public name. Ente's app and server stay on the private
//! network; the gate signs in as the demo (it holds that password already),
//! opens the photos the way Ente's apps do, and serves the pictures. Only
//! the demo's photos, which exist to be shown, and only to the demo.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Redirect, Response};
use ente::gallery::{Gallery, Photo};

use crate::App;
use crate::pages;

/// how long a listing is believed before the account is asked again
const FRESH: Duration = Duration::from_secs(600);
/// the biggest original shown whole; a larger one shows its thumbnail
const ORIGINAL: usize = 25 << 20;

/// a signed-in gallery, its photos, and when they were listed
type Open = (Arc<Gallery>, Arc<Vec<Photo>>, Instant);

#[derive(Default)]
pub struct DemoPhotos {
    open: tokio::sync::Mutex<Option<Open>>,
    thumbs: std::sync::Mutex<HashMap<i64, Arc<Vec<u8>>>>,
}

impl DemoPhotos {
    /// the gallery and its photos, signing in again when the listing is old
    /// or the last attempt failed
    async fn get(&self, app: &App) -> Result<(Arc<Gallery>, Arc<Vec<Photo>>), StatusCode> {
        let mut open = self.open.lock().await;
        if let Some((g, ps, at)) = open.as_ref()
            && at.elapsed() < FRESH
        {
            return Ok((g.clone(), ps.clone()));
        }
        let p = app.photos.as_ref().ok_or(StatusCode::NOT_FOUND)?;
        let pw = p.demo_password.as_ref().ok_or(StatusCode::NOT_FOUND)?;
        let email = format!("{}{}", pages::DEMO_USER, p.email_suffix);
        let g = Gallery::sign_in(&p.api, &email, pw).await.map_err(|e| {
            eprintln!("demo photos: sign-in: {e:#}");
            StatusCode::BAD_GATEWAY
        })?;
        let ps = g.photos().await.map_err(|e| {
            eprintln!("demo photos: listing: {e:#}");
            StatusCode::BAD_GATEWAY
        })?;
        let (g, ps) = (Arc::new(g), Arc::new(ps));
        *open = Some((g.clone(), ps.clone(), Instant::now()));
        Ok((g, ps))
    }

    /// Every thumbnail opened ahead, a few at a time: the first person to
    /// look finds the grid ready instead of waiting on thirty of them.
    fn warm(self: &Arc<Self>, g: Arc<Gallery>, ps: Arc<Vec<Photo>>) {
        let me = self.clone();
        tokio::spawn(async move {
            for chunk in ps.chunks(4) {
                let todo: Vec<&Photo> = chunk
                    .iter()
                    .filter(|p| !me.thumbs.lock().unwrap().contains_key(&p.id))
                    .collect();
                let got = futures_util::future::join_all(todo.iter().map(|p| g.thumbnail(p))).await;
                for (p, b) in todo.iter().zip(got) {
                    if let Ok(b) = b {
                        me.thumbs.lock().unwrap().insert(p.id, Arc::new(b));
                    }
                }
            }
        });
    }
}

/// where anyone but the demo goes instead: a member has their own Photos,
/// and nobody at all starts the demo
fn not_demo(app: &App, headers: &HeaderMap) -> Option<Response> {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok());
    match app.sessions.user(cookie) {
        Some(u) if u == pages::DEMO_USER => None,
        Some(_) => Some(Redirect::to("/_dd/photos").into_response()),
        None => Some(Redirect::to("/_dd/demo").into_response()),
    }
}

/// GET /_dd/photos/demo: the page
pub(crate) async fn page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Some(r) = not_demo(&app, &headers) {
        return r;
    }
    crate::page("demo-photos")
}

/// GET /_dd/photos/demo/list: newest first, what the grid needs
pub(crate) async fn list(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Some(r) = not_demo(&app, &headers) {
        return r;
    }
    match app.demo_photos.get(&app).await {
        Ok((g, ps)) => {
            app.demo_photos.warm(g, ps.clone());
            Json(
                ps.iter()
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id,
                            "title": p.title,
                            "taken": p.taken / 1_000_000,
                            "still": p.kind == 0,
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .into_response()
        }
        Err(s) => s.into_response(),
    }
}

/// the picture's type from its first bytes: only what a browser shows
fn kind(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if b.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

fn picture(bytes: Arc<Vec<u8>>) -> Response {
    let Some(ct) = kind(&bytes) else {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    };
    (
        [
            ("content-type", ct),
            ("cache-control", "private, max-age=3600"),
        ],
        bytes.as_ref().clone(),
    )
        .into_response()
}

/// GET /_dd/photos/demo/thumb/{id}
pub(crate) async fn thumb(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Some(r) = not_demo(&app, &headers) {
        return r;
    }
    if let Some(t) = app.demo_photos.thumbs.lock().unwrap().get(&id).cloned() {
        return picture(t);
    }
    let (g, ps) = match app.demo_photos.get(&app).await {
        Ok(x) => x,
        Err(s) => return s.into_response(),
    };
    let Some(p) = ps.iter().find(|p| p.id == id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match g.thumbnail(p).await {
        Ok(b) => {
            let b = Arc::new(b);
            app.demo_photos.thumbs.lock().unwrap().insert(id, b.clone());
            picture(b)
        }
        Err(e) => {
            eprintln!("demo photos: thumbnail {id}: {e:#}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// GET /_dd/photos/demo/photo/{id}: the image whole, or its thumbnail where
/// the original is not something a browser shows
pub(crate) async fn photo(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Some(r) = not_demo(&app, &headers) {
        return r;
    }
    let (g, ps) = match app.demo_photos.get(&app).await {
        Ok(x) => x,
        Err(s) => return s.into_response(),
    };
    let Some(p) = ps.iter().find(|p| p.id == id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match g.original(p, ORIGINAL).await {
        Ok(b) if kind(&b).is_some() => picture(Arc::new(b)),
        _ => thumb(State(app), headers, Path(id)).await,
    }
}
