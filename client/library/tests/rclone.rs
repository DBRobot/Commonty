//! The format against rclone itself: what we write, rclone reads, and the
//! other way, names and bytes. Needs `rclone` on PATH (nixpkgs); skipped
//! where it is not.

use std::path::Path;
use std::process::Command;

use library::crypt::{BLOCK, Cipher, HEADER, SEALED_BLOCK};

fn rclone() -> Option<String> {
    std::env::var("RCLONE").ok().or_else(|| {
        Command::new("rclone")
            .arg("version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| "rclone".to_string())
    })
}

fn obscure(rclone: &str, s: &str) -> String {
    let o = Command::new(rclone).args(["obscure", s]).output().unwrap();
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

/// an rclone config with a crypt remote over a local folder
fn config(rclone: &str, dir: &Path, password: &str, salt: &str) -> std::path::PathBuf {
    let cfg = dir.join("rclone.conf");
    std::fs::write(
        &cfg,
        format!(
            "[enc]\ntype = crypt\nremote = {}\npassword = {}\npassword2 = {}\n",
            dir.join("store").display(),
            obscure(rclone, password),
            obscure(rclone, salt)
        ),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("store")).unwrap();
    cfg
}

fn run(rclone: &str, cfg: &Path, args: &[&str]) -> Vec<u8> {
    let o = Command::new(rclone)
        .arg("--config")
        .arg(cfg)
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "rclone {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    o.stdout
}

fn encrypt_file(c: &Cipher, plain: &[u8]) -> Vec<u8> {
    let e = c.encrypter();
    let mut out = e.header();
    for (n, chunk) in plain.chunks(BLOCK).enumerate() {
        out.extend(e.block(n as u64, chunk));
    }
    out
}

fn decrypt_file(c: &Cipher, sealed: &[u8]) -> Vec<u8> {
    let d = c.decrypter(&sealed[..HEADER]).unwrap();
    let mut out = Vec::new();
    for (n, chunk) in sealed[HEADER..].chunks(SEALED_BLOCK).enumerate() {
        out.extend(d.block(n as u64, chunk).unwrap());
    }
    out
}

#[test]
fn rclone_reads_what_we_write_and_back() {
    let Some(rclone) = rclone() else {
        eprintln!("no rclone here; skipped");
        return;
    };
    let dir = std::env::temp_dir().join(format!("library-rclone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (password, salt) = (
        "a library key, base64 in real life",
        "a996a28ca51c9cf1d3f8e2038c8339c8",
    );
    let cfg = config(&rclone, &dir, password, salt);
    let c = Cipher::new(password, salt);

    // ours -> rclone: a file under a folder, more than one block, odd size
    let plain: Vec<u8> = (0..(BLOCK * 3 + 12345))
        .map(|i| (i * 7 % 251) as u8)
        .collect();
    let enc_path = c.encrypt_path("Movies/Big Buck Bunny (2008)/film.mkv");
    let dst = dir.join("store").join(&enc_path);
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    std::fs::write(&dst, encrypt_file(&c, &plain)).unwrap();
    let listed = String::from_utf8(run(&rclone, &cfg, &["lsf", "-R", "enc:"])).unwrap();
    assert!(
        listed.contains("Movies/Big Buck Bunny (2008)/film.mkv"),
        "rclone did not see our name: {listed}"
    );
    let got = run(
        &rclone,
        &cfg,
        &["cat", "enc:Movies/Big Buck Bunny (2008)/film.mkv"],
    );
    assert_eq!(got, plain, "rclone read different bytes");

    // rclone -> ours: rclone writes, we list and read
    let src = dir.join("theirs.bin");
    let theirs: Vec<u8> = (0..(BLOCK + 1)).map(|i| (i % 253) as u8).collect();
    std::fs::write(&src, &theirs).unwrap();
    run(
        &rclone,
        &cfg,
        &[
            "copyto",
            src.to_str().unwrap(),
            "enc:Files/notes & things/theirs.bin",
        ],
    );
    let mut found = None;
    for top in std::fs::read_dir(dir.join("store")).unwrap() {
        let top = top.unwrap();
        let name = c
            .decrypt_segment(top.file_name().to_str().unwrap())
            .unwrap();
        if name == "Files" {
            for sub in std::fs::read_dir(top.path()).unwrap() {
                let sub = sub.unwrap();
                assert_eq!(
                    c.decrypt_segment(sub.file_name().to_str().unwrap())
                        .unwrap(),
                    "notes & things"
                );
                for f in std::fs::read_dir(sub.path()).unwrap() {
                    let f = f.unwrap();
                    assert_eq!(
                        c.decrypt_segment(f.file_name().to_str().unwrap()).unwrap(),
                        "theirs.bin"
                    );
                    found = Some(std::fs::read(f.path()).unwrap());
                }
            }
        }
    }
    let sealed = found.expect("rclone's file");
    assert_eq!(decrypt_file(&c, &sealed), theirs);
    // and its blocks open with their own keys, as a box gets them
    let keys = c.block_keys(&sealed[..HEADER], 2).unwrap();
    let o = library::crypt::BlockOpener::new(&sealed[..HEADER], keys).unwrap();
    let mut back = Vec::new();
    for (n, chunk) in sealed[HEADER..].chunks(SEALED_BLOCK).enumerate() {
        back.extend(o.block(n as u64, chunk).unwrap());
    }
    assert_eq!(back, theirs);
    let _ = std::fs::remove_dir_all(&dir);
}
