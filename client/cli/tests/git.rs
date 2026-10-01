//! Commits signed by the device key, verified against the directory.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

fn scratch(what: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "dd-git-{}-{}-{what}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn run(home: &Path, keyring: &Path, dir: &Path, program: &str, args: &[&str]) -> (bool, String) {
    let o = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("DD_KEYRING_FILE", keyring)
        .env("GIT_AUTHOR_NAME", "Sarah")
        .env("GIT_AUTHOR_EMAIL", "sarah@example.com")
        .env("GIT_COMMITTER_NAME", "Sarah")
        .env("GIT_COMMITTER_EMAIL", "sarah@example.com")
        .output()
        .unwrap();
    (
        o.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_are_signed_by_the_device_and_verified_through_the_directory() {
    let (addr, _task) = verify::start(verify::Config {
        home: vec![],
        fleet: Default::default(),
        thanos: None,
        adblock: None,
        forge_events: None,
        storage_ledger: None,
        mail_forward: None,
        demo_library: None,
        tmdb: None,
        search: None,
        oidc: None,
        app_manifest: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        dir: scratch("box").join("keys"),
        peers: vec![],
        sync_secs: 300,
        members: None,
        release_pub: None,
        web_dir: None,
        photos: None,
        library: None,
        network: None,
        domain: None,
    })
    .await
    .unwrap();
    let directory = format!("http://{addr}/_dd/directory");
    let home = scratch("home");
    let keyring = home.join("keys.json");
    let dd = env!("CARGO_BIN_EXE_dd");
    let ok = |dir: &Path, program: &str, args: &[&str]| {
        let (ok, out) = run(&home, &keyring, dir, program, args);
        assert!(ok, "{program} {}:\n{out}", args.join(" "));
        out
    };

    ok(
        &home,
        dd,
        &[
            "identity",
            "new",
            "--name",
            "sarah",
            "--directory",
            &directory,
        ],
    );
    ok(&home, dd, &["git", "setup"]);
    let out = ok(&home, dd, &["git", "signers", "--directory", &directory]);
    assert!(out.contains("1 signer(s) from 1 entries"), "{out}");
    let signers = std::fs::read_to_string(home.join(".config/dd/allowed_signers")).unwrap();
    assert!(signers.starts_with("sarah@dd ssh-ed25519 "), "{signers}");

    // a commit made with the configured key verifies and names the person
    let repo = scratch("repo");
    ok(&repo, "git", &["init", "-q", "-b", "main"]);
    ok(
        &repo,
        "git",
        &["commit", "-q", "--allow-empty", "-m", "signed"],
    );
    let out = ok(&repo, "git", &["log", "--format=%G? %GS", "-1"]);
    assert_eq!(out.trim(), "G sarah@dd", "{out}");

    // a commit made with a key no entry lists is not trusted
    let rogue = scratch("rogue").join("key");
    ok(
        &home,
        "ssh-keygen",
        &[
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            rogue.to_str().unwrap(),
        ],
    );
    ok(
        &repo,
        "git",
        &[
            "-c",
            &format!("user.signingkey={}", rogue.display()),
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "rogue",
        ],
    );
    let out = ok(&repo, "git", &["log", "--format=%G?", "-1"]);
    assert_eq!(out.trim(), "U", "{out}");
}
