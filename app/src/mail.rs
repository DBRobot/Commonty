//! Mail in this app (docs/dev/mail.md): the member's mail key, which only
//! their devices hold, handed to the app's own mail page; the device's
//! signature on what that page asks of the mail Worker; and sending, from
//! here: the Worker signs the headers (DKIM), and this device runs SMTP and
//! TLS to the other side's server through the box's pipe, which carries the
//! bytes and cannot read them.

use std::sync::Arc;

use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use sha2::Digest as _;
use tauri::{AppHandle, Manager as _, Runtime};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const WHAT: &str = "mail";
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const B64U: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

type Fail = (u16, String);
fn bad(e: impl std::fmt::Display) -> Fail {
    (502, e.to_string())
}

struct Me {
    opener: media::gate::Opener,
    user: String,
    token: String,
    files: String,
    mail: String,
}

async fn me<R: Runtime>(app: &AppHandle<R>) -> Result<Me, Fail> {
    crate::local::network_ready().await;
    let keys = app.state::<crate::account::Keys>();
    let (opener, user, token) = media::gate::Opener::load(&keys.0).map_err(bad)?;
    let files = media::gate::files_base(&crate::account::dirs()).map_err(bad)?;
    let mail = files.replacen("://files.", "://mail.", 1);
    Ok(Me {
        opener,
        user,
        token,
        files,
        mail,
    })
}

/// GET /_dd/app/mail/key (create = false) or POST (create = true): the
/// member's mail key, `{ikm, device: {fp, key}}`. Made the first time on
/// this device, sealed to every key of the member's entry this device may
/// seal to (account::seal_for_keys) and kept on the box, which cannot open it.
pub(crate) async fn key<R: Runtime>(
    app: &AppHandle<R>,
    create: bool,
) -> Result<serde_json::Value, Fail> {
    let m = me(app).await?;
    let keys = app.state::<crate::account::Keys>();
    let entry = account::own_entry(&keys.0, &crate::account::dirs(), &m.user)
        .await
        .map_err(|e| bad(format!("{e:#}")))?;
    let http = crate::local::http().map_err(bad)?;
    let at = format!("{}/_dd/mail/o/key/doc", m.files);
    let r = crate::local::send(http.get(&at).bearer_auth(&m.token))
        .await
        .map_err(|e| bad(crate::local::why(&e)))?;
    let doc: serde_json::Value = match r.status().as_u16() {
        200 => serde_json::from_slice(&r.bytes().await.map_err(bad)?).map_err(bad)?,
        404 if create => {
            let mut secret = zeroize::Zeroizing::new([0u8; 32]);
            getrandom::fill(&mut secret[..]).map_err(bad)?;
            let doc = account::seal_for_keys(&entry.entry, m.opener.device().1, WHAT, &secret[..])
                .map_err(|e| bad(format!("{e:#}")))?;
            let put = http
                .put(&at)
                .bearer_auth(&m.token)
                .header("content-type", "application/octet-stream")
                .body(serde_json::to_vec(&doc).map_err(bad)?);
            let r = crate::local::send(put)
                .await
                .map_err(|e| bad(crate::local::why(&e)))?;
            if !r.status().is_success() {
                return Err((
                    r.status().as_u16(),
                    format!("the box did not keep your mail key: {}", r.status()),
                ));
            }
            doc
        }
        404 => return Err((404, "no mail key yet".into())),
        s => return Err((s, format!("the box said {s}"))),
    };
    let k = account::open_for_device(
        &doc,
        &entry.entry,
        WHAT,
        m.opener.device(),
        m.opener.root.as_ref(),
    )
    .map_err(|e| {
        (
            403,
            format!("your mail key does not open on this device: {e:#}"),
        )
    })?;
    // a device added since, or a paper key: sealed to it too, so it opens
    // there without this device (the same rule as the libraries')
    let have: Vec<String> = doc["keys"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    let (fp, _) = m.opener.device();
    if account::due_for_keys(&entry.entry, fp)
        .iter()
        .any(|l| !have.contains(l))
    {
        match account::seal_for_keys(&entry.entry, m.opener.device().1, WHAT, &k[..]) {
            Ok(again) => {
                let put = http
                    .put(&at)
                    .bearer_auth(&m.token)
                    .header("content-type", "application/octet-stream")
                    .body(serde_json::to_vec(&again).map_err(bad)?);
                if let Err(e) = crate::local::send(put).await {
                    eprintln!(
                        "mail: sealing the key for your other devices: {}",
                        crate::local::why(&e)
                    );
                }
            }
            Err(e) => eprintln!("mail: sealing the key for your other devices: {e:#}"),
        }
    }
    use ed25519_dalek::Signer as _;
    let (fp, dev) = m.opener.device();
    let pk = B64U.encode(public_of(&k[..]));
    let sig = dev.sign(format!("commonty mail key v1\0{}\0{pk}", m.user).as_bytes());
    Ok(serde_json::json!({
        "ikm": B64U.encode(&k[..]),
        "pk": pk,
        "sig": B64.encode(sig.to_bytes()),
        "device": { "fp": fp, "key": B64.encode(dev.verifying_key().to_bytes()) },
        "name": m.user,
    }))
}

/// The mail key's public half from its 32 bytes, as the page's HPKE makes it
/// (RFC 9180 DeriveKeyPair for DHKEM(X25519, HKDF-SHA256)): what this device
/// signs, so the Worker locks mail to its key and no other.
fn public_of(ikm: &[u8]) -> [u8; 32] {
    use hmac::Mac as _;
    type H = hmac::Hmac<sha2::Sha256>;
    let suite = b"KEM\x00\x20";
    let mut x = <H as hmac::Mac>::new_from_slice(&[0u8; 32]).expect("hmac");
    x.update(b"HPKE-v1");
    x.update(suite);
    x.update(b"dkp_prk");
    x.update(ikm);
    let prk = x.finalize().into_bytes();
    let mut e = <H as hmac::Mac>::new_from_slice(&prk).expect("hmac");
    e.update(&32u16.to_be_bytes());
    e.update(b"HPKE-v1");
    e.update(suite);
    e.update(b"sk");
    e.update(&[1]);
    let mut sk = [0u8; 32];
    sk.copy_from_slice(&e.finalize().into_bytes()[..32]);
    let pk = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(sk));
    zeroize::Zeroize::zeroize(&mut sk);
    pk.to_bytes()
}

fn device_headers(
    user: &str,
    fp: &str,
    key: &ed25519_dalek::SigningKey,
    method: &str,
    path: &str,
    body: &[u8],
) -> Vec<(&'static str, String)> {
    use ed25519_dalek::Signer as _;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let digest: String = sha2::Sha256::digest(body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let msg = format!("commonty mail device v1\0{user}\0{fp}\0{at}\0{method}\0{path}\0{digest}");
    vec![
        ("x-dd-name", user.to_string()),
        ("x-dd-device", fp.to_string()),
        ("x-dd-at", at),
        ("x-dd-sig", B64.encode(key.sign(msg.as_bytes()).to_bytes())),
    ]
}

/// a request to the mail Worker, signed by this device (inbox.js device)
async fn worker(
    m: &Me,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<(u16, serde_json::Value), Fail> {
    let (fp, key) = m.opener.device();
    let http = crate::local::http().map_err(bad)?;
    let url = format!("{}{path}", m.mail);
    let mut req = if method == "POST" {
        http.post(&url)
            .header("content-type", "application/json")
            .body(body.to_vec())
    } else {
        http.get(&url)
    };
    for (k, v) in device_headers(&m.user, fp, key, method, path, body) {
        req = req.header(k, v);
    }
    let r = req.send().await.map_err(|e| bad(crate::local::why(&e)))?;
    let s = r.status().as_u16();
    Ok((s, r.json().await.unwrap_or_default()))
}

/// POST /_dd/app/mail/worker {method, path, body}: the page's question to
/// the mail Worker, signed by this device. Only the routes a device asks.
pub(crate) async fn ask<R: Runtime>(
    app: &AppHandle<R>,
    body: &[u8],
) -> Result<serde_json::Value, Fail> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| (400, e.to_string()))?;
    let path = v["path"].as_str().unwrap_or("");
    let method = v["method"].as_str().unwrap_or("GET");
    if !((path.starts_with("/api/mailkey/") || path == "/api/addresses") && method == "GET") {
        return Err((400, "not a question for the Worker".into()));
    }
    let m = me(app).await?;
    let (s, j) = worker(&m, method, path, b"").await?;
    if s != 200 {
        return Err((
            s,
            j["error"]
                .as_str()
                .unwrap_or("the mail Worker said no")
                .to_string(),
        ));
    }
    Ok(j)
}

/// POST /_dd/app/mail/vouch {id, hs, hr}: this device's signature on mail
/// it locked for another member (mail-object.js, by "member"): the hashes
/// of what it sealed, under this member's name and this device's, which
/// this route puts in itself - a page cannot have it sign as anyone else
pub(crate) async fn vouch<R: Runtime>(
    app: &AppHandle<R>,
    body: &[u8],
) -> Result<serde_json::Value, Fail> {
    use ed25519_dalek::Signer as _;
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| (400, e.to_string()))?;
    let field = |k: &str, len: usize| -> Result<String, Fail> {
        let x = v[k].as_str().unwrap_or("");
        if x.len() == len
            && x.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            Ok(x.to_string())
        } else {
            Err((400, format!("not a {k}")))
        }
    };
    let (id, hs, hr) = (field("id", 21)?, field("hs", 43)?, field("hr", 43)?);
    let m = me(app).await?;
    let (fp, key) = m.opener.device();
    let msg = format!(
        "commonty mail object v1\0in\0{id}\0{hs}\0{hr}\0{}\0{fp}",
        m.user
    );
    Ok(
        serde_json::json!({ "name": m.user, "fp": fp, "sig": B64.encode(key.sign(msg.as_bytes()).to_bytes()) }),
    )
}

/// the MX hosts of a domain, best first, or the domain itself (RFC 5321)
async fn mx(http: &reqwest::Client, domain: &str) -> Result<Vec<String>, Fail> {
    let v: serde_json::Value = http
        .get("https://cloudflare-dns.com/dns-query")
        .query(&[("name", domain), ("type", "MX")])
        .header("accept", "application/dns-json")
        .send()
        .await
        .map_err(|e| bad(crate::local::why(&e)))?
        .json()
        .await
        .map_err(bad)?;
    let mut hosts: Vec<(u64, String)> = v["Answer"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|r| r["type"].as_u64() == Some(15))
                .filter_map(|r| {
                    let mut it = r["data"].as_str()?.split_whitespace();
                    let p = it.next()?.parse().ok()?;
                    Some((p, it.next()?.trim_end_matches('.').to_ascii_lowercase()))
                })
                .collect()
        })
        .unwrap_or_default();
    hosts.sort();
    // a null MX (RFC 7505): the domain takes no mail at all
    if hosts.iter().any(|(_, h)| h.is_empty()) {
        return Err((400, format!("{domain} takes no mail")));
    }
    Ok(if hosts.is_empty() {
        vec![domain.to_string()]
    } else {
        hosts.into_iter().map(|(_, h)| h).collect()
    })
}

/// the box's pipe to `host`:25 as a stream: a WebSocket over the same way
/// this app reaches the box, its frames turned back into bytes
async fn pipe(m: &Me, domain: &str, host: &str) -> Result<tokio::io::DuplexStream, Fail> {
    let http = crate::local::http().map_err(bad)?;
    let key = {
        let mut k = [0u8; 16];
        getrandom::fill(&mut k).map_err(bad)?;
        B64.encode(k)
    };
    let r = http
        .get(format!("{}/_dd/mail/pipe", m.files))
        .query(&[("domain", domain), ("host", host)])
        .version(reqwest::Version::HTTP_11)
        .bearer_auth(&m.token)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", key)
        .send()
        .await
        .map_err(|e| bad(crate::local::why(&e)))?;
    if r.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
        let s = r.status().as_u16();
        let why = r.text().await.unwrap_or_default();
        return Err((
            s,
            if why.is_empty() {
                format!("the box would not open the way out ({s})")
            } else {
                why
            },
        ));
    }
    let raw = r.upgrade().await.map_err(bad)?;
    let ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
        raw,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    let (mine, theirs) = tokio::io::duplex(64 << 10);
    tokio::spawn(async move {
        let (mut tx, mut rx) = ws.split();
        let (mut rd, mut wr) = tokio::io::split(theirs);
        let up = async {
            let mut buf = vec![0u8; 16 << 10];
            loop {
                match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx
                            .send(tokio_tungstenite::tungstenite::Message::Binary(
                                buf[..n].to_vec().into(),
                            ))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            let _ = tx.close().await;
        };
        let down = async {
            while let Some(Ok(msg)) = rx.next().await {
                let b = match msg {
                    tokio_tungstenite::tungstenite::Message::Binary(b) => b.to_vec(),
                    tokio_tungstenite::tungstenite::Message::Text(t) => t.as_bytes().to_vec(),
                    tokio_tungstenite::tungstenite::Message::Close(_) => break,
                    _ => continue,
                };
                if wr.write_all(&b).await.is_err() {
                    break;
                }
            }
            let _ = wr.shutdown().await;
        };
        tokio::join!(up, down);
    });
    Ok(mine)
}

/// POST /_dd/app/mail/send {from, to: [..], raw}: the page's message, sent
/// from here to each recipient's own server. What became of each one.
pub(crate) async fn send<R: Runtime>(
    app: &AppHandle<R>,
    body: &[u8],
) -> Result<serde_json::Value, Fail> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| (400, e.to_string()))?;
    let from = v["from"].as_str().unwrap_or("").to_string();
    let raw = B64U
        .decode(v["raw"].as_str().unwrap_or(""))
        .map_err(|e| (400, e.to_string()))?;
    let to: Vec<String> = v["to"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_ascii_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    if to.is_empty() || to.len() > 100 || raw.len() > 25 << 20 {
        return Err((400, "one to a hundred recipients, and at most 25 MB".into()));
    }
    let m = me(app).await?;
    let (hs, bh) = mailsend::dkim_request(&raw);
    let ask = serde_json::to_vec(&serde_json::json!({ "headers": hs.iter().map(|(n, v)| [n, v]).collect::<Vec<_>>(), "bh": bh })).map_err(bad)?;
    let (s, j) = worker(&m, "POST", "/api/dkim", &ask).await?;
    if s != 200 {
        return Err((
            s,
            j["error"]
                .as_str()
                .unwrap_or("the mail Worker would not sign it")
                .to_string(),
        ));
    }
    let msg = mailsend::signed(&raw, j["header"].as_str().unwrap_or(""));
    let helo = m
        .mail
        .trim_start_matches("https://mail.")
        .trim_end_matches('/')
        .to_string();
    let http = crate::local::http().map_err(bad)?;
    let mut by_domain: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for r in &to {
        let d = r
            .rsplit_once('@')
            .map(|(_, d)| d.to_string())
            .unwrap_or_default();
        by_domain.entry(d).or_default().push(r.clone());
    }
    let roots = mailsend::web_roots();
    let mut out = Vec::new();
    for (domain, rcpts) in by_domain {
        let hosts = match mx(&http, &domain).await {
            Ok(h) => h,
            Err((_, why)) => {
                out.extend(
                    rcpts
                        .iter()
                        .map(|r| serde_json::json!({ "to": r, "ok": false, "why": why })),
                );
                continue;
            }
        };
        let mut last = String::from("no mail server answered");
        let mut done = None;
        for host in hosts.iter().take(3) {
            let stream = match pipe(&m, &domain, host).await {
                Ok(s) => s,
                Err((_, why)) => {
                    last = why;
                    continue;
                }
            };
            match mailsend::deliver(stream, host, &helo, &from, &rcpts, &msg, Arc::clone(&roots))
                .await
            {
                Ok(o) => {
                    done = Some(o);
                    break;
                }
                Err(e) => last = format!("{e:#}"),
            }
        }
        match done {
            Some(o) => out.extend(o.into_iter().map(|o| match o.result {
                Ok(()) => serde_json::json!({ "to": o.rcpt, "ok": true }),
                Err(why) => serde_json::json!({ "to": o.rcpt, "ok": false, "why": why }),
            })),
            None => out.extend(
                rcpts
                    .iter()
                    .map(|r| serde_json::json!({ "to": r, "ok": false, "why": last })),
            ),
        }
    }
    Ok(serde_json::json!({ "sent": out }))
}

#[cfg(test)]
mod tests {
    // RFC 9180 A.1.1's recipient: ikmR to pkRm
    #[test]
    fn the_rfc_key_pair() {
        let ikm: Vec<u8> = (0..32)
            .map(|i| {
                u8::from_str_radix(
                    &"6db9df30aa07dd42ee5e8181afdb977e538f5e1fec8a06223f33f7013e525037"
                        [i * 2..i * 2 + 2],
                    16,
                )
                .unwrap()
            })
            .collect();
        let pk: String = super::public_of(&ikm)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            pk,
            "3948cfe0ad1ddb695d780e59077195da6c56506b027329794ab02bca80815c4d"
        );
    }
}
