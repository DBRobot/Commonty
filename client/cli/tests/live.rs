//! A guest walks the real fleet. Not part of `cargo test` unless pointed at
//! it: DD_LIVE_BASE is the domain (commonty.org) and
//! DD_LIVE_CODE an invite code minted moments ago with `dd invite`. A
//! software passkey makes an account with the code, then opens every tile
//! the way a browser would and reports what each service did with a person
//! it has never seen. Run after a release:
//!
//!   DD_LIVE_BASE=... DD_LIVE_CODE=$(dd invite | sed -n 's/^code: //p') \
//!     cargo test --test live -- --nocapture

use std::time::Duration;

use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap()
}

/// What the browser sends for a code: the code key's proof for this root.
fn grant_header(code: &str, root: &str) -> String {
    use base64::Engine as _;
    let key = identity::code_key(code);
    let pk = identity::encode_public(&key.verifying_key());
    let redeemed = identity::now();
    let proof = identity::prove(&key, &pk, root, redeemed);
    base64::engine::general_purpose::STANDARD.encode(
        serde_json::json!({ "invite_public_key": pk, "redeemed": redeemed, "proof": proof })
            .to_string(),
    )
}

/// Follow redirects by hand so the cookie can ride along to every
/// subdomain; reqwest's own store would not send it across hosts.
async fn walk(http: &reqwest::Client, cookie: &str, start: &str) -> (Vec<String>, u16, String) {
    let mut url = Url::parse(start).unwrap();
    let mut hops = vec![];
    for _ in 0..12 {
        let r = http
            .get(url.clone())
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        hops.push(format!("{status} {}", url));
        if let Some(l) = r.headers().get("location").and_then(|v| v.to_str().ok()) {
            url = url.join(l).unwrap();
            continue;
        }
        return (hops, status, r.text().await.unwrap_or_default());
    }
    (hops, 0, "too many redirects".into())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guest_walks_every_tile() {
    let (Ok(base), Ok(code)) = (std::env::var("DD_LIVE_BASE"), std::env::var("DD_LIVE_CODE"))
    else {
        eprintln!("DD_LIVE_BASE and DD_LIVE_CODE not set: not walking the fleet");
        return;
    };
    let home = format!("https://home.{base}");
    let origin = Url::parse(&home).unwrap();
    // guest: reserved for probes, dropped by every box a quarter hour after
    // its last update, so the same name is free again next time
    let name = std::env::var("DD_LIVE_USER").unwrap_or_else(|_| "guest".into());
    let http = client();
    let mut key = SoftPasskey::new(true);

    // the join page, as the browser does it
    let r = http
        .post(format!("{home}/_dd/join/start"))
        .header("x-dd-probe", "1")
        .json(&serde_json::json!({ "username": name }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "join start: {}",
        r.text().await.unwrap_or_default()
    );
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let ccr: CreationChallengeResponse = serde_json::from_value(j).unwrap();
    let reg = key.do_registration(origin.clone(), ccr).unwrap();
    let root = format!("webauthn:{}", reg.id);
    let r = http
        .post(format!("{home}/_dd/join/finish"))
        .header("x-dd-ceremony", &ceremony)
        .header("x-dd-grant", grant_header(&code, &root))
        .json(&reg)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "join finish: {}",
        r.text().await.unwrap_or_default()
    );
    let mut j: serde_json::Value = r.json().await.unwrap();
    let ceremony = j["ceremony"].as_str().unwrap().to_string();
    j.as_object_mut().unwrap().remove("ceremony");
    let rcr: RequestChallengeResponse = serde_json::from_value(j).unwrap();
    let cred = key.do_authentication(origin.clone(), rcr).unwrap();
    let r = http
        .post(format!("{home}/_dd/join/sign"))
        .header("x-dd-ceremony", &ceremony)
        .json(&cred)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "join sign: {}",
        r.text().await.unwrap_or_default()
    );
    let cookie = r
        .headers()
        .get("set-cookie")
        .expect("a session")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    println!("account {name} made with the code");

    // home: the tiles, not the waiting page
    let (_, st, body) = walk(&http, &cookie, &format!("{home}/_dd/home")).await;
    assert_eq!(st, 200);
    assert!(
        body.contains("Your services"),
        "home page is not the tiles:\n{body}"
    );
    println!("home: tiles");

    let mut failed = vec![];
    let mut report = |tile: &str, ok: bool, detail: String| {
        println!("{}  {tile}: {detail}", if ok { "ok " } else { "BAD" });
        if !ok {
            failed.push(tile.to_string());
        }
    };

    // Metrics: grafana takes the verifier's username header and makes a viewer
    let (hops, st, body) = walk(&http, &cookie, &format!("https://grafana.{base}/api/user")).await;
    let login = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["login"].as_str().map(String::from));
    report(
        "Metrics",
        st == 200 && login.as_deref() == Some(name.as_str()),
        format!("{st} as {login:?}  {}", hops.join(" -> ")),
    );

    // Files: the dav share answers the guest at all
    let r = http
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
            format!("https://files.{base}/"),
        )
        .header("cookie", &cookie)
        .header("depth", "1")
        .send()
        .await
        .unwrap();
    let st = r.status().as_u16();
    let body = r.text().await.unwrap_or_default();
    let n = body.matches("<D:href>").count() + body.matches("<d:href>").count();
    report("Files", st == 207, format!("{st}, {n} entries listed"));

    // Chat: the gateway lets the guest in
    let (hops, st, body) = walk(&http, &cookie, &format!("https://llm.{base}/")).await;
    if let Ok(dir) = std::env::var("DD_LIVE_DUMP") {
        std::fs::write(format!("{dir}/chat.html"), &body).unwrap();
        // what the page's own first requests get
        for path in ["/props", "/v1/models", "/index.html"] {
            let (_, st, b) = walk(&http, &cookie, &format!("https://llm.{base}{path}")).await;
            std::fs::write(
                format!("{dir}/chat{}.txt", path.replace('/', "_")),
                format!("{st}\n{b}"),
            )
            .unwrap();
        }
    }
    report(
        "Chat",
        st == 200 && !body.contains("/_dd/login"),
        format!("{st}  {}", hops.join(" -> ")),
    );

    // Code: forgejo takes the header and registers the guest
    let (hops, st, body) = walk(&http, &cookie, &format!("https://git.{base}/api/v1/user")).await;
    let login = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["login"].as_str().map(String::from));
    report(
        "Code",
        st == 200 && login.as_deref() == Some(name.as_str()),
        format!("{st} as {login:?}  {}", hops.join(" -> ")),
    );

    // Movies & TV: our page and player, behind the same gate
    let (hops, st, body) = walk(&http, &cookie, &format!("https://files.{base}/_dd/media")).await;
    report(
        "Videos",
        st == 200 && body.contains("Movies"),
        format!("{st} ({} bytes)  {}", body.len(), hops.join(" -> ")),
    );

    // Photos: our page, which asks the passkey for the account's secret; a
    // software passkey has no PRF, so the walk stops at the page itself
    let (hops, st, body) = walk(&http, &cookie, &format!("https://photos.{base}/_dd/photos")).await;
    report(
        "Photos",
        st == 200 && body.contains("/_dd/web/dd_web.js"),
        format!("{st}, the passkey page  {}", hops.join(" -> ")),
    );
    let (_, st, _) = walk(
        &http,
        &cookie,
        &format!("https://photos.{base}/_dd/web/dd_web.js"),
    )
    .await;
    report(
        "Photos wasm",
        st == 200,
        format!("{st} for the browser-side rust"),
    );

    assert!(failed.is_empty(), "tiles that failed the guest: {failed:?}");
}
