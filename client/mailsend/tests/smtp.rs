//! Delivery against a made-up mail server over an in-memory pipe, with a
//! certificate from the test CA in testdata/ (ca.pem; mx.pem names mx.example.test).

use std::sync::{Arc, Mutex};

use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Default, Debug)]
struct Seen {
    from: Option<String>,
    rcpts: Vec<String>,
    data: Option<Vec<u8>>,
    plain_after_ehlo: Vec<String>,
}

fn roots() -> Arc<rustls::RootCertStore> {
    let mut r = rustls::RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(include_bytes!("testdata/ca.pem")) {
        r.add(c.unwrap()).unwrap();
    }
    Arc::new(r)
}

fn server_config() -> Arc<rustls::ServerConfig> {
    let certs: Vec<_> = CertificateDer::pem_slice_iter(include_bytes!("testdata/mx.pem"))
        .map(|c| c.unwrap())
        .collect();
    let key = PrivateKeyDer::from_pem_slice(include_bytes!("testdata/mx.key")).unwrap();
    Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap(),
    )
}

async fn line<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> String {
    let mut l = String::new();
    r.read_line(&mut l).await.unwrap();
    l.trim_end().to_string()
}

/// a server: with or without STARTTLS, refusing any rcpt starting "nobody"
async fn server(io: tokio::io::DuplexStream, tls: bool, seen: Arc<Mutex<Seen>>) {
    let mut r = BufReader::new(io);
    r.get_mut()
        .write_all(b"220 mx.example.test ESMTP ready\r\n")
        .await
        .unwrap();
    let ehlo = line(&mut r).await;
    assert!(ehlo.starts_with("EHLO "));
    let caps = if tls {
        "250-mx.example.test\r\n250-SIZE 35882577\r\n250-STARTTLS\r\n250 8BITMIME\r\n"
    } else {
        "250-mx.example.test\r\n250 8BITMIME\r\n"
    };
    r.get_mut().write_all(caps.as_bytes()).await.unwrap();
    let next = line(&mut r).await;
    if next != "STARTTLS" {
        // a client that goes on in the clear: what it says is recorded
        let mut l = next;
        while !l.is_empty() {
            seen.lock().unwrap().plain_after_ehlo.push(l.clone());
            r.get_mut().write_all(b"250 ok\r\n").await.unwrap();
            l = line(&mut r).await;
        }
        return;
    }
    r.get_mut().write_all(b"220 go ahead\r\n").await.unwrap();
    let Ok(t) = tokio_rustls::TlsAcceptor::from(server_config())
        .accept(r.into_inner())
        .await
    else {
        return;
    };
    let mut r = BufReader::new(t);
    loop {
        let l = line(&mut r).await;
        let w = |s: &'static str| s.as_bytes();
        let reply: &[u8] = if l.starts_with("EHLO") {
            w("250-mx.example.test\r\n250 SIZE 35882577\r\n")
        } else if let Some(f) = l.strip_prefix("MAIL FROM:<") {
            seen.lock().unwrap().from = Some(f.split('>').next().unwrap().to_string());
            w("250 ok\r\n")
        } else if let Some(t) = l.strip_prefix("RCPT TO:<") {
            let t = t.trim_end_matches('>').to_string();
            if t.starts_with("nobody") {
                w("550 5.1.1 no such user\r\n")
            } else {
                seen.lock().unwrap().rcpts.push(t);
                w("250 ok\r\n")
            }
        } else if l == "DATA" {
            r.get_mut().write_all(b"354 end with .\r\n").await.unwrap();
            let mut data = Vec::new();
            loop {
                let mut b = Vec::new();
                r.read_until(b'\n', &mut b).await.unwrap();
                if b == b".\r\n" {
                    break;
                }
                let b = if b.starts_with(b"..") {
                    b[1..].to_vec()
                } else {
                    b
                };
                data.extend_from_slice(&b);
            }
            seen.lock().unwrap().data = Some(data);
            w("250 2.0.0 queued\r\n")
        } else if l == "QUIT" {
            r.get_mut().write_all(b"221 bye\r\n").await.unwrap();
            return;
        } else {
            w("500 what\r\n")
        };
        r.get_mut().write_all(reply).await.unwrap();
    }
}

const MSG: &[u8] = b"From: Tester <tester@commonty.test>\r\nTo: friend@example.test\r\nSubject: Lunch?\r\nDate: Sun, 18 Oct 2026 10:00:00 +0000\r\nMessage-ID: <1@commonty.test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSee you at noon.\r\n.a line with a dot\r\n";

#[tokio::test]
async fn delivered_over_tls() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s = tokio::spawn(server(b, true, seen.clone()));
    let out = mailsend::deliver(
        a,
        "mx.example.test",
        "commonty.test",
        "tester@commonty.test",
        &["friend@example.test".into(), "nobody@example.test".into()],
        MSG,
        roots(),
    )
    .await
    .unwrap();
    s.await.unwrap();
    assert_eq!(out[0].rcpt, "friend@example.test");
    assert_eq!(out[0].result, Ok(()));
    assert!(out[1].result.as_ref().unwrap_err().starts_with("550"));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.from.as_deref(), Some("tester@commonty.test"));
    assert_eq!(seen.rcpts, vec!["friend@example.test"]);
    assert_eq!(
        seen.data.as_deref(),
        Some(MSG),
        "the message arrives as it was, its dot line unstuffed"
    );
}

#[tokio::test]
async fn no_starttls_means_nothing_sent() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s = tokio::spawn(server(b, false, seen.clone()));
    let e = mailsend::deliver(
        a,
        "mx.example.test",
        "commonty.test",
        "tester@commonty.test",
        &["friend@example.test".into()],
        MSG,
        roots(),
    )
    .await
    .unwrap_err();
    assert!(format!("{e}").contains("offers no encryption"), "{e}");
    drop(s);
    assert!(
        seen.lock().unwrap().plain_after_ehlo.is_empty(),
        "no envelope in the clear"
    );
    assert!(seen.lock().unwrap().data.is_none());
}

#[tokio::test]
async fn a_certificate_for_another_name_means_nothing_sent() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let _s = tokio::spawn(server(b, true, seen.clone()));
    let e = mailsend::deliver(
        a,
        "mx.other.test",
        "commonty.test",
        "tester@commonty.test",
        &["friend@example.test".into()],
        MSG,
        roots(),
    )
    .await
    .unwrap_err();
    assert!(format!("{e:#}").contains("not its own"), "{e:#}");
    assert!(seen.lock().unwrap().from.is_none());
}

#[tokio::test]
async fn a_certificate_from_no_known_root_means_nothing_sent() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let _s = tokio::spawn(server(b, true, seen.clone()));
    assert!(
        mailsend::deliver(
            a,
            "mx.example.test",
            "commonty.test",
            "tester@commonty.test",
            &["friend@example.test".into()],
            MSG,
            mailsend::web_roots()
        )
        .await
        .is_err()
    );
    assert!(seen.lock().unwrap().from.is_none());
}

#[test]
fn addresses_that_would_break_the_envelope_are_refused() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for bad in ["a@b>\r\nRCPT TO:<c@d", "no-at-sign", "", "a b@c.d"] {
        let (a, _b) = tokio::io::duplex(64);
        let r = rt.block_on(mailsend::deliver(
            a,
            "mx.example.test",
            "x",
            "tester@commonty.test",
            &[bad.into()],
            MSG,
            roots(),
        ));
        assert!(r.is_err(), "{bad:?}");
    }
}

#[test]
fn relaxed_body_as_the_rfc_has_it() {
    // RFC 6376 3.4.5's example
    assert_eq!(
        mailsend::relaxed_body(b" C \r\nD \t E\r\n\r\n\r\n"),
        b" C\r\nD E\r\n"
    );
    assert_eq!(mailsend::relaxed_body(b""), b"");
    assert_eq!(mailsend::relaxed_body(b"x"), b"x\r\n");
}

#[test]
fn what_the_worker_is_asked_to_sign() {
    let (hs, bh) = mailsend::dkim_request(b"From: a@commonty.test\r\nSubject: secret\r\nTo: b@x.test,\r\n c@x.test\r\nX-Other: 1\r\n\r\nbody\r\n");
    assert_eq!(
        hs,
        vec![
            ("From".to_string(), "a@commonty.test".to_string()),
            ("To".to_string(), "b@x.test, c@x.test".to_string())
        ]
    );
    use base64::Engine as _;
    use sha2::Digest as _;
    assert_eq!(
        bh,
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(b"body\r\n"))
    );
}

#[tokio::test]
async fn a_dotted_long_message_round_trips() {
    let mut big = MSG.to_vec();
    for i in 0..5000 {
        big.extend_from_slice(format!(".line {i}\r\n").as_bytes());
    }
    let (a, b) = tokio::io::duplex(1 << 12);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s = tokio::spawn(server(b, true, seen.clone()));
    let out = mailsend::deliver(
        a,
        "mx.example.test",
        "commonty.test",
        "tester@commonty.test",
        &["friend@example.test".into()],
        &big,
        roots(),
    )
    .await
    .unwrap();
    s.await.unwrap();
    assert!(out[0].result.is_ok());
    assert_eq!(seen.lock().unwrap().data.as_deref(), Some(&big[..]));
    let mut x = Vec::new();
    let _ = tokio::io::empty().read_to_end(&mut x).await;
}
