//! The GitHub copy (fleet/policy.json forgeMirrors), pushed from this
//! machine as the release is published, over ssh with the owner's own key.
//! No box holds a GitHub credential: the forge used to push it with a token
//! kept in its secrets, which a box that turned hostile could have used to
//! write to the copy (docs/dev/mirror.md). A push that fails - the account
//! suspended, no key here - is said and never stops a release.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// where the copy is reached over ssh: a GitHub https address turned into
/// its ssh one, anything else as it is written
pub fn ssh_url(to: &str) -> String {
    match to.strip_prefix("https://github.com/") {
        Some(path) => format!("git@github.com:{path}"),
        None => to.to_string(),
    }
}

/// every mirror's address, from the policy
pub fn targets(root: &Path) -> Result<Vec<String>> {
    let policy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fleet/policy.json"))?)
            .context("fleet/policy.json")?;
    Ok(policy["forgeMirrors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["to"].as_str().map(ssh_url))
                .collect()
        })
        .unwrap_or_default())
}

/// the released commit, as `ref` on each copy. Never forced: a copy that
/// went its own way is said, not overwritten
pub fn push(root: &Path, rev: &str, r#ref: &str) -> Vec<String> {
    let mut failed = vec![];
    let to = match targets(root) {
        Ok(t) => t,
        Err(e) => return vec![format!("{e:#}")],
    };
    for url in to {
        let out = Command::new("timeout")
            .args(["120", "git", "-C"])
            .arg(root)
            .args(["push", "-q", &url, &format!("{rev}:refs/heads/{ref}")])
            // no prompt for a key or a host: this runs inside a release
            .env(
                "GIT_SSH_COMMAND",
                "ssh -o BatchMode=yes -o ConnectTimeout=15",
            )
            .output();
        match out {
            Ok(o) if o.status.success() => println!("== mirrored to {url}"),
            Ok(o) => failed.push(format!(
                "{url}: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => failed.push(format!("{url}: {e}")),
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_address_is_pushed_over_ssh() {
        assert_eq!(
            ssh_url("https://github.com/DBRobot/Home-Server.git"),
            "git@github.com:DBRobot/Home-Server.git"
        );
        assert_eq!(
            ssh_url("git@example.org:a/b.git"),
            "git@example.org:a/b.git"
        );
    }

    #[test]
    fn the_policy_names_the_copies() {
        let d = std::env::temp_dir().join(format!("dd-mirror-{}", std::process::id()));
        std::fs::create_dir_all(d.join("fleet")).unwrap();
        std::fs::write(
            d.join("fleet/policy.json"),
            r#"{"forgeMirrors":[{"to":"https://github.com/a/b.git"},{"user":"x"}]}"#,
        )
        .unwrap();
        assert_eq!(targets(&d).unwrap(), vec!["git@github.com:a/b.git"]);
        std::fs::write(d.join("fleet/policy.json"), "{}").unwrap();
        assert!(targets(&d).unwrap().is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
