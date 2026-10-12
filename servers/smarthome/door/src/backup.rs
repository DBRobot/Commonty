//! Backups (docs/dev/home-controller.md, Backups): Home Assistant's own
//! backup and the door's state in one tar, locked with the backup key K,
//! K sealed to every key the people in offer. No box can open one.
//!
//! ```text
//! "CBK1" | u32 BE header length | header JSON | secretbox(K, nonce, tar)
//! header  {nonce, sealed: [{to, key}], made}
//! tar     home-assistant.tar, door-state.json
//! ```

use crate::{App, Fut, state};
use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use crypto_secretbox::aead::AeadInPlace as _;
use crypto_secretbox::{KeyInit as _, XSalsa20Poly1305};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MAGIC: &[u8; 4] = b"CBK1";
pub const KEY_FILE: &str = "backup.key";
/// the most a backup file may be, uploaded or restored
pub const MAX_FILE: usize = 512 << 20;
pub const HA_TAR: &str = "home-assistant.tar";
pub const STATE_JSON: &str = "door-state.json";
const MAX_HEADER: usize = 1 << 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    /// 24 bytes, base64url unpadded
    pub nonce: String,
    pub sealed: Vec<Sealed>,
    pub made: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sealed {
    /// the ed25519 public key K is sealed to, as the entry lists it
    pub to: String,
    /// library::seal_to(to, K)
    pub key: String,
}

/// Where a sealed backup goes: the gate's presigned url, in the door.
pub trait Upload: Send + Sync {
    fn put(&self, file: Vec<u8>) -> Fut<'_, ()>;
}

/// The backup key: 32 random bytes, made once, beside the state.
pub fn backup_key(dir: &Path) -> Result<library::Key> {
    let path = dir.join(KEY_FILE);
    match std::fs::read(&path) {
        Ok(b) => {
            let k: [u8; 32] = b[..]
                .try_into()
                .map_err(|_| anyhow!("{} is not 32 bytes", path.display()))?;
            Ok(library::Key::new(k))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
            let k = library::random_key();
            keep_key(dir, &k)?;
            Ok(k)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn keep_key(dir: &Path, k: &[u8; 32]) -> Result<()> {
    state::write_atomic(&dir.join(KEY_FILE), k, 0o600)
}

/// Every key the people in offer: their devices' (the root and recovery
/// keys live offline, and a passkey holds none).
pub fn readers(s: &state::State) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in s.people.values() {
        let e = &p.entry.entry;
        // keys come only from devices (docs/dev/app.md): a passkey signs
        // in and holds none, so a backup is sealed to the devices alone
        for k in e.devices.iter().map(|d| d.public_key.clone()) {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

pub fn seal(k: &[u8; 32], to: &[String], mut tar: Vec<u8>, made: u64) -> Result<Vec<u8>> {
    ensure!(!to.is_empty(), "nobody to seal the backup to");
    let sealed = to
        .iter()
        .map(|t| {
            Ok(Sealed {
                to: t.clone(),
                key: library::seal_to(t, k).map_err(|e| anyhow!("sealing to {t}: {e}"))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).expect("randomness");
    let header = serde_json::to_vec(&Header {
        nonce: B64.encode(nonce),
        sealed,
        made,
    })?;
    XSalsa20Poly1305::new(crypto_secretbox::Key::from_slice(k))
        .encrypt_in_place(crypto_secretbox::Nonce::from_slice(&nonce), b"", &mut tar)
        .map_err(|_| anyhow!("sealing the backup"))?;
    let mut out = Vec::with_capacity(8 + header.len() + tar.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header.len() as u32).to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&tar);
    Ok(out)
}

/// The header, and where the sealed tar starts.
pub fn header(file: &[u8]) -> Result<(Header, usize)> {
    ensure!(
        file.len() >= 8 && &file[..4] == MAGIC,
        "not a Commonty backup"
    );
    let n = u32::from_be_bytes(file[4..8].try_into().unwrap()) as usize;
    ensure!(n <= MAX_HEADER && file.len() >= 8 + n, "a backup cut short");
    let h: Header = serde_json::from_slice(&file[8..8 + n]).context("the backup's header")?;
    Ok((h, 8 + n))
}

/// The tar, opened with K in place.
pub fn open(mut file: Vec<u8>, k: &[u8; 32]) -> Result<Vec<u8>> {
    let (h, at) = header(&file)?;
    let nonce: [u8; 24] = B64
        .decode(&h.nonce)
        .ok()
        .and_then(|n| n.try_into().ok())
        .ok_or_else(|| anyhow!("the backup's nonce is not one"))?;
    file.drain(..at);
    XSalsa20Poly1305::new(crypto_secretbox::Key::from_slice(k))
        .decrypt_in_place(crypto_secretbox::Nonce::from_slice(&nonce), b"", &mut file)
        .map_err(|_| anyhow!("the backup does not open with this key, or was changed"))?;
    Ok(file)
}

/// K and the tar, with one person's key: what the app does.
pub fn open_as(file: Vec<u8>, key: &identity::SigningKey) -> Result<(library::Key, Vec<u8>)> {
    let me = identity::encode_public(&key.verifying_key());
    let (h, _) = header(&file)?;
    let s = h
        .sealed
        .iter()
        .find(|s| s.to == me)
        .ok_or_else(|| anyhow!("this backup is not sealed to this key"))?;
    let k = library::open_with(key, &s.key).map_err(|e| anyhow!("{e}"))?;
    let tar = open(file, &k)?;
    Ok((k, tar))
}

/// A tar, as small as it can be: plain files, ustar headers.
pub mod tar {
    use anyhow::{Result, bail, ensure};
    use std::ops::Range;

    fn octal(field: &mut [u8], n: u64) {
        let w = field.len() - 1;
        let s = format!("{n:0w$o}");
        field[..w].copy_from_slice(s.as_bytes());
        field[w] = 0;
    }

    pub fn write(files: &[(&str, &[u8])], mtime: u64) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data) in files {
            assert!(name.len() < 100, "a short name");
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            octal(&mut h[100..108], 0o600);
            octal(&mut h[108..116], 0);
            octal(&mut h[116..124], 0);
            octal(&mut h[124..136], data.len() as u64);
            octal(&mut h[136..148], mtime);
            h[156] = b'0';
            h[257..263].copy_from_slice(b"ustar\0");
            h[263..265].copy_from_slice(b"00");
            h[148..156].fill(b' ');
            let sum: u64 = h.iter().map(|b| *b as u64).sum();
            octal(&mut h[148..155], sum);
            h[155] = b' ';
            out.extend_from_slice(&h);
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out.resize(out.len() + 1024, 0);
        out
    }

    fn number(f: &[u8]) -> Result<u64> {
        if f.first().is_some_and(|b| b & 0x80 != 0) {
            // base-256, as GNU and Python write a big size
            let mut n: u64 = (f[0] & 0x7f) as u64;
            for b in &f[1..] {
                ensure!(n >> 56 == 0, "a size too big");
                n = (n << 8) | *b as u64;
            }
            return Ok(n);
        }
        let s = std::str::from_utf8(f)?.trim_matches(|c| c == '\0' || c == ' ');
        if s.is_empty() {
            return Ok(0);
        }
        Ok(u64::from_str_radix(s, 8)?)
    }

    fn text(f: &[u8]) -> String {
        let end = f.iter().position(|b| *b == 0).unwrap_or(f.len());
        String::from_utf8_lossy(&f[..end]).into_owned()
    }

    /// The plain files in a tar: their names and where their bytes are.
    pub fn entries(t: &[u8]) -> Result<Vec<(String, Range<usize>)>> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 512 <= t.len() {
            let h = &t[at..at + 512];
            if h.iter().all(|b| *b == 0) {
                return Ok(out);
            }
            let sum: u64 = h
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    if (148..156).contains(&i) {
                        32
                    } else {
                        *b as u64
                    }
                })
                .sum();
            ensure!(number(&h[148..156])? == sum, "a damaged tar header");
            let size = usize::try_from(number(&h[124..136])?)?;
            let start = at + 512;
            let end = start
                .checked_add(size)
                .filter(|e| *e <= t.len())
                .ok_or_else(|| anyhow::anyhow!("a tar cut short"))?;
            let mut name = text(&h[..100]);
            if &h[257..262] == b"ustar" {
                let prefix = text(&h[345..500]);
                if !prefix.is_empty() {
                    name = format!("{prefix}/{name}");
                }
            }
            if matches!(h[156], b'0' | 0) {
                out.push((name, start..end));
            }
            at = start + size.div_ceil(512) * 512;
        }
        bail!("a tar with no end")
    }

    pub fn find<'a>(t: &'a [u8], name: &str) -> Result<Option<&'a [u8]>> {
        Ok(entries(t)?
            .into_iter()
            .find(|(n, _)| n.trim_start_matches("./") == name)
            .map(|(_, r)| &t[r]))
    }
}

/// Home Assistant's id for one of its backup files: backup.json's slug.
pub fn ha_backup_id(ha_tar: &[u8]) -> Result<String> {
    let j = tar::find(ha_tar, "backup.json")?
        .ok_or_else(|| anyhow!("not a Home Assistant backup: no backup.json"))?;
    let v: serde_json::Value = serde_json::from_slice(j).context("backup.json")?;
    let id = v["slug"]
        .as_str()
        .ok_or_else(|| anyhow!("backup.json names no backup"))?;
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "a backup id that is not one"
    );
    Ok(id.to_string())
}

/// The newest .tar in Home Assistant's backups, made since `since`.
fn newest(dir: &Path, since: std::time::SystemTime) -> Result<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let e = e?;
        let p = e.path();
        if p.extension().is_none_or(|x| x != "tar") {
            continue;
        }
        let m = e.metadata()?.modified()?;
        if best.as_ref().is_none_or(|(t, _)| m > *t) {
            best = Some((m, p));
        }
    }
    match best {
        Some((t, p)) if t + Duration::from_secs(5) >= since => Ok(p),
        _ => bail!("Home Assistant made no backup"),
    }
}

/// The door's state as a backup keeps it: never a session.
fn saved_state(s: &state::State) -> state::State {
    let mut s = s.clone();
    s.sessions.clear();
    s.backup = None;
    s
}

/// One backup, made, sealed and sent.
pub async fn run(app: &App) -> Result<()> {
    let readers = app.with_store(|s| -> Result<_> {
        ensure!(s.state.finished, "not paired yet");
        Ok(readers(&s.state))
    })?;
    ensure!(!readers.is_empty(), "nobody in has a key to seal it to");
    let started = std::time::SystemTime::now();
    {
        let _one = app.ha_work.lock().await;
        let mut admin = app.admin()?;
        let made = app.ha.backup(&mut admin).await;
        app.keep_admin(admin)?;
        made?;
    }
    let path = newest(&app.cfg.ha_backups, started)?;
    // after Home Assistant's: its admin as it is now
    let state = app.with_store(|s| saved_state(&s.state));
    let sent = async {
        let ha = tokio::fs::read(&path)
            .await
            .with_context(|| format!("reading {}", path.display()))?;
        let st = serde_json::to_vec_pretty(&state)?;
        let tar = tar::write(&[(HA_TAR, &ha), (STATE_JSON, &st)], identity::now());
        drop(ha);
        let k: [u8; 32] = **app.backup_key.lock().unwrap();
        let file = seal(&k, &readers, tar, identity::now())?;
        ensure!(file.len() <= MAX_FILE, "the backup is bigger than 512 MiB");
        app.upload.put(file).await
    }
    .await;
    // sent or not, the card keeps none: nightly ones would fill it
    let _ = tokio::fs::remove_file(&path).await;
    sent
}

/// Run one, and keep how it went for /_door/status.
/// Whether it went up.
pub async fn run_and_keep(app: &App) -> bool {
    let r = run(app).await;
    let ok = r.is_ok();
    if let Err(e) = &r {
        eprintln!("backup: {e:#}");
    } else {
        eprintln!("backup: sent");
    }
    let outcome = state::BackupOutcome {
        at: identity::now(),
        ok: r.is_ok(),
        error: r.err().map(|e| format!("{e:#}")),
    };
    let _ = app.with_store(|s| {
        s.state.backup = Some(outcome);
        s.save()
    });
    ok
}

/// Every night, the first an hour after start (DOOR_BACKUP_FIRST); and when
/// pairing finishes, and after a let-in or a remove (App::backup_now); a
/// failed one again after backup_retry. Nothing until the door is paired,
/// nor while a restore is running.
pub async fn schedule(app: std::sync::Arc<App>) {
    let mut next = tokio::time::Instant::now() + app.cfg.backup_first;
    loop {
        tokio::select! {
            _ = app.backup_now.notified() => {}
            _ = tokio::time::sleep_until(next) => {
                next = tokio::time::Instant::now() + app.cfg.backup_every;
            }
        }
        // never while a restore is putting the home back: a backup then
        // would be of a home half restored
        let restoring = matches!(&*app.restoring.lock().unwrap(), Some(r) if r.state == "working");
        // one that did not go up (Home Assistant restarting, the network
        // not there yet) is tried again soon, not the next night
        if app.with_store(|s| s.state.finished) && !restoring && !run_and_keep(&app).await {
            next = next.min(tokio::time::Instant::now() + app.cfg.backup_retry);
        }
    }
}

/// What a backup holds, checked and read before anything is changed.
pub struct Opened {
    pub tar: Vec<u8>,
    pub ha: std::ops::Range<usize>,
    pub ha_id: String,
    pub state: state::State,
}

pub fn read(tar: Vec<u8>) -> Result<Opened> {
    let mut ha = None;
    let mut st = None;
    for (n, r) in tar::entries(&tar)? {
        match n.as_str() {
            HA_TAR => ha = Some(r),
            STATE_JSON => st = Some(r),
            _ => {}
        }
    }
    let ha = ha.ok_or_else(|| anyhow!("the backup holds no Home Assistant backup"))?;
    let st = st.ok_or_else(|| anyhow!("the backup holds no door state"))?;
    let state: state::State =
        serde_json::from_slice(&tar[st]).context("the backup's door state")?;
    for (name, p) in &state.people {
        ensure!(
            p.entry.entry.name == *name && p.root == p.entry.entry.root,
            "the backup's people do not add up"
        );
        identity::accept(None, &p.entry)
            .map_err(|_| anyhow!("{name}'s entry in the backup is not signed by its root"))?;
    }
    let ha_id = ha_backup_id(&tar[ha.clone()])?;
    Ok(Opened {
        tar,
        ha,
        ha_id,
        state,
    })
}

/// How a restore is going, for /pair/status.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Restoring {
    /// working, done or failed
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Home Assistant's backup back in its place, then the people, and K.
pub async fn restore(app: &App, o: Opened, k: library::Key) -> Result<()> {
    let _one = app.ha_work.lock().await;
    let dir = &app.cfg.ha_backups;
    tokio::fs::create_dir_all(dir).await?;
    let path = dir.join(format!("commonty-restore-{}.tar", o.ha_id));
    state::write_atomic(&path, &o.tar[o.ha.clone()], 0o644)?;
    // Home Assistant's own files: its user reads and later removes it
    if let Ok(m) = std::fs::metadata(dir) {
        use std::os::unix::fs::MetadataExt as _;
        let _ = std::os::unix::fs::chown(&path, Some(m.uid()), Some(m.gid()));
    }
    drop(o.tar);
    let mut admin = app.admin()?;
    let ready = app.ha.setup(&mut admin).await;
    app.keep_admin(admin.clone())?;
    ready?;
    // it reads its backups directory when it starts
    app.ha.restart().await?;
    let restored = app.ha.restore(&mut admin, &o.ha_id).await;
    app.keep_admin(admin)?;
    restored?;
    let saved = o.state;
    app.with_store(|s| -> Result<()> {
        let pairer = s
            .state
            .pairer
            .clone()
            .and_then(|n| s.state.people.get(&n).map(|p| (n, p.clone())));
        let mut people = saved.people.clone();
        let mut retired = saved.retired.clone();
        if let Some((name, now)) = pairer {
            match people.get_mut(&name) {
                Some(old)
                    if old.root == now.root
                        || identity::accept(Some(&old.entry), &now.entry).is_ok() =>
                {
                    if old.root != now.root || old.entry.entry.version < now.entry.entry.version {
                        old.root = now.root.clone();
                        old.entry = now.entry.clone();
                    }
                }
                old => {
                    // someone else by that name, saved: the one who paired now is in
                    if let Some(a) = old.and_then(|o| o.address) {
                        retired.insert(a);
                    }
                    people.insert(
                        name,
                        state::Person {
                            ha_user: None,
                            address: None,
                            ..now
                        },
                    );
                }
            }
        }
        s.state.people = people;
        s.state.retired = retired;
        s.state.ha = saved.ha.clone();
        s.save()
    })?;
    keep_key(&app.cfg.state_dir, &k)?;
    *app.backup_key.lock().unwrap() = k;
    app.write_users().await?;
    app.reconcile.notify_one();
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tests::*;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    #[derive(Default)]
    pub struct FakeUpload {
        pub files: Mutex<Vec<Vec<u8>>>,
        pub down: std::sync::atomic::AtomicBool,
    }
    impl Upload for FakeUpload {
        fn put(&self, file: Vec<u8>) -> Fut<'_, ()> {
            Box::pin(async move {
                ensure!(
                    !self.down.load(Ordering::SeqCst),
                    "the gate is not answering"
                );
                self.files.lock().unwrap().push(file);
                Ok(())
            })
        }
    }

    /// A Home Assistant backup as Home Assistant writes one: a tar with
    /// backup.json beside the archives.
    pub fn ha_backup(id: &str) -> Vec<u8> {
        let json = serde_json::json!({ "slug": id, "name": "Commonty" }).to_string();
        tar::write(
            &[
                ("./backup.json", json.as_bytes()),
                ("./homeassistant.tar.gz", b"the home"),
            ],
            1,
        )
    }

    fn finished(r: &Rig) {
        r.app.with_store(|s| {
            s.state.finished = true;
            s.save().unwrap()
        });
    }

    #[test]
    fn a_tar_reads_back_and_a_damaged_one_does_not() {
        let big = vec![7u8; 1500];
        let t = tar::write(&[("a", b"one"), ("b/c.json", &big)], 5);
        assert_eq!(t.len() % 512, 0);
        let e = tar::entries(&t).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(&t[e[0].1.clone()], b"one");
        assert_eq!(e[1].0, "b/c.json");
        assert_eq!(&t[e[1].1.clone()], &big[..]);
        let mut bad = t.clone();
        bad[3] ^= 1;
        assert!(tar::entries(&bad).is_err(), "checksum");
        assert!(tar::entries(&t[..700]).is_err(), "cut short");
        assert_eq!(ha_backup_id(&ha_backup("abc123")).unwrap(), "abc123");
        assert!(ha_backup_id(&ha_backup("../x")).is_err());
    }

    #[test]
    fn a_backup_opens_with_any_one_persons_key_and_nothing_else() {
        let (ann, ak) = signed_entry("ann");
        let (bob, bk) = signed_entry("bob");
        let k = library::random_key();
        let to = vec![ann.entry.root.clone(), bob.entry.root.clone()];
        let tar = tar::write(&[("x", b"hello")], 1);
        let file = seal(&k, &to, tar.clone(), 9).unwrap();
        assert_eq!(&file[..4], b"CBK1");
        let (h, _) = header(&file).unwrap();
        assert_eq!(h.made, 9);
        assert_eq!(h.sealed.len(), 2);
        assert!(!file.windows(5).any(|w| w == b"hello"), "sealed");

        let (k2, opened) = open_as(file.clone(), &bk).unwrap();
        assert_eq!(*k2, *k);
        assert_eq!(opened, tar);
        assert_eq!(open_as(file.clone(), &ak).unwrap().1, tar);
        assert!(open_as(file.clone(), &identity::generate()).is_err());
        assert!(open(file.clone(), &[1; 32]).is_err(), "the wrong K");
        // a byte changed anywhere in the sealed part
        let mut bad = file.clone();
        let n = bad.len() - 3;
        bad[n] ^= 1;
        assert!(open(bad, &k).is_err());
        // a header that lies about its length, or another file
        let mut bad = file.clone();
        bad[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(open(bad, &k).is_err());
        assert!(open(b"PK\x03\x04 a zip".to_vec(), &k).is_err());
        // ann's K swapped for another: hers opens, the file does not
        let (mut h, at) = header(&file).unwrap();
        h.sealed[0].key = library::seal_to(&ann.entry.root, &[3; 32]).unwrap();
        let h = serde_json::to_vec(&h).unwrap();
        let mut wrong = b"CBK1".to_vec();
        wrong.extend_from_slice(&(h.len() as u32).to_be_bytes());
        wrong.extend_from_slice(&h);
        wrong.extend_from_slice(&file[at..]);
        assert!(open_as(wrong.clone(), &ak).is_err());
        assert!(open_as(wrong, &bk).is_ok(), "bob's is still his");
        assert!(seal(&k, &[], vec![], 1).is_err(), "nobody to seal to");
    }

    #[test]
    fn the_readers_are_the_devices_of_the_people_in() {
        let r = rig("readers");
        let (ann, _, _) = person(&r.app, "ann");
        let (bob, _, _) = person(&r.app, "bob");
        let got = r.app.with_store(|s| readers(&s.state));
        let mut want = vec![
            ann.entry.devices[0].public_key.clone(),
            bob.entry.devices[0].public_key.clone(),
        ];
        want.sort();
        let mut got = got;
        got.sort();
        assert_eq!(got, want, "a device each, and no passkey");
    }

    #[tokio::test]
    async fn a_backup_is_made_sealed_sent_and_the_cards_copy_removed() {
        let r = rig("backup");
        let (_, ak, _) = person(&r.app, "ann");
        finished(&r);
        run_and_keep(&r.app).await;
        let outcome = r.app.with_store(|s| s.state.backup.clone()).unwrap();
        assert!(outcome.ok, "{outcome:?}");
        let files = r.upload.files.lock().unwrap().clone();
        assert_eq!(files.len(), 1);
        let (_, tar) = open_as(files[0].clone(), &ak).unwrap();
        let o = read(tar).unwrap();
        assert_eq!(o.ha_id, "fake0001");
        assert!(o.state.people.contains_key("ann"));
        assert!(o.state.sessions.is_empty(), "never a session");
        assert!(o.state.ha.password.is_some(), "Home Assistant's admin");
        let left: Vec<_> = std::fs::read_dir(&r.app.cfg.ha_backups).unwrap().collect();
        assert!(left.is_empty(), "the card keeps none");
        assert_eq!(*r.ha.backups.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_failed_backup_is_kept_as_failed() {
        let r = rig("backup-no");
        person(&r.app, "ann");
        run_and_keep(&r.app).await;
        let o = r.app.with_store(|s| s.state.backup.clone()).unwrap();
        assert!(!o.ok);
        assert_eq!(o.error.as_deref(), Some("not paired yet"));
        finished(&r);
        r.upload.down.store(true, Ordering::SeqCst);
        run_and_keep(&r.app).await;
        let o = r.app.with_store(|s| s.state.backup.clone()).unwrap();
        assert_eq!(o.error.as_deref(), Some("the gate is not answering"));
        r.upload.down.store(false, Ordering::SeqCst);
        r.ha.down.store(true, Ordering::SeqCst);
        run_and_keep(&r.app).await;
        let o = r.app.with_store(|s| s.state.backup.clone()).unwrap();
        assert!(!o.ok);
        assert!(r.upload.files.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_schedule_runs_after_its_first_wait_and_when_asked() {
        let r = rig_with("backup-when", |c| {
            c.backup_first = Duration::from_millis(200);
            c.backup_every = Duration::from_secs(3600);
        });
        person(&r.app, "ann");
        finished(&r);
        tokio::spawn(schedule(r.app.clone()));
        let sent = || r.upload.files.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(sent(), 0, "not at once");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(sent(), 1, "after the first wait");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(sent(), 1, "then not until the night after");
        r.app.backup_now.notify_one();
        for _ in 0..50 {
            if sent() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(sent(), 2, "a let-in or remove asks for one");
    }

    #[tokio::test]
    async fn a_backup_that_failed_is_tried_again_soon() {
        let r = rig_with("backup-retry", |c| {
            c.backup_first = Duration::from_secs(3600);
            c.backup_every = Duration::from_secs(3600);
            c.backup_retry = Duration::from_millis(200);
        });
        person(&r.app, "ann");
        finished(&r);
        // Home Assistant restarting as pairing finishes: the first one fails
        r.ha.down.store(true, Ordering::SeqCst);
        tokio::spawn(schedule(r.app.clone()));
        r.app.backup_now.notify_one();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!r.app.with_store(|s| s.state.backup.clone()).unwrap().ok);
        r.ha.down.store(false, Ordering::SeqCst);
        let sent = || r.upload.files.lock().unwrap().len();
        for _ in 0..50 {
            if sent() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(sent(), 1, "tried again after the retry, not the next night");
    }

    #[tokio::test]
    async fn a_let_in_and_a_remove_ask_for_a_backup() {
        let r = rig("backup-people");
        let (_, _, ann) = person(&r.app, "ann");
        person(&r.app, "bob");
        let url = serve(crate::router(r.app.clone())).await;
        let res = client()
            .post(format!("{url}/_door/remove"))
            .header("cookie", &ann)
            .json(&serde_json::json!({"name":"bob"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204);
        tokio::time::timeout(Duration::from_secs(1), r.app.backup_now.notified())
            .await
            .expect("asked for");
    }
}
