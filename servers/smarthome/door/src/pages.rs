//! The door's pages and what they load, as files: read once at start from
//! a directory laid out like the tree (the flake's pages), so a missing
//! one stops the door rather than a page answering 404.

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::path::Path;

/// Served at /_door/static/<name>, and at /_dd/static/<name> as well: the
/// shared stylesheets ask for the bar and the fonts there.
pub const STATIC: &[(&str, &str, &str)] = &[
    ("home.css", "servers/web-shared/home.css", CSS),
    ("bar.css", "servers/web-shared/bar.css", CSS),
    ("kit.js", "servers/web-shared/kit.js", JS),
    ("webauthn.js", "servers/web-shared/webauthn.js", JS),
    (
        "favicon.svg",
        "servers/web-shared/favicon.svg",
        "image/svg+xml",
    ),
    (
        "public-sans.woff2",
        "servers/web-shared/fonts/public-sans.woff2",
        FONT,
    ),
    (
        "plex-mono-400.woff2",
        "servers/web-shared/fonts/plex-mono-400.woff2",
        FONT,
    ),
    (
        "plex-mono-500.woff2",
        "servers/web-shared/fonts/plex-mono-500.woff2",
        FONT,
    ),
    ("door.css", "servers/smarthome/web/door.css", CSS),
    ("door-ha.css", "servers/smarthome/web/door-ha.css", CSS),
    ("door-signin.js", "servers/smarthome/web/door-signin.js", JS),
    ("door-people.js", "servers/smarthome/web/door-people.js", JS),
    ("door-pair.js", "servers/smarthome/web/door-pair.js", JS),
];

pub const PAGES: &[(&str, &str)] = &[
    ("signin", "servers/smarthome/web/pages/door-signin.html"),
    ("people", "servers/smarthome/web/pages/door-people.html"),
    ("pair", "servers/smarthome/web/pages/door-pair.html"),
    // what goes into Home Assistant's pages, after <body>
    ("ha-bar", "servers/smarthome/web/ha-bar.html"),
];

const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const FONT: &str = "font/woff2";

struct File {
    bytes: Vec<u8>,
    etag: String,
}

pub struct Assets {
    files: HashMap<&'static str, File>,
}

impl Assets {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let mut files = HashMap::new();
        let mut missing = Vec::new();
        for p in STATIC
            .iter()
            .map(|(_, p, _)| *p)
            .chain(PAGES.iter().map(|(_, p)| *p))
        {
            match std::fs::read(dir.join(p)) {
                Ok(bytes) => {
                    use sha2::Digest as _;
                    let d = sha2::Sha256::digest(&bytes);
                    let etag = format!(
                        "\"{}\"",
                        d[..12]
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    );
                    files.insert(p, File { bytes, etag });
                }
                Err(_) => missing.push(p),
            }
        }
        anyhow::ensure!(
            missing.is_empty(),
            "the pages directory {} is missing {}",
            dir.display(),
            missing.join(", ")
        );
        Ok(Self { files })
    }

    /// For the tests: DOOR_PAGES_DIR where the build hands the pages over
    /// (the flake's per-crate test), else the checkout itself.
    pub fn tree() -> anyhow::Result<Self> {
        match std::env::var_os("DOOR_PAGES_DIR") {
            Some(d) => Self::load(Path::new(&d)),
            None => Self::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")),
        }
    }

    pub fn page(&self, name: &str) -> &[u8] {
        let path = PAGES.iter().find(|(n, _)| *n == name).map(|(_, p)| *p);
        path.and_then(|p| self.files.get(p))
            .map(|f| f.bytes.as_slice())
            .unwrap_or_default()
    }

    pub fn page_response(&self, name: &str, status: StatusCode) -> Response {
        (
            status,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            self.page(name).to_vec(),
        )
            .into_response()
    }

    pub fn serve(&self, name: &str, headers: &HeaderMap) -> Response {
        let Some((_, path, kind)) = STATIC.iter().find(|(n, _, _)| *n == name) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(f) = self.files.get(path) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let cache = if *kind == FONT {
            "public, max-age=604800"
        } else {
            "no-cache"
        };
        let etag = HeaderValue::from_str(&f.etag).expect("hex");
        if headers.get(header::IF_NONE_MATCH) == Some(&etag) {
            return (
                StatusCode::NOT_MODIFIED,
                [
                    (header::ETAG, etag),
                    (header::CACHE_CONTROL, cache.parse().unwrap()),
                ],
            )
                .into_response();
        }
        (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(kind)),
                (header::ETAG, etag),
                (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            ],
            f.bytes.clone(),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_in_the_tree_and_the_pages_load_only_what_is_served() {
        let a = Assets::tree().unwrap();
        for (name, _) in PAGES {
            let page = String::from_utf8(a.page(name).to_vec()).unwrap();
            for src in page
                .split("/_door/static/")
                .skip(1)
                .map(|s| s.split(['"', '\'', ')']).next().unwrap())
            {
                assert!(
                    STATIC.iter().any(|(n, _, _)| *n == src),
                    "{name} loads {src}, which is not served"
                );
            }
            assert!(!page.contains("<script>"), "{name}: no inline script");
        }
    }

    #[test]
    fn a_file_the_browser_holds_is_not_sent_again() {
        let a = Assets::tree().unwrap();
        let r = a.serve("door.css", &HeaderMap::new());
        assert_eq!(r.status(), StatusCode::OK);
        let etag = r.headers()[header::ETAG].clone();
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, etag);
        assert_eq!(a.serve("door.css", &h).status(), StatusCode::NOT_MODIFIED);
        assert_eq!(a.serve("nope.js", &h).status(), StatusCode::NOT_FOUND);
    }
}
