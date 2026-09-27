//! git-remote-dd: an encrypted git remote inside an ordinary repository.
//!
//!   git remote add origin dd::ssh://forgejo@git.example/sarah/thing.git
//!
//! The repository at that url holds nothing but opaque files: encrypted
//! packs, an encrypted manifest of refs, and a signed list of who holds the
//! key. The forge stores and serves it, applies its permissions to who may
//! push, and cannot read a byte of it. Every push here encrypts the new
//! objects with the repository key before they leave this machine; every
//! fetch decrypts. Git does not know.
//!
//! What a hostile forge can do is serve an old state or refuse. A signed,
//! counted manifest makes the first visible - this helper refuses to go
//! backwards - and git's own hashes make tampering fail on fetch.
//!
//! Layout of the backing repository, branch `dd`:
//!   keys.json      who may read and who may sign, with the repository key
//!                  sealed to each reader's device key; signed by a signer
//!   manifest.enc   { counter, refs, packs } encrypted with the repository key
//!   packs/<id>     encrypted git packs, one per push
//!   sig.json       { counter, by, signature } over sha256(keys.json) and
//!                  sha256(manifest.enc)

mod crypto;

use std::collections::BTreeMap;
use std::io::{BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use auth::KeyStore;
use serde::{Deserialize, Serialize};

const SERVICE: &str = "commonty";
const DEFAULT_DIRECTORIES: [&str; 1] = ["https://home.commonty.org/_dd/directory"];

#[derive(Serialize, Deserialize, Clone, Default)]
struct Keys {
    /// device public keys that may sign a new state, base64
    signers: Vec<Reader>,
    /// which repository these states are: owner/name, as its url says. In
    /// the keys, so every state's signature covers it. Without it, one
    /// device's signature on repository B was as good on repository A: a
    /// forge could hand a fresh clone of A the history of B, or lay B's
    /// newer state on top of A's. Absent on states made before this; the
    /// first push after sets it, and from then no state may change or
    /// drop it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Reader {
    fingerprint: String,
    public_key: String,
    /// the repository key sealed to this device
    sealed_key: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Manifest {
    counter: u64,
    refs: BTreeMap<String, String>,
    packs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Sig {
    counter: u64,
    by: String,
    keys_sha256: String,
    manifest_sha256: String,
    signature: String,
}

impl Sig {
    fn message(&self) -> Vec<u8> {
        format!(
            "dd-remote v1\ncounter {}\nkeys {}\nmanifest {}\n",
            self.counter, self.keys_sha256, self.manifest_sha256
        )
        .into_bytes()
    }
}

/// What this machine remembers about a remote between runs, so a box that
/// serves an older state is caught.
#[derive(Serialize, Deserialize, Default)]
struct Local {
    counter: u64,
    /// the last commit of the backing repository this machine verified;
    /// the next check walks the chain from here
    #[serde(default)]
    commit: Option<String>,
    /// packs already unpacked into the local repository
    applied: Vec<String>,
}

struct Remote {
    /// the plain git url of the backing repository
    url: String,
    /// the directory name of whoever owns it (first path segment)
    owner: String,
    /// owner/name, as the url says: what every state has to say it is
    repo: String,
    /// our clone of the backing repository
    store: PathBuf,
    /// the repository git is running us for
    git_dir: PathBuf,
    local_path: PathBuf,
    device: crypto::Device,
    user: String,
    directories: Vec<String>,
}

/// A git command against one git directory. git launched us with GIT_DIR
/// (and friends) set relative to its own cwd; every child gets the exact
/// directory it is meant to touch instead, and nothing inherited.
fn gitcmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_DIR", dir);
    c
}

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let o = gitcmd(dir)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !o.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(o.stdout)
}

fn git_in(dir: &Path, args: &[&str], stdin: &[u8]) -> Result<Vec<u8>> {
    let mut c = gitcmd(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("git {}", args.join(" ")))?;
    c.stdin.take().unwrap().write_all(stdin)?;
    let o = c.wait_with_output()?;
    if !o.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(o.stdout)
}

impl Remote {
    fn open(url: &str) -> Result<Self> {
        let git_dir: PathBuf = std::env::var("GIT_DIR")
            .context("GIT_DIR is not set; run through git")?
            .into();
        let git_dir = git_dir.canonicalize()?;
        // whose repository: the path segment before the repository name,
        // the way every forge lays urls out. DD_REPO_OWNER overrides it for
        // a backing repository that is a plain path.
        let owner = match std::env::var("DD_REPO_OWNER") {
            Ok(o) if !o.is_empty() => o,
            _ => url
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .rsplit('/')
                .nth(1)
                .map(|s| s.rsplit(':').next().unwrap_or(s).to_string())
                .context("cannot tell the owner from the url")?,
        };
        let name = url
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        let id = &crypto::sha256_hex(url.as_bytes())[..16];
        let base = git_dir.join("dd").join(id);
        std::fs::create_dir_all(&base)?;
        let keys = auth::open(SERVICE);
        let kp = auth::device::load(&keys)?.context("no device key here - `dd device show`")?;
        let user = keys
            .get("user")?
            .context("no name on this machine - `dd identity new` or `dd identity import`")?
            .to_string();
        let directories = std::env::var("DD_DIRECTORIES")
            .ok()
            .map(|s| s.split(',').map(str::to_string).collect())
            .unwrap_or_else(|| DEFAULT_DIRECTORIES.iter().map(|s| s.to_string()).collect());
        Ok(Self {
            url: url.to_string(),
            repo: format!("{owner}/{name}"),
            owner,
            store: base.join("store"),
            local_path: base.join("local.json"),
            git_dir,
            device: crypto::Device::from_biscuit(&kp)?,
            user,
            directories,
        })
    }

    fn local(&self) -> Local {
        std::fs::read(&self.local_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn store_local(&self, l: &Local) -> Result<()> {
        std::fs::write(&self.local_path, serde_json::to_vec_pretty(l)?)?;
        Ok(())
    }

    /// Bring our clone of the backing repository up to date. An empty or
    /// missing `dd` branch means nothing has ever been pushed.
    fn sync_store(&self) -> Result<bool> {
        if !self.store.join("HEAD").exists() {
            git(
                &self.git_dir,
                &[
                    "clone",
                    "-q",
                    "--bare",
                    &self.url,
                    self.store.to_str().unwrap(),
                ],
            )?;
        } else if let Err(e) = git(
            &self.store,
            &[
                "fetch",
                "-q",
                "--force",
                "origin",
                "+refs/heads/dd:refs/heads/dd",
            ],
        ) {
            // the one fetch that may fail: nothing pushed yet, ever. A
            // forge that cannot be reached, or that stops showing a
            // branch we have already seen, is not "up to date".
            let seen = git(
                &self.store,
                &["rev-parse", "-q", "--verify", "refs/heads/dd"],
            )
            .is_ok();
            let there = git(&self.store, &["ls-remote", "--heads", "origin", "dd"])
                .with_context(|| format!("fetching {}", self.url))?;
            if seen || !there.is_empty() {
                return Err(e.context(format!("fetching {}", self.url)));
            }
        }
        Ok(git(
            &self.store,
            &["rev-parse", "-q", "--verify", "refs/heads/dd"],
        )
        .is_ok())
    }

    fn store_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.store_file_at("refs/heads/dd", path)
    }

    fn store_file_at(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let o = gitcmd(&self.store)
            .args(["cat-file", "-p", &format!("{commit}:{path}")])
            .output()?;
        Ok(o.status.success().then_some(o.stdout))
    }

    /// The owner's current device keys, from the directory. The one time a
    /// state is accepted on the strength of the directory rather than the
    /// previous state is the first sight of this remote.
    fn owner_devices(&self) -> Result<Vec<String>> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        let mut best: Option<identity::SignedEntry> = None;
        for d in &self.directories {
            let Ok(r) = http
                .get(format!("{}/{}", d.trim_end_matches('/'), self.owner))
                .send()
            else {
                continue;
            };
            if !r.status().is_success() {
                continue;
            }
            let Ok(e) = r.json::<identity::SignedEntry>() else {
                continue;
            };
            // the entry has to verify, and to be the one asked for
            if identity::verify(&e).is_err() || e.entry.name != self.owner {
                continue;
            }
            // and the directories have to agree whose root this name has.
            // A stale one is fine - same root, lower version - but two
            // different roots means one of them is making it up, and this
            // is the moment a first sight of a remote is decided on.
            if let Some(b) = &best {
                if b.entry.root != e.entry.root {
                    anyhow::bail!(
                        "the directories disagree about {}: {} and {}",
                        self.owner,
                        b.entry.root,
                        e.entry.root
                    );
                }
                if e.entry.version <= b.entry.version {
                    continue;
                }
            }
            best = Some(e);
        }
        let e = best.with_context(|| format!("no directory has an entry for {}", self.owner))?;
        Ok(e.entry
            .devices
            .iter()
            .map(|d| d.public_key.clone())
            .collect())
    }

    /// Check one state of the remote: the signature covers the files, and
    /// the signer was allowed by the state before (or, at the root, is one
    /// of the owner's devices in the directory). Needs no key, so a chain
    /// can be walked by a device that only became a reader later on.
    fn verify_state(&self, commit: &str, previous: Option<&Keys>) -> Result<(Keys, Sig)> {
        let keys_raw = self
            .store_file_at(commit, "keys.json")?
            .context("no keys.json on the remote")?;
        let manifest_raw = self
            .store_file_at(commit, "manifest.enc")?
            .context("no manifest.enc on the remote")?;
        let sig: Sig = serde_json::from_slice(
            &self
                .store_file_at(commit, "sig.json")?
                .context("no sig.json")?,
        )?;
        if sig.keys_sha256 != crypto::sha256_hex(&keys_raw)
            || sig.manifest_sha256 != crypto::sha256_hex(&manifest_raw)
        {
            bail!("the remote's signature does not cover the files it serves");
        }
        let keys: Keys = serde_json::from_slice(&keys_raw)?;
        let allowed: Vec<String> = match previous {
            Some(p) => p.signers.iter().map(|s| s.public_key.clone()).collect(),
            None => self.owner_devices()?,
        };
        let by = allowed
            .iter()
            .find(|pk| identity::fingerprint(pk) == sig.by)
            .with_context(|| {
                format!("state signed by {} which may not sign this remote", sig.by)
            })?;
        crypto::verify(by, &sig.message(), &sig.signature).context("remote state")?;
        Ok((keys, sig))
    }

    /// Open a verified state with this device's key.
    fn open_state(
        &self,
        commit: &str,
        keys: &Keys,
        sig: &Sig,
    ) -> Result<(Manifest, crypto::RepoKey)> {
        let me = self.device.fingerprint();
        let mine = keys.signers.iter().find(|r| r.fingerprint == me).with_context(|| {
            format!("device {me} is not a reader of this repository; ask its owner to run `dd repo share`")
        })?;
        let repo_key = self.device.unseal(&mine.sealed_key)?;
        let manifest_raw = self
            .store_file_at(commit, "manifest.enc")?
            .context("no manifest.enc on the remote")?;
        let manifest: Manifest =
            serde_json::from_slice(&crypto::decrypt(&repo_key, &manifest_raw)?)?;
        if manifest.counter != sig.counter {
            bail!("manifest and signature disagree on the counter");
        }
        Ok((manifest, repo_key))
    }

    /// Pull the backing repo and check every state since the one this
    /// machine last verified, each against the signers of the one before
    /// it. On first sight the chain starts at the first commit, whose
    /// signer has to be one of the owner's devices in the directory. A box
    /// can therefore neither invent a state nor drop back to an older one:
    /// history has to extend what was seen. Nothing is unpacked here.
    fn load(&self) -> Result<Option<(Keys, Manifest, crypto::RepoKey)>> {
        if !self.sync_store()? {
            return Ok(None);
        }
        let local = self.local();
        let head = String::from_utf8_lossy(&git(&self.store, &["rev-parse", "refs/heads/dd"])?)
            .trim()
            .to_string();
        let mut prev_keys: Option<Keys> = None;
        let range = match &local.commit {
            Some(c) => {
                if git(&self.store, &["merge-base", "--is-ancestor", c, &head]).is_err() {
                    bail!(
                        "the remote's history no longer contains state {} which this machine verified: refusing",
                        local.counter
                    );
                }
                prev_keys = std::fs::read(self.store.join("last-keys.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok());
                format!("{c}..{head}")
            }
            None => head.clone(),
        };
        let commits = git(
            &self.store,
            &["rev-list", "--reverse", "--first-parent", &range],
        )?;
        let commits: Vec<String> = String::from_utf8_lossy(&commits)
            .lines()
            .map(str::to_string)
            .collect();
        let mut counter = local.counter;
        let mut last: Option<(Keys, Sig)> = None;
        for c in &commits {
            let (keys, sig) = self.verify_state(c, prev_keys.as_ref())?;
            match (&keys.repo, prev_keys.as_ref().and_then(|p| p.repo.as_ref())) {
                (Some(r), _) if *r != self.repo => {
                    bail!(
                        "the remote serves a state of {r} as {}: refusing",
                        self.repo
                    )
                }
                (None, Some(was)) => {
                    bail!(
                        "a state of {} stopped saying which repository it is (was {was}): refusing",
                        self.repo
                    )
                }
                _ => {}
            }
            if sig.counter <= counter {
                bail!(
                    "state {} follows state {} on the remote: refusing to go backwards",
                    sig.counter,
                    counter
                );
            }
            counter = sig.counter;
            prev_keys = Some(keys.clone());
            last = Some((keys, sig));
        }
        let (keys, sig) = match last {
            Some(l) => {
                std::fs::write(self.store.join("last-keys.json"), serde_json::to_vec(&l.0)?)?;
                let mut l2 = self.local();
                l2.commit = Some(head.clone());
                l2.counter = l.1.counter;
                self.store_local(&l2)?;
                l
            }
            // nothing new since last time: re-read the head we already trust
            None => {
                let keys: Keys = prev_keys.context("no verified state and nothing new")?;
                let sig: Sig = serde_json::from_slice(
                    &self
                        .store_file_at(&head, "sig.json")?
                        .context("no sig.json")?,
                )?;
                (keys, sig)
            }
        };
        let (manifest, repo_key) = self.open_state(&head, &keys, &sig)?;
        Ok(Some((keys, manifest, repo_key)))
    }

    /// Fetch: load, then unpack every pack not yet applied into the local
    /// repository, and remember the counter.
    fn refresh(&self) -> Result<Option<(Keys, Manifest, crypto::RepoKey)>> {
        let Some((keys, manifest, repo_key)) = self.load()? else {
            return Ok(None);
        };
        let mut local = self.local();
        for p in &manifest.packs {
            if local.applied.contains(p) {
                continue;
            }
            let blob = self
                .store_file(&format!("packs/{p}"))?
                .with_context(|| format!("pack {p} missing"))?;
            let plain = crypto::decrypt(&repo_key, &blob)?;
            git_in(&self.git_dir, &["unpack-objects", "-q"], &plain)?;
            local.applied.push(p.clone());
        }
        local.counter = manifest.counter;
        self.store_local(&local)?;
        std::fs::write(
            self.store.join("last-keys.json"),
            serde_json::to_vec(&keys)?,
        )?;
        Ok(Some((keys, manifest, repo_key)))
    }

    /// Write a new state to the backing repository and push it.
    fn publish(
        &self,
        keys: &Keys,
        manifest: &Manifest,
        repo_key: &crypto::RepoKey,
        new_packs: &[(String, Vec<u8>)],
    ) -> Result<()> {
        let keys_raw = serde_json::to_vec_pretty(keys)?;
        let manifest_raw = crypto::encrypt(repo_key, &serde_json::to_vec(manifest)?)?;
        let mut sig = Sig {
            counter: manifest.counter,
            by: self.device.fingerprint(),
            keys_sha256: crypto::sha256_hex(&keys_raw),
            manifest_sha256: crypto::sha256_hex(&manifest_raw),
            signature: String::new(),
        };
        sig.signature = self.device.sign(&sig.message());
        // build the tree with plumbing: no working tree, no author config.
        // mktree takes one level, so packs/ is its own subtree.
        let parent = git(
            &self.store,
            &["rev-parse", "-q", "--verify", "refs/heads/dd"],
        )
        .ok()
        .map(|p| String::from_utf8_lossy(&p).trim().to_string());
        let existing = |path: &str| -> BTreeMap<String, String> {
            let Some(p) = &parent else {
                return BTreeMap::new();
            };
            git(&self.store, &["ls-tree", &format!("{p}:{path}")])
                .map(|b| {
                    String::from_utf8_lossy(&b)
                        .lines()
                        .filter_map(|l| {
                            let (meta, name) = l.split_once('\t')?;
                            Some((name.to_string(), meta.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let blob = |data: &[u8]| -> Result<String> {
            let oid = git_in(&self.store, &["hash-object", "-w", "--stdin"], data)?;
            Ok(format!(
                "100644 blob {}",
                String::from_utf8_lossy(&oid).trim()
            ))
        };
        let mut packs = existing("packs");
        for (id, data) in new_packs {
            packs.insert(id.clone(), blob(data)?);
        }
        let mktree = |entries: &BTreeMap<String, String>| -> Result<String> {
            let listing: String = entries.iter().map(|(n, m)| format!("{m}\t{n}\n")).collect();
            let t = git_in(&self.store, &["mktree"], listing.as_bytes())?;
            Ok(String::from_utf8_lossy(&t).trim().to_string())
        };
        let mut top = BTreeMap::new();
        top.insert("keys.json".to_string(), blob(&keys_raw)?);
        top.insert("manifest.enc".to_string(), blob(&manifest_raw)?);
        top.insert(
            "sig.json".to_string(),
            blob(&serde_json::to_vec_pretty(&sig)?)?,
        );
        if !packs.is_empty() {
            top.insert(
                "packs".to_string(),
                format!("040000 tree {}", mktree(&packs)?),
            );
        }
        let tree = mktree(&top)?;
        let msg = format!("state {}", manifest.counter);
        let mut args = vec!["commit-tree", &tree, "-m", &msg];
        if let Some(p) = &parent {
            args.extend(["-p", p]);
        }
        let commit = gitcmd(&self.store)
            .args(&args)
            .env("GIT_AUTHOR_NAME", "dd")
            .env("GIT_AUTHOR_EMAIL", "dd@localhost")
            .env("GIT_COMMITTER_NAME", "dd")
            .env("GIT_COMMITTER_EMAIL", "dd@localhost")
            .output()?;
        if !commit.status.success() {
            bail!("commit-tree failed");
        }
        let commit = String::from_utf8_lossy(&commit.stdout).trim().to_string();
        git(&self.store, &["update-ref", "refs/heads/dd", &commit])?;
        git(
            &self.store,
            &["push", "-q", "origin", "refs/heads/dd:refs/heads/dd"],
        )?;
        std::fs::write(self.store.join("last-keys.json"), &keys_raw)?;
        let mut local = self.local();
        local.counter = manifest.counter;
        local.commit = Some(commit);
        for (id, _) in new_packs {
            local.applied.push(id.clone());
        }
        self.store_local(&local)?;
        Ok(())
    }
}

fn main() -> Result<()> {
    match run() {
        // git closes our pipes the moment it has what it needs
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        r => r,
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // management, outside git: `git-remote-dd share <url> <device public key>`
    // and `git-remote-dd readers <url>`. dd repo wraps these.
    match args.get(1).map(String::as_str) {
        Some("share") => return share(&args[2..]),
        Some("unshare") => return unshare(&args[2..]),
        Some("readers") => return readers(&args[2..]),
        _ => {}
    }
    let url = args
        .get(2)
        .or(args.get(1))
        .context("usage: git-remote-dd <remote> <url>")?;
    let url = url.strip_prefix("dd::").unwrap_or(url).to_string();
    let remote = Remote::open(&url)?;

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut state: Option<(Keys, Manifest, crypto::RepoKey)> = None;
    let mut pushes: Vec<(bool, String, String)> = Vec::new();
    for line in stdin.lock().lines() {
        let line = line?;
        let mut words = line.split_whitespace();
        match words.next() {
            Some("capabilities") => {
                writeln!(out, "fetch\npush\n")?;
            }
            Some("list") => {
                state = remote.refresh()?;
                if let Some((_, m, _)) = &state {
                    for (r, sha) in &m.refs {
                        writeln!(out, "{sha} {r}")?;
                    }
                    if m.refs.contains_key("refs/heads/main") {
                        writeln!(out, "@refs/heads/main HEAD")?;
                    }
                }
                writeln!(out)?;
            }
            Some("fetch") => {
                // everything is already unpacked by `list`; drain the batch
            }
            Some("push") => {
                let spec = words.next().context("push without refspec")?;
                let (force, spec) = match spec.strip_prefix('+') {
                    Some(s) => (true, s),
                    None => (false, spec),
                };
                let (src, dst) = spec.split_once(':').context("bad refspec")?;
                pushes.push((force, src.to_string(), dst.to_string()));
            }
            Some("") | None => {
                if pushes.is_empty() {
                    // end of a fetch batch
                    writeln!(out)?;
                    continue;
                }
                let results = do_push(&remote, &mut state, std::mem::take(&mut pushes));
                match results {
                    Ok(lines) => {
                        for l in lines {
                            writeln!(out, "{l}")?;
                        }
                        writeln!(out)?;
                    }
                    Err(e) => {
                        eprintln!("dd: push failed: {e:#}");
                        std::process::exit(1);
                    }
                }
            }
            Some(other) => bail!("unsupported command {other}"),
        }
        out.flush()?;
    }
    Ok(())
}

fn do_push(
    remote: &Remote,
    state: &mut Option<(Keys, Manifest, crypto::RepoKey)>,
    pushes: Vec<(bool, String, String)>,
) -> Result<Vec<String>> {
    // first push to an empty remote: this device becomes the first reader
    // and signer, and the repository key is born here
    let (mut keys, mut manifest, repo_key) = match state.take().or(remote.refresh()?) {
        Some(s) => s,
        None => {
            let repo_key = crypto::new_repo_key();
            let keys = Keys {
                signers: vec![Reader {
                    fingerprint: remote.device.fingerprint(),
                    public_key: remote.device.public_b64(),
                    sealed_key: crypto::seal_to(&remote.device.public_b64(), &repo_key)?,
                }],
                repo: Some(remote.repo.clone()),
            };
            eprintln!(
                "dd: new encrypted repository; key held by device {} of {}",
                remote.device.fingerprint(),
                remote.user
            );
            (keys, Manifest::default(), repo_key)
        }
    };
    let mut results = Vec::new();
    let mut new_refs = manifest.refs.clone();
    let mut wanted: Vec<String> = Vec::new();
    for (force, src, dst) in &pushes {
        if src.is_empty() {
            new_refs.remove(dst);
            results.push(format!("ok {dst}"));
            continue;
        }
        let sha = String::from_utf8_lossy(&git(&remote.git_dir, &["rev-parse", "--verify", src])?)
            .trim()
            .to_string();
        if let Some(old) = manifest.refs.get(dst)
            && !force
            && old != &sha
            && git(&remote.git_dir, &["merge-base", "--is-ancestor", old, &sha]).is_err()
        {
            results.push(format!("error {dst} non-fast-forward"));
            continue;
        }
        new_refs.insert(dst.clone(), sha.clone());
        wanted.push(sha);
        results.push(format!("ok {dst}"));
    }
    // the objects the remote lacks: everything reachable from what we push,
    // minus everything reachable from what it already has
    let mut revs = String::new();
    for w in &wanted {
        revs.push_str(w);
        revs.push('\n');
    }
    for old in manifest.refs.values() {
        if git(&remote.git_dir, &["cat-file", "-e", old]).is_ok() {
            revs.push('^');
            revs.push_str(old);
            revs.push('\n');
        }
    }
    let mut new_packs = Vec::new();
    if !wanted.is_empty() {
        let pack = git_in(
            &remote.git_dir,
            &["pack-objects", "--revs", "--stdout", "-q"],
            revs.as_bytes(),
        )?;
        let id = crypto::sha256_hex(&pack)[..24].to_string();
        let blob = crypto::encrypt(&repo_key, &pack)?;
        manifest.packs.push(id.clone());
        new_packs.push((id, blob));
    }
    manifest.refs = new_refs;
    manifest.counter += 1;
    // a repository made before states named themselves names itself now
    keys.repo.get_or_insert_with(|| remote.repo.clone());
    remote.publish(&keys, &manifest, &repo_key, &new_packs)?;
    Ok(results)
}

/// Outside git there is no GIT_DIR; the store lives under the cache dir.
fn management_remote(url: &str) -> Result<Remote> {
    let cache = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .context("no HOME")?
        .join("dd")
        .join("repos");
    std::fs::create_dir_all(&cache)?;
    // SAFETY: single-threaded here; set before Remote::open reads it
    unsafe { std::env::set_var("GIT_DIR", &cache) };
    let url = url.strip_prefix("dd::").unwrap_or(url);
    Remote::open(url)
}

/// Seal the repository key to another device, and make it a signer too:
/// a reader who cannot push is not a collaborator. Only a current signer
/// can do this, since the new state has to carry a signature the others
/// accept.
fn share(args: &[String]) -> Result<()> {
    let (url, public_key) = match args {
        [u, p] => (u, p),
        _ => bail!("usage: git-remote-dd share <url> <device public key>"),
    };
    identity::decode_public(public_key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let remote = management_remote(url)?;
    let (mut keys, mut manifest, repo_key) = remote
        .load()?
        .context("nothing has been pushed to that remote yet")?;
    let fp = identity::fingerprint(public_key);
    if keys.signers.iter().any(|r| r.fingerprint == fp) {
        println!("device {fp} already has the key");
        return Ok(());
    }
    keys.signers.push(Reader {
        fingerprint: fp.clone(),
        public_key: public_key.clone(),
        sealed_key: crypto::seal_to(public_key, &repo_key)?,
    });
    manifest.counter += 1;
    remote.publish(&keys, &manifest, &repo_key, &[])?;
    println!(
        "device {fp} can now read and push; state {}",
        manifest.counter
    );
    Ok(())
}

/// Take a device off the repository: it can no longer push, and no longer
/// hand the key to anyone else.
///
/// What it does not do is take back what that device already has. The
/// repository key is sealed to it in every state published so far, and
/// those states are on a remote it can still fetch. Reading stops when the
/// key changes, and the key changes by making the repository again. The
/// command says so rather than letting the name imply otherwise.
fn unshare(args: &[String]) -> Result<()> {
    let (url, public_key) = match args {
        [u, p] => (u, p),
        _ => bail!("usage: git-remote-dd unshare <url> <device public key>"),
    };
    let remote = management_remote(url)?;
    let (mut keys, mut manifest, repo_key) = remote
        .load()?
        .context("nothing has been pushed to that remote yet")?;
    let fp = identity::fingerprint(public_key);
    let before = keys.signers.len();
    keys.signers.retain(|r| r.fingerprint != fp);
    if keys.signers.len() == before {
        bail!("device {fp} is not on this repository");
    }
    if keys.signers.is_empty() {
        bail!("that is the last device; it would leave the repository unreadable");
    }
    manifest.counter += 1;
    remote.publish(&keys, &manifest, &repo_key, &[])?;
    println!(
        "device {fp} can no longer push or share; state {}",
        manifest.counter
    );
    println!(
        "it can still read what is already published: the repository key is \n\
         sealed to it in every earlier state. Make the repository again to change that."
    );
    Ok(())
}

fn readers(args: &[String]) -> Result<()> {
    let [url] = args else {
        bail!("usage: git-remote-dd readers <url>");
    };
    let remote = management_remote(url)?;
    let (keys, manifest, _) = remote
        .load()?
        .context("nothing has been pushed to that remote yet")?;
    println!(
        "state {} - {} ref(s), {} pack(s)",
        manifest.counter,
        manifest.refs.len(),
        manifest.packs.len()
    );
    for r in &keys.signers {
        println!("  {}", r.fingerprint);
    }
    Ok(())
}
