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
    let source = UnverifiedBiscuit::from_base64(token)?
        .print_block_source(0)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    anyhow::ensure!(
        source.matches("ticket(").count() == 1,
        "a join ticket names one ticket"
    );
    let id = ticket_id(&source).ok_or_else(|| anyhow::anyhow!("no ticket fact"))?;
    Ok((user, id))
}

/// `ticket("<32 hex>")` out of the authority block's source
fn ticket_id(source: &str) -> Option<String> {
    let i = source.find("ticket(\"")? + 8;
    let rest = &source[i..];
    let id = &rest[..rest.find('"')?];
    (id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())).then(|| id.to_string())
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
        });
    }
    out
}

/// The agreed state's joined boxes and whose they are, read from this
/// box's dd-control at most every half minute.
#[derive(Default)]
pub(crate) struct Cache {
    at: Option<Instant>,
    state: serde_json::Value,
    owned: Vec<Owned>,
}

pub(crate) async fn current(app: &Arc<App>) -> (serde_json::Value, Vec<Owned>) {
    {
        let c = app.member_boxes.lock().unwrap();
        if c.at.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            return (c.state.clone(), c.owned.clone());
        }
    }
    let Some(control) = &app.control else {
        return (serde_json::Value::Null, vec![]);
    };
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
    let Some(st) = st else {
        let c = app.member_boxes.lock().unwrap();
        return (c.state.clone(), c.owned.clone());
    };
    let owned = owners(&st["joined"], |u| app.entry(u));
    let mut c = app.member_boxes.lock().unwrap();
    c.at = Some(Instant::now());
    c.state = st.clone();
    c.owned = owned.clone();
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

/// The path a ticket may write: pending/<its id>/ and below, nothing else
pub(crate) fn ticket_may(raw: &str, ticket: &str) -> bool {
    raw.strip_prefix("pending/")
        .and_then(|r| r.strip_prefix(ticket))
        .is_some_and(|r| r.is_empty() || r.starts_with('/'))
        && !raw.split('/').any(|s| s == ".." || s == ".")
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
    fn a_ticket_writes_its_own_pending_folder_and_nothing_else() {
        assert!(ticket_may(&format!("pending/{ID}"), ID));
        assert!(ticket_may(&format!("pending/{ID}/data/ab/cd"), ID));
        assert!(!ticket_may(&format!("pending/{ID}x/config"), ID));
        assert!(!ticket_may(
            "pending/ffffffffffffffffffffffffffffffff/config",
            ID
        ));
        assert!(!ticket_may("config", ID));
        assert!(!ticket_may("snapshots/x", ID));
        assert!(!ticket_may("pending", ID));
        assert!(!ticket_may(&format!("pending/{ID}/../../config"), ID));
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
