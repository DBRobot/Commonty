use anyhow::{Context, Result};

fn env(k: &str) -> Result<String> {
    std::env::var(k).with_context(|| format!("{k} is not set"))
}
fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let full = env_or("VERIFY_ROLE", "full") != "directory";
    let cfg = verify::Config {
        bind: env_or("VERIFY_BIND", "127.0.0.1:4181").parse()?,
        dir: env("VERIFY_DIR")?.into(),
        peers: env_or("VERIFY_PEERS", "")
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
        sync_secs: env_or("VERIFY_SYNC_SECS", "300").parse()?,
        domain: if full {
            Some(env("VERIFY_DOMAIN")?)
        } else {
            None
        },
        home: serde_json::from_str(&env_or("VERIFY_HOME", "[]")).context("VERIFY_HOME")?,
        // every box and where its prometheus answers, for the pages that
        // show the fleet; a box that is not told has none to show
        fleet: serde_json::from_str(&env_or("VERIFY_FLEET", "{}")).context("VERIFY_FLEET")?,
        app_manifest: std::env::var("VERIFY_APP_MANIFEST")
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
        release_pub: std::env::var("VERIFY_RELEASE_PUB")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    };
    let (_, task) = verify::start(cfg).await?;
    task.await?
}
