//! A member's mail to another domain, from their own device (docs/dev/mail.md):
//! the DKIM part (what the mail Worker signs: the headers alone, and the
//! body's hash), and SMTP over whatever stream the box gives - its pipe -
//! with STARTTLS required and the certificate checked against the mail
//! server's own name. A server that offers no TLS, or the wrong
//! certificate, gets nothing: the box in between would read it otherwise.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use sha2::Digest as _;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// the headers the Worker signs, when the message has them (dkim.js SIGNED)
const SIGNED: &[&str] = &[
    "from",
    "to",
    "cc",
    "reply-to",
    "date",
    "message-id",
    "in-reply-to",
    "references",
    "mime-version",
    "content-type",
    "content-transfer-encoding",
];
const MAX_LINE: usize = 4096;
/// lines in one reply at most: EHLO's are a dozen
const MAX_LINES: usize = 100;

/// header block and body, split at the first empty line
fn split(msg: &[u8]) -> (&[u8], &[u8]) {
    for i in 0..msg.len() {
        if msg[i..].starts_with(b"\r\n\r\n") {
            return (&msg[..i + 2], &msg[i + 4..]);
        }
        if msg[i..].starts_with(b"\n\n") {
            return (&msg[..i + 1], &msg[i + 2..]);
        }
    }
    (msg, &[])
}

/// every header, unfolded: (name as written, value)
pub fn headers(msg: &[u8]) -> Vec<(String, String)> {
    let (head, _) = split(msg);
    let text = String::from_utf8_lossy(head);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            if let Some(l) = out.last_mut() {
                l.1.push(' ');
                l.1.push_str(line.trim());
            }
        } else if let Some((n, v)) = line.split_once(':') {
            out.push((n.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

/// RFC 6376 relaxed body: spaces run together, none at line ends, no
/// empty lines at the end, CRLF line ends; the empty body stays empty
pub fn relaxed_body(body: &[u8]) -> Vec<u8> {
    let text = body;
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for raw in text.split(|b| *b == b'\n') {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let mut l = Vec::with_capacity(raw.len());
        let mut ws = false;
        for &c in raw {
            if c == b' ' || c == b'\t' {
                ws = true;
            } else {
                if ws {
                    l.push(b' ');
                    ws = false;
                }
                l.push(c);
            }
        }
        lines.push(l);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut out = Vec::new();
    for l in lines {
        out.extend_from_slice(&l);
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// What the Worker is asked to sign: the signed headers, in the order the
/// message has them, and the body's hash. Never the Subject, never the body.
pub fn dkim_request(msg: &[u8]) -> (Vec<(String, String)>, String) {
    let hs = headers(msg)
        .into_iter()
        .filter(|(n, _)| SIGNED.contains(&n.to_ascii_lowercase().as_str()))
        .collect();
    let (_, body) = split(msg);
    let bh =
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(relaxed_body(body)));
    (hs, bh)
}

/// the message with its signature on top, in CRLF lines throughout
pub fn signed(msg: &[u8], dkim_header: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.len() + dkim_header.len() + 4);
    out.extend_from_slice(dkim_header.trim_end().as_bytes());
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&crlf(msg));
    out
}

/// lone LFs as CRLF
pub fn crlf(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.len() + msg.len() / 40);
    for (i, &b) in msg.iter().enumerate() {
        if b == b'\n' && (i == 0 || msg[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

/// DATA's body: a line starting with a dot gets another, then the end
fn stuffed(msg: &[u8]) -> Vec<u8> {
    let m = crlf(msg);
    let mut out = Vec::with_capacity(m.len() + 8);
    let mut start = true;
    for &b in &m {
        if start && b == b'.' {
            out.push(b'.');
        }
        out.push(b);
        start = b == b'\n';
    }
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    out
}

/// What became of each recipient: accepted, or the server's refusal
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub rcpt: String,
    pub result: std::result::Result<(), String>,
}

/// roots a mail server's certificate is checked against: the usual web ones
pub fn web_roots() -> Arc<rustls::RootCertStore> {
    Arc::new(rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    })
}

struct Conn<S> {
    s: BufReader<S>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Conn<S> {
    /// one reply: its code and its text, every line of a multi-line one
    async fn reply(&mut self) -> Result<(u16, String)> {
        let mut text = String::new();
        for _ in 0..MAX_LINES {
            let mut line = String::new();
            let n = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                tokio::io::AsyncReadExt::take(&mut self.s, MAX_LINE as u64).read_line(&mut line),
            )
            .await
            .context("the mail server went quiet")??;
            if n == 0 {
                bail!("the mail server hung up");
            }
            let l = line.trim_end();
            if l.len() < 3 {
                bail!("not an SMTP reply: {l}");
            }
            let code: u16 = l[..3]
                .parse()
                .map_err(|_| anyhow!("not an SMTP reply: {l}"))?;
            text.push_str(l.get(4..).unwrap_or(""));
            text.push('\n');
            if l.as_bytes().get(3) != Some(&b'-') {
                return Ok((code, text.trim_end().to_string()));
            }
        }
        bail!("the mail server's reply went on and on")
    }
    async fn say(&mut self, line: &str) -> Result<(u16, String)> {
        let w = self.s.get_mut();
        w.write_all(line.as_bytes()).await?;
        w.write_all(b"\r\n").await?;
        w.flush().await?;
        self.reply().await
    }
}

fn clean(addr: &str) -> Result<&str> {
    let a = addr.trim();
    if a.is_empty() || a.len() > 254 || a.contains(['<', '>', '\r', '\n', ' ']) || !a.contains('@')
    {
        bail!("not an address: {a}");
    }
    Ok(a)
}

/// The whole of one delivery over `stream`: greeting, EHLO, STARTTLS, the
/// TLS handshake with `mx` as the name its certificate must carry, EHLO
/// again, the envelope, DATA, QUIT. Recipients the server refuses are said
/// so; the rest have it.
pub async fn deliver<S>(
    stream: S,
    mx: &str,
    helo: &str,
    from: &str,
    rcpts: &[String],
    msg: &[u8],
    roots: Arc<rustls::RootCertStore>,
) -> Result<Vec<Outcome>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let from = clean(from)?;
    let rcpts: Vec<&str> = rcpts.iter().map(|r| clean(r)).collect::<Result<_>>()?;
    if rcpts.is_empty() {
        bail!("nobody to send it to");
    }
    let mut c = Conn {
        s: BufReader::new(stream),
    };
    let (code, text) = c.reply().await?;
    if code != 220 {
        bail!("{mx} would not take mail: {code} {text}");
    }
    let (code, caps) = c.say(&format!("EHLO {helo}")).await?;
    if code != 250 {
        bail!("{mx} refused EHLO: {code} {caps}");
    }
    if !caps
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("STARTTLS"))
    {
        bail!("{mx} offers no encryption; nothing was sent, as the box in between would read it");
    }
    let (code, text) = c.say("STARTTLS").await?;
    if code != 220 {
        bail!("{mx} refused STARTTLS: {code} {text}");
    }
    // nothing left over in the buffer: the server's next bytes are TLS
    if !c.s.buffer().is_empty() {
        bail!("{mx} spoke out of turn before TLS");
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(mx.to_string())
        .map_err(|_| anyhow!("not a server name: {mx}"))?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, c.s.into_inner())
        .await
        .with_context(|| format!("{mx}'s certificate is not its own; nothing was sent"))?;
    let mut c = Conn {
        s: BufReader::new(tls),
    };
    let (code, caps) = c.say(&format!("EHLO {helo}")).await?;
    if code != 250 {
        bail!("{mx} refused EHLO over TLS: {code} {caps}");
    }
    let utf8 = !from.is_ascii() || rcpts.iter().any(|r| !r.is_ascii());
    if utf8
        && !caps
            .lines()
            .any(|l| l.trim().eq_ignore_ascii_case("SMTPUTF8"))
    {
        bail!("{mx} cannot take an address in that alphabet");
    }
    let size = if caps
        .lines()
        .any(|l| l.to_ascii_uppercase().starts_with("SIZE"))
    {
        format!(" SIZE={}", msg.len())
    } else {
        String::new()
    };
    let (code, text) = c
        .say(&format!(
            "MAIL FROM:<{from}>{size}{}",
            if utf8 { " SMTPUTF8" } else { "" }
        ))
        .await?;
    if code != 250 {
        bail!("{mx} refused the sender: {code} {text}");
    }
    let mut out = Vec::new();
    for r in &rcpts {
        let (code, text) = c.say(&format!("RCPT TO:<{r}>")).await?;
        out.push(Outcome {
            rcpt: r.to_string(),
            result: if code == 250 || code == 251 {
                Ok(())
            } else {
                Err(format!("{code} {text}"))
            },
        });
    }
    if out.iter().all(|o| o.result.is_err()) {
        let _ = c.say("QUIT").await;
        return Ok(out);
    }
    let (code, text) = c.say("DATA").await?;
    if code != 354 {
        bail!("{mx} refused the message: {code} {text}");
    }
    let w = c.s.get_mut();
    w.write_all(&stuffed(msg)).await?;
    w.flush().await?;
    let (code, text) = c.reply().await?;
    if code != 250 {
        let why = format!("{code} {text}");
        for o in out.iter_mut().filter(|o| o.result.is_ok()) {
            o.result = Err(why.clone());
        }
    }
    let _ = c.say("QUIT").await;
    let _ = c.s.get_mut().shutdown().await;
    Ok(out)
}
