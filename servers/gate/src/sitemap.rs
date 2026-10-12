//! The site map: every place a member can open, for the assistant in the
//! corner of each page (servers/web-shared/guide.js) and the docs' drawing
//! of it. Made from what is already there - the home tiles, the Settings
//! page's own tabs, the Git pages' tabs, the docs' index - so a page added
//! is a place on the map with nothing more to write. Places only: nothing
//! about who is asking, nothing an admin alone may open.

use crate::pages::{Role, Service};
use serde_json::{Value, json};

/// The map for a member or the demo; `docs` is the docs' index (docs.rs)
pub fn build(services: &[Service], role: Role, domain: &str, docs: &Value) -> Value {
    let demo = role == Role::Demo;
    let home = format!("https://home.{domain}");
    let mut places = vec![place(
        "home",
        "Home",
        None,
        "page",
        &format!("{home}/_dd/home"),
        "home page start services tiles",
    )];
    let mut model = Value::Null;
    let mut report = Value::Null;
    let mut tiles = vec![];
    let mut views = vec![];
    // the tiles a member sees, the demo's own doors for the demo; the fleet's
    // figures (Metrics) are in the menu for the people who run boxes, not here
    for s in services.iter().filter(|s| !s.menu_only) {
        if demo && s.demo.is_none() {
            continue;
        }
        let url = match (demo, &s.demo_url) {
            (true, Some(u)) => u,
            _ => &s.url,
        };
        // a member's own keys open these, so their pages come from the app
        let app = !demo
            && matches!(
                s.icon.as_str(),
                "photos" | "passwords" | "files" | "videos" | "chat"
            );
        let tile_id = format!("svc-{}", slug(&s.name));
        let mut p = place(
            &tile_id,
            &s.name,
            Some("home"),
            if app { "app" } else { "page" },
            url,
            &format!("{} {}", s.name, s.blurb),
        );
        if s.icon == "code" {
            p["subs"] = git_tabs(!demo).into();
        }
        // what a page shows inside itself, with no address of its own
        if let Some((_, subs)) = SUBS.iter().find(|(i, _)| *i == s.icon) {
            p["subs"] = (*subs).into();
        }
        if let Some(o) = origin(url) {
            for (icon, id, label, parent, path, words, members) in VIEWS {
                if *icon == s.icon && (!demo || !members) {
                    let parent = if parent.is_empty() { &tile_id } else { *parent };
                    let mut v = place(
                        id,
                        label,
                        Some(parent),
                        "page",
                        &format!("{o}{path}"),
                        words,
                    );
                    if path.contains("{repo}") {
                        v["slot"] = "repo".into();
                    }
                    views.push(v);
                }
            }
            // a repository's settings, a pane each, from the page's own list
            if s.icon == "code" && !demo {
                for (k, label) in repo_settings_panes() {
                    let mut v = place(
                        &format!("code-repo-settings-{k}"),
                        &label,
                        Some("code-repo-settings"),
                        "page",
                        &format!("{o}/{{repo}}/settings/{k}"),
                        &format!("repository settings {label} git"),
                    );
                    v["slot"] = "repo".into();
                    views.push(v);
                }
            }
        }
        if s.icon == "chat" {
            model = origin(&s.url)
                .map(|o| format!("{o}/v1/chat/completions"))
                .into();
        }
        // where a member writes up a bug: the fleet's own repository, in
        // Code. The demo has no account to.
        if s.icon == "code"
            && !demo
            && let Some(repo) = docs["repo"].as_str().filter(|r| owner_name(r))
        {
            report = origin(&s.url)
                .map(|o| format!("{o}/{repo}/issues/new"))
                .into();
        }
        tiles.push(target(
            &format!("tile-{}", slug(&s.name)),
            &format!("Open {}", s.name),
            &format!("open {} tile", s.name),
            None,
        ));
        places.push(p);
    }
    places[0]["targets"] = tiles.into();
    places.push(place(
        "download",
        "Get the app",
        Some("home"),
        "page",
        &format!("{home}/_dd/download"),
        "get the app download install commonty app windows mac linux android",
    ));
    places.extend(views);
    if !demo {
        let friends = crate::pages::page("friends").unwrap_or_default();
        let mut p = place(
            "friends",
            "Friends",
            Some("home"),
            "page",
            &format!("{home}/_dd/friends"),
            "friends invite guests game link",
        );
        p["subs"] = headings_in(friends).into();
        places.push(p);
        places.push(place(
            "adblock",
            "Ad blocking at home",
            Some("home"),
            "page",
            &format!("{home}/_dd/adblock"),
            "ad blocking adblock ads pi-hole dns blocklists pause a site broke home network",
        ));
        // the demo has no account to set: Settings is a member's
        places.push(place(
            "settings",
            "Settings",
            Some("home"),
            "page",
            &format!("{home}/_dd/settings"),
            "settings account preferences options",
        ));
        let page = crate::pages::page("settings").unwrap_or_default();
        let mut tabs = vec![];
        for (tab, label, subs, words) in settings_tabs(page) {
            let mut p = place(
                &format!("settings-{tab}"),
                &label,
                Some("settings"),
                "page",
                &format!("{home}/_dd/settings#{tab}"),
                &words,
            );
            p["group"] = "Settings".into();
            p["subs"] = subs.into();
            places.push(p);
            if page.contains(&format!("data-guide=\"tab-{tab}\"")) {
                tabs.push(target(
                    &format!("tab-{tab}"),
                    &format!("{label} tab"),
                    &format!("{label} tab"),
                    None,
                ));
            }
        }
        if let Some(s) = places.iter_mut().find(|p| p["id"] == "settings") {
            s["targets"] = tabs.into();
        }
    }
    for (at, name, label, words, hint) in TARGETS {
        if let Some(p) = places.iter_mut().find(|p| p["id"] == *at) {
            let t = target(name, label, words, (!hint.is_empty()).then_some(*hint));
            match p["targets"].as_array_mut() {
                Some(ts) => ts.push(t),
                None => p["targets"] = json!([t]),
            }
        }
    }
    places.extend(docs_places(docs, domain));
    // the repository a `{repo}` place means when nothing names another:
    // the fleet's own
    let repo = docs["repo"].as_str().filter(|r| owner_name(r));
    json!({ "places": places, "model": model, "report": report, "repo": repo })
}

/// `owner/name`, as dd.repo is (VERIFY_REPO, in the docs' index): nothing
/// that could make the address anything but a repository's
fn owner_name(repo: &str) -> bool {
    let ok = |p: &str| {
        !p.is_empty()
            && !p.starts_with('.')
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(repo.split_once('/'), Some((o, n)) if ok(o) && ok(n))
}

/// Every view of a service the tiles do not list, under its tile ("") or
/// another view: the tile's icon, the place's id, its name, its parent, its path on
/// the tile's site, its words, and whether it is a member's alone. A
/// `{repo}` in the path is a repository's view, filled in by the assistant
/// (`owner/name`). Every route the Git pages answer is here: a test reads
/// them from servers/forge/web and fails on one that is not.
const VIEWS: &[(&str, &str, &str, &str, &str, &str, bool)] = &[
    (
        "games",
        "games-servers",
        "Your servers",
        "",
        "/servers",
        "your servers hosting invited saved worlds game server start",
        false,
    ),
    (
        "code",
        "code-yours",
        "Your repositories",
        "",
        "/",
        "your repositories repos list mine",
        true,
    ),
    (
        "code",
        "code-new",
        "A new repository",
        "",
        "/new",
        "new repository repo create make start",
        true,
    ),
    (
        "code",
        "code-notifications",
        "Notifications",
        "",
        "/notifications",
        "notifications unread mentions inbox bell",
        true,
    ),
    (
        "code",
        "code-search",
        "Search Git",
        "",
        "/search",
        "search find code repositories issues",
        false,
    ),
    (
        "code",
        "code-settings",
        "Your Git settings",
        "",
        "/settings",
        "git settings account",
        true,
    ),
    (
        "code",
        "code-settings-tokens",
        "API tokens",
        "code-settings",
        "/settings/tokens",
        "api tokens access token key",
        true,
    ),
    (
        "code",
        "code-settings-keys",
        "SSH & signing keys",
        "code-settings",
        "/settings/keys",
        "ssh signing keys gpg public key",
        true,
    ),
    (
        "code",
        "code-settings-profile",
        "Profile",
        "code-settings",
        "/settings/profile",
        "profile name full name",
        true,
    ),
    (
        "code",
        "code-repo",
        "A repository",
        "",
        "/{repo}",
        "repository repo code files readme",
        false,
    ),
    (
        "code",
        "code-repo-commits",
        "Commits",
        "code-repo",
        "/{repo}/commits",
        "commits history log changes",
        false,
    ),
    (
        "code",
        "code-repo-branches",
        "Branches",
        "code-repo",
        "/{repo}/branches",
        "branches branch",
        false,
    ),
    (
        "code",
        "code-repo-releases",
        "Releases",
        "code-repo",
        "/{repo}/releases",
        "releases tags versions downloads",
        false,
    ),
    (
        "code",
        "code-repo-issues",
        "Issues",
        "code-repo",
        "/{repo}/issues",
        "issues bugs tickets open closed",
        true,
    ),
    (
        "code",
        "code-repo-new-issue",
        "New issue",
        "code-repo-issues",
        "/{repo}/issues/new",
        "new issue open bug file report",
        true,
    ),
    (
        "code",
        "code-repo-pulls",
        "Pull requests",
        "code-repo",
        "/{repo}/pulls",
        "pull requests prs merge review",
        true,
    ),
    (
        "code",
        "code-repo-compare",
        "Compare branches",
        "code-repo-pulls",
        "/{repo}/compare",
        "compare branches diff new pull request",
        true,
    ),
    (
        "code",
        "code-repo-actions",
        "Actions",
        "code-repo",
        "/{repo}/actions",
        "actions ci runs workflows builds checks pipeline git",
        true,
    ),
    (
        "code",
        "code-repo-latest-run",
        "The latest run",
        "code-repo-actions",
        "/{repo}/actions/runs/latest",
        "latest newest last recent run ci build workflow job log git actions",
        true,
    ),
    (
        "code",
        "code-repo-activity",
        "Activity",
        "code-repo",
        "/{repo}/activity",
        "activity pulse charts contributors stats",
        true,
    ),
    (
        "code",
        "code-repo-settings",
        "Repository settings",
        "code-repo",
        "/{repo}/settings",
        "repository settings admin collaborators",
        true,
    ),
];

/// The views of a page that have no address of their own: its tabs and
/// panes, by the tile's icon, as the page names them
const SUBS: &[(&str, &[&str])] = &[
    (
        "files",
        &[
            "My files",
            "Trash",
            "List",
            "Grid",
            "Search this folder",
            "New folder",
            "Upload",
        ],
    ),
    (
        "videos",
        &[
            "All",
            "Films",
            "Shows",
            "Continue watching",
            "Recently added",
            "Add films",
            "New show",
        ],
    ),
    (
        "chat",
        &[
            "New chat",
            "Search chats",
            "Settings",
            "General",
            "About you",
            "Your chats",
        ],
    ),
];

/// The controls the assistant may point at, under the place they are on: a
/// `data-guide` name our own pages give a button, what it is called there,
/// its words, and where on the place it is when it is not on its first
/// page. Only ours, never anything a member wrote. Settings' tabs and the
/// home tiles are added from the pages themselves.
const TARGETS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "home",
        "menu",
        "Your menu",
        "menu account name sign out settings",
        "",
    ),
    ("home", "search", "Search", "search find docs", ""),
    (
        "settings-devices",
        "add-device",
        "Add a device",
        "add new device phone computer code scan",
        "",
    ),
    (
        "settings-devices",
        "show-names",
        "Show names",
        "show device names",
        "",
    ),
    (
        "settings-devices",
        "sign-out-others",
        "Sign out everywhere else",
        "sign out everywhere else other",
        "",
    ),
    (
        "settings-network",
        "change-wifi",
        "Change Wi-Fi details",
        "change wifi wi-fi password network",
        "",
    ),
    (
        "settings-backups",
        "open-backups",
        "Open them in the app",
        "open backups disk images app",
        "",
    ),
    (
        "settings-backups",
        "get-app",
        "Get the app",
        "get download install app",
        "",
    ),
    (
        "settings-this-device",
        "show-assistant",
        "Show the assistant",
        "show turn off on hide assistant",
        "",
    ),
    (
        "settings-this-device",
        "shortcut",
        "Keyboard shortcut for the assistant",
        "keyboard shortcut key ctrl",
        "",
    ),
    (
        "settings-leave",
        "leave",
        "Leave and delete everything",
        "leave delete account quit",
        "",
    ),
    ("svc-files", "upload", "Upload", "upload add files", ""),
    (
        "svc-files",
        "new-folder",
        "New folder",
        "new folder make create",
        "",
    ),
    (
        "svc-code",
        "new-repo",
        "New repository",
        "new repository repo create make",
        "",
    ),
    (
        "svc-code",
        "new-issue",
        "New issue",
        "new issue open bug",
        "in a repository, on its Issues tab",
    ),
];

fn target(name: &str, label: &str, words: &str, hint: Option<&str>) -> Value {
    let mut t = json!({ "name": name, "label": label, "words": words.to_lowercase() });
    if let Some(h) = hint {
        t["where"] = h.into();
    }
    t
}

fn place(id: &str, label: &str, parent: Option<&str>, kind: &str, url: &str, words: &str) -> Value {
    let mut p = json!({ "id": id, "label": label, "kind": kind, "url": url, "words": words.to_lowercase() });
    if let Some(parent) = parent {
        p["parent"] = parent.into();
    }
    p
}

/// `https://llm.x` of `https://llm.x/`
fn origin(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    (u.scheme() == "https").then(|| u.origin().ascii_serialization())
}

/// A heading's anchor, as shell.js makes it
pub(crate) fn slug(text: &str) -> String {
    let kept: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Markup's words, without the markup
fn text_of(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' => {
                tag = false;
                out.push(' ');
            }
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// what each `<h2>` in some markup says
fn headings_in(html: &str) -> Vec<String> {
    html.split("<h2")
        .skip(1)
        .filter_map(|h| {
            let inner = &h[h.find('>')? + 1..];
            Some(text_of(&inner[..inner.find("</h2>")?]))
        })
        .filter(|h| !h.is_empty())
        .collect()
}

/// what the words of a tab say of it, short
const TAB_WORDS: usize = 600;

/// Settings' tabs from the page itself: each `data-tab` link in its nav,
/// its name, the headings of its pane and the pane's words
fn settings_tabs(page: &str) -> Vec<(String, String, Vec<String>, String)> {
    let Some(nav) = page
        .split("<nav class=\"se-nav\"")
        .nth(1)
        .and_then(|n| n.split("</nav>").next())
    else {
        return vec![];
    };
    nav.split("<a ")
        .skip(1)
        .filter_map(|a| {
            let tab = a.split("data-tab=\"").nth(1)?.split('"').next()?;
            let label = text_of(&a[a.find('>')? + 1..a.find("</a>")?]);
            let pane = page
                .split(&format!("data-pane=\"{tab}\""))
                .nth(1)
                .map(|p| p.split("<div class=\"se-pane").next().unwrap_or(p))
                .map(|p| p.split("</main>").next().unwrap_or(p))
                .unwrap_or_default();
            let mut words = format!("{label} {}", text_of(&format!("<{pane}")));
            let mut end = words.len().min(TAB_WORDS);
            while !words.is_char_boundary(end) {
                end -= 1;
            }
            words.truncate(end);
            Some((tab.to_string(), label, headings_in(pane), words))
        })
        .collect()
}

/// A repository's tabs, from the Git pages' own header (git-repo.js): the
/// words a member sees there. Not places of their own - each is a
/// repository's - and Settings is only a repository admin's.
fn git_tabs(member: bool) -> Vec<String> {
    let js = crate::pages::static_file("git-repo.js")
        .and_then(|(b, _, _)| std::str::from_utf8(b).ok())
        .unwrap_or_default();
    let Some(nav) = js.split("class: 'rtabs'").nth(1) else {
        return vec![];
    };
    let nav = nav.split("))))").next().unwrap_or(nav);
    nav.split(" t('")
        .skip(1)
        .filter_map(|call| {
            let call = call.split(')').next()?;
            let quoted: Vec<&str> = call.split('\'').collect();
            // key', `href`, 'icon', 'Label'
            let key = quoted.first()?;
            let label = quoted.get(4)?;
            let only_members = call.contains("!open") || *key == "activity";
            (*key != "settings" && (member || !only_members)).then(|| label.to_string())
        })
        .collect()
}

/// A repository's settings panes, from the Git pages' own list
/// (git-settings.js PANES): the key in the address and what the pane is called
fn repo_settings_panes() -> Vec<(String, String)> {
    let js = crate::pages::static_file("git-settings.js")
        .and_then(|(b, _, _)| std::str::from_utf8(b).ok())
        .unwrap_or_default();
    let Some(list) = js
        .split("const PANES = [")
        .nth(1)
        .and_then(|l| l.split("];").next())
    else {
        return vec![];
    };
    list.split('[')
        .skip(1)
        .filter_map(|pair| {
            let q: Vec<&str> = pair.split('\'').collect();
            let (k, t) = (q.get(1)?, q.get(3)?);
            k.chars()
                .all(|c| c.is_ascii_lowercase())
                .then(|| (k.to_string(), t.to_string()))
        })
        .collect()
}

/// Every docs page under the docs' own groups, with its `##` headings
fn docs_places(index: &Value, domain: &str) -> Vec<Value> {
    let docs = format!("https://{domain}/docs");
    let mut out = vec![place(
        "docs",
        "Docs",
        None,
        "docs",
        &docs,
        "docs documentation help how",
    )];
    let pages = &index["pages"];
    for g in index["groups"].as_array().into_iter().flatten() {
        let name = g["name"].as_str().unwrap_or_default();
        let parent = if name.is_empty() {
            "docs".to_string()
        } else {
            let id = format!("docs-{}", slug(name));
            let mut p = place(&id, name, Some("docs"), "docs", &docs, name);
            p["group"] = "Docs".into();
            out.push(p);
            id
        };
        for file in g["pages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let page = &pages[file];
            let title = page["title"].as_str().unwrap_or(file);
            let at = file
                .trim_end_matches(".md")
                .trim_end_matches("/README")
                .to_string();
            let heads: Vec<&str> = page["headings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|h| h[0] == 2)
                .filter_map(|h| h[1].as_str())
                .collect();
            let mut p = place(
                &format!("doc:{at}"),
                title,
                Some(&parent),
                "docs",
                &format!("{docs}/{at}"),
                &format!("{title} {}", heads.join(" ")),
            );
            if !name.is_empty() {
                p["group"] = name.into();
            }
            p["doc"] = file.into();
            p["subs"] = heads.into();
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(name: &str, icon: &str, url: &str) -> Service {
        Service {
            name: name.into(),
            url: url.into(),
            icon: icon.into(),
            color: String::new(),
            blurb: format!("about {name}"),
            demo: None,
            demo_url: None,
            menu_only: false,
        }
    }

    fn tiles() -> Vec<Service> {
        let mut photos = tile("Photos", "photos", "https://photos.x/_dd/photos");
        photos.demo = Some("read".into());
        photos.demo_url = Some("https://home.x/_dd/photos/demo".into());
        let mut chat = tile("Chat", "chat", "https://llm.x/");
        chat.demo = Some("rate:10/60".into());
        let mut metrics = tile("Metrics", "metrics", "https://metrics.x/");
        metrics.menu_only = true;
        vec![
            photos,
            tile("Passwords", "passwords", "https://vault.x/"),
            chat,
            tile("Code", "code", "https://git.x/"),
            metrics,
        ]
    }

    fn docs() -> Value {
        json!({
            "repo": "fleet/commonty",
            "home": "README.md",
            "groups": [{ "name": "Guide", "pages": ["guide/leaving.md"] }],
            "pages": {
                "README.md": { "title": "Docs", "headings": [], "text": "" },
                "guide/leaving.md": {
                    "title": "Leaving",
                    "headings": [[2, "Take your things"], [3, "Photos"], [2, "What goes"]],
                    "text": "When you leave",
                },
            },
        })
    }

    fn ids(m: &Value) -> Vec<String> {
        m["places"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_string())
            .collect()
    }

    fn by<'a>(m: &'a Value, id: &str) -> &'a Value {
        m["places"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
    }

    /// Every route the Git pages answer is on the map: a place of its own,
    /// or one thing a place lists (an issue of Issues, a run of Actions). A
    /// route added to servers/forge/web fails here until it is listed.
    #[test]
    fn every_git_route_is_on_the_map() {
        const ROUTES: &[(&str, &str)] = &[
            (r"/^\/$/", "code-yours"),
            (r"/^\/new$/", "code-new"),
            (r"/^\/notifications$/", "code-notifications"),
            (r"/^\/search$/", "code-search"),
            (
                r"/^\/settings(?:\/(tokens|keys|profile))?$/",
                "code-settings",
            ),
            (r"/^\/([^/]+)$/", "code-yours"),
            (r"/^\/([^/]+)\/([^/]+)\/pulls$/", "code-repo-pulls"),
            (r"/^\/([^/]+)\/([^/]+)\/issues$/", "code-repo-issues"),
            (
                r"/^\/([^/]+)\/([^/]+)\/issues\/new$/",
                "code-repo-new-issue",
            ),
            (r"/^\/([^/]+)\/([^/]+)\/issues\/(\d+)$/", "code-repo-issues"),
            (
                r"/^\/([^/]+)\/([^/]+)\/pulls?\/(\d+)(?:\/(commits|checks|files))?$/",
                "code-repo-pulls",
            ),
            (
                r"/^\/([^/]+)\/([^/]+)\/compare(?:\/(.+?)\.\.\.(.+))?$/",
                "code-repo-compare",
            ),
            (r"/^\/([^/]+)\/([^/]+)(?:\/tree\/(.+))?$/", "code-repo"),
            (r"/^\/([^/]+)\/([^/]+)\/blob\/(.+)$/", "code-repo"),
            (
                r"/^\/([^/]+)\/([^/]+)\/commits(?:\/(.+))?$/",
                "code-repo-commits",
            ),
            (
                r"/^\/([^/]+)\/([^/]+)\/commit\/([0-9a-f]{4,64})$/",
                "code-repo-commits",
            ),
            (r"/^\/([^/]+)\/([^/]+)\/branches$/", "code-repo-branches"),
            (
                r"/^\/([^/]+)\/([^/]+)\/releases(?:\/tag\/(.+))?$/",
                "code-repo-releases",
            ),
            (r"/^\/([^/]+)\/([^/]+)\/activity$/", "code-repo-activity"),
            (
                r"/^\/([^/]+)\/([^/]+)\/src\/(?:branch|tag|commit)\/(.+)$/",
                "code-repo",
            ),
            (
                r"/^\/([^/]+)\/([^/]+)\/(?:commits\/branch)\/(.+)$/",
                "code-repo-commits",
            ),
            (
                r"/^\/([^/]+)\/([^/]+)\/settings(?:\/([a-z]+))?$/",
                "code-repo-settings",
            ),
            (
                r"/^\/([^/]+)\/([^/]+)\/actions\/runs\/latest$/",
                "code-repo-latest-run",
            ),
            (r"/^\/([^/]+)\/([^/]+)\/actions$/", "code-repo-actions"),
            (
                r"/^\/([^/]+)\/([^/]+)\/actions\/runs\/(\d+)(?:\/jobs\/(\d+))?$/",
                "code-repo-actions",
            ),
        ];
        let m = build(&tiles(), Role::Member, "x", &docs());
        let ids = ids(&m);
        for (_, id) in ROUTES {
            assert!(ids.iter().any(|i| i == id), "{id}");
        }
        for f in [
            "git-home.js",
            "git-repo.js",
            "git-pulls.js",
            "git-actions.js",
            "git-settings.js",
        ] {
            let js = std::str::from_utf8(crate::pages::static_file(f).unwrap().0).unwrap();
            for line in js.lines().filter(|l| l.starts_with("route(/")) {
                let src = format!("{}$/", line["route(".len()..].split("$/,").next().unwrap());
                assert!(
                    ROUTES.iter().any(|(r, _)| *r == src),
                    "{f}: {src} is not on the map (VIEWS, and this test's ROUTES)"
                );
            }
        }
        // a repository's views are filled in, the fleet's own when none is named
        assert_eq!(m["repo"], "fleet/commonty");
        let latest = by(&m, "code-repo-latest-run");
        assert_eq!(latest["url"], "https://git.x/{repo}/actions/runs/latest");
        assert_eq!(latest["slot"], "repo");
        assert!(
            ids.iter().any(|i| i == "code-repo-settings-careful"),
            "{ids:?}"
        );
        // the demo has no account: none of a member's own views
        let d = build(&tiles(), Role::Demo, "x", &docs());
        let dids = super::tests::ids(&d);
        assert!(
            !dids
                .iter()
                .any(|i| i == "code-settings" || i == "code-repo-actions"),
            "{dids:?}"
        );
    }

    #[test]
    fn a_member_gets_the_tiles_settings_and_the_docs_with_their_headings() {
        let m = build(&tiles(), Role::Member, "x", &docs());
        let ids = ids(&m);
        for id in [
            "home",
            "svc-photos",
            "svc-passwords",
            "svc-chat",
            "svc-code",
            "friends",
            "settings",
            "docs",
            "docs-guide",
            "doc:guide/leaving",
        ] {
            assert!(ids.contains(&id.to_string()), "{id} in {ids:?}");
        }
        // the Settings page's own tabs, from its file
        for tab in [
            "profile", "devices", "storage", "network", "backups", "leave",
        ] {
            let p = by(&m, &format!("settings-{tab}"));
            assert_eq!(p["parent"], "settings");
            assert_eq!(p["url"], format!("https://home.x/_dd/settings#{tab}"));
        }
        assert_eq!(by(&m, "settings-devices")["label"], "Devices");
        assert!(
            by(&m, "settings-devices")["subs"]
                .as_array()
                .unwrap()
                .contains(&json!("Signed in now"))
        );
        assert!(
            by(&m, "settings-devices")["words"]
                .as_str()
                .unwrap()
                .contains("add a device")
        );
        // the docs: the page, its address, and its ## headings, not deeper ones
        let leaving = by(&m, "doc:guide/leaving");
        assert_eq!(leaving["url"], "https://x/docs/guide/leaving");
        assert_eq!(leaving["subs"], json!(["Take your things", "What goes"]));
        assert_eq!(leaving["doc"], "guide/leaving.md");
        assert_eq!(leaving["parent"], "docs-guide");
        // what opens through the app, so it goes through the hand-off
        assert_eq!(by(&m, "svc-photos")["kind"], "app");
        assert_eq!(by(&m, "svc-passwords")["kind"], "app");
        assert_eq!(by(&m, "svc-code")["kind"], "page");
        // Git's tabs as words under it, never a repository admin's settings
        let git: Vec<&str> = by(&m, "svc-code")["subs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            git.contains(&"Pull requests") && git.contains(&"Actions"),
            "{git:?}"
        );
        assert!(!git.contains(&"Settings"), "{git:?}");
        // the model Chat uses, where Chat is
        assert_eq!(m["model"], "https://llm.x/v1/chat/completions");
    }

    #[test]
    fn nothing_for_admins_and_nothing_of_anyone() {
        for role in [Role::Member, Role::Demo] {
            let m = build(&tiles(), role, "x", &docs());
            let all = m.to_string();
            assert!(!ids(&m).iter().any(|i| i.contains("metrics")), "{role:?}");
            for not in [
                "metrics.x",
                "/_dd/fleet",
                "/_dd/directory",
                "/verify",
                "/internal",
                "/_dd/app/",
                "/_dd/sso",
                "/_dd/logout",
            ] {
                assert!(!all.contains(not), "{role:?}: {not}");
            }
            // every place is on the fleet's own names
            for p in m["places"].as_array().unwrap() {
                let u = url::Url::parse(p["url"].as_str().unwrap()).unwrap();
                assert_eq!(u.scheme(), "https");
                let h = u.host_str().unwrap();
                assert!(h == "x" || h.ends_with(".x"), "{h}");
            }
        }
    }

    #[test]
    fn the_demo_gets_its_own_doors_and_no_settings() {
        let m = build(&tiles(), Role::Demo, "x", &docs());
        let ids = ids(&m);
        assert!(
            !ids.iter()
                .any(|i| i.starts_with("settings") || i == "friends"),
            "{ids:?}"
        );
        // shut to the demo: not on its map
        assert!(!ids.contains(&"svc-passwords".to_string()));
        assert!(!ids.contains(&"svc-code".to_string()));
        assert_eq!(
            by(&m, "svc-photos")["url"],
            "https://home.x/_dd/photos/demo"
        );
        assert_eq!(by(&m, "svc-photos")["kind"], "page");
        assert!(ids.contains(&"doc:guide/leaving".to_string()));
        assert_eq!(m["model"], "https://llm.x/v1/chat/completions");
    }

    /// Whether some file of ours marks a control with this name, as markup
    /// or as kit.js's el() writes it
    fn marks(src: &str, name: &str) -> bool {
        src.contains(&format!("data-guide=\"{name}\""))
            || src.contains(&format!("'data-guide': '{name}'"))
    }

    fn ours(files: &[&str]) -> String {
        files
            .iter()
            .map(|f| {
                crate::pages::page(f)
                    .map(str::to_string)
                    .or_else(|| {
                        crate::pages::static_file(f)
                            .map(|(b, _, _)| String::from_utf8_lossy(b).into_owned())
                    })
                    .unwrap_or_else(|| panic!("no file {f}"))
            })
            .collect()
    }

    #[test]
    fn every_control_on_the_map_is_one_our_pages_mark() {
        let mut tiles = tiles();
        tiles.push(tile("Files", "files", "https://files.x/_dd/files"));
        let m = build(&tiles, Role::Member, "x", &docs());
        let mut seen = 0;
        for p in m["places"].as_array().unwrap() {
            let id = p["id"].as_str().unwrap();
            let files: &[&str] = match id {
                "home" | "settings" => &["shell.js", "settings"],
                "svc-files" => &["files"],
                "svc-code" => &["git-home.js", "git-pulls.js"],
                _ if id.starts_with("settings-") => &["settings", "settings.js", "backups.js"],
                _ => &[],
            };
            let src = ours(files);
            for t in p["targets"].as_array().into_iter().flatten() {
                let name = t["name"].as_str().unwrap();
                seen += 1;
                assert!(
                    name.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                    "{name}"
                );
                if name.starts_with("tile-") {
                    assert!(src.contains("'data-guide': `tile-${slug(s.name)}`"));
                    continue;
                }
                assert!(
                    marks(&src, name),
                    "{id}.{name} is marked in none of {files:?}"
                );
            }
        }
        // the ones asked for, each under its place
        let has = |at: &str, name: &str| {
            by(&m, at)["targets"]
                .as_array()
                .is_some_and(|ts| ts.iter().any(|t| t["name"] == name))
        };
        for (at, name) in [
            ("home", "menu"),
            ("home", "tile-photos"),
            ("home", "tile-passwords"),
            ("settings", "tab-devices"),
            ("settings", "tab-this-device"),
            ("settings-devices", "add-device"),
            ("settings-devices", "show-names"),
            ("settings-backups", "open-backups"),
            ("settings-backups", "get-app"),
            ("settings-leave", "leave"),
            ("settings-this-device", "shortcut"),
            ("svc-files", "upload"),
            ("svc-code", "new-repo"),
            ("svc-code", "new-issue"),
        ] {
            assert!(has(at, name), "{at}.{name}");
        }
        assert!(seen > 20, "{seen}");
        assert_eq!(m["report"], "https://git.x/fleet/commonty/issues/new");
        // the demo writes up nothing, and has none of Settings' controls
        let d = build(&tiles, Role::Demo, "x", &docs());
        assert!(d["report"].is_null());
        assert!(!d.to_string().contains("add-device"));
    }

    /// Bugs go to the fleet's own repository (dd.repo), never a name
    /// written here, and only one shaped like owner/name
    #[test]
    fn bug_reports_go_to_the_fleets_repository() {
        let report = |repo: Value| {
            let mut d = docs();
            d["repo"] = repo;
            build(&tiles(), Role::Member, "x", &d)["report"].clone()
        };
        assert_eq!(
            report(json!("someone/their-fork")),
            "https://git.x/someone/their-fork/issues/new"
        );
        for not in [
            json!(null),
            json!(""),
            json!("nameonly"),
            json!("a/b/c"),
            json!("../x"),
            json!("a/b?x=1"),
            json!("a/b#x"),
            json!("evil.example/@x"),
            json!(7),
        ] {
            assert!(report(not.clone()).is_null(), "{not}");
        }
        let src = include_str!("sitemap.rs");
        assert!(!src.contains(&["david", "commonty"].join("/")));
    }

    #[test]
    fn slugs_are_the_pages_own() {
        assert_eq!(slug("Movies & TV"), "movies-tv");
        assert_eq!(slug("  Your recovery key "), "your-recovery-key");
        assert_eq!(slug("Wi-Fi"), "wi-fi");
    }
}
