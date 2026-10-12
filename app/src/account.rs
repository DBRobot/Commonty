//! Who this device is: its key, the name it signs for, and what the
//! directories say about that name. Signed in means the entry under the
//! name lists this device; the laptop's `dd device admit` puts it there.

use std::time::Duration;

use auth::KeyStore as _;
use serde::Serialize;
use tauri::State;

/// keyring account holding the name this device signs for
pub const USER: &str = "user";

pub struct Keys(pub auth::Store);

/// Which of the app's own screens applies: no name yet, a name whose entry
/// does not list this device (waiting to be approved), or signed in, when
/// the site's own pages take over.
#[derive(Serialize)]
pub struct Status {
    pub name: Option<String>,
    pub admitted: bool,
}

/// the directories this app asks: the front door's, by name
pub fn dirs() -> Vec<String> {
    match std::env::var("COMMONTY_DIRECTORY") {
        Ok(s) if !s.is_empty() => s.split_whitespace().map(str::to_string).collect(),
        _ => directory::DEFAULT
            .iter()
            .filter(|d| d.starts_with("https://"))
            .map(|s| s.to_string())
            .collect(),
    }
}

/// the network's control server: on the gate's host
pub fn control_url() -> String {
    format!("https://headscale.{}", domain())
}

/// the fleet's domain: the directory's host without its home. (or files.)
pub fn domain() -> String {
    let base = gate_base();
    let host = base
        .split("://")
        .nth(1)
        .unwrap_or(&base)
        .split(['/', ':'])
        .next()
        .unwrap_or_default();
    host.strip_prefix("home.")
        .or_else(|| host.strip_prefix("files."))
        .unwrap_or(host)
        .to_string()
}

fn gate_base() -> String {
    dirs()
        .first()
        .map(|d| d.trim_end_matches("/_dd/directory").to_string())
        .unwrap_or_default()
}

#[tauri::command]
pub async fn status(keys: State<'_, Keys>) -> Result<Status, String> {
    let (kp, _fresh) = auth::device::load_or_create(&keys.0).map_err(|e| e.to_string())?;
    let fingerprint = identity::fingerprint(&auth::device::public_b64(&kp));
    let name = keys
        .0
        .get(USER)
        .map_err(|e| e.to_string())?
        .map(|s| s.to_string());
    let mut st = Status {
        name: name.clone(),
        admitted: false,
    };
    let Some(name) = name else { return Ok(st) };
    let found = directory::fetch(&dirs(), &name).await;
    let Some(signed) = directory::newest(&found) else {
        return Ok(st);
    };
    if let Err(e) = identity::verify(&signed) {
        return Err(format!("the entry for {name} does not verify: {e}"));
    }
    st.admitted = signed
        .entry
        .devices
        .iter()
        .any(|d| d.fingerprint == fingerprint);
    Ok(st)
}

#[tauri::command]
pub fn set_name(keys: State<'_, Keys>, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if !identity::valid_name(&name) {
        return Err("a name is lowercase letters, digits and dashes".to_string());
    }
    keys.0.set(USER, &name).map_err(|e| e.to_string())
}

/// This device, joining an account from the code another device of it shows
/// (Settings > Devices > Add a device). Its key is offered against the code
/// with a commitment to a random number of its own; once the approving
/// device's number arrives, its own is revealed, and the six digits both
/// screens show are worked out here from the key and both numbers - not
/// taken from the box that carried them. The account's main key approves
/// there; the status the page checks says when this device is in.
#[tauri::command]
pub async fn join_with_code(keys: State<'_, Keys>, code: String) -> Result<String, String> {
    let (kp, _) = auth::device::load_or_create(&keys.0).map_err(|e| e.to_string())?;
    let public_key = auth::device::public_b64(&kp);
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| e.to_string())?;
    let mine: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let code = code.trim().to_string();
    let base = format!("https://home.{}/_dd/add", domain());
    let http = directory::http().map_err(|e| e.to_string())?;
    let r = http
        .post(format!("{base}/offer"))
        .json(&serde_json::json!({
            "code": code,
            "public_key": public_key,
            "commit": identity::pairing_commit(&public_key, &mine),
        }))
        .send()
        .await
        .map_err(|e| format!("the box did not answer: {e}"))?;
    if !r.status().is_success() {
        return Err(r
            .text()
            .await
            .unwrap_or_else(|_| "that did not work".into()));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
    let user = v["user"]
        .as_str()
        .ok_or("the box gave no name")?
        .to_string();
    // the other device's number, once it has seen this one's offer
    let theirs = loop {
        let w: serde_json::Value = http
            .get(format!("{base}/wait"))
            .query(&[("code", &code)])
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|_| "that code ran out; make a new one on your other device".to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        if let Some(t) = w["theirs"].as_str() {
            break t.to_string();
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    http.post(format!("{base}/reveal"))
        .json(&serde_json::json!({ "code": code, "nonce": mine }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    keys.0.set(USER, &user).map_err(|e| e.to_string())?;
    Ok(identity::pairing_digits(&public_key, &mine, &theirs))
}

/// A new member, from a code: the invite it derives is fetched from the
/// directory, a root and a recovery key are made here, the entry with its
/// grant and the terms agreed to is published, the root goes into the
/// keystore. The recovery key comes back once, for the page to show and
/// never keep. The page asked the age question itself; the birth date never
/// reaches here, only that the terms were agreed to.
#[tauri::command]
pub async fn sign_up(
    keys: State<'_, Keys>,
    name: String,
    code: String,
    agreed: bool,
) -> Result<String, String> {
    if age_refused_here(&keys) {
        return Err(AGE_REFUSED_TEXT.to_string());
    }
    if !agreed {
        return Err("agree to the Terms of use and the Privacy policy first".to_string());
    }
    let name = name.trim().to_string();
    if !identity::valid_name(&name) {
        return Err("a name is lowercase letters, digits and dashes".to_string());
    }
    if code.trim().is_empty() {
        return Err("the invite code is what lets you in".to_string());
    }
    let (kp, _) = auth::device::load_or_create(&keys.0).map_err(|e| e.to_string())?;
    let recovery = account::create(
        &keys.0,
        &dirs(),
        &name,
        &kp,
        Some(code.trim()),
        identity::TERMS,
    )
    .await
    .map_err(|e| format!("{e:#}"))?;
    keys.0.set(USER, &name).map_err(|e| e.to_string())?;
    Ok(recovery.to_string())
}

/// what the store keeps once the age question was answered under 13 on
/// this device: that alone, never the answer
const AGE_REFUSED: &str = "age-refused";
const AGE_REFUSED_TEXT: &str = "Commonty is only for people aged 13 or over.";

fn age_refused_here(keys: &Keys) -> bool {
    keys.0.get(AGE_REFUSED).ok().flatten().is_some()
}

/// The sign-up's age question said under 13 on this device: kept, so going
/// back and answering again changes nothing
#[tauri::command]
pub fn age_refused(keys: State<'_, Keys>) -> bool {
    age_refused_here(&keys)
}

#[tauri::command]
pub fn refuse_age(keys: State<'_, Keys>) -> Result<(), String> {
    keys.0.set(AGE_REFUSED, "1").map_err(|e| e.to_string())
}

/// The Terms of use or the Privacy policy, from the sign-up's "I agree",
/// in the system's browser: the window keeps the sign-up
#[tauri::command]
pub fn open_policy<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    which: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;
    let page = match which.as_str() {
        "terms" => "terms",
        "privacy" => "privacy",
        _ => return Err("no such page".into()),
    };
    app.opener()
        .open_url(
            format!("https://{}/docs/policies/{page}", domain()),
            None::<&str>,
        )
        .map_err(|e| e.to_string())
}

fn root_here(keys: &Keys) -> Result<ed25519_dalek::SigningKey, String> {
    account::load_root(&keys.0)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the account's root key is not on this device: add devices from the one that made the account".to_string())
}

fn name_here(keys: &Keys) -> Result<String, String> {
    keys.0
        .get(USER)
        .map_err(|e| e.to_string())?
        .map(|s| s.to_string())
        .ok_or_else(|| "no name on this device".to_string())
}

#[tauri::command]
pub async fn passkey_add(app: tauri::AppHandle, keys: State<'_, Keys>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;
    let root = root_here(&keys)?;
    let name = name_here(&keys)?;
    let (kp, _) = auth::device::load_or_create(&keys.0).map_err(|e| e.to_string())?;
    let tok = auth::device::mint_for(&kp, &name, Duration::from_secs(600), Some("enrol"))
        .map_err(|e| e.to_string())?;
    let enrol = format!("{gate}/_dd/enrol", gate = gate_base());
    app.opener()
        .open_url(format!("{enrol}?t={tok}"), None::<&str>)
        .map_err(|e| format!("could not open the browser: {e}"))?;
    let http = directory::http().map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    let cred = loop {
        if std::time::Instant::now() > deadline {
            return Err("no passkey arrived within ten minutes".into());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let r = http
            .get(format!("{enrol}/result"))
            .bearer_auth(&tok)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        match r.status().as_u16() {
            200 => {
                break r
                    .json::<identity::Passkey>()
                    .await
                    .map_err(|e| e.to_string())?;
            }
            204 => continue,
            s => return Err(format!("{s} {}", r.text().await.unwrap_or_default())),
        }
    };
    account::admit_passkey(&dirs(), &name, &root, cred)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

#[tauri::command]
pub async fn recover(
    app: tauri::AppHandle,
    keys: State<'_, Keys>,
    name: String,
    recovery: String,
) -> Result<String, String> {
    let name = name.trim().to_string();
    if !identity::valid_name(&name) {
        return Err("a name is lowercase letters, digits and dashes".to_string());
    }
    let (kp, _) = auth::device::load_or_create(&keys.0).map_err(|e| e.to_string())?;
    // and the entry as it was, held to the release as the recovery was:
    // whose devices signed what was sealed to the paper key
    let (next, before) = account::recover(&keys.0, &dirs(), &name, recovery.trim(), &kp)
        .await
        .map_err(|e| format!("{e:#}"))?;
    keys.0.set(USER, &name).map_err(|e| e.to_string())?;
    // Photos' password, out of its copy sealed to the paper key just used
    if let Ok(paper) = identity::decode_secret(recovery.trim())
        && let Err(e) = crate::photos::recovered(&app, &paper, &before.entry).await
    {
        eprintln!("recover: photos: {e}");
    }
    Ok(next.to_string())
}

/// leave: the name goes, the device key stays (a key is cheap to keep and
/// costly to re-admit)
#[tauri::command]
pub fn forget(keys: State<'_, Keys>) -> Result<(), String> {
    crate::net::forget_browser();
    // the photo password, and the root this device vouched for, went with
    // the name they were for
    let _ = keys.0.clear(crate::photos::KEY);
    let _ = keys.0.clear(crate::photos::LEGACY);
    let _ = keys.0.clear(account::ROOT_PIN);
    keys.0.clear(USER).map_err(|e| e.to_string())
}
