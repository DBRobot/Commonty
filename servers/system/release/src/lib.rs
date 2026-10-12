//! A release: what every box should run, signed by the one key allowed to
//! say so.
//!
//! The laptop writes one of these per release: the commit, a counter, and
//! for every box the exact store path and the hash of its contents - the
//! paths CI built, once two boxes have vouched for them, or the laptop's
//! own builds when CI's are missing. It signs the whole thing with the
//! release key and publishes it where every box can fetch it. A box checks the
//! signature against the public key baked into it, refuses a counter lower
//! than the last it applied, fetches its own path, checks the hash, and
//! switches. Nothing a box or the forge does can produce a valid one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub mod setup;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};

pub use identity::{decode_public, decode_secret, encode_public, encode_secret, generate};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("signature does not verify")]
    Signature,
    #[error("signed by {0}, not the release key")]
    Signer(String),
    #[error("key: {0}")]
    Key(String),
    #[error("attestation: {0}")]
    Attest(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// One box's share of a release.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoxRelease {
    /// the system closure to run, /nix/store/...-nixos-system-<box>-...
    pub path: String,
    /// its nar hash as `nix path-info` prints it; what the box checks after
    /// fetching, since the store path alone does not pin the contents
    pub nar_hash: String,
    /// One hash over the whole closure: every path it depends on, with its
    /// own nar hash, sorted and digested.
    ///
    /// The top path's hash is not enough on its own. A nixos toplevel is a
    /// tree of symlinks, so its nar pins the *names* of what it points at
    /// and nothing about the contents. Everything underneath arrives from
    /// the cache bucket, which more than one machine can write, and a
    /// poisoned dependency would install unremarked. Absent on releases
    /// made before this existed; a box checks it when it is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure: Option<String>,
}

/// The digest a `closure` carries: every path in `nix path-info -r --json`,
/// as "<path> <narHash>" lines, sorted, sha256, hex.
pub fn closure_digest(path_info_json: &str) -> Result<String> {
    use sha2::Digest as _;
    let v: serde_json::Value = serde_json::from_str(path_info_json)?;
    let mut lines: Vec<String> = Vec::new();
    match &v {
        serde_json::Value::Object(m) => {
            for (path, e) in m {
                let h = e
                    .get("narHash")
                    .and_then(|h| h.as_str())
                    .ok_or_else(|| Error::Key(format!("no narHash for {path}")))?;
                lines.push(format!("{path} {h}"));
            }
        }
        serde_json::Value::Array(a) => {
            for e in a {
                let path = e
                    .get("path")
                    .and_then(|p| p.as_str())
                    .ok_or_else(|| Error::Key("path-info entry has no path".into()))?;
                let h = e
                    .get("narHash")
                    .and_then(|h| h.as_str())
                    .ok_or_else(|| Error::Key(format!("no narHash for {path}")))?;
                lines.push(format!("{path} {h}"));
            }
        }
        _ => return Err(Error::Key("path-info is neither object nor array".into())),
    }
    if lines.is_empty() {
        return Err(Error::Key("path-info listed nothing".into()));
    }
    lines.sort();
    let mut h = sha2::Sha256::new();
    for l in &lines {
        h.update(l.as_bytes());
        h.update(b"\n");
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Payload {
    /// strictly increasing; a box never goes back
    pub counter: u64,
    /// the commit every path was built from
    pub rev: String,
    /// seconds since the epoch, when this was signed
    pub issued: u64,
    /// box name -> what it runs. Sorted, so the bytes signed are stable.
    pub boxes: BTreeMap<String, BoxRelease>,
    /// The boxes' signed words that they rebuilt these closures
    /// (dd-attest), as publish received them. A box switches only to a
    /// closure two boxes of its fleet vouch for here (`vouchers`).
    /// Absent on releases made before boxes checked them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestations: Vec<Attestation>,
    /// Published with no box asked to vouch (DD_ATTEST=off), and saying so
    /// under the release key: for bootstrap alone. A box that has taken an
    /// attested release never takes one of these again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bootstrap: bool,
}

/// The namespace dd-attest signs in: an ssh signature made for anything
/// else is not an attestation.
pub const ATTEST_NAMESPACE: &str = "commonty-attest-v1";
/// How many distinct boxes must vouch for each box's closure. The box
/// itself may be one of them, never the only one.
pub const ATTEST_NEED: usize = 2;

/// One box's signed answer from dd-attest: the statement exactly as signed,
/// `{"v":1,"attester","flake","rev","boxes":{<name>:{path,closure}}}`, and
/// its ssh signature by that box's host key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attestation {
    pub attester: String,
    pub statement: String,
    pub signature: String,
}

/// Which boxes of `fleet` (name -> ssh host key) vouch, in `payload`, for
/// `name`'s closure: a statement about this release's commit, naming the
/// same path and the same closure digest, signed by the attester's own host
/// key as `fleet` has it (`signed` checks that; `ssh_signed` is the real
/// check). The second list says why each other attestation did not count.
pub fn vouchers(
    payload: &Payload,
    name: &str,
    fleet: &BTreeMap<String, String>,
    signed: impl Fn(&Attestation, &str) -> bool,
) -> (BTreeSet<String>, Vec<String>) {
    let mut by = BTreeSet::new();
    let mut not = Vec::new();
    let Some(mine) = payload.boxes.get(name) else {
        return (by, vec![format!("the release has nothing for {name}")]);
    };
    let Some(closure) = &mine.closure else {
        return (
            by,
            vec![format!("no closure digest for {name} to vouch for")],
        );
    };
    for a in &payload.attestations {
        let who = &a.attester;
        let Some(host_key) = fleet.get(who) else {
            not.push(format!("{who}: not one of this fleet's boxes"));
            continue;
        };
        let Ok(st) = serde_json::from_str::<serde_json::Value>(&a.statement) else {
            not.push(format!("{who}: the statement is not json"));
            continue;
        };
        let about = st["v"] == 1
            && st["attester"].as_str() == Some(who)
            && st["rev"].as_str() == Some(payload.rev.as_str())
            && st["flake"].as_str().is_some_and(|f| !f.is_empty());
        if !about {
            not.push(format!(
                "{who}: the statement is about another commit or box"
            ));
            continue;
        }
        let b = &st["boxes"][name];
        if b["path"].as_str() != Some(mine.path.as_str())
            || b["closure"].as_str() != Some(closure.as_str())
        {
            not.push(format!(
                "{who}: does not vouch for this path and closure of {name}"
            ));
            continue;
        }
        if !signed(a, host_key) {
            not.push(format!("{who}: not signed by its host key"));
            continue;
        }
        by.insert(who.clone());
    }
    (by, not)
}

/// `a.statement` signed by `host_key` (as fleet/boxes.json has it), in the
/// attest namespace, checked by ssh-keygen. `dir` is a fresh directory no
/// one else may write in; the files it is given there are removed after.
pub fn ssh_signed(dir: &Path, a: &Attestation, host_key: &str) -> Result<()> {
    use std::io::Write as _;
    // the allowed-signers line is built from these: neither may break out of it
    let plain = !a.attester.is_empty()
        && a.attester
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !plain || host_key.contains(['\n', '\r', '"']) {
        return Err(Error::Attest(format!(
            "{}: not a plain name and key",
            a.attester
        )));
    }
    let allowed = dir.join("allowed");
    let sig = dir.join("sig");
    std::fs::write(
        &allowed,
        format!(
            "{} namespaces=\"{ATTEST_NAMESPACE}\" {host_key}\n",
            a.attester
        ),
    )
    .map_err(|e| Error::Attest(e.to_string()))?;
    std::fs::write(&sig, &a.signature).map_err(|e| Error::Attest(e.to_string()))?;
    let checked = (|| -> std::io::Result<bool> {
        let mut c = std::process::Command::new("ssh-keygen")
            .args(["-Y", "verify", "-f"])
            .arg(&allowed)
            .args(["-I", &a.attester, "-n", ATTEST_NAMESPACE, "-s"])
            .arg(&sig)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        c.stdin
            .take()
            .expect("piped")
            .write_all(a.statement.as_bytes())?;
        Ok(c.wait()?.success())
    })();
    let _ = std::fs::remove_file(&allowed);
    let _ = std::fs::remove_file(&sig);
    match checked {
        Ok(true) => Ok(()),
        Ok(false) => Err(Error::Attest(format!(
            "{}: the signature is not its host key's",
            a.attester
        ))),
        Err(e) => Err(Error::Attest(format!("ssh-keygen: {e}"))),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signed {
    pub payload: Payload,
    /// public key, base64
    pub signer: String,
    /// over `canonical(payload)`, base64
    pub signature: String,
}

pub fn canonical(p: &Payload) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(p)?)
}

/// What a release is signed over, from the payload as it arrived rather
/// than as this build's struct would write it back out: every field, known
/// or not, keys in sorted order, no whitespace.
///
/// The first form signed the struct. An agent reading it drops any field
/// it has never heard of, writes the rest back out, and checks the
/// signature over that - so the first release to add a field is refused by
/// every box running the agent before it, and the release that would have
/// fixed the agent is the one it refuses. Signed this way, a field an
/// older agent does not know is still in the bytes it checks.
pub fn canonical_v2(payload: &serde_json::Value) -> Result<Vec<u8>> {
    // sorted here rather than trusted to serde_json's map, whose order is a
    // feature flag any crate in the build can turn on
    let mut out = Vec::new();
    write_sorted(payload, &mut out)?;
    Ok(out)
}

// keys sorted at every level, so the output does not depend on how the
// map it came from happens to iterate
fn write_sorted(v: &serde_json::Value, out: &mut Vec<u8>) -> Result<()> {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(serde_json::to_vec(k)?);
                out.push(b':');
                write_sorted(&m[*k], out)?;
            }
            out.push(b'}');
        }
        serde_json::Value::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_sorted(x, out)?;
            }
            out.push(b']');
        }
        other => out.extend(serde_json::to_vec(other)?),
    }
    Ok(())
}

/// Which bytes a new release is signed over. `Legacy` exists for one
/// release: the one that carries an agent able to read `V2` to a fleet
/// whose agents cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Legacy,
    V2,
}

pub fn sign(payload: Payload, key: &SigningKey) -> Result<Signed> {
    sign_as(payload, key, Form::V2)
}

pub fn sign_as(payload: Payload, key: &SigningKey, form: Form) -> Result<Signed> {
    let bytes = match form {
        Form::Legacy => canonical(&payload)?,
        Form::V2 => canonical_v2(&serde_json::to_value(&payload)?)?,
    };
    let sig: Signature = key.sign(&bytes);
    Ok(Signed {
        payload,
        signer: encode_public(&key.verifying_key()),
        signature: B64.encode(sig.to_bytes()),
    })
}

/// Anything else the release key signs, in the V2 form: a JSON payload that
/// names its own `kind`. The app manifest is one. A box release has no
/// kind and a different shape, so neither can be passed off as the other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedDoc {
    pub payload: serde_json::Value,
    pub signer: String,
    pub signature: String,
}

pub fn sign_doc(kind: &str, mut payload: serde_json::Value, key: &SigningKey) -> Result<SignedDoc> {
    payload["kind"] = serde_json::Value::String(kind.to_string());
    let sig: Signature = key.sign(&canonical_v2(&payload)?);
    Ok(SignedDoc {
        payload,
        signer: encode_public(&key.verifying_key()),
        signature: B64.encode(sig.to_bytes()),
    })
}

/// A signed document as published, if it is signed by `trusted` and is of
/// `kind`; its payload.
pub fn verify_doc(raw: &str, kind: &str, trusted: &VerifyingKey) -> Result<serde_json::Value> {
    let d: SignedDoc = serde_json::from_str(raw)?;
    if d.signer != encode_public(trusted) {
        return Err(Error::Signer(d.signer));
    }
    let sig = B64.decode(&d.signature).map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    trusted
        .verify(&canonical_v2(&d.payload)?, &sig)
        .map_err(|_| Error::Signature)?;
    if d.payload["kind"].as_str() != Some(kind) {
        return Err(Error::Key(format!("not a {kind}")));
    }
    Ok(d.payload)
}

/// A release file, checked against the bytes it was published as. Either
/// form verifies; the payload comes back as this build understands it,
/// which is only after the whole of it - unknown fields included - was
/// found to be signed.
pub fn verify_json(raw: &str, trusted: &VerifyingKey) -> Result<Signed> {
    let v: serde_json::Value = serde_json::from_str(raw)?;
    let signed: Signed = serde_json::from_value(v.clone())?;
    if signed.signer != encode_public(trusted) {
        return Err(Error::Signer(signed.signer.clone()));
    }
    let sig = B64
        .decode(&signed.signature)
        .map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    let v2 = canonical_v2(&v["payload"])?;
    if trusted.verify(&v2, &sig).is_ok() {
        return Ok(signed);
    }
    trusted
        .verify(&canonical(&signed.payload)?, &sig)
        .map_err(|_| Error::Signature)?;
    Ok(signed)
}

/// Signed by exactly `trusted`? Any other signer, however valid, is refused.
pub fn verify(signed: &Signed, trusted: &VerifyingKey) -> Result<()> {
    if signed.signer != encode_public(trusted) {
        return Err(Error::Signer(signed.signer.clone()));
    }
    let sig = B64
        .decode(&signed.signature)
        .map_err(|_| Error::Signature)?;
    let sig = Signature::from_slice(&sig).map_err(|_| Error::Signature)?;
    // a struct in hand has only the fields this build knows, so this is
    // for releases this build made; one read off the wire goes through
    // verify_json, which keeps the rest
    let v2 = canonical_v2(&serde_json::to_value(&signed.payload)?)?;
    if trusted.verify(&v2, &sig).is_ok() {
        return Ok(());
    }
    trusted
        .verify(&canonical(&signed.payload)?, &sig)
        .map_err(|_| Error::Signature)
}

/// The nar hash `nix path-info --json` printed for `path`. Nix has printed
/// both a map keyed by path and a list of entries; take either.
pub fn nar_hash_from_path_info(json: &str, path: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let entry = v.get(path).cloned().or_else(|| {
        v.as_array()?
            .iter()
            .find(|e| e.get("path").and_then(|p| p.as_str()) == Some(path))
            .cloned()
    });
    entry
        .as_ref()
        .and_then(|e| e.get("narHash"))
        .and_then(|h| h.as_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Key(format!("path-info has no narHash for {path}")))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(counter: u64) -> Payload {
        let mut boxes = BTreeMap::new();
        boxes.insert(
            "node2".into(),
            BoxRelease {
                path: "/nix/store/aaaa-nixos-system-node2".into(),
                nar_hash: "sha256-AAAA".into(),
                closure: None,
            },
        );
        Payload {
            counter,
            rev: "deadbeef".into(),
            issued: 1,
            boxes,
            attestations: vec![],
            bootstrap: false,
        }
    }

    #[test]
    fn signed_by_the_release_key_verifies() {
        let k = generate();
        let s = sign(payload(1), &k).unwrap();
        verify(&s, &k.verifying_key()).unwrap();
    }

    #[test]
    fn another_key_is_refused_even_when_its_signature_is_valid() {
        let k = generate();
        let other = generate();
        let s = sign(payload(1), &other).unwrap();
        assert!(matches!(
            verify(&s, &k.verifying_key()),
            Err(Error::Signer(_))
        ));
    }

    #[test]
    fn a_changed_payload_is_refused() {
        let k = generate();
        let mut s = sign(payload(1), &k).unwrap();
        s.payload.counter = 2;
        assert!(matches!(
            verify(&s, &k.verifying_key()),
            Err(Error::Signature)
        ));
    }

    #[test]
    fn survives_a_json_round_trip() {
        let k = generate();
        let s = sign(payload(7), &k).unwrap();
        let back: Signed =
            serde_json::from_str(&serde_json::to_string_pretty(&s).unwrap()).unwrap();
        verify(&back, &k.verifying_key()).unwrap();
        assert_eq!(back.payload, payload(7));
    }
}

#[cfg(test)]
mod closure_tests {
    use super::*;

    /// The point of the digest: change anything anywhere under the top
    /// path and it stops matching. The top path's own hash would not.
    #[test]
    fn every_path_under_the_top_one_is_pinned() {
        let honest = r#"{
          "/nix/store/aaa-system": {"narHash": "sha256-top"},
          "/nix/store/bbb-openssl": {"narHash": "sha256-ssl"},
          "/nix/store/ccc-glibc": {"narHash": "sha256-libc"}
        }"#;
        let a = closure_digest(honest).unwrap();

        // the same closure, listed in another order and in the array shape
        let reordered = r#"[
          {"path": "/nix/store/ccc-glibc", "narHash": "sha256-libc"},
          {"path": "/nix/store/aaa-system", "narHash": "sha256-top"},
          {"path": "/nix/store/bbb-openssl", "narHash": "sha256-ssl"}
        ]"#;
        assert_eq!(
            a,
            closure_digest(reordered).unwrap(),
            "order must not matter"
        );

        // one dependency swapped for different contents at the same path
        let poisoned = honest.replace("sha256-ssl", "sha256-someone-elses");
        assert_ne!(a, closure_digest(&poisoned).unwrap());

        // a dependency added, the top path untouched
        let extra = r#"{
          "/nix/store/aaa-system": {"narHash": "sha256-top"},
          "/nix/store/bbb-openssl": {"narHash": "sha256-ssl"},
          "/nix/store/ccc-glibc": {"narHash": "sha256-libc"},
          "/nix/store/ddd-extra": {"narHash": "sha256-extra"}
        }"#;
        assert_ne!(a, closure_digest(extra).unwrap());

        assert!(
            closure_digest("{}").is_err(),
            "nothing listed is not a closure"
        );
    }
}

#[cfg(test)]
mod form_tests {
    use super::*;

    fn payload(closure: Option<&str>) -> Payload {
        let mut boxes = BTreeMap::new();
        boxes.insert(
            "node2".into(),
            BoxRelease {
                path: "/nix/store/aaaa-nixos-system-node2".into(),
                nar_hash: "sha256-AAAA".into(),
                closure: closure.map(str::to_string),
            },
        );
        Payload {
            counter: 7,
            rev: "deadbeef".into(),
            issued: 1,
            boxes,
            attestations: vec![],
            bootstrap: false,
        }
    }

    /// What the agent deployed today does with a release: reads it into a
    /// struct that has never heard of `closure`, and checks the signature
    /// over that struct written back out.
    fn old_agent_accepts(published: &str, key: &VerifyingKey) -> bool {
        let mut v: serde_json::Value = serde_json::from_str(published).unwrap();
        for b in v["payload"]["boxes"].as_object_mut().unwrap().values_mut() {
            b.as_object_mut().unwrap().remove("closure");
        }
        let s: Signed = serde_json::from_value(v).unwrap();
        verify(&s, key).is_ok()
    }

    #[test]
    fn the_release_that_ships_the_new_agent_is_one_the_old_agent_takes() {
        let k = generate();
        let vk = k.verifying_key();

        // the trap: a closure digest in a release the old agent must read
        let trap = serde_json::to_string(&sign_as(payload(Some("abc")), &k, Form::Legacy).unwrap())
            .unwrap();
        assert!(
            !old_agent_accepts(&trap, &vk),
            "this is what would strand the fleet"
        );

        // the bridge: the old form, no closure - taken by the old agent...
        let bridge =
            serde_json::to_string(&sign_as(payload(None), &k, Form::Legacy).unwrap()).unwrap();
        assert!(old_agent_accepts(&bridge, &vk));
        // ...and by the new one
        assert!(verify_json(&bridge, &vk).is_ok());

        // after it: the new form, closure and all, read by the new agent
        let next = serde_json::to_string_pretty(&sign(payload(Some("abc")), &k).unwrap()).unwrap();
        assert_eq!(
            verify_json(&next, &vk).unwrap().payload.boxes["node2"]
                .closure
                .as_deref(),
            Some("abc")
        );
    }

    /// And never again: a publisher newer than this agent adds a field
    /// this agent has not heard of, and the release is still taken.
    #[test]
    fn a_field_this_agent_has_never_heard_of_does_not_break_it() {
        let k = generate();
        let mut v = serde_json::to_value(payload(Some("abc"))).unwrap();
        v["waves"] = serde_json::json!({ "node2": 2 });
        v["boxes"]["node2"]["something_new"] = serde_json::json!(true);
        let sig: Signature = k.sign(&canonical_v2(&v).unwrap());
        let published = serde_json::to_string_pretty(&serde_json::json!({
            "payload": v,
            "signer": encode_public(&k.verifying_key()),
            "signature": B64.encode(sig.to_bytes()),
        }))
        .unwrap();
        assert!(verify_json(&published, &k.verifying_key()).is_ok());

        // but a change to any of it, known or not, is caught
        let tampered = published.replace("\"something_new\": true", "\"something_new\": false");
        assert!(verify_json(&tampered, &k.verifying_key()).is_err());
        let moved = published.replace("sha256-AAAA", "sha256-BBBB");
        assert!(verify_json(&moved, &k.verifying_key()).is_err());
    }
}

#[cfg(test)]
mod doc_tests {
    use super::*;

    #[test]
    fn a_document_is_what_it_says_it_is_and_nothing_else() {
        let k = generate();
        let vk = k.verifying_key();
        let app = sign_doc("app", serde_json::json!({ "tag": "v1", "files": {} }), &k).unwrap();
        let raw = serde_json::to_string_pretty(&app).unwrap();
        assert_eq!(verify_doc(&raw, "app", &vk).unwrap()["tag"], "v1");

        // not something else by another name
        assert!(verify_doc(&raw, "invite", &vk).is_err());
        // not a box release either
        assert!(verify_json(&raw, &vk).is_err());
        // and not changed
        assert!(verify_doc(&raw.replace("v1", "v2"), "app", &vk).is_err());
        // nor signed by some other key
        assert!(verify_doc(&raw, "app", &generate().verifying_key()).is_err());
    }
}

#[cfg(test)]
mod attest_tests {
    use super::*;

    const REV: &str = "0123456789abcdef0123456789abcdef01234567";

    fn statement(attester: &str, rev: &str, path: &str, closure: &str) -> String {
        serde_json::json!({
            "v": 1, "attester": attester, "flake": "git+https://git.example/o/r", "rev": rev,
            "boxes": { "node2": { "path": path, "closure": closure } },
        })
        .to_string()
    }

    fn att(attester: &str, statement: String) -> Attestation {
        Attestation {
            attester: attester.into(),
            statement,
            signature: format!("signed by {attester}"),
        }
    }

    fn release(attestations: Vec<Attestation>) -> Payload {
        let mut boxes = BTreeMap::new();
        boxes.insert(
            "node2".into(),
            BoxRelease {
                path: "/nix/store/aaaa-nixos-system-node2".into(),
                nar_hash: "sha256-AAAA".into(),
                closure: Some("c2".into()),
            },
        );
        Payload {
            counter: 9,
            rev: REV.into(),
            issued: 1,
            boxes,
            attestations,
            bootstrap: false,
        }
    }

    fn fleet() -> BTreeMap<String, String> {
        ["node1", "node2", "node3"]
            .iter()
            .map(|n| (n.to_string(), format!("key of {n}")))
            .collect()
    }

    // a signature counts when it is the attester's own key's
    fn fake(a: &Attestation, key: &str) -> bool {
        a.signature == format!("signed by {}", a.attester)
            && key == format!("key of {}", a.attester)
    }

    #[test]
    fn two_boxes_of_the_fleet_vouch_for_exactly_this_closure() {
        let good = |who: &str| {
            att(
                who,
                statement(who, REV, "/nix/store/aaaa-nixos-system-node2", "c2"),
            )
        };
        let (by, not) = vouchers(
            &release(vec![good("node1"), good("node2")]),
            "node2",
            &fleet(),
            fake,
        );
        assert_eq!(by.into_iter().collect::<Vec<_>>(), ["node1", "node2"]);
        assert!(not.is_empty(), "{not:?}");

        // the same box twice is one box
        let (by, _) = vouchers(
            &release(vec![good("node1"), good("node1")]),
            "node2",
            &fleet(),
            fake,
        );
        assert_eq!(by.len(), 1);

        let cases = [
            // a box the fleet does not know
            att(
                "stranger",
                statement("stranger", REV, "/nix/store/aaaa-nixos-system-node2", "c2"),
            ),
            // another path, another closure, another commit
            att(
                "node1",
                statement("node1", REV, "/nix/store/bbbb-nixos-system-node2", "c2"),
            ),
            att(
                "node1",
                statement("node1", REV, "/nix/store/aaaa-nixos-system-node2", "c9"),
            ),
            att(
                "node1",
                statement(
                    "node1",
                    &"f".repeat(40),
                    "/nix/store/aaaa-nixos-system-node2",
                    "c2",
                ),
            ),
            // one box's statement claimed by another
            att(
                "node3",
                statement("node1", REV, "/nix/store/aaaa-nixos-system-node2", "c2"),
            ),
            // signed by someone else's key
            Attestation {
                signature: "signed by node3".into(),
                ..good("node1")
            },
        ];
        for (i, c) in cases.into_iter().enumerate() {
            let (by, not) = vouchers(&release(vec![c, good("node2")]), "node2", &fleet(), fake);
            assert_eq!(by.len(), 1, "case {i}");
            assert_eq!(not.len(), 1, "case {i}: {not:?}");
        }

        // vouching for one box is not vouching for another
        let (by, not) = vouchers(
            &release(vec![good("node1"), good("node2")]),
            "node1",
            &fleet(),
            fake,
        );
        assert!(by.is_empty() && not.len() == 1);
        // nor for a box whose closure the release does not pin
        let mut p = release(vec![good("node1"), good("node2")]);
        p.boxes.get_mut("node2").unwrap().closure = None;
        assert!(vouchers(&p, "node2", &fleet(), fake).0.is_empty());
    }

    #[test]
    fn they_are_signed_with_the_release_and_absent_when_there_are_none() {
        let k = generate();
        let mut p = release(vec![att(
            "node1",
            statement("node1", REV, "/nix/store/aaaa-nixos-system-node2", "c2"),
        )]);
        p.bootstrap = true;
        let published = serde_json::to_string_pretty(&sign(p.clone(), &k).unwrap()).unwrap();
        assert_eq!(
            verify_json(&published, &k.verifying_key()).unwrap().payload,
            p
        );
        // an attestation cannot be added, dropped or swapped after signing
        let dropped = published.replace("\"bootstrap\": true", "\"bootstrap\": false");
        assert!(verify_json(&dropped, &k.verifying_key()).is_err());
        // a release without them is the bytes it always was, so the legacy
        // form and the agents before this field read it unchanged
        let bare = serde_json::to_string(&sign(release(vec![]), &k).unwrap()).unwrap();
        assert!(!bare.contains("attestations") && !bare.contains("bootstrap"));
    }

    /// The real check, with ssh-keygen as the boxes have it
    #[test]
    fn only_the_host_key_in_the_attest_namespace_signs() {
        let dir = std::env::temp_dir().join(format!("dd-attest-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keygen = |n: &str| {
            let k = dir.join(n);
            assert!(
                std::process::Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                    .arg(&k)
                    .status()
                    .unwrap()
                    .success()
            );
            let p = std::fs::read_to_string(k.with_extension("pub")).unwrap();
            (
                k,
                p.split_whitespace().take(2).collect::<Vec<_>>().join(" "),
            )
        };
        let sign = |key: &Path, ns: &str, text: &str| {
            use std::io::Write as _;
            let mut c = std::process::Command::new("ssh-keygen")
                .args(["-Y", "sign", "-q", "-f"])
                .arg(key)
                .args(["-n", ns])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
            String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap()
        };
        let (k1, pub1) = keygen("node1");
        let (_, pub2) = keygen("node2");
        let st = statement("node1", REV, "/nix/store/aaaa-nixos-system-node2", "c2");
        let a = Attestation {
            attester: "node1".into(),
            signature: sign(&k1, ATTEST_NAMESPACE, &st),
            statement: st.clone(),
        };
        let work = dir.join("check");
        std::fs::create_dir_all(&work).unwrap();
        ssh_signed(&work, &a, &pub1).unwrap();
        assert!(ssh_signed(&work, &a, &pub2).is_err(), "another box's key");
        let other_ns = Attestation {
            signature: sign(&k1, "file", &st),
            ..a.clone()
        };
        assert!(
            ssh_signed(&work, &other_ns, &pub1).is_err(),
            "made for anything else"
        );
        let changed = Attestation {
            statement: st.replace("c2", "c3"),
            ..a.clone()
        };
        assert!(
            ssh_signed(&work, &changed, &pub1).is_err(),
            "changed under it"
        );
        let sneaky = Attestation {
            attester: "node1 namespaces=\"x\"".into(),
            ..a.clone()
        };
        assert!(ssh_signed(&work, &sneaky, &pub1).is_err());
        assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0, "nothing left");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
