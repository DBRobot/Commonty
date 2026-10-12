//! The controller's name at the DNS Worker (docs/dev/home-controller.md,
//! servers/system/dns/controller.js): registered once on the pairing
//! person's root's word, moved when its address on the network changes, and
//! its certificate's challenges asked with the controller's own key.

use crate::pair::Registration;
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::Signer as _;
use identity::SigningKey;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sign(key: &SigningKey, msg: &str) -> String {
    B64.encode(key.sign(msg.as_bytes()).to_bytes())
}

/// the controller's word for where it is: commonty controller address v1
pub fn address_sig(key: &SigningKey, label: &str, address: &str, at: u64) -> String {
    sign(
        key,
        &format!("commonty controller address v1\0{label}\0{address}\0{at}"),
    )
}

/// the controller's word on one challenge for its own name
pub fn acme_sig(
    key: &SigningKey,
    label: &str,
    at: u64,
    op: &str,
    name: &str,
    value: &str,
) -> String {
    sign(
        key,
        &format!("commonty acme v1\0{label}\0{at}\0{op}\0{name}\0{value}"),
    )
}

async fn post(dns: &str, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    let r = reqwest::Client::new()
        .post(format!("{}{path}", dns.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .with_context(|| format!("asking {dns}"))?;
    let status = r.status();
    let v: serde_json::Value = r.json().await.unwrap_or_default();
    if !status.is_success() {
        bail!("{path}: {status}: {}", v["error"].as_str().unwrap_or(""));
    }
    Ok(v)
}

/// Register the name, or, once registered, say where the controller is now.
pub async fn register(
    dns: &str,
    reg: &Registration,
    key: &SigningKey,
    address: &str,
) -> Result<String> {
    let at = now();
    let v = post(
        dns,
        "/api/controller",
        serde_json::json!({
            "label": reg.label,
            "controller": reg.controller,
            "entry": serde_json::to_string(&reg.entry)?,
            "sig": reg.name_sig,
            "address": address,
            "at": at,
            "own": address_sig(key, &reg.label, address, at),
        }),
    )
    .await?;
    v["name"]
        .as_str()
        .map(str::to_string)
        .context("the DNS Worker gave no name")
}

/// One challenge, as lego's exec provider asks: `present` or `cleanup`, the
/// record's name (`_acme-challenge.<name>.`) and its value.
pub async fn challenge(
    dns: &str,
    reg: &Registration,
    key: &SigningKey,
    op: &str,
    fqdn: &str,
    value: &str,
) -> Result<()> {
    let label = reg.label.as_str();
    let op = match op {
        "present" | "cleanup" => op,
        _ => bail!("present or cleanup, not {op}"),
    };
    let name = fqdn
        .trim_end_matches('.')
        .strip_prefix("_acme-challenge.")
        .context("not a challenge's name")?;
    let at = now();
    post(
        dns,
        "/api/challenge",
        serde_json::json!({
            "controller": label,
            "at": at,
            "op": op,
            "name": name,
            "value": value,
            "sig": acme_sig(key, label, at, op, name, value),
            // the member's word again, for a member still: whoever has
            // left names nothing and certifies nothing any more
            "key": reg.controller,
            "entry": serde_json::to_string(&reg.entry)?,
            "name_sig": reg.name_sig,
        }),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier as _;

    #[test]
    fn the_words_are_the_workers() {
        let k = SigningKey::from_bytes(&[5; 32]);
        let s = acme_sig(
            &k,
            "abcdefgh234567ab",
            9,
            "present",
            "abcdefgh234567ab.home.x",
            "v",
        );
        let sig = ed25519_dalek::Signature::from_slice(&B64.decode(s).unwrap()).unwrap();
        let msg = "commonty acme v1\0abcdefgh234567ab\09\0present\0abcdefgh234567ab.home.x\0v";
        assert!(k.verifying_key().verify(msg.as_bytes(), &sig).is_ok());
        let a = address_sig(&k, "abcdefgh234567ab", "100.64.0.9", 9);
        let sig = ed25519_dalek::Signature::from_slice(&B64.decode(a).unwrap()).unwrap();
        let msg = "commonty controller address v1\x00abcdefgh234567ab\x00100.64.0.9\x009";
        assert!(k.verifying_key().verify(msg.as_bytes(), &sig).is_ok());
    }

    #[tokio::test]
    async fn only_a_challenge_for_its_own_kind_is_asked() {
        let k = SigningKey::from_bytes(&[5; 32]);
        let reg = Registration {
            name: "l.home.x".into(),
            label: "l".into(),
            controller: "C".into(),
            entry: crate::tests::signed_entry("ann").0,
            name_sig: "S".into(),
        };
        assert!(
            challenge(
                "http://127.0.0.1:9",
                &reg,
                &k,
                "delete",
                "_acme-challenge.x.",
                "v"
            )
            .await
            .is_err()
        );
        assert!(
            challenge("http://127.0.0.1:9", &reg, &k, "present", "x.", "v")
                .await
                .is_err()
        );
    }
}
