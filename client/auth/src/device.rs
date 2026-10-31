//! The key a person actually holds.
//!
//! An ed25519 key, generated here, private half in the OS keyring. Its public
//! half is admitted to the person's own signed entry (the identity crate),
//! and from then on this device signs its own tokens. No server ever holds
//! anything that can sign as you - the most a compromised one can do is
//! serve what it already serves.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
/// the device key type, for callers that hold one without depending on biscuit
pub use biscuit_auth::KeyPair;
use biscuit_auth::{Biscuit, PrivateKey, builder::Algorithm};
use zeroize::Zeroizing;

use crate::{KeyStore, Result};

/// keyring account holding the private key, base64
pub const ACCOUNT: &str = "device-key";

pub fn generate() -> KeyPair {
    KeyPair::new_with_algorithm(Algorithm::Ed25519)
}

pub fn encode_private(kp: &KeyPair) -> Zeroizing<String> {
    Zeroizing::new(B64.encode(kp.private().to_bytes().as_slice()))
}

pub fn decode_private(s: &str) -> Result<KeyPair> {
    let bytes = Zeroizing::new(
        B64.decode(s.trim())
            .map_err(|e| crate::Error::Token(format!("device key: {e}")))?,
    );
    let sk = PrivateKey::from_bytes(&bytes, Algorithm::Ed25519)
        .map_err(|e| crate::Error::Token(format!("device key: {e}")))?;
    Ok(KeyPair::from(&sk))
}

/// The public half, as sent to the verifier and stored in its directory.
pub fn public_b64(kp: &KeyPair) -> String {
    B64.encode(kp.public().to_bytes())
}

/// Load the device key from the keyring, if this device has one.
pub fn load(keys: &impl KeyStore) -> Result<Option<KeyPair>> {
    match keys.get(ACCOUNT)? {
        Some(s) => Ok(Some(decode_private(&s)?)),
        None => Ok(None),
    }
}

/// Load or create, storing a new key in the keyring.
pub fn load_or_create(keys: &impl KeyStore) -> Result<(KeyPair, bool)> {
    if let Some(kp) = load(keys)? {
        return Ok((kp, false));
    }
    let kp = generate();
    keys.set(ACCOUNT, &encode_private(&kp))?;
    Ok((kp, true))
}

/// A token good for `ttl` from now, signed by this device, naming the user.
pub fn mint(kp: &KeyPair, user: &str, ttl: Duration) -> Result<String> {
    mint_for(kp, user, ttl, None)
}

/// The same, restricted to one operation. The verifier states what a request
/// is for ("access", "enrol") and a token that names one is refused for the
/// other: an enrol link in a terminal scrollback opens no file.
pub fn mint_for(
    kp: &KeyPair,
    user: &str,
    ttl: Duration,
    operation: Option<&str>,
) -> Result<String> {
    mint_at(kp, user, ttl, operation, None)
}

/// The same again, good at one name and nowhere else.
///
/// `audience` is the host this token will be sent to - `files.example`,
/// not the box behind it. A box refuses a token whose audience is some
/// other name, so one taken from a service that has been got at opens
/// that service and nothing else on the fleet. Minting happens here, on
/// the device, with no round trip, so a caller can make exactly the token
/// for the call it is about to make and it costs nothing to be narrow.
///
/// None keeps the old shape: good anywhere the user is known. `dd token`
/// still prints one of those unless asked otherwise, because a token
/// typed into curl by hand has no one host in mind.
pub fn mint_at(
    kp: &KeyPair,
    user: &str,
    ttl: Duration,
    operation: Option<&str>,
    audience: Option<&str>,
) -> Result<String> {
    let exp = (SystemTime::now() + ttl)
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let err = |e: biscuit_auth::error::Token| crate::Error::Token(format!("mint: {e}"));
    let token = Biscuit::builder()
        .fact(format!("user({:?})", user).as_str())
        .map_err(err)?
        .fact(format!("device({:?})", fingerprint(kp)).as_str())
        .map_err(err)?
        // the check is in the token itself, so even a verifier that forgot to
        // supply the time could not accept an expired one
        .check(format!("check if time($t), $t < {exp}").as_str())
        .map_err(err)?;
    // both directions: the check keeps this token out of everything else,
    // the fact is what an operation that demands a purpose-built token asks for
    let token = match audience {
        // both directions again: the fact is what the box's deny rule reads,
        // the check is what keeps this token out of any other name
        Some(a) => token
            .fact(format!("audience({a:?})").as_str())
            .map_err(err)?
            .check(format!("check if here({a:?})").as_str())
            .map_err(err)?,
        None => token,
    };
    let token = match operation {
        Some(op) => token
            .fact(format!("purpose({op:?})").as_str())
            .map_err(err)?
            .check(format!("check if operation({op:?})").as_str())
            .map_err(err)?,
        None => token,
    };
    token.build(kp).map_err(err)?.to_base64().map_err(err)
}

/// What a browser's sign-in grant is for: a box takes it for a session and
/// for nothing else, and takes nothing else for one
pub const SIGNIN: &str = "browser-signin";
/// how long a grant lives: the browser carries it straight to the box
pub const SIGNIN_TTL: Duration = Duration::from_secs(60);
/// What the call that ends a browser's sessions on a box is for
pub const SIGNOUT: &str = "browser-signout";

/// A sign-in grant for a browser this device let in (the app, local.rs):
/// good at `host` alone, for SIGNIN alone, for a minute, and naming the
/// one-time `nonce` that box gave the browser, which it spends on taking
/// it. It makes a session there, as a passkey sign-in would, and opens
/// nothing itself. `sid` is this device's sign-in id for that browser: the
/// box's session is made from it, so ending it (SIGNOUT) ends them all.
pub fn mint_signin(kp: &KeyPair, user: &str, host: &str, nonce: &str, sid: &str) -> Result<String> {
    let exp = (SystemTime::now() + SIGNIN_TTL)
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let err = |e: biscuit_auth::error::Token| crate::Error::Token(format!("mint: {e}"));
    Biscuit::builder()
        .fact(format!("user({user:?})").as_str())
        .map_err(err)?
        .fact(format!("device({:?})", fingerprint(kp)).as_str())
        .map_err(err)?
        .fact(format!("purpose({SIGNIN:?})").as_str())
        .map_err(err)?
        .fact(format!("audience({host:?})").as_str())
        .map_err(err)?
        .fact(format!("nonce({nonce:?})").as_str())
        .map_err(err)?
        .fact(format!("sid({sid:?})").as_str())
        .map_err(err)?
        .check(format!("check if time($t), $t < {exp}").as_str())
        .map_err(err)?
        .check(format!("check if here({host:?})").as_str())
        .map_err(err)?
        .check(format!("check if operation({SIGNIN:?})").as_str())
        .map_err(err)?
        .build(kp)
        .map_err(err)?
        .to_base64()
        .map_err(err)
}

/// How long a join ticket is good for: the drive it is written to can sit
/// a week before the box is set up.
pub const JOIN_TICKET_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// A member's join ticket (docs/dev/joining.md): a token for the one
/// operation `join`, naming a ticket id of 16 random bytes. A box set up
/// with it joins as this member's, and it writes that box's backup under
/// /_dd/images/pending/<id>/ and nowhere else; it opens nothing of the
/// member's, as any purpose-built token opens nothing else. The ticket and
/// its id, hex.
pub fn mint_join_ticket(kp: &KeyPair, user: &str) -> Result<(String, String)> {
    // 16 bytes from the OS's generator: the seed of a key made for nothing
    // else, which is what the generator gave it
    let id: String = generate()
        .private()
        .to_bytes()
        .iter()
        .take(16)
        .map(|x| format!("{x:02x}"))
        .collect();
    let exp = (SystemTime::now() + JOIN_TICKET_TTL)
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let err = |e: biscuit_auth::error::Token| crate::Error::Token(format!("mint: {e}"));
    let token = Biscuit::builder()
        .fact(format!("user({user:?})").as_str())
        .map_err(err)?
        .fact(format!("device({:?})", fingerprint(kp)).as_str())
        .map_err(err)?
        .fact(format!("ticket({id:?})").as_str())
        .map_err(err)?
        .fact("purpose(\"join\")")
        .map_err(err)?
        .check(format!("check if time($t), $t < {exp}").as_str())
        .map_err(err)?
        .check("check if operation(\"join\")")
        .map_err(err)?;
    Ok((token.build(kp).map_err(err)?.to_base64().map_err(err)?, id))
}

/// Short, stable name for a key, the same one the directory entry shows.
pub fn fingerprint(kp: &KeyPair) -> String {
    let b = kp.public().to_bytes();
    b.iter().take(6).map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod join_ticket_tests {
    use super::*;

    #[test]
    fn a_signin_grant_names_its_box_its_challenge_and_its_minute() {
        let kp = generate();
        let t = mint_signin(&kp, "ann", "files.example", "n0nce", "s1d").unwrap();
        let src = biscuit_auth::UnverifiedBiscuit::from_base64(&t)
            .unwrap()
            .print_block_source(0)
            .unwrap();
        for want in [
            "user(\"ann\");",
            "purpose(\"browser-signin\");",
            "audience(\"files.example\");",
            "nonce(\"n0nce\");",
            "sid(\"s1d\");",
            "check if here(\"files.example\");",
            "check if operation(\"browser-signin\");",
        ] {
            assert!(src.lines().any(|l| l.trim() == want), "{want} not in {src}");
        }
        let exp: u64 = src
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("check if time($t), $t < ")?
                    .strip_suffix(';')?
                    .parse()
                    .ok()
            })
            .unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(exp > now && exp <= now + 61, "{exp} {now}");
    }

    #[test]
    fn a_join_ticket_names_its_member_its_id_and_join_alone() {
        let kp = generate();
        let (t, id) = mint_join_ticket(&kp, "ann").unwrap();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|c| c.is_ascii_hexdigit()));
        let src = biscuit_auth::UnverifiedBiscuit::from_base64(&t)
            .unwrap()
            .print_block_source(0)
            .unwrap();
        for want in [
            "user(\"ann\")".to_string(),
            format!("ticket(\"{id}\")"),
            "purpose(\"join\")".to_string(),
            "check if operation(\"join\")".to_string(),
        ] {
            assert!(src.contains(&want), "{want} not in {src}");
        }
        // two tickets are never one
        assert_ne!(mint_join_ticket(&kp, "ann").unwrap().1, id);
    }
}
