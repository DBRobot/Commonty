//! A signed-in session in the shapes Ente's web app reads to be signed in
//! (`seed()` in app/web/photos-open.js): `user` and `keyAttributes` in
//! localStorage, the token in its kv store, and the master key in
//! sessionStorage sealed with a key that lives only in that tab. The page
//! makes one itself (client/web), or the app makes one once and hands it
//! to each page it opens (app/src/photos.rs).

use ente_core::{b64, crypto};
use zeroize::Zeroizing;

use crate::{AuthFlow, AuthenticatedAccount, LoginParams, Result};

/// The client Ente's web app is: a token it is given is one museum takes
/// from that app's pages
pub fn web_client(origin: &str) -> Result<crate::AccountsClient> {
    crate::AccountsClient::new(crate::AccountsClientConfig {
        origin: origin.trim_end_matches('/').to_string(),
        auth_token: None,
        client_package: "io.ente.photos.web".into(),
        client_version: None,
        user_agent: None,
    })
}

pub struct Session {
    pub user_id: i64,
    pub email: String,
    /// url-safe with padding: what Ente's web app keeps and museum decodes
    pub token: Zeroizing<String>,
    pub key_attributes: serde_json::Value,
    master: Zeroizing<Vec<u8>>,
}

impl Session {
    pub fn new(email: &str, account: &AuthenticatedAccount) -> Result<Self> {
        Ok(Self {
            user_id: account.user_id,
            email: email.to_string(),
            token: Zeroizing::new(b64::encode_url_safe(&account.secrets.token)),
            key_attributes: serde_json::to_value(&account.key_attributes)?,
            master: Zeroizing::new(account.secrets.master_key.clone()),
        })
    }

    /// Open the account with its password: SRP, the master key unwrapped
    /// here
    pub async fn sign_in(origin: &str, email: &str, password: &str) -> Result<Self> {
        let client = web_client(origin)?;
        let mut ui = crate::gallery::NoUi;
        let account = AuthFlow::new(&client, &mut ui)
            .login(LoginParams {
                email: email.to_string(),
                password: Zeroizing::new(password.to_string()),
            })
            .await?;
        Self::new(email, &account)
    }

    /// Whether museum still takes the token, with the key attributes it
    /// has for the account now (another device may have changed them)
    pub async fn still_good(&self, origin: &str) -> Result<bool> {
        let client = web_client(origin)?;
        client.set_auth_token(Some(self.token.to_string()));
        match client.get_session_validity().await {
            Ok(v) => Ok(match v.key_attributes {
                Some(k) => serde_json::to_value(&k)? == self.key_attributes,
                None => false,
            }),
            Err(crate::Error::SessionInvalid) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// For one tab: the master key sealed with a key made now
    pub fn seed(&self) -> serde_json::Value {
        let key = crypto::Key::generate();
        let sealed = crypto::secretbox::encrypt(&self.master, &key);
        serde_json::json!({
            "userId": self.user_id,
            "email": self.email,
            "token": self.token.as_str(),
            "keyAttributes": self.key_attributes,
            "sessionKey": {
                "encryptedData": b64::encode(&sealed.encrypted_data),
                "key": b64::encode(key.as_bytes()),
                "nonce": b64::encode(sealed.nonce.as_bytes()),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(master: &[u8]) -> Session {
        Session {
            user_id: 7,
            email: "a@users.example.org".into(),
            token: Zeroizing::new("dG9rZW4=".into()),
            key_attributes: serde_json::json!({ "kekSalt": "s" }),
            master: Zeroizing::new(master.to_vec()),
        }
    }

    #[test]
    fn each_seed_opens_to_the_master_key_under_its_own_key() {
        let master = [9u8; 32];
        let s = session(&master);
        let (a, b) = (s.seed(), s.seed());
        assert_ne!(a["sessionKey"]["key"], b["sessionKey"]["key"]);
        for v in [a, b] {
            assert_eq!(v["userId"], 7);
            assert_eq!(v["email"], "a@users.example.org");
            assert_eq!(v["token"], "dG9rZW4=");
            assert_eq!(v["keyAttributes"]["kekSalt"], "s");
            let k = &v["sessionKey"];
            let open = |f: &str| b64::decode(k[f].as_str().unwrap()).unwrap();
            let back = crypto::secretbox::decrypt(
                &open("encryptedData"),
                &crypto::Nonce::try_from_slice(&open("nonce")).unwrap(),
                &crypto::Key::try_from_slice(&open("key")).unwrap(),
            )
            .unwrap();
            assert_eq!(back, master);
        }
    }
}
