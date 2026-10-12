//! A signed-in person to Home Assistant, from their own loopback address
//! (127.0.1.<n>), which its trusted networks sign in as their user. Nothing
//! the request says about where it came from is passed on, nor the door's
//! cookie; Home Assistant's pages get the Commonty bar.

use crate::App;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hyper_util::rt::TokioIo;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

/// the largest page of Home Assistant's the door reads whole to add the bar
const MAX_PAGE: usize = 4 << 20;

fn wants_page(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

pub async fn handle(State(app): State<Arc<App>>, req: Request) -> Response {
    let page = wants_page(req.headers());
    let address = match app.who(req.headers()) {
        None if page => return app.assets.page_response("signin", StatusCode::OK),
        None => return StatusCode::UNAUTHORIZED.into_response(),
        // signed in, not let in yet: the page where they ask
        Some((_, false)) if page => return app.assets.page_response("people", StatusCode::OK),
        Some((_, false)) => return StatusCode::FORBIDDEN.into_response(),
        Some((name, true)) => app.with_store(|s| {
            s.state
                .people
                .get(&name)
                .filter(|p| p.ha_user.is_some())
                .and_then(|p| p.address)
        }),
    };
    let Some(n) = address else {
        // let in, their Home Assistant user not made yet
        return app
            .assets
            .page_response("people", StatusCode::SERVICE_UNAVAILABLE);
    };
    match forward(&app, req, n, page).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("proxy: {e:#}");
            (StatusCode::BAD_GATEWAY, "Home Assistant is not answering").into_response()
        }
    }
}

/// The headers that describe one hop, not the request.
const HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// What a client may claim about where a request came from.
fn forwarding(name: &str) -> bool {
    name == "forwarded"
        || name.starts_with("x-forwarded-")
        || matches!(
            name,
            "x-real-ip" | "client-ip" | "true-client-ip" | "cf-connecting-ip" | "via"
        )
}

pub fn clean_request(h: &mut HeaderMap, upgrade: bool, page: bool) {
    let named: Vec<String> = h
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    let upgrade_to = h.get(header::UPGRADE).cloned();
    let drop: Vec<header::HeaderName> = h
        .keys()
        .filter(|k| {
            let k = k.as_str();
            HOP.contains(&k) || forwarding(k) || named.iter().any(|n| n == k)
        })
        .cloned()
        .collect();
    for k in drop {
        h.remove(k);
    }
    if upgrade && let Some(u) = upgrade_to {
        h.insert(header::UPGRADE, u);
        h.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    }
    // the door's cookie is the door's; Home Assistant's own stay
    let rest: Vec<String> = h
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|kv| !kv.is_empty() && kv.split('=').next() != Some(crate::COOKIE))
        .map(str::to_string)
        .collect();
    h.remove(header::COOKIE);
    if !rest.is_empty()
        && let Ok(v) = HeaderValue::from_str(&rest.join("; "))
    {
        h.insert(header::COOKIE, v);
    }
    // a page comes back plain, so the bar can go in; Home Assistant's
    // scripts and the rest keep their compression
    if page {
        h.remove(header::ACCEPT_ENCODING);
    }
}

fn is_upgrade(h: &HeaderMap) -> bool {
    h.contains_key(header::UPGRADE)
        && h.get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.to_ascii_lowercase().contains("upgrade"))
}

/// The bar, just inside <body>: bar.css touches nothing outside its own
/// header, so Home Assistant's page is left as it was.
pub fn add_bar(html: &[u8], bar: &[u8]) -> Option<Vec<u8>> {
    let lower = html.to_ascii_lowercase();
    let at = lower.windows(5).position(|w| w == b"<body")?;
    let end = at + lower[at..].iter().position(|b| *b == b'>')? + 1;
    let mut out = Vec::with_capacity(html.len() + bar.len());
    out.extend_from_slice(&html[..end]);
    out.extend_from_slice(bar);
    out.extend_from_slice(&html[end..]);
    Some(out)
}

async fn forward(app: &App, mut req: Request, n: u8, page: bool) -> anyhow::Result<Response> {
    let upgrade = is_upgrade(req.headers());
    let client_side = upgrade.then(|| hyper::upgrade::on(&mut req));
    let (mut parts, body) = req.into_parts();
    clean_request(&mut parts.headers, upgrade, page);
    parts.uri = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/")
        .parse()?;
    let req = Request::from_parts(parts, body);

    let sock = tokio::net::TcpSocket::new_v4()?;
    sock.bind(SocketAddr::new(Ipv4Addr::new(127, 0, 1, n).into(), 0))?;
    let stream = sock.connect(app.cfg.ha).await?;
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            eprintln!("proxy: connection: {e}");
        }
    });
    let mut resp = send.send_request(req).await?;

    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        let Some(client_side) = client_side else {
            anyhow::bail!("an upgrade nobody asked for");
        };
        let ha_side = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            match tokio::try_join!(client_side, ha_side) {
                Ok((c, h)) => {
                    let _ =
                        tokio::io::copy_bidirectional(&mut TokioIo::new(c), &mut TokioIo::new(h))
                            .await;
                }
                Err(e) => eprintln!("proxy: upgrade: {e}"),
            }
        });
        let (parts, _) = resp.into_parts();
        return Ok(Response::from_parts(parts, Body::empty()));
    }

    let (mut parts, body) = resp.into_parts();
    for k in HOP {
        parts.headers.remove(*k);
    }
    let html = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let encoded = parts.headers.contains_key(header::CONTENT_ENCODING);
    if !(page && html && !encoded) {
        return Ok(Response::from_parts(parts, Body::new(body)));
    }
    let bytes = axum::body::to_bytes(Body::new(body), MAX_PAGE).await?;
    let out = add_bar(&bytes, app.assets.page("ha-bar")).unwrap_or_else(|| bytes.to_vec());
    parts.headers.remove(header::CONTENT_LENGTH);
    Ok(Response::from_parts(parts, Body::from(out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;
    use axum::extract::ConnectInfo;
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

    /// Home Assistant as the proxy sees it: who connected, what arrived.
    async fn upstream(ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
        if req.headers().contains_key(header::UPGRADE) {
            let on = hyper::upgrade::on(&mut req);
            tokio::spawn(async move {
                let mut io = TokioIo::new(on.await.unwrap());
                let mut b = [0u8; 4];
                io.read_exact(&mut b).await.unwrap();
                let reply = format!("{} {}", peer.ip(), String::from_utf8_lossy(&b));
                io.write_all(reply.as_bytes()).await.unwrap();
            });
            return Response::builder()
                .status(StatusCode::SWITCHING_PROTOCOLS)
                .header(header::UPGRADE, "websocket")
                .header(header::CONNECTION, "upgrade")
                .body(Body::empty())
                .unwrap();
        }
        if req.uri().path() == "/" {
            return (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                "<!doctype html><html><head></head><BODY class=x><p>home</p></body></html>",
            )
                .into_response();
        }
        let headers: serde_json::Map<String, serde_json::Value> = req
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().into()))
            .collect();
        axum::Json(serde_json::json!({
            "peer": peer.ip().to_string(),
            "uri": req.uri().to_string(),
            "headers": headers,
        }))
        .into_response()
    }

    async fn setup(tag: &str) -> (Rig, String, String) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ha = l.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                l,
                axum::Router::new()
                    .fallback(upstream)
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let r = rig_with(tag, |c| c.ha = ha);
        let (_, _, ann) = person(&r.app, "ann");
        r.app.reconcile_ha().await.unwrap();
        let url = serve(crate::router(r.app.clone())).await;
        (r, url, ann)
    }

    #[tokio::test]
    async fn a_person_reaches_home_assistant_from_their_own_address_and_nothing_else() {
        let (_r, url, ann) = setup("proxy").await;
        let v: serde_json::Value = client()
            .get(format!("{url}/api/states?x=1"))
            .header("cookie", format!("a=1; {ann}; b=2"))
            .header("x-forwarded-for", "8.8.8.8")
            .header("forwarded", "for=8.8.8.8")
            .header("x-real-ip", "8.8.8.8")
            .header("connection", "x-secret")
            .header("x-secret", "hop")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(v["peer"], "127.0.1.1");
        assert_eq!(v["uri"], "/api/states?x=1");
        let h = v["headers"].as_object().unwrap();
        assert_eq!(
            h["cookie"], "a=1; b=2",
            "the door's cookie stays at the door"
        );
        for k in ["x-forwarded-for", "forwarded", "x-real-ip", "x-secret"] {
            assert!(!h.contains_key(k), "{k} was passed on");
        }
    }

    #[tokio::test]
    async fn home_assistants_pages_get_the_bar() {
        let (r, url, ann) = setup("proxy-bar").await;
        let page = client()
            .get(format!("{url}/"))
            .header("cookie", &ann)
            .header("accept", "text/html")
            .header("accept-encoding", "gzip")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let bar = String::from_utf8(r.app.assets.page("ha-bar").to_vec()).unwrap();
        assert!(
            page.contains(&format!("<BODY class=x>{bar}<p>home</p>")),
            "{page}"
        );
        // not a page: not touched
        let v: serde_json::Value = client()
            .get(format!("{url}/api/x"))
            .header("cookie", &ann)
            .header("accept-encoding", "gzip")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(v["headers"]["accept-encoding"], "gzip");
    }

    #[tokio::test]
    async fn the_websocket_is_carried_through() {
        let (_r, url, ann) = setup("proxy-ws").await;
        let addr = url.trim_start_matches("http://");
        let mut c = tokio::io::BufReader::new(tokio::net::TcpStream::connect(addr).await.unwrap());
        c.get_mut()
            .write_all(
                format!("GET /api/websocket HTTP/1.1\r\nHost: {addr}\r\nCookie: {ann}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").as_bytes(), // gitleaks:allow (RFC 6455's sample nonce)
            )
            .await
            .unwrap();
        let mut status = String::new();
        c.read_line(&mut status).await.unwrap();
        assert!(status.starts_with("HTTP/1.1 101"), "{status}");
        loop {
            let mut l = String::new();
            c.read_line(&mut l).await.unwrap();
            if l == "\r\n" {
                break;
            }
        }
        c.get_mut().write_all(b"ping").await.unwrap();
        let mut back = vec![0u8; "127.0.1.1 ping".len()];
        c.read_exact(&mut back).await.unwrap();
        assert_eq!(back, b"127.0.1.1 ping");
    }

    #[tokio::test]
    async fn nobody_else_gets_through() {
        let (r, url, _) = setup("proxy-no").await;
        let get = |cookie: Option<String>, accept: &str| {
            let mut q = client()
                .get(format!("{url}/api/states"))
                .header("accept", accept);
            if let Some(c) = cookie {
                q = q.header("cookie", c);
            }
            async move {
                let res = q.send().await.unwrap();
                (res.status().as_u16(), res.text().await.unwrap())
            }
        };
        let signin = String::from_utf8(r.app.assets.page("signin").to_vec()).unwrap();
        let people = String::from_utf8(r.app.assets.page("people").to_vec()).unwrap();
        assert_eq!(get(None, "text/html").await, (200, signin));
        assert_eq!(get(None, "*/*").await.0, 401);
        assert_eq!(get(Some("door=forged".into()), "*/*").await.0, 401);

        // signed in but not let in: the page to ask from, never Home Assistant
        let cookie = crate::random_b64url(32);
        r.app.with_store(|s| {
            s.state.sessions.insert(
                crate::state::hash(&cookie),
                crate::state::Session {
                    name: "bob".into(),
                    root: "x".into(),
                    expires: identity::now() + 60,
                    entry: None,
                },
            );
        });
        assert_eq!(
            get(Some(format!("door={cookie}")), "text/html").await,
            (200, people)
        );
        assert_eq!(get(Some(format!("door={cookie}")), "*/*").await.0, 403);

        // a session from before ann's root changed is not ann
        r.app.with_store(|s| {
            for sess in s.state.sessions.values_mut() {
                sess.name = "ann".into();
            }
        });
        assert_eq!(get(Some(format!("door={cookie}")), "*/*").await.0, 403);
    }

    #[test]
    fn the_bar_goes_inside_body_or_nowhere() {
        assert_eq!(
            add_bar(b"<html><body id=a>x</body>", b"BAR").unwrap(),
            b"<html><body id=a>BARx</body>"
        );
        assert!(add_bar(b"{\"json\":1}", b"BAR").is_none());
    }
}
