//! The verifier's pages: what each template gets. The HTML is in
//! templates/, the look in web/home.css, the scripts in web/*.js, served
//! from /_dd/static/.

use askama::Template;

/// One tile on the home page. The roles a box runs declare these.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub url: String,
    /// photos, videos, files, chat, code, metrics, games, or anything else for a plain mark
    pub icon: String,
    /// a css colour for the tile's icon
    pub color: String,
    /// one line under the name on the home page: what it is for
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blurb: String,
    /// What the demo account may do here, as the gate enforces it: `full`
    /// (the service's own permissions are the limit), `read` (no writing
    /// method), `rate:N` or `rate:N/M` (reads free, N other requests an
    /// hour for each demo, M for all demos together). None:
    /// nothing, and the tile is greyed on its home page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demo: Option<String>,
    /// Where the demo goes instead. A member's Movies & TV is their own
    /// library; the demo has none, and gets the box's own films.
    #[serde(rename = "demoUrl", default, skip_serializing_if = "Option::is_none")]
    pub demo_url: Option<String>,
    /// In the bar's menu and not on the home page. Looking at the fleet
    /// is not a service the way photos and films are, and a tile for it
    /// sits oddly beside them.
    #[serde(
        rename = "menuOnly",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub menu_only: bool,
}

/// The account that needs no invite and no key: a look at what a member
/// sees, with nothing of their own and nothing kept.
pub const DEMO_USER: &str = "demo";

// The pages, scripts, styles, fonts and pictures are files, read from a
// directory laid out like the tree (servers/web-shared/..., servers/<service>/web/...),
// not compiled into the gate: a release builds them into a directory of
// their own, so a page changing is not the gate being rebuilt, its tests
// run again, or its checks redone. All of them are read once, at start,
// and a missing one stops the gate starting rather than a page 404ing.
static ASSETS: std::sync::OnceLock<std::collections::HashMap<&'static str, Vec<u8>>> =
    std::sync::OnceLock::new();

fn every_path() -> impl Iterator<Item = &'static str> {
    STATIC
        .iter()
        .map(|(_, p, _)| *p)
        .chain(PAGES.iter().map(|(_, p)| *p))
        .chain(ICONS.iter().map(|(_, p)| *p))
        .chain(OS_ICONS.iter().map(|(_, p)| *p))
}

fn read_all(
    dir: &std::path::Path,
) -> anyhow::Result<std::collections::HashMap<&'static str, Vec<u8>>> {
    let mut out = std::collections::HashMap::new();
    let mut missing = Vec::new();
    for p in every_path() {
        match std::fs::read(dir.join(p)) {
            Ok(b) => {
                out.insert(p, b);
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
    Ok(out)
}

/// Read every file from `dir` now; the gate's start calls this, so a
/// release missing one does not come up at all.
pub fn load(dir: &std::path::Path) -> anyhow::Result<()> {
    let all = read_all(dir)?;
    let _ = ASSETS.set(all);
    Ok(())
}

/// the tree itself, where the gate runs from a checkout (its tests)
pub(crate) fn tree() -> std::path::PathBuf {
    std::env::var_os("VERIFY_PAGES_DIR")
        .map(Into::into)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

fn asset(path: &str) -> &'static [u8] {
    ASSETS
        .get_or_init(|| read_all(&tree()).unwrap_or_else(|e| panic!("{e:#}")))
        .get(path)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// Every file the gate serves as it is, by the name under /_dd/static/:
/// where it lives in the tree and its type.
const STATIC: &[(&str, &str, &str)] = &[
    (
        "public-sans.woff2",
        "servers/web-shared/fonts/public-sans.woff2",
        "font/woff2",
    ),
    (
        "plex-mono-400.woff2",
        "servers/web-shared/fonts/plex-mono-400.woff2",
        "font/woff2",
    ),
    (
        "plex-mono-500.woff2",
        "servers/web-shared/fonts/plex-mono-500.woff2",
        "font/woff2",
    ),
    (
        "fonts-license.txt",
        "servers/web-shared/fonts/LICENSE",
        "text/plain; charset=utf-8",
    ),
    (
        "home.css",
        "servers/web-shared/home.css",
        "text/css; charset=utf-8",
    ),
    (
        "bar.css",
        "servers/web-shared/bar.css",
        "text/css; charset=utf-8",
    ),
    (
        "favicon.svg",
        "servers/web-shared/favicon.svg",
        "image/svg+xml",
    ),
    (
        "webauthn.js",
        "servers/web-shared/webauthn.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "age.js",
        "servers/web-shared/age.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "invite.js",
        "servers/gate/web/invite.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "login.js",
        "servers/gate/web/login.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "join.js",
        "servers/gate/web/join.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "friends.js",
        "servers/home/web/friends.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "chat.js",
        "servers/chat/web/chat.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "chat.css",
        "servers/chat/web/chat.css",
        "text/css; charset=utf-8",
    ),
    (
        "git.js",
        "servers/forge/web/git.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git-core.js",
        "servers/forge/web/git-core.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git-charts.js",
        "servers/forge/web/git-charts.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git-diff.js",
        "servers/forge/web/git-diff.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git-home.js",
        "servers/forge/web/git-home.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git-repo.js",
        "servers/forge/web/git-repo.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git-pulls.js",
        "servers/forge/web/git-pulls.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git-actions.js",
        "servers/forge/web/git-actions.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git-settings.js",
        "servers/forge/web/git-settings.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "git.css",
        "servers/forge/web/git.css",
        "text/css; charset=utf-8",
    ),
    (
        "metrics.js",
        "servers/observe/web/metrics.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "metrics.css",
        "servers/observe/web/metrics.css",
        "text/css; charset=utf-8",
    ),
    (
        "send.js",
        "servers/vault/web/send.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "send.css",
        "servers/vault/web/send.css",
        "text/css; charset=utf-8",
    ),
    (
        "adblock.js",
        "servers/adblock/web/adblock.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "storage.js",
        "servers/home/web/storage.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "settings.js",
        "servers/home/web/settings.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "leave.js",
        "servers/home/web/leave.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "zip.js",
        "servers/home/web/zip.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "storage.css",
        "servers/home/web/storage.css",
        "text/css; charset=utf-8",
    ),
    (
        "backups.css",
        "servers/fleet/web/backups.css",
        "text/css; charset=utf-8",
    ),
    (
        "settings.css",
        "servers/home/web/settings.css",
        "text/css; charset=utf-8",
    ),
    (
        "add.js",
        "servers/home/web/add.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "use-app.js",
        "servers/home/web/use-app.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "demo-photos.js",
        "servers/photos/web/demo-photos.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "demo-photos.css",
        "servers/photos/web/demo-photos.css",
        "text/css; charset=utf-8",
    ),
    (
        "adblock.css",
        "servers/adblock/web/adblock.css",
        "text/css; charset=utf-8",
    ),
    (
        "enrol.js",
        "servers/gate/web/enrol.js",
        "text/javascript; charset=utf-8",
    ),
    // the one page that asks a passkey's PRF, for the old photo password
    // the app moves the account off (photos.rs)
    (
        "photos-move.js",
        "servers/photos/web/photos-move.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "files.js",
        "servers/files/web/files.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "media.js",
        "servers/media/web/media.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "shelf.js",
        "servers/media/web/shelf.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "tmdb.svg",
        "servers/media/web/icons/tmdb.svg",
        "image/svg+xml",
    ),
    (
        "library.js",
        "servers/home/web/library.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "backups.js",
        "servers/fleet/web/backups.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "devices.js",
        "servers/fleet/web/devices.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "network.js",
        "servers/fleet/web/network.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "shell.js",
        "servers/web-shared/shell.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "footer.js",
        "servers/web-shared/footer.js",
        "text/javascript; charset=utf-8",
    ),
    // the assistant in the corner of every signed-in page (sitemap.rs)
    (
        "guide.js",
        "servers/web-shared/guide.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "guide-look.js",
        "servers/web-shared/guide-look.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "guide-keep.js",
        "servers/web-shared/guide-keep.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "guide-face.js",
        "servers/web-shared/guide-face.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "guide-tour.js",
        "servers/web-shared/guide-tour.js",
        "text/javascript; charset=utf-8",
    ),
    // the docs' drawing of the site map
    (
        "docs-map.js",
        "servers/docs/web/docs-map.js",
        "text/javascript; charset=utf-8",
    ),
    // what every page shares (docs/dev/ui.md)
    (
        "kit.js",
        "servers/web-shared/kit.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "controller-name.js",
        "servers/home/web/controller-name.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "docs.js",
        "servers/docs/web/docs.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "docs.css",
        "servers/docs/web/docs.css",
        "text/css; charset=utf-8",
    ),
    (
        "landing.js",
        "servers/landing/web/landing.js",
        "text/javascript; charset=utf-8",
    ),
    (
        "landing.css",
        "servers/landing/web/landing.css",
        "text/css; charset=utf-8",
    ),
];

/// the stylesheet, the scripts, the fonts and the pictures, by the name
/// under /_dd/static/
pub fn static_file(name: &str) -> Option<(&'static [u8], &'static str, &'static str)> {
    STATIC
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, path, ty)| (asset(path), *ty, etag(path)))
}

/// A file's tag for the browser's "has it changed": the same bytes, the
/// same tag, so a page opened again is told 304 rather than sent it all.
pub fn tag_of(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(bytes);
    format!(
        "\"{}\"",
        h[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

static ETAGS: std::sync::OnceLock<std::collections::HashMap<&'static str, String>> =
    std::sync::OnceLock::new();

fn etag(path: &str) -> &'static str {
    ETAGS
        .get_or_init(|| every_path().map(|p| (p, tag_of(asset(p)))).collect())
        .get(path)
        .map(String::as_str)
        .unwrap_or_default()
}

/// What kind of account is looking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Member,
    /// the one account every visitor shares
    Demo,
    /// in through a friend link: invited game servers, and nothing else
    Guest,
    /// suspended for breaking the terms: reads and takes their own files,
    /// and nothing else, until their things are deleted
    Suspended,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Member => "member",
            Role::Demo => "demo",
            Role::Guest => "guest",
            Role::Suspended => "suspended",
        }
    }
}

/// the mark on a tile: an svg fragment from web/icons/, by the role's name
const ICONS: &[(&str, &str)] = &[
    ("photos", "servers/web-shared/icons/photos.svg"),
    ("videos", "servers/web-shared/icons/videos.svg"),
    ("files", "servers/web-shared/icons/files.svg"),
    ("chat", "servers/web-shared/icons/chat.svg"),
    ("code", "servers/web-shared/icons/code.svg"),
    ("metrics", "servers/web-shared/icons/metrics.svg"),
    ("games", "servers/web-shared/icons/games.svg"),
    ("computer", "servers/web-shared/icons/computer.svg"),
    ("passwords", "servers/web-shared/icons/passwords.svg"),
    ("plain", "servers/web-shared/icons/plain.svg"),
];

fn icon(key: &str) -> &'static str {
    let path = ICONS
        .iter()
        .find(|(k, _)| *k == key)
        .or_else(|| ICONS.iter().find(|(k, _)| *k == "plain"))
        .map(|(_, p)| *p)
        .unwrap_or_default();
    std::str::from_utf8(asset(path)).unwrap_or_default()
}

pub(crate) fn initial(user: &str) -> String {
    user.chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_default()
}

/// A signed-in page: a static file that fills itself from /_dd/me
/// (web/pages/). The app carries the same files.
const PAGES: &[(&str, &str)] = &[
    ("home", "servers/home/web/pages/home.html"),
    ("files", "servers/files/web/pages/files.html"),
    ("media", "servers/media/web/pages/media.html"),
    ("friends", "servers/home/web/pages/friends.html"),
    ("chat", "servers/chat/web/pages/chat.html"),
    ("git", "servers/forge/web/pages/git.html"),
    // not signed in: anyone, on the forge's public name, read-only
    ("git-public", "servers/forge/web/pages/git-public.html"),
    ("metrics", "servers/observe/web/pages/metrics.html"),
    ("friend", "servers/home/web/pages/friend.html"),
    ("adblock", "servers/adblock/web/pages/adblock.html"),
    // the demo's Photos, read through the gate (demo_photos.rs)
    ("demo-photos", "servers/photos/web/pages/demo-photos.html"),
    // the member's own: their account, their email (the mail Worker,
    // client/mail, in a frame) and their disk images
    ("settings", "servers/home/web/pages/settings.html"),
    // not signed in: whoever has a Send's link (modules/vault)
    ("send", "servers/vault/web/pages/send.html"),
    // not signed in: the new device a QR code was scanned on (adddevice.rs)
    ("add", "servers/home/web/pages/add.html"),
    // not signed in: photos.<domain> and vault.<domain> in a browser, whose
    // clients come with the app (modules/photos, modules/vault)
    ("use-app", "servers/home/web/pages/use-app.html"),
    // the assistant's conversation for this tab, across the site's names (guide-keep.js)
    ("guide-keep", "servers/home/web/pages/guide-keep.html"),
    // a member naming their home controller with the passkey (controller.rs)
    (
        "controller-name",
        "servers/home/web/pages/controller-name.html",
    ),
    // not signed in: anyone, at <domain>/docs (docs.rs)
    ("docs", "servers/docs/web/pages/docs.html"),
    // not signed in: anyone, at the bare name's front (roles/gateway.nix)
    ("landing", "servers/landing/web/pages/landing.html"),
];

pub fn page(name: &str) -> Option<&'static str> {
    PAGES
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, p)| std::str::from_utf8(asset(p)).unwrap_or_default())
}

/// Everything a signed-in page shows about who is looking: the name, the
/// bar's menu, and the services, with the demo's shut doors marked. The
/// pages are static files that fill themselves from this (web/shell.js),
/// the same files in a browser and in the app.
pub fn me_json(user: &str, services: &[Service], role: Role, domain: &str) -> serde_json::Value {
    let demo = user == DEMO_USER;
    let guest = role == Role::Guest;
    let suspended = role == Role::Suspended;
    let menu = if guest || suspended {
        Menu::guest()
    } else {
        Menu::of(user, services)
    };
    // a guest is shown the one door the gate opens for them, a suspended
    // member theirs: their files, to take them
    let games = format!("games.{domain}");
    let files = format!("files.{domain}");
    let services: Vec<&Service> = services
        .iter()
        .filter(|s| !guest || host_of(&s.url) == games)
        .filter(|s| !suspended || host_of(&s.url) == files)
        .collect();
    serde_json::json!({
        "user": user,
        "initial": initial(user),
        "demo": demo,
        "role": role.as_str(),
        // the front door, for links that must land there wherever this page is
        "home": format!("https://home.{domain}"),
        "menu": menu.groups.iter().map(|g| g.iter().map(|i| serde_json::json!({
            "label": i.label,
            "url": i.url,
        })).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "services": services.iter().filter(|s| !s.menu_only).map(|s| serde_json::json!({
            "name": s.name,
            "url": match (demo, &s.demo_url) {
                (true, Some(u)) => u,
                _ => &s.url,
            },
            "icon": icon(&s.icon),
            "blurb": s.blurb,
            "color": s.color,
            "host": host_of(&s.url),
            "shut": demo && s.demo.is_none(),
        })).collect::<Vec<_>>(),
    })
}

/// One line in the bar's menu.
pub struct MenuItem {
    pub label: &'static str,
    pub url: String,
}

/// Everything in the bar that is not a service: the member's own pages,
/// the fleet's, and the way out. Groups are drawn with a rule between.
pub struct Menu {
    pub groups: Vec<Vec<MenuItem>>,
}

impl Menu {
    /// A guest's: their own settings (devices, sign-ins), the network their
    /// game servers are on, and the way out.
    pub(crate) fn guest() -> Menu {
        let item = |label, url: &str| MenuItem {
            label,
            url: url.to_string(),
        };
        Menu {
            groups: vec![
                vec![item("Settings", "/_dd/settings")],
                vec![item("Sign out", "/_dd/logout")],
            ],
        }
    }

    /// What this person may actually open. The demo has no library, no
    /// devices and no backups, so it is offered none of them.
    pub(crate) fn of(user: &str, services: &[Service]) -> Menu {
        let item = |label, url: &str| MenuItem {
            label,
            url: url.to_string(),
        };
        let demo = user == DEMO_USER;
        // never the demo's: the gate refuses it there (metrics_page, verify)
        let metrics = services
            .iter()
            .find(|s| s.icon == "metrics" && !demo)
            .map(|s| item("Metrics", &s.url));
        // The tile's url, where a tile says. A library page belongs on the
        // gate's own host: that is the only one serving /_dd/transcode, so
        // a relative link followed from another host plays nothing.
        // Files and Movies & TV are tiles on the home page, and everything
        // of a member's own is in Settings: the menu repeats neither
        let mut groups = Vec::new();
        if let Some(m) = metrics {
            groups.push(vec![m]);
        }
        if demo {
            groups.push(vec![item("Sign out", "/_dd/logout")]);
        } else {
            groups.push(vec![
                item("Settings", "/_dd/settings"),
                item("Sign out", "/_dd/logout"),
            ]);
        }
        Menu { groups }
    }
}

#[derive(Template)]
#[template(path = "login.html")]
struct Login<'a> {
    domain: &'a str,
}

#[derive(Template)]
#[template(path = "enrol.html")]
struct Enrol;

#[derive(Template)]
#[template(path = "join.html")]
struct Join<'a> {
    domain: &'a str,
}

#[derive(Template)]
#[template(path = "confirm.html")]
struct Confirm<'a> {
    title: &'a str,
    lead: &'a str,
    action: &'a str,
    button: &'a str,
}

#[derive(Template)]
#[template(path = "waiting.html")]
struct Waiting<'a> {
    user: &'a str,
    initial: String,
    menu: Menu,
}

/// one platform's card on the downloads page
struct Platform {
    name: &'static str,
    icon: &'static str,
    /// what to call the file, where it is, and its sha256; empty means not
    /// yet, or nothing signed for this platform
    files: Vec<(&'static str, String, String)>,
    /// shown under the name when there is nothing to download
    soon: &'static str,
}

struct Group {
    title: &'static str,
    platforms: Vec<Platform>,
}

#[derive(Template)]
#[template(path = "download.html")]
struct Download<'a> {
    domain: &'a str,
    /// the source the files were built from, at the commit
    source: String,
    tag: &'a str,
    /// the signed manifest itself, for anyone who wants to check
    manifest: &'a str,
    groups: Vec<Group>,
}

/// A file the release key has vouched for.
#[derive(Clone)]
pub struct AppFile {
    pub url: String,
    pub sha256: String,
}

/// the logo on a platform's card, from web/icons/os/
const OS_ICONS: &[(&str, &str)] = &[
    ("linux", "servers/gate/web/icons/os/linux.svg"),
    ("android", "servers/gate/web/icons/os/android.svg"),
    ("apple", "servers/gate/web/icons/os/apple.svg"),
    ("windows", "servers/gate/web/icons/os/windows.svg"),
];

fn os_icon(key: &str) -> &'static str {
    let path = OS_ICONS
        .iter()
        .find(|(k, _)| *k == key)
        .or_else(|| OS_ICONS.iter().find(|(k, _)| *k == "windows"))
        .map(|(_, p)| *p)
        .unwrap_or_default();
    std::str::from_utf8(asset(path)).unwrap_or_default()
}

/// Where a stranger gets the app. Public: someone invited has nothing to
/// sign in with until they have it. The artifacts are built after a tag
/// and published as a release on the mirror, so the links point there by
/// that tag, not at a file this box holds. A platform with no files is
/// shown anyway, so the page says what is coming rather than hiding it.
/// The download page from a signed app manifest: a file is offered only if
/// the manifest names it, with the hash the release key signed.
pub fn download(
    domain: &str,
    from: &str,
    commit: &str,
    tag: &str,
    manifest: &str,
    signed: &std::collections::BTreeMap<String, AppFile>,
) -> String {
    let pick = |label: &'static str, name: &str| -> Option<(&'static str, String, String)> {
        signed
            .get(name)
            .map(|f| (label, f.url.clone(), f.sha256.clone()))
    };
    let v = tag.trim_start_matches('v');
    let files = |want: Vec<(&'static str, String)>| -> Vec<(&'static str, String, String)> {
        want.into_iter().filter_map(|(l, n)| pick(l, &n)).collect()
    };
    let groups = vec![
        Group {
            title: "Desktop",
            platforms: vec![
                Platform {
                    name: "Linux",
                    icon: os_icon("linux"),
                    files: files(vec![
                        (".deb", format!("commonty_{v}_amd64.deb")),
                        ("AppImage", format!("commonty_{v}_amd64.AppImage")),
                    ]),
                    soon: "Nothing signed yet",
                },
                Platform {
                    name: "Windows",
                    icon: os_icon("windows"),
                    files: files(vec![("Installer", "commonty-setup.exe".into())]),
                    soon: "Nothing signed yet",
                },
                Platform {
                    name: "macOS",
                    icon: os_icon("apple"),
                    files: vec![],
                    soon: "Not built yet",
                },
            ],
        },
        Group {
            title: "Mobile",
            platforms: vec![
                Platform {
                    name: "Android",
                    icon: os_icon("android"),
                    files: files(vec![("APK", "commonty.apk".into())]),
                    soon: "Nothing signed yet",
                },
                Platform {
                    name: "iOS",
                    icon: os_icon("apple"),
                    files: vec![],
                    soon: "Not built yet",
                },
            ],
        },
    ];
    render(Download {
        domain,
        // the forge the app was released from; a manifest from before
        // names its GitHub repository instead
        source: if from.starts_with("https://") {
            format!("{from}/src/commit/{commit}")
        } else {
            format!("https://github.com/{from}/tree/{commit}")
        },
        tag,
        manifest,
        groups,
    })
}

/// a url's host: what a row shows as where the service lives
fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', ':'])
        .next()
        .unwrap_or_default()
}

pub(crate) fn render<T: Template>(t: T) -> String {
    t.render().unwrap_or_default()
}

/// Sign in: username, then the passkey. Carries the domain so it can
/// point at the downloads page, which lives on the bare name.
pub fn login(domain: &str) -> String {
    render(Login { domain })
}

/// Set up a passkey, from a link a device signed.
pub fn enrol() -> String {
    render(Enrol)
}

/// An account, from nothing, in the browser: a name and a passkey.
pub fn join(domain: &str) -> String {
    render(Join { domain })
}

/// One button that POSTs to `action`: what a link to something that
/// changes the session lands on, so another site's link cannot do it.
pub fn confirm(title: &str, lead: &str, action: &str, button: &str) -> String {
    render(Confirm {
        title,
        lead,
        action,
        button,
    })
}

/// Signed in but not on the member list: the account exists, nothing is
/// open to it yet. A code from the owner opens it here.
pub fn waiting(user: &str, services: &[Service]) -> String {
    render(Waiting {
        user,
        initial: initial(user),
        menu: Menu::of(user, services),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(name: &str) -> &'static str {
        std::str::from_utf8(static_file(name).unwrap().0).unwrap()
    }

    fn svc(name: &str, icon: &str) -> Service {
        Service {
            name: name.into(),
            url: format!("https://{}.example/", name.to_lowercase()),
            icon: icon.into(),
            color: "#123456".into(),
            blurb: String::new(),
            demo: None,
            demo_url: None,
            menu_only: false,
        }
    }

    #[test]
    fn auth_pages_are_one_shell_one_script() {
        for page in [login("commonty.org"), enrol(), join("commonty.org")] {
            assert!(page.starts_with("<!doctype html>"));
            assert!(page.trim_end().ends_with("</html>"));
            // the page's own script, and the bar's (its docs and search)
            assert_eq!(page.matches("<script").count(), 2);
            assert!(page.contains("/_dd/static/shell.js"));
            assert_eq!(page.matches("</header>").count(), 1);
            assert!(page.contains("/_dd/static/home.css"));
        }
        // nobody signing in is told to use a command line
        assert!(!login("commonty.org").contains("dd enrol"));
        assert!(!enrol().contains("dd enrol"));
        assert!(login("commonty.org").contains("/_dd/static/login.js"));
        assert!(login("commonty.org").contains("href=\"/_dd/join\""));
        // somebody signing in on a phone needs the app before any of this
        for html in [login("commonty.org"), join("commonty.org")] {
            assert!(
                html.contains("https://home.commonty.org/_dd/download"),
                "no way to the app from a page a stranger lands on"
            );
        }
        assert!(enrol().contains("/_dd/static/enrol.js"));
        assert!(join("commonty.org").contains("/_dd/static/join.js"));
        for f in ["login.js", "join.js", "enrol.js", "photos-move.js"] {
            assert!(static_file(f).is_some(), "{f}");
        }
        assert!(text("login.js").contains("/_dd/login/start"));
        assert!(text("join.js").contains("/_dd/join/sign"));
        assert!(text("enrol.js").contains("/_dd/enrol/start"));
        // an invite code is typed into the app, never a box's page
        for f in ["join.js", "login.js", "enrol.js"] {
            assert!(!text(f).contains("x-dd-grant"), "{f}");
        }
        assert!(static_file("redeem.js").is_none());
        assert!(static_file("nope").is_none());
    }

    /// One footer, from one file: every page ends with it, through the
    /// shell or on its own (docs/dev/ui.md)
    #[test]
    fn every_page_ends_with_the_one_footer() {
        assert!(text("shell.js").contains("import './footer.js';"));
        assert!(static_file("footer.js").is_some());
        let base = include_str!("../templates/base.html");
        assert!(base.contains("/_dd/static/shell.js"));
        // the assistant's conversation frame is never shown: no page to end
        for (name, _) in PAGES.iter().filter(|(n, _)| *n != "guide-keep") {
            let html = page(name).unwrap();
            assert!(
                html.contains("/_dd/static/shell.js") || html.contains("/_dd/static/footer.js"),
                "{name} has no footer"
            );
            assert!(
                !html.contains("dd-foot"),
                "{name} keeps a footer of its own"
            );
        }
        let foot = text("footer.js");
        for path in [
            "/docs/policies/privacy",
            "/docs/policies/terms",
            "/docs/policies/security",
            "/docs/guide/joining",
            "/docs/guide/hosting",
            "/_dd/demo",
            "/download",
        ] {
            assert!(foot.contains(path), "{path}");
        }
        assert!(!foot.contains("commonty.org"));
    }

    /// A page's script that imports a sibling the gate does not serve
    /// fails whole, and the page with it (leave.js and its zip.js)
    #[test]
    fn every_script_a_script_imports_is_served() {
        for (name, _, kind) in STATIC {
            if !kind.starts_with("text/javascript") {
                continue;
            }
            let src = text(name);
            for part in src.split("from './").skip(1) {
                let Some(dep) = part.split('\'').next() else {
                    continue;
                };
                assert!(static_file(dep).is_some(), "{name} imports {dep}");
            }
        }
    }

    /// The browser's sign-up asks month and year of birth, nothing chosen,
    /// and an unticked "I agree" to both policies; the answer to the age
    /// question stays in the page.
    #[test]
    fn the_sign_up_asks_age_and_agreement_and_sends_neither_date() {
        let page = join("commonty.org");
        for id in [
            "born-month",
            "born-year",
            "guardian",
            "agree",
            "age-refused",
        ] {
            assert!(page.contains(&format!("id=\"{id}\"")), "{id}");
        }
        assert!(
            !page.contains("checked>") && !page.contains("checked="),
            "a box ticked for them"
        );
        assert!(!page.contains("selected"), "an answer chosen for them");
        assert!(page.contains("https://commonty.org/docs/policies/terms"));
        assert!(page.contains("https://commonty.org/docs/policies/privacy"));
        assert!(page.contains("<button id=\"go\" disabled>"));
        let js = text("join.js");
        assert!(js.contains("from './age.js'"));
        assert!(!js.contains("born"), "the birth date is sent");
        assert!(static_file("age.js").is_some());
        assert!(text("age.js").contains("localStorage"));
        assert!(text("age.js").contains("document.cookie"));
    }

    /// Every file a box serves to a browser (servers/**/web), read for the
    /// passkey PRF extension. A passkey answers it to any page of the
    /// domain, so no page may make a key of it. One page is let through,
    /// the move of an old photo account to the app, and only for the label
    /// that account was made with: what it gets opens nothing once moved.
    #[test]
    fn no_box_page_asks_a_passkey_for_a_key() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if matches!(
                    p.extension().and_then(|x| x.to_str()),
                    Some("js" | "mjs" | "html")
                ) {
                    out.push(p);
                }
            }
        }
        let root = tree().join("servers");
        let mut files = Vec::new();
        for s in std::fs::read_dir(&root).unwrap().flatten() {
            walk(&s.path().join("web"), &mut files);
        }
        walk(&root.join("web-shared"), &mut files);
        assert!(
            files.len() > 30,
            "only {} pages found under {root:?}",
            files.len()
        );
        let asks = |t: &str| {
            let b = t.as_bytes();
            (0..b.len().saturating_sub(2)).any(|i| {
                b[i..i + 3].eq_ignore_ascii_case(b"prf")
                    && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
                    && b.get(i + 3).is_none_or(|c| !c.is_ascii_alphanumeric())
            }) || t.contains("hmac-secret")
                || t.contains("largeBlob")
        };
        let mut found: Vec<String> = files
            .iter()
            .filter(|p| asks(&std::fs::read_to_string(p).unwrap_or_default()))
            .map(|p| p.strip_prefix(&root).unwrap().display().to_string())
            .collect();
        found.sort();
        assert_eq!(found, ["photos/web/photos-move.js"]);
        // an address a box or a search hands over is linked only once it is
        // shown to be a web one (kit.js webUrl), on the pages that hold keys
        assert!(text("kit.js").contains("x.protocol === 'https:' || x.protocol === 'http:'"));
        for f in ["chat.js", "shell.js"] {
            assert!(text(f).contains("webUrl("), "{f} links what it is given");
        }
        assert!(!text("chat.js").contains("href: r.url"));
        // and the bar's links to the fleet are held to the domain the app
        // writes into its pages, not one a box names
        assert!(text("shell.js").contains("meta[name=\"dd-domain\"]"));
        let mover = text("photos-move.js");
        assert!(mover.contains("'dd-photos'"));
        for other in [
            "dd-library",
            "library_",
            "file_seal",
            "/_dd/dav",
            "/_dd/images",
        ] {
            assert!(!mover.contains(other), "the move page touches {other}");
        }
    }

    /// What the assistant's model says is data: shown as words, never as
    /// markup or code, and a place is only ever an id the map holds, which
    /// the page's own code turns into where to go (guide.js)
    #[test]
    fn the_assistant_writes_no_markup_and_goes_only_to_the_map() {
        for f in [
            "guide.js",
            "guide-face.js",
            "guide-tour.js",
            "guide-keep.js",
            "guide-look.js",
            "docs-map.js",
        ] {
            let js = text(f);
            for not in [
                "innerHTML",
                "outerHTML",
                "insertAdjacentHTML",
                "eval(",
                "new Function",
                "document.write",
            ] {
                assert!(!js.contains(not), "{f}: {not}");
            }
        }
        let g = text("guide.js");
        assert!(g.contains("Object.hasOwn(places, id)"));
        assert!(g.contains("const n = byId[id];") && g.contains("opts.here(url"));
        // a control is found by the name our pages give it, never by words
        let t = text("guide-tour.js");
        assert!(t.contains("querySelectorAll(`[data-guide=\"${name}\"]`)"));
        assert!(t.contains("const NAME = /^[a-z0-9-]{1,40}$/;"));
        for not in [
            "textContent.includes",
            "innerText",
            ".textContent ===",
            "querySelectorAll('*')",
        ] {
            assert!(!t.contains(not) && !g.contains(not), "{not}");
        }
        // a drafted issue is words in a query, opened for the member to post
        assert!(g.contains("u.searchParams.set('title', issue.title)"));
        assert!(
            !g.contains("/issues', { method: 'POST'")
                && !g.contains("method: 'POST', body: { title")
        );
        // the conversation's frame answers only the pages the assistant runs on
        let k = text("guide-keep.js");
        assert!(k.contains("if (e.source !== parent || !ours.test(e.origin)) return;"));
        assert!(k.contains(
            "parent.postMessage({ op: d.op === 'get' ? 'got' : 'kept', id, v }, e.origin);"
        ));
        assert!(!k.contains("localStorage") && !k.contains("fetch("));
        assert!(g.contains("e.source !== f.contentWindow || e.origin !== origin"));
        // what it looks up of a member's own is read, never written: no
        // method but GET, and nothing Locked (privacy.md)
        let l = text("guide-look.js");
        assert!(!l.contains("method:") && !l.contains("'POST'"));
        for not in [
            "/_dd/devices",
            "/_dd/files",
            "/_dd/media",
            "vault.",
            "/_dd/backups",
            "/_dd/app/",
        ] {
            assert!(!l.contains(not), "guide-look.js reads {not}");
        }
        assert!(text("shell.js").contains("import { start as guide } from './guide.js';"));
        assert!(text("shell.js").contains("export function here("));
    }

    #[test]
    fn the_downloads_page_asks_for_nothing_and_points_at_the_release() {
        let at = |n: &str| AppFile {
            url: format!("https://commonty.org/download/v0.2.0/{n}"),
            sha256: "ab".repeat(32),
        };
        let mut signed = std::collections::BTreeMap::new();
        for n in [
            "commonty.apk",
            "commonty_0.2.0_amd64.deb",
            "commonty_0.2.0_amd64.AppImage",
            "commonty-setup.exe",
        ] {
            signed.insert(n.to_string(), at(n));
        }
        let html = download(
            "commonty.org",
            "https://git.commonty.org/david/commonty",
            "abc123",
            "v0.2.0",
            "https://m/app.json",
            &signed,
        );
        assert!(html.contains("https://commonty.org/download/v0.2.0/commonty.apk"));
        // the source at the commit that was signed, and the manifest itself
        assert!(html.contains("https://git.commonty.org/david/commonty/src/commit/abc123"));
        assert!(html.contains("https://m/app.json"));
        assert!(html.contains("commonty_0.2.0_amd64.deb"));
        assert!(html.contains("commonty_0.2.0_amd64.AppImage"));
        // desktop and mobile, each with what is there and what is not
        assert!(html.contains("Desktop") && html.contains("Mobile"));
        for os in ["Linux", "macOS", "Windows", "Android", "iOS"] {
            assert!(html.contains(os), "no card for {os}");
        }
        // a platform with nothing to download says so and offers no link
        // only the two nobody has built yet are greyed
        assert_eq!(html.matches("class=\"os off\"").count(), 2);
        assert!(html.contains("commonty-setup.exe"));
        // what WinFsp's licence asks of us, in the interface
        assert!(html.contains("Bill Zissimopoulos") && html.contains("winfsp"));
        // the button takes the first format; the others are links beside it
        assert!(
            html.contains("Download .deb"),
            "the default is not on the button"
        );
        assert!(html.contains(">AppImage<"));
        // one button per platform with files; only Linux has a second format
        assert_eq!(html.matches("class=\"get\"").count(), 3);
        assert_eq!(html.matches("class=\"alt\"").count(), 1);
        // a stranger is who this is for: no name, no avatar, no menu
        assert!(!html.contains("class=\"me\"") && !html.contains("/_dd/logout"));
        assert!(html.contains("https://home.commonty.org/"));

        // a platform the manifest does not name is greyed, not linked
        signed.remove("commonty.apk");
        let html = download(
            "commonty.org",
            "https://git.commonty.org/david/commonty",
            "abc123",
            "v0.2.0",
            "https://m/app.json",
            &signed,
        );
        assert!(!html.contains("commonty.apk"));
        assert_eq!(html.matches("class=\"os off\"").count(), 3);
        // a manifest from the GitHub days names its repository, not a url
        let old = download(
            "commonty.org",
            "x/y",
            "abc123",
            "v0.2.0",
            "https://m/app.json",
            &signed,
        );
        assert!(old.contains("https://github.com/x/y/tree/abc123"));
        assert!(html.contains("Nothing signed yet"));
    }

    /// what a signed-in page fills itself from, as the page reads it
    fn me(user: &str, svcs: &[Service]) -> serde_json::Value {
        let role = if user == DEMO_USER {
            Role::Demo
        } else {
            Role::Member
        };
        me_json(user, svcs, role, "x")
    }

    #[test]
    fn a_guest_is_shown_the_games_door_alone() {
        let mut games = svc("Games", "games");
        games.url = "https://games.x/".into();
        let mut files = svc("Files", "files");
        files.url = "https://files.x/_dd/files".into();
        let m = me_json("tom", &[files, games], Role::Guest, "x");
        let names: Vec<&str> = m["services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Games"]);
        assert_eq!(m["role"], "guest");
        // their own devices and network, and the way out; none of the fleet's pages
        assert_eq!(menu_urls(&m), ["/_dd/settings", "/_dd/logout"]);
    }
    fn menu_urls(m: &serde_json::Value) -> Vec<String> {
        m["menu"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g.as_array().unwrap().iter())
            .map(|i| i["url"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn the_menu_offers_a_member_their_own_pages_and_the_demo_none_of_them() {
        let svcs = [svc("Metrics", "metrics"), svc("Chat", "chat")];
        let menu = menu_urls(&me("tom", &svcs));
        assert!(menu.iter().any(|u| u == "/_dd/settings"));
        // services on the home page, and what is a tab of Settings now
        for page in ["/_dd/files", "/_dd/media", "/_dd/boxes", "/_dd/storage"] {
            assert!(!menu.iter().any(|u| u == page), "the menu repeats {page}");
        }
        assert!(menu.iter().any(|u| u == "https://metrics.example/"));
        assert!(menu.iter().any(|u| u == "/_dd/logout"));
        // the demo opens the library the box keeps for it, and nothing
        // that belongs to an account with devices and boxes of its own
        let menu = menu_urls(&me(DEMO_USER, &svcs));
        assert!(
            !menu.iter().any(|u| u == "/_dd/settings"),
            "the demo was offered Settings"
        );
        // nor Metrics, even from a tile that grants it something
        assert!(!menu.iter().any(|u| u.contains("metrics")), "{menu:?}");
        let mut granted = svc("Metrics", "metrics");
        granted.demo = Some("read".into());
        let menu = menu_urls(&me(DEMO_USER, &[granted]));
        assert!(!menu.iter().any(|u| u.contains("metrics")), "{menu:?}");
        // no metrics on this box: no line for it, and nothing else moves
        let menu = menu_urls(&me("tom", &[svc("Chat", "chat")]));
        assert!(!menu.iter().any(|u| u.contains("metrics")));
        assert!(menu.iter().any(|u| u == "/_dd/settings"));
    }

    #[test]
    fn the_signed_in_pages_are_files_that_fill_themselves() {
        for (name, script) in [
            ("home", "shell.js"),
            ("files", "files.js"),
            ("media", "media.js"),
            ("settings", "settings.js"),
            ("git", "git.js"),
            ("metrics", "metrics.js"),
        ] {
            let html = page(name).unwrap();
            // nobody's name in the file: it comes from /_dd/me
            assert!(!html.contains("{{") && !html.contains("{%"), "{name}");
            assert!(html.contains("/_dd/static/shell.js"), "{name}");
            assert!(html.contains(&format!("/_dd/static/{script}")), "{name}");
            assert!(html.contains("/_dd/static/home.css"), "{name}");
        }
        assert!(
            page("media").unwrap().contains("Movies") && page("media").unwrap().contains("Shows")
        );
        // the public Git page is the same views, its bar marked as anyone's:
        // nobody there is sent to sign in, and nothing there asks for a name
        let public = page("git-public").unwrap();
        assert!(public.contains("data-public") && public.contains("/_dd/static/git.js"));
        assert!(!public.contains("/_dd/home") && !public.contains("git-bell"));
        assert!(text("shell.js").contains("/_dd/me"));
        // the scripts that need the name ask for it, rather than read the page
        for f in ["files.js", "media.js", "devices.js"] {
            assert!(text(f).contains("await me()"), "{f}");
        }
        // both library pages open the library through the one module
        for f in ["files.js", "media.js"] {
            assert!(text(f).contains("from './library.js'"));
        }
        assert!(text("library.js").contains("/_dd/dav/"));
        // a film goes through the box, with its block keys sealed to it
        let lib = text("library.js");
        assert!(lib.contains("/_dd/transcode/session") && lib.contains("library_blocks_for_box"));
        // the player comes from this box, never from someone else's
        assert!(lib.contains("'/_dd/web/hls.js'"));
        assert!(!lib.contains("http://") && !lib.contains("https://"));
    }

    #[test]
    fn waiting_page_names_the_person_and_offers_nothing() {
        let html = waiting("tom", &[]);
        assert!(html.contains("<span>tom</span>"));
        assert!(html.contains("data-user=\"tom\""));
        assert!(html.contains("/_dd/logout"));
        assert!(!html.contains("class=\"service\""));
        // a code goes into the app; nothing here takes one
        assert!(!html.contains("<input") && html.contains("/_dd/download"));
    }

    #[test]
    fn the_demo_sees_every_service_and_the_shut_ones_marked() {
        let mut files = svc("Files", "files");
        files.url = "https://files.x/".into();
        files.demo = Some("read".into());
        let mut chat = svc("Chat", "chat");
        chat.url = "https://llm.x/".into();
        let m = me(DEMO_USER, &[files.clone(), chat.clone()]);
        assert_eq!(m["demo"], true);
        assert_eq!(m["services"][0]["url"], "https://files.x/");
        assert_eq!(m["services"][0]["shut"], false);
        assert_eq!(m["services"][1]["shut"], true, "chat is not in the demo");
        let m = me("tom", &[files, chat]);
        assert_eq!(m["demo"], false);
        assert_eq!(m["services"][1]["shut"], false);
        // the banner is in the page, shown only when the answer says demo
        assert!(page("home").unwrap().contains("id=\"demo\""));
        assert!(text("shell.js").contains("m.demo"));
    }

    #[test]
    fn a_service_can_send_the_demo_somewhere_else() {
        let mut tv = svc("Games", "games");
        tv.url = "https://files.x/_dd/games".into();
        tv.demo_url = Some("https://games.x/demo".into());
        tv.demo = Some("full".into());
        assert_eq!(
            me("tom", &[tv.clone()])["services"][0]["url"],
            "https://files.x/_dd/games"
        );
        assert_eq!(
            me(DEMO_USER, &[tv])["services"][0]["url"],
            "https://games.x/demo"
        );
    }

    #[test]
    fn the_name_is_what_opens_the_menu() {
        // the shell builds the control from the name and the initial, and
        // puts nothing else in the bar beside it
        let shell = text("shell.js");
        assert!(shell.contains("el('summary'") && shell.contains("class: 'avatar'"));
        assert!(!shell.contains("aria-label': 'Menu'"));
        assert_eq!(me("tom", &[])["initial"], "t");
    }

    #[test]
    fn a_menu_only_service_is_in_the_menu_and_not_on_the_home_page() {
        let mut m = svc("Metrics", "metrics");
        m.menu_only = true;
        let answer = me("tom", &[svc("Photos", "photos"), m.clone()]);
        assert_eq!(
            answer["services"].as_array().unwrap().len(),
            1,
            "metrics got a row"
        );
        assert!(
            menu_urls(&answer)
                .iter()
                .any(|u| u == "https://metrics.example/")
        );
        // the name it is given comes from the service, so the json the
        // module emits and the field here have to agree
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("menuOnly"), "{json}");
        let back: Service = serde_json::from_str(&json).unwrap();
        assert!(back.menu_only);
    }

    #[test]
    fn a_row_per_service_with_its_mark() {
        let m = me("tom", &[svc("Photos", "photos"), svc("Odd", "odd")]);
        let rows = m["services"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "Photos");
        // where it lives, from its url
        assert_eq!(rows[0]["host"], "photos.example");
        assert_eq!(rows[0]["url"], "https://photos.example/");
        assert!(
            rows[0]["icon"].as_str().unwrap().contains("cx=\"7.5\""),
            "the photos mark"
        );
        assert!(
            rows[1]["icon"].as_str().unwrap().contains("rx=\"3\""),
            "the plain mark"
        );
        assert_eq!(m["user"], "tom");
        assert!(text("shell.js").contains("Nothing runs here yet"));
    }
}
