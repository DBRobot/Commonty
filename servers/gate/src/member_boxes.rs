//! A member's own boxes (docs/dev/joining.md). Whose a joined box is,
//! is the agreed identity state's word: each member's `boxes`, the boxes
//! the judge has them enlisted with (state/ident). No gate judges a join
//! ticket for it, so every box agrees whose it is, and none vouches alone.
//!
//! What being a member's gives the box is the member's: it is on their
//! Devices tab, they see how it is, and the backup made of its old disk
//! is kept for them. It runs nothing a stranger does not.

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
/// one backup, never whose a box is, and only until a box is enlisted
/// with it (pending_ticket).
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

/// Whose each joined box is: `enlisted` is the agreed identity state's
/// box -> whose and with which ticket. A joined box no member has enlisted
/// is nobody's; an enlisted box not in the control state's `joined` is not
/// here yet.
pub(crate) fn owners(
    joined: &serde_json::Value,
    enlisted: &HashMap<String, crate::ident::Owner>,
) -> Vec<Owned> {
    let mut out: Vec<Owned> = joined
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, j)| {
            let o = enlisted.get(name)?;
            Some(Owned {
                name: name.clone(),
                user: o.owner.clone(),
                ticket: o.ticket.clone(),
                at: j["at"].as_i64().unwrap_or(0),
                key: j["key"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| (a.at, &a.name).cmp(&(b.at, &b.name)));
    out
}

/// The agreed identity state's enlistments; None when this box does not
/// read it (identityFrom replay) or has not read it yet
fn enlisted(app: &App) -> Option<HashMap<String, crate::ident::Owner>> {
    Some(app.directory.ident.as_ref()?.state()?.enlisted.clone())
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
    let owned = owners(&st["joined"], &enlisted(app).unwrap_or_default())
        .into_iter()
        .filter(|o| app.registered(&o.user))
        .collect();
    (st, owned)
}

/// The joined boxes that came with a ticket of `user`'s and no one has
/// enlisted: theirs to enlist. The judge's word (the agreed identity
/// state's `waiting`): the box's ticket claim signed by a device of theirs,
/// for them, good now, unused, and the first box to join with its id. The
/// ticket itself is never in the agreed state, only its claim.
fn waiting(
    waiting: &HashMap<String, crate::ident::Owner>,
    user: &str,
    enlisted: &HashMap<String, crate::ident::Owner>,
) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = waiting
        .iter()
        .filter(|(name, w)| w.owner == user && !enlisted.contains_key(*name))
        .map(|(name, w)| {
            serde_json::json!({
                "name": name,
                "ticket": w.ticket,
                "joined": w.since,
                "enlisted": false,
            })
        })
        .collect();
    out.sort_by_key(|r| r["name"].as_str().unwrap_or_default().to_string());
    out
}

/// The agreed identity state's waiting boxes; none when it is not read
fn waiting_of(app: &App) -> HashMap<String, crate::ident::Owner> {
    app.directory
        .ident
        .as_ref()
        .and_then(|a| a.state())
        .map(|s| s.waiting.clone())
        .unwrap_or_default()
}

/// The tickets boxes were enlisted with (the agreed identity state's,
/// never a joined box's word): their pending backups stay. None, and
/// nothing may go, when the agreed state is not a fresh read (current's)
/// or the identity state has not been read.
/// With them, the time to judge a week by: the earlier of this box's clock
/// and the agreed time, and only when the two are within ten minutes.
pub(crate) async fn joined_tickets(app: &Arc<App>) -> Option<(BTreeSet<String>, u64)> {
    let st = state(app).await;
    let agreed = st.get("time")?.as_u64()? / 1000;
    let now = identity::now();
    if now.abs_diff(agreed) > 600 {
        return None;
    }
    Some((tickets_in(&st, enlisted(app).as_ref())?, now.min(agreed)))
}

fn tickets_in(
    st: &serde_json::Value,
    enlisted: Option<&HashMap<String, crate::ident::Owner>>,
) -> Option<BTreeSet<String>> {
    // a state without its boxes is no state
    st.get("boxes")?.as_object()?;
    Some(enlisted?.values().map(|o| o.ticket.clone()).collect())
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
    let waiting = waiting(
        &waiting_of(&app),
        &user,
        &enlisted(&app).unwrap_or_default(),
    );
    axum::Json(serde_json::json!({ "boxes": rows, "waiting": waiting })).into_response()
}

/// GET /_dd/network/joined: the same boxes, as the app reads them to move
/// each one's pending backup into the member's images (`enlisted`), and
/// the boxes that joined with a ticket of theirs and wait for their root to
/// enlist them: `[{ "name", "ticket", "enlisted" }]`
pub(crate) async fn joined_mine(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (_, owned) = current(&app).await;
    let mut rows: Vec<serde_json::Value> = owned
        .into_iter()
        .filter(|o| o.user == user)
        .map(|o| serde_json::json!({ "name": o.name, "ticket": o.ticket, "enlisted": true }))
        .collect();
    rows.extend(waiting(
        &waiting_of(&app),
        &user,
        &enlisted(&app).unwrap_or_default(),
    ));
    axum::Json(rows).into_response()
}

/// PUT /_dd/boxes/{kind}: a member's root-signed `enlist` or `unenlist` of
/// a box, or the fleet admin's `extend` of a member's grace, into the
/// control plane's log. The directory's PUT, step for step: the one
/// per-visitor limiter, a body of 64 KiB at most, the kind the path names,
/// signed in, the item theirs, its signature checked against the root the
/// agreed identity state has for them now, then the shared path
/// (duplicates by the item hash, allowances counted after the signature).
/// Judging it is the control plane's.
pub(crate) async fn put_item(
    State(app): State<Arc<App>>,
    axum::extract::Path(kind): axum::extract::Path<String>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    let d = &app.directory;
    let body = match crate::directory::read_put(d, &headers, body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    use identity::envelope::{ENLIST, EXTEND, UNENLIST};
    if ![ENLIST, UNENLIST, EXTEND].contains(&kind.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(user) = app.identify(&headers, "access") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(view) = d.ident.as_ref().and_then(|a| a.state()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "who is who is not read yet",
        )
            .into_response();
    };
    if !d.takes_v2() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "this box sends no envelopes yet",
        )
            .into_response();
    }
    let env = match crate::ident::Envelope::parse(&body) {
        Ok(e) => e,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    if env.kind != kind {
        return (
            StatusCode::BAD_REQUEST,
            format!("a {} envelope, put where a {kind} goes", env.kind),
        )
            .into_response();
    }
    let Some(on_file) = view.members.get(&user).and_then(|m| m.entry()) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    // theirs: an enlistment names the signed-in member's root, and a box
    // the agreed view has as theirs to unenlist, or as nobody's to enlist;
    // an extension is the admin's alone
    let owner = env
        .field("box")
        .and_then(|b| view.enlisted.get(b))
        .map(|o| o.owner.as_str());
    let theirs = match kind.as_str() {
        EXTEND => app.admin.as_deref() == Some(user.as_str()),
        _ if env.field("member") != Some(identity::member_id(&on_file.entry.root).as_str()) => {
            false
        }
        UNENLIST => owner == Some(user.as_str()),
        _ => owner.is_none(),
    };
    if !theirs {
        return (StatusCode::FORBIDDEN, "not theirs to say").into_response();
    }
    let check = || -> std::result::Result<String, String> {
        let e: identity::envelope::Envelope =
            serde_json::from_slice(&body).map_err(|e| format!("not an envelope: {e}"))?;
        identity::envelope::verify_root(&e, &on_file.entry.root, &on_file.entry.passkeys)
            .map_err(|e| e.to_string())?;
        Ok(on_file.entry.root.clone())
    };
    match d.submit_verified(&user, &env, &body, check).await {
        Ok(hash) => (
            StatusCode::ACCEPTED,
            axum::Json(serde_json::json!({ "submitted": true, "item": hash })),
        )
            .into_response(),
        Err((status, why)) => (status, why).into_response(),
    }
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
    let (user, id) = match verify_ticket(|u| app.entry(u), tok, identity::now()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("join ticket refused: {e:#}");
            return None;
        }
    };
    // a ticket a box was enlisted with is used: its backup is done, and it
    // opens nothing more. A box that reads the identity state and has not
    // yet takes none
    if app.directory.ident.is_some() {
        let used = enlisted(app)?.values().any(|o| o.ticket == id);
        if used {
            return None;
        }
    }
    Some((user, id))
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

    fn entry_with(name: &str, kp: &KeyPair) -> identity::SignedEntry {
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
            removed_boxes: vec![],
            version: 1,
            updated: 1,
        };
        identity::sign(e, &root).unwrap()
    }

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn owner(user: &str, ticket: &str) -> crate::ident::Owner {
        crate::ident::Owner {
            ticket: ticket.into(),
            owner: user.into(),
            since: 1,
        }
    }

    #[test]
    fn a_box_is_the_member_its_enlisted_with() {
        let kp = device();
        let t = ticket(&kp, "ann", ID, identity::now() + 60);
        let other = ticket(
            &kp,
            "ann",
            "ffeeddccbbaa99887766554433221100",
            identity::now() + 60,
        );
        let joined = serde_json::json!({
            // the ticket is the enlistment's, never the one the box came with
            "jaaaaaaaaaa": { "at": 2, "ticket": other, "key": "K" },
            // a ticket alone makes no box anyone's
            "jbbbbbbbbbb": { "at": 1, "ticket": t },
            "jcccccccccc": { "at": 3 },
        });
        let enlisted = HashMap::from([
            ("jaaaaaaaaaa".to_string(), owner("ann", ID)),
            ("jcccccccccc".to_string(), owner("bob", "")),
            // enlisted, not joined yet
            ("jdddddddddd".to_string(), owner("ann", ID)),
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
        assert!(owners(&joined, &HashMap::new()).is_empty());
    }

    #[test]
    fn a_box_waits_for_its_member_by_the_judges_word() {
        let w = HashMap::from([
            ("jaaaaaaaaaa".to_string(), owner("ann", ID)),
            ("jbbbbbbbbbb".to_string(), owner("bob", OTHER)),
            // enlisted since this view of waiting was read
            ("jcccccccccc".to_string(), owner("ann", OTHER)),
        ]);
        let enlisted = HashMap::from([("jcccccccccc".to_string(), owner("ann", OTHER))]);
        let got = waiting(&w, "ann", &enlisted);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["name"], "jaaaaaaaaaa");
        assert_eq!(got[0]["ticket"], ID);
        assert_eq!(got[0]["joined"], 1);
        assert_eq!(got[0]["enlisted"], false);
        assert!(waiting(&w, "eve", &enlisted).is_empty());
    }

    const OTHER: &str = "ffeeddccbbaa99887766554433221100";

    // a protobuf message's length-delimited fields, by number
    fn pb_fields(mut b: &[u8]) -> Vec<(u64, Vec<u8>)> {
        fn varint(b: &mut &[u8]) -> u64 {
            let mut v = 0u64;
            for i in 0.. {
                let c = b[0];
                *b = &b[1..];
                v |= u64::from(c & 0x7f) << (7 * i);
                if c & 0x80 == 0 {
                    break;
                }
            }
            v
        }
        let mut out = vec![];
        while !b.is_empty() {
            let tag = varint(&mut b);
            match tag & 7 {
                0 => {
                    varint(&mut b);
                }
                2 => {
                    let n = varint(&mut b) as usize;
                    out.push((tag >> 3, b[..n].to_vec()));
                    b = &b[n..];
                }
                w => panic!("wire type {w}"),
            }
        }
        out
    }

    fn pb_bytes(out: &mut Vec<u8>, num: u64, v: &[u8]) {
        let mut put = |mut x: u64| loop {
            let c = (x & 0x7f) as u8;
            x >>= 7;
            if x == 0 {
                out.push(c);
                break;
            }
            out.push(c | 0x80);
        };
        put(num << 3 | 2);
        put(v.len() as u64);
        out.extend_from_slice(v);
    }

    #[test]
    fn a_copy_of_the_agreed_state_cannot_write_a_pending_backup() {
        // what the agreed state keeps of a ticket a box joined with is its
        // id and its signed authority block (ident.Claim), never the proof
        // key that makes the biscuit usable: none of it, nor a biscuit
        // rebuilt around the block, writes the member's pending backup
        let b64 = base64::engine::general_purpose::URL_SAFE;
        let kp = device();
        let t = ticket(&kp, "ann", ID, identity::now() + 600);
        let ann = entry_with("ann", &kp);
        let of = |u: &str| Ok((u == "ann").then(|| ann.clone()));
        assert!(verify_ticket(of, &t, identity::now()).is_ok());
        let raw = b64.decode(&t).unwrap();
        let block = pb_fields(&raw)
            .into_iter()
            .find(|(n, _)| *n == 2)
            .unwrap()
            .1;
        let mut rebuilt = vec![];
        pb_bytes(&mut rebuilt, 2, &block);
        let mut proof = vec![];
        pb_bytes(&mut proof, 1, &device().private().to_bytes());
        pb_bytes(&mut rebuilt, 4, &proof);
        let mut bare = vec![];
        pb_bytes(&mut bare, 2, &block);
        for (what, tok) in [
            ("the ticket's id", ID.to_string()),
            ("the signed block", b64.encode(&block)),
            (
                "the signed block, as the state keeps it",
                base64::engine::general_purpose::STANDARD.encode(&block),
            ),
            ("a biscuit of the block with no proof", b64.encode(&bare)),
            (
                "a biscuit of the block with another proof",
                b64.encode(&rebuilt),
            ),
        ] {
            assert!(
                verify_ticket(of, &tok, identity::now()).is_err(),
                "{what} wrote the pending backup"
            );
        }
    }

    #[test]
    fn a_ticket_opens_nothing_else() {
        let kp = device();
        let t = ticket(&kp, "ann", ID, identity::now() + 60);
        let ann = entry_with("ann", &kp);
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
        let ann = entry_with("ann", &kp);
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
        let enlisted = HashMap::from([("jaaaaaaaaaa".to_string(), owner("ann", ID))]);
        // the enlisted tickets are kept, whatever ticket a box says it came with
        let st = serde_json::json!({ "boxes": {}, "joined": { "jaaaaaaaaaa": { "ticket": "x" }, "jbbbbbbbbbb": { "ticket": t } } });
        assert_eq!(
            tickets_in(&st, Some(&enlisted)),
            Some(BTreeSet::from([ID.to_string()]))
        );
        // no state, one without its boxes, or no identity state: nothing goes
        assert_eq!(tickets_in(&serde_json::Value::Null, Some(&enlisted)), None);
        assert_eq!(
            tickets_in(&serde_json::json!({ "joined": {} }), Some(&enlisted)),
            None
        );
        assert_eq!(tickets_in(&st, None), None);
        // nothing enlisted is a sure answer
        assert_eq!(
            tickets_in(&serde_json::json!({ "boxes": {} }), Some(&HashMap::new())),
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

    // --- PUT /_dd/boxes/{kind}, and access, on a whole gate reading the
    // agreed identity state from a stand-in dd-control

    type View = Arc<std::sync::Mutex<Option<serde_json::Value>>>;
    type Got = Arc<std::sync::Mutex<Vec<Vec<u8>>>>;

    /// a dd-control stand-in's submit socket: answers 202, keeps each item
    fn submit_socket(path: &std::path::Path) -> Got {
        use std::io::{BufRead as _, Read as _, Write as _};
        let got: Got = Default::default();
        let g = got.clone();
        let _ = std::fs::remove_file(path);
        let l = std::os::unix::net::UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let mut c = c.unwrap();
                let mut r = std::io::BufReader::new(c.try_clone().unwrap());
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                g.lock().unwrap().push(body);
                c.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                    .unwrap();
            }
        });
        got
    }

    /// a gate in agreed mode whose dd-control serves `view` as /identity
    /// (500 while it is None); tom is the fleet's admin
    async fn agreed_gate(what: &str, view: View) -> (String, crate::session::Sessions, Got) {
        use axum::routing::get;
        let d = std::env::temp_dir().join(format!("dd-boxes-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let v = view.clone();
        let control = axum::Router::new()
            .route(
                "/identity",
                get(move || {
                    let v = v.lock().unwrap().clone();
                    async move {
                        match v {
                            Some(v) => axum::Json(v).into_response(),
                            None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                        }
                    }
                }),
            )
            .route(
                "/state",
                get(|| async {
                    axum::Json(serde_json::json!({
                        "height": 1, "time": identity::now() * 1000, "boxes": {},
                        "joined": {"jaaaaaaaaaa": {"at": 1, "key": "K"}}
                    }))
                }),
            )
            .route(
                "/directory",
                get(|| async {
                    axum::Json(serde_json::json!({"items": [], "next": 0, "height": 1}))
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let caddr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, control).await });
        let sock = d.join("submit.sock");
        let got = submit_socket(&sock);
        let cfg = d.join("control.json");
        std::fs::write(
            &cfg,
            r#"{"boxes": {"box": {}}, "validators": [{"name": "box"}]}"#,
        )
        .unwrap();
        let (addr, _) = crate::start(crate::Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            dir: d.join("keys"),
            house_peers: vec![],
            directory_readers: vec![],
            names: vec!["127.0.0.1".into()],
            control: Some((format!("http://{caddr}"), sock, cfg)),
            identity_from: crate::ident::From::Agreed,
            domain: Some("commonty.test".into()),
            oidc: None,
            home: vec![],
            members: None,
            release_pub: None,
            web_dir: None,
            photos: None,
            library: None,
            network: None,
            demo_library: None,
            app_manifest: None,
            member_list: None,
            fleet: Default::default(),
            thanos: None,
            adblock: None,
            forge_events: None,
            storage_ledger: None,
            tmdb: None,
            search: None,
            id_origin: None,
            box_only: false,
            owner: None,
            admin: Some("tom".into()),
            boxes: Default::default(),
            mail: None,
        })
        .await
        .unwrap();
        let sessions = crate::session::Sessions::open(&d, "commonty.test", false).unwrap();
        (format!("http://{addr}"), sessions, got)
    }

    fn root_entry(name: &str, k: &ed25519_dalek::SigningKey) -> identity::SignedEntry {
        let e = identity::Entry {
            name: name.into(),
            root: identity::encode_public(&k.verifying_key()),
            recovery: identity::encode_public(&identity::generate().verifying_key()),
            recovered_from: vec![],
            devices: vec![],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            terms: None,
            removed_boxes: vec![],
            version: 1,
            updated: 1,
        };
        identity::sign(e, k).unwrap()
    }

    fn member(e: &identity::SignedEntry, status: &str, access: &str) -> serde_json::Value {
        serde_json::json!({
            "id": identity::member_id(&e.entry.root), "status": status, "access": access,
            "item": {"entry": e},
        })
    }

    fn item(kind: &str, body: serde_json::Value, k: &ed25519_dalek::SigningKey) -> Vec<u8> {
        serde_json::to_vec(&identity::envelope::seal_json(kind, &body, k).unwrap()).unwrap()
    }

    fn enlist(kind: &str, k: &ed25519_dalek::SigningKey, at: u64) -> serde_json::Value {
        let root = identity::encode_public(&k.verifying_key());
        serde_json::to_value(identity::envelope::Enlist::new(
            kind,
            &root,
            "jaaaaaaaaaa",
            ID,
            at,
        ))
        .unwrap()
    }

    async fn put(url: &str, kind: &str, cookie: Option<&str>, peer: &str, body: Vec<u8>) -> u16 {
        let mut r = reqwest::Client::new()
            .put(format!("{url}/_dd/boxes/{kind}"))
            .header("x-dd-peer", peer)
            .body(body);
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        r.send().await.unwrap().status().as_u16()
    }

    fn cookie_of(set: &str) -> String {
        set.split(';').next().unwrap().to_string()
    }

    async fn until_read(url: &str, c: &str) {
        // the gate reads /identity in the background: wait for its first read
        for _ in 0..100 {
            let r = reqwest::Client::new()
                .get(format!("{url}/_dd/me"))
                .header("cookie", c)
                .send()
                .await
                .unwrap();
            if r.status() == 200 {
                tokio::time::sleep(Duration::from_millis(1200)).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn an_enlistment_goes_through_the_directorys_checks_in_order() {
        let ann_k = identity::generate();
        let bob_k = identity::generate();
        let tom_k = identity::generate();
        let (ann, bob, tom) = (
            root_entry("ann", &ann_k),
            root_entry("bob", &bob_k),
            root_entry("tom", &tom_k),
        );
        let view: View = Arc::new(std::sync::Mutex::new(Some(serde_json::json!({
            "height": 1,
            "members": {"ann": member(&ann, "none", "none"), "bob": member(&bob, "member", "invite"), "tom": member(&tom, "member", "invite")},
        }))));
        let (url, s, got) = agreed_gate("put-checks", view.clone()).await;
        let (ca, cb, ct) = (
            cookie_of(&s.issue("ann")),
            cookie_of(&s.issue("bob")),
            cookie_of(&s.issue("tom")),
        );
        until_read(&url, &ca).await;
        let now = identity::now();
        let good = item("enlist", enlist("enlist", &ann_k, now), &ann_k);
        // not a kind put here; not signed in
        assert_eq!(put(&url, "entry", Some(&ca), "p1", good.clone()).await, 404);
        assert_eq!(put(&url, "enlist", None, "p1", good.clone()).await, 401);
        // the kind the path names
        assert_eq!(
            put(&url, "unenlist", Some(&ca), "p1", good.clone()).await,
            400
        );
        // theirs: bob signed in, sending ann's
        assert_eq!(
            put(&url, "enlist", Some(&cb), "p1", good.clone()).await,
            403
        );
        // signed by the root on file: eve's key, ann's name on it
        let eve = identity::generate();
        let mut forged = enlist("enlist", &ann_k, now);
        forged["at"] = serde_json::json!(now + 1);
        for n in 0..(crate::directory::ENVELOPES_PER_NAME * 2) {
            let peer = format!("f{n}");
            assert_eq!(
                put(
                    &url,
                    "enlist",
                    Some(&ca),
                    &peer,
                    item("enlist", forged.clone(), &eve)
                )
                .await,
                403
            );
        }
        assert!(got.lock().unwrap().is_empty());
        // her own: through, as it came, and her allowance untouched by the forgeries
        assert_eq!(
            put(&url, "enlist", Some(&ca), "p2", good.clone()).await,
            202
        );
        assert_eq!(got.lock().unwrap().last().unwrap(), &good);
        // the same item again, however it is written
        let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
        v["extra"] = serde_json::json!(1);
        assert_eq!(
            put(
                &url,
                "enlist",
                Some(&ca),
                "p3",
                serde_json::to_vec(&v).unwrap()
            )
            .await,
            409
        );
        // an unenlist of a box the agreed view does not have as hers
        let un = item("unenlist", enlist("unenlist", &ann_k, now + 2), &ann_k);
        assert_eq!(
            put(&url, "unenlist", Some(&ca), "p4", un.clone()).await,
            403
        );
        // once it is hers: hers to unenlist, and nobody's to enlist again
        view.lock().unwrap().as_mut().unwrap()["enlisted"] =
            serde_json::json!({"jaaaaaaaaaa": {"ticket": ID, "owner": "ann", "since": 1}});
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let bob_un = item("unenlist", enlist("unenlist", &bob_k, now + 2), &bob_k);
        assert_eq!(put(&url, "unenlist", Some(&cb), "p4", bob_un).await, 403);
        let bob_en = item("enlist", enlist("enlist", &bob_k, now + 2), &bob_k);
        assert_eq!(put(&url, "enlist", Some(&cb), "p4", bob_en).await, 403);
        assert_eq!(got.lock().unwrap().len(), 1);
        assert_eq!(put(&url, "unenlist", Some(&ca), "p4", un).await, 202);
        // an extension: the admin's alone
        let ext = |k: &ed25519_dalek::SigningKey| {
            item(
                "extend",
                serde_json::json!({"name": "ann", "until": now + 86400, "at": now}),
                k,
            )
        };
        assert_eq!(put(&url, "extend", Some(&cb), "p5", ext(&bob_k)).await, 403);
        assert_eq!(put(&url, "extend", Some(&ct), "p5", ext(&bob_k)).await, 403);
        assert_eq!(put(&url, "extend", Some(&ct), "p5", ext(&tom_k)).await, 202);
        assert_eq!(got.lock().unwrap().len(), 3);
        // an item the agreed log judged already
        let again = item("enlist", enlist("enlist", &ann_k, now + 3), &ann_k);
        let hash = identity::item_hash(&serde_json::from_slice(&again).unwrap());
        // (the box nobody's again: unenlisted)
        view.lock().unwrap().as_mut().unwrap()["enlisted"] = serde_json::json!({});
        view.lock().unwrap().as_mut().unwrap()["outcomes"] = serde_json::json!({ hash: "ok" });
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(put(&url, "enlist", Some(&ca), "p6", again).await, 409);
        // the limiter first: past it, nothing is read or checked
        for _ in 0..crate::directory::PUTS_PER_SOURCE {
            put(&url, "nothing", None, "busy", vec![]).await;
        }
        assert_eq!(
            put(&url, "enlist", Some(&ca), "busy", good.clone()).await,
            429
        );
        // and a body past 64 KiB is cut off
        assert_eq!(
            put(
                &url,
                "enlist",
                Some(&ca),
                "big",
                vec![b' '; crate::directory::ENVELOPE_MAX + 1]
            )
            .await,
            413
        );
        assert_eq!(got.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn no_identity_state_no_enlistment_and_no_access() {
        let view: View = Default::default();
        let (url, s, got) = agreed_gate("put-unread", view).await;
        let k = identity::generate();
        let c = cookie_of(&s.issue("ann"));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let b = item("enlist", enlist("enlist", &k, identity::now()), &k);
        assert_eq!(put(&url, "enlist", Some(&c), "p", b).await, 503);
        assert!(got.lock().unwrap().is_empty());
        let r = reqwest::Client::new()
            .get(format!("{url}/verify"))
            .header("cookie", &c)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
    }

    #[tokio::test]
    async fn services_open_for_an_invite_or_a_box_and_nothing_else() {
        let ks: Vec<_> = (0..5).map(|_| identity::generate()).collect();
        let names = ["ivy", "bo", "nan", "rex", "lou"];
        let es: Vec<_> = names
            .iter()
            .zip(&ks)
            .map(|(n, k)| root_entry(n, k))
            .collect();
        let view: View = Arc::new(std::sync::Mutex::new(Some(serde_json::json!({
            "height": 1,
            "members": {
                "ivy": member(&es[0], "member", "invite"),
                "bo": member(&es[1], "member", "box"),
                // signed up, and nothing more
                "nan": member(&es[2], "none", "none"),
                // removed: whatever else
                "rex": member(&es[3], "revoked", "none"),
                "lou": member(&es[4], "left", "none"),
            },
        }))));
        let (url, s, _got) = agreed_gate("access", view).await;
        let first = cookie_of(&s.issue("ivy"));
        until_read(&url, &first).await;
        for (name, want) in [
            ("ivy", 200),
            ("bo", 200),
            ("nan", 403),
            ("rex", 403),
            ("lou", 403),
            ("nobody", 403),
        ] {
            let c = cookie_of(&s.issue(name));
            let r = reqwest::Client::new()
                .get(format!("{url}/verify"))
                .header("cookie", &c)
                .header("x-original-host", "files.commonty.test")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status().as_u16(), want, "{name}");
            // the directory is members' to read (may_read), a member by a
            // box's as an invited one's
            let r = reqwest::Client::new()
                .get(format!("{url}/_dd/directory"))
                .header("cookie", &c)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status().as_u16(), if want == 200 { 200 } else { 401 }, "{name} lists");
        }
        // and a client reads whether someone is a member, by their root
        let c = cookie_of(&s.issue("ivy"));
        for (name, i, member) in [("bo", 1, true), ("ivy", 0, true), ("nan", 2, false)] {
            let v: serde_json::Value = reqwest::Client::new()
                .get(format!("{url}/_dd/directory/{name}/standing"))
                .header("cookie", &c)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(v["member"], member, "{name}: {v}");
            assert_eq!(v["root"], es[i].entry.root.as_str());
        }
    }
}
