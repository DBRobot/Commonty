use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use crate::{Disk, Failure, Target};

/// A disk holding any of these is the running system's.
const SYSTEM_MOUNTS: &[&str] = &[
    "/",
    "/boot",
    "/boot/efi",
    "/efi",
    "/usr",
    "/nix",
    "/nix/store",
    "/var",
    "/home",
];

#[derive(Debug, PartialEq)]
pub struct Mount {
    pub dev: String,
    pub point: String,
    pub source: String,
}

/// mountinfo escapes space, tab, newline and backslash as \ooo.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let left: Vec<&str> = left.split(' ').collect();
            let right: Vec<&str> = right.split(' ').collect();
            Some(Mount {
                dev: left.get(2)?.to_string(),
                point: unescape(left.get(4)?),
                source: unescape(right.get(1)?),
            })
        })
        .collect()
}

fn read_trim(p: impl AsRef<Path>) -> String {
    fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// The whole disks under a block device: itself, its parent when it is a
/// partition, and its slaves' disks when it is dm or md.
fn whole_disks(sys: &Path, out: &mut BTreeSet<String>, depth: u32) {
    let Ok(sys) = fs::canonicalize(sys) else {
        return;
    };
    if depth > 8 {
        return;
    }
    if sys.join("partition").exists() {
        if let Some(n) = sys.parent().and_then(Path::file_name) {
            out.insert(n.to_string_lossy().into_owned());
        }
        return;
    }
    let slaves: Vec<PathBuf> = fs::read_dir(sys.join("slaves"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    if slaves.is_empty()
        && let Some(n) = sys.file_name()
    {
        out.insert(n.to_string_lossy().into_owned());
    }
    for s in slaves {
        whole_disks(&s, out, depth + 1);
    }
}

fn disks_of(m: &Mount) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let by_dev = PathBuf::from(format!("/sys/dev/block/{}", m.dev));
    if by_dev.exists() {
        whole_disks(&by_dev, &mut out, 0);
    } else if m.source.starts_with("/dev/") {
        // btrfs and friends report an anonymous device number
        if let Some(name) = fs::canonicalize(&m.source)
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        {
            whole_disks(&Path::new("/sys/class/block").join(name), &mut out, 0);
        }
    }
    out
}

fn mounts() -> Vec<Mount> {
    parse_mountinfo(&fs::read_to_string("/proc/self/mountinfo").unwrap_or_default())
}

/// Bus and removability from what sysfs says about one disk.
pub fn classify(
    name: &str,
    removable_flag: &str,
    devpath: &str,
    mmc_type: &str,
) -> (&'static str, bool) {
    if name.starts_with("mmcblk") {
        // eMMC soldered to the board says MMC; a card in a slot says SD
        return if mmc_type == "SD" {
            ("sd", true)
        } else {
            ("mmc", false)
        };
    }
    let flag = removable_flag == "1";
    if devpath.contains("/usb") {
        // some USB card readers and SATA bridges do not set the flag
        return ("usb", true);
    }
    let bus = if name.starts_with("nvme") {
        "nvme"
    } else if name.starts_with("vd") {
        "virtio"
    } else if devpath.contains("/ata") {
        "sata"
    } else if name.starts_with("sd") {
        "scsi"
    } else {
        "other"
    };
    (bus, flag)
}

fn skipped(name: &str) -> bool {
    ["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd", "zd"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// Every disk in /sys/block but loop, ram, dm and read-only ones.
pub fn list() -> io::Result<Vec<Disk>> {
    let mounts = mounts();
    let mut mounted: Vec<(BTreeSet<String>, &Mount)> =
        mounts.iter().map(|m| (disks_of(m), m)).collect();
    mounted.retain(|(d, _)| !d.is_empty());
    let mut disks = Vec::new();
    for e in fs::read_dir("/sys/block")?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if skipped(&name) {
            continue;
        }
        let sys = e.path();
        if read_trim(sys.join("ro")) == "1" {
            continue;
        }
        let devpath = fs::canonicalize(&sys)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (bus, removable) = classify(
            &name,
            &read_trim(sys.join("removable")),
            &devpath,
            &read_trim(sys.join("device/type")),
        );
        let model = if name.starts_with("mmcblk") {
            read_trim(sys.join("device/name"))
        } else {
            let v = read_trim(sys.join("device/vendor"));
            let m = read_trim(sys.join("device/model"));
            format!("{v} {m}").trim().to_string()
        };
        let size_bytes = read_trim(sys.join("size")).parse::<u64>().unwrap_or(0) * 512;
        let sector = read_trim(sys.join("queue/logical_block_size"))
            .parse()
            .unwrap_or(512);
        let on: Vec<&Mount> = mounted
            .iter()
            .filter(|(d, _)| d.contains(&name))
            .map(|(_, m)| *m)
            .collect();
        disks.push(Disk {
            id: name.clone(),
            name: format!("/dev/{name}"),
            model,
            bus: bus.to_string(),
            size_bytes,
            removable,
            mounted: on.iter().map(|m| m.point.clone()).collect(),
            system: on.iter().any(|m| SYSTEM_MOUNTS.contains(&m.point.as_str())),
            sector,
        });
    }
    disks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(disks)
}

fn unmount(point: &str) -> io::Result<()> {
    let c = CString::new(point).map_err(io::Error::other)?;
    // SAFETY: a valid C string
    if unsafe { libc::umount2(c.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    // busy: detach it, then the exclusive open below waits until it is gone
    if unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) } == 0 {
        return Ok(());
    }
    Err(io::Error::last_os_error())
}

pub struct Device {
    file: File,
}

pub fn open(disk: &Disk) -> Result<Device, Failure> {
    let mut on: Vec<Mount> = mounts()
        .into_iter()
        .filter(|m| disks_of(m).contains(&disk.id))
        .collect();
    // the deepest first, so nothing is mounted on what is unmounted
    on.sort_by_key(|m| std::cmp::Reverse(m.point.len()));
    for m in &on {
        if SYSTEM_MOUNTS.contains(&m.point.as_str()) {
            return Err(Failure::Refused(format!("{} holds {}", disk.name, m.point)));
        }
        unmount(&m.point)
            .map_err(|e| Failure::Failed(format!("cannot unmount {}: {e}", m.point)))?;
    }
    let mut tries = 0;
    let file = loop {
        // O_EXCL on a block device fails while anything still mounts it;
        // O_DIRECT so the read-back reads the card, not the page cache
        match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_EXCL | libc::O_DIRECT | libc::O_SYNC)
            .open(&disk.name)
        {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) && tries < 20 => {
                tries += 1;
                sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(Failure::Failed(format!("cannot open {}: {e}", disk.name))),
        }
    };
    Ok(Device { file })
}

impl Device {
    pub fn target(&mut self) -> &mut dyn Target {
        &mut self.file
    }

    /// Syncs and has the kernel read the new partition table, so the app can
    /// mount the boot partition for the pairing key.
    pub fn finish(self) -> Result<(), Failure> {
        self.file
            .sync_all()
            .map_err(|e| Failure::Failed(format!("syncing: {e}")))?;
        const BLKRRPART: libc::c_ulong = 0x125f;
        // SAFETY: a block device descriptor; failure only means a stale table
        unsafe { libc::ioctl(self.file.as_raw_fd(), BLKRRPART as _) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_parses_and_unescapes() {
        let text = "\
26 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
92 26 8:17 / /media/david/boot\\040fs rw,nosuid master:2 - vfat /dev/sdb1 rw,uid=1000
93 26 0:45 / /home rw shared:3 - btrfs /dev/mapper/home rw
bad line";
        let m = parse_mountinfo(text);
        assert_eq!(m.len(), 3);
        assert_eq!(
            m[1],
            Mount {
                dev: "8:17".into(),
                point: "/media/david/boot fs".into(),
                source: "/dev/sdb1".into()
            }
        );
        assert_eq!(m[2].dev, "0:45");
        assert_eq!(unescape("a\\134b\\011"), "a\\b\t");
        assert_eq!(unescape("end\\04"), "end\\04");
    }

    #[test]
    fn classifies_buses() {
        assert_eq!(
            classify("mmcblk0", "0", "/sys/devices/platform/mmc", "SD"),
            ("sd", true)
        );
        assert_eq!(
            classify("mmcblk0", "0", "/sys/devices/platform/mmc", "MMC"),
            ("mmc", false)
        );
        assert_eq!(
            classify(
                "sdb",
                "0",
                "/sys/devices/pci0000:00/0000:00:14.0/usb2/2-1/host6",
                ""
            ),
            ("usb", true)
        );
        assert_eq!(
            classify("sda", "0", "/sys/devices/pci0000:00/ata1/host0", ""),
            ("sata", false)
        );
        assert_eq!(
            classify("sdc", "1", "/sys/devices/pci0000:00/ata3/host2", ""),
            ("sata", true)
        );
        assert_eq!(
            classify("nvme0n1", "0", "/sys/devices/pci0000:00/nvme", ""),
            ("nvme", false)
        );
        assert!(skipped("loop0") && skipped("dm-1") && skipped("zram0") && !skipped("sdb"));
    }

    #[test]
    fn this_machine_lists_and_its_root_disk_is_refused() {
        // a build sandbox has no /sys/block, or no disks in it: nothing to see
        let Ok(disks) = list() else {
            return;
        };
        for d in disks.iter().filter(|d| d.system) {
            assert!(crate::refusal(d, 1).is_some(), "{d:?}");
        }
    }
}
