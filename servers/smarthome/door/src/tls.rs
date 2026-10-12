//! The door ends TLS itself. The certificate is read from its files and
//! read again when they change, so a renewal needs no restart.

use anyhow::{Context, Result};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls_pki_types::pem::PemObject as _;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;

#[derive(Debug)]
pub struct Reloading {
    cert: PathBuf,
    key: PathBuf,
    current: Mutex<Option<(SystemTime, Arc<CertifiedKey>)>>,
}

fn load(cert: &PathBuf, key: &PathBuf) -> Result<CertifiedKey> {
    let certs = CertificateDer::pem_file_iter(cert)
        .with_context(|| format!("reading {}", cert.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    let key =
        PrivateKeyDer::from_pem_file(key).with_context(|| format!("reading {}", key.display()))?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key)?;
    Ok(CertifiedKey::new(certs, signer))
}

impl Reloading {
    pub fn new(cert: PathBuf, key: PathBuf) -> Self {
        Self {
            cert,
            key,
            current: Mutex::new(None),
        }
    }

    fn get(&self) -> Option<Arc<CertifiedKey>> {
        let changed = std::fs::metadata(&self.cert)
            .and_then(|m| m.modified())
            .ok();
        let mut cur = self.current.lock().unwrap();
        match (&*cur, changed) {
            (Some((at, k)), Some(m)) if *at == m => return Some(k.clone()),
            (Some((_, k)), None) => return Some(k.clone()),
            _ => {}
        }
        match load(&self.cert, &self.key) {
            Ok(k) => {
                let k = Arc::new(k);
                *cur = Some((changed.unwrap_or(SystemTime::UNIX_EPOCH), k.clone()));
                Some(k)
            }
            Err(e) => {
                eprintln!("tls: {e:#}");
                cur.as_ref().map(|(_, k)| k.clone())
            }
        }
    }
}

impl ResolvesServerCert for Reloading {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.get()
    }
}

pub fn config(cert: PathBuf, key: PathBuf) -> Result<Arc<rustls::ServerConfig>> {
    let mut c = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(Reloading::new(cert, key)));
    c.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(c))
}

/// TLS connections for axum::serve, each handshake on its own task so a
/// slow one holds up nobody.
pub struct Listener {
    rx: tokio::sync::mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl Listener {
    pub fn new(tcp: TcpListener, config: Arc<rustls::ServerConfig>) -> Result<Self> {
        let local = tcp.local_addr()?;
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        tokio::spawn(async move {
            loop {
                let (s, peer) = match tcp.accept().await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("tls: accept: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Ok(Ok(s)) =
                        tokio::time::timeout(Duration::from_secs(10), acceptor.accept(s)).await
                    {
                        let _ = tx.send((s, peer)).await;
                    }
                });
            }
        });
        Ok(Self { rx, local })
    }
}

impl axum::serve::Listener for Listener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(c) => c,
            // the accepting task never ends while the listener lives
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;

    // self-signed for tests::NAME, a hundred years
    // a test certificate and its key, made for these tests alone
    pub const CERT: &str = include_str!("testdata/cert.pem");
    const KEY: &str = include_str!("testdata/key.pem");

    #[tokio::test]
    async fn the_door_answers_over_tls_with_the_certificate_on_disk() {
        let dir = tempdir("tls");
        std::fs::write(dir.join("cert.pem"), CERT.trim()).unwrap();
        std::fs::write(dir.join("key.pem"), KEY.trim()).unwrap();
        let r = rig("tls");
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let l = Listener::new(
            tcp,
            super::config(dir.join("cert.pem"), dir.join("key.pem")).unwrap(),
        )
        .unwrap();
        tokio::spawn(async move { axum::serve(l, crate::router(r.app.clone())).await.unwrap() });
        let c = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(CERT.trim().as_bytes()).unwrap())
            .resolve(NAME, addr)
            .build()
            .unwrap();
        let res = c
            .get(format!("https://{NAME}:{}/_door/challenge", addr.port()))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        // and the serial Certificate Transparency is compared by
        assert_eq!(
            crate::ct::serial_of_file(&dir.join("cert.pem")).unwrap(),
            "2d1a46ae78661a6254e96f9aa15821946331b45a"
        );
        // a handshake that never finishes holds up nobody else
        let _stuck = tokio::net::TcpStream::connect(addr).await.unwrap();
        let res = c
            .get(format!("https://{NAME}:{}/_door/challenge", addr.port()))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }
}
