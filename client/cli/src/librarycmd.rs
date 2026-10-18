//! `dd library`: a member's encrypted libraries from the terminal. The key
//! comes out of the entry (sealed to this device or the root), every byte
//! is encrypted here before it goes anywhere, and the box's gate only ever
//! speaks WebDAV over the library's prefix (client/media/gate.rs), in
//! rclone's format. `dd media` mounts the same libraries with rclone; this
//! is the plain path in and out.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use library::Library;
pub use media::gate::{Gate, Opener, files_base, openable};

use crate::who;

#[derive(Subcommand)]
pub enum LibraryCmd {
    /// Make a new library: a key sealed to every key of yours, in your entry
    New,
    /// The libraries you can open
    List,
    /// A library's key as rclone wants it (`password`; the id is
    /// `password2`): yours, on your machine, for an `rclone config` by hand
    Key { library: String },
    /// What a library holds
    Ls { library: String },
    /// A file into a library, under a name (default: the file's name); `-` reads stdin
    Put {
        library: String,
        file: PathBuf,
        #[arg(long = "as")]
        name: Option<String>,
    },
    /// A file out of a library, by name
    Get {
        library: String,
        name: String,
        out: PathBuf,
    },
    /// Make a folder (the pages and the mount expect Files, Movies and Shows)
    Mkdir { library: String, path: String },
    /// Move a file to the trash (nothing is gone until the box purges it)
    Trash { library: String, name: String },
    /// Let another member open one of your libraries: its key sealed to
    /// their devices, in your entry (needs the root). With --write they may
    /// change what is in it too, but not delete anything for good.
    Share {
        library: String,
        member: String,
        #[arg(long)]
        write: bool,
    },
    /// Stop sharing one of your libraries with a member (needs the root).
    /// What they already copied off it stays theirs.
    Unshare { library: String, member: String },
    /// Seal every library key again to your devices, the root and the
    /// paper key alone, and drop what was sealed to a passkey (needs the
    /// root). The app does it at launch; once is enough.
    Reseal,
}

pub async fn run(cmd: LibraryCmd, keys: &auth::Store, dirs: &[String]) -> Result<()> {
    let (opener, user, token) = Opener::load(keys)?;
    let base = files_base(dirs)?;
    match cmd {
        LibraryCmd::New => {
            let root = opener
                .root
                .as_ref()
                .context("the root key is not on this machine: a library is made where it is")?;
            let cur = who::ours(dirs, &user, root).await?;
            let key = library::random_key();
            let lib = Library {
                id: library::random_id(),
                keys: library::seal_for_entry(&cur.entry, &key)?,
                readers: vec![],
                created: identity::now(),
            };
            let id = lib.id.clone();
            let sealed = lib.keys.len();
            who::add_library(dirs, &user, root, lib).await?;
            println!("library {id}: sealed to {sealed} of your keys, in your entry");
            // the three the pages and the mount expect; a library with
            // none of them opens empty everywhere and looks broken
            let gate = Gate::new(&base, &id, &token, &key);
            for folder in ["Files", "Movies", "Shows"] {
                gate.mkdir(folder).await?;
            }
            println!("  with Files, Movies and Shows in it");
        }
        LibraryCmd::Share {
            library: id,
            member,
            write,
        } => {
            let root = opener
                .root
                .as_ref()
                .context("the root key is not on this machine: share from the one that holds it")?;
            let list = account::released(keys, dirs).await?;
            let (lib, key) = pick(keys, dirs, &user, &opener, &id).await?;
            let mut readers: Vec<(String, bool)> = lib
                .readers
                .iter()
                .filter(|r| r.name != member)
                .map(|r| (r.name.clone(), r.writes))
                .collect();
            readers.push((member.clone(), write));
            account::set_readers(dirs, &user, root, &id, &key, &readers, &list).await?;
            println!(
                "{id}: {member} may {} it",
                if write { "open and change" } else { "open" }
            );
        }
        LibraryCmd::Unshare {
            library: id,
            member,
        } => {
            let root = opener
                .root
                .as_ref()
                .context("the root key is not on this machine: run this where it is")?;
            let list = account::released(keys, dirs).await?;
            let (lib, key) = pick(keys, dirs, &user, &opener, &id).await?;
            if !lib.readers.iter().any(|r| r.name == member) {
                bail!("{id} is not shared with {member}");
            }
            let readers: Vec<(String, bool)> = lib
                .readers
                .iter()
                .filter(|r| r.name != member)
                .map(|r| (r.name.clone(), r.writes))
                .collect();
            account::set_readers(dirs, &user, root, &id, &key, &readers, &list).await?;
            println!("{id}: no longer shared with {member}");
        }
        LibraryCmd::Reseal => {
            let root = opener
                .root
                .as_ref()
                .context("the root key is not on this machine: run this where it is")?;
            match account::reseal(dirs, &user, root).await? {
                Some(s) => println!(
                    "your libraries are sealed to your devices, root and paper key alone; version {}",
                    s.entry.version
                ),
                None => println!("nothing to do: no library is sealed to a passkey"),
            }
        }
        LibraryCmd::List => {
            let list = released(keys, dirs).await;
            let libs = openable(dirs, &user, &opener, list.as_ref()).await?;
            if libs.is_empty() {
                println!("no libraries - `dd library new` makes one");
            }
            for (owner, lib, _) in libs {
                let whose = if owner == user {
                    "yours".to_string()
                } else {
                    format!("{owner}'s")
                };
                println!("{}  {whose}  {} key(s)", lib.id, lib.keys.len());
            }
        }
        LibraryCmd::Key { library } => {
            let (_, key) = pick(keys, dirs, &user, &opener, &library).await?;
            println!("{}", library::crypt::password_of(&key));
        }
        LibraryCmd::Ls { library } => {
            let (_, key) = pick(keys, dirs, &user, &opener, &library).await?;
            let gate = Gate::new(&base, &library, &token, &key);
            for it in gate.walk("").await? {
                if it.dir {
                    println!("{:>12}  {}/", "-", it.path);
                } else {
                    println!("{:>12}  {}", it.size, it.path);
                }
            }
        }
        LibraryCmd::Put {
            library,
            file,
            name,
        } => {
            let (_, key) = pick(keys, dirs, &user, &opener, &library).await?;
            let gate = Gate::new(&base, &library, &token, &key);
            let name = name.unwrap_or_else(|| {
                file.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            if name.is_empty() {
                bail!("a file from stdin needs a name: --as <name>");
            }
            // a file streams a block at a time; stdin has no length, so it
            // is read whole first (a pipe is for small things)
            let (src, len): (Box<dyn std::io::Read + Send>, u64) = if file.as_os_str() == "-" {
                let mut all = Vec::new();
                std::io::Read::read_to_end(&mut std::io::stdin(), &mut all)?;
                let len = all.len() as u64;
                (Box::new(std::io::Cursor::new(all)), len)
            } else {
                let f = std::fs::File::open(&file)
                    .with_context(|| format!("opening {}", file.display()))?;
                let len = f.metadata()?.len();
                (Box::new(f), len)
            };
            let shown = name.clone();
            gate.put(&name, src, len, move |done| {
                eprint!("\r{shown}: {} MiB", done >> 20);
            })
            .await?;
            eprintln!();
            println!("{name}: {len} bytes");
        }
        LibraryCmd::Get { library, name, out } => {
            let (_, key) = pick(keys, dirs, &user, &opener, &library).await?;
            let gate = Gate::new(&base, &library, &token, &key);
            let f = std::fs::File::create(&out)?;
            let shown = name.clone();
            let n = gate
                .get(&name, std::io::BufWriter::new(f), move |done| {
                    eprint!("\r{shown}: {} MiB", done >> 20);
                })
                .await?;
            eprintln!();
            println!("{name}: {n} bytes -> {}", out.display());
        }
        LibraryCmd::Mkdir { library, path } => {
            let (_, key) = pick(keys, dirs, &user, &opener, &library).await?;
            let gate = Gate::new(&base, &library, &token, &key);
            gate.mkdir(&path).await?;
            println!("{path}: made");
        }
        LibraryCmd::Trash { library, name } => {
            let (_, key) = pick(keys, dirs, &user, &opener, &library).await?;
            let gate = Gate::new(&base, &library, &token, &key);
            gate.trash(&name).await?;
            println!("{name}: in the trash; nothing is gone until the box purges");
        }
    }
    Ok(())
}

/// The release's member list, which a library shared with this member is
/// held to; without it, a word on why and this member's own alone.
pub async fn released(keys: &auth::Store, dirs: &[String]) -> Option<identity::MemberList> {
    match account::released(keys, dirs).await {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!("libraries shared with you are not shown: {e:#}");
            None
        }
    }
}

async fn pick(
    keys: &auth::Store,
    dirs: &[String],
    user: &str,
    opener: &Opener,
    library: &str,
) -> Result<(Library, library::Key)> {
    let list = released(keys, dirs).await;
    openable(dirs, user, opener, list.as_ref())
        .await?
        .into_iter()
        .find(|(_, l, _)| l.id == library)
        .map(|(_, l, k)| (l, k))
        .with_context(|| format!("no library {library} that this machine can open"))
}
