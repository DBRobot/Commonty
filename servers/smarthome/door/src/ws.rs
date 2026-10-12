//! Just enough of a WebSocket client for Home Assistant's api on loopback:
//! text messages, masked as a client must, pings answered. The tree has no
//! WebSocket crate, and this is all the door asks of one.

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;

pub struct Ws {
    io: BufReader<TcpStream>,
}

impl Ws {
    pub async fn connect(addr: std::net::SocketAddr, path: &str) -> Result<Self> {
        let tcp = TcpStream::connect(addr)
            .await
            .with_context(|| format!("connecting to {addr}"))?;
        let mut io = BufReader::new(tcp);
        let key = {
            use base64::Engine as _;
            let mut k = [0u8; 16];
            getrandom::fill(&mut k).expect("randomness");
            base64::engine::general_purpose::STANDARD.encode(k)
        };
        io.get_mut()
            .write_all(
                format!(
                    "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let mut status = String::new();
        io.read_line(&mut status).await?;
        if !status.starts_with("HTTP/1.1 101") {
            bail!("no websocket: {}", status.trim());
        }
        loop {
            let mut line = String::new();
            if io.read_line(&mut line).await? == 0 {
                bail!("closed during the handshake");
            }
            if line == "\r\n" {
                break;
            }
        }
        Ok(Self { io })
    }

    pub async fn send(&mut self, v: &serde_json::Value) -> Result<()> {
        self.frame(0x1, v.to_string().as_bytes()).await
    }

    async fn frame(&mut self, op: u8, payload: &[u8]) -> Result<()> {
        let mut out = vec![0x80 | op];
        match payload.len() {
            n if n < 126 => out.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(0x80 | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mut mask = [0u8; 4];
        getrandom::fill(&mut mask).expect("randomness");
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.io.get_mut().write_all(&out).await?;
        Ok(())
    }

    /// The next whole text message, as JSON.
    pub async fn recv(&mut self) -> Result<serde_json::Value> {
        let mut msg = Vec::new();
        loop {
            let mut head = [0u8; 2];
            self.io.read_exact(&mut head).await?;
            let (fin, op) = (head[0] & 0x80 != 0, head[0] & 0x0f);
            let masked = head[1] & 0x80 != 0;
            let len = match head[1] & 0x7f {
                126 => self.io.read_u16().await? as u64,
                127 => self.io.read_u64().await?,
                n => n as u64,
            };
            if len > 16 << 20 {
                bail!("a message too big to be an answer");
            }
            let mut mask = [0u8; 4];
            if masked {
                self.io.read_exact(&mut mask).await?;
            }
            let mut payload = vec![0u8; len as usize];
            self.io.read_exact(&mut payload).await?;
            if masked {
                payload
                    .iter_mut()
                    .enumerate()
                    .for_each(|(i, b)| *b ^= mask[i % 4]);
            }
            match op {
                0x8 => bail!("home assistant closed the websocket"),
                0x9 => self.frame(0xA, &payload).await?,
                0xA => {}
                0x0..=0x2 => {
                    msg.extend_from_slice(&payload);
                    if fin {
                        return Ok(serde_json::from_slice(&msg)?);
                    }
                }
                _ => bail!("websocket opcode {op}"),
            }
        }
    }
}
