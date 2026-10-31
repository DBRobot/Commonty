//! Taking everything out, from Settings' Leave tab: one zip of every photo,
//! made here with the Photos password this device holds (the page never
//! sees it), and the zip of a library the page makes itself, decrypting with
//! the library's key, which only passes through here on its way to disk.
//!
//! A zip goes to the member's browser as a download (a browser let in from
//! the window, local.rs): the page starts a job, opens the job's download,
//! and the bytes stream to it as they are made. In the app's own window,
//! whose scheme cannot stream an answer, the zip is written to the Downloads
//! folder instead (site.rs). Either way nothing is held whole: a few pieces
//! are in flight at a time, and a slow download holds the making back.
//!
//! A job is its starter's alone: the window's, or one browser session's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::response::IntoResponse as _;
use serde::Serialize;
use tauri::{AppHandle, Manager as _, Runtime};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;

/// pieces in flight between the making and the download
const IN_FLIGHT: usize = 4;
/// how long a piece waits for the download to take it
const WAIT: Duration = Duration::from_secs(120);
/// what one piece from the page may be
pub const MOST: usize = 16 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Photos,
    Files,
}

impl Kind {
    pub fn of(s: &str) -> Option<Kind> {
        match s {
            "photos" => Some(Kind::Photos),
            "files" => Some(Kind::Files),
            _ => None,
        }
    }
    fn file_name(self) -> String {
        let what = match self {
            Kind::Photos => "photos",
            Kind::Files => "files",
        };
        format!("commonty-{what}-{}.zip", today())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Owner {
    Window,
    Browser(String),
}

/// what the page shows while it goes
#[derive(Clone, Default, Serialize)]
pub struct Progress {
    pub files: u64,
    pub total: Option<u64>,
    pub bytes: u64,
    #[serde(rename = "totalBytes")]
    pub total_bytes: Option<u64>,
    pub done: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    /// in the window: the file it was written to
    pub saved: Option<String>,
}

type Piece = Result<Vec<u8>, std::io::Error>;

enum Sink {
    Download(mpsc::Sender<Piece>),
    File(tokio::fs::File, PathBuf),
}

pub struct Job {
    owner: Owner,
    kind: Kind,
    name: String,
    progress: Mutex<Progress>,
    cancel: AtomicBool,
    sink: tokio::sync::Mutex<Option<Sink>>,
    download: Mutex<Option<mpsc::Receiver<Piece>>>,
}

static JOBS: LazyLock<Mutex<HashMap<String, Arc<Job>>>> = LazyLock::new(Default::default);

fn random_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn today() -> String {
    let days = (identity::now() / 86400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub type Refused = (u16, String);

fn job(owner: &Owner, id: &str) -> Result<Arc<Job>, Refused> {
    JOBS.lock()
        .map_err(|_| (500, "no jobs".to_string()))?
        .get(id)
        .filter(|j| &j.owner == owner)
        .cloned()
        .ok_or((404, "no such download".into()))
}

/// A new job for `owner`, ending any of theirs of the same kind. In the
/// window it writes to the Downloads folder; for a browser, to a download
/// the page opens next (`take_download`).
pub async fn start<R: Runtime>(
    app: &AppHandle<R>,
    owner: Owner,
    kind: Kind,
) -> Result<serde_json::Value, Refused> {
    let name = kind.file_name();
    let (sink, download) = if owner == Owner::Window {
        let dir = app
            .path()
            .download_dir()
            .map_err(|e| (500, format!("no Downloads folder here: {e}")))?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| (500, e.to_string()))?;
        let mut path = dir.join(&name);
        for n in 2.. {
            if !path.exists() {
                break;
            }
            path = dir.join(name.replace(".zip", &format!(" ({n}).zip")));
        }
        let f = tokio::fs::File::create(&path)
            .await
            .map_err(|e| (500, format!("{}: {e}", path.display())))?;
        (Sink::File(f, path), None)
    } else {
        let (tx, rx) = mpsc::channel(IN_FLIGHT);
        (Sink::Download(tx), Some(rx))
    };
    let id = random_id();
    let j = Arc::new(Job {
        owner: owner.clone(),
        kind,
        name: name.clone(),
        progress: Mutex::new(Progress::default()),
        cancel: AtomicBool::new(false),
        sink: tokio::sync::Mutex::new(Some(sink)),
        download: Mutex::new(download),
    });
    {
        let mut jobs = JOBS.lock().map_err(|_| (500, "no jobs".to_string()))?;
        // one at a time of each kind: a new one ends the last
        jobs.retain(|_, old| {
            let same = old.owner == owner && old.kind == kind;
            if same {
                old.cancel.store(true, Ordering::SeqCst);
            }
            !same
        });
        jobs.insert(id.clone(), j.clone());
    }
    if kind == Kind::Photos {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { photos_zip(app, j).await });
    }
    Ok(serde_json::json!({ "id": id, "name": name }))
}

/// The download a browser's page opens for its job, once: the zip's
/// pieces as they come, and its name. A failure breaks the stream off, so
/// the browser shows the download as failed rather than keep half a zip.
pub fn take_download(
    owner: &Owner,
    id: &str,
) -> Result<(impl futures_util::Stream<Item = Piece> + use<>, String), Refused> {
    let j = job(owner, id)?;
    let rx = j
        .download
        .lock()
        .map_err(|_| (500, "no download".to_string()))?
        .take()
        .ok_or((409, "this download was opened already".to_string()))?;
    let stream =
        futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|p| (p, rx)) });
    Ok((stream, j.name.clone()))
}

pub fn state(owner: &Owner, id: &str) -> Result<serde_json::Value, Refused> {
    let j = job(owner, id)?;
    let p = j.progress.lock().map(|p| p.clone()).unwrap_or_default();
    Ok(serde_json::to_value(p).unwrap_or_default())
}

/// Stop: the app's own zip stops at its next piece; the page's is broken
/// off here, since the page has stopped sending
pub async fn cancel(owner: &Owner, id: &str) -> Result<(), Refused> {
    let j = job(owner, id)?;
    j.cancel.store(true, Ordering::SeqCst);
    j.set(|p| p.cancelled = true);
    if j.kind == Kind::Files {
        j.finish(Some("cancelled".into())).await;
    }
    Ok(())
}

impl Job {
    /// one piece on to its sink, waiting while the download is behind
    async fn put(&self, bytes: Vec<u8>) -> Result<(), String> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let mut sink = self.sink.lock().await;
        match sink.as_mut() {
            Some(Sink::File(f, _)) => f.write_all(&bytes).await.map_err(|e| e.to_string()),
            Some(Sink::Download(tx)) => {
                match tokio::time::timeout(WAIT, tx.send(Ok(bytes))).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(_)) => Err("the download was stopped".into()),
                    Err(_) => Err("the download did not start".into()),
                }
            }
            None => Err("this download is over".into()),
        }
    }

    /// the end: the file closed (or the download ended), or on a failure the
    /// download broken off so the browser does not keep half a zip as whole,
    /// and the half-written file removed
    async fn finish(&self, failed: Option<String>) {
        let sink = self.sink.lock().await.take();
        let mut saved = None;
        match (sink, &failed) {
            (Some(Sink::File(mut f, path)), None) => match f.flush().await {
                Ok(()) => saved = Some(path.display().to_string()),
                Err(e) => {
                    self.set(|p| p.error = Some(e.to_string()));
                }
            },
            (Some(Sink::File(f, path)), Some(_)) => {
                drop(f);
                let _ = tokio::fs::remove_file(&path).await;
            }
            (Some(Sink::Download(tx)), Some(why)) => {
                let _ = tx.try_send(Err(std::io::Error::other(why.clone())));
            }
            _ => {}
        }
        let cancelled = self.cancel.load(Ordering::SeqCst);
        self.set(|p| {
            p.saved = saved;
            p.cancelled = cancelled;
            match failed {
                Some(e) if !cancelled => p.error = Some(e),
                Some(_) => {}
                None => p.done = p.error.is_none(),
            }
        });
    }

    fn set(&self, f: impl FnOnce(&mut Progress)) {
        if let Ok(mut p) = self.progress.lock() {
            f(&mut p);
        }
    }
}

/// a piece of the zip the page is making of a library, on to its sink
pub async fn chunk(owner: &Owner, id: &str, bytes: Vec<u8>) -> Result<(), Refused> {
    let j = job(owner, id)?;
    if j.kind != Kind::Files {
        return Err((400, "the app makes this one itself".into()));
    }
    let n = bytes.len() as u64;
    match j.put(bytes).await {
        Ok(()) => {
            j.set(|p| p.bytes += n);
            Ok(())
        }
        Err(e) => {
            j.finish(Some(e.clone())).await;
            Err((410, e))
        }
    }
}

/// the page's zip is whole
pub async fn end(owner: &Owner, id: &str) -> Result<serde_json::Value, Refused> {
    let j = job(owner, id)?;
    if j.kind != Kind::Files {
        return Err((400, "the app makes this one itself".into()));
    }
    j.finish(None).await;
    state(owner, id)
}

// ---- every photo, as one zip

async fn photos_zip<R: Runtime>(app: AppHandle<R>, j: Arc<Job>) {
    let r = make_photos(&app, &j).await;
    if let Err(e) = &r {
        eprintln!("export: photos: {e}");
    }
    j.finish(r.err()).await;
}

/// The folder a photo goes in: the first of its albums by name, folders and
/// favourites after real albums, or "Not in an album"
fn folder_of(p: &ente::gallery::Photo, albums: &[ente::gallery::Album]) -> String {
    let rank = |k: &str| match k {
        "album" | "" => 0,
        "folder" => 1,
        "favorites" => 2,
        _ => 3,
    };
    p.albums
        .iter()
        .filter_map(|id| albums.iter().find(|a| a.id == *id))
        .filter(|a| a.kind != "uncategorized" && !a.name.trim().is_empty())
        .min_by(|a, b| {
            rank(&a.kind)
                .cmp(&rank(&b.kind))
                .then_with(|| a.name.cmp(&b.name))
        })
        .map(|a| a.name.trim().to_string())
        .unwrap_or_else(|| "Not in an album".into())
}

/// a photo's name in its folder: its own, the live photo's two halves kept
/// together as Ente keeps them (a zip of both)
fn name_of(p: &ente::gallery::Photo) -> String {
    let title = if p.title.trim().is_empty() {
        format!("photo-{}", p.id)
    } else {
        p.title.trim().to_string()
    };
    if p.kind == 2 {
        let stem = title.rsplit_once('.').map_or(title.as_str(), |(s, _)| s);
        format!("{stem} (live photo).zip")
    } else {
        title
    }
}

async fn make_photos<R: Runtime>(app: &AppHandle<R>, j: &Job) -> Result<(), String> {
    let pw = crate::photos::current(app)
        .ok_or("Photos does not open on this device yet: open Photos once in the app")?;
    let (api, email) = crate::photos::config(app).await?;
    // the downloads: over the fleet's network, as long as each takes
    let http = directory::builder()
        .map_err(|e| e.to_string())?
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let (login, relay) = relay(&api).await?;
    let signed = ente::gallery::Gallery::sign_in_with(&login, &api, &email, &pw, http).await;
    relay.abort();
    let g = signed.map_err(|e| format!("signing in to Photos: {e:#}"))?;
    let (albums, photos) = g.photos().await.map_err(|e| format!("{e:#}"))?;
    j.set(|p| {
        p.total = Some(photos.len() as u64);
        p.total_bytes = photos.iter().map(|x| x.size).sum::<Option<u64>>();
    });
    let mut zip = crate::zip::Zip::new();
    let mut folders = std::collections::BTreeSet::new();
    for p in &photos {
        if j.cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let folder = folder_of(p, &albums);
        let when = p.taken / 1_000_000;
        if folders.insert(folder.clone()) {
            j.put(zip.folder(&folder, when)).await?;
        }
        let mut original = g.original_stream(p).await.map_err(|e| format!("{e:#}"))?;
        j.put(zip.file(&format!("{folder}/{}", name_of(p)), when))
            .await?;
        while let Some(piece) = original.next().await.map_err(|e| format!("{e:#}"))? {
            zip.data(&piece);
            let n = piece.len() as u64;
            j.put(piece).await?;
            j.set(|p| p.bytes += n);
        }
        j.put(zip.done()).await?;
        j.set(|p| p.files += 1);
    }
    j.put(zip.end()).await
}

/// Ente's sign-in reaches museum with a client of its own, which knows
/// nothing of the fleet's network. For the moment it takes, an address on
/// this computer passes it on: under a path no one else knows, to museum
/// alone, and carrying no credential of this device's.
pub(crate) async fn relay(
    api: &str,
) -> Result<(String, tauri::async_runtime::JoinHandle<()>), String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let secret = random_id();
    let prefix = format!("/{secret}");
    let api = api.trim_end_matches('/').to_string();
    let router = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let prefix = prefix.clone();
        let api = api.clone();
        async move {
            let (mut parts, body) = req.into_parts();
            let Some(rest) = parts.uri.path().strip_prefix(&prefix).map(str::to_string) else {
                return axum::http::StatusCode::NOT_FOUND.into_response();
            };
            let pq = match parts.uri.query() {
                Some(q) => format!("{rest}?{q}"),
                None => rest,
            };
            match pq.parse() {
                Ok(u) => parts.uri = u,
                Err(_) => return axum::http::StatusCode::BAD_REQUEST.into_response(),
            }
            crate::local::forward(
                &api,
                None,
                axum::extract::Request::from_parts(parts, body),
                false,
            )
            .await
        }
    });
    let task = tauri::async_runtime::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://127.0.0.1:{port}/{secret}"), task))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(albums: Vec<i64>, title: &str, kind: i64) -> ente::gallery::Photo {
        ente::gallery::Photo::for_test(1, title, kind, albums)
    }

    fn album(id: i64, name: &str, kind: &str) -> ente::gallery::Album {
        ente::gallery::Album {
            id,
            name: name.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn a_photo_goes_in_its_first_album_or_none() {
        let albums = vec![
            album(1, "Zoo", "album"),
            album(2, "Beach", "album"),
            album(3, "Favorites", "favorites"),
            album(4, "Uncategorized", "uncategorized"),
            album(5, "Camera", "folder"),
        ];
        assert_eq!(
            folder_of(&photo(vec![1, 2, 3], "a.jpg", 0), &albums),
            "Beach"
        );
        assert_eq!(folder_of(&photo(vec![3, 5], "a.jpg", 0), &albums), "Camera");
        assert_eq!(folder_of(&photo(vec![3], "a.jpg", 0), &albums), "Favorites");
        assert_eq!(
            folder_of(&photo(vec![4], "a.jpg", 0), &albums),
            "Not in an album"
        );
        assert_eq!(
            folder_of(&photo(vec![99], "a.jpg", 0), &albums),
            "Not in an album"
        );
        assert_eq!(
            folder_of(&photo(vec![], "a.jpg", 0), &albums),
            "Not in an album"
        );
    }

    #[test]
    fn a_live_photo_keeps_its_two_halves_together() {
        assert_eq!(
            name_of(&photo(vec![], "IMG_1.HEIC", 2)),
            "IMG_1 (live photo).zip"
        );
        assert_eq!(name_of(&photo(vec![], "IMG_2.mov", 1)), "IMG_2.mov");
        assert_eq!(name_of(&photo(vec![], " ", 0)), "photo-1");
    }

    #[test]
    fn a_job_is_its_starters_alone() {
        let id = "j1".to_string();
        let (tx, rx) = mpsc::channel(1);
        JOBS.lock().unwrap().insert(
            id.clone(),
            Arc::new(Job {
                owner: Owner::Browser("a".into()),
                kind: Kind::Files,
                name: "x.zip".into(),
                progress: Mutex::new(Progress::default()),
                cancel: AtomicBool::new(false),
                sink: tokio::sync::Mutex::new(Some(Sink::Download(tx))),
                download: Mutex::new(Some(rx)),
            }),
        );
        assert_eq!(state(&Owner::Browser("b".into()), &id).unwrap_err().0, 404);
        assert_eq!(state(&Owner::Window, &id).unwrap_err().0, 404);
        assert!(take_download(&Owner::Window, &id).is_err());
        assert!(take_download(&Owner::Browser("a".into()), &id).is_ok());
        assert_eq!(
            take_download(&Owner::Browser("a".into()), &id)
                .err()
                .map(|e| e.0),
            Some(409),
            "a download opens once"
        );
    }

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }
}
