//! A member's own boxes (docs/dev/joining.md). Whose a joined box is,
//! is the agreed identity state's word: each member's `boxes`, the boxes
//! the judge has them enlisted with (state/ident). No gate judges a join
//! ticket for it, so every box agrees whose it is, and none vouches alone.
//!
//! What being a member's gives the box is the member's: it is on their
//! Devices tab, they see how it is, and the backup made of its old disk
//! is kept for them. It runs nothing a stranger does not.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use biscuit_auth::UnverifiedBiscuit;

use crate::App;

/// how long a pending backup waits for its box to join, from its first
/// write: as long as the ticket it was written with is good for
pub(crate) const PENDING_SECS: u64 = 7 * 24 * 3600;

/// A joined box that is a member's: the agreed identity state has them
/// enlisted with it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Owned {
    pub name: String,
    pub user: String,
    /// the id of the ticket it joined with: its pending backup's folder
    pub ticket: String,
    /// when it joined, agreed ms
    pub at: i64,
    /// its key, as it joined with it: the name is this key's (JoinName)
    pub key: String,
}

/// A join ticket, as a pending backup's upload brings it: signed by a
/// device in the member's entry, minted for `join`, and good at `at`
/// (seconds). The member and the ticket's id. It says who may write that
/// one backup, never whose a box is.
// TODO(enlist): an upload credential the enlistment gives, in place of
// the ticket
pub(crate) fn verify_ticket(
    entry_of: impl Fn(&str) -> anyhow::Result<Option<identity::SignedEntry>>,
    token: &str,
    at: u64,
) -> anyhow::Result<(String, String)> {
    let (user, _) = crate::verify_device_token_at(entry_of, token, "join", "", at)?;
    // the authority block alone: what the member's device signed, never a
    // block a holder added after
    let id = ticket_of(token).ok_or_else(|| anyhow::anyhow!("not one ticket fact"))?;
    Ok((user, id))
}

/// The ticket id a token's authority block names, unchecked: a name for a
/// pending folder, nothing more.
fn ticket_of(token: &str) -> Option<String> {
    let source = UnverifiedBiscuit::from_base64(token)
        .ok()?
        .print_block_source(0)
        .ok()?;
    ticket_id(&source)
}

/// The one `ticket("<32 lowercase hex>");` fact of an authority block's
/// source, a line of its own, as auth::device::mint_join_ticket writes it:
/// none, two, or any other shape is no ticket. The same 32 characters are
/// the pending folder's name and what the purge compares, byte for byte.
fn ticket_id(source: &str) -> Option<String> {
    let mut ids = source
        .lines()
        .map(str::trim)
        .filter(|l| l.contains("ticket"))
        .map(|l| {
            l.strip_prefix("ticket(\"")
                .and_then(|r| r.strip_suffix("\");"))
                .filter(|id| {
                    id.len() == 32 && id.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
                })
        });
    match (ids.next(), ids.next()) {
        (Some(Some(id)), None) => Some(id.to_string()),
        _ => None,
    }
}

/// Whose each joined box is: `enlisted` is box -> member, from the agreed
/// identity state's members' `boxes`. A joined box no member has enlisted
/// is nobody's; an enlisted box not in the control state's `joined` is
/// not here yet.
pub(crate) fn owners(
    joined: &serde_json::Value,
    enlisted: &BTreeMap<String, String>,
) -> Vec<Owned> {
    let mut out: Vec<Owned> = joined
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, j)| {
            let user = enlisted.get(name)?;
            Some(Owned {
                name: name.clone(),
                user: user.clone(),
                // TODO(enlist): the ticket as enlisted (Enlisted.Ticket),
                // once the identity view carries it
                ticket: j["ticket"].as_str().and_then(ticket_of).unwrap_or_default(),
                at: j["at"].as_i64().unwrap_or(0),
                key: j["key"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| (a.at, &a.name).cmp(&(b.at, &b.name)));
    out
}

/// box -> member, from the agreed identity state; empty when this box does
/// not read it (identityFrom replay) or has not read it yet
fn enlisted(app: &App) -> BTreeMap<String, String> {
    let Some(st) = app.directory.ident.as_ref().and_then(|a| a.state()) else {
        return BTreeMap::new();
    };
    st.members
        .iter()
        .flat_map(|(name, m)| m.boxes.iter().map(move |b| (b.clone(), name.clone())))
        .collect()
}

/// The agreed state, as last read from this box's dd-control: read again
/// after ten seconds, and good for no more than five minutes when it can't
/// be (then no joined box is anyone's, and nothing pending goes).
#[derive(Default)]
pub(crate) struct Cache {
    read: Option<Instant>,
    asked: Option<Instant>,
    state: serde_json::Value,
}

const STATE_FRESH: Duration = Duration::from_secs(10);
const STATE_STALE: Duration = Duration::from_secs(300);

async fn state(app: &Arc<App>) -> serde_json::Value {
    let ask = {
        let c = app.member_boxes.lock().unwrap();
        c.asked.is_none_or(|t| t.elapsed() >= STATE_FRESH)
    };
    if ask && let Some(control) = &app.control {
        app.member_boxes.lock().unwrap().asked = Some(Instant::now());
        let st: Option<serde_json::Value> = async {
            crate::http()
                .get(format!("{}/state", control.trim_end_matches('/')))
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .ok()?
                .json()
                .await
                .ok()
        }
        .await;
        if let Some(st) = st.filter(|s| s.is_object()) {
            let mut c = app.member_boxes.lock().unwrap();
            c.read = Some(Instant::now());
            c.state = st;
        }
    }
    let c = app.member_boxes.lock().unwrap();
    if c.read.is_some_and(|t| t.elapsed() < STATE_STALE) {
        c.state.clone()
    } else {
        serde_json::Value::Null
    }
}

/// The agreed state and whose each joined box is, worked out afresh on
/// every call: a box unenlisted, a member who left or is suspended,
/// counts from the next request on.
pub(crate) async fn current(app: &Arc<App>) -> (serde_json::Value, Vec<Owned>) {
    let st = state(app).await;
    let owned = owners(&st["joined"], &enlisted(app))
        .into_iter()
        .filter(|o| app.role(&o.user) == Some(crate::pages::Role::Member))
        .collect();
    (st, owned)
}

/// The tickets any joined box came with: their pending backups stay. Read
/// from the tickets as they are, whether or not anyone owns the box now -
/// an entry not read yet, a device since removed, a member suspended keep
/// a backup as surely as an owned box does. None, and nothing may go, when
/// the agreed state is not a fresh read (current's) or any joined box's
/// ticket can't be read.
/// With them, the time to judge a week by: the earlier of this box's clock
/// and the agreed time, and only when the two are within ten minutes.
pub(crate) async fn joined_tickets(app: &Arc<App>) -> Option<(BTreeSet<String>, u64)> {
    let st = state(app).await;
    let agreed = st.get("time")?.as_u64()? / 1000;
    let now = identity::now();
    if now.abs_diff(agreed) > 600 {
        return None;
    }
    Some((tickets_in(&st)?, now.min(agreed)))
}

fn tickets_in(st: &serde_json::Value) -> Option<BTreeSet<String>> {
    // a state without its boxes is no state
    st.get("boxes")?.as_object()?;
    let joined = match st.get("joined") {
        None | Some(serde_json::Value::Null) => return Some(BTreeSet::new()),
        Some(j) => j.as_object()?,
    };
    let mut out = BTreeSet::new();
    for j in joined.values() {
        match j.get("ticket") {
            None => continue,
            Some(t) => {
                let t = t.as_str()?;
                if t.is_empty() {
                    continue;
                }
                // a ticket that names none of the right shape: unsure, so
                // nothing goes
                // TODO(enlist): the enlisted tickets, from the identity view
                out.insert(ticket_of(t)?);
            }
        }
    }
    Some(out)
}

/// GET /_dd/boxes/mine: the boxes this member set up, with how each is as
/// its own heartbeats say (the agreed state's), for their Devices tab.
pub(crate) async fn mine(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (st, owned) = current(&app).await;
    let rows: Vec<serde_json::Value> = owned
        .into_iter()
        .filter(|o| o.user == user)
        .map(|o| {
            let b = &st["boxes"][&o.name];
            serde_json::json!({
                "name": o.name,
                "ticket": o.ticket,
                "joined": o.at,
                "lastSeen": b["lastSeen"],
                "facts": b["facts"],
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "boxes": rows })).into_response()
}

/// GET /_dd/network/joined: the same boxes, as the app's installer window
/// reads them to move each one's pending backup into the member's images:
/// `[{ "name", "ticket" }]`
pub(crate) async fn joined_mine(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let rows: Vec<serde_json::Value> = current(&app)
        .await
        .1
        .into_iter()
        .filter(|o| o.user == user)
        .map(|o| serde_json::json!({ "name": o.name, "ticket": o.ticket }))
        .collect();
    axum::Json(rows).into_response()
}

/// A request path as nginx matched it is the same path the gate reads:
/// no escapes, no dot segments, no doubled slashes. nginx decodes and
/// resolves those before it picks a location, and X-Original-URI is the
/// request as sent, so on the metrics host any path that is not already
/// in that form is refused rather than read two ways.
pub(crate) fn plain_path(uri: &str) -> bool {
    let path = uri.split('?').next().unwrap_or_default();
    path.starts_with('/')
        && !path.contains('%')
        && !path.contains("//")
        && !path.contains('\\')
        && !path.split('/').any(|s| s == "." || s == "..")
}

/// The joined box a metrics request is for: `/joined/<its name>/api/v1/...`
pub(crate) fn metrics_box(uri: &str) -> Option<String> {
    let name = uri.strip_prefix("/joined/")?.split('/').next()?;
    let ok = name.len() == 11
        && name.starts_with('j')
        && name[1..]
            .bytes()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    (ok && uri[8 + name.len()..].starts_with("/api/v1/")).then(|| name.to_string())
}

/// May this person read these metrics: a joined box's (`/joined/<name>/`)
/// if it is theirs, or they are the fleet's admin, who sees every box; the
/// rest, if they see the fleet's boxes (`sees_box`: the owner of the
/// observe box, or the admin)
pub(crate) fn sees_metrics(
    user: &str,
    sees_box: bool,
    admin: bool,
    uri: &str,
    owned: &[Owned],
) -> bool {
    if !plain_path(uri) {
        return false;
    }
    match metrics_box(uri) {
        Some(name) => admin || owned.iter().any(|o| o.name == name && o.user == user),
        None => sees_box,
    }
}

/// A pending backup's ticket, from a request's bearer: the member and the
/// ticket id, when it is a join ticket good now.
pub(crate) fn pending_ticket(app: &App, headers: &HeaderMap) -> Option<(String, String)> {
    let tok = crate::bearer(headers)?;
    let source = UnverifiedBiscuit::from_base64(tok)
        .ok()?
        .print_block_source(0)
        .ok()?;
    if !source.contains("purpose(\"join\")") {
        return None;
    }
    match verify_ticket(|u| app.entry(u), tok, identity::now()) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("join ticket refused: {e:#}");
            None
        }
    }
}

/// The pending backups under each member's images, by ticket id: those
/// first written PENDING_SECS ago whose box has not joined go.
pub(crate) fn expired(
    pending: &BTreeMap<String, u64>,
    joined: &BTreeSet<String>,
    now: u64,
) -> Vec<String> {
    pending
        .iter()
        .filter(|(t, first)| !joined.contains(*t) && **first + PENDING_SECS <= now)
        .map(|(t, _)| t.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use biscuit_auth::{Biscuit, KeyPair, builder::Algorithm};

    fn device() -> KeyPair {
        KeyPair::new_with_algorithm(Algorithm::Ed25519)
    }

    /// as auth::device::mint_join_ticket makes one
    fn ticket(kp: &KeyPair, user: &str, id: &str, exp: u64) -> String {
        Biscuit::builder()
            .fact(format!("user({user:?})").as_str())
            .unwrap()
            .fact(format!("ticket({id:?})").as_str())
            .unwrap()
            .fact("purpose(\"join\")")
            .unwrap()
            .check(format!("check if time($t), $t < {exp}").as_str())
            .unwrap()
            .check("check if operation(\"join\")")
            .unwrap()
            .build(kp)
            .unwrap()
            .to_base64()
            .unwrap()
    }

    fn entry_with(name: &str, kp: &KeyPair, removed: Vec<String>) -> identity::SignedEntry {
        let root = identity::generate();
        let pk = base64::engine::general_purpose::STANDARD.encode(kp.public().to_bytes());
        let e = identity::Entry {
            name: name.into(),
            root: identity::encode_public(&root.verifying_key()),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![identity::Device {
                fingerprint: identity::fingerprint(&pk),
                public_key: pk,
                added: 1,
            }],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            terms: None,
            removed_boxes: removed,
            version: 1,
            updated: 1,
        };
        identity::sign(e, &root).unwrap()
    }

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn a_box_is_the_member_its_enlisted_with() {
        let kp = device();
        let t = ticket(&kp, "ann", ID, identity::now() + 60);
        let joined = serde_json::json!({
            "jaaaaaaaaaa": { "at": 2, "ticket": t, "key": "K" },
            // a ticket alone makes no box anyone's
            "jbbbbbbbbbb": { "at": 1, "ticket": t },
            "jcccccccccc": { "at": 3 },
        });
        let enlisted = BTreeMap::from([
            ("jaaaaaaaaaa".to_string(), "ann".to_string()),
            ("jcccccccccc".to_string(), "bob".to_string()),
            // enlisted, not joined yet
            ("jdddddddddd".to_string(), "ann".to_string()),
        ]);
        let o = owners(&joined, &enlisted);
        assert_eq!(
            o.iter()
                .map(|o| (
                    o.name.as_str(),
                    o.user.as_str(),
                    o.ticket.as_str(),
                    o.key.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                ("jaaaaaaaaaa", "ann", ID, "K"),
                ("jcccccccccc", "bob", "", "")
            ]
        );
        assert!(owners(&joined, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn a_ticket_opens_nothing_else() {
        let kp = device();
        let t = ticket(&kp, "ann", ID, identity::now() + 60);
        let ann = entry_with("ann", &kp, vec![]);
        let of = |u: &str| Ok((u == "ann").then(|| ann.clone()));
        for op in ["access", "enrol", identity::DIRECTORY_READ] {
            assert!(
                crate::verify_device_token(of, &t, op, "files.example").is_err(),
                "a join ticket was taken for {op}"
            );
        }
        assert!(verify_ticket(of, &t, identity::now()).is_ok());
        // and the other way: any other token of ann's is no ticket
        let a = Biscuit::builder()
            .fact("user(\"ann\")")
            .unwrap()
            .build(&kp)
            .unwrap()
            .to_base64()
            .unwrap();
        assert!(verify_ticket(of, &a, identity::now()).is_err());
    }

    #[test]
    fn a_ticket_id_is_the_authority_blocks_one_exact_fact() {
        let ok = format!("user(\"ann\");\nticket(\"{ID}\");\npurpose(\"join\");\n");
        assert_eq!(ticket_id(&ok).as_deref(), Some(ID));
        for bad in [
            format!("ticket(\"{ID}\");\nticket(\"{ID}\");\n"),
            format!("xticket(\"{ID}\");\n"),
            format!("ticket(\"{}\");\n", ID.to_uppercase()),
            format!("ticket(\"{ID}0\");\n"),
            format!("ticket(\"{ID}\", \"x\");\n"),
            format!("ticket(\"{ID}\");\nnotaticket(\"x\");\n"),
            "user(\"ann\");\n".to_string(),
        ] {
            assert_eq!(ticket_id(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_block_a_holder_adds_names_no_ticket() {
        // a token of ann's for join that names no ticket, and a block a
        // holder appended naming one: only the signed block counts
        let kp = device();
        let t = Biscuit::builder()
            .fact("user(\"ann\")")
            .unwrap()
            .fact("purpose(\"join\")")
            .unwrap()
            .check("check if operation(\"join\")")
            .unwrap()
            .build(&kp)
            .unwrap()
            .append(
                biscuit_auth::builder::BlockBuilder::new()
                    .fact(format!("ticket({ID:?})").as_str())
                    .unwrap(),
            )
            .unwrap()
            .to_base64()
            .unwrap();
        let ann = entry_with("ann", &kp, vec![]);
        let of = |u: &str| Ok((u == "ann").then(|| ann.clone()));
        assert!(verify_ticket(of, &t, identity::now()).is_err());
    }

    #[test]
    fn a_metrics_request_names_one_joined_box() {
        assert_eq!(
            metrics_box("/joined/j0123456789/api/v1/query?query=up").as_deref(),
            Some("j0123456789")
        );
        assert_eq!(metrics_box("/api/v1/query?query=up"), None);
        assert_eq!(metrics_box("/joined/node1/api/v1/query"), None);
        assert_eq!(metrics_box("/joined/j0123456789/_dd/login"), None);
        assert_eq!(metrics_box("/joined/J0123456789/api/v1/query"), None);
    }

    #[test]
    fn a_joined_boxs_metrics_are_its_members_and_the_admins() {
        let owned = [Owned {
            name: "j0123456789".into(),
            user: "ann".into(),
            ticket: ID.into(),
            at: 0,
            key: String::new(),
        }];
        let q = "/joined/j0123456789/api/v1/query?query=up";
        assert!(sees_metrics("ann", false, false, q, &owned));
        // the fleet's admin sees every box, this one too
        assert!(sees_metrics("david", true, true, q, &owned));
        // another member does not, nor the observe box's owner who is not
        // the admin
        assert!(!sees_metrics("bob", false, false, q, &owned));
        assert!(!sees_metrics("olga", true, false, q, &owned));
        assert!(!sees_metrics(
            "ann",
            false,
            false,
            "/joined/jffffffffff/api/v1/query",
            &owned
        ));
        // a path nginx reads one way and the gate another is nobody's:
        // nginx would route each of these to the joined box's location
        for odd in [
            "/api/v1/../../joined/j0123456789/api/v1/query",
            "/%6aoined/j0123456789/api/v1/query",
            "/joined/j0123456789/api/v1/%71uery",
            "//joined/j0123456789/api/v1/query",
            "/joined/./j0123456789/api/v1/query",
        ] {
            assert!(!sees_metrics("david", true, true, odd, &owned), "{odd}");
            assert!(!sees_metrics("ann", false, false, odd, &owned), "{odd}");
        }
        // and the fleet's view is as it was
        assert!(sees_metrics(
            "david",
            true,
            true,
            "/api/v1/query?query=up",
            &owned
        ));
        assert!(!sees_metrics(
            "ann",
            false,
            false,
            "/api/v1/query?query=up",
            &owned
        ));
    }

    #[test]
    fn the_purge_keeps_everything_unless_it_is_sure() {
        let kp = device();
        let t = ticket(&kp, "ann", ID, identity::now() + 60);
        // the box joined, owned by nobody now (no entry for ann): kept
        let st = serde_json::json!({ "boxes": {}, "joined": { "jaaaaaaaaaa": { "ticket": t } } });
        assert_eq!(tickets_in(&st), Some(BTreeSet::from([ID.to_string()])));
        // no state, or one without its boxes: nothing goes
        assert_eq!(tickets_in(&serde_json::Value::Null), None);
        assert_eq!(tickets_in(&serde_json::json!({ "joined": {} })), None);
        // one joined box whose ticket can't be read: nothing goes
        for bad in [
            serde_json::json!("not a biscuit"),
            serde_json::json!(7),
            serde_json::json!(ticket(&kp, "ann", "short", identity::now() + 60)),
        ] {
            let st = serde_json::json!({ "boxes": {}, "joined": {
                "jaaaaaaaaaa": { "ticket": t },
                "jbbbbbbbbbb": { "ticket": bad },
            } });
            assert_eq!(tickets_in(&st), None);
        }
        // a joined map that is not one: nothing goes
        assert_eq!(
            tickets_in(&serde_json::json!({ "boxes": {}, "joined": [] })),
            None
        );
        // strangers, and no joined boxes at all, are sure answers
        let st = serde_json::json!({ "boxes": {}, "joined": { "jaaaaaaaaaa": {} } });
        assert_eq!(tickets_in(&st), Some(BTreeSet::new()));
        assert_eq!(
            tickets_in(&serde_json::json!({ "boxes": {} })),
            Some(BTreeSet::new())
        );
    }

    #[test]
    fn a_pending_backup_goes_a_week_on_unless_its_box_joined() {
        let mut p = BTreeMap::new();
        p.insert("old".to_string(), 0);
        p.insert("kept".to_string(), 0);
        p.insert("new".to_string(), PENDING_SECS);
        let joined = BTreeSet::from(["kept".to_string()]);
        assert_eq!(expired(&p, &joined, PENDING_SECS), vec!["old".to_string()]);
    }
}
