//! A per-box OpenID Connect issuer, for the one service that signs people
//! in no other way: Passwords (Vaultwarden, modules/vault). Its signing key is
//! made here on first start and trusted by exactly one client on exactly this
//! box. What it vouches for is who someone is; it opens no vault - each is
//! still encrypted with its owner's master password on their own devices. The
//! person signs in with a passkey at the verifier; this only translates that
//! session into the shape the client asks for.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use p256::ecdsa::SigningKey;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use serde::Serialize;

use crate::session::{now, random_id};

pub struct Issuer {
    pub issuer: String,
    /// a member's address in the client: <name>@<this>, never their own
    email_domain: String,
    pub client_id: String,
    client_secret: String,
    pub redirect_uri: String,
    key: SigningKey,
    encoding: jsonwebtoken::EncodingKey,
    kid: String,
    codes: Mutex<HashMap<String, Grant>>,
    tokens: Mutex<HashMap<String, Grant>>,
}

#[derive(Clone)]
struct Grant {
    user: String,
    nonce: Option<String>,
    /// PKCE: the S256 challenge the client made; the token request must
    /// bring what hashes to it
    challenge: Option<String>,
    issued: u64,
}

impl Issuer {
    pub fn open(
        dir: &Path,
        issuer: String,
        client_id: String,
        client_secret: String,
        redirect_uri: String,
        email_domain: String,
    ) -> Result<Self> {
        let p = dir.join("oidc.key");
        let key = match std::fs::read(&p) {
            Ok(der) => {
                SigningKey::from_pkcs8_der(&der).context("oidc.key is not a pkcs8 p256 key")?
            }
            Err(_) => {
                let k = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
                let der = k.to_pkcs8_der()?;
                std::fs::write(&p, der.as_bytes()).context("writing oidc.key")?;
                k
            }
        };
        let der = key.to_pkcs8_der()?;
        let encoding = jsonwebtoken::EncodingKey::from_ec_der(der.as_bytes());
        let point = key.verifying_key().to_encoded_point(false);
        let kid: String = point
            .x()
            .unwrap()
            .iter()
            .take(6)
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(Self {
            issuer,
            email_domain,
            client_id,
            client_secret,
            redirect_uri,
            key,
            encoding,
            kid,
            codes: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        })
    }

    pub fn discovery(&self) -> serde_json::Value {
        let i = &self.issuer;
        serde_json::json!({
            "issuer": i,
            "authorization_endpoint": format!("{i}/authorize"),
            "token_endpoint": format!("{i}/token"),
            "userinfo_endpoint": format!("{i}/userinfo"),
            "jwks_uri": format!("{i}/jwks"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"],
            "scopes_supported": ["openid", "profile", "email"],
            "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
            "claims_supported": ["sub", "preferred_username", "name", "email", "email_verified"],
            "code_challenge_methods_supported": ["S256"],
            "grant_types_supported": ["authorization_code"],
        })
    }

    pub fn jwks(&self) -> serde_json::Value {
        let point = self.key.verifying_key().to_encoded_point(false);
        serde_json::json!({ "keys": [{
            "kty": "EC", "crv": "P-256", "use": "sig", "alg": "ES256", "kid": self.kid,
            "x": B64.encode(point.x().unwrap()),
            "y": B64.encode(point.y().unwrap()),
        }]})
    }

    /// An authorization code for a user who has just proven a session.
    pub fn code(&self, user: &str, nonce: Option<String>, challenge: Option<String>) -> String {
        let code = random_id();
        let mut codes = self.codes.lock().unwrap();
        // codes nobody redeemed go when the next one is made
        codes.retain(|_, g| now() <= g.issued + 300);
        codes.insert(
            code.clone(),
            Grant {
                user: user.to_string(),
                nonce,
                challenge,
                issued: now(),
            },
        );
        code
    }

    pub fn client_ok(&self, id: &str, secret: &str) -> bool {
        crate::session::same(id.as_bytes(), self.client_id.as_bytes())
            & crate::session::same(secret.as_bytes(), self.client_secret.as_bytes())
    }

    /// Redeem a code: an id token plus an opaque access token for userinfo.
    pub fn redeem(&self, code: &str, verifier: Option<&str>) -> Option<serde_json::Value> {
        let grant = self.codes.lock().unwrap().remove(code)?;
        if now() > grant.issued + 300 {
            return None;
        }
        if let Some(challenge) = &grant.challenge {
            use sha2::{Digest, Sha256};
            let made = B64.encode(Sha256::digest(verifier?.as_bytes()));
            if !crate::session::same(made.as_bytes(), challenge.as_bytes()) {
                return None;
            }
        }
        let email = format!("{}@{}", grant.user, self.email_domain);
        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            sub: &'a str,
            aud: &'a str,
            exp: u64,
            iat: u64,
            preferred_username: &'a str,
            name: &'a str,
            email: &'a str,
            email_verified: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            nonce: Option<String>,
        }
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        let id_token = jsonwebtoken::encode(
            &header,
            &Claims {
                iss: &self.issuer,
                sub: &grant.user,
                aud: &self.client_id,
                exp: now() + 3600,
                iat: now(),
                preferred_username: &grant.user,
                name: &grant.user,
                email: &email,
                email_verified: true,
                nonce: grant.nonce.clone(),
            },
            &self.encoding,
        )
        .ok()?;
        let access = random_id();
        let mut tokens = self.tokens.lock().unwrap();
        tokens.retain(|_, g| now() <= g.issued + 3600);
        tokens.insert(access.clone(), grant);
        drop(tokens);
        Some(serde_json::json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": 3600,
            "id_token": id_token,
        }))
    }

    pub fn userinfo(&self, access: &str) -> Option<serde_json::Value> {
        let g = self.tokens.lock().unwrap().get(access).cloned()?;
        if now() > g.issued + 3600 {
            return None;
        }
        let email = format!("{}@{}", g.user, self.email_domain);
        Some(
            serde_json::json!({ "sub": g.user, "preferred_username": g.user, "name": g.user, "email": email, "email_verified": true }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn issuer() -> (Issuer, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("dd-oidc-{}", random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let i = Issuer::open(
            &dir,
            "https://home.x/_dd/oidc".into(),
            "vault".into(),
            "s3cret".into(),
            "https://vault.x/identity/connect/oidc-signin".into(),
            "x".into(),
        )
        .unwrap();
        (i, dir)
    }

    #[test]
    fn a_code_made_with_a_challenge_needs_its_verifier() {
        let (i, dir) = issuer();
        let verifier = "a-long-random-verifier-the-client-kept";
        let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
        let code = i.code("tom", None, Some(challenge.clone()));
        assert!(
            i.redeem(&code, Some("something else")).is_none(),
            "a wrong verifier"
        );
        let code = i.code("tom", None, Some(challenge.clone()));
        assert!(i.redeem(&code, None).is_none(), "no verifier at all");
        let code = i.code("tom", None, Some(challenge));
        let t = i.redeem(&code, Some(verifier)).expect("the right one");
        // and a code is good once
        assert!(i.redeem(&code, Some(verifier)).is_none());
        let info = i.userinfo(t["access_token"].as_str().unwrap()).unwrap();
        assert_eq!(info["email"], "tom@x");
        assert_eq!(info["email_verified"], true);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn only_the_one_client_redeems() {
        let (i, dir) = issuer();
        assert!(i.client_ok("vault", "s3cret"));
        assert!(!i.client_ok("vault", "guess"));
        assert!(!i.client_ok("other", "s3cret"));
        std::fs::remove_dir_all(dir).ok();
    }
}
