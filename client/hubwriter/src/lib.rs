//! Writes a home controller image onto a removable disk: the one part of the
//! app's installer that runs with admin rights.
//!
//! NOTICE: the disk handling is ported from USBImager by bzt (MIT,
//! https://gitlab.com/bztsrc/usbimager) and Raspberry Pi Imager by
//! Raspberry Pi Ltd (Apache-2.0, https://github.com/raspberrypi/rpi-imager).

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub mod macos_plist;
mod zstd;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux as platform;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos as platform;
#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows as platform;

pub use zstd::ZstdStream;

const CHUNK: usize = 4 << 20;
const ALIGN: usize = 4096;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Disk {
    /// what `write` takes: `sdb`, `disk4`, `PhysicalDrive2`
    pub id: String,
    /// the OS's path for it
    pub name: String,
    pub model: String,
    /// usb, sd, mmc, sata, nvme, scsi, virtio, pcie or other
    pub bus: String,
    pub size_bytes: u64,
    pub removable: bool,
    pub mounted: Vec<String>,
    /// holds the running system: never listed, never written
    #[serde(skip)]
    pub system: bool,
    #[serde(skip)]
    pub sector: u32,
}

#[derive(Debug, PartialEq)]
pub enum Failure {
    /// nothing was written: exit 2
    Refused(String),
    /// the disk may hold part of the image: exit 3
    Failed(String),
    /// bad arguments or an unreadable image: exit 1
    Other(String),
}

impl Failure {
    pub fn code(&self) -> i32 {
        match self {
            Failure::Refused(_) => 2,
            Failure::Failed(_) => 3,
            Failure::Other(_) => 1,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Failure::Refused(m) | Failure::Failed(m) | Failure::Other(m) => m,
        }
    }
}

/// Why `disk` must not get an image of `image_len` bytes, if it must not.
pub fn refusal(disk: &Disk, image_len: u64) -> Option<String> {
    if disk.system {
        return Some(format!("{} holds the running system", disk.name));
    }
    if !disk.removable {
        return Some(format!("{} is not a removable disk", disk.name));
    }
    if disk.size_bytes == 0 {
        return Some(format!("{} has no medium", disk.name));
    }
    let padded = round_up(image_len, disk.sector.max(512) as u64);
    if padded > disk.size_bytes {
        return Some(format!(
            "the image ({image_len} bytes) is larger than {} ({} bytes)",
            disk.name, disk.size_bytes
        ));
    }
    None
}

/// Accepts `sdb`, `/dev/sdb`, `disk4`, `/dev/rdisk4`, `PhysicalDrive2` or
/// `\\.\PhysicalDrive2`, and gives the bare id.
pub fn normalize_id(id: &str) -> String {
    let id = id.strip_prefix(r"\\.\").unwrap_or(id);
    let id = id.strip_prefix("/dev/").unwrap_or(id);
    match id.strip_prefix("rdisk") {
        Some(n) => format!("disk{n}"),
        None => id.to_string(),
    }
}

pub fn find<'a>(disks: &'a [Disk], id: &str) -> Option<&'a Disk> {
    let id = normalize_id(id);
    disks.iter().find(|d| d.id.eq_ignore_ascii_case(&id))
}

pub fn parse_sha256(hex_str: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    hex::decode_to_slice(hex_str.trim(), &mut out).ok()?;
    Some(out)
}

fn round_up(n: u64, to: u64) -> u64 {
    n.div_ceil(to) * to
}

#[derive(Serialize)]
struct ProgressLine<'a> {
    phase: &'a str,
    done: u64,
    total: u64,
    bytes_per_sec: u64,
}

/// JSON lines for the app's progress bar: at most one each 250 ms, and
/// always the one that ends a phase.
pub struct Progress<'w> {
    out: Option<&'w mut dyn Write>,
    phase: &'static str,
    total: u64,
    start: Instant,
    last: Option<Instant>,
    ended: bool,
}

impl<'w> Progress<'w> {
    pub fn new(out: Option<&'w mut dyn Write>) -> Self {
        Progress {
            out,
            phase: "",
            total: 0,
            start: Instant::now(),
            last: None,
            ended: false,
        }
    }

    pub fn begin(&mut self, phase: &'static str, total: u64) {
        self.phase = phase;
        self.total = total;
        self.start = Instant::now();
        self.ended = false;
    }

    pub fn update(&mut self, done: u64) {
        let Some(out) = self.out.as_mut() else { return };
        let now = Instant::now();
        let end = done >= self.total;
        if self.ended
            || (!end
                && self
                    .last
                    .is_some_and(|l| now - l < Duration::from_millis(250)))
        {
            return;
        }
        let secs = (now - self.start).as_secs_f64();
        let line = ProgressLine {
            phase: self.phase,
            done,
            total: self.total,
            bytes_per_sec: if secs > 0.0 {
                (done as f64 / secs) as u64
            } else {
                0
            },
        };
        if let Ok(s) = serde_json::to_string(&line) {
            let _ = writeln!(out, "{s}");
            let _ = out.flush();
        }
        self.last = Some(now);
        self.ended = end;
    }
}

/// A buffer at a 4096-byte address, as unbuffered disk I/O on Windows and
/// O_DIRECT on Linux need.
struct Aligned {
    ptr: *mut u8,
    len: usize,
}

impl Aligned {
    fn new(len: usize) -> Self {
        let layout = Layout::from_size_align(len, ALIGN).expect("layout");
        // SAFETY: len is non-zero and the layout valid
        let ptr = unsafe { alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "out of memory");
        Aligned { ptr, len }
    }

    fn buf(&mut self) -> &mut [u8] {
        // SAFETY: ptr holds len initialised bytes, owned by self
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for Aligned {
    fn drop(&mut self) {
        // SAFETY: allocated in new with this layout
        unsafe { dealloc(self.ptr, Layout::from_size_align_unchecked(self.len, ALIGN)) }
    }
}

fn fill(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

struct Counting<R> {
    inner: R,
    hash: Sha256,
    read: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

fn is_zstd(file: &mut File) -> io::Result<bool> {
    let mut magic = [0u8; 4];
    let n = fill(file, &mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(n == 4 && magic == ZSTD_MAGIC)
}

/// What the check found: the bytes that go onto the disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub compressed: bool,
    pub len: u64,
    pub sha256: [u8; 32],
}

/// The image, opened once: it is checked and written through this one
/// handle, so whatever is at its path afterwards (another file, a link to
/// one only root may read) is never what this writes.
pub struct Image {
    file: File,
    name: String,
}

impl Image {
    pub fn open(path: &Path) -> Result<Image, Failure> {
        let file = File::open(path)
            .map_err(|e| Failure::Other(format!("cannot read {}: {e}", path.display())))?;
        Ok(Image {
            file,
            name: path.display().to_string(),
        })
    }
}

/// Hashes the image file as given and compares it with `expected`, before
/// anything is written. A zstd image is decompressed on the way, so its
/// length and the hash of what will be written are known up front.
pub fn check_image(
    image: &mut Image,
    expected: &[u8; 32],
    progress: &mut Progress,
) -> Result<Checked, Failure> {
    let name = image.name.clone();
    let open = |e: io::Error| Failure::Other(format!("cannot read {name}: {e}"));
    let mut file = image.file.try_clone().map_err(open)?;
    file.seek(SeekFrom::Start(0)).map_err(open)?;
    let file_len = file.metadata().map_err(open)?.len();
    let compressed = is_zstd(&mut file).map_err(open)?;
    progress.begin("hash", file_len);
    let mut src = BufReader::with_capacity(
        CHUNK,
        Counting {
            inner: file,
            hash: Sha256::new(),
            read: 0,
        },
    );
    let mut buf = vec![0u8; CHUNK];
    let mut out_hash = Sha256::new();
    let mut out_len = 0u64;
    let mut bad_stream = None;
    if compressed {
        let mut z = ZstdStream::new(&mut src);
        loop {
            match fill(&mut z, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    out_hash.update(&buf[..n]);
                    out_len += n as u64;
                    progress.update(z.get_ref().get_ref().read);
                }
                Err(e) => {
                    bad_stream = Some(e);
                    break;
                }
            }
        }
    }
    // the rest of the file, all of it when it is raw
    loop {
        let n = src.fill_buf().map_err(open)?.len();
        if n == 0 {
            break;
        }
        src.consume(n);
        progress.update(src.get_ref().read);
    }
    let counted = src.into_inner();
    progress.update(counted.read);
    let file_sha: [u8; 32] = counted.hash.finalize().into();
    if &file_sha != expected {
        return Err(Failure::Refused(format!(
            "the image's sha256 is {}, not {}",
            hex::encode(file_sha),
            hex::encode(expected)
        )));
    }
    if let Some(e) = bad_stream {
        return Err(Failure::Refused(format!(
            "the image is not valid zstd: {e}"
        )));
    }
    let (len, sha256) = if compressed {
        (out_len, out_hash.finalize().into())
    } else {
        (counted.read, file_sha)
    };
    Ok(Checked {
        compressed,
        len,
        sha256,
    })
}

/// Where the image goes: a raw disk, or a plain file in the tests.
pub trait Target: Read + Write + Seek {
    fn sync(&mut self) -> io::Result<()>;
}

impl Target for File {
    fn sync(&mut self) -> io::Result<()> {
        self.sync_all()
    }
}

/// Writes the checked image to `target` in sector-padded, aligned chunks,
/// syncs, then reads it back and compares.
pub fn write_image(
    image: &mut Image,
    checked: &Checked,
    target: &mut dyn Target,
    sector: u32,
    progress: &mut Progress,
) -> Result<(), Failure> {
    let sector = sector.max(512) as usize;
    let open = |e: io::Error| Failure::Failed(format!("cannot read {}: {e}", image.name));
    let mut file = image.file.try_clone().map_err(open)?;
    file.seek(SeekFrom::Start(0)).map_err(open)?;
    let mut src: Box<dyn Read> = if checked.compressed {
        Box::new(ZstdStream::new(BufReader::with_capacity(CHUNK, file)))
    } else {
        Box::new(file)
    };
    let failed = |what: &str, e: io::Error| Failure::Failed(format!("{what}: {e}"));
    let mut aligned = Aligned::new(CHUNK);
    let buf = aligned.buf();
    let mut hash = Sha256::new();
    let mut done = 0u64;
    progress.begin("write", checked.len);
    target
        .seek(SeekFrom::Start(0))
        .map_err(|e| failed("seek", e))?;
    loop {
        let n = fill(&mut src, buf).map_err(|e| failed("reading the image", e))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        done += n as u64;
        if done > checked.len {
            return Err(Failure::Failed("the image changed while writing".into()));
        }
        let padded = n.div_ceil(sector) * sector;
        buf[n..padded].fill(0);
        target
            .write_all(&buf[..padded])
            .map_err(|e| failed("writing", e))?;
        progress.update(done);
    }
    progress.update(done);
    let sha: [u8; 32] = hash.finalize().into();
    if done != checked.len || sha != checked.sha256 {
        return Err(Failure::Failed("the image changed while writing".into()));
    }
    target.flush().map_err(|e| failed("flushing", e))?;
    target.sync().map_err(|e| failed("syncing", e))?;

    progress.begin("verify", checked.len);
    target
        .seek(SeekFrom::Start(0))
        .map_err(|e| failed("seek", e))?;
    let mut hash = Sha256::new();
    let mut left = checked.len;
    while left > 0 {
        let want = (left as usize).min(CHUNK);
        let padded = want.div_ceil(sector) * sector;
        target
            .read_exact(&mut buf[..padded])
            .map_err(|e| failed("reading back", e))?;
        hash.update(&buf[..want]);
        left -= want as u64;
        progress.update(checked.len - left);
    }
    progress.update(checked.len);
    let back: [u8; 32] = hash.finalize().into();
    if back != checked.sha256 {
        return Err(Failure::Failed(format!(
            "the disk reads back {}, the image is {}",
            hex::encode(back),
            hex::encode(checked.sha256)
        )));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub mod platform {
    use super::{Disk, Failure, Target};

    pub fn list() -> std::io::Result<Vec<Disk>> {
        Ok(Vec::new())
    }

    pub struct Device;

    pub fn open(_: &Disk) -> Result<Device, Failure> {
        Err(Failure::Failed("not supported on this system".into()))
    }

    impl Device {
        pub fn target(&mut self) -> &mut dyn Target {
            unreachable!()
        }
        pub fn finish(self) -> Result<(), Failure> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn disk() -> Disk {
        Disk {
            id: "sdb".into(),
            name: "/dev/sdb".into(),
            model: "SD card".into(),
            bus: "usb".into(),
            size_bytes: 8 << 20,
            removable: true,
            mounted: vec![],
            system: false,
            sector: 512,
        }
    }

    fn image(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 + i / 4096) as u8).collect()
    }

    fn sha(b: &[u8]) -> [u8; 32] {
        Sha256::digest(b).into()
    }

    fn temp_with(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn refuses_system_fixed_empty_and_small() {
        assert_eq!(refusal(&disk(), 4 << 20), None);
        assert_eq!(refusal(&disk(), 8 << 20), None);
        let system = Disk {
            system: true,
            ..disk()
        };
        assert!(refusal(&system, 1).unwrap().contains("running system"));
        let fixed = Disk {
            removable: false,
            ..disk()
        };
        assert!(refusal(&fixed, 1).unwrap().contains("not a removable"));
        let empty = Disk {
            size_bytes: 0,
            ..disk()
        };
        assert!(refusal(&empty, 1).unwrap().contains("no medium"));
        assert!(refusal(&disk(), (8 << 20) + 1).unwrap().contains("larger"));
        // a removable system disk is still the system disk
        let both = Disk {
            system: true,
            removable: true,
            ..disk()
        };
        assert!(refusal(&both, 1).is_some());
    }

    #[test]
    fn padding_counts_against_the_size() {
        let d = Disk {
            size_bytes: 4096,
            sector: 4096,
            ..disk()
        };
        assert_eq!(refusal(&d, 4096), None);
        assert!(refusal(&d, 4097).is_some());
    }

    #[test]
    fn ids_normalize() {
        assert_eq!(normalize_id("/dev/sdb"), "sdb");
        assert_eq!(normalize_id("/dev/rdisk4"), "disk4");
        assert_eq!(normalize_id("disk4"), "disk4");
        assert_eq!(normalize_id(r"\\.\PhysicalDrive2"), "PhysicalDrive2");
        let disks = vec![
            disk(),
            Disk {
                id: "PhysicalDrive2".into(),
                ..disk()
            },
        ];
        assert_eq!(find(&disks, "/dev/sdb").unwrap().id, "sdb");
        assert_eq!(
            find(&disks, r"\\.\physicaldrive2").unwrap().id,
            "PhysicalDrive2"
        );
        assert!(find(&disks, "sdc").is_none());
    }

    #[test]
    fn sha_parses() {
        assert!(parse_sha256(&"ab".repeat(32)).is_some());
        assert!(parse_sha256(&"AB".repeat(32)).is_some());
        assert!(parse_sha256("abc").is_none());
        assert!(parse_sha256(&"zz".repeat(32)).is_none());
    }

    #[test]
    fn raw_image_writes_and_verifies() {
        // not a multiple of the sector, nor of the chunk
        let img = image(CHUNK + 1000);
        let f = temp_with(&img);
        let mut out = Vec::new();
        let mut target = tempfile::tempfile().unwrap();
        {
            let mut p = Progress::new(Some(&mut out));
            let mut im = Image::open(f.path()).unwrap();
            let c = check_image(&mut im, &sha(&img), &mut p).unwrap();
            assert_eq!(
                c,
                Checked {
                    compressed: false,
                    len: img.len() as u64,
                    sha256: sha(&img)
                }
            );
            write_image(&mut im, &c, &mut target, 512, &mut p).unwrap();
        }
        let mut written = Vec::new();
        target.seek(SeekFrom::Start(0)).unwrap();
        target.read_to_end(&mut written).unwrap();
        assert_eq!(written.len() % 512, 0);
        assert_eq!(&written[..img.len()], &img[..]);
        assert!(written[img.len()..].iter().all(|&b| b == 0));

        let lines: Vec<serde_json::Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for phase in ["hash", "write", "verify"] {
            let last = lines.iter().rfind(|l| l["phase"] == phase).unwrap();
            assert_eq!(last["done"], last["total"], "{phase}");
        }
        let verify = lines.iter().rfind(|l| l["phase"] == "verify").unwrap();
        assert_eq!(verify["total"], img.len() as u64);
        assert!(lines.iter().all(|l| l["bytes_per_sec"].is_u64()));
    }

    #[test]
    fn zstd_image_writes_decompressed() {
        let img = image(3 * CHUNK / 2);
        let z = ruzstd::encoding::compress_to_vec(
            Cursor::new(&img),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let f = temp_with(&z);
        let mut p = Progress::new(None);
        // the sha256 is of the file as given, the compressed bytes
        assert!(matches!(
            check_image(&mut Image::open(f.path()).unwrap(), &sha(&img), &mut p),
            Err(Failure::Refused(_))
        ));
        let mut im = Image::open(f.path()).unwrap();
        let c = check_image(&mut im, &sha(&z), &mut p).unwrap();
        assert!(c.compressed);
        assert_eq!(c.len, img.len() as u64);
        assert_eq!(c.sha256, sha(&img));
        let mut target = tempfile::tempfile().unwrap();
        write_image(&mut im, &c, &mut target, 512, &mut p).unwrap();
        let mut written = Vec::new();
        target.seek(SeekFrom::Start(0)).unwrap();
        target.read_to_end(&mut written).unwrap();
        assert_eq!(&written[..img.len()], &img[..]);
    }

    #[test]
    fn two_zstd_frames_and_a_skippable_one() {
        let a = image(10_000);
        let b = image(20_000);
        let level = ruzstd::encoding::CompressionLevel::Fastest;
        let mut z = ruzstd::encoding::compress_to_vec(Cursor::new(&a), level);
        z.extend_from_slice(&[0x50, 0x2a, 0x4d, 0x18, 3, 0, 0, 0, 1, 2, 3]);
        z.extend(ruzstd::encoding::compress_to_vec(Cursor::new(&b), level));
        let mut out = Vec::new();
        ZstdStream::new(Cursor::new(z))
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, [a, b].concat());
    }

    #[test]
    fn corrupt_zstd_is_refused_after_the_hash() {
        let img = image(100_000);
        let mut z = ruzstd::encoding::compress_to_vec(
            Cursor::new(&img),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        z.truncate(z.len() / 2);
        let f = temp_with(&z);
        let mut p = Progress::new(None);
        let err = check_image(&mut Image::open(f.path()).unwrap(), &sha(&z), &mut p).unwrap_err();
        assert!(err.message().contains("not valid zstd"), "{err:?}");
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn hash_mismatch_refuses_before_writing() {
        let img = image(5000);
        let f = temp_with(&img);
        let mut p = Progress::new(None);
        let err = check_image(&mut Image::open(f.path()).unwrap(), &[0u8; 32], &mut p).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.message().contains(&hex::encode(sha(&img))));
    }

    #[test]
    fn missing_image_is_not_a_refusal() {
        let err = Image::open(Path::new("/nonexistent/x.img")).err().unwrap();
        assert_eq!(err.code(), 1);
    }

    #[test]
    fn image_changed_after_the_check_fails() {
        let img = image(5000);
        let f = temp_with(&img);
        let mut p = Progress::new(None);
        let mut im = Image::open(f.path()).unwrap();
        let c = check_image(&mut im, &sha(&img), &mut p).unwrap();
        std::fs::write(f.path(), image(6000)).unwrap();
        let mut target = tempfile::tempfile().unwrap();
        let err = write_image(&mut im, &c, &mut target, 512, &mut p).unwrap_err();
        assert_eq!(err.code(), 3);
    }

    #[test]
    fn another_file_put_at_the_path_is_never_read() {
        let img = image(5000);
        let f = temp_with(&img);
        let mut p = Progress::new(None);
        let mut im = Image::open(f.path()).unwrap();
        let c = check_image(&mut im, &sha(&img), &mut p).unwrap();
        // the path now names something else entirely
        let secret = temp_with(b"only root reads this");
        std::fs::remove_file(f.path()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(secret.path(), f.path()).unwrap();
        let mut target = tempfile::tempfile().unwrap();
        write_image(&mut im, &c, &mut target, 512, &mut p).unwrap();
        let mut written = Vec::new();
        target.seek(SeekFrom::Start(0)).unwrap();
        target.read_to_end(&mut written).unwrap();
        assert_eq!(&written[..img.len()], &img[..]);
    }

    struct Flaky(Cursor<Vec<u8>>);

    impl Read for Flaky {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let at = self.0.position();
            let n = self.0.read(buf)?;
            if at == 0 && n > 100 {
                buf[100] ^= 1;
            }
            Ok(n)
        }
    }

    impl Write for Flaky {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for Flaky {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.0.seek(pos)
        }
    }

    impl Target for Flaky {
        fn sync(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn read_back_difference_fails() {
        let img = image(8192);
        let f = temp_with(&img);
        let mut p = Progress::new(None);
        let mut im = Image::open(f.path()).unwrap();
        let c = check_image(&mut im, &sha(&img), &mut p).unwrap();
        let mut target = Flaky(Cursor::new(Vec::new()));
        let err = write_image(&mut im, &c, &mut target, 512, &mut p).unwrap_err();
        assert_eq!(err.code(), 3);
        assert!(err.message().contains("reads back"));
    }

    #[test]
    fn progress_is_rate_limited() {
        let mut out = Vec::new();
        {
            let mut p = Progress::new(Some(&mut out));
            p.begin("write", 1000);
            for i in 0..=1000 {
                p.update(i);
            }
            p.update(1000);
        }
        let s = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        // the first update and the final one, nothing between in a tight
        // loop, and nothing after the end
        assert_eq!(lines.len(), 2, "{s}");
        assert!(lines[1].contains("\"done\":1000"));
    }
}
