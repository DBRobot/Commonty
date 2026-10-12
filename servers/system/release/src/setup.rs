//! What the box installer's screen decides (bin/dd-box-setup.rs,
//! docs/dev/joining.md): which firmware settings it may change and how,
//! what the machine's firmware already allows, the join ticket's id, and a
//! backup's progress. Nothing here writes; the screen does, from these.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The settings the installer may change, by the firmware driver that
/// offers them (bios-settings.json, beside this crate). A setting not in
/// it is never touched.
pub const TABLE: &str = include_str!("../bios-settings.json");

pub fn table() -> Table {
    serde_json::from_str(TABLE).expect("bios-settings.json is the table")
}

#[derive(Debug, Deserialize)]
pub struct Table {
    pub drivers: Vec<Driver>,
}

#[derive(Debug, Deserialize)]
pub struct Driver {
    /// the directory under /sys/class/firmware-attributes
    pub driver: String,
    /// where the names come from
    pub source: String,
    /// what is wanted (tpm, power, virt), and the attribute and value
    /// that give it, in the order they are tried
    pub settings: BTreeMap<String, Vec<Choice>>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub attribute: String,
    pub value: String,
}

/// One change to make: write `value` to `path` (an attribute's
/// current_value).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub want: String,
    pub attribute: String,
    pub value: String,
    #[serde(skip)]
    pub path: PathBuf,
}

/// What the firmware lets the installer do about each wanted setting.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// the driver found, if any
    pub driver: Option<String>,
    /// a BIOS password is set: nothing can be changed from here
    pub locked: bool,
    /// already as wanted
    pub already: Vec<String>,
    /// to change, then restart
    pub change: Vec<Change>,
    /// not offered here: to change at the firmware's own screen
    pub manual: Vec<String>,
}

fn read(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// The plan for `wants`, from the firmware attributes under `root`
/// (/sys/class/firmware-attributes). A value is set only where the
/// attribute says it takes it (possible_values) and its current value can
/// be read; anything else is left to the person, by hand.
pub fn plan(root: &Path, table: &Table, wants: &[&str]) -> Plan {
    let mut out = Plan::default();
    let found = table
        .drivers
        .iter()
        .find(|d| root.join(&d.driver).join("attributes").is_dir());
    let Some(d) = found else {
        out.manual = wants.iter().map(|w| w.to_string()).collect();
        return out;
    };
    out.driver = Some(d.driver.clone());
    let auth = root.join(&d.driver).join("authentication");
    out.locked = fs::read_dir(&auth)
        .map(|r| {
            r.flatten()
                .any(|e| read(&e.path().join("is_enabled")).as_deref() == Some("1"))
        })
        .unwrap_or(false);
    for want in wants {
        let choice = d.settings.get(*want).and_then(|cs| {
            cs.iter().find_map(|c| {
                let a = root.join(&d.driver).join("attributes").join(&c.attribute);
                let current = read(&a.join("current_value"))?;
                let possible = read(&a.join("possible_values"))?;
                possible
                    .split(';')
                    .any(|v| v.trim() == c.value)
                    .then(|| (c, current, a.join("current_value")))
            })
        });
        match choice {
            Some((c, current, _)) if current == c.value => out.already.push(want.to_string()),
            Some((c, _, path)) if !out.locked => out.change.push(Change {
                want: want.to_string(),
                attribute: c.attribute.clone(),
                value: c.value.clone(),
                path,
            }),
            _ => out.manual.push(want.to_string()),
        }
    }
    out
}

/// The firmware wants a restart before a change counts.
pub fn pending_reboot(root: &Path, driver: &str) -> bool {
    read(&root.join(driver).join("attributes/pending_reboot")).as_deref() == Some("1")
}

/// What this machine's firmware gives a box, read from `sys` (/sys) and
/// `dev` (/dev).
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Checks {
    /// booted by UEFI
    pub uefi: bool,
    /// a TPM 2.0 the kernel found
    pub tpm: bool,
    /// Secure Boot on: the installer does not boot then, so this is the
    /// firmware's word only
    pub secure_boot: bool,
    /// a disk controller in RAID mode (Intel RST): its disks are hidden
    pub raid: bool,
    /// virtual machines can run (KVM): games and CI need it
    pub virt: bool,
}

fn efivar(sys: &Path, name: &str) -> Option<u8> {
    let p = sys.join(format!(
        "firmware/efi/efivars/{name}-8be4df61-93ca-11d2-aa0d-00e098032b8c"
    ));
    fs::read(p).ok().and_then(|b| b.get(4).copied())
}

pub fn checks(sys: &Path, dev: &Path) -> Checks {
    let raid = fs::read_dir(sys.join("bus/pci/devices"))
        .map(|r| {
            r.flatten()
                .any(|e| read(&e.path().join("class")).as_deref() == Some("0x010400"))
        })
        .unwrap_or(false);
    Checks {
        uefi: sys.join("firmware/efi").is_dir(),
        tpm: read(&sys.join("class/tpm/tpm0/tpm_version_major")).as_deref() == Some("2"),
        secure_boot: efivar(sys, "SecureBoot") == Some(1),
        raid,
        virt: dev.join("kvm").exists(),
    }
}

/// The ticket's id (`ticket("<hex>")` in its first block), unverified: the
/// gate checks the ticket, this only says where its backup goes.
pub fn ticket_id(ticket: &str) -> Option<String> {
    let b = biscuit_auth::UnverifiedBiscuit::from_base64(ticket.trim()).ok()?;
    let src = b.print_block_source(0).ok()?;
    let i = src.find("ticket(\"")? + 8;
    let id: String = src[i..].chars().take_while(|c| *c != '"').collect();
    (id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())).then_some(id)
}

/// A backup's progress from restic's `--json` lines: done (0..1) and the
/// seconds it says are left.
pub fn restic_progress(line: &str) -> Option<(f64, Option<u64>)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    match v.get("message_type")?.as_str()? {
        "status" => Some((
            v.get("percent_done")?.as_f64()?,
            v.get("seconds_remaining").and_then(|s| s.as_u64()),
        )),
        "summary" => Some((1.0, Some(0))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(root: &Path, drv: &str, name: &str, current: &str, possible: &str) {
        let a = root.join(drv).join("attributes").join(name);
        fs::create_dir_all(&a).unwrap();
        fs::write(a.join("current_value"), format!("{current}\n")).unwrap();
        fs::write(a.join("possible_values"), format!("{possible}\n")).unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dd-setup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn every_driver_says_where_its_names_come_from() {
        for d in table().drivers {
            assert!(d.source.starts_with("https://"), "{}", d.driver);
            for w in ["tpm", "power", "virt"] {
                assert!(d.settings.contains_key(w), "{} has no {w}", d.driver);
            }
        }
    }

    #[test]
    fn no_driver_means_by_hand() {
        let root = tmp("none");
        let p = plan(&root, &table(), &["tpm", "virt"]);
        assert_eq!(p.driver, None);
        assert_eq!(p.manual, vec!["tpm", "virt"]);
        assert!(p.change.is_empty());
    }

    #[test]
    fn sets_only_what_the_attribute_takes() {
        let root = tmp("dell");
        attr(
            &root,
            "dell-wmi-sysman",
            "Virtualization",
            "Disabled",
            "Disabled;Enabled",
        );
        attr(&root, "dell-wmi-sysman", "AcPwrRcvry", "Off", "Off;Last;On");
        // a value the table wants that this machine does not offer
        attr(
            &root,
            "dell-wmi-sysman",
            "TpmSecurity",
            "Disabled",
            "Disabled",
        );
        let p = plan(&root, &table(), &["tpm", "power", "virt"]);
        assert_eq!(p.driver.as_deref(), Some("dell-wmi-sysman"));
        let set: Vec<_> = p
            .change
            .iter()
            .map(|c| (c.want.as_str(), c.value.as_str()))
            .collect();
        assert_eq!(set, vec![("power", "On"), ("virt", "Enabled")]);
        assert_eq!(p.manual, vec!["tpm"]);
    }

    #[test]
    fn already_set_is_left() {
        let root = tmp("lenovo");
        attr(
            &root,
            "thinklmi",
            "VirtualizationTechnology",
            "Enable",
            "Disable;Enable",
        );
        let p = plan(&root, &table(), &["virt"]);
        assert_eq!(p.already, vec!["virt"]);
        assert!(p.change.is_empty());
    }

    #[test]
    fn a_bios_password_changes_nothing() {
        let root = tmp("locked");
        attr(
            &root,
            "hp-bioscfg",
            "After Power Loss",
            "Power Off",
            "Power Off;Power On;Previous State",
        );
        let auth = root.join("hp-bioscfg/authentication/Setup Password");
        fs::create_dir_all(&auth).unwrap();
        fs::write(auth.join("is_enabled"), "1\n").unwrap();
        let p = plan(&root, &table(), &["power"]);
        assert!(p.locked);
        assert!(p.change.is_empty());
        assert_eq!(p.manual, vec!["power"]);
    }

    #[test]
    fn checks_read_the_firmware() {
        let sys = tmp("sys");
        let dev = tmp("dev");
        fs::create_dir_all(sys.join("firmware/efi/efivars")).unwrap();
        fs::write(
            sys.join("firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"),
            [6, 0, 0, 0, 0],
        )
        .unwrap();
        fs::create_dir_all(sys.join("class/tpm/tpm0")).unwrap();
        fs::write(sys.join("class/tpm/tpm0/tpm_version_major"), "2\n").unwrap();
        fs::create_dir_all(sys.join("bus/pci/devices/0000:00:17.0")).unwrap();
        fs::write(sys.join("bus/pci/devices/0000:00:17.0/class"), "0x010400\n").unwrap();
        let c = checks(&sys, &dev);
        assert_eq!(
            c,
            Checks {
                uefi: true,
                tpm: true,
                secure_boot: false,
                raid: true,
                virt: false
            }
        );
    }

    #[test]
    fn restic_lines() {
        let s = r#"{"message_type":"status","percent_done":0.25,"seconds_remaining":120}"#;
        assert_eq!(restic_progress(s), Some((0.25, Some(120))));
        assert_eq!(
            restic_progress(r#"{"message_type":"summary"}"#),
            Some((1.0, Some(0)))
        );
        assert_eq!(restic_progress("not json"), None);
    }

    #[test]
    fn a_tickets_id() {
        // as client/auth mint_join_ticket makes one
        let kp = biscuit_auth::KeyPair::new();
        let id = "00112233445566778899aabbccddeeff";
        let t = biscuit_auth::Biscuit::builder()
            .fact("user(\"ada\")")
            .unwrap()
            .fact(format!("ticket({id:?})").as_str())
            .unwrap()
            .fact("purpose(\"join\")")
            .unwrap()
            .build(&kp)
            .unwrap()
            .to_base64()
            .unwrap();
        assert_eq!(ticket_id(&t).as_deref(), Some(id));
    }

    #[test]
    fn a_ticket_that_is_not_one_has_no_id() {
        assert_eq!(ticket_id("not a ticket"), None);
    }
}
