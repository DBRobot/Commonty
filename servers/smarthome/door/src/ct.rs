//! Certificate Transparency: every hour, the certificates logged for the
//! door's name, against the ones the door itself has served. One it did
//! not ask for is recorded and shown on the people page.

use crate::{App, Fut, state};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Issued {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub issuer_name: String,
    #[serde(default)]
    pub not_before: String,
    pub serial_number: String,
}

pub trait CtSource: Send + Sync {
    fn issued<'a>(&'a self, name: &'a str) -> Fut<'a, Vec<Issued>>;
}

pub struct CrtSh(pub reqwest::Client);

impl CtSource for CrtSh {
    fn issued<'a>(&'a self, name: &'a str) -> Fut<'a, Vec<Issued>> {
        Box::pin(async move {
            Ok(self
                .0
                .get("https://crt.sh/")
                .query(&[("q", name), ("output", "json")])
                .timeout(std::time::Duration::from_secs(60))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?)
        })
    }
}

/// lowercase hex, no leading zeros, no separators: crt.sh's and ours alike
pub fn normalise(serial: &str) -> String {
    let s: String = serial
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase();
    let t = s.trim_start_matches('0');
    if t.is_empty() { "0".into() } else { t.into() }
}

/// One DER element: (tag, contents, the rest).
fn tlv(b: &[u8]) -> Result<(u8, &[u8], &[u8])> {
    if b.len() < 2 {
        bail!("short");
    }
    let (tag, first) = (b[0], b[1]);
    let (len, head) = if first < 0x80 {
        (first as usize, 2)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || b.len() < 2 + n {
            bail!("bad length");
        }
        (
            b[2..2 + n]
                .iter()
                .fold(0usize, |a, x| (a << 8) | *x as usize),
            2 + n,
        )
    };
    if b.len() < head + len {
        bail!("truncated");
    }
    Ok((tag, &b[head..head + len], &b[head + len..]))
}

/// The serial of a certificate: Certificate → TBSCertificate → [0] version?
/// → INTEGER.
pub fn serial(der: &[u8]) -> Result<String> {
    let (t, cert, _) = tlv(der)?;
    let (t2, tbs, _) = tlv(cert)?;
    if t != 0x30 || t2 != 0x30 {
        bail!("not a certificate");
    }
    let (mut tag, mut val, rest) = tlv(tbs)?;
    if tag == 0xa0 {
        (tag, val, _) = tlv(rest)?;
    }
    if tag != 0x02 {
        bail!("no serial");
    }
    Ok(normalise(
        &val.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    ))
}

/// The serial of the certificate in a PEM file: the first one, the door's own.
pub fn serial_of_file(path: &Path) -> Result<String> {
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject as _;
    let first = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("reading {}", path.display()))?
        .next()
        .context("no certificate")??;
    serial(&first)
}

/// Remember the certificate the door serves now, if it is a new one.
pub fn note_own(app: &App) {
    let Some((cert, _)) = &app.cfg.tls else {
        return;
    };
    let Ok(s) = serial_of_file(cert) else {
        return;
    };
    let _ = app.with_store(|st| {
        if st.state.certs.contains(&s) {
            return Ok(());
        }
        st.state.certs.push(s);
        st.save()
    });
}

/// One look: what is logged for the name and is not ours is recorded once.
pub async fn check(app: &App) -> Result<usize> {
    let Some(name) = app.name() else {
        return Ok(0);
    };
    let logged = app.ct.issued(&name).await?;
    // a renewal may have landed since the last look
    note_own(app);
    app.with_store(|s| {
        let now = identity::now();
        let mut found = 0;
        for i in logged {
            let serial = normalise(&i.serial_number);
            if s.state.certs.contains(&serial)
                || s.state.ct.unexpected.iter().any(|u| u.serial == serial)
            {
                continue;
            }
            found += 1;
            s.state.ct.unexpected.push(state::Unexpected {
                serial,
                issuer: i.issuer_name,
                not_before: i.not_before,
                id: i.id,
                seen: now,
            });
        }
        s.state.ct.checked = Some(now);
        s.save()?;
        Ok(found)
    })
}

pub async fn watch(app: std::sync::Arc<App>) {
    loop {
        match check(&app).await {
            Ok(0) => {}
            Ok(n) => eprintln!("ct: {n} certificate(s) for this door's name it did not ask for"),
            Err(e) => eprintln!("ct: {e:#}"),
        }
        crate::signin::refresh(&app).await;
        tokio::time::sleep(app.cfg.ct_every).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;

    /// Certificate { TBS { [0] { INTEGER 2 }, INTEGER serial, ... } }
    fn cert(serial: &[u8]) -> Vec<u8> {
        let mut tbs = vec![0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, serial.len() as u8];
        tbs.extend_from_slice(serial);
        tbs.extend_from_slice(&[0x30, 0x00]);
        let mut t = vec![0x30, tbs.len() as u8];
        t.extend(tbs);
        let mut c = vec![0x30, t.len() as u8];
        c.extend(t);
        c
    }

    #[test]
    fn serials_read_and_compare_the_same_way() {
        assert_eq!(serial(&cert(&[0x00, 0x8f, 0x01])).unwrap(), "8f01");
        assert_eq!(normalise("00:8F:01"), "8f01");
        assert_eq!(normalise("0"), "0");
        assert!(serial(&[0x30, 0x05, 0x01]).is_err());
        assert!(serial(&[0x02, 0x01, 0x01]).is_err());
    }

    #[tokio::test]
    async fn a_certificate_the_door_did_not_get_is_recorded_once() {
        let dir = tempdir("ct-pem");
        let pem = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(cert(&[0x0a, 0xbc]))
            }
        );
        std::fs::write(dir.join("cert.pem"), pem).unwrap();
        let r = rig_with("ct", |c| {
            c.tls = Some((dir.join("cert.pem"), dir.join("key.pem")))
        });
        let issued = |s: &str| Issued {
            id: 7,
            issuer_name: "C=US, O=Let's Encrypt".into(),
            not_before: "2026-10-01T00:00:00".into(),
            serial_number: s.into(),
        };
        // only ours: nothing
        *r.ct.0.lock().unwrap() = vec![issued("0abc")];
        assert_eq!(check(&r.app).await.unwrap(), 0);
        assert_eq!(
            r.app.with_store(|s| s.state.certs.clone()),
            vec!["abc".to_string()]
        );
        // and someone else's
        r.ct.0.lock().unwrap().push(issued("dead"));
        assert_eq!(check(&r.app).await.unwrap(), 1);
        assert_eq!(check(&r.app).await.unwrap(), 0, "once");
        let u = r.app.with_store(|s| s.state.ct.unexpected.clone());
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].serial, "dead");
        assert!(r.app.with_store(|s| s.state.ct.checked.is_some()));
    }
}
