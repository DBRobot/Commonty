//! dd-hub-writer from the app: the one part of setting up a home controller
//! that runs with admin rights (client/hubwriter). Listing needs none;
//! writing goes through the platform's own prompt - pkexec on Linux,
//! osascript on macOS, PowerShell's RunAs on Windows - and its progress
//! lines come back here to draw (docs/dev/home-controller.md, Writing the card).

use std::path::{Path, PathBuf};
use std::process::Stdio;
#[cfg(not(target_os = "linux"))]
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager as _, Runtime};
#[cfg(target_os = "linux")]
use tokio::io::AsyncBufReadExt as _;
use tokio::io::AsyncReadExt as _;
#[cfg(not(target_os = "linux"))]
use tokio::io::AsyncSeekExt as _;

#[cfg(windows)]
const NAME: &str = "dd-hub-writer.exe";
#[cfg(not(windows))]
const NAME: &str = "dd-hub-writer";

/// what `dd-hub-writer list` says of a disk
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Disk {
    pub id: String,
    pub name: String,
    pub model: String,
    pub bus: String,
    pub size_bytes: u64,
    pub removable: bool,
    #[serde(default)]
    pub mounted: Vec<String>,
}

/// one of `write --progress`'s lines
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Line {
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub bytes_per_sec: u64,
}

pub fn parse_line(s: &str) -> Option<Line> {
    let l: Line = serde_json::from_str(s.trim()).ok()?;
    matches!(l.phase.as_str(), "hash" | "write" | "verify").then_some(l)
}

/// Seconds left in this line's phase, and in the phases after it: writing
/// and reading back each go over the whole image, so once the write's size
/// is known the read back is about as long again (a card reads faster than
/// it writes, so this errs long). Before the write, only the check is known.
pub fn eta_secs(l: &Line) -> Option<u64> {
    if l.bytes_per_sec == 0 || l.total == 0 {
        return None;
    }
    let left = l.total.saturating_sub(l.done) as f64 / l.bytes_per_sec as f64;
    let after = match l.phase.as_str() {
        "write" => l.total as f64 / l.bytes_per_sec as f64,
        _ => 0.0,
    };
    Some((left + after).ceil() as u64)
}

/// the helper: one the app was started with (a development run), the one
/// it carries, or one beside it
fn helper<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("COMMONTY_HUB_WRITER") {
        return Ok(PathBuf::from(p));
    }
    let mut tries = Vec::new();
    if let Ok(d) = app.path().resource_dir() {
        tries.push(d.join(NAME));
    }
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        tries.push(d.join(NAME));
    }
    tries
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| "this copy of the app came without its card writer".to_string())
}

pub async fn list<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<Disk>, String> {
    let out = command(&helper(app)?)
        .arg("list")
        .output()
        .await
        .map_err(|e| format!("the card writer did not start: {e}"))?;
    if !out.status.success() {
        return Err(why(&out.stderr));
    }
    let disks: Vec<Disk> =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("the card writer: {e}"))?;
    Ok(disks.into_iter().filter(|d| d.removable).collect())
}

fn command(p: &Path) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut c = tokio::process::Command::new(p);
    // no console window flashing up
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    c
}

fn why(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    let s = s.trim().trim_start_matches("dd-hub-writer: ");
    if s.is_empty() {
        "the card writer stopped without saying why".into()
    } else {
        s.to_string()
    }
}

/// what an exit means to the person
fn failed(code: Option<i32>, stderr: &[u8]) -> String {
    let reason = why(stderr);
    match code {
        Some(2) => format!("Nothing was written: {reason}"),
        Some(3) => format!(
            "Writing the card did not finish, so it may hold part of the image: {reason}. Write it again."
        ),
        // pkexec: the prompt was dismissed (126) or not allowed (127)
        Some(126 | 127) => "The card was not written: permission was not given.".into(),
        _ => format!("The card was not written: {reason}"),
    }
}

/// Writes `image` to `device` elevated, checking it against `sha256` first,
/// and hands each progress line to `seen`.
pub async fn write<R: Runtime>(
    app: &AppHandle<R>,
    image: &Path,
    device: &str,
    sha256: &str,
    seen: impl FnMut(Line) + Send,
) -> Result<(), String> {
    let helper = helper(app)?;
    let args = [
        "write".to_string(),
        image.to_string_lossy().into_owned(),
        device.to_string(),
        sha256.to_string(),
        "--progress".to_string(),
    ];
    elevated(&helper, &args, seen).await
}

#[cfg(target_os = "linux")]
async fn elevated(
    helper: &Path,
    args: &[String],
    mut seen: impl FnMut(Line) + Send,
) -> Result<(), String> {
    // Inside an AppImage the helper is on the user's own FUSE mount, which
    // root cannot read: a copy in a folder only this user can open
    let dir = private_dir()?;
    let helper = if std::env::var_os("APPIMAGE").is_some() {
        let to = dir.join(NAME);
        std::fs::copy(helper, &to).map_err(|e| e.to_string())?;
        to
    } else {
        helper.to_path_buf()
    };
    let mut child = tokio::process::Command::new("pkexec")
        .arg(&helper)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not ask for permission (pkexec): {e}"))?;
    let mut err = child.stderr.take().ok_or("no stderr")?;
    let errs = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b).await;
        b
    });
    let mut lines = tokio::io::BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    while let Ok(Some(l)) = lines.next_line().await {
        if let Some(l) = parse_line(&l) {
            seen(l);
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let stderr = errs.await.unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    if status.success() {
        Ok(())
    } else {
        Err(failed(status.code(), &stderr))
    }
}

/// macOS and Windows hand an elevated program no pipes, so it writes its
/// lines to a file in a folder of this user's, read here as they come
#[cfg(not(target_os = "linux"))]
async fn elevated(
    helper: &Path,
    args: &[String],
    mut seen: impl FnMut(Line) + Send,
) -> Result<(), String> {
    let dir = private_dir()?;
    let (out, err) = (dir.join("progress"), dir.join("stderr"));
    std::fs::write(&out, b"").map_err(|e| e.to_string())?;
    let mut child = elevate(helper, args, &dir)?;
    let mut f = tokio::fs::File::open(&out)
        .await
        .map_err(|e| e.to_string())?;
    let mut at = 0u64;
    let mut buf = String::new();
    let code = loop {
        let exited = child.try_wait().map_err(|e| e.to_string())?;
        f.seek(std::io::SeekFrom::Start(at))
            .await
            .map_err(|e| e.to_string())?;
        let mut more = String::new();
        at += f
            .read_to_string(&mut more)
            .await
            .map_err(|e| e.to_string())? as u64;
        buf.push_str(&more);
        while let Some(n) = buf.find('\n') {
            if let Some(l) = parse_line(&buf[..n]) {
                seen(l);
            }
            buf.drain(..=n);
        }
        if let Some(s) = exited {
            break exit_code(s, &dir);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let stderr = std::fs::read(&err).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    if code == Some(0) {
        Ok(())
    } else {
        Err(failed(code, &stderr))
    }
}

#[cfg(target_os = "macos")]
fn elevate(helper: &Path, args: &[String], dir: &Path) -> Result<tokio::process::Child, String> {
    let q = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    let p = |f: &str| q(&dir.join(f).to_string_lossy());
    let mut cmd = q(&helper.to_string_lossy());
    for a in args {
        cmd.push(' ');
        cmd.push_str(&q(a));
    }
    // the exit status to a file: do shell script fails the whole script on
    // anything but 0, and says only that it did
    let cmd = format!(
        "{cmd} >{} 2>{}; echo $? >{}",
        p("progress"),
        p("stderr"),
        p("code")
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        cmd.replace('\\', "\\\\").replace('"', "\\\"")
    );
    command(Path::new("/usr/bin/osascript"))
        .args(["-e", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not ask for permission: {e}"))
}

#[cfg(windows)]
fn elevate(helper: &Path, args: &[String], dir: &Path) -> Result<tokio::process::Child, String> {
    let p = |f: &str| dir.join(f).to_string_lossy().into_owned();
    let mut inner = format!("\"{}\"", helper.display());
    for a in args {
        inner.push_str(&format!(" \"{a}\""));
    }
    // cmd's own /c strips the outer quotes; RunAs gives no pipes, so cmd
    // sends the lines to files, and its exit is the writer's
    let line = format!(
        "/c \"{inner} > \"{}\" 2> \"{}\"\"",
        p("progress"),
        p("stderr")
    );
    let ps = format!(
        "$p = Start-Process -FilePath 'cmd.exe' -ArgumentList '{}' -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode",
        line.replace('\'', "''")
    );
    command(Path::new("powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not ask for permission: {e}"))
}

#[cfg(target_os = "macos")]
fn exit_code(s: std::process::ExitStatus, dir: &Path) -> Option<i32> {
    // osascript itself fails when the prompt is cancelled
    if !s.success() {
        return Some(126);
    }
    std::fs::read_to_string(dir.join("code"))
        .ok()
        .and_then(|c| c.trim().parse().ok())
}

#[cfg(windows)]
fn exit_code(s: std::process::ExitStatus, _dir: &Path) -> Option<i32> {
    // Start-Process throws when the prompt is declined: powershell exits 1
    // with nothing from the writer
    s.code()
}

/// a fresh folder only this user can open
fn private_dir() -> Result<PathBuf, String> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    let name: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let d = std::env::temp_dir().join(format!("commonty-writer-{name}"));
    std::fs::create_dir(&d).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_writers_progress_lines() {
        let l = parse_line(
            r#"{"phase":"write","done":1048576,"total":4194304,"bytes_per_sec":524288}"#,
        )
        .unwrap();
        assert_eq!(l.phase, "write");
        assert_eq!(
            (l.done, l.total, l.bytes_per_sec),
            (1 << 20, 4 << 20, 1 << 19)
        );
        assert!(
            parse_line("  {\"phase\":\"hash\",\"done\":0,\"total\":1,\"bytes_per_sec\":0}\r")
                .is_some()
        );
        assert!(parse_line(r#"{"phase":"erase","done":0,"total":1,"bytes_per_sec":0}"#).is_none());
        assert!(parse_line("dd-hub-writer: no disk sdz").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn time_left() {
        let at = |phase: &str, done, total, bps| Line {
            phase: phase.into(),
            done,
            total,
            bytes_per_sec: bps,
        };
        // the check: what is left of it alone
        assert_eq!(eta_secs(&at("hash", 100, 1000, 100)), Some(9));
        // writing: what is left, and the read back after it
        assert_eq!(eta_secs(&at("write", 400, 1000, 100)), Some(16));
        assert_eq!(eta_secs(&at("verify", 500, 1000, 100)), Some(5));
        assert_eq!(eta_secs(&at("verify", 1000, 1000, 100)), Some(0));
        // no speed yet, or nothing to do: no guess
        assert_eq!(eta_secs(&at("write", 0, 1000, 0)), None);
        assert_eq!(eta_secs(&at("hash", 0, 0, 10)), None);
        // done past total never goes below nothing
        assert_eq!(eta_secs(&at("hash", 2000, 1000, 100)), Some(0));
    }

    #[test]
    fn what_an_exit_says() {
        assert!(
            failed(Some(2), b"dd-hub-writer: sdb is not removable")
                .starts_with("Nothing was written: sdb is not removable")
        );
        assert!(failed(Some(3), b"").contains("may hold part"));
        assert!(failed(Some(126), b"").contains("permission"));
        assert!(failed(Some(1), b"dd-hub-writer: not a sha256").ends_with("not a sha256"));
    }

    #[test]
    fn the_list_as_the_writer_gives_it() {
        let d: Vec<Disk> = serde_json::from_str(
            r#"[{"id":"sdb","name":"/dev/sdb","model":"SD Card Reader","bus":"usb","size_bytes":31914983424,"removable":true,"mounted":["/media/a/boot"]}]"#,
        )
        .unwrap();
        assert_eq!(d[0].id, "sdb");
        assert_eq!(d[0].mounted, vec!["/media/a/boot".to_string()]);
    }
}
