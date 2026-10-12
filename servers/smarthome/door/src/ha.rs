//! Home Assistant, as the door's own admin: onboarding on first start (an
//! owner "Commonty" with a password only the door holds), its tokens, and a
//! user for each person let in. The door talks to it from 127.0.0.1, which
//! the trusted networks do not cover, so this is the one sign-in by password.

use crate::state::HaAdmin;
use crate::{Fut, ws::Ws};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::net::SocketAddr;

pub trait Ha: Send + Sync {
    /// Onboarded, and the admin's tokens good; updates `admin` as it goes.
    fn setup<'a>(&'a self, admin: &'a mut HaAdmin) -> Fut<'a, ()>;
    /// A new user for a person; its id.
    fn create_user<'a>(
        &'a self,
        admin: &'a mut HaAdmin,
        name: &'a str,
        group: &'a str,
    ) -> Fut<'a, String>;
    /// The ids of the users it has.
    fn users<'a>(&'a self, admin: &'a mut HaAdmin) -> Fut<'a, Vec<String>>;
    /// Home Assistant reads the users file only when it starts.
    fn restart(&self) -> Fut<'_, ()>;
    /// A backup to its local agent, made and finished: a new tar in its
    /// backups directory.
    fn backup<'a>(&'a self, admin: &'a mut HaAdmin) -> Fut<'a, ()>;
    /// One of its local backups restored; it restarts itself, and this
    /// returns once it answers again.
    fn restore<'a>(&'a self, admin: &'a mut HaAdmin, id: &'a str) -> Fut<'a, ()>;
}

pub const USERNAME: &str = "commonty";

pub struct HomeAssistant {
    pub addr: SocketAddr,
    pub restart: Vec<String>,
    pub http: reqwest::Client,
    /// how often a backup or a restart is looked at; the waits are counted in these
    pub poll: std::time::Duration,
}

/// Home Assistant's own agent: backups in its config's backups directory
pub const LOCAL_AGENT: &str = "backup.local";

impl HomeAssistant {
    pub fn new(addr: SocketAddr, restart: Vec<String>) -> Self {
        Self {
            addr,
            restart,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("a client"),
            poll: std::time::Duration::from_secs(2),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// Home Assistant's clients are named by a url; the redirect must share
    /// its host.
    fn client_id(&self) -> String {
        self.url("/")
    }

    async fn json(&self, r: reqwest::RequestBuilder, what: &str) -> Result<Value> {
        let r = r.send().await.with_context(|| what.to_string())?;
        let status = r.status();
        let body = r.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{what}: {status} {body}");
        }
        serde_json::from_str(&body).with_context(|| format!("{what}: {body}"))
    }

    /// Whether the owner exists: the user step is done, or onboarding is
    /// over and its api gone.
    async fn onboarded(&self) -> Result<bool> {
        let r = self.http.get(self.url("/api/onboarding")).send().await?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(true);
        }
        let steps: Value = r.error_for_status()?.json().await?;
        Ok(steps
            .as_array()
            .into_iter()
            .flatten()
            .any(|s| s["step"] == "user" && s["done"] == true))
    }

    async fn tokens(&self, form: &[(&str, &str)]) -> Result<Value> {
        self.json(self.http.post(self.url("/auth/token")).form(form), "token")
            .await
    }

    async fn exchange_code(&self, admin: &mut HaAdmin, code: &str) -> Result<String> {
        let cid = self.client_id();
        let t = self
            .tokens(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", &cid),
            ])
            .await?;
        if let Some(rt) = t["refresh_token"].as_str() {
            admin.refresh_token = Some(rt.to_string());
        }
        t["access_token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("no access token"))
    }

    async fn onboard(&self, admin: &mut HaAdmin) -> Result<()> {
        let password = admin.password.clone().context("no password yet")?;
        let cid = self.client_id();
        let made = self
            .json(
                self.http
                    .post(self.url("/api/onboarding/users"))
                    .json(&json!({
                        "client_id": cid,
                        "name": "Commonty",
                        "username": USERNAME,
                        "password": password,
                        "language": "en",
                    })),
                "onboarding",
            )
            .await?;
        let code = made["auth_code"].as_str().context("no auth code")?;
        let access = self.exchange_code(admin, code).await?;
        // the rest of onboarding, so nobody meets its pages; each is
        // harmless to have done already
        for (step, body) in [
            ("core_config", json!({})),
            ("analytics", json!({})),
            (
                "integration",
                json!({ "client_id": cid, "redirect_uri": format!("{cid}?auth_callback=1") }),
            ),
        ] {
            let r = self
                .http
                .post(self.url(&format!("/api/onboarding/{step}")))
                .bearer_auth(&access)
                .json(&body)
                .send()
                .await;
            if let Err(e) = r.and_then(|r| r.error_for_status()) {
                eprintln!("home assistant: onboarding {step}: {e}");
            }
        }
        Ok(())
    }

    /// An access token: from the refresh token, or by signing in again
    /// with the password when that is gone.
    async fn access(&self, admin: &mut HaAdmin) -> Result<String> {
        let cid = self.client_id();
        if let Some(rt) = admin.refresh_token.clone() {
            match self
                .tokens(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &rt),
                    ("client_id", &cid),
                ])
                .await
            {
                Ok(t) => {
                    if let Some(a) = t["access_token"].as_str() {
                        return Ok(a.to_string());
                    }
                }
                Err(e) => eprintln!("home assistant: refresh: {e:#}"),
            }
        }
        let password = admin.password.clone().context("no password yet")?;
        let redirect = format!("{cid}?auth_callback=1");
        let flow = self
            .json(
                self.http.post(self.url("/auth/login_flow")).json(&json!({
                    "client_id": cid,
                    "handler": ["homeassistant", null],
                    "redirect_uri": redirect,
                })),
                "login flow",
            )
            .await?;
        let id = flow["flow_id"].as_str().context("no flow id")?;
        let done = self
            .json(
                self.http
                    .post(self.url(&format!("/auth/login_flow/{id}")))
                    .json(&json!({ "client_id": cid, "username": USERNAME, "password": password })),
                "sign in",
            )
            .await?;
        if done["type"] != "create_entry" {
            bail!("home assistant refused the door's own sign-in: {done}");
        }
        let code = done["result"].as_str().context("no code")?;
        self.exchange_code(admin, code).await
    }

    /// The websocket api, signed in as the door's admin.
    async fn socket(&self, access: &str) -> Result<Ws> {
        let mut ws = Ws::connect(self.addr, "/api/websocket").await?;
        let hello = ws.recv().await?;
        if hello["type"] != "auth_required" {
            bail!("unexpected greeting {hello}");
        }
        ws.send(&json!({ "type": "auth", "access_token": access }))
            .await?;
        let ok = ws.recv().await?;
        if ok["type"] != "auth_ok" {
            bail!("websocket sign-in refused: {ok}");
        }
        Ok(ws)
    }

    async fn new_user(&self, access: &str, name: &str, group: &str) -> Result<String> {
        let mut ws = self.socket(access).await?;
        let r = call(
            &mut ws,
            1,
            json!({
                "type": "config/auth/create",
                "name": name,
                "group_ids": [group],
                // they arrive from loopback, always
                "local_only": true,
            }),
        )
        .await
        .with_context(|| format!("making {name}'s user"))?;
        let id = r["user"]["id"].as_str().context("no user id")?;
        // it goes into a YAML file Home Assistant reads
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            bail!("a user id that is not one: {id:?}");
        }
        Ok(id.to_string())
    }

    async fn user_ids(&self, access: &str) -> Result<Vec<String>> {
        let mut ws = self.socket(access).await?;
        let r = call(&mut ws, 1, json!({ "type": "config/auth/list" }))
            .await
            .context("listing its users")?;
        Ok(r.as_array()
            .context("no list of users")?
            .iter()
            .filter_map(|u| u["id"].as_str().map(str::to_string))
            .collect())
    }

    async fn make_backup(&self, access: &str) -> Result<()> {
        let mut ws = self.socket(access).await?;
        call(
            &mut ws,
            1,
            json!({
                "type": "backup/generate",
                "agent_ids": [LOCAL_AGENT],
                "include_homeassistant": true,
                "include_database": true,
                "name": "Commonty",
            }),
        )
        .await
        .context("asking for a backup")?;
        // done when its backup manager is idle again: 30 minutes at most
        for id in 2..902 {
            tokio::time::sleep(self.poll).await;
            let info = call(&mut ws, id, json!({ "type": "backup/info" })).await?;
            if info["state"] != "idle" {
                continue;
            }
            let last = &info["last_action_event"];
            if last["manager_state"] == "create_backup" && last["state"] == "failed" {
                bail!("Home Assistant's backup failed: {}", last["reason"]);
            }
            return Ok(());
        }
        bail!("Home Assistant's backup did not finish")
    }

    /// Ask for the restore, of a backup it knows.
    async fn ask_restore(&self, access: &str, id: &str) -> Result<()> {
        let mut ws = self.socket(access).await?;
        let info = call(&mut ws, 1, json!({ "type": "backup/info" })).await?;
        let known = info["backups"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| b["backup_id"] == id);
        if !known {
            bail!("Home Assistant does not have backup {id} yet");
        }
        ws.send(&json!({
            "id": 2,
            "type": "backup/restore",
            "backup_id": id,
            "agent_id": LOCAL_AGENT,
            "restore_database": true,
            "restore_homeassistant": true,
        }))
        .await?;
        loop {
            // it may go down before it answers: that is it restoring
            let Ok(m) = ws.recv().await else {
                return Ok(());
            };
            if m["id"] == 2 && m["type"] == "result" {
                if m["success"] != true {
                    bail!("restoring backup {id}: {}", m["error"]);
                }
                return Ok(());
            }
        }
    }

    async fn answers(&self) -> bool {
        self.http
            .get(self.url("/api/onboarding"))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .is_ok()
    }
}

/// One command's result; events and other answers on the way are passed over.
async fn call(ws: &mut Ws, id: u64, mut msg: Value) -> Result<Value> {
    msg["id"] = json!(id);
    ws.send(&msg).await?;
    loop {
        let m = ws.recv().await?;
        if m["id"] != id || m["type"] != "result" {
            continue;
        }
        if m["success"] != true {
            bail!("{}: {}", msg["type"], m["error"]);
        }
        return Ok(m["result"].clone());
    }
}

impl Ha for HomeAssistant {
    fn setup<'a>(&'a self, admin: &'a mut HaAdmin) -> Fut<'a, ()> {
        Box::pin(async move {
            if !self.onboarded().await? {
                eprintln!("home assistant: onboarding as {USERNAME}");
                return self.onboard(admin).await;
            }
            self.access(admin).await.map(|_| ())
        })
    }

    fn create_user<'a>(
        &'a self,
        admin: &'a mut HaAdmin,
        name: &'a str,
        group: &'a str,
    ) -> Fut<'a, String> {
        Box::pin(async move {
            let access = self.access(admin).await?;
            self.new_user(&access, name, group).await
        })
    }

    fn users<'a>(&'a self, admin: &'a mut HaAdmin) -> Fut<'a, Vec<String>> {
        Box::pin(async move {
            let access = self.access(admin).await?;
            self.user_ids(&access).await
        })
    }

    fn restart(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            let Some((cmd, args)) = self.restart.split_first() else {
                return Ok(());
            };
            let s = tokio::process::Command::new(cmd)
                .args(args)
                .status()
                .await
                .with_context(|| format!("running {cmd}"))?;
            anyhow::ensure!(s.success(), "{} failed: {s}", self.restart.join(" "));
            Ok(())
        })
    }

    fn backup<'a>(&'a self, admin: &'a mut HaAdmin) -> Fut<'a, ()> {
        Box::pin(async move {
            let access = self.access(admin).await?;
            self.make_backup(&access).await
        })
    }

    fn restore<'a>(&'a self, admin: &'a mut HaAdmin, id: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            // back from the restart that read the backups directory, and
            // knowing the backup: 10 minutes at most
            let mut tries = 0;
            loop {
                let asked = match self.access(admin).await {
                    Ok(a) => self.ask_restore(&a, id).await,
                    Err(e) => Err(e),
                };
                match asked {
                    Ok(()) => break,
                    Err(e) if tries >= 300 => return Err(e),
                    Err(_) => tries += 1,
                }
                tokio::time::sleep(self.poll).await;
            }
            // it goes down to restore, then comes back up
            for _ in 0..60 {
                if !self.answers().await {
                    break;
                }
                tokio::time::sleep(self.poll).await;
            }
            for _ in 0..900 {
                if self.answers().await {
                    return Ok(());
                }
                tokio::time::sleep(self.poll).await;
            }
            bail!("Home Assistant did not come back after the restore")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

    /// What the fake saw and holds.
    #[derive(Default)]
    struct Seen {
        onboarded: bool,
        password: Option<String>,
        created: Vec<Value>,
        refresh_ok: bool,
        /// polls of backup/info left before the backup is done
        busy: u32,
        backup_fails: bool,
        commands: Vec<Value>,
    }

    /// A Home Assistant just big enough: plain HTTP/1.1, one request per
    /// connection, and the websocket's three messages.
    async fn fake() -> (SocketAddr, Arc<Mutex<Seen>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Seen {
            refresh_ok: true,
            ..Default::default()
        }));
        let s = seen.clone();
        tokio::spawn(async move {
            loop {
                let (c, _) = l.accept().await.unwrap();
                let s = s.clone();
                tokio::spawn(async move { serve(c, s).await });
            }
        });
        (addr, seen)
    }

    async fn serve(c: tokio::net::TcpStream, seen: Arc<Mutex<Seen>>) {
        let mut io = BufReader::new(c);
        let mut line = String::new();
        io.read_line(&mut line).await.unwrap();
        let path = line.split(' ').nth(1).unwrap_or_default().to_string();
        let mut len = 0;
        loop {
            let mut h = String::new();
            io.read_line(&mut h).await.unwrap();
            if h == "\r\n" {
                break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0u8; len];
        io.read_exact(&mut body).await.unwrap();
        let body = String::from_utf8(body).unwrap();
        let reply = |status: &str, v: Value| {
            let b = v.to_string();
            format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}",
                b.len()
            )
        };
        let out = match path.as_str() {
            "/api/onboarding" => {
                let done = seen.lock().unwrap().onboarded;
                reply("200 OK", json!([{ "step": "user", "done": done }]))
            }
            "/api/onboarding/users" => {
                let v: Value = serde_json::from_str(&body).unwrap();
                let mut s = seen.lock().unwrap();
                if s.onboarded {
                    reply("403 Forbidden", json!({}))
                } else {
                    assert_eq!(v["username"], USERNAME);
                    s.onboarded = true;
                    s.password = v["password"].as_str().map(str::to_string);
                    reply("200 OK", json!({ "auth_code": "code1" }))
                }
            }
            "/auth/token" => {
                let ok = body.contains("grant_type=authorization_code&code=code")
                    || (body.contains("grant_type=refresh_token")
                        && seen.lock().unwrap().refresh_ok);
                if ok {
                    reply(
                        "200 OK",
                        json!({ "access_token": "acc", "refresh_token": "ref" }),
                    )
                } else {
                    reply("400 Bad Request", json!({ "error": "invalid_grant" }))
                }
            }
            "/auth/login_flow" => reply("200 OK", json!({ "flow_id": "f1" })),
            "/auth/login_flow/f1" => {
                let v: Value = serde_json::from_str(&body).unwrap();
                let right = seen.lock().unwrap().password.as_deref() == v["password"].as_str();
                if right {
                    reply(
                        "200 OK",
                        json!({ "type": "create_entry", "result": "code2" }),
                    )
                } else {
                    reply(
                        "200 OK",
                        json!({ "type": "form", "errors": { "base": "invalid_auth" } }),
                    )
                }
            }
            p if p.starts_with("/api/onboarding/") => reply("200 OK", json!({})),
            "/api/websocket" => {
                io.get_mut()
                    .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                    .await
                    .unwrap();
                let send = |v: Value| {
                    let b = v.to_string().into_bytes();
                    let mut f = vec![0x81];
                    if b.len() < 126 {
                        f.push(b.len() as u8);
                    } else {
                        f.push(126);
                        f.extend_from_slice(&(b.len() as u16).to_be_bytes());
                    }
                    f.extend(b);
                    f
                };
                io.get_mut()
                    .write_all(&send(json!({"type":"auth_required"})))
                    .await
                    .unwrap();
                let auth = read_frame(&mut io).await.unwrap();
                assert_eq!(auth["access_token"], "acc");
                io.get_mut()
                    .write_all(&send(json!({"type":"auth_ok"})))
                    .await
                    .unwrap();
                while let Some(m) = read_frame(&mut io).await {
                    let id = m["id"].clone();
                    let ok = |r: Value| json!({"id":id,"type":"result","success":true,"result":r});
                    seen.lock().unwrap().commands.push(m.clone());
                    let answer = match m["type"].as_str().unwrap() {
                        "config/auth/create" => {
                            seen.lock().unwrap().created.push(m.clone());
                            let uid = if m["name"] == "evil" {
                                "a\"b"
                            } else {
                                "0123abcd"
                            };
                            ok(json!({"user":{"id":uid}}))
                        }
                        "config/auth/list" => ok(json!([{"id":"0123abcd"},{"id":"owner1"}])),
                        "backup/generate" => {
                            seen.lock().unwrap().busy = 2;
                            // an event on the way, before the answer
                            io.get_mut()
                                .write_all(&send(json!({"id":99,"type":"event","event":{}})))
                                .await
                                .unwrap();
                            ok(json!({"backup_job_id":"job1"}))
                        }
                        "backup/info" => {
                            let mut s = seen.lock().unwrap();
                            let state = if s.busy > 0 { "create_backup" } else { "idle" };
                            s.busy = s.busy.saturating_sub(1);
                            let result = if s.backup_fails {
                                "failed"
                            } else {
                                "completed"
                            };
                            ok(json!({
                                "state": state,
                                "backups": [{"backup_id": "b1"}],
                                "last_action_event": {"manager_state":"create_backup","state":result},
                            }))
                        }
                        // it goes down to restore, before it answers
                        "backup/restore" => return,
                        _ => {
                            json!({"id":id,"type":"result","success":false,"error":{"code":"unknown_command"}})
                        }
                    };
                    io.get_mut().write_all(&send(answer)).await.unwrap();
                }
                return;
            }
            _ => reply("404 Not Found", json!({})),
        };
        io.get_mut().write_all(out.as_bytes()).await.unwrap();
    }

    async fn read_frame(io: &mut BufReader<tokio::net::TcpStream>) -> Option<Value> {
        let mut h = [0u8; 2];
        io.read_exact(&mut h).await.ok()?;
        assert_eq!(h[1] & 0x80, 0x80, "a client masks");
        let len = match h[1] & 0x7f {
            126 => io.read_u16().await.ok()? as usize,
            n => n as usize,
        };
        let mut mask = [0u8; 4];
        io.read_exact(&mut mask).await.unwrap();
        let mut p = vec![0u8; len];
        io.read_exact(&mut p).await.unwrap();
        p.iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b ^= mask[i % 4]);
        Some(serde_json::from_slice(&p).unwrap())
    }

    fn quick(addr: SocketAddr) -> HomeAssistant {
        HomeAssistant {
            poll: std::time::Duration::from_millis(5),
            ..HomeAssistant::new(addr, vec![])
        }
    }

    #[tokio::test]
    async fn a_backup_is_asked_of_the_local_agent_and_waited_for() {
        let (addr, seen) = fake().await;
        let ha = quick(addr);
        let mut admin = HaAdmin {
            password: Some("pw".into()),
            refresh_token: None,
        };
        ha.setup(&mut admin).await.unwrap();
        ha.backup(&mut admin).await.unwrap();
        let c = seen.lock().unwrap().commands.clone();
        assert_eq!(c[0]["type"], "backup/generate");
        assert_eq!(c[0]["agent_ids"], json!(["backup.local"]));
        assert_eq!(c[0]["include_homeassistant"], true);
        assert_eq!(c[0]["include_database"], true);
        let polls = c.iter().filter(|m| m["type"] == "backup/info").count();
        assert_eq!(polls, 3, "until it is idle");
        seen.lock().unwrap().backup_fails = true;
        assert!(ha.backup(&mut admin).await.is_err());
    }

    #[tokio::test]
    async fn a_restore_is_of_a_backup_it_knows() {
        let (addr, seen) = fake().await;
        let ha = quick(addr);
        let mut admin = HaAdmin {
            password: Some("pw".into()),
            refresh_token: None,
        };
        ha.setup(&mut admin).await.unwrap();
        ha.restore(&mut admin, "b1").await.unwrap();
        let c = seen.lock().unwrap().commands.clone();
        let r = c.iter().find(|m| m["type"] == "backup/restore").unwrap();
        assert_eq!(r["backup_id"], "b1");
        assert_eq!(r["agent_id"], "backup.local");
        assert_eq!(r["restore_homeassistant"], true);
        assert_eq!(r["restore_database"], true);
        let unknown = HomeAssistant {
            poll: std::time::Duration::from_micros(10),
            ..HomeAssistant::new(addr, vec![])
        };
        assert!(unknown.restore(&mut admin, "nope").await.is_err());
    }

    #[tokio::test]
    async fn onboarding_then_a_user_per_person() {
        let (addr, seen) = fake().await;
        let ha = HomeAssistant::new(addr, vec![]);
        let mut admin = HaAdmin {
            password: Some("pw".into()),
            refresh_token: None,
        };
        ha.setup(&mut admin).await.unwrap();
        assert_eq!(seen.lock().unwrap().password.as_deref(), Some("pw"));
        assert_eq!(admin.refresh_token.as_deref(), Some("ref"));
        let id = ha
            .create_user(&mut admin, "ann", "system-admin")
            .await
            .unwrap();
        assert_eq!(id, "0123abcd");
        let c = seen.lock().unwrap().created[0].clone();
        assert_eq!(c["type"], "config/auth/create");
        assert_eq!(c["group_ids"], json!(["system-admin"]));
        assert_eq!(c["local_only"], true);
        assert_eq!(
            ha.users(&mut admin).await.unwrap(),
            vec!["0123abcd".to_string(), "owner1".to_string()]
        );
        // a second start: onboarded already, the token still good
        ha.setup(&mut admin).await.unwrap();
    }

    #[tokio::test]
    async fn a_lost_token_signs_in_again_and_a_wrong_password_does_not() {
        let (addr, seen) = fake().await;
        let ha = HomeAssistant::new(addr, vec![]);
        let mut admin = HaAdmin {
            password: Some("pw".into()),
            refresh_token: None,
        };
        ha.setup(&mut admin).await.unwrap();
        seen.lock().unwrap().refresh_ok = false;
        ha.setup(&mut admin).await.unwrap();
        let mut stranger = HaAdmin {
            password: Some("guess".into()),
            refresh_token: None,
        };
        assert!(ha.setup(&mut stranger).await.is_err());
    }

    #[tokio::test]
    async fn a_user_id_that_could_break_the_users_file_is_refused() {
        let (addr, _) = fake().await;
        let ha = HomeAssistant::new(addr, vec![]);
        let mut admin = HaAdmin {
            password: Some("pw".into()),
            refresh_token: None,
        };
        ha.setup(&mut admin).await.unwrap();
        assert!(
            ha.create_user(&mut admin, "evil", "system-users")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn the_restart_command_runs_and_its_failure_is_said() {
        let ok = HomeAssistant::new("127.0.0.1:1".parse().unwrap(), vec!["true".into()]);
        ok.restart().await.unwrap();
        let bad = HomeAssistant::new("127.0.0.1:1".parse().unwrap(), vec!["false".into()]);
        assert!(bad.restart().await.is_err());
    }
}
