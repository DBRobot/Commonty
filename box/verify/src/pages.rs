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
    /// method), `rate:N` (reads free, N other requests an hour). None:
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

/// the stylesheet and the scripts, by the name under /_dd/static/
pub fn static_file(name: &str) -> Option<(&'static [u8], &'static str)> {
    let js = "text/javascript; charset=utf-8";
    let woff2 = "font/woff2";
    let text = |s: &'static str, ty| Some((s.as_bytes(), ty));
    // the typefaces the stylesheet names, under the licence in web/fonts
    match name {
        "public-sans.woff2" => {
            return Some((include_bytes!("../../web/fonts/public-sans.woff2"), woff2));
        }
        "plex-mono-400.woff2" => {
            return Some((include_bytes!("../../web/fonts/plex-mono-400.woff2"), woff2));
        }
        "plex-mono-500.woff2" => {
            return Some((include_bytes!("../../web/fonts/plex-mono-500.woff2"), woff2));
        }
        "fonts-license.txt" => {
            return text(
                include_str!("../../web/fonts/LICENSE"),
                "text/plain; charset=utf-8",
            );
        }
        _ => {}
    }
    Some(match name {
        "home.css" => (
            include_str!("../../web/home.css"),
            "text/css; charset=utf-8",
        ),
        "bar.css" => (include_str!("../../web/bar.css"), "text/css; charset=utf-8"),
        "webauthn.js" => (include_str!("../../web/webauthn.js"), js),
        "invite.js" => (include_str!("../web/invite.js"), js),
        "login.js" => (include_str!("../web/login.js"), js),
        "join.js" => (include_str!("../web/join.js"), js),
        "friends.js" => (include_str!("../../web/friends.js"), js),
        "chat.js" => (include_str!("../../chat/web/chat.js"), js),
        "chat.css" => (
            include_str!("../../chat/web/chat.css"),
            "text/css; charset=utf-8",
        ),
        "enrol.js" => (include_str!("../web/enrol.js"), js),
        "redeem.js" => (include_str!("../web/redeem.js"), js),
        "photos.js" => (include_str!("../../photos/web/photos.js"), js),
        "photos-passkey.js" => (include_str!("../../photos/web/photos-passkey.js"), js),
        "files.js" => (include_str!("../../files/web/files.js"), js),
        "media.js" => (include_str!("../../media/web/media.js"), js),
        "shelf.js" => (include_str!("../../media/web/shelf.js"), js),
        // TMDB's own logo, unaltered, for the credit their terms ask for
        "tmdb.svg" => (
            include_str!("../../media/web/icons/tmdb.svg"),
            "image/svg+xml",
        ),
        "library.js" => (include_str!("../../web/library.js"), js),
        "boxes.js" => (include_str!("../../fleet/web/boxes.js"), js),
        "backups.js" => (include_str!("../../fleet/web/backups.js"), js),
        "devices.js" => (include_str!("../../fleet/web/devices.js"), js),
        "network.js" => (include_str!("../../fleet/web/network.js"), js),
        "panel.js" => (include_str!("../../fleet/web/panel.js"), js),
        "shell.js" => (include_str!("../../web/shell.js"), js),
        _ => return None,
    })
    .map(|(s, ty)| (s.as_bytes(), ty))
}

/// What kind of account is looking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Member,
    /// the one account every visitor shares
    Demo,
    /// in through a friend link: invited game servers, and nothing else
    Guest,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Member => "member",
            Role::Demo => "demo",
            Role::Guest => "guest",
        }
    }
}

/// the mark on a tile: an svg fragment from web/icons/, by the role's name
fn icon(key: &str) -> &'static str {
    match key {
        "photos" => include_str!("../../web/icons/photos.svg"),
        "videos" => include_str!("../../web/icons/videos.svg"),
        "files" => include_str!("../../web/icons/files.svg"),
        "chat" => include_str!("../../web/icons/chat.svg"),
        "code" => include_str!("../../web/icons/code.svg"),
        "metrics" => include_str!("../../web/icons/metrics.svg"),
        "games" => include_str!("../../web/icons/games.svg"),
        _ => include_str!("../../web/icons/plain.svg"),
    }
}

pub(crate) fn initial(user: &str) -> String {
    user.chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_default()
}

/// A signed-in page: a static file that fills itself from /_dd/me
/// (web/pages/). The app carries the same files.
pub fn page(name: &str) -> Option<&'static str> {
    Some(match name {
        "home" => include_str!("../../web/pages/home.html"),
        "files" => include_str!("../../files/web/pages/files.html"),
        "media" => include_str!("../../media/web/pages/media.html"),
        "boxes" => include_str!("../../fleet/web/pages/boxes.html"),
        "backups" => include_str!("../../fleet/web/pages/backups.html"),
        "devices" => include_str!("../../fleet/web/pages/devices.html"),
        "network" => include_str!("../../fleet/web/pages/network.html"),
        "friends" => include_str!("../../web/pages/friends.html"),
        "chat" => include_str!("../../chat/web/pages/chat.html"),
        "friend" => include_str!("../../web/pages/friend.html"),
        _ => return None,
    })
}

/// Everything a signed-in page shows about who is looking: the name, the
/// bar's menu, and the services, with the demo's shut doors marked. The
/// pages are static files that fill themselves from this (web/shell.js),
/// the same files in a browser and in the app.
pub fn me_json(user: &str, services: &[Service], role: Role, domain: &str) -> serde_json::Value {
    let demo = user == DEMO_USER;
    let guest = role == Role::Guest;
    let menu = if guest {
        Menu::guest()
    } else {
        Menu::of(user, services)
    };
    // a guest is shown the one door the gate opens for them
    let games = format!("games.{domain}");
    let services: Vec<&Service> = services
        .iter()
        .filter(|s| !guest || host_of(&s.url) == games)
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
    /// A guest's: their own devices, the network their game servers are
    /// on, and the way out.
    pub(crate) fn guest() -> Menu {
        let item = |label, url: &str| MenuItem {
            label,
            url: url.to_string(),
        };
        Menu {
            groups: vec![
                vec![
                    item("Devices", "/_dd/devices"),
                    item("Network", "/_dd/network"),
                ],
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
        let metrics = services
            .iter()
            .find(|s| s.icon == "metrics")
            .map(|s| item("Metrics", &s.url));
        // The tile's url, where a tile says. A library page belongs on the
        // gate's own host: that is the only one serving /_dd/transcode, so
        // a relative link followed from another host plays nothing.
        let demo = user == DEMO_USER;
        let mut groups = Vec::new();
        if !demo {
            // Files and Movies & TV are tiles on the home page; repeating
            // them here would be the same door twice
            let mut fleet = vec![
                item("Backups", "/_dd/backups"),
                item("Devices", "/_dd/devices"),
                item("Network", "/_dd/network"),
                item("Boxes", "/_dd/boxes"),
            ];
            fleet.extend(metrics);
            groups.push(fleet);
        } else if let Some(m) = metrics {
            // the fleet's pages mean nothing to an account with no
            // devices, no backups and no boxes of its own
            groups.push(vec![m]);
        }
        groups.push(vec![item("Sign out", "/_dd/logout")]);
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
fn os_icon(key: &str) -> &'static str {
    match key {
        "linux" => include_str!("../web/icons/os/linux.svg"),
        "android" => include_str!("../web/icons/os/android.svg"),
        "apple" => include_str!("../web/icons/os/apple.svg"),
        _ => include_str!("../web/icons/os/windows.svg"),
    }
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
        source: format!("https://github.com/{from}/tree/{commit}"),
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
            assert_eq!(page.matches("<script").count(), 1);
            assert_eq!(page.matches("</header>").count(), 1);
            assert!(page.contains("/_dd/static/home.css"));
        }
        assert!(login("commonty.org").contains("dd enrol"));
        assert!(enrol().contains("dd enrol"));
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
        for f in ["login.js", "join.js", "enrol.js", "redeem.js", "photos.js"] {
            assert!(static_file(f).is_some(), "{f}");
        }
        assert!(text("login.js").contains("/_dd/login/start"));
        assert!(text("join.js").contains("/_dd/join/sign"));
        assert!(text("enrol.js").contains("/_dd/enrol/start"));
        assert!(text("redeem.js").contains("/_dd/redeem/start"));
        assert!(static_file("nope").is_none());
    }

    #[test]
    fn the_downloads_page_asks_for_nothing_and_points_at_the_release() {
        let at = |n: &str| AppFile {
            url: format!("https://github.com/x/y/releases/download/v0.2.0/{n}"),
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
            "x/y",
            "abc123",
            "v0.2.0",
            "https://m/app.json",
            &signed,
        );
        assert!(html.contains("https://github.com/x/y/releases/download/v0.2.0/commonty.apk"));
        // the source at the commit that was signed, and the manifest itself
        assert!(html.contains("https://github.com/x/y/tree/abc123"));
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
            "x/y",
            "abc123",
            "v0.2.0",
            "https://m/app.json",
            &signed,
        );
        assert!(!html.contains("commonty.apk"));
        assert_eq!(html.matches("class=\"os off\"").count(), 3);
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
        assert_eq!(
            menu_urls(&m),
            ["/_dd/devices", "/_dd/network", "/_dd/logout"]
        );
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
        for page in ["/_dd/backups", "/_dd/devices", "/_dd/network", "/_dd/boxes"] {
            assert!(
                menu.iter().any(|u| u == page),
                "member's menu is missing {page}"
            );
        }
        // these are services on the home page, so the menu must not repeat them
        for page in ["/_dd/files", "/_dd/media"] {
            assert!(!menu.iter().any(|u| u == page), "the menu repeats {page}");
        }
        assert!(menu.iter().any(|u| u == "https://metrics.example/"));
        assert!(menu.iter().any(|u| u == "/_dd/logout"));
        // the demo opens the library the box keeps for it, and nothing
        // that belongs to an account with devices and boxes of its own
        let menu = menu_urls(&me(DEMO_USER, &svcs));
        for page in ["/_dd/backups", "/_dd/devices", "/_dd/network", "/_dd/boxes"] {
            assert!(
                !menu.iter().any(|u| u == page),
                "the demo was offered {page}"
            );
        }
        assert!(menu.iter().any(|u| u == "https://metrics.example/"));
        // no metrics on this box: no line for it, and nothing else moves
        let menu = menu_urls(&me("tom", &[svc("Chat", "chat")]));
        assert!(!menu.iter().any(|u| u.contains("metrics")));
        assert!(menu.iter().any(|u| u == "/_dd/boxes"));
    }

    #[test]
    fn the_signed_in_pages_are_files_that_fill_themselves() {
        for (name, script) in [
            ("home", "shell.js"),
            ("files", "files.js"),
            ("media", "media.js"),
            ("boxes", "boxes.js"),
            ("backups", "backups.js"),
            ("devices", "devices.js"),
            ("network", "network.js"),
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
        // a film goes through the box, with the key sealed to it
        let lib = text("library.js");
        assert!(lib.contains("/_dd/transcode/session") && lib.contains("library_key_for_box"));
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
        assert!(html.contains("/_dd/static/redeem.js"));
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
