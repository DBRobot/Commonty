//! The Games pages: what each template gets. The HTML is in templates/,
//! the look in web/games.css, the little script in web/games.js.

use askama::Template;

use crate::{Game, Instance, Manager, Setting, State, World};

pub fn state_name(st: State) -> &'static str {
    match st {
        State::Stopped => "stopped",
        State::Starting => "starting",
        State::Updating => "updating",
        State::Running => "running",
        State::Failed => "failed",
    }
}

/// a server as the pages show it
pub struct ServerView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub game: Option<&'a Game>,
    pub state: &'static str,
    pub label: &'static str,
    pub who: String,
    pub address: Option<String>,
}

impl<'a> ServerView<'a> {
    fn new(m: &'a Manager, i: &'a Instance, user: &str) -> Self {
        let st = m.state(i);
        let game = m.cfg.catalogue.get(&i.game);
        let port = i
            .ports
            .iter()
            .find(|p| p.var == "SERVER_PORT")
            .or(i.ports.first());
        Self {
            id: &i.id,
            name: game.map(|g| g.name.as_str()).unwrap_or(&i.game),
            game,
            state: state_name(st),
            label: st.label(),
            who: if i.owner == user {
                "yours".into()
            } else {
                format!("{}'s", i.owner)
            },
            address: port.map(|p| format!("{}:{}", m.address(), p.port)),
        }
    }
}

/// a game opened over the library
pub struct OpenView<'a> {
    pub game: &'a Game,
    pub back: String,
    pub ours: Vec<&'a Setting>,
    pub more: Vec<&'a Setting>,
    pub memory_gb: u64,
    pub cores: u32,
}

#[derive(Template)]
#[template(path = "library.html")]
pub struct Library<'a> {
    pub user: &'a str,
    /// the demo may look, not start
    pub demo: bool,
    /// in through a friend link: plays where invited, hosts nothing
    pub guest: bool,
    pub home: &'a str,
    pub q: &'a str,
    pub count: usize,
    pub notice: Option<&'a str>,
    pub mine: Vec<ServerView<'a>>,
    pub others: Vec<ServerView<'a>>,
    pub games: Vec<&'a Game>,
    pub open: Option<OpenView<'a>>,
    /// this person's kept worlds, newest first
    pub worlds: Vec<WorldView>,
}

pub struct WorldView {
    pub name: String,
    pub game_name: String,
    pub cover: Option<String>,
    pub when: String,
    pub files: usize,
}

/// The library, with one game open over it when `open` says so.
pub fn library(
    m: &Manager,
    user: &str,
    guest: bool,
    q: &str,
    open: Option<&str>,
    notice: Option<&str>,
) -> String {
    let all = m.instances();
    let ql = q.trim().to_lowercase();
    // yours, and the ones you are invited to; nobody else's is shown
    let mine: Vec<&Instance> = all.iter().filter(|i| i.owner == user).collect();
    let others: Vec<&Instance> = all
        .iter()
        .filter(|i| i.owner != user && m.may_see(user, i))
        .collect();
    let open = open.and_then(|id| m.cfg.catalogue.get(id)).map(|g| {
        let (ours, more) = g.visible_settings().partition(|s| s.ours.is_some());
        OpenView {
            game: g,
            back: if q.is_empty() {
                "/".to_string()
            } else {
                format!("/?q={q}")
            },
            ours,
            more,
            memory_gb: g.memory() / 1024,
            cores: g.cores(),
        }
    });
    Library {
        user,
        demo: user == "demo",
        guest,
        home: &m.cfg.home,
        q,
        count: m.cfg.catalogue.len(),
        notice,
        mine: mine.iter().map(|i| ServerView::new(m, i, user)).collect(),
        others: others.iter().map(|i| ServerView::new(m, i, user)).collect(),
        games: m
            .cfg
            .catalogue
            .values()
            .filter(|g| ql.is_empty() || g.name.to_lowercase().contains(&ql))
            .collect(),
        open,
        worlds: m
            .worlds(user)
            .into_iter()
            .map(|w: World| {
                let g = m.cfg.catalogue.get(&w.game);
                WorldView {
                    name: w.name.clone(),
                    game_name: g.map(|g| g.name.clone()).unwrap_or(w.game.clone()),
                    cover: g.filter(|g| g.cover).map(|g| g.id.clone()),
                    when: ago(w.kept),
                    files: w.files,
                }
            })
            .collect(),
    }
    .render()
    .unwrap_or_default()
}

fn ago(t: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(t);
    let d = now.saturating_sub(t);
    if d < 3600 {
        format!("{} min ago", d / 60)
    } else if d < 86400 {
        format!("{} h ago", d / 3600)
    } else {
        format!("{} days ago", d / 86400)
    }
}

pub struct PortView {
    pub port: u16,
    pub label: String,
}

#[derive(Template)]
#[template(path = "server.html")]
pub struct Server<'a> {
    pub user: &'a str,
    pub demo: bool,
    /// the settings a person may change, with this server's values
    pub fields: Vec<Setting>,
    pub home: &'a str,
    pub s: ServerView<'a>,
    pub address: String,
    /// the port a player types: SERVER_PORT, else the first
    pub main_port: Option<PortView>,
    pub other_ports: Vec<PortView>,
    pub settings: String,
    pub mine: bool,
    pub idle: bool,
    pub busy: bool,
    pub log: String,
    pub owner: &'a str,
    /// who plays here besides the owner
    pub players: Vec<String>,
    /// the owner's friends, each ticked if invited: the invite form
    pub friends: Vec<(String, bool)>,
}

/// One server: what it is, where it is, what it says, and its controls.
/// `friends` is the owner's, from the gate, when the owner is looking.
pub fn server(m: &Manager, user: &str, i: &Instance, friends: &[String]) -> String {
    let players = m.players(&i.id);
    let st = m.state(i);
    let g = m.cfg.catalogue.get(&i.game);
    let settings = g
        .map(|g| {
            g.visible_settings()
                .filter(|s| s.ours.is_some())
                .filter_map(|s| i.env.get(&s.var).map(|v| format!("{}: {v}", s.label)))
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default();
    let fields = g
        .map(|g| {
            g.visible_settings()
                .map(|s| {
                    let mut f = s.clone();
                    if let Some(v) = i.env.get(&s.var) {
                        f.default = v.clone();
                    }
                    f
                })
                .collect()
        })
        .unwrap_or_default();
    Server {
        user,
        demo: user == "demo",
        fields,
        home: &m.cfg.home,
        s: ServerView::new(m, i, user),
        address: m.address(),
        main_port: i
            .ports
            .iter()
            .find(|p| p.var == "SERVER_PORT")
            .or(i.ports.first())
            .map(|p| PortView {
                port: p.port,
                label: String::new(),
            }),
        other_ports: {
            let main = i
                .ports
                .iter()
                .find(|p| p.var == "SERVER_PORT")
                .or(i.ports.first())
                .map(|p| p.var.clone());
            i.ports
                .iter()
                .filter(|p| Some(&p.var) != main.as_ref())
                .map(|p| PortView {
                    port: p.port,
                    label: p.var.replace("_PORT", "").replace('_', " ").to_lowercase(),
                })
                .collect()
        },
        settings,
        mine: i.owner == user,
        idle: matches!(st, State::Stopped | State::Failed),
        busy: matches!(st, State::Starting | State::Updating),
        // what a game server prints is the owner's to read
        log: if i.owner == user {
            m.log_tail(&i.id, 80)
        } else {
            String::new()
        },
        owner: &i.owner,
        friends: friends
            .iter()
            .map(|f| (f.clone(), players.contains(f)))
            .collect(),
        players,
    }
    .render()
    .unwrap_or_default()
}
