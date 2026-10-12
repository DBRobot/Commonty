//! `diskutil list -plist` and `diskutil info -plist`, read into disks.

use std::collections::BTreeMap;

use plist::{Dictionary, Value};

use crate::Disk;

pub struct Listing {
    /// physical whole disks when listed with `physical`, every whole disk otherwise
    pub whole_disks: Vec<String>,
    /// mount points by the physical whole disk under them, through APFS
    /// containers to their stores
    pub mounts: BTreeMap<String, Vec<String>>,
}

pub fn parse_list(bytes: &[u8]) -> Result<Listing, String> {
    let root = Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let root = root.as_dictionary().ok_or("not a dictionary")?;
    let whole_disks = strings(root.get("WholeDisks"));
    let mut mounts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in array(root.get("AllDisksAndPartitions")) {
        let Some(entry) = entry.as_dictionary() else {
            continue;
        };
        let id = string(entry, "DeviceIdentifier");
        let stores: Vec<String> = array(entry.get("APFSPhysicalStores"))
            .iter()
            .filter_map(|s| s.as_dictionary())
            .map(|s| whole_of(&string(s, "DeviceIdentifier")))
            .collect();
        let owners = if stores.is_empty() {
            vec![whole_of(&id)]
        } else {
            stores
        };
        let mut here = Vec::new();
        if let Some(m) = entry.get("MountPoint").and_then(Value::as_string) {
            here.push(m.to_string());
        }
        for key in ["Partitions", "APFSVolumes"] {
            for p in array(entry.get(key)) {
                if let Some(m) = p
                    .as_dictionary()
                    .and_then(|p| p.get("MountPoint"))
                    .and_then(Value::as_string)
                {
                    here.push(m.to_string());
                }
            }
        }
        for o in owners {
            mounts.entry(o).or_default().extend(here.iter().cloned());
        }
    }
    Ok(Listing {
        whole_disks,
        mounts,
    })
}

/// `disk4s2` and `disk3s1s1` are on `disk4` and `disk3`.
pub fn whole_of(id: &str) -> String {
    let digits = id
        .strip_prefix("disk")
        .map(|r| r.chars().take_while(char::is_ascii_digit).count())
        .unwrap_or(0);
    if digits == 0 {
        return id.to_string();
    }
    id[..4 + digits].to_string()
}

pub fn is_system_mount(m: &str) -> bool {
    m == "/" || m.starts_with("/System/Volumes/")
}

/// One whole disk from `diskutil info -plist diskN` and the listing.
pub fn parse_info(id: &str, bytes: &[u8], listing: &Listing) -> Result<Disk, String> {
    let root = Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let info = root.as_dictionary().ok_or("not a dictionary")?;
    let protocol = string(info, "BusProtocol");
    let bus = match protocol.as_str() {
        "USB" => "usb".to_string(),
        "Secure Digital" => "sd".to_string(),
        "SATA" => "sata".to_string(),
        "PCI-Express" | "Apple Fabric" => "pcie".to_string(),
        "NVMe" => "nvme".to_string(),
        "" => "other".to_string(),
        other => other.to_ascii_lowercase(),
    };
    Ok(disk(id, info, bus, listing))
}

fn disk(id: &str, info: &Dictionary, bus: String, listing: &Listing) -> Disk {
    let mounted = listing.mounts.get(id).cloned().unwrap_or_default();
    let system = mounted.iter().any(|m| is_system_mount(m));
    let removable =
        flag(info, "RemovableMedia") || flag(info, "Removable") || bus == "usb" || bus == "sd";
    let mut model = string(info, "MediaName");
    if model.is_empty() {
        model = string(info, "IORegistryEntryName");
    }
    let size_bytes = ["TotalSize", "Size"]
        .iter()
        .find_map(|k| info.get(k).and_then(Value::as_unsigned_integer))
        .unwrap_or(0);
    let sector = info
        .get("DeviceBlockSize")
        .and_then(Value::as_unsigned_integer)
        .unwrap_or(512) as u32;
    Disk {
        id: id.to_string(),
        name: format!("/dev/{id}"),
        model: model.trim().to_string(),
        bus,
        size_bytes,
        removable: removable && flag_or(info, "WritableMedia", true),
        mounted,
        system,
        sector,
    }
}

fn array(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn strings(v: Option<&Value>) -> Vec<String> {
    array(v)
        .iter()
        .filter_map(Value::as_string)
        .map(str::to_string)
        .collect()
}

fn string(d: &Dictionary, key: &str) -> String {
    d.get(key)
        .and_then(Value::as_string)
        .unwrap_or_default()
        .to_string()
}

fn flag(d: &Dictionary, key: &str) -> bool {
    flag_or(d, key, false)
}

fn flag_or(d: &Dictionary, key: &str, default: bool) -> bool {
    d.get(key).and_then(Value::as_boolean).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>AllDisksAndPartitions</key><array>
 <dict><key>DeviceIdentifier</key><string>disk0</string><key>Size</key><integer>500277790720</integer>
  <key>Partitions</key><array>
   <dict><key>DeviceIdentifier</key><string>disk0s1</string></dict>
   <dict><key>DeviceIdentifier</key><string>disk0s2</string></dict>
  </array></dict>
 <dict><key>DeviceIdentifier</key><string>disk3</string>
  <key>APFSPhysicalStores</key><array><dict><key>DeviceIdentifier</key><string>disk0s2</string></dict></array>
  <key>APFSVolumes</key><array>
   <dict><key>DeviceIdentifier</key><string>disk3s1s1</string><key>MountPoint</key><string>/</string></dict>
   <dict><key>DeviceIdentifier</key><string>disk3s5</string><key>MountPoint</key><string>/System/Volumes/Data</string></dict>
  </array></dict>
 <dict><key>DeviceIdentifier</key><string>disk4</string><key>Size</key><integer>31914983424</integer>
  <key>Partitions</key><array>
   <dict><key>DeviceIdentifier</key><string>disk4s1</string><key>MountPoint</key><string>/Volumes/bootfs</string></dict>
   <dict><key>DeviceIdentifier</key><string>disk4s2</string></dict>
  </array></dict>
</array>
<key>WholeDisks</key><array><string>disk0</string><string>disk3</string><string>disk4</string></array>
</dict></plist>"#;

    fn info(bus: &str, internal: bool, removable_media: bool) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>BusProtocol</key><string>{bus}</string>
<key>Internal</key><{internal}/>
<key>RemovableMedia</key><{removable_media}/>
<key>MediaName</key><string>SD Card Reader </string>
<key>TotalSize</key><integer>31914983424</integer>
<key>DeviceBlockSize</key><integer>512</integer>
<key>WritableMedia</key><true/>
</dict></plist>"#
        )
    }

    #[test]
    fn whole_disks() {
        assert_eq!(whole_of("disk4s2"), "disk4");
        assert_eq!(whole_of("disk3s1s1"), "disk3");
        assert_eq!(whole_of("disk12"), "disk12");
        assert_eq!(whole_of("weird"), "weird");
    }

    #[test]
    fn listing_follows_apfs_to_its_store() {
        let l = parse_list(LIST.as_bytes()).unwrap();
        assert_eq!(l.whole_disks, ["disk0", "disk3", "disk4"]);
        assert_eq!(l.mounts["disk0"], ["/", "/System/Volumes/Data"]);
        assert_eq!(l.mounts["disk4"], ["/Volumes/bootfs"]);
        assert!(!l.mounts.contains_key("disk3"));
    }

    #[test]
    fn info_reads_into_a_disk() {
        let l = parse_list(LIST.as_bytes()).unwrap();
        let sd = parse_info("disk4", info("Secure Digital", true, true).as_bytes(), &l).unwrap();
        assert_eq!(sd.bus, "sd");
        assert!(sd.removable && !sd.system);
        assert_eq!(sd.model, "SD Card Reader");
        assert_eq!(sd.size_bytes, 31914983424);
        assert_eq!(sd.mounted, ["/Volumes/bootfs"]);
        assert_eq!(crate::refusal(&sd, 1 << 30), None);

        let usb = parse_info("disk4", info("USB", false, false).as_bytes(), &l).unwrap();
        assert!(usb.removable);

        let internal =
            parse_info("disk0", info("Apple Fabric", true, false).as_bytes(), &l).unwrap();
        assert_eq!(internal.bus, "pcie");
        assert!(!internal.removable && internal.system);
        assert!(crate::refusal(&internal, 1).is_some());

        // a USB disk the Mac booted from is still the system disk
        let booted = parse_info("disk0", info("USB", false, true).as_bytes(), &l).unwrap();
        assert!(booted.removable && booted.system);
        assert!(
            crate::refusal(&booted, 1)
                .unwrap()
                .contains("running system")
        );
    }
}
