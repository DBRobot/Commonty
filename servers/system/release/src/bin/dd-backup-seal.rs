//! A box that joined makes its backup's password and seals it to its owner's
//! keys, so a dead box's backup can still be opened (nix/modules/storage/backup-seal.nix).
//! Who the owner is, and their keys, are the agreed identity state's word on
//! this box's own dd-control, the judge's after checking their entry. Nothing
//! signs the copy: a device opening it takes it only to read this box's
//! repository, never to encrypt anything of its own (client/account
//! open_for_device, which refuses a box's copy, is for those).

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use clap::Parser;
use serde_json::{Value, json};

const FOR: &str = "box-backup";

#[derive(Parser)]
struct Args {
    /// this box's name in the agreed state
    #[arg(long = "box")]
    name: String,
    #[arg(long)]
    password_file: PathBuf,
    /// where the sealed copy goes
    #[arg(long)]
    sealed: PathBuf,
    /// this box's dd-control
    #[arg(long, default_value = "http://127.0.0.1:4195")]
    control: String,
}

fn main() {
    let args = Args::parse();
    match run(&args) {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("dd-backup-seal: {e:#}");
            std::process::exit(1);
        }
    }
}

fn run(args: &Args) -> Result<String> {
    let body = ureq::get(format!("{}/identity", args.control))
        .call()
        .context("the agreed identity state")?
        .into_body()
        .read_to_string()
        .context("the agreed identity state")?;
    let view: Value = serde_json::from_str(&body).context("the agreed identity state")?;
    seal_here(&view, &args.name, &args.password_file, &args.sealed)
}

/// What this does with what the identity state says; the files are its own.
fn seal_here(view: &Value, name: &str, password: &Path, sealed: &Path) -> Result<String> {
    // no copy where the backup looks for one, and it stops; kept beside it, it
    // says whose the password was, whoever comes next
    let was_file = sealed.with_extension("was");
    let Some(owner) = view["enlisted"][name]["owner"].as_str() else {
        if sealed.exists() {
            fs::rename(sealed, &was_file)?;
            return Ok("no owner any more: no backup".into());
        }
        return Ok("no owner: nothing to seal, and no backup".into());
    };
    let read = |p: &Path| -> Option<Value> {
        fs::read(p)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    };
    let before = read(sealed);
    if let Some(was) = before
        .as_ref()
        .or(read(&was_file).as_ref())
        .and_then(|d| d["owner"].as_str())
        && was != owner
    {
        if sealed.exists() {
            fs::rename(sealed, &was_file)?;
        }
        bail!("this box was {was}'s and is {owner}'s now: no backup under {was}'s password");
    }
    let keys = keys_of(&view["members"][owner]);
    if keys.is_empty() {
        bail!("{owner} has no key to seal to");
    }
    if before.as_ref().is_some_and(|d| {
        d["keys"]
            .as_object()
            .is_some_and(|k| k.keys().eq(keys.iter().map(|(to, _)| to)))
    }) {
        return Ok(format!("sealed to {owner}'s keys already"));
    }
    let secret = password_bytes(password)?;
    let mut out = serde_json::Map::new();
    for (to, public) in keys {
        out.insert(
            to,
            Value::String(library::seal_to(&public, &secret).map_err(|e| anyhow::anyhow!("{e}"))?),
        );
    }
    let doc = json!({ "for": FOR, "box": name, "owner": owner, "keys": out });
    let tmp = sealed.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec(&doc)?)?;
    fs::rename(&tmp, sealed)?;
    Ok(format!("sealed to {owner}'s keys"))
}

/// Everyone a member's password is sealed to, by label, sorted as a JSON map's
/// keys are: as a library's key is (library::seal_for_entry)
fn keys_of(member: &Value) -> Vec<(String, String)> {
    let mut out = vec![];
    for d in member["devices"].as_array().into_iter().flatten() {
        if let (Some(fp), Some(k)) = (d["fingerprint"].as_str(), d["public_key"].as_str()) {
            out.push((format!("device:{fp}"), k.to_string()));
        }
    }
    if let Some(root) = member["root"].as_str()
        && !root.is_empty()
        && !root.starts_with(identity::WEBAUTHN_ROOT)
    {
        out.push(("root".to_string(), root.to_string()));
    }
    if let Some(r) = member["recovery"].as_str()
        && !r.is_empty()
    {
        out.push(("recovery".to_string(), r.to_string()));
    }
    out.sort();
    out
}

/// The password's 32 bytes, made once. Restic reads it as base64, as an
/// opener hands it: the file is the text
fn password_bytes(path: &Path) -> Result<Vec<u8>> {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    if let Ok(text) = fs::read_to_string(path) {
        return b64.decode(text.trim()).context("the password file");
    }
    let id = [library::random_id(), library::random_id()].concat();
    let mut raw = vec![0u8; 32];
    for (i, b) in raw.iter_mut().enumerate() {
        *b = u8::from_str_radix(&id[i * 2..i * 2 + 2], 16)?;
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o400)
        .open(&tmp)?;
    f.write_all(b64.encode(&raw).as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dd-seal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn key() -> (ed25519_dalek::SigningKey, String) {
        let k = identity::generate();
        let public = identity::encode_public(&k.verifying_key());
        (k, public)
    }

    fn view(owner: Option<&str>, devices: &[(&str, &str)], root: &str) -> Value {
        let mut v = json!({ "members": { "ada": {
            "root": root,
            "recovery": "",
            "devices": devices.iter().map(|(f, k)| json!({"fingerprint": f, "public_key": k})).collect::<Vec<_>>(),
        }}});
        if let Some(o) = owner {
            v["enlisted"]["jb"] = json!({ "owner": o });
        }
        v
    }

    fn open(doc: &Value, to: &str, k: &ed25519_dalek::SigningKey) -> Option<Vec<u8>> {
        library::open_bytes_with(k, doc["keys"][to].as_str()?)
            .ok()
            .map(|b| b.to_vec())
    }

    #[test]
    fn a_stranger_makes_no_password() {
        let d = dir("stranger");
        let (p, s) = (d.join("password"), d.join("sealed"));
        let msg = seal_here(&view(None, &[], ""), "jb", &p, &s).unwrap();
        assert!(msg.contains("no owner"), "{msg}");
        assert!(!p.exists() && !s.exists());
    }

    #[test]
    fn the_owner_opens_it_with_the_box_gone_and_nobody_else_does() {
        let d = dir("owner");
        let (p, s) = (d.join("password"), d.join("sealed"));
        let (phone, phone_pub) = key();
        let (root, root_pub) = key();
        let (other, _) = key();
        seal_here(
            &view(Some("ada"), &[("fp", &phone_pub)], &root_pub),
            "jb",
            &p,
            &s,
        )
        .unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(&s).unwrap()).unwrap();
        assert_eq!(doc["for"], FOR);
        assert_eq!(doc["owner"], "ada");
        // what restic was given
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let written = fs::read_to_string(&p).unwrap();
        let opened = open(&doc, "device:fp", &phone).unwrap();
        assert_eq!(b64.encode(&opened), written);
        assert_eq!(open(&doc, "root", &root).unwrap(), opened);
        assert_eq!(opened.len(), 32);
        assert!(open(&doc, "device:fp", &other).is_none());
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o400
        );
    }

    #[test]
    fn a_new_device_gets_the_same_password_and_a_passkey_root_none() {
        let d = dir("device");
        let (p, s) = (d.join("password"), d.join("sealed"));
        let (_, one) = key();
        let (two, two_pub) = key();
        let root = format!("{}x", identity::WEBAUTHN_ROOT);
        seal_here(&view(Some("ada"), &[("a", &one)], &root), "jb", &p, &s).unwrap();
        let first = fs::read(&s).unwrap();
        // nothing changed: the same copy, not another
        let again = seal_here(&view(Some("ada"), &[("a", &one)], &root), "jb", &p, &s).unwrap();
        assert!(again.contains("already"), "{again}");
        assert_eq!(fs::read(&s).unwrap(), first);
        let written = fs::read_to_string(&p).unwrap();
        seal_here(
            &view(Some("ada"), &[("a", &one), ("b", &two_pub)], &root),
            "jb",
            &p,
            &s,
        )
        .unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(&s).unwrap()).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), written);
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        assert_eq!(b64.encode(open(&doc, "device:b", &two).unwrap()), written);
        assert!(doc["keys"].get("root").is_none());
    }

    #[test]
    fn a_new_owner_gets_nothing_of_the_old_owners() {
        let d = dir("hands");
        let (p, s) = (d.join("password"), d.join("sealed"));
        let (_, one) = key();
        seal_here(&view(Some("ada"), &[("a", &one)], ""), "jb", &p, &s).unwrap();
        let mut v = view(Some("bob"), &[("a", &one)], "");
        v["members"]["bob"] = v["members"]["ada"].clone();
        let e = seal_here(&v, "jb", &p, &s).unwrap_err();
        assert!(format!("{e}").contains("ada's"), "{e}");
        // and so the backup stops (ConditionPathExists)
        assert!(!s.exists());
        // and stays stopped, whatever order it comes in
        assert!(seal_here(&v, "jb", &p, &s).is_err());
    }

    #[test]
    fn unenlisted_it_stops_and_the_next_owner_still_gets_nothing() {
        let d = dir("gone");
        let (p, s) = (d.join("password"), d.join("sealed"));
        let (_, one) = key();
        seal_here(&view(Some("ada"), &[("a", &one)], ""), "jb", &p, &s).unwrap();
        let msg = seal_here(&view(None, &[], ""), "jb", &p, &s).unwrap();
        assert!(msg.contains("no owner any more"), "{msg}");
        assert!(!s.exists());
        let mut bob = view(Some("bob"), &[("a", &one)], "");
        bob["members"]["bob"] = bob["members"]["ada"].clone();
        assert!(seal_here(&bob, "jb", &p, &s).is_err());
        assert!(!s.exists());
        // she enlists it again: it goes on, same password
        seal_here(&view(Some("ada"), &[("a", &one)], ""), "jb", &p, &s).unwrap();
        assert!(s.exists());
    }

    #[test]
    fn no_key_to_seal_to_is_no_password() {
        let d = dir("nokey");
        let (p, s) = (d.join("password"), d.join("sealed"));
        assert!(seal_here(&view(Some("ada"), &[], ""), "jb", &p, &s).is_err());
        assert!(!p.exists());
    }
}
