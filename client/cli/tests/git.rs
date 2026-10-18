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

/// the release key `dd` holds other members' entries to here
fn release_key() -> &'static identity::SigningKey {
    static K: std::sync::OnceLock<identity::SigningKey> = std::sync::OnceLock::new();
    K.get_or_init(identity::generate)
}

/// the releases branch's member list, as whatever `body` holds when asked
fn serve(body: std::sync::Arc<std::sync::Mutex<String>>) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let body = body.lock().unwrap().clone();
            let r = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = std::io::Write::write_all(&mut s, r.as_bytes());
        }
    });
    format!("http://{addr}/members.json")
}

fn run(home: &Path, keyring: &Path, dir: &Path, program: &str, args: &[&str]) -> (bool, String) {
    let o = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env(
            "DD_RELEASE_PUB",
            identity::encode_public(&release_key().verifying_key()),
        )
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
    let listed = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let box_dir = scratch("box").join("keys");
    let (addr, _task) = verify::start(verify::Config {
        mail: None,
        home: vec![],
        fleet: Default::default(),
        thanos: None,
        adblock: None,
        forge_events: None,
        storage_ledger: None,
        demo_library: None,
        tmdb: None,
        search: None,
        id_origin: None,
        box_only: false,
        owner: None,
        admin: None,
        boxes: Default::default(),
        oidc: None,
        app_manifest: None,
        member_list: Some(serve(listed.clone())),
        bind: "127.0.0.1:0".parse().unwrap(),
        dir: box_dir.clone(),
        directory_readers: vec![],
        names: vec!["127.0.0.1".into()],
        control: None,
        identity_from: Default::default(),
        house_peers: vec![],
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
            "--born",
            "1990-01",
            "--agree-terms",
            "--name",
            "sarah",
            "--directory",
            &directory,
        ],
    );
    ok(&home, dd, &["git", "setup"]);
    // the release lists sarah; a box plants mallory, a key of its own
    let id = ok(&home, dd, &["identity", "show", "--directory", &directory])
        .lines()
        .find_map(|l| l.strip_prefix("member id:"))
        .unwrap()
        .trim()
        .to_string();
    // and carol, whose entry no directory will serve
    let carol = identity::member_id("carol's root");
    let list = identity::MemberList {
        counter: identity::fleet::MEMBER_LIST_FLOOR + 1,
        members: vec![id.clone(), carol.clone()],
        names: [
            (id.clone(), identity::name_tag(&id, "sarah")),
            (carol.clone(), identity::name_tag(&carol, "carol")),
        ]
        .into(),
        ..Default::default()
    };
    *listed.lock().unwrap() =
        serde_json::to_string(&identity::sign_member_list(list, release_key()).unwrap()).unwrap();
    let boxed = identity::generate();
    let planted_key = identity::generate();
    let planted_pub = identity::encode_public(&planted_key.verifying_key());
    let planted = identity::sign(
        identity::Entry {
            name: "mallory".into(),
            root: identity::encode_public(&boxed.verifying_key()),
            recovery: String::new(),
            recovered_from: vec![],
            devices: vec![identity::Device {
                fingerprint: identity::fingerprint(&planted_pub),
                public_key: planted_pub,
                added: 1,
            }],
            passkeys: vec![],
            grant: None,
            libraries: vec![],
            terms: None,
            removed_boxes: vec![],
            version: 1,
            updated: 1,
        },
        &boxed,
    )
    .unwrap();
    std::fs::write(
        box_dir.join("mallory.json"),
        serde_json::to_vec(&planted).unwrap(),
    )
    .unwrap();
    let out = ok(&home, dd, &["git", "signers", "--directory", &directory]);
    assert!(out.contains("1 signer(s) from 1 entries"), "{out}");
    let signers = std::fs::read_to_string(home.join(".config/dd/allowed_signers")).unwrap();
    assert!(signers.starts_with("sarah@dd ssh-ed25519 "), "{signers}");
    assert!(
        !signers.contains("mallory"),
        "a box's entry is no signer: {signers}"
    );
    let file = home.join(".config/dd/allowed_signers");
    let sarah_key = signers.trim_end().split_once(' ').unwrap().1.to_string();
    let signers_now = || {
        run(
            &home,
            &keyring,
            &home,
            dd,
            &["git", "signers", "--directory", &directory],
        )
    };
    // someone the file trusted whom the list no longer names (removed):
    // they go
    std::fs::write(&file, format!("{signers}bob@dd {sarah_key}\n")).unwrap();
    let (fine, out) = signers_now();
    assert!(
        fine && out.contains("bob@dd: no longer on the member list"),
        "{out}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), signers);
    // someone the list still names, whose entry will not anchor: the file
    // stays as it was rather than quietly losing them
    let had = format!("{signers}carol@dd {sarah_key}\n");
    std::fs::write(&file, &had).unwrap();
    let (fine, out) = signers_now();
    assert!(!fine && out.contains("carol@dd still listed"), "{out}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), had);

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
