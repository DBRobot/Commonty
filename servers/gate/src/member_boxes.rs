//! A member's own boxes (docs/dev/joining.md): a box set up from the
//! member's app comes with their join ticket, which the control plane
//! carries with the joined box and judges no more than it judges the
//! directory. Every gate checks it the same way, against the member's
//! signed entry, so every box agrees whose it is, and none vouches alone.
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

/// A joined box that is a member's: the ticket it came with verifies.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Owned {
    pub name: String,
    pub user: String,
    pub ticket: String,
    /// when it joined, agreed ms
    pub at: i64,
    /// its key, as it joined with it: the name is this key's (JoinName)
    pub key: String,
}

/// A join ticket, as a box came with it: signed by a device in the
/// member's entry, minted for `join`, and good at `at` (seconds). The
/// member and the ticket's id.
pub(crate) fn verify_ticket(
    entry_of: impl Fn(&str) -> anyhow::Result<Option<identity::SignedEntry>>,
    token: &str,
    at: u64,
) -> anyhow::Result<(String, String)> {
    let (user, _) = crate::verify_device_token_at(entry_of, token, "join", "", at)?;
    // the authority block alone: what the member's device signed, never a
    // block a holder added after
    let source = UnverifiedBiscuit::from_base64(token)?
        .print_block_source(0)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let id = ticket_id(&source).ok_or_else(|| anyhow::anyhow!("not one ticket fact"))?;
    Ok((user, id))
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

/// Whose each joined box is, from the agreed state's `joined`: in the
/// order they joined, a ticket counts for the first box it verifies for
/// and no other, and not once its member has removed that box.
pub(crate) fn owners(
    joined: &serde_json::Value,
    entry_of: impl Fn(&str) -> anyhow::Result<Option<identity::SignedEntry>> + Copy,
) -> Vec<Owned> {
    let mut boxes: Vec<(i64, &String, &str)> = joined
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, j)| {
            let t = j["ticket"].as_str().filter(|t| !t.is_empty())?;
            Some((j["at"].as_i64().unwrap_or(0), name, t))
        })
        .collect();
    boxes.sort();
    let mut used = BTreeSet::new();
    let mut out = Vec::new();
    for (at, name, token) in boxes {
        let Ok((user, ticket)) = verify_ticket(entry_of, token, (at.max(0) / 1000) as u64) else {
            continue;
        };
        if !used.insert(ticket.clone()) {
            continue;
        }
        let removed = entry_of(&user)
            .ok()
            .flatten()
            .is_some_and(|e| e.entry.removed_boxes.contains(&ticket));
        if removed {
            continue;
        }
        out.push(Owned {
            name: name.clone(),
            user,
            ticket,
            at,
            key: joined[name.as_str()]["key"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        });
    }
    out
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
/// every call from it and the members' entries as they are now: a box its
/// member removed, a member who left or is suspended, a device since taken
/// out of the entry - each counts from the next request on.
pub(crate) async fn current(app: &Arc<App>) -> (serde_json::Value, Vec<Owned>) {
    let st = state(app).await;
    let owned = owners(&st["joined"], |u| app.entry(u))
        .into_iter()
        .filter(|o| app.role(&o.user) == Some(crate::pages::Role::Member))
        .collect();
    (st, owned)
}

/// The tickets whose boxes joined: their pending backups stay. None when
/// the agreed state could not be read, when nothing may go.
pub(crate) async fn joined_tickets(app: &Arc<App>) -> Option<BTreeSet<String>> {
    let (st, owned) = current(app).await;
    st.is_object()
        .then(|| owned.into_iter().map(|o| o.ticket).collect())
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
/// only if it is theirs, whoever else they are; the rest, if they see the
/// fleet's boxes (`sees_box`: the owner of the observe box, or the admin)
pub(crate) fn sees_metrics(user: &str, sees_box: bool, uri: &str, owned: &[Owned]) -> bool {
    if !plain_path(uri) {
        return false;
    }
    match metrics_box(uri) {
        Some(name) => owned.iter().any(|o| o.name == name && o.user == user),
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
    fn a_ticket_is_its_members_for_one_box_and_not_once_removed() {
        let kp = device();
        let now = identity::now();
        let t = ticket(&kp, "ann", ID, now + PENDING_SECS);
        let forged = ticket(&device(), "ann", ID, now + PENDING_SECS);
        let ann = entry_with("ann", &kp, vec![]);
        let of = |u: &str| Ok((u == "ann").then(|| ann.clone()));
        let at = now as i64 * 1000;
        let joined = serde_json::json!({
            "jaaaaaaaaaa": { "at": at, "ticket": t },
            // the same ticket again, later: someone's copy of it
            "jbbbbbbbbbb": { "at": at + 1, "ticket": t },
            // signed by a key that is none of ann's devices
            "jcccccccccc": { "at": at - 1, "ticket": forged },
            "jdddddddddd": { "at": at },
        });
        let o = owners(&joined, of);
        assert_eq!(o.len(), 1, "{o:?}");
        assert_eq!(
            (o[0].name.as_str(), o[0].user.as_str()),
            ("jaaaaaaaaaa", "ann")
        );
        assert_eq!(o[0].ticket, ID);
        // a box joining after the ticket's week is nobody's
        let late = serde_json::json!({
            "jaaaaaaaaaa": { "at": at + (PENDING_SECS as i64 + 1) * 1000, "ticket": t }
        });
        assert!(owners(&late, of).is_empty());
        // removed by its member: nobody's
        let gone = entry_with("ann", &kp, vec![ID.into()]);
        assert!(owners(&joined, |u: &str| Ok((u == "ann").then(|| gone.clone()))).is_empty());
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
    fn a_ticket_counts_only_with_a_device_still_in_the_entry() {
        let kp = device();
        let t = ticket(&kp, "ann", ID, identity::now() + 60);
        let joined = serde_json::json!({ "jaaaaaaaaaa": { "at": identity::now() as i64 * 1000, "ticket": t, "key": "K" } });
        let ann = entry_with("ann", &kp, vec![]);
        let o = owners(&joined, |u: &str| Ok((u == "ann").then(|| ann.clone())));
        assert_eq!(o[0].key, "K");
        // the device that signed it, taken out of ann's entry since
        let other = entry_with("ann", &device(), vec![]);
        assert!(owners(&joined, |u: &str| Ok((u == "ann").then(|| other.clone()))).is_empty());
        // and a member with no entry at all owns nothing
        assert!(owners(&joined, |_: &str| Ok(None)).is_empty());
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
    fn a_joined_boxs_metrics_are_its_members_and_nobody_elses() {
        let owned = [Owned {
            name: "j0123456789".into(),
            user: "ann".into(),
            ticket: ID.into(),
            at: 0,
            key: String::new(),
        }];
        let q = "/joined/j0123456789/api/v1/query?query=up";
        assert!(sees_metrics("ann", false, q, &owned));
        // the fleet's admin sees every release box, and not this one
        assert!(!sees_metrics("david", true, q, &owned));
        assert!(!sees_metrics("bob", false, q, &owned));
        assert!(!sees_metrics(
            "ann",
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
            assert!(!sees_metrics("david", true, odd, &owned), "{odd}");
            assert!(!sees_metrics("ann", false, odd, &owned), "{odd}");
        }
        // and the fleet's view is as it was
        assert!(sees_metrics(
            "david",
            true,
            "/api/v1/query?query=up",
            &owned
        ));
        assert!(!sees_metrics(
            "ann",
            false,
            "/api/v1/query?query=up",
            &owned
        ));
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
