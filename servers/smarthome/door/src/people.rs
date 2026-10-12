//! Who is in: the people the household let in, the people who asked, and
//! what Certificate Transparency showed. Anyone in may let someone in or
//! take someone out; no box can (docs/dev/home-controller.md, People).

use crate::{App, refuse, same_origin, state};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// how many may be waiting at once; asking is open to any member
const MAX_ASKED: usize = 50;

/// What a page shows for a root: a device root's fingerprint, or for a
/// passkey root the start of its member id.
pub fn fingerprint(root: &str) -> String {
    if root.starts_with(identity::WEBAUTHN_ROOT) {
        identity::member_id(root)[..12].to_string()
    } else {
        identity::fingerprint(root)
    }
}

pub async fn page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    match app.who(&headers) {
        Some(_) => app.assets.page_response("people", StatusCode::OK),
        None => app.assets.page_response("signin", StatusCode::OK),
    }
}

pub async fn list(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some((me, is_in)) = app.who(&headers) else {
        return refuse(StatusCode::UNAUTHORIZED, "sign in first");
    };
    let name = app.name();
    app.with_store(|s| {
        if !is_in {
            // someone outside sees only themselves
            return Json(json!({
                "door": name,
                "me": { "name": me, "in": false, "asked": s.state.asked.contains_key(&me) },
            }))
            .into_response();
        }
        let people: Vec<_> = s
            .state
            .people
            .iter()
            .map(|(n, p)| {
                json!({
                    "name": n,
                    "fingerprint": fingerprint(&p.root),
                    "since": p.since,
                    "ready": p.ha_user.is_some() && p.address.is_some(),
                })
            })
            .collect();
        let asked: Vec<_> = s
            .state
            .asked
            .iter()
            .map(|(n, a)| json!({ "name": n, "fingerprint": fingerprint(&a.entry.entry.root), "at": a.at }))
            .collect();
        Json(json!({
            "door": name,
            "me": { "name": me, "in": true },
            "people": people,
            "asked": asked,
            "ct": s.state.ct,
        }))
        .into_response()
    })
}

pub async fn ask(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !same_origin(&app, &headers) {
        return refuse(StatusCode::FORBIDDEN, "not from this door's page");
    }
    let Some(c) = crate::cookie(&headers) else {
        return refuse(StatusCode::UNAUTHORIZED, "sign in first");
    };
    let now = identity::now();
    app.with_store(|s| {
        let Some(sess) = s.session(&c, now).cloned() else {
            return refuse(StatusCode::UNAUTHORIZED, "sign in first");
        };
        if s.state.people.contains_key(&sess.name) {
            return refuse(StatusCode::CONFLICT, "you are already in");
        }
        let Some(entry) = sess.entry else {
            return refuse(StatusCode::BAD_REQUEST, "sign in again to ask");
        };
        if !s.state.asked.contains_key(&sess.name) && s.state.asked.len() >= MAX_ASKED {
            return refuse(
                StatusCode::TOO_MANY_REQUESTS,
                "too many are waiting already",
            );
        }
        s.state
            .asked
            .insert(sess.name.clone(), state::Asked { entry, at: now });
        match s.save() {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => {
                eprintln!("ask: {e:#}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    })
}

#[derive(Deserialize)]
pub struct Named {
    pub name: String,
}

/// Only someone in, from our own page.
#[allow(clippy::result_large_err)]
fn insider(app: &App, headers: &HeaderMap) -> Result<String, Response> {
    if !same_origin(app, headers) {
        return Err(refuse(StatusCode::FORBIDDEN, "not from this door's page"));
    }
    match app.who(headers) {
        Some((me, true)) => Ok(me),
        Some(_) => Err(refuse(
            StatusCode::FORBIDDEN,
            "only someone already in may do that",
        )),
        None => Err(refuse(StatusCode::UNAUTHORIZED, "sign in first")),
    }
}

#[allow(clippy::result_large_err)]
pub async fn let_in(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(Named { name }): Json<Named>,
) -> Response {
    let me = match insider(&app, &headers) {
        Ok(me) => me,
        Err(r) => return r,
    };
    let moved = app.with_store(|s| {
        if s.state.people.contains_key(&name) {
            return Err(refuse(StatusCode::CONFLICT, "already in"));
        }
        let Some(a) = s.state.asked.remove(&name) else {
            return Err(refuse(StatusCode::NOT_FOUND, "nobody by that name asked"));
        };
        s.state.people.insert(
            name.clone(),
            state::Person {
                root: a.entry.entry.root.clone(),
                entry: a.entry,
                ha_user: None,
                address: None,
                since: identity::now(),
            },
        );
        s.save().map_err(|e| {
            eprintln!("let-in: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })
    });
    if let Err(r) = moved {
        return r;
    }
    eprintln!("let-in: {me} let {name} in");
    app.backup_now.notify_one();
    match app.reconcile_ha().await {
        Ok(_) => Json(json!({ "ready": true })).into_response(),
        Err(e) => {
            // in at the door; Home Assistant catches up when it can
            eprintln!("let-in: home assistant: {e:#}");
            app.reconcile.notify_one();
            (StatusCode::ACCEPTED, Json(json!({ "ready": false }))).into_response()
        }
    }
}

#[allow(clippy::result_large_err)]
pub async fn remove(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(Named { name }): Json<Named>,
) -> Response {
    let me = match insider(&app, &headers) {
        Ok(me) => me,
        Err(r) => return r,
    };
    let removed = app.with_store(|s| {
        if !s.state.people.contains_key(&name) {
            // a waiting ask can be turned down the same way
            if s.state.asked.remove(&name).is_some() {
                return s
                    .save()
                    .map(|_| false)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }
            return Err(refuse(StatusCode::NOT_FOUND, "nobody by that name is in"));
        }
        if s.state.people.len() == 1 {
            return Err(refuse(
                StatusCode::CONFLICT,
                "the last person in cannot be removed: nobody could open the door",
            ));
        }
        if let Some(a) = s.state.people.remove(&name).and_then(|p| p.address) {
            s.state.retired.insert(a);
        }
        s.state.sessions.retain(|_, sess| sess.name != name);
        s.save()
            .map(|_| true)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    });
    match removed {
        Err(r) => r,
        Ok(false) => StatusCode::NO_CONTENT.into_response(),
        Ok(true) => {
            eprintln!("remove: {me} removed {name}");
            app.backup_now.notify_one();
            // already shut out at the door; this takes their address too
            match app.rewrite_users().await {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => {
                    eprintln!("remove: home assistant: {e:#}");
                    refuse(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "removed at the door; Home Assistant could not be restarted",
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::signin::tests::device_sig;
    use crate::tests::*;
    use std::sync::atomic::Ordering;

    async fn post(url: &str, path: &str, cookie: &str, body: serde_json::Value) -> u16 {
        client()
            .post(format!("{url}{path}"))
            .header("cookie", cookie)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    async fn get(url: &str, cookie: &str) -> (u16, serde_json::Value) {
        let r = client()
            .get(format!("{url}/_door/people"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or_default())
    }

    /// bob signs in with his device, as someone not in
    async fn stranger(url: &str, name: &str) -> String {
        let (e, k) = signed_entry(name);
        let n: serde_json::Value = client()
            .get(format!("{url}/_door/challenge"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let n = n["nonce"].as_str().unwrap();
        let r = client()
            .post(format!("{url}/_door/signin"))
            .json(&serde_json::json!({"name":name,"entry":e,"device":e.entry.root,"sig":device_sig(&k,NAME,n,name),"nonce":n}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        r.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn asking_letting_in_and_removing() {
        let r = rig("people");
        let (_, _, ann) = person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;
        let bob = stranger(&url, "bob").await;

        // bob outside sees only himself, and may not let anyone in
        let (s, v) = get(&url, &bob).await;
        assert_eq!(s, 200);
        assert_eq!(v["me"]["in"], false);
        assert!(v.get("people").is_none());
        assert_eq!(
            post(
                &url,
                "/_door/let-in",
                &bob,
                serde_json::json!({"name":"bob"})
            )
            .await,
            403
        );

        assert_eq!(
            post(&url, "/_door/ask", &bob, serde_json::json!({})).await,
            204
        );
        let (_, v) = get(&url, &bob).await;
        assert_eq!(v["me"]["asked"], true);
        let (_, v) = get(&url, &ann).await;
        assert_eq!(v["asked"][0]["name"], "bob");

        // nobody asked by that name; ann already in
        assert_eq!(
            post(
                &url,
                "/_door/let-in",
                &ann,
                serde_json::json!({"name":"cy"})
            )
            .await,
            404
        );
        assert_eq!(
            post(
                &url,
                "/_door/let-in",
                &ann,
                serde_json::json!({"name":"ann"})
            )
            .await,
            409
        );

        assert_eq!(
            post(
                &url,
                "/_door/let-in",
                &ann,
                serde_json::json!({"name":"bob"})
            )
            .await,
            200
        );
        // his user, in the group configured, and he is in on the session he has
        assert!(
            r.ha.made
                .lock()
                .unwrap()
                .contains(&("bob".into(), "system-admin".into()))
        );
        let (_, v) = get(&url, &bob).await;
        assert_eq!(v["me"]["in"], true);
        let yaml = std::fs::read_to_string(&r.app.cfg.users_file).unwrap();
        assert!(yaml.contains("127.0.1.2"), "{yaml}");
        let restarts = r.ha.restarts.load(Ordering::SeqCst);

        // bob takes ann out: her session goes, her address leaves the file
        assert_eq!(
            post(
                &url,
                "/_door/remove",
                &bob,
                serde_json::json!({"name":"ann"})
            )
            .await,
            204
        );
        assert_eq!(get(&url, &ann).await.0, 401);
        let yaml = std::fs::read_to_string(&r.app.cfg.users_file).unwrap();
        assert!(!yaml.contains("127.0.1.1\""), "{yaml}");
        assert_eq!(r.ha.restarts.load(Ordering::SeqCst), restarts + 1);

        // and the last one in stays
        assert_eq!(
            post(
                &url,
                "/_door/remove",
                &bob,
                serde_json::json!({"name":"bob"})
            )
            .await,
            409
        );
        assert_eq!(
            post(
                &url,
                "/_door/remove",
                &bob,
                serde_json::json!({"name":"zed"})
            )
            .await,
            404
        );
    }

    #[tokio::test]
    async fn someone_signed_out_or_from_elsewhere_changes_nothing() {
        let r = rig("people-no");
        let (_, _, ann) = person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;
        assert_eq!(get(&url, "door=nope").await.0, 401);
        assert_eq!(
            post(&url, "/_door/ask", "door=nope", serde_json::json!({})).await,
            401
        );
        assert_eq!(
            post(&url, "/_door/let-in", "", serde_json::json!({"name":"x"})).await,
            401
        );
        assert_eq!(
            post(&url, "/_door/ask", &ann, serde_json::json!({})).await,
            409
        );
        let s = client()
            .post(format!("{url}/_door/remove"))
            .header("cookie", &ann)
            .header("origin", "https://evil.example")
            .json(&serde_json::json!({"name":"ann"}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(s, 403);
    }

    #[tokio::test]
    async fn a_let_in_while_home_assistant_is_down_is_finished_later() {
        let r = rig("people-later");
        let (_, _, ann) = person(&r.app, "ann");
        let url = serve(crate::router(r.app.clone())).await;
        let bob = stranger(&url, "bob").await;
        assert_eq!(
            post(&url, "/_door/ask", &bob, serde_json::json!({})).await,
            204
        );
        r.ha.down.store(true, Ordering::SeqCst);
        assert_eq!(
            post(
                &url,
                "/_door/let-in",
                &ann,
                serde_json::json!({"name":"bob"})
            )
            .await,
            202
        );
        let (_, v) = get(&url, &ann).await;
        assert_eq!(v["people"][1]["name"], "bob");
        assert_eq!(v["people"][1]["ready"], false);
    }
}
