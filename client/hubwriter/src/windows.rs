use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::thread::sleep;
use std::time::Duration;

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BusTypeAta, BusTypeMmc, BusTypeNvme, BusTypeSata, BusTypeScsi, BusTypeSd, BusTypeUsb,
    CreateFileW, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    GetVolumePathNamesForVolumeNameW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, FSCTL_ALLOW_EXTENDED_DASD_IO, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME,
    FSCTL_UNLOCK_VOLUME, GET_LENGTH_INFORMATION, IOCTL_DISK_DELETE_DRIVE_LAYOUT,
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_GET_LENGTH_INFO, IOCTL_DISK_UPDATE_PROPERTIES,
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR,
    STORAGE_PROPERTY_QUERY, StorageDeviceProperty, VOLUME_DISK_EXTENTS,
};
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;

use crate::{Disk, Failure, Target};

const MAX_DRIVES: u32 = 64;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

fn open_handle(path: &str, access: u32, flags: u32) -> io::Result<OwnedHandle> {
    let p = wide(path);
    // SAFETY: p is a NUL-terminated wide string
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            flags,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a handle CreateFileW just returned, owned from here on
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

/// DeviceIoControl with plain in and out buffers; gives the bytes returned.
fn ioctl(h: &impl AsRawHandle, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<u32> {
    let mut got = 0u32;
    // SAFETY: the buffers are valid for the lengths given
    let ok = unsafe {
        DeviceIoControl(
            h.as_raw_handle() as HANDLE,
            code,
            if input.is_empty() {
                null()
            } else {
                input.as_ptr() as *const c_void
            },
            input.len() as u32,
            if output.is_empty() {
                null_mut()
            } else {
                output.as_mut_ptr() as *mut c_void
            },
            output.len() as u32,
            &mut got,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(got)
}

fn as_bytes<T>(v: &T) -> &[u8] {
    // SAFETY: a plain repr(C) struct read as bytes
    unsafe { std::slice::from_raw_parts(v as *const T as *const u8, size_of::<T>()) }
}

/// An 8-byte aligned buffer, for the structs the ioctls fill.
fn out_buf(len: usize) -> Vec<u64> {
    vec![0u64; len.div_ceil(8)]
}

fn bytes_mut(b: &mut [u64]) -> &mut [u8] {
    // SAFETY: u64s viewed as bytes
    unsafe { std::slice::from_raw_parts_mut(b.as_mut_ptr() as *mut u8, b.len() * 8) }
}

/// The physical drives a volume lies on.
fn extents(h: &OwnedHandle) -> io::Result<Vec<u32>> {
    let mut b = out_buf(8 + 24 * 32);
    ioctl(
        h,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        &[],
        bytes_mut(&mut b),
    )?;
    let v = b.as_ptr() as *const VOLUME_DISK_EXTENTS;
    // SAFETY: the buffer is aligned and large enough for 32 extents
    unsafe {
        let n = ((*v).NumberOfDiskExtents as usize).min(32);
        Ok((0..n)
            .map(|i| (*(*v).Extents.as_ptr().add(i)).DiskNumber)
            .collect())
    }
}

struct Volume {
    /// `\\?\Volume{guid}` without its trailing backslash, which opens the volume
    device: String,
    disks: Vec<u32>,
    paths: Vec<String>,
}

fn volumes() -> Vec<Volume> {
    let mut out = Vec::new();
    let mut name = [0u16; 512];
    // SAFETY: buffer and length agree
    let find = unsafe { FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32) };
    if find == INVALID_HANDLE_VALUE {
        return out;
    }
    loop {
        let guid = from_wide(&name);
        let device = guid.trim_end_matches('\\').to_string();
        let disks = open_handle(&device, 0, 0)
            .and_then(|h| extents(&h))
            .unwrap_or_default();
        let mut paths = [0u16; 1024];
        let mut len = 0u32;
        let g = wide(&guid);
        // SAFETY: buffer and length agree; g is NUL-terminated
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                g.as_ptr(),
                paths.as_mut_ptr(),
                paths.len() as u32,
                &mut len,
            )
        };
        let paths = if ok != 0 {
            paths[..len as usize]
                .split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(String::from_utf16_lossy)
                .collect()
        } else {
            Vec::new()
        };
        out.push(Volume {
            device,
            disks,
            paths,
        });
        // SAFETY: find is a valid search handle
        if unsafe { FindNextVolumeW(find, name.as_mut_ptr(), name.len() as u32) } == 0 {
            break;
        }
    }
    // SAFETY: closing the search handle once
    unsafe { FindVolumeClose(find) };
    out
}

/// The drives the Windows directory's volume lies on.
fn system_disks() -> Vec<u32> {
    let mut dir = [0u16; 300];
    // SAFETY: buffer and length agree
    let n = unsafe { GetSystemWindowsDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
    let dir = String::from_utf16_lossy(&dir[..n.min(dir.len())]);
    let Some(letter) = dir.get(..2) else {
        return vec![0];
    };
    open_handle(&format!(r"\\.\{letter}"), 0, 0)
        .and_then(|h| extents(&h))
        // unknown: take drive 0, as USBImager does for C:
        .unwrap_or_else(|_| vec![0])
}

fn ascii_at(buf: &[u8], off: u32) -> String {
    if off == 0 || off as usize >= buf.len() {
        return String::new();
    }
    let s = &buf[off as usize..];
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..end]).trim().to_string()
}

fn describe(h: &OwnedHandle) -> io::Result<(String, bool, String)> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut b = out_buf(1024);
    ioctl(
        h,
        IOCTL_STORAGE_QUERY_PROPERTY,
        as_bytes(&query),
        bytes_mut(&mut b),
    )?;
    let d = b.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR;
    // SAFETY: the buffer is aligned and the ioctl filled the descriptor
    let (bus, removable, vendor, product) = unsafe {
        (
            (*d).BusType,
            (*d).RemovableMedia,
            (*d).VendorIdOffset,
            (*d).ProductIdOffset,
        )
    };
    let raw = bytes_mut(&mut b);
    let model = format!("{} {}", ascii_at(raw, vendor), ascii_at(raw, product))
        .trim()
        .to_string();
    #[allow(non_upper_case_globals)]
    let (bus, by_bus) = match bus {
        BusTypeUsb => ("usb", true),
        BusTypeSd => ("sd", true),
        BusTypeMmc => ("mmc", true),
        BusTypeSata | BusTypeAta => ("sata", false),
        BusTypeNvme => ("nvme", false),
        BusTypeScsi => ("scsi", false),
        _ => ("other", false),
    };
    Ok((bus.to_string(), removable || by_bus, model))
}

/// Size and sector size from the geometry, which a handle opened for no
/// access may ask for; the length ioctl wants read access.
fn size_and_sector(h: &OwnedHandle) -> (u64, u32) {
    let mut g = out_buf(size_of::<DISK_GEOMETRY_EX>() + 64);
    let (size, sector) = match ioctl(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], bytes_mut(&mut g)) {
        // SAFETY: aligned, and filled by the ioctl
        Ok(_) => unsafe {
            let g = &*(g.as_ptr() as *const DISK_GEOMETRY_EX);
            (g.DiskSize.max(0) as u64, g.Geometry.BytesPerSector)
        },
        Err(_) => (0, 512),
    };
    let size = if size > 0 {
        size
    } else {
        let mut len = out_buf(size_of::<GET_LENGTH_INFORMATION>());
        match ioctl(h, IOCTL_DISK_GET_LENGTH_INFO, &[], bytes_mut(&mut len)) {
            Ok(_) => len[0],
            Err(_) => 0,
        }
    };
    (size, if sector == 0 { 512 } else { sector })
}

/// Every physical drive Windows has, removable or not.
pub fn list() -> io::Result<Vec<Disk>> {
    let vols = volumes();
    let system = system_disks();
    let mut disks = Vec::new();
    for n in 0..MAX_DRIVES {
        let name = format!(r"\\.\PhysicalDrive{n}");
        // no access asked: listing works without admin rights
        let Ok(h) = open_handle(&name, 0, 0) else {
            continue;
        };
        let (bus, removable, model) =
            describe(&h).unwrap_or(("other".into(), false, String::new()));
        let (size_bytes, sector) = size_and_sector(&h);
        let mounted = vols
            .iter()
            .filter(|v| v.disks.contains(&n))
            .flat_map(|v| {
                if v.paths.is_empty() {
                    vec![v.device.clone()]
                } else {
                    v.paths.clone()
                }
            })
            .collect();
        disks.push(Disk {
            id: format!("PhysicalDrive{n}"),
            name,
            model,
            bus,
            size_bytes,
            removable,
            mounted,
            system: system.contains(&n),
            sector,
        });
    }
    Ok(disks)
}

pub struct Device {
    file: File,
    /// locked and dismounted volumes, held until the write is done
    locks: Vec<OwnedHandle>,
}

fn lock(h: &OwnedHandle) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match ioctl(h, FSCTL_LOCK_VOLUME, &[], &mut []) {
            Ok(_) => return Ok(()),
            Err(_) if tries < 10 => {
                tries += 1;
                sleep(Duration::from_millis(500));
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn open(disk: &Disk) -> Result<Device, Failure> {
    let n: u32 = disk
        .id
        .strip_prefix("PhysicalDrive")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Failure::Refused(format!("{} is not a physical drive", disk.id)))?;
    let mut locks = Vec::new();
    for v in volumes().into_iter().filter(|v| v.disks.contains(&n)) {
        let what = v.paths.first().unwrap_or(&v.device).clone();
        let fail = |e: io::Error| Failure::Failed(format!("cannot unmount {what}: {e}"));
        let h = open_handle(&v.device, GENERIC_READ | GENERIC_WRITE, 0).map_err(fail)?;
        lock(&h).map_err(fail)?;
        ioctl(&h, FSCTL_DISMOUNT_VOLUME, &[], &mut []).map_err(fail)?;
        locks.push(h);
    }
    let h = open_handle(
        &disk.name,
        GENERIC_READ | GENERIC_WRITE,
        FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
    )
    .map_err(|e| Failure::Failed(format!("cannot open {}: {e}", disk.name)))?;
    let _ = ioctl(&h, FSCTL_ALLOW_EXTENDED_DASD_IO, &[], &mut []);
    // diskpart's clean, so Windows lets go of the old partitions; failing
    // is fine, the image's own partition table replaces the old one
    let _ = ioctl(&h, IOCTL_DISK_DELETE_DRIVE_LAYOUT, &[], &mut []);
    Ok(Device {
        file: File::from(h),
        locks,
    })
}

impl Device {
    pub fn target(&mut self) -> &mut dyn Target {
        &mut self.file
    }

    /// Flushes, has Windows read the new partition table, and lets the
    /// volumes go, so the boot partition gets a letter for the pairing key.
    pub fn finish(self) -> Result<(), Failure> {
        self.file
            .sync_all()
            .map_err(|e| Failure::Failed(format!("flushing: {e}")))?;
        let _ = ioctl(&self.file, IOCTL_DISK_UPDATE_PROPERTIES, &[], &mut []);
        drop(self.file);
        for h in self.locks {
            let _ = ioctl(&h, FSCTL_UNLOCK_VOLUME, &[], &mut []);
        }
        Ok(())
    }
}
