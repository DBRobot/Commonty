//! Encrypted archives of old computers, in restic's repository format, stored
//! in a per-user directory behind the verifier-guarded webdav on a box.
//!
//! What is ours here is only the plumbing: the place the repository lives and
//! the token that gets it there. Encryption, chunking, deduplication and
//! integrity are restic's - `rustic_core` is the Rust implementation of that
//! format, so anything that reads restic repositories reads these.
//!
//! No terminal, no prompts, no config files: a frontend supplies a password
//! and a source and gets progress events back.

mod backend;
mod progress;
mod token;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use auth::KeyStore;
use rustic_core::repofile::SnapshotFile;
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, KeyOptions, LsOptions, PathList, ProgressBars,
    PruneOptions, RepairIndexOptions, Repository, RepositoryBackends, RepositoryOptions,
    SnapshotOptions,
};
use url::Url;
use zeroize::Zeroizing;

pub use progress::Stderr;
pub use token::TokenProvider;

pub struct Archive<K: KeyStore, P: ProgressBars> {
    backends: RepositoryBackends,
    password: Zeroizing<String>,
    progress: P,
    _keys: std::marker::PhantomData<K>,
}

/// What to archive.
pub enum Source {
    /// A regular file - an image somebody already made.
    File(PathBuf),
    /// Whatever arrives on stdin. This is how a block device gets in:
    /// `sudo cat /dev/sdX | dd image push - --name old-laptop`, so that only
    /// `cat` runs as root and the keyring stays the user's.
    Stdin,
}

pub struct Entry {
    pub id: String,
    pub name: String,
    pub time: String,
    pub bytes: u64,
}

impl<K: KeyStore + Send + Sync + 'static, P: ProgressBars + Clone> Archive<K, P> {
    /// `repo` is the user's directory on the server, e.g.
    /// `https://files.example/images/`. The username is not in the url: nginx
    /// derives it from the token, so nobody can name somebody else's.
    pub fn new(
        repo: Url,
        tokens: Arc<TokenProvider<K>>,
        password: Zeroizing<String>,
        progress: P,
    ) -> Result<Self> {
        let be = backend::Webdav::new(repo, tokens)?;
        Ok(Self {
            backends: RepositoryBackends::new(Arc::new(be), None),
            password,
            progress,
            _keys: std::marker::PhantomData,
        })
    }

    fn repo(&self) -> Result<Repository<()>> {
        Ok(Repository::new_with_progress(
            &RepositoryOptions::default(),
            &self.backends,
            self.progress.clone(),
        )?)
    }

    fn credentials(&self) -> Credentials {
        Credentials::Password(self.password.to_string())
    }

    /// Whether a repository has been created here yet.
    pub fn exists(&self) -> Result<bool> {
        Ok(self.repo()?.config_id()?.is_some())
    }

    /// Create the repository. Once per user; `push` does it if it is missing.
    pub fn init(&self) -> Result<()> {
        self.repo()?
            .init(
                &self.credentials(),
                &KeyOptions::default(),
                &ConfigOptions::default(),
            )
            .context("creating the repository")?;
        Ok(())
    }

    /// Archive `source` as a snapshot named `name`. Interrupted and run again,
    /// restic's dedup means every block already uploaded is skipped.
    pub fn push(&self, source: Source, name: &str) -> Result<Entry> {
        // First push creates the repository. A person who signed up and then
        // ran `dd image push` should not have to know that step exists.
        if !self.exists()? {
            self.init()?;
        }
        let repo = self
            .repo()?
            .open(&self.credentials())
            .context("opening the repository - wrong password?")?
            .to_indexed_ids()?;

        let (paths, opts) = match source {
            Source::File(p) => (
                PathList::from_iter([p]),
                BackupOptions::default().as_path(Some(PathBuf::from(format!("/{name}")))),
            ),
            Source::Stdin => (
                PathList::from_string("-")?,
                BackupOptions::default().stdin_filename(name.to_string()),
            ),
        };
        let snap = SnapshotFile::from_options(&SnapshotOptions::default().label(name.to_string()))?;
        let snap = repo.backup(&opts, &paths, snap).context("backup")?;
        Ok(entry(&snap))
    }

    /// Rebuild the index from the packs that are actually on the server. An
    /// interrupted push leaves its uploaded packs unindexed, and a rerun would
    /// upload all of them again; after this it deduplicates against them.
    pub fn repair(&self) -> Result<()> {
        let repo = self.repo()?.open(&self.credentials())?;
        repo.repair_index(&RepairIndexOptions::default(), false)
            .context("repairing the index")?;
        Ok(())
    }

    /// Forget the archives whose ids start with these, and give their space
    /// back: whatever no other archive shares is removed from the server.
    /// A prefix that names no archive, or more than one, stops it before
    /// anything is touched.
    pub fn delete(&self, prefixes: &[String]) -> Result<Vec<Entry>> {
        let repo = self.repo()?.open(&self.credentials())?;
        let snaps = repo.get_all_snapshots()?;
        let mut gone = Vec::new();
        for p in prefixes {
            let hits: Vec<_> = snaps
                .iter()
                .filter(|s| s.id.to_string().starts_with(p.as_str()))
                .collect();
            anyhow::ensure!(
                hits.len() == 1,
                "{p}: names {} archives, not one",
                hits.len()
            );
            gone.push(hits[0]);
        }
        let ids: Vec<_> = gone.iter().map(|s| s.id).collect();
        repo.delete_snapshots(&ids)
            .context("forgetting the archives")?;
        let opts = PruneOptions::default();
        let plan = repo.prune_plan(&opts).context("planning what to remove")?;
        repo.prune(&opts, plan)
            .context("removing what nothing uses")?;
        Ok(gone.into_iter().map(entry).collect())
    }

    pub fn list(&self) -> Result<Vec<Entry>> {
        let repo = self.repo()?.open(&self.credentials())?;
        let mut snaps = repo.get_all_snapshots()?;
        snaps.sort_by(|a, b| a.time.cmp(&b.time));
        Ok(snaps.iter().map(entry).collect())
    }

    /// Stream one archived file back out. `snapshot` is an id prefix or
    /// "latest"; `name` is what it was pushed as. The tree is walked for the
    /// name rather than addressed by path, because where rustic places a
    /// single file depends on whether it came from a path or from stdin.
    pub fn pull(&self, snapshot: &str, name: &str, out: &mut impl Write) -> Result<()> {
        let repo = self.repo()?.open(&self.credentials())?.to_indexed()?;
        let root = repo.node_from_snapshot_path(snapshot, |_| true)?;
        let files: Vec<_> = repo
            .ls(&root, &LsOptions::default().recursive(true))?
            .filter_map(Result::ok)
            .map(|(_, node)| node)
            .filter(|n| n.is_file())
            .collect();
        // by name if it was pushed from stdin; otherwise a push is one file
        // and rustic kept its original filename, so there is exactly one
        let node = files
            .iter()
            .find(|n| n.name().to_string_lossy() == name)
            .or_else(|| (files.len() == 1).then(|| &files[0]))
            .with_context(|| format!("no `{name}` in snapshot {snapshot}"))?;
        repo.dump(node, out).context("restoring")?;
        Ok(())
    }
}

fn entry(s: &SnapshotFile) -> Entry {
    Entry {
        id: s.id.to_string(),
        name: s.label.clone(),
        time: s.time.to_string(),
        bytes: s
            .summary
            .as_ref()
            .map(|x| x.total_bytes_processed)
            .unwrap_or(0),
    }
}
