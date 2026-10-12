//! A member's own identity, from any of their devices: a root key here or
//! on a phone, a recovery key on paper, and the entry they sign with it,
//! published to every directory they name. `dd` and the app share this.
//!
//! No server is asked for anything but storage. A directory that lies can
//! withhold or replay, and publishing to more than one is how that is caught.

use anyhow::{Context, Result, bail};
use auth::KeyStore;
use directory::{Took, fetch, fetch_proven, newest};
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

/// This device's token for reading the directory at `host`, if it has a
/// name and a key: good for directory reads there and nothing else, for
/// five minutes, so one sent to the wrong host opens nothing.
pub fn read_token(keys: &impl KeyStore, host: &str) -> Option<String> {
    let user = keys.get(USER).ok()??;
    let kp = auth::device::load(keys).ok()??;
    auth::device::mint_at(
        &kp,
        &user,
        std::time::Duration::from_secs(300),
        Some(identity::DIRECTORY_READ),
        Some(host),
    )
    .ok()
}

/// every directory read from here on carries this device's read token: the
/// directory is members' to read, and each member's own
pub fn read_as_device<S: KeyStore + 'static>(open: impl Fn() -> S + Send + Sync + 'static) {
    directory::read_with(Some(std::sync::Arc::new(move |host: &str| {
        read_token(&open(), host)
    })));
}

/// keyring account holding the root this device first saw its entry under:
/// what anything sealed to the entry's keys is checked against
pub const ROOT_PIN: &str = "entry-root";

/// keyring account holding the counter of the newest signed member list
/// this device has seen: an older one is refused from then on
pub const MEMBERS_SEEN: &str = "members-seen";

/// The release's member list, signed by the release key, and no older than
/// the newest this device has seen (directory::member_list).
pub async fn released(keys: &impl KeyStore, dirs: &[String]) -> Result<identity::MemberList> {
    let list = directory::member_list(dirs).await?;
    let seen: u64 = keys
        .get(MEMBERS_SEEN)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if list.counter < seen {
        bail!(
            "the directories hand back member list {} after this device saw {seen}",
            list.counter
        );
    }
    if list.counter > seen {
        keys.set(MEMBERS_SEEN, &list.counter.to_string())?;
    }
    Ok(list)
}

/// what a person let in by an invite sees until a release lists them, for
/// any check that holds an entry to the released list
pub const NOT_RELEASED: &str = "not in a signed release yet: an account let in by an invite is \
     unverified until the next release lists it, and until then cannot be shared with or recovered \
     on a new device";

/// This member's entry, as this device can vouch for it: under the root it
/// holds, or the one it pinned the first time it looked (kept from then on).
/// A box that serves an entry under some other root, listing keys of its
/// own beside this device's, is refused here rather than sealed to. That
/// first look is held to the release's member list, never taken on sight.
pub async fn own_entry(keys: &impl KeyStore, dirs: &[String], name: &str) -> Result<SignedEntry> {
    let found = fetch(dirs, name).await;
    let held = load_root(keys)?.map(|r| identity::encode_public(&r.verifying_key()));
    let pin = keys.get(ROOT_PIN)?.map(|s| s.to_string());
    let e = match held.or(pin.clone()) {
        Some(r) => directory::resolve(&found, directory::Anchor::Root(&r))
            .context("your entry is not one this device can vouch for (signed out and in again after a recovery elsewhere?)")?,
        None => {
            let list = released(keys, dirs).await?;
            let handovers = directory::handovers(dirs, name, &[]).await;
            directory::resolve(
                &found,
                directory::Anchor::Released {
                    members: &list.members,
                    revoked: &list.revoked,
                    handovers: &handovers,
                    names: Some(&list.names),
                },
            )
            .with_context(|| format!("your entry is {NOT_RELEASED}"))?
        }
    };
    if pin.as_deref() != Some(e.entry.root.as_str()) {
        keys.set(ROOT_PIN, &e.entry.root)?;
    }
    Ok(e)
}
/// A secret a member's devices share (the archive password, the Photos
/// password), sealed to every key of the entry that a device holds - each
/// device, a root kept on one, the paper key, the same rule as a library's -
/// and signed by the device that made it:
///
/// ```text
/// {"for": "<what>", "by": "<signer's fingerprint>",
///  "keys": {"device:<fp>" | "root" | "recovery": sealed}, "sig": base64}
/// ```
///
/// Sealing needs only a public key, so a box could seal a secret of its own
/// choosing to a device; the signature, by a device the root-signed entry
/// names, is what tells the member's copy from that. `what` keeps one
/// secret from being handed over as another.
///
/// An entry whose root is a passkey is signed by an assertion, and a box's
/// page can have a passkey make one over an entry the box wrote, a device
/// or a paper key of its own in it. So for one of those it goes to this
/// device (`me`) alone, and only this device's signature is believed:
/// nothing else the entry names is this device's to trust. Such a member
/// has no other copy, the paper key included (docs/dev/app.md).
pub fn seal_for_keys(
    entry: &Entry,
    me: &ed25519_dalek::SigningKey,
    what: &str,
    secret: &[u8],
) -> Result<serde_json::Value> {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let fp = identity::fingerprint(&identity::encode_public(&me.verifying_key()));
    let key: library::Key = {
        let mut k = zeroize::Zeroizing::new([0u8; 32]);
        anyhow::ensure!(secret.len() == 32, "a shared secret is 32 bytes");
        k.copy_from_slice(secret);
        k
    };
    let mut entry = entry.clone();
    if entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        entry.devices.retain(|d| d.fingerprint == fp);
        // its paper key too is what the entry says, and the entry is what a
        // box could have had signed; and a passkey is never sealed to
        entry.recovery.clear();
        entry.passkeys.clear();
    }
    let mut keys = serde_json::Map::new();
    for s in library::seal_for_entry(&entry, &key)? {
        keys.insert(s.to, serde_json::Value::String(s.sealed));
    }
    let keys = serde_json::Value::Object(keys);
    let sig = me.sign(&signed_bytes(what, &fp, &keys)?);
    Ok(serde_json::json!({
        "for": what,
        "by": fp,
        "keys": keys,
        "sig": base64::engine::general_purpose::STANDARD.encode(sig.to_bytes()),
    }))
}

/// what a shared secret's signature is over: a label, what it is for, who
/// signed, and the sealed keys (serde_json keeps a map's keys sorted)
fn signed_bytes(what: &str, by: &str, keys: &serde_json::Value) -> Result<Vec<u8>> {
    let mut b = b"commonty shared secret v1\0".to_vec();
    b.extend_from_slice(what.as_bytes());
    b.push(0);
    b.extend_from_slice(by.as_bytes());
    b.push(0);
    b.extend_from_slice(&serde_json::to_vec(keys)?);
    Ok(b)
}

/// The labels a shared secret is sealed to now, by `seal_for_keys`' rule
/// for this entry and this device: what a copy that lacks one is missing.
pub fn due_for_keys(entry: &Entry, me: &str) -> Vec<String> {
    if entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        return vec![format!("device:{me}")];
    }
    let mut want: Vec<String> = entry
        .devices
        .iter()
        .map(|d| format!("device:{}", d.fingerprint))
        .collect();
    want.push("root".into());
    if !entry.recovery.is_empty() {
        want.push("recovery".into());
    }
    want
}

/// The sealed keys of a shared secret, once its signature is one the
/// member's own entry vouches for: by a device that entry names (by this
/// device alone, for a passkey root), for this purpose. `entry` must be the
/// one this device resolved under its pinned root (`own_entry`).
pub fn vouched<'a>(
    doc: &'a serde_json::Value,
    entry: &Entry,
    me: &ed25519_dalek::VerifyingKey,
    what: &str,
) -> Result<&'a serde_json::Value> {
    use base64::Engine as _;
    use ed25519_dalek::Verifier as _;
    if doc["for"].as_str() != Some(what) {
        bail!("that copy is not of the {what}");
    }
    let by = doc["by"]
        .as_str()
        .context("that copy says nobody made it")?;
    let key = if entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        // this device's own key as it holds it, not as the entry says
        if by != identity::fingerprint(&identity::encode_public(me)) {
            bail!("only this device's own copy is believed for an account a passkey signs");
        }
        *me
    } else {
        let signer = entry
            .devices
            .iter()
            .find(|d| d.fingerprint == by)
            .context("that copy was made by no device of this account")?;
        identity::decode_public(&signer.public_key)?
    };
    let sig = base64::engine::general_purpose::STANDARD
        .decode(doc["sig"].as_str().context("that copy is not signed")?)
        .ok()
        .and_then(|b| ed25519_dalek::Signature::from_slice(&b).ok())
        .context("that copy's signature is not one")?;
    key.verify(&signed_bytes(what, by, &doc["keys"])?, &sig)
        .ok()
        .context("that copy's signature is not its device's")?;
    Ok(&doc["keys"])
}

/// A shared secret, vouched for (`vouched`) and opened with this device's
/// key or the root here
pub fn open_for_device(
    doc: &serde_json::Value,
    entry: &Entry,
    what: &str,
    device: (&str, &ed25519_dalek::SigningKey),
    root: Option<&ed25519_dalek::SigningKey>,
) -> Result<library::Key> {
    let keys = vouched(doc, entry, &device.1.verifying_key(), what)?;
    let lib = identity::Library {
        id: String::new(),
        keys: keys
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(to, v)| {
                Some(identity::SealedKey {
                    to: to.clone(),
                    sealed: v.as_str()?.to_string(),
                })
            })
            .collect(),
        readers: vec![],
        created: 0,
    };
    library::open_library(&lib, Some(device), root).context("no key of this device opens it")
}

/// Leaving (`dd leave`, the app's Settings): the statement, signed with the
/// root here, to every directory given; the first that takes it puts it in
/// the fleet's log, and every box deletes what is theirs at once.
pub async fn leave(dirs: &[String], name: &str, root: &ed25519_dalek::SigningKey) -> Result<()> {
    let s = identity::sign_leave(
        identity::Leave {
            name: name.to_string(),
            root: identity::encode_public(&root.verifying_key()),
            at: identity::now(),
            back: false,
        },
        root,
    )?;
    let http = directory::http()?;
    let mut why = Vec::new();
    for d in dirs {
        let base = d.trim_end_matches('/');
        let base = base.strip_suffix("/_dd/directory").unwrap_or(base);
        match http
            .put(format!("{base}/_dd/leaving/{name}"))
            .json(&s)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => {
                let st = r.status();
                why.push(format!("{d}: {st} {}", r.text().await.unwrap_or_default()))
            }
            Err(e) => why.push(format!("{d}: {e}")),
        }
    }
    bail!("no directory took it:\n  {}", why.join("\n  "))
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
    identity::drop_passkey_keys(&mut entry);
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
/// first and only one, version 1, signed by the root. `terms` is the version
/// of the Terms of use the person agreed to (identity::TERMS); asking them is
/// the caller's. Returns the recovery secret exactly once, for paper.
pub async fn create(
    keys: &impl KeyStore,
    dirs: &[String],
    name: &str,
    kp: &auth::device::KeyPair,
    code: Option<&str>,
    terms: &str,
) -> Result<zeroize::Zeroizing<String>> {
    if keys.get(ROOT)?.is_some() {
        bail!("this machine already holds an identity - `dd identity show`");
    }
    if !identity::valid_name(name) {
        bail!("{name:?} is not a valid name: lowercase letters, digits, - _ .");
    }
    let taken = || {
        format!(
            "there is already an entry for {name}. If it is yours, `dd identity import` \
             the root here or `dd identity recover` with the paper key."
        )
    };
    // a directory that does not show strangers its entries cannot say; the
    // publish below is refused for a name someone holds
    let found = fetch(dirs, name).await;
    for (d, r) in &found {
        match r {
            Ok(Some(_)) => bail!("{d}: {}", taken()),
            Ok(None) => {}
            Err(e) if e.to_string().starts_with("401") => {}
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
        recovered_from: vec![],
        devices: vec![device_of(kp)],
        passkeys: vec![],
        grant,
        libraries: vec![],
        terms: Some(terms.to_string()),
        version: 1,
        updated: identity::now(),
    };
    let signed = identity::sign(entry, &root)?;
    publish(dirs, &signed).await.map_err(|e| {
        let why = format!("{e:#}");
        if why.contains("refused") || why.contains("taken") {
            anyhow::anyhow!("{why}\n{}", taken())
        } else {
            e
        }
    })?;
    keys.set(ROOT, &identity::encode_secret(&root))?;
    keys.set(ROOT_PIN, &identity::encode_public(&root.verifying_key()))?;
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
    let found = fetch_proven(dirs, name, &[root]).await;
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
    identity::drop_passkey_keys(&mut entry);
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
    identity::drop_passkey_keys(&mut entry);
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(signed)
}

/// Whether a library is sealed to exactly what `seal_for_entry` would seal
/// it to now: every device, the root, the paper key, and nothing else.
fn sealed_as_due(entry: &Entry, lib: &identity::Library) -> bool {
    let mut want: Vec<String> = entry
        .devices
        .iter()
        .map(|d| format!("device:{}", d.fingerprint))
        .collect();
    if !entry.root.starts_with(identity::WEBAUTHN_ROOT) {
        want.push("root".into());
    }
    if !entry.recovery.is_empty() {
        want.push("recovery".into());
    }
    let mut have: Vec<String> = lib.keys.iter().map(|k| k.to.clone()).collect();
    want.sort();
    have.sort();
    want == have
}

/// The move off passkey keys, for an entry this root signs: each library
/// key opened here and sealed again to the devices, the root and the paper
/// key alone, every passkey's seal and browser key dropped. Nothing is
/// published when there is nothing to change (None). The app runs it at
/// launch and `dd library reseal` by hand; a library key a passkey's PRF
/// opened stays what it was, so this stops new copies, not old ones
/// (docs/dev/libraries.md, "Moving off passkey keys").
pub async fn reseal(
    dirs: &[String],
    name: &str,
    root: &ed25519_dalek::SigningKey,
) -> Result<Option<SignedEntry>> {
    let cur = ours(dirs, name, root).await?;
    let mut entry = cur.entry.clone();
    let stale = identity::holds_passkey_keys(&entry)
        || entry.libraries.iter().any(|l| !sealed_as_due(&entry, l));
    if !stale {
        return Ok(None);
    }
    let fresh = entry.clone();
    for lib in &mut entry.libraries {
        let key = library::open_library(lib, None, Some(root))
            .with_context(|| format!("library {}: the root does not open it", lib.id))?;
        lib.keys = library::seal_for_entry(&fresh, &key)?;
    }
    identity::drop_passkey_keys(&mut entry);
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(Some(signed))
}

/// Recovery: the paper key installs a new root and a new recovery key, and
/// the device list starts over with this device alone. Membership stays:
/// the old root goes into `recovered_from`, and the invite with it. Everything a lost or
/// stolen device could sign is dead the moment a directory takes this.
/// Returns the new paper key, and the entry as it was before.
pub async fn recover(
    keys: &impl KeyStore,
    dirs: &[String],
    name: &str,
    recovery_secret: &str,
    kp: &auth::device::KeyPair,
) -> Result<(zeroize::Zeroizing<String>, SignedEntry)> {
    let recovery = identity::decode_secret(recovery_secret).context("bad recovery key")?;
    let found = fetch_proven(dirs, name, &[&recovery]).await;
    let handovers = directory::handovers(dirs, name, &[&recovery]).await;
    // What follows reseals library keys out of this entry and co-signs the
    // result with the real recovery key, so a made-up one would be the
    // fleet accepting keys the liar chose and could read. The recovery key
    // matching is not the check it looks like: it is public. So the entry
    // is held to the root this device pinned, or, on a machine that knows
    // none, to the release's member list: on it, or recovered from a root
    // that is, every step proven.
    // a pin is for the name this device is signed in as, no other
    let pinned = match load_root(keys)? {
        _ if keys.get(USER)?.as_deref().map(|s| s.as_str()) != Some(name) => None,
        Some(r) => Some(identity::encode_public(&r.verifying_key())),
        None => keys.get(ROOT_PIN)?.map(|s| s.to_string()),
    };
    let list = match &pinned {
        Some(r) => identity::MemberList {
            members: vec![identity::member_id(r)],
            ..Default::default()
        },
        None => released(keys, dirs).await?,
    };
    let cur = directory::resolve(
        &found,
        directory::Anchor::Released {
            members: &list.members,
            revoked: &list.revoked,
            handovers: &handovers,
            // a pin is for this name already (above)
            names: pinned.is_none().then_some(&list.names),
        },
    )
    .with_context(|| match pinned {
        Some(_) => format!("the entry for {name} does not follow from the root this device knows"),
        None => format!("{name}: no usable entry, or one {NOT_RELEASED}"),
    })?;
    if cur.entry.recovery != identity::encode_public(&recovery.verifying_key()) {
        bail!("that recovery key does not match the entry for {name}");
    }
    let root = identity::generate();
    let next_recovery = identity::generate();
    let entry = Entry {
        name: name.to_string(),
        root: identity::encode_public(&root.verifying_key()),
        recovery: identity::encode_public(&next_recovery.verifying_key()),
        // the root this replaces joins the earlier ones, so membership and
        // the invite it was let in by come along (identity::is_member)
        recovered_from: cur
            .entry
            .recovered_from
            .iter()
            .cloned()
            .chain([cur.entry.root.clone()])
            .collect(),
        devices: vec![device_of(kp)],
        passkeys: vec![],
        grant: cur.entry.grant.clone(),
        libraries: vec![],
        terms: cur.entry.terms.clone(),
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
    keys.set(ROOT_PIN, &identity::encode_public(&root.verifying_key()))?;
    Ok((identity::encode_secret(&next_recovery), cur))
}

/// Sign a passkey into the entry. The box that ran the browser ceremony hands
/// the credential to `dd enrol`; nothing on that box could add it itself.
/// It signs the member in, and opens nothing: no key is sealed to it.
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
    entry.passkeys.push(pk);
    identity::drop_passkey_keys(&mut entry);
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
    identity::drop_passkey_keys(&mut entry);
    entry.version += 1;
    entry.updated = identity::now();
    let signed = identity::sign(entry, root)?;
    publish(dirs, &signed).await?;
    Ok(signed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(k: &ed25519_dalek::SigningKey) -> Device {
        let p = identity::encode_public(&k.verifying_key());
        Device {
            fingerprint: identity::fingerprint(&p),
            public_key: p,
            added: 1,
        }
    }

    fn entry(root: String, devices: Vec<Device>, recovery: String) -> Entry {
        Entry {
            name: "amy".into(),
            root,
            recovery,
            recovered_from: vec![],
            devices,
            passkeys: vec![identity::Passkey {
                id: "p1".into(),
                cred: serde_json::json!({}),
                added: 1,
                rp_id: None,
                library_key: Some(identity::encode_public(
                    &identity::generate().verifying_key(),
                )),
            }],
            grant: None,
            libraries: vec![],
            terms: None,
            version: 1,
            updated: 1,
        }
    }

    fn labels(doc: &serde_json::Value) -> Vec<String> {
        let mut k: Vec<String> = doc["keys"].as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    #[test]
    fn a_shared_secret_goes_to_the_keys_a_device_can_vouch_for() {
        let (me, other, root, paper) = (
            identity::generate(),
            identity::generate(),
            identity::generate(),
            identity::generate(),
        );
        let fp = device(&me).fingerprint;
        let paper_pub = identity::encode_public(&paper.verifying_key());
        let secret = [7u8; 32];

        // a root a device holds signs the entry: every device, the root,
        // the paper key; never the passkey
        let held = entry(
            identity::encode_public(&root.verifying_key()),
            vec![device(&me), device(&other)],
            paper_pub.clone(),
        );
        let doc = seal_for_keys(&held, &me, "photos", &secret).unwrap();
        let mut want = due_for_keys(&held, &fp);
        want.sort();
        assert_eq!(labels(&doc), want);
        assert_eq!(want.len(), 4);
        assert_eq!(
            &open_for_device(&doc, &held, "photos", (&fp, &me), None).unwrap()[..],
            &secret
        );
        // made by the other device, it opens here just the same
        let theirs = seal_for_keys(&held, &other, "photos", &secret).unwrap();
        assert!(open_for_device(&theirs, &held, "photos", (&fp, &me), None).is_ok());
        // but not as another secret
        assert!(open_for_device(&doc, &held, "archive", (&fp, &me), None).is_err());

        // a passkey root: whatever else the entry names - another device, a
        // paper key - a box may have had signed, so this device alone
        let passkey = entry(
            "webauthn:p1:digest".into(),
            vec![device(&me), device(&other)],
            paper_pub,
        );
        let doc = seal_for_keys(&passkey, &me, "photos", &secret).unwrap();
        assert_eq!(labels(&doc), [format!("device:{fp}")]);
        assert_eq!(due_for_keys(&passkey, &fp), [format!("device:{fp}")]);
        let ofp = device(&other).fingerprint;
        assert!(open_for_device(&doc, &passkey, "photos", (&ofp, &other), None).is_err());
        // and the other device's copy is not believed here, signed or not
        let theirs = seal_for_keys(&passkey, &other, "photos", &secret).unwrap();
        assert!(open_for_device(&theirs, &passkey, "photos", (&fp, &me), None).is_err());
    }

    /// Sealing takes only a public key, so a box can seal a password of its
    /// own to this device. What it cannot do is sign it as a device the
    /// member's root-signed entry names.
    #[test]
    fn a_copy_a_box_made_is_refused() {
        let (me, root, stranger) = (
            identity::generate(),
            identity::generate(),
            identity::generate(),
        );
        let fp = device(&me).fingerprint;
        let e = entry(
            identity::encode_public(&root.verifying_key()),
            vec![device(&me)],
            String::new(),
        );
        // sealed to this device by a key the entry does not name
        let forged = seal_for_keys(&e, &stranger, "photos", &[9u8; 32]).unwrap();
        assert!(open_for_device(&forged, &e, "photos", (&fp, &me), None).is_err());
        // a real copy with its sealed keys swapped for the box's own
        let mut swapped = seal_for_keys(&e, &me, "photos", &[7u8; 32]).unwrap();
        swapped["keys"] = forged["keys"].clone();
        assert!(open_for_device(&swapped, &e, "photos", (&fp, &me), None).is_err());
        // claiming to be this device, signed by another key
        let mut claimed = forged.clone();
        claimed["by"] = serde_json::Value::String(fp.clone());
        assert!(open_for_device(&claimed, &e, "photos", (&fp, &me), None).is_err());
        // unsigned
        let mut bare = seal_for_keys(&e, &me, "photos", &[7u8; 32]).unwrap();
        bare.as_object_mut().unwrap().remove("sig");
        assert!(open_for_device(&bare, &e, "photos", (&fp, &me), None).is_err());

        // and the entry it is checked against is one under the pinned root:
        // a box's entry under its own root, or one that does not verify, is
        // never what a copy is checked with (own_entry)
        let mut boxed = e.clone();
        boxed.root = identity::encode_public(&stranger.verifying_key());
        let boxed = identity::sign(boxed, &stranger).unwrap();
        let pin = identity::encode_public(&root.verifying_key());
        let found = |s: identity::SignedEntry| vec![("d".to_string(), Ok(Some(s)))];
        assert!(directory::resolve(&found(boxed), directory::Anchor::Root(&pin)).is_err());
        let mut bent = identity::sign(e.clone(), &root).unwrap();
        bent.entry.devices.push(device(&stranger));
        assert!(directory::resolve(&found(bent), directory::Anchor::Root(&pin)).is_err());
        let good = identity::sign(e, &root).unwrap();
        assert!(directory::resolve(&found(good), directory::Anchor::Root(&pin)).is_ok());
    }
}
