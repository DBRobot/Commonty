//! The browser side of Photos. A person whose key is a passkey has no
//! password; the passkey's PRF output is the password, and this code, run
//! in the page, makes or opens their ente account with it. ente's key
//! derivation, SRP and secret boxes are ente's crates compiled to wasm; the
//! master key exists in the browser and nowhere else.

use ente_accounts::client::AccountsClient;
use ente_accounts::error::{Error, Result};
use ente_accounts::flow::{
    AuthFlow, AuthFlowUi, AuthenticatedAccount, ChangePasswordParams, CreateAccountParams,
    LoginParams, OtpPurpose, SecondFactorMethod, TotpPurpose,
};
use ente_accounts::types::AccountsClientConfig;
use ente_core::b64;
use ente_core::crypto;
use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

pub mod archive;
pub mod entry;
pub mod library;

/// No prompts: the code is given up front, and an account made here has
/// no second factor.
struct NoUi;

impl AuthFlowUi for NoUi {
    fn read_email_otp(&mut self, _: &str, _: OtpPurpose, _: bool) -> Result<String> {
        Err(Error::Decode("this account asks for an email code".into()))
    }
    fn read_totp_code(&mut self, _: TotpPurpose) -> Result<String> {
        Err(Error::Decode("this account has a second factor".into()))
    }
    fn report_retryable_error(&mut self, message: &str) -> Result<()> {
        Err(Error::Decode(message.into()))
    }
    fn choose_second_factor(&mut self, _: &[SecondFactorMethod]) -> Result<SecondFactorMethod> {
        Err(Error::Decode("this account has a second factor".into()))
    }
    fn present_passkey_verification(&mut self, _: &str) -> Result<()> {
        Err(Error::Decode("this account has a second factor".into()))
    }
    fn wait_for_passkey_verification(&mut self) -> Result<()> {
        Err(Error::Decode("this account has a second factor".into()))
    }
    fn present_totp_secret(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
}

/// What ente's web app needs in its storage to be signed in, in the shapes
/// it reads: `user` and `keyAttributes` in localStorage, the token in its
/// kv store, and the master key in sessionStorage sealed with a key that
/// lives only in that tab.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    user_id: i64,
    email: String,
    token: String,
    key_attributes: serde_json::Value,
    session_key: SessionKey,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionKey {
    encrypted_data: String,
    key: String,
    nonce: String,
}

fn client(origin: &str) -> Result<AccountsClient> {
    AccountsClient::new(AccountsClientConfig {
        origin: origin.trim_end_matches('/').to_string(),
        auth_token: None,
        client_package: "io.ente.photos.web".into(),
        client_version: None,
        user_agent: None,
    })
}

fn session(email: &str, account: AuthenticatedAccount) -> Result<String> {
    let key = crypto::Key::generate();
    let sealed = crypto::secretbox::encrypt(&account.secrets.master_key, &key);
    let s = Session {
        user_id: account.user_id,
        email: email.to_string(),
        // url-safe with padding: what ente web keeps and museum decodes
        token: b64::encode_url_safe(&account.secrets.token),
        key_attributes: serde_json::to_value(&account.key_attributes)?,
        session_key: SessionKey {
            encrypted_data: b64::encode(&sealed.encrypted_data),
            key: b64::encode(key.as_bytes()),
            nonce: b64::encode(sealed.nonce.as_bytes()),
        },
    };
    Ok(serde_json::to_string(&s)?)
}

fn js(e: Error) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Open the account: SRP with the password, the master key unwrapped here.
#[wasm_bindgen]
pub async fn ente_login(
    origin: &str,
    email: &str,
    password: &str,
) -> std::result::Result<String, JsValue> {
    let client = client(origin).map_err(js)?;
    let mut ui = NoUi;
    let mut flow = AuthFlow::new(&client, &mut ui);
    let account = flow
        .login(LoginParams {
            email: email.to_string(),
            password: Zeroizing::new(password.to_string()),
        })
        .await
        .map_err(js)?;
    session(email, account).map_err(js)
}

/// Make the account: the verification code is the fleet's, for addresses
/// under its own domain; the keys are made and wrapped here.
#[wasm_bindgen]
pub async fn ente_create(
    origin: &str,
    email: &str,
    password: &str,
    code: &str,
) -> std::result::Result<String, JsValue> {
    let client = client(origin).map_err(js)?;
    // museum takes the code only once it has opened one for the address
    client.send_otp(email, "signup").await.map_err(js)?;
    let mut ui = NoUi;
    let mut flow = AuthFlow::new(&client, &mut ui);
    let account = flow
        .create_account_with_otp(
            CreateAccountParams {
                email: email.to_string(),
                password: Zeroizing::new(password.to_string()),
                source: None,
            },
            code,
        )
        .await
        .map_err(js)?;
    session(email, account).map_err(js)
}

/// An account that already exists under another address and password, made
/// the passkey's: sign in with the old pair once, move it to the fleet's
/// address, re-key it to the passkey's secret. The photos stay where they
/// are; the master key is re-wrapped here, in the browser.
#[wasm_bindgen]
pub async fn ente_adopt(
    origin: &str,
    old_email: &str,
    old_password: &str,
    email: &str,
    password: &str,
    code: &str,
) -> std::result::Result<String, JsValue> {
    let client = client(origin).map_err(js)?;
    let mut ui = NoUi;
    let account = {
        let mut flow = AuthFlow::new(&client, &mut ui);
        flow.login(LoginParams {
            email: old_email.to_string(),
            password: Zeroizing::new(old_password.to_string()),
        })
        .await
        .map_err(js)?
    };
    // museum reads the token as url-safe base64, as ente's own clients send it
    client.set_auth_token(Some(b64::encode_url_safe(&account.secrets.token)));
    // museum only takes a code it has opened for that address, and opens
    // the fleet's only when asked as for a sign-up (a "change" gets a random
    // one, mailed to an address that has no mailbox)
    client.send_otp(email, "signup").await.map_err(js)?;
    client.change_email(email, code).await.map_err(js)?;
    let changed = {
        let flow = AuthFlow::new(&client, &mut ui);
        flow.change_password(ChangePasswordParams {
            email: email.to_string(),
            password: Zeroizing::new(password.to_string()),
            master_key: crypto::SecretVec::new(account.secrets.master_key.clone()),
            key_attributes: account.key_attributes.clone(),
            log_out_other_devices: false,
        })
        .await
        .map_err(js)?
    };
    session(
        email,
        AuthenticatedAccount {
            user_id: account.user_id,
            key_attributes: changed.key_attributes,
            secrets: account.secrets,
            recovery_key: None,
        },
    )
    .map_err(js)
}
