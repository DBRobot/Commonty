//! The libraries as folders on a desktop: rclone does it. Each library is
//! an rclone crypt remote over the gate's WebDAV, with a device token as
//! bearer and the library key as password; `rclone mount` puts it under
//! a folder of its own. Nothing of ours is in the path of a byte. The
//! token comes from a command rclone runs at start and again whenever the
//! gate answers 401, so a mount outlives every token it is given.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

pub struct Mounted {
    pub id: String,
    pub owner: String,
    pub at: PathBuf,
    child: Child,
}

impl Drop for Mounted {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        release(&self.at);
    }
}

/// Let go of a mount point. Killing rclone is enough on Windows, where
/// WinFsp tears the drive down with the process that served it; on Linux
/// a dead fuse mount leaves a folder nothing can open until it is
/// unmounted by name.
fn release(at: &Path) {
    if cfg!(windows) {
        return;
    }
    let _ = Command::new("fusermount3")
        .arg("-uz")
        .arg(at)
        .stderr(Stdio::null())
        .status();
}

/// the rclone binary: named by the packaging, or on PATH
pub fn rclone() -> PathBuf {
    std::env::var("DD_RCLONE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("rclone"))
}

/// what rclone stores in place of a password: not a secret, a shape.
///
/// Given on stdin, not as an argument: an argument is readable by every
/// process on the machine through /proc for as long as rclone runs, and
/// one beginning with a dash is taken for a flag.
pub fn obscure(rclone: &Path, plain: &str) -> Result<String> {
    use std::io::Write as _;
    let mut child = Command::new(rclone)
        .args(["obscure", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {}", rclone.display()))?;
    child
        .stdin
        .take()
        .context("rclone's stdin")?
        .write_all(plain.as_bytes())?;
    let o = child.wait_with_output()?;
    if !o.status.success() {
        bail!("rclone obscure: {}", String::from_utf8_lossy(&o.stderr));
    }
    Ok(String::from_utf8(o.stdout)?.trim().to_string())
}

/// A command line as rclone's bearer_token_command reads it: words split
/// on spaces, a word with a space or a quote in double quotes, a quote in
/// it doubled (a Windows install is under "Program Files").
pub fn token_command(exe: &Path, args: &[&str]) -> String {
    let exe = exe.to_string_lossy();
    std::iter::once(exe.as_ref())
        .chain(args.iter().copied())
        .map(|w| {
            if w.contains([' ', '"']) {
                format!("\"{}\"", w.replace('"', "\"\""))
            } else {
                w.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// mount one library at `at`, as rclone crypt over the gate's WebDAV, the
/// bearer token from running `token_command`
pub fn mount(
    id: &str,
    owner: &str,
    dav_url: &str,
    token_command: &str,
    key: &library::Key,
    at: &Path,
) -> Result<Mounted> {
    let rc = rclone();
    // a mount left by a process that died is a folder nothing can open
    if std::fs::read_dir(at).is_err() {
        release(at);
    }
    std::fs::create_dir_all(at)?;
    let password = obscure(&rc, &library::crypt::password_of(key))?;
    let salt = obscure(&rc, id)?;
    let child = Command::new(&rc)
        .args([
            "mount",
            "lib:",
            at.to_str().context("mount path")?,
            "--vfs-cache-mode",
            "writes",
            "--dir-cache-time",
            "10s",
            "--poll-interval",
            "0",
            "--quiet",
        ])
        // the two remotes, in the environment: nothing written to disk
        .env("RCLONE_CONFIG_DAV_TYPE", "webdav")
        .env("RCLONE_CONFIG_DAV_URL", dav_url)
        .env("RCLONE_CONFIG_DAV_VENDOR", "other")
        .env("RCLONE_CONFIG_DAV_BEARER_TOKEN_COMMAND", token_command)
        .env("RCLONE_CONFIG_LIB_TYPE", "crypt")
        .env("RCLONE_CONFIG_LIB_REMOTE", "dav:")
        .env("RCLONE_CONFIG_LIB_PASSWORD", password)
        .env("RCLONE_CONFIG_LIB_PASSWORD2", salt)
        .env("RCLONE_CONFIG", "/dev/null")
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", rc.display()))?;
    Ok(Mounted {
        id: id.to_string(),
        owner: owner.to_string(),
        at: at.to_path_buf(),
        child,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_command_rclone_splits_back_into_its_words() {
        assert_eq!(
            token_command(Path::new("/nix/store/x-dd/bin/dd"), &["media", "--token"]),
            "/nix/store/x-dd/bin/dd media --token"
        );
        assert_eq!(
            token_command(Path::new(r"C:\Program Files\dd.exe"), &["media", "--token"]),
            r#""C:\Program Files\dd.exe" media --token"#
        );
        assert_eq!(token_command(Path::new(r#"a"b"#), &[]), r#""a""b""#);
    }
}
