mod derive;
mod librarycmd;
mod mail_worker;
mod mediacmd;
mod member;
mod release_cmd;
mod storage_ledger;
mod ui;
mod vault;
mod who;

use anyhow::{Context, Result};
use auth::KeyStore;
use clap::{Parser, Subcommand};
use ente::LoginParams;
use vault::Vault;
use zeroize::Zeroizing;

const SERVICE: &str = "commonty";
/// DD_KEYRING names a different credential-store service: a second "device"
/// on one machine. DD_KEYRING_FILE swaps the OS store for a file - tests.
fn service() -> String {
    std::env::var("DD_KEYRING").unwrap_or_else(|_| SERVICE.to_string())
}
/// The archive password. Separate again: it decrypts old computers, not
/// photos, and `dd lock` should be able to forget one without the other.
const ARCHIVE: &str = "archive";
const DEFAULT_ENTE: &str = "https://api.commonty.org";
const DEFAULT_IMAGES: &str = "https://files.commonty.org/images/";
/// Any browser-facing host does; the session cookie covers the whole domain.
const DEFAULT_ENROL: &str = "https://home.commonty.org/_dd/enrol";
/// Where a person's signed entry lives. Any box can hold one; a client that
/// names several sees whether they agree. node2 has no public name yet, so
/// its copy is reachable on the tailnet only.
const DEFAULT_DIRECTORIES: [&str; 1] = directory::DEFAULT;
/// keyring account holding the name the device key signs for
const USER: &str = "user";

#[derive(Parser)]
#[command(name = "dd", about = "the commonty client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The fleet moves when this machine says so: build every box from a
    /// commit, push the closures to the cache, sign and publish the
    /// release the agents fetch.
    Release {
        #[command(subcommand)]
        cmd: release_cmd::ReleaseCmd,
        /// the repository checkout; defaults to the current directory
        #[arg(long, default_value = ".", global = true)]
        repo: String,
    },
    /// The mail Worker: members' addresses, kept at Cloudflare and out of
    /// every box's reach (client/mail). `dd release publish` uploads it.
    Mail {
        #[command(subcommand)]
        cmd: MailCmd,
        /// the repository checkout; defaults to the current directory
        #[arg(long, default_value = ".", global = true)]
        repo: String,
    },
    /// Join, from nothing. Publishes your identity with this device as its
    /// first (that IS the account - nobody approves it), then creates the
    /// end-to-end encrypted photo account, which no server can do for you.
    Signup {
        #[arg(long)]
        username: String,
        /// For the photo account only; nothing else here has an email.
        #[arg(long)]
        email: String,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES)]
        directories: Vec<String>,
        #[arg(long, default_value = DEFAULT_ENTE)]
        ente_origin: String,
        /// Stop after the services account and leave photos for later.
        #[arg(long)]
        skip_photos: bool,
        /// Read the photo password from stdin instead of prompting.
        #[arg(long)]
        password_stdin: bool,
    },
    /// A browser login: prints a link, signed by this device and good for ten
    /// minutes, that sets up a passkey in the browser that opens it. Waits
    /// for that, then signs the passkey into your entry with the root held
    /// here - a box can run the ceremony but cannot add the result.
    Enrol {
        #[arg(long, default_value = DEFAULT_ENROL)]
        url: String,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES)]
        directories: Vec<String>,
    },
    /// Your encrypted libraries: files and media no box can read.
    Library {
        #[command(subcommand)]
        cmd: librarycmd::LibraryCmd,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES, global = true)]
        directories: Vec<String>,
    },
    /// Your libraries as folders on this machine, for any player.
    Media {
        /// where to mount (default ~/Commonty)
        #[arg(long)]
        at: Option<std::path::PathBuf>,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES)]
        directories: Vec<String>,
    },
    /// The browser passkeys in your entry.
    Passkey {
        #[command(subcommand)]
        cmd: PasskeyCmd,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES, global = true)]
        directories: Vec<String>,
    },
    /// Who you are: a root key here, a recovery key on paper, and the entry
    /// you sign listing your devices. No server issues it.
    Identity {
        #[command(subcommand)]
        cmd: IdentityCmd,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES, global = true)]
        directories: Vec<String>,
    },
    /// Who may use the services: fleet/members.json, a list of member ids
    /// (a hash of each person's root, no names). Edited here, committed,
    /// released; every box checks it and none can add to it.
    Member {
        #[command(subcommand)]
        cmd: MemberCmd,
        /// the repository checkout; defaults to the current directory
        #[arg(long, default_value = ".", global = true)]
        repo: String,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES, global = true)]
        directories: Vec<String>,
    },
    /// A code that lets one person in: signed by the release key here,
    /// held by every box until it expires, typed once on the join page.
    Invite {
        /// how long the code is good for: 5m, 30m, 1h
        #[arg(long, default_value = "5m")]
        ttl: String,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES)]
        directories: Vec<String>,
    },
    /// The fleet: every box, its roles and where it stands. Reads
    /// fleet/boxes.json; with --private, the owner and place behind the ids
    /// (secrets/fleet.yaml, readable by the release signer's dd key).
    Box {
        #[command(subcommand)]
        cmd: BoxCmd,
        /// the repository checkout; defaults to the current directory
        #[arg(long, default_value = ".")]
        repo: String,
    },
    /// The repo's sops files, edited with an age key that lives in this
    /// machine's credential store. `dd secret run -- <sops arguments>` runs
    /// sops with that key; `init` makes the key and prints the recipient to
    /// add to .sops.yaml.
    Secret {
        #[command(subcommand)]
        cmd: SecretCmd,
    },
    /// Commits signed by your device key and checked against the directory:
    /// anyone who has your entry can verify what you wrote.
    Git {
        #[command(subcommand)]
        cmd: GitCmd,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES, global = true)]
        directories: Vec<String>,
    },
    /// Encrypted repositories: `git remote add origin dd::<url>` and push.
    /// The forge holds ciphertext; whoever holds the key reads.
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// This device's key.
    Device {
        #[command(subcommand)]
        cmd: DeviceCmd,
        #[arg(long = "directory", default_values = DEFAULT_DIRECTORIES, global = true)]
        directories: Vec<String>,
    },
    /// Unlock ente. Asks for the ente password once, then stores the derived
    /// keys in the OS credential store so it is not asked again.
    Unlock {
        #[arg(long, default_value = DEFAULT_ENTE)]
        origin: String,
        #[arg(long)]
        email: String,
        /// Read the password from stdin instead of prompting. rpassword needs
        /// a controlling terminal, so the prompt cannot work when piped.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Print a token for the services, signed by this device. Good for an
    /// hour, made offline; this is what scripts and rclone should call.
    Token,
    /// Encrypted archives of old computers. restic's format, in your own
    /// directory on the server; the password never leaves this machine.
    Image {
        #[command(subcommand)]
        cmd: ImageCmd,
        #[arg(long, default_value = DEFAULT_IMAGES, global = true)]
        repo: String,
    },
    /// The storage ledger, on a box (a timer runs it): what each member keeps
    /// across every service, counted and written for the gate, and each
    /// member's Photos held to what is left of their allowance.
    StorageLedger {
        /// the gate on this box, for the members and their libraries
        #[arg(long, default_value = "http://127.0.0.1:4181")]
        gate: String,
        #[arg(long, default_value = DEFAULT_ENTE)]
        ente_origin: String,
        /// a member's photo address is their name and this: @users.<domain>
        #[arg(long)]
        photos_suffix: String,
        /// a member's Passwords address is their name and this: @<domain>
        #[arg(long)]
        vault_suffix: String,
        /// the ledger's own photo account's password (secrets:
        /// ente-storage-password); the account is storage+ledger<photos-suffix>
        #[arg(long)]
        password_file: std::path::PathBuf,
        /// each member's allowance, in GB
        #[arg(long)]
        budget_gb: u64,
        /// where the count is written, for the gate
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// The demo's photo account: made with the fleet's code and the
    /// password the boxes hold for it, then given a quota of nothing, which
    /// is ente's read-only. Once per fleet; safe to run again.
    PhotosDemo {
        #[arg(long, default_value = DEFAULT_ENTE)]
        ente_origin: String,
        /// the demo's address: demo@users.<domain>
        #[arg(long)]
        email: String,
        /// the fleet's verification code (secrets: ente-ott)
        #[arg(long)]
        code: String,
        /// the demo's password (secrets: ente-demo-password)
        #[arg(long)]
        password: String,
        /// the address the account had before (a domain move): signed
        /// into with the same password and moved to --email, nothing made
        #[arg(long)]
        from: Option<String>,
        /// Room to give it, in bytes. One byte (the default) fits no photo:
        /// read-only. More, briefly, to put the demo's own photos in, then
        /// run again without it.
        #[arg(long, default_value_t = 1)]
        quota: u64,
    },
    /// What is cached on this machine.
    Status,
    /// Forget the stored ente keys and device key on this machine.
    Lock {
        /// Also remove the identity root. Not undoable except by recovery.
        #[arg(long)]
        forget_identity: bool,
    },
}

#[derive(Subcommand)]
enum IdentityCmd {
    /// Create your identity and publish it, with this device as the first.
    /// Prints the recovery key once. With an invite code, the entry
    /// carries the grant that makes you a member.
    New {
        /// Your name; defaults to the one already on this machine.
        #[arg(long)]
        name: Option<String>,
        /// the invite code someone gave you (`dd invite` on their side)
        #[arg(long)]
        code: Option<String>,
    },
    /// What every directory has for you, and whether it is yours.
    Show,
    /// Bring every directory up to the newest entry any of them holds. No
    /// signing: the entry carries its own, so a copy needs no key here.
    Publish,
    /// Lost every device? The paper key installs a new root and starts the
    /// device list over with this one.
    Recover {
        #[arg(long)]
        name: Option<String>,
        /// Read the recovery key from stdin instead of prompting.
        #[arg(long)]
        key_stdin: bool,
    },
    /// Print the root secret, to move it to another device you own.
    Export,
    /// Read a root secret from stdin and keep it here.
    Import {
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
enum PasskeyCmd {
    /// Give a passkey already in your entry the key a browser derived
    /// from it, so that browser can open your libraries. The browser
    /// prints the key; this signs it in.
    Link { id: String, library_key: String },
    /// Every passkey in your entry, by id.
    List,
    /// Drop one; the browser that holds it stops working everywhere at once.
    Remove {
        /// base64url, so it may start with a hyphen
        #[arg(allow_hyphen_values = true)]
        id: String,
    },
    /// Sign in a credential record from a file - the one a box used to keep
    /// in <name>.passkeys.json before passkeys lived in the entry.
    Add { file: String },
}

#[derive(Subcommand)]
enum MemberCmd {
    /// Let a person in: their name (looked up in the directory) or member id.
    Add { who: String },
    /// Shut a person out.
    Remove { who: String },
    /// Every member id, with the name behind it where a directory has one.
    List,
}

#[derive(Subcommand)]
enum BoxCmd {
    /// Every box in the list.
    List {
        /// Also the owner, site and region behind the opaque ids.
        #[arg(long)]
        private: bool,
    },
    /// What the world sees this box as. The front door is a tunnel the box
    /// opens outward, so this only says whether a direct forward could ever
    /// work here too: a public address on the router, yes; carrier nat
    /// (a shared address, as Starry's blocks are), never.
    Public { name: String },
}

#[derive(Subcommand)]
enum SecretCmd {
    /// Make the key (once) and print its public recipient.
    Init,
    /// Print the public recipient of the key held here.
    Recipient,
    /// Run sops with the key: `dd secret run -- set secrets/node1.yaml '["k"]' '"v"'`.
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum GitCmd {
    /// Point git at this device's key for signing and at the signers file
    /// for verifying. Writes the key as an OpenSSH file under ~/.config/dd.
    Setup,
    /// Write git's allowed-signers file from every entry in the directory:
    /// each person's devices, so `git log --show-signature` names them.
    Signers,
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Give another device the key: it reads and pushes from then on.
    Share {
        /// the remote, with or without the dd:: prefix
        url: String,
        /// their device public key, from `dd device show` there
        public_key: String,
    },
    /// Who holds the key, and where the remote stands.
    Readers { url: String },
}

#[derive(Subcommand)]
enum DeviceCmd {
    /// Show this device's key: fingerprint and the public key to admit.
    Show,
    /// Admit a device to your identity by its public key, from `dd device
    /// show` there. Needs the root key on this machine.
    Admit { public_key: String },
    /// Remove a device by its fingerprint, or the start of it (Settings >
    /// Devices shows it): it is signed out everywhere and cannot sign in
    /// again. Needs the root key on this machine.
    Remove { fingerprint: String },
}

#[derive(Subcommand)]
enum MailCmd {
    /// Once: make the Worker's store, upload it, give it its two secrets
    /// and the name mail.<domain>; writes fleet/mail.json
    Setup {
        #[arg(long, default_value = "commonty.org")]
        domain: String,
        /// the branch whose Worker goes up first
        #[arg(long, default_value = "main")]
        r#ref: String,
    },
    /// Upload the Worker a branch builds (publish does this itself)
    Deploy {
        #[arg(long, default_value = "main")]
        r#ref: String,
    },
}

#[derive(Subcommand)]
enum ImageCmd {
    /// Create your repository. Once; `push` does it too if it is missing.
    /// The archive password is derived from the photo password by
    /// `dd unlock` / `dd signup` - there is nothing new to choose here.
    Init {
        /// This machine unlocked before archives existed: ask for the photo
        /// password once more to derive the archive password from it.
        #[arg(long)]
        rederive: bool,
    },
    /// Archive a file, or stdin with `-`. For a whole drive:
    /// `sudo cat /dev/sdX | dd image push - --name old-laptop`
    /// so only cat runs as root and the keyring stays yours.
    Push {
        source: String,
        #[arg(long)]
        name: String,
    },
    /// Every archive in your repository.
    List,
    /// Let your passkey open your archives in a browser (the Backups page):
    /// the archive password, sealed to each passkey in your entry, beside
    /// the archives. `push` does it too.
    Passkey,
    /// Delete archives, by id prefix (from `list`), and give their space
    /// back. Asks first unless --yes.
    Delete {
        ids: Vec<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Rebuild the index from what is on the server. After an interrupted
    /// push, this lets the rerun skip everything already uploaded.
    Repair,
    /// Write an archive back out to stdout.
    Pull {
        name: String,
        /// Snapshot id prefix; the newest if omitted.
        #[arg(long, default_value = "latest")]
        snapshot: String,
    },
}

/// The archive password, sealed to every passkey in this person's entry and
/// put beside the archives as dd-passkeys.json: a browser holding one of
/// those passkeys opens it the way it opens a library (client/web), and
/// nothing on the server can. Returns how many passkeys it was sealed to.
fn seal_for_passkeys(keys: &impl auth::KeyStore, repo: &str, password: &str) -> Result<usize> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(password)
        .context("the archive password is not the derived kind")?;
    let user = keys.get(USER)?.context("no name on this machine")?;
    let kp = auth::device::load(keys)?.context("no device key here - `dd device show`")?;
    let token = auth::device::mint(&kp, user.as_str(), std::time::Duration::from_secs(600))?;
    let base = url::Url::parse(repo)?;
    let host = base
        .host_str()
        .context("no host in the repository's address")?;
    let domain = host.split_once('.').map(|(_, d)| d).unwrap_or(host);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let http = reqwest::Client::new();
        let entry: serde_json::Value = http
            .get(format!(
                "https://home.{domain}/_dd/directory/{}",
                user.as_str()
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut sealed = serde_json::Map::new();
        for p in entry["entry"]["passkeys"].as_array().into_iter().flatten() {
            if let (Some(id), Some(k)) = (p["id"].as_str(), p["library_key"].as_str()) {
                sealed.insert(
                    id.to_string(),
                    serde_json::Value::String(library::seal_to(k, &raw)?),
                );
            }
        }
        anyhow::ensure!(
            !sealed.is_empty(),
            "no passkey in your entry can open things in a browser yet: `dd enrol` one"
        );
        http.put(base.join("dd-passkeys.json")?)
            .bearer_auth(&token)
            .json(&serde_json::Value::Object(sealed.clone()))
            .send()
            .await?
            .error_for_status()?;
        Ok(sealed.len())
    })
}

/// Archives are written without locks (rustic keeps none), and the Backups
/// page deletes packs straight away. So each says it is at work with a
/// file beside the archives, `{"until": <unix secs>}` kept five minutes
/// ahead while it runs, and neither starts while the other's is current.
struct Busy {
    stop: std::sync::mpsc::Sender<()>,
    beat: Option<std::thread::JoinHandle<()>>,
}

fn busy_put(http: &reqwest::Client, url: &url::Url, token: &str) -> reqwest::RequestBuilder {
    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + 300;
    http.put(url.clone())
        .bearer_auth(token)
        .json(&serde_json::json!({ "until": until }))
}

async fn busy_current(http: &reqwest::Client, url: url::Url, token: &str) -> Result<bool> {
    let r = http.get(url).bearer_auth(token).send().await?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    let v: serde_json::Value = r.error_for_status()?.json().await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(v["until"].as_u64().is_some_and(|u| u > now))
}

impl Busy {
    fn start(keys: &impl auth::KeyStore, repo: &str) -> Result<Busy> {
        let user = keys.get(USER)?.context("no name on this machine")?;
        let kp = auth::device::load(keys)?.context("no device key here - `dd device show`")?;
        let base = url::Url::parse(&format!("{}/", repo.trim_end_matches('/')))?;
        let mine = base.join("dd-pushing")?;
        let theirs = base.join("dd-deleting")?;
        let token =
            move || auth::device::mint(&kp, user.as_str(), std::time::Duration::from_secs(600));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let http = reqwest::Client::new();
        let t = token()?;
        rt.block_on(async {
            anyhow::ensure!(
                !busy_current(&http, theirs.clone(), &t).await?,
                "the Backups page is deleting an image here; push again when it is done"
            );
            busy_put(&http, &mine, &t)
                .send()
                .await?
                .error_for_status()?;
            // both may have looked at once: the second to look backs off
            if busy_current(&http, theirs, &t).await? {
                let _ = http.delete(mine.clone()).bearer_auth(&t).send().await;
                anyhow::bail!(
                    "the Backups page is deleting an image here; push again when it is done"
                );
            }
            Ok(())
        })?;
        let (stop, rx) = std::sync::mpsc::channel::<()>();
        let beat = std::thread::spawn(move || {
            loop {
                let done = !matches!(
                    rx.recv_timeout(std::time::Duration::from_secs(60)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                );
                let Ok(t) = token() else { continue };
                rt.block_on(async {
                    let _ = if done {
                        http.delete(mine.clone()).bearer_auth(&t).send().await
                    } else {
                        busy_put(&http, &mine, &t).send().await
                    };
                });
                if done {
                    break;
                }
            }
        });
        Ok(Busy {
            stop,
            beat: Some(beat),
        })
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(b) = self.beat.take() {
            let _ = b.join();
        }
    }
}

fn image(cmd: ImageCmd, repo: String) -> Result<()> {
    use archive::{Archive, Source, Stderr, TokenProvider};
    let keys = auth::open(&service());

    if let ImageCmd::Init { rederive: true } = &cmd {
        let master = Zeroizing::new(rpassword::prompt_password("photo password: ")?);
        keys.set(ARCHIVE, &derive::archive_password(&master))?;
    }
    let password = keys.get(ARCHIVE)?.context(
        "no archive password on this machine - run `dd unlock`, or `dd image init --rederive`",
    )?;

    let tokens = std::sync::Arc::new(TokenProvider::new(auth::open(&service())));
    let archive = Archive::new(url::Url::parse(&repo)?, tokens, password.clone(), Stderr)?;

    match cmd {
        ImageCmd::Init { .. } => {
            archive.init()?;
            println!("repository created at {repo}");
        }
        ImageCmd::Push { source, name } => {
            let src = if source == "-" {
                Source::Stdin
            } else {
                Source::File(std::path::PathBuf::from(source))
            };
            let busy = Busy::start(&keys, &repo)?;
            let e = archive.push(src, &name)?;
            drop(busy);
            println!(
                "archived {} as {}  ({} bytes)  snapshot {}",
                name, e.name, e.bytes, e.id
            );
            // so the Backups page opens it too; not a reason to fail the push
            if let Err(err) = seal_for_passkeys(&keys, &repo, &password) {
                eprintln!(
                    "(the Backups page will not open this until `dd image passkey`: {err:#})"
                );
            }
        }
        ImageCmd::Repair => {
            archive.repair()?;
            println!("index rebuilt - rerun the push, it will skip what is already there");
        }
        ImageCmd::Passkey => {
            let n = seal_for_passkeys(&keys, &repo, &password)?;
            println!("your archives open with {n} passkey(s) on the Backups page");
        }
        ImageCmd::Delete { ids, yes } => {
            anyhow::ensure!(!ids.is_empty(), "which archives? ids from `dd image list`");
            let all = archive.list()?;
            for p in &ids {
                for e in all.iter().filter(|e| e.id.starts_with(p.as_str())) {
                    println!(
                        "{:<10} {:<28} {:>14}  {}",
                        &e.id[..8.min(e.id.len())],
                        e.name,
                        e.bytes,
                        e.time
                    );
                }
            }
            if !yes {
                eprint!("delete these for good? type yes: ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                anyhow::ensure!(answer.trim() == "yes", "nothing deleted");
            }
            for e in archive.delete(&ids)? {
                println!("deleted {} ({})", e.name, &e.id[..8.min(e.id.len())]);
            }
        }
        ImageCmd::List => {
            let entries = archive.list()?;
            if entries.is_empty() {
                println!("nothing archived yet");
            }
            for e in entries {
                println!(
                    "{:<10} {:<28} {:>14}  {}",
                    &e.id[..8.min(e.id.len())],
                    e.name,
                    e.bytes,
                    e.time
                );
            }
        }
        ImageCmd::Pull { name, snapshot } => {
            let mut out = std::io::stdout().lock();
            archive.pull(&snapshot, &name, &mut out)?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let keys = auth::open(&service());

    match cli.command {
        Command::Signup {
            username,
            email,
            directories,
            ente_origin,
            skip_photos,
            password_stdin,
        } => {
            let (kp, _) = auth::device::load_or_create(&keys)?;
            let recovery = who::create(&keys, &directories, &username, &kp, None).await?;
            keys.set(USER, &username)?;
            println!(
                "identity {username} published; device {} is its first",
                auth::device::fingerprint(&kp)
            );
            who::print_recovery(&recovery);

            if skip_photos {
                println!(
                    "\nphotos: skipped - `dd unlock --email you@example.com` once you have \
                     made the account in the photos app"
                );
                return Ok(());
            }
            if keys.get("ente")?.is_some() {
                println!("\nphotos: already unlocked on this machine, leaving it alone");
                return Ok(());
            }

            // A SECOND password, deliberately. This one is never sent anywhere:
            // it derives the key that decrypts the photos, which is why no
            // amount of sso can create this account for you.
            println!(
                "\nNow the photo account. This password is separate and cannot be \
                 recovered by anyone here - it derives the key your photos are \
                 encrypted with."
            );
            let password = if password_stdin {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                Zeroizing::new(line.trim_end_matches(['\n', '\r']).to_string())
            } else {
                let first = Zeroizing::new(rpassword::prompt_password("photo password: ")?);
                let again = Zeroizing::new(rpassword::prompt_password("again: ")?);
                anyhow::ensure!(first == again, "those did not match");
                anyhow::ensure!(first.len() >= 8, "use at least 8 characters");
                first
            };

            let archive_pw = derive::archive_password(&password);
            let client = ente::client(&ente_origin)?;
            let mut ui = ui::Term;
            let mut flow = ente::AuthFlow::new(&client, &mut ui);
            let account = flow
                .create_account(ente::CreateAccountParams {
                    email: email.clone(),
                    password,
                    source: None,
                })
                .await?;

            // Generated now rather than offered later, because the moment an
            // account exists is the only moment its owner has nothing to lose
            // by writing this down.
            let recovery = match &account.recovery_key {
                Some(k) => k.clone(),
                None => {
                    flow.create_recovery_key(&account.secrets.master_key, &account.key_attributes)
                        .await?
                        .recovery_key
                }
            };

            let v = Vault::from_secrets(account.user_id, &account.secrets);
            keys.set("ente", &v.to_json()?)?;
            keys.set(ARCHIVE, &archive_pw)?;

            println!(
                "\nphotos: created and unlocked (user_id {})",
                account.user_id
            );
            println!("\n  RECOVERY KEY - write this down, on paper, now.");
            println!("  It is the only way back in if you forget the photo password.");
            println!("  Nobody running this service can recover it for you.\n");
            println!("    {recovery}\n");
            println!("For a browser: `dd enrol` prints a link that sets up a passkey.");
        }

        Command::Enrol { url, directories } => {
            let kp = auth::device::load(&keys)?.context("no device key here - `dd device show`")?;
            let root = who::load_root(&keys)?
                .context("no root key on this machine - the passkey has to be signed in")?;
            let user = keys
                .get(USER)?
                .context("no name on this machine - `dd identity new` or `dd identity import`")?;
            let tok = auth::device::mint_for(
                &kp,
                &user,
                std::time::Duration::from_secs(600),
                Some("enrol"),
            )?;
            println!(
                "open this within ten minutes, in the browser that should get the passkey:
"
            );
            println!(
                "  {url}?t={tok}
"
            );
            println!("waiting for the browser...");
            let http = reqwest::Client::new();
            let result = format!("{}/result", url.trim_end_matches('/'));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
            let cred = loop {
                if std::time::Instant::now() > deadline {
                    anyhow::bail!("no passkey arrived within ten minutes");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let r = http.get(&result).bearer_auth(&tok).send().await?;
                match r.status().as_u16() {
                    200 => break r.json::<identity::Passkey>().await?,
                    204 => continue,
                    s => anyhow::bail!("{s} {}", r.text().await.unwrap_or_default()),
                }
            };
            let id = cred.id.clone();
            let signed = who::admit_passkey(&directories, &user, &root, cred).await?;
            println!(
                "passkey {id} signed into your entry; version {}",
                signed.entry.version
            );
            println!("the browser can sign in now, on every box that has your entry.");
        }

        Command::Library { cmd, directories } => librarycmd::run(cmd, &keys, &directories).await?,
        Command::Media { at, directories } => mediacmd::run(&keys, &directories, at).await?,

        Command::Passkey { cmd, directories } => match cmd {
            PasskeyCmd::List => {
                let name = keys
                    .get(USER)?
                    .context("no name here - `dd identity new` or `dd identity import`")?
                    .to_string();
                let found = who::fetch(&directories, &name).await;
                let e = directory::resolve(&found, directory::Anchor::FirstSight)
                    .context("no directory has a usable entry")?;
                if e.entry.passkeys.is_empty() {
                    println!("no passkeys - `dd enrol` adds one");
                }
                for p in &e.entry.passkeys {
                    println!("{}  added {}", p.id, p.added);
                }
            }
            PasskeyCmd::Remove { id } => {
                let root = who::load_root(&keys)?.context("no root key on this machine")?;
                let name = keys.get(USER)?.context("no name here")?.to_string();
                let signed = who::remove_passkey(&directories, &name, &root, &id).await?;
                println!("passkey {id} removed; version {}", signed.entry.version);
            }
            PasskeyCmd::Link { id, library_key } => {
                let root = who::load_root(&keys)?.context("no root key on this machine")?;
                let name = keys.get(USER)?.context("no name here")?.to_string();
                let signed =
                    account::link_passkey(&directories, &name, &root, &id, &library_key).await?;
                println!(
                    "passkey {id} can open your libraries; version {}",
                    signed.entry.version
                );
            }
            PasskeyCmd::Add { file } => {
                let root = who::load_root(&keys)?.context("no root key on this machine")?;
                let name = keys.get(USER)?.context("no name here")?.to_string();
                let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
                let creds: Vec<serde_json::Value> = match v.get("passkeys") {
                    Some(serde_json::Value::Array(a)) => a.clone(),
                    _ => vec![v],
                };
                for cred in creds {
                    let id = cred
                        .pointer("/cred/cred_id")
                        .and_then(|x| x.as_str())
                        .context("no cred.cred_id in that record")?
                        .to_string();
                    // a record the browser made carries the key it derived
                    // from this passkey; one from elsewhere does not
                    let library_key = cred
                        .get("library_key")
                        .and_then(|x| x.as_str())
                        .map(str::to_string);
                    // the box that ran the ceremony says which domain it
                    // was for; without one the assertion is checked as it
                    // always was
                    let rp_id = cred
                        .get("rp_id")
                        .and_then(|x| x.as_str())
                        .map(str::to_string);
                    let signed = who::admit_passkey(
                        &directories,
                        &name,
                        &root,
                        identity::Passkey {
                            id: id.clone(),
                            cred,
                            added: identity::now(),
                            library_key,
                            rp_id,
                        },
                    )
                    .await?;
                    println!("passkey {id} signed in; version {}", signed.entry.version);
                }
            }
        },

        Command::Identity { cmd, directories } => match cmd {
            IdentityCmd::New { name, code } => {
                let name = match name.or(keys.get(USER)?.map(|z| z.to_string())) {
                    Some(n) => n,
                    None => anyhow::bail!("no name: pass --name"),
                };
                let (kp, _) = auth::device::load_or_create(&keys)?;
                let recovery =
                    who::create(&keys, &directories, &name, &kp, code.as_deref()).await?;
                keys.set(USER, &name)?;
                println!(
                    "identity {name} published; device {} is its first",
                    auth::device::fingerprint(&kp)
                );
                who::print_recovery(&recovery);
            }
            IdentityCmd::Show => {
                let name = keys
                    .get(USER)?
                    .context("no name here - `dd identity new` or `dd identity import`")?
                    .to_string();
                let mine =
                    who::load_root(&keys)?.map(|r| identity::encode_public(&r.verifying_key()));
                let here = auth::device::load(&keys)?.map(|kp| auth::device::public_b64(&kp));
                println!("{name}");
                println!(
                    "root here:   {}",
                    mine.as_deref()
                        .map(identity::fingerprint)
                        .unwrap_or("none".into())
                );
                if let Some(m) = &mine {
                    println!("member id:   {}", identity::member_id(m));
                }
                for (d, r) in who::fetch(&directories, &name).await {
                    match r {
                        Ok(None) => println!("{d}: no entry"),
                        Err(e) => println!("{d}: unreachable ({e})"),
                        Ok(Some(e)) => {
                            let owner = match &mine {
                                Some(m) if *m == e.entry.root => "yours",
                                Some(_) => "NOT YOURS - different root",
                                None => "root not held here",
                            };
                            println!(
                                "{d}: version {} root {} ({owner})",
                                e.entry.version,
                                identity::fingerprint(&e.entry.root)
                            );
                            for dev in &e.entry.devices {
                                let this = here.as_deref() == Some(dev.public_key.as_str());
                                println!(
                                    "  device {}{}",
                                    dev.fingerprint,
                                    if this { " (this one)" } else { "" }
                                );
                            }
                            for p in &e.entry.passkeys {
                                println!("  passkey {}", p.id);
                            }
                        }
                    }
                }
            }
            IdentityCmd::Publish => {
                let name = keys
                    .get(USER)?
                    .context("no name here - `dd identity new` or `dd identity import`")?
                    .to_string();
                let found = who::fetch(&directories, &name).await;
                let newest = directory::resolve(&found, directory::Anchor::FirstSight)
                    .context("no directory has a usable entry")?;
                let behind: Vec<String> = found
                    .iter()
                    .filter(|(_, r)| !matches!(r, Ok(Some(e)) if e.entry.version >= newest.entry.version))
                    .map(|(d, _)| d.clone())
                    .collect();
                if behind.is_empty() {
                    println!("every directory has version {}", newest.entry.version);
                } else {
                    who::publish(&behind, &newest).await?;
                }
            }
            IdentityCmd::Recover { name, key_stdin } => {
                let name = match name.or(keys.get(USER)?.map(|z| z.to_string())) {
                    Some(n) => n,
                    None => anyhow::bail!("no name: pass --name"),
                };
                let key = if key_stdin {
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    Zeroizing::new(line.trim().to_string())
                } else {
                    Zeroizing::new(rpassword::prompt_password("recovery key: ")?)
                };
                let (kp, _) = auth::device::load_or_create(&keys)?;
                let next = who::recover(&keys, &directories, &name, &key, &kp).await?;
                keys.set(USER, &name)?;
                println!("recovered {name}: new root here, every other device dropped");
                who::print_recovery(&next);
            }
            IdentityCmd::Export => {
                let root = who::load_root(&keys)?.context("no identity here")?;
                println!("{}", *identity::encode_secret(&root));
            }
            IdentityCmd::Import { name } => {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                let root = identity::decode_secret(line.trim()).context("bad root secret")?;
                keys.set(who::ROOT, &identity::encode_secret(&root))?;
                keys.set(USER, &name)?;
                println!(
                    "root for {name} stored ({})",
                    identity::fingerprint(&identity::encode_public(&root.verifying_key()))
                );
            }
        },

        Command::Release { cmd, repo } => release_cmd::run(cmd, &repo, &auth::open(&service()))?,
        Command::Mail { cmd, repo } => {
            let root = release_cmd::repo_root(&repo)?;
            let keys = auth::open(&service());
            match cmd {
                MailCmd::Setup { domain, r#ref } => {
                    mail_worker::setup(&root, &keys, &domain, &r#ref).await?
                }
                MailCmd::Deploy { r#ref } => mail_worker::deploy(&root, &keys, &r#ref).await?,
            }
        }

        Command::Invite { ttl, directories } => {
            let key = release_cmd::load(&auth::open(&service()))?;
            member::invite(&ttl, &directories, &key).await?
        }
        Command::Member {
            cmd,
            repo,
            directories,
        } => match cmd {
            MemberCmd::Add { who } => member::add(&repo, &who, &directories).await?,
            MemberCmd::Remove { who } => member::remove(&repo, &who, &directories).await?,
            MemberCmd::List => member::list(&repo, &directories).await?,
        },
        Command::Box { cmd, repo } => match cmd {
            BoxCmd::Public { name } => {
                let path = std::path::Path::new(&repo).join("fleet/boxes.json");
                let boxes: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
                let b = boxes.get(&name).with_context(|| format!("no box {name}"))?;
                let addr = b["tailnet"]
                    .as_str()
                    .context("box has no tailnet address")?;
                // on the box: what the world sees, and what the router thinks
                let probe = concat!(
                    "seen=$(curl -fsS -4 --max-time 10 https://api.ipify.org || echo none); ",
                    "gw=$(ip -4 route show default | awk '{print $3; exit}'); ",
                    "local=$(ip -4 -o addr show $(ip -4 route show default | awk '{print $5; exit}') | awk '{print $4; exit}'); ",
                    "echo \"seen=$seen gw=$gw local=$local\""
                );
                let out = std::process::Command::new("ssh")
                    .args(["-o", "BatchMode=yes", &format!("admin@{addr}"), probe])
                    .output()
                    .context("ssh to the box")?;
                anyhow::ensure!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                let text = String::from_utf8_lossy(&out.stdout);
                println!("{name}: {}", text.trim());
                let seen = text
                    .split_whitespace()
                    .find_map(|kv| kv.strip_prefix("seen="))
                    .unwrap_or("none");
                let private = |ip: &str| {
                    ip.starts_with("10.")
                        || ip.starts_with("192.168.")
                        || ip.starts_with("100.")
                        || (ip.starts_with("172.")
                            && ip
                                .split('.')
                                .nth(1)
                                .and_then(|o| o.parse::<u8>().ok())
                                .is_some_and(|o| (16..=31).contains(&o)))
                };
                if seen == "none" {
                    println!("the box cannot reach the internet: nothing to decide yet");
                } else if private(seen) {
                    println!(
                        "the world sees a private address: carrier nat. No forward will ever reach this box; the front door stays a tunnel."
                    );
                } else {
                    println!(
                        "the world sees {seen}. The front door is a tunnel either way; a direct forward would also work only if the router's WAN page shows this same address and the block is not a carrier's shared pool (check the address's owner)."
                    );
                }
            }
            BoxCmd::List { private } => {
                let path = std::path::Path::new(&repo).join("fleet/boxes.json");
                let boxes: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
                )?;
                let secret: Option<serde_json::Value> = if private {
                    let out = std::process::Command::new(std::env::current_exe()?)
                        .args(["secret", "run", "--", "-d", "--output-type", "json"])
                        .arg(std::path::Path::new(&repo).join("secrets/fleet.yaml"))
                        .env(
                            "DD_KEYRING_FILE",
                            std::env::var("DD_KEYRING_FILE").unwrap_or_default(),
                        )
                        .output()?;
                    anyhow::ensure!(out.status.success(), "decrypting secrets/fleet.yaml");
                    Some(serde_json::from_slice(&out.stdout)?)
                } else {
                    None
                };
                for (name, b) in boxes.as_object().context("boxes.json is not an object")? {
                    let roles: Vec<&str> = b["roles"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|r| r.as_str()).collect())
                        .unwrap_or_default();
                    println!(
                        "{name}  {} {}  tailnet {}  {}",
                        b["siteId"].as_str().unwrap_or("-"),
                        b["regionId"].as_str().unwrap_or("-"),
                        b["tailnet"].as_str().unwrap_or("-"),
                        if b["public"].as_bool().unwrap_or(false) {
                            "public"
                        } else {
                            "tailnet only"
                        },
                    );
                    println!("  roles: {}", roles.join(" "));
                    // what the box itself says, from its own metrics over the tailnet
                    if let Some(tailnet) = b["tailnet"].as_str() {
                        let out = std::process::Command::new("curl")
                            .args([
                                "-sf",
                                "-m",
                                "5",
                                "--get",
                                "--data-urlencode",
                                "query=dd_box_location",
                            ])
                            .arg(format!("http://{tailnet}:9090/api/v1/query"))
                            .output();
                        let seen = out
                            .ok()
                            .and_then(|o| {
                                serde_json::from_slice::<serde_json::Value>(&o.stdout).ok()
                            })
                            .and_then(|v| v["data"]["result"].as_array()?.first().cloned());
                        match seen {
                            Some(r) => {
                                let m = &r["metric"];
                                let (site, region) = (
                                    m["site"].as_str().unwrap_or("?"),
                                    m["region"].as_str().unwrap_or("?"),
                                );
                                let agree = site == b["siteId"].as_str().unwrap_or("")
                                    && region == b["regionId"].as_str().unwrap_or("");
                                println!(
                                    "  says: {site} {region} ({}){}",
                                    m["source"].as_str().unwrap_or("?"),
                                    if agree {
                                        ""
                                    } else {
                                        "  <- differs from the list"
                                    }
                                );
                            }
                            None => println!("  says: unreachable"),
                        }
                    }
                    if let Some(sec) = &secret {
                        let sb = &sec["boxes"][name];
                        let site = sb["site"].as_str().unwrap_or("-");
                        let region = sb["region"].as_str().unwrap_or("-");
                        println!(
                            "  owner: {}   site: {}   region: {}",
                            sb["owner"].as_str().unwrap_or("-"),
                            sec["sites"][site].as_str().unwrap_or(site),
                            sec["regions"][region].as_str().unwrap_or(region)
                        );
                        if let Some(c) = sb["contents"].as_array() {
                            for item in c {
                                println!(
                                    "  holds: {} - {}",
                                    item["role"].as_str().unwrap_or("-"),
                                    item["state"].as_str().unwrap_or("-")
                                );
                            }
                        }
                    }
                }
            }
        },

        Command::Secret { cmd } => {
            use secrecy::ExposeSecret as _;
            const AGE: &str = "age-key";
            match cmd {
                SecretCmd::Init => {
                    if keys.get(AGE)?.is_some() {
                        anyhow::bail!("this machine already holds a key - `dd secret recipient`");
                    }
                    let id = age::x25519::Identity::generate();
                    keys.set(AGE, id.to_string().expose_secret())?;
                    println!("{}", id.to_public());
                    println!(
                        "add that as a recipient in .sops.yaml, then re-encrypt with a key that already can:"
                    );
                    println!("  sops updatekeys secrets/node1.yaml");
                }
                SecretCmd::Recipient => {
                    let id: age::x25519::Identity = keys
                        .get(AGE)?
                        .context("no key here - `dd secret init`")?
                        .parse()
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    println!("{}", id.to_public());
                }
                SecretCmd::Run { args } => {
                    let key = keys.get(AGE)?.context("no key here - `dd secret init`")?;
                    // sops from PATH, else through nix, so a fresh machine
                    // works. Through nix it is this repo's own pinned sops,
                    // not whatever nixpkgs-unstable is serving today: the
                    // fleet's age key goes into that process's environment,
                    // and an unpinned binary fetched at the moment of use is
                    // a stranger to hand it to.
                    let (prog, pre): (&str, Vec<&str>) = if which("sops") {
                        ("sops", vec![])
                    } else {
                        ("nix", vec!["run", ".#sops", "--"])
                    };
                    let status = std::process::Command::new(prog)
                        .args(pre)
                        .args(&args)
                        .env("SOPS_AGE_KEY", key.as_str())
                        .status()?;
                    std::process::exit(status.code().unwrap_or(1));
                }
            }
        }

        Command::Git { cmd, directories } => {
            let dir = std::env::var("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|_| {
                    std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
                })
                .context("no HOME")?
                .join("dd");
            std::fs::create_dir_all(&dir)?;
            let signers = dir.join("allowed_signers");
            match cmd {
                GitCmd::Setup => {
                    let (kp, _) = auth::device::load_or_create(&keys)?;
                    let seed: [u8; 32] = kp.private().to_bytes()[..]
                        .try_into()
                        .context("device key is not 32 bytes")?;
                    let name = keys.get(USER)?.map(|n| n.to_string()).unwrap_or_default();
                    let mut key = ssh_key::PrivateKey::from(
                        ssh_key::private::Ed25519Keypair::from_seed(&seed),
                    );
                    key.set_comment(format!("dd device of {name}"));
                    let path = dir.join("device_ed25519");
                    // the same key the keyring holds, as a file git's ssh signing
                    // can read; 0600 from the moment it exists (and narrowed
                    // before writing, should an older copy be wider), no
                    // passphrase, the standing of any ssh key
                    {
                        use std::io::Write as _;
                        let mut o = std::fs::OpenOptions::new();
                        o.write(true).create(true).truncate(true);
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::OpenOptionsExt as _;
                            o.mode(0o600);
                        }
                        let mut f = o.open(&path)?;
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt as _;
                            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                        }
                        f.write_all(key.to_openssh(ssh_key::LineEnding::LF)?.as_bytes())?;
                    }
                    std::fs::write(
                        dir.join("device_ed25519.pub"),
                        format!("{}\n", key.public_key().to_openssh()?),
                    )?;
                    for (k, v) in [
                        ("gpg.format", "ssh".to_string()),
                        ("user.signingkey", path.display().to_string()),
                        ("gpg.ssh.allowedSignersFile", signers.display().to_string()),
                        ("commit.gpgsign", "true".to_string()),
                        ("tag.gpgsign", "true".to_string()),
                    ] {
                        let s = std::process::Command::new("git")
                            .args(["config", "--global", k, &v])
                            .status()?;
                        anyhow::ensure!(s.success(), "git config {k}");
                    }
                    println!(
                        "git signs with device {} from {}",
                        auth::device::fingerprint(&kp),
                        path.display()
                    );
                    if !signers.exists() {
                        println!(
                            "run `dd git signers` so verification has something to check against"
                        );
                    }
                }
                GitCmd::Signers => {
                    let mut lines = std::collections::BTreeSet::new();
                    let mut people = 0;
                    // every name any directory lists, then each entry asked
                    // of all of them: one directory's word is not enough to
                    // put a key in the file git trusts signatures against
                    let mut names = std::collections::BTreeSet::new();
                    for d in &directories {
                        if let Ok(ns) = directory::names(d).await {
                            names.extend(ns);
                        }
                    }
                    for name in names {
                        if !identity::valid_name(&name) {
                            continue;
                        }
                        let found = who::fetch(&directories, &name).await;
                        let Ok(e) = directory::resolve(&found, directory::Anchor::FirstSight)
                        else {
                            continue;
                        };
                        // the entry has to be the one asked for: a listing
                        // is a directory's claim, the entry is the person's
                        if e.entry.name != name {
                            continue;
                        }
                        people += 1;
                        for dev in &e.entry.devices {
                            let Ok(vk) = identity::decode_public(&dev.public_key) else {
                                continue;
                            };
                            let pk = ssh_key::PublicKey::from(ssh_key::public::Ed25519PublicKey(
                                vk.to_bytes(),
                            ));
                            // git matches the principal against the signer's email;
                            // name@domain is what `dd git setup` puts in user.email
                            lines.insert(format!(
                                "{name}@dd {}",
                                pk.to_openssh().unwrap_or_default()
                            ));
                        }
                    }
                    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
                    std::fs::write(&signers, body)?;
                    println!(
                        "{} signer(s) from {} entries -> {}",
                        lines.len(),
                        people,
                        signers.display()
                    );
                }
            }
        }

        Command::Repo { cmd } => {
            // the remote helper owns the format; it lives next to dd
            let helper = std::env::current_exe()?
                .parent()
                .map(|d| d.join("git-remote-dd"))
                .filter(|p| p.exists())
                .unwrap_or_else(|| "git-remote-dd".into());
            let status = match cmd {
                RepoCmd::Share { url, public_key } => std::process::Command::new(&helper)
                    .args(["share", &url, &public_key])
                    .status()?,
                RepoCmd::Readers { url } => std::process::Command::new(&helper)
                    .args(["readers", &url])
                    .status()?,
            };
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }

        Command::Device { cmd, directories } => match cmd {
            DeviceCmd::Show => {
                // made on first sight: a new device's first step is to show
                // its key to one that can admit it, before any login
                let (kp, _) = auth::device::load_or_create(&keys)?;
                println!("{}", auth::device::fingerprint(&kp));
                println!("{}", auth::device::public_b64(&kp));
            }
            DeviceCmd::Admit { public_key } => {
                let root = who::load_root(&keys)?.context(
                    "no root key on this machine - `dd identity export` on one that has it",
                )?;
                let name = keys
                    .get(USER)?
                    .context("no name here - `dd identity new` or `dd identity import`")?;
                let signed = who::admit(&directories, &name, &root, &public_key).await?;
                println!(
                    "device {} admitted; version {}",
                    identity::fingerprint(&public_key),
                    signed.entry.version
                );
            }
            DeviceCmd::Remove { fingerprint } => {
                let root = who::load_root(&keys)?.context(
                    "no root key on this machine - `dd identity export` on one that has it",
                )?;
                let name = keys
                    .get(USER)?
                    .context("no name here - `dd identity new` or `dd identity import`")?;
                let signed = who::remove_device(&directories, &name, &root, &fingerprint).await?;
                println!("device removed; version {}", signed.entry.version);
            }
        },

        Command::Unlock {
            origin,
            email,
            password_stdin,
        } => {
            if keys.get("ente")?.is_some() {
                println!("already unlocked on this machine - `dd lock` first to redo it");
                return Ok(());
            }
            let password = if password_stdin {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                Zeroizing::new(line.trim_end_matches(['\n', '\r']).to_string())
            } else {
                Zeroizing::new(rpassword::prompt_password("ente password: ")?)
            };
            let archive_pw = derive::archive_password(&password);
            let client = ente::client(&origin)?;
            let mut ui = ui::Term;
            let mut flow = ente::AuthFlow::new(&client, &mut ui);
            let account = flow.login(LoginParams { email, password }).await?;

            let v = Vault::from_secrets(account.user_id, &account.secrets);
            keys.set("ente", &v.to_json()?)?;
            // the same typed password also yields the archive password, so
            // `dd image` never asks for one
            keys.set(ARCHIVE, &archive_pw)?;
            println!("unlocked and stored for user_id {}", account.user_id);
        }

        Command::Token => {
            let kp = auth::device::load(&keys)?.context("no device key here - `dd device show`")?;
            let user = keys
                .get(USER)?
                .context("no name on this machine - `dd identity new` or `dd identity import`")?;
            println!(
                "{}",
                auth::device::mint(&kp, &user, std::time::Duration::from_secs(3600))?
            );
        }

        // rustic_core is synchronous and the token provider blocks on its own
        // small runtime. Neither may run on a tokio worker thread, so the
        // whole command gets a plain thread of its own.
        Command::Image { cmd, repo } => {
            return std::thread::spawn(move || image(cmd, repo))
                .join()
                .map_err(|_| anyhow::anyhow!("image command panicked"))?;
        }

        Command::Status => {
            match who::load_root(&keys)? {
                Some(r) => println!(
                    "identity: {} root held here",
                    identity::fingerprint(&identity::encode_public(&r.verifying_key()))
                ),
                None => {
                    println!("identity: no root here - `dd identity new` or `dd identity import`")
                }
            }
            match auth::device::load(&keys)? {
                Some(kp) => println!(
                    "device:  {} - signs its own tokens",
                    auth::device::fingerprint(&kp)
                ),
                None => println!("device:  none - `dd device show` makes one"),
            }
            match keys.get(ARCHIVE)? {
                Some(_) => println!("archive: password stored - `dd image push` works"),
                None => println!("archive: none - run `dd image init`"),
            }
            match keys.get("ente")? {
                Some(raw) => {
                    let v = Vault::from_json(&raw)?;
                    println!("ente:   unlocked (user_id {})", v.user_id);
                }
                None => println!("ente:   locked - run `dd unlock --email you@example.com`"),
            }
        }

        Command::StorageLedger {
            gate,
            ente_origin,
            photos_suffix,
            vault_suffix,
            password_file,
            budget_gb,
            out,
        } => {
            storage_ledger::run(storage_ledger::Ledger {
                gate,
                ente_origin,
                photos_suffix,
                vault_suffix,
                password_file,
                budget: budget_gb * 1_000_000_000,
                out,
            })
            .await?;
        }

        Command::PhotosDemo {
            ente_origin,
            email,
            code,
            password,
            from,
            quota,
        } => {
            let client = ente::client(&ente_origin)?;
            // the terminal's prompts, but the fleet's code answered for it
            struct Code(String, ui::Term);
            impl ente::AuthFlowUi for Code {
                fn read_email_otp(
                    &mut self,
                    _: &str,
                    _: ente::OtpPurpose,
                    _: bool,
                ) -> ente::Result<String> {
                    Ok(self.0.clone())
                }
                fn read_totp_code(&mut self, p: ente::TotpPurpose) -> ente::Result<String> {
                    self.1.read_totp_code(p)
                }
                fn report_retryable_error(&mut self, m: &str) -> ente::Result<()> {
                    self.1.report_retryable_error(m)
                }
                fn choose_second_factor(
                    &mut self,
                    m: &[ente::SecondFactorMethod],
                ) -> ente::Result<ente::SecondFactorMethod> {
                    self.1.choose_second_factor(m)
                }
                fn present_passkey_verification(&mut self, u: &str) -> ente::Result<()> {
                    self.1.present_passkey_verification(u)
                }
                fn wait_for_passkey_verification(&mut self) -> ente::Result<()> {
                    self.1.wait_for_passkey_verification()
                }
                fn present_totp_secret(&mut self, s: &str, q: &str) -> ente::Result<()> {
                    self.1.present_totp_secret(s, q)
                }
            }
            let mut ui = Code(code.clone(), ui::Term);
            // the account: made, or signed into if it already is
            let user_id = {
                let mut flow = ente::AuthFlow::new(&client, &mut ui);
                let login = flow
                    .login(ente::LoginParams {
                        email: email.clone(),
                        password: Zeroizing::new(password.clone()),
                    })
                    .await;
                match login {
                    Ok(a) => {
                        eprintln!("demo account exists (user {})", a.user_id);
                        a.user_id
                    }
                    Err(_) if from.is_some() => {
                        // the same account under its old address: moved,
                        // as the photos page moves a member's (ente_adopt)
                        let a = flow
                            .login(ente::LoginParams {
                                email: from.clone().unwrap(),
                                password: Zeroizing::new(password.clone()),
                            })
                            .await?;
                        client.set_auth_token(Some({
                            use base64::Engine as _;
                            base64::engine::general_purpose::URL_SAFE.encode(&a.secrets.token)
                        }));
                        client.send_otp(&email, "signup").await?;
                        client.change_email(&email, &code).await?;
                        eprintln!("demo account moved to {email} (user {})", a.user_id);
                        return Ok(());
                    }
                    Err(_) => {
                        client.send_otp(&email, "signup").await?;
                        let a = flow
                            .create_account_with_otp(
                                ente::CreateAccountParams {
                                    email: email.clone(),
                                    password: Zeroizing::new(password.clone()),
                                    source: None,
                                },
                                &code,
                            )
                            .await?;
                        eprintln!("demo account made (user {})", a.user_id);
                        a.user_id
                    }
                }
            };
            // the quota, with this machine's ente session: the admin's. The
            // api refuses a literal zero (a required field), so one byte,
            // which fits no photo.
            let raw = keys
                .get("ente")?
                .context("this machine holds no ente session: `dd unlock` as the admin first")?;
            let admin = Vault::from_json(&raw)?;
            // the vault keeps the token standard-base64; museum reads it url-safe
            let token = {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD.decode(&admin.token)?;
                base64::engine::general_purpose::URL_SAFE.encode(bytes)
            };
            let body = serde_json::json!({
                "userID": user_id,
                "storage": quota,
                "transactionID": "dd-demo",
                "productID": "free",
                "expiryTime": 4102444800000000i64,
                "paymentProvider": "",
            });
            let r = reqwest::Client::new()
                .put(format!(
                    "{}/admin/user/subscription",
                    ente_origin.trim_end_matches('/')
                ))
                .header("X-Auth-Token", &token)
                .json(&body)
                .send()
                .await?;
            anyhow::ensure!(
                r.status().is_success(),
                "museum refused the quota: {} {}",
                r.status(),
                r.text().await.unwrap_or_default()
            );
            if quota <= 1 {
                println!("demo photo account ready: {email}, quota 1 byte (read-only)");
            } else {
                println!(
                    "demo photo account: {email}, quota {quota} bytes; run again without --quota to make it read-only"
                );
            }
        }

        Command::Lock { forget_identity } => {
            keys.clear(auth::device::ACCOUNT)?;
            keys.clear(USER)?;
            keys.clear(ARCHIVE)?;
            keys.clear("ente")?;
            println!("ente keys and device key removed from this machine's credential store");
            if keys.get(who::ROOT)?.is_some() {
                if forget_identity {
                    keys.clear(who::ROOT)?;
                    println!("identity root removed too - only the paper key gets it back");
                } else {
                    println!("the identity root stays; `dd lock --forget-identity` removes it");
                }
            }
        }
    }
    Ok(())
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}
