//! A member's own identity, from any of their devices: a root key here or
//! on a phone, a recovery key on paper, and the entry they sign with it,
//! published to every directory they name. `dd` and the app share this.
//!
//! No server is asked for anything but storage. A directory that lies can
//! withhold or replay, and publishing to more than one is how that is caught.

use anyhow::{Context, Result, bail};
use auth::KeyStore;
use directory::{Took, fetch, newest};
use identity::{Device, Entry, SignedEntry};

/// keyring account holding the name the device key signs for
pub const USER: &str = "user";
/// keyring account holding the root secret, base64
pub const ROOT: &str = "identity-root";

/// what every directory said to the last publish, for a caller that shows
/// it; a publish that any directory did not take is an error with the
/// verdicts in it, because a directory left behind serves a stale entry
pub async fn publish(dirs: &[String], signed: &SignedEntry) -> Result<Vec<(String, Took)>> {
    let took = directory::publish(dirs, signed).await?;
    let bad: Vec<String> = took
        .iter()
        .filter_map(|(d, t)| match t {
            Took::Accepted => None,
            Took::Refused(why) => Some(format!("{d}: refused ({why})")),
            Took::Unreachable(why) => Some(format!("{d}: unreachable ({why})")),
        })
        .collect();
    if !bad.is_empty() {
        bail!(
            "{} of {} directories did not take the update:\n  {}",
            bad.len(),
            dirs.len(),
            bad.join("\n  ")
        );
    }
    Ok(took)
}

/// A code, as a person was handed it: the invite it derives, fetched from
/// a directory and checked against the release key there, and the proof
/// for a root, ready to go into that root's entry as its grant.
pub async fn redeem(dirs: &[String], code: &str, root_public: &str) -> Result<identity::Grant> {
    use base64::Engine as _;
    let code = identity::normalize_code(code);
    let key = identity::code_key(&code);
    let public = identity::encode_public(&key.verifying_key());
    let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public.as_bytes());
    let http = directory::http()?;
    let mut invite = None;
    let mut why = Vec::new();
    for d in dirs {
        let base = d.trim_end_matches('/');
        let base = base.strip_suffix("/_dd/directory").unwrap_or(base);
        match http.get(format!("{base}/_dd/invite/{id}")).send().await {
            Ok(r) if r.status().is_success() => {
                invite = r.json::<identity::SignedInvite>().await.ok();
                break;
            }
            Ok(r) => why.push(format!("{d}: {}", r.status())),
            Err(e) => why.push(format!("{d}: {e}")),
        }
    }
    let invite = invite.with_context(|| {
        format!(
            "that code is not valid here, or it has expired:\n  {}",
            why.join("\n  ")
        )
    })?;
    let redeemed = identity::now();
    if redeemed >= invite.invite.expires {
        bail!("that code has expired");
    }
    let proof = identity::prove(&key, &invite.invite.public_key, root_public, redeemed);
    Ok(identity::Grant {
        invite,
        redeemed,
        proof,
    })
}

pub fn load_root(keys: &impl KeyStore) -> Result<Option<ed25519_dalek::SigningKey>> {
    match keys.get(ROOT)? {
        Some(s) => Ok(Some(identity::decode_secret(&s)?)),
        None => Ok(None),
    }
}

/// a library into our entry: the key was sealed already, this signs it in
pub async fn add_library(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
    lib: identity::Library,
) -> Result<SignedEntry> {
    let cur = ours(dirs, name, root).await?;
    let mut entry = cur.entry.clone();
    entry.libraries.push(lib);
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(signed)
}

pub fn device_of(kp: &auth::device::KeyPair) -> Device {
    let public_key = auth::device::public_b64(kp);
    Device {
        fingerprint: identity::fingerprint(&public_key),
        public_key,
        added: identity::now(),
    }
}

/// A brand new identity: root and recovery generated here, this device the
/// first and only one, version 1, signed by the root. Returns the recovery
/// secret exactly once, for paper.
pub async fn create(
    keys: &impl KeyStore,
    dirs: &[String],
    name: &str,
    kp: &auth::device::KeyPair,
    code: Option<&str>,
) -> Result<zeroize::Zeroizing<String>> {
    if keys.get(ROOT)?.is_some() {
        bail!("this machine already holds an identity - `dd identity show`");
    }
    if !identity::valid_name(name) {
        bail!("{name:?} is not a valid name: lowercase letters, digits, - _ .");
    }
    let found = fetch(dirs, name).await;
    for (d, r) in &found {
        match r {
            Ok(Some(_)) => bail!(
                "{d} already has an entry for {name}. If it is yours, `dd identity import` \
                 the root here or `dd identity recover` with the paper key."
            ),
            Ok(None) => {}
            Err(e) => bail!("{d}: {e} - not creating an identity blind"),
        }
    }
    let root = identity::generate();
    let recovery = identity::generate();
    let root_public = identity::encode_public(&root.verifying_key());
    let grant = match code {
        Some(c) => Some(redeem(dirs, c, &root_public).await?),
        None => None,
    };
    let entry = Entry {
        name: name.to_string(),
        root: root_public,
        recovery: identity::encode_public(&recovery.verifying_key()),
        devices: vec![device_of(kp)],
        passkeys: vec![],
        grant,
        libraries: vec![],
        version: 1,
        updated: identity::now(),
    };
    let signed = identity::sign(entry, &root)?;
    publish(dirs, &signed).await?;
    keys.set(ROOT, &identity::encode_secret(&root))?;
    Ok(identity::encode_secret(&recovery))
}

/// The entry as the directories have it, insisting it is ours: signed by the
/// root this machine holds. A directory serving somebody else's entry under
/// our name, or a forgery, fails here rather than getting a signature from us.
pub async fn ours(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
) -> Result<SignedEntry> {
    let found = fetch(dirs, name).await;
    // verified, and the directories have to agree - but not that the root
    // is the one in hand. After a recovery run elsewhere this machine's
    // root is legitimately the old one, and finding that out is part of
    // what this call is for.
    if let Err(e) = directory::resolve(&found, directory::Anchor::FirstSight)
        && found.iter().any(|(_, r)| matches!(r, Ok(Some(_))))
    {
        bail!("{e:#}");
    }
    let _ = root;
    let cur = newest(&found).with_context(|| {
        let why: Vec<String> = found
            .iter()
            .map(|(d, r)| match r {
                Ok(None) => format!("{d}: no entry"),
                Ok(Some(_)) => unreachable!(),
                Err(e) => format!("{d}: {e:#}"),
            })
            .collect();
        format!(
            "no directory has an entry for {name}:\n  {}",
            why.join("\n  ")
        )
    })?;
    let mine = identity::encode_public(&root.verifying_key());
    if cur.entry.root != mine {
        bail!(
            "the entry for {name} is under a different root ({}) than this machine's ({})",
            identity::fingerprint(&cur.entry.root),
            identity::fingerprint(&mine)
        );
    }
    identity::verify(&cur).context("the entry under our root does not carry a valid signature")?;
    Ok(cur)
}

/// Add a device by its public key, signed by the root here.
pub async fn admit(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
    public_key: &str,
) -> Result<SignedEntry> {
    identity::decode_public(public_key).context("that is not an ed25519 public key")?;
    let cur = ours(dirs, name, root).await?;
    let mut entry = cur.entry.clone();
    if entry.devices.iter().any(|d| d.public_key == public_key) {
        bail!("that device is already in the entry");
    }
    entry.devices.push(Device {
        fingerprint: identity::fingerprint(public_key),
        public_key: public_key.to_string(),
        added: identity::now(),
    });
    // the new device opens every library this root can
    for lib in &mut entry.libraries {
        let key = library::open_library(lib, None, Some(root))
            .with_context(|| format!("library {}: the root does not open it", lib.id))?;
        lib.keys.push(library::SealedKey {
            to: format!("device:{}", identity::fingerprint(public_key)),
            sealed: library::seal_to(public_key, &key[..])?,
        });
    }
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(signed)
}

/// Remove a device, signed by the root here: it leaves the list and every
/// library key sealed to it goes with it. What it cached it keeps, as any
/// lost device would; a library shared with it is rotated by its owner.
pub async fn remove_device(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
    fingerprint: &str,
) -> Result<SignedEntry> {
    let cur = ours(dirs, name, root).await?;
    let mut entry = cur.entry.clone();
    // the whole fingerprint, or enough of its start to name one device
    let hits: Vec<String> = entry
        .devices
        .iter()
        .filter(|d| d.fingerprint.starts_with(fingerprint))
        .map(|d| d.fingerprint.clone())
        .collect();
    let fingerprint = match hits.as_slice() {
        [one] => one.clone(),
        [] => bail!("no device {fingerprint} in the entry"),
        _ => bail!("{fingerprint} starts more than one device's fingerprint; give more of it"),
    };
    if entry.devices.len() == 1 && !entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        bail!("that is the last device, and an entry needs one");
    }
    entry.devices.retain(|d| d.fingerprint != fingerprint);
    let sealed_to = format!("device:{fingerprint}");
    for lib in &mut entry.libraries {
        lib.keys.retain(|k| k.to != sealed_to);
    }
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(signed)
}

/// A passkey in the entry gains the key a browser derived from it, and
/// every library key is sealed to that key as well: from then on any
/// browser holding that passkey opens the member's libraries.
pub async fn link_passkey(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
    id: &str,
    library_key: &str,
) -> Result<SignedEntry> {
    identity::decode_public(library_key).context("that is not an ed25519 public key")?;
    let cur = ours(dirs, name, root).await?;
    let mut entry = cur.entry.clone();
    let Some(p) = entry.passkeys.iter_mut().find(|p| p.id == id) else {
        bail!("no passkey {id} in the entry");
    };
    if p.library_key.as_deref() == Some(library_key) {
        bail!("that passkey already has this key");
    }
    p.library_key = Some(library_key.to_string());
    let sealed_to = format!("passkey:{id}");
    for lib in &mut entry.libraries {
        let key = library::open_library(lib, None, Some(root))
            .with_context(|| format!("library {}: the root does not open it", lib.id))?;
        lib.keys.retain(|k| k.to != sealed_to);
        lib.keys.push(library::SealedKey {
            to: sealed_to.clone(),
            sealed: library::seal_to(library_key, &key[..])?,
        });
    }
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(signed)
}

/// Recovery: the paper key installs a new root and a new recovery key, and
/// the device list starts over with this device alone. Everything a lost or
/// stolen device could sign is dead the moment a directory takes this.
pub async fn recover(
    keys: &impl KeyStore,
    dirs: &[String],
    name: &str,
    recovery_secret: &str,
    kp: &auth::device::KeyPair,
) -> Result<zeroize::Zeroizing<String>> {
    let recovery = identity::decode_secret(recovery_secret).context("bad recovery key")?;
    let found = fetch(dirs, name).await;
    // Verified, and every directory asked has to agree about whose root
    // this name has. What follows reseals library keys out of this entry
    // and co-signs the result with the real recovery key, so a made-up one
    // would be the fleet accepting a key the liar chose. The recovery key
    // matching is not the check it looks like: it is public.
    let cur = directory::resolve(&found, directory::Anchor::FirstSight)
        .with_context(|| format!("no directory has a usable entry for {name}"))?;
    if cur.entry.recovery != identity::encode_public(&recovery.verifying_key()) {
        bail!("that recovery key does not match the entry for {name}");
    }
    let root = identity::generate();
    let next_recovery = identity::generate();
    let entry = Entry {
        name: name.to_string(),
        root: identity::encode_public(&root.verifying_key()),
        recovery: identity::encode_public(&next_recovery.verifying_key()),
        devices: vec![device_of(kp)],
        passkeys: vec![],
        // a grant proves a root; this one is new. `dd invite` again
        grant: None,
        libraries: vec![],
        version: cur.entry.version + 1,
        updated: identity::now(),
    };
    // the libraries come along: the paper key opens each one, the new
    // root and device and recovery key get it sealed afresh
    let mut entry = entry;
    for lib in &cur.entry.libraries {
        let key = lib
            .keys
            .iter()
            .find(|k| k.to == "recovery")
            .and_then(|k| library::open_with(&recovery, &k.sealed).ok())
            .with_context(|| format!("library {}: the paper key does not open it", lib.id))?;
        entry.libraries.push(library::Library {
            id: lib.id.clone(),
            keys: library::seal_for_entry(&entry, &key)?,
            readers: lib.readers.clone(),
            created: lib.created,
        });
    }
    let signed = identity::sign_recovery(entry, &root, &recovery)?;
    publish(dirs, &signed).await?;
    keys.set(ROOT, &identity::encode_secret(&root))?;
    Ok(identity::encode_secret(&next_recovery))
}

/// Sign a passkey into the entry. The box that ran the browser ceremony hands
/// the credential to `dd enrol`; nothing on that box could add it itself.
pub async fn admit_passkey(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
    pk: identity::Passkey,
) -> Result<SignedEntry> {
    let cur = ours(dirs, name, root).await?;
    let mut entry = cur.entry.clone();
    if entry.passkeys.iter().any(|p| p.id == pk.id) {
        bail!("that passkey is already in the entry");
    }
    // The passkey brought the key the Files and Movies pages open with: the
    // libraries are sealed to it in the same signed step, so the browser
    // opens them at once and nobody runs `dd passkey link` by hand.
    if let Some(library_key) = &pk.library_key {
        identity::decode_public(library_key).context("that is not an ed25519 public key")?;
        let sealed_to = format!("passkey:{}", pk.id);
        for lib in &mut entry.libraries {
            let key = library::open_library(lib, None, Some(root))
                .with_context(|| format!("library {}: the root does not open it", lib.id))?;
            lib.keys.retain(|k| k.to != sealed_to);
            lib.keys.push(library::SealedKey {
                to: sealed_to.clone(),
                sealed: library::seal_to(library_key, &key[..])?,
            });
        }
    }
    entry.passkeys.push(pk);
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(signed)
}

pub async fn remove_passkey(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
    id: &str,
) -> Result<SignedEntry> {
    let cur = ours(dirs, name, root).await?;
    let mut entry = cur.entry.clone();
    let before = entry.passkeys.len();
    entry.passkeys.retain(|p| p.id != id);
    if entry.passkeys.len() == before {
        bail!("no passkey {id} in the entry");
    }
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(signed)
}
