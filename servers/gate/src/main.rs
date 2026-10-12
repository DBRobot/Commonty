use anyhow::{Context, Result};

fn env(k: &str) -> Result<String> {
    std::env::var(k).with_context(|| format!("{k} is not set"))
}
fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let role = env_or("VERIFY_ROLE", "full");
    let full = role != "directory";
    // the pages the full gate serves, all read now: a release missing one
    // does not come up at all (pages::load)
    if full {
        let dir = env("VERIFY_PAGES_DIR")?;
        verify::pages::load(std::path::Path::new(&dir)).context("the pages")?;
    }
    // Pi-hole's api password, for the gate alone (modules/adblock)
    let adblock_password = match std::env::var("VERIFY_ADBLOCK_PASSWORD_FILE") {
        Ok(f) if !f.is_empty() => Some(
            std::fs::read_to_string(&f)
                .with_context(|| format!("reading {f}"))?
                .trim()
                .to_string(),
        ),
        _ => None,
    };
    let cfg = verify::Config {
        bind: env_or("VERIFY_BIND", "127.0.0.1:4181").parse()?,
        dir: env("VERIFY_DIR")?.into(),
        house_peers: env_or("VERIFY_HOUSE_PEERS", "")
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
        // what else this box is asked under (its fleet address): a token's
        // audience must be one of its own names
        names: env_or("VERIFY_NAMES", "")
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect(),
        // the mail Worker's key (fleet/mail.json): single entries, never the list
        directory_readers: env_or("VERIFY_DIRECTORY_READERS", "")
            .split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .collect(),
        control: std::env::var("VERIFY_CONTROL")
            .ok()
            .filter(|c| !c.is_empty())
            .map(|c| {
                (
                    c,
                    env_or("VERIFY_CONTROL_SUBMIT", "/run/dd-control/submit.sock").into(),
                    env_or("VERIFY_CONTROL_CONFIG", "/etc/dd-control/config.json").into(),
                )
            }),
        domain: if full {
            Some(env("VERIFY_DOMAIN")?)
        } else {
            None
        },
        home: serde_json::from_str(&env_or("VERIFY_HOME", "[]")).context("VERIFY_HOME")?,
        // every box and where its prometheus answers, for the pages that
        // show the fleet; a box that is not told has none to show
        fleet: serde_json::from_str(&env_or("VERIFY_FLEET", "{}")).context("VERIFY_FLEET")?,
        thanos: std::env::var("VERIFY_THANOS")
            .ok()
            .filter(|s| !s.is_empty()),
        releases: std::env::var("VERIFY_RELEASES")
            .ok()
            .filter(|s| !s.is_empty()),
        forge_events: std::env::var("VERIFY_FORGE_EVENTS")
            .ok()
            .filter(|s| !s.is_empty()),
        // what the storage ledger counted, for the allowance (storage.rs)
        storage_ledger: std::env::var("VERIFY_STORAGE_LEDGER")
            .ok()
            .filter(|s| !s.is_empty())
            .map(Into::into),
        // Pi-hole's api on this box, the members of the house it serves,
        // and the box's address on the house network
        adblock: std::env::var("VERIFY_ADBLOCK")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|api| verify::adblock::Adblock {
                api,
                household: env_or("VERIFY_ADBLOCK_HOUSEHOLD", "")
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
                lan: env_or("VERIFY_ADBLOCK_LAN", ""),
                password: adblock_password.clone(),
                remember: std::env::var("VERIFY_ADBLOCK_STATE")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(Into::into),
            }),
        app_manifest: std::env::var("VERIFY_APP_MANIFEST")
            .ok()
            .filter(|u| !u.is_empty()),
        member_list: std::env::var("VERIFY_MEMBER_LIST")
            .ok()
            .filter(|u| !u.is_empty()),
        demo_library: match (
            std::env::var("VERIFY_DEMO_LIBRARY_ID"),
            std::env::var("VERIFY_DEMO_LIBRARY_KEY"),
        ) {
            (Ok(id), Ok(key)) if !id.is_empty() && !key.is_empty() => Some((id, key)),
            _ => None,
        },
        // required on a full box: an unset list would be an open door
        members: if full {
            Some(verify::Members::parse(&env("VERIFY_MEMBERS")?).context("VERIFY_MEMBERS")?)
        } else {
            None
        },
        // every box holds it: a directory-only box keeps invites too
        web_dir: std::env::var("VERIFY_WEB_DIR").ok().map(Into::into),
        photos: match std::env::var("VERIFY_PHOTOS_API") {
            Ok(api) => Some(verify::Photos {
                api,
                email_suffix: env("VERIFY_PHOTOS_SUFFIX")?,
                code: std::fs::read_to_string(env("VERIFY_PHOTOS_CODE_FILE")?)?
                    .trim()
                    .to_string(),
                demo_password: match std::env::var("VERIFY_PHOTOS_DEMO_FILE") {
                    Ok(f) => Some(std::fs::read_to_string(f)?.trim().to_string()),
                    Err(_) => None,
                },
            }),
            Err(_) => None,
        },
        library: verify::library::from_env()?,
        network: verify::network::from_env()?,
        tmdb: match std::env::var("VERIFY_TMDB_KEY_FILE") {
            Ok(f) => Some(std::fs::read_to_string(f)?.trim().to_string()),
            Err(_) => None,
        }
        .filter(|k| !k.is_empty()),
        oidc: match std::env::var("VERIFY_OIDC_ISSUER") {
            Ok(issuer) if full => Some(verify::OidcConfig {
                issuer,
                client_id: env("VERIFY_OIDC_CLIENT_ID")?,
                client_secret: std::fs::read_to_string(env("VERIFY_OIDC_CLIENT_SECRET_FILE")?)?
                    .trim()
                    .to_string(),
                redirect: env("VERIFY_OIDC_REDIRECT")?,
            }),
            _ => None,
        },
        search: std::env::var("VERIFY_SEARCH")
            .ok()
            .filter(|s| !s.is_empty()),
        // sign-in at the id Worker (sso.rs): https://id.<domain>
        box_only: role == "box",
        owner: std::env::var("VERIFY_OWNER").ok().filter(|s| !s.is_empty()),
        admin: std::env::var("VERIFY_ADMIN").ok().filter(|s| !s.is_empty()),
        boxes: serde_json::from_str(&env_or("VERIFY_BOXES", "{}")).context("VERIFY_BOXES")?,
        id_origin: std::env::var("VERIFY_ID")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| s.starts_with("https://")),
        release_pub: std::env::var("VERIFY_RELEASE_PUB")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    };
    if let Some(ab) = cfg.adblock.clone() {
        tokio::spawn(verify::adblock::restore(ab));
    }
    let (_, task) = verify::start(cfg).await?;
    task.await?
}
