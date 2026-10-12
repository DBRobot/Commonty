use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;

use crate::macos_plist::{parse_info, parse_list};
use crate::{Disk, Failure, Target};

fn diskutil(args: &[&str]) -> io::Result<Vec<u8>> {
    let out = Command::new("/usr/sbin/diskutil").args(args).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "diskutil {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Every physical whole disk, removable or not.
pub fn list() -> io::Result<Vec<Disk>> {
    let all = parse_list(&diskutil(&["list", "-plist"])?).map_err(io::Error::other)?;
    let physical = parse_list(&diskutil(&["list", "-plist", "physical"])?)
        .map_err(io::Error::other)?
        .whole_disks;
    let mut disks = Vec::new();
    for id in physical {
        let info = diskutil(&["info", "-plist", &id])?;
        disks.push(parse_info(&id, &info, &all).map_err(io::Error::other)?);
    }
    Ok(disks)
}

pub struct Device {
    file: File,
    id: String,
}

pub fn open(disk: &Disk) -> Result<Device, Failure> {
    let node = format!("/dev/{}", disk.id);
    if diskutil(&["unmountDisk", &node]).is_err() {
        diskutil(&["unmountDisk", "force", &node])
            .map_err(|e| Failure::Failed(format!("cannot unmount {node}: {e}")))?;
    }
    // the raw node: no buffer cache, so the read-back reads the card
    let raw = format!("/dev/r{}", disk.id);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_EXCL | libc::O_SYNC)
        .open(&raw)
        .map_err(|e| Failure::Failed(format!("cannot open {raw}: {e}")))?;
    // SAFETY: a valid descriptor and a documented fcntl
    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) };
    Ok(Device {
        file,
        id: disk.id.clone(),
    })
}

impl Device {
    pub fn target(&mut self) -> &mut dyn Target {
        &mut self.file
    }

    /// Closes the disk and mounts its new partitions, so the app can write
    /// the pairing key onto the boot partition.
    pub fn finish(self) -> Result<(), Failure> {
        self.file
            .sync_all()
            .map_err(|e| Failure::Failed(format!("syncing: {e}")))?;
        drop(self.file);
        let _ = diskutil(&["mountDisk", &format!("/dev/{}", self.id)]);
        Ok(())
    }
}
