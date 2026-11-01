//! What the box installer's screen decides (bin/dd-box-setup.rs,
//! docs/dev/joining.md): which firmware settings it may change and how,
//! what the machine's firmware already allows, the join ticket's id, and a
//! backup's progress. Nothing here writes; the screen does, from these.

use std::collections::{BTreeMap, HashMap};
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
    /// Secure Boot on (the firmware's word)
    pub secure_boot: bool,
    /// Secure Boot in setup mode, its keys cleared: the box enrolls its own
    pub setup_mode: bool,
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
        setup_mode: efivar(sys, "SetupMode") == Some(1),
        raid,
        virt: dev.join("kvm").exists(),
    }
}

// The join's own minimum (state/ident/hardware.go, where these are tuned),
// asked before the disk is wiped rather than after it. Only what is known here
// and is not asked already: Checks has the TPM and UEFI, the network its step.
const MIN_THREADS: u64 = 4;
const MIN_MEM_TOTAL: u64 = 7 << 30;
pub const MIN_SYSTEM_DISK: u64 = 128_000_000_000;

/// What this machine lacks of the minimum, in the words a refused join uses
pub fn missing(threads: u64, mem_total: u64) -> Vec<String> {
    let mut why = Vec::new();
    if threads < MIN_THREADS {
        why.push(format!(
            "{MIN_THREADS} processor threads (it has {threads})"
        ));
    }
    if mem_total < MIN_MEM_TOTAL {
        why.push(format!(
            "8 GiB of memory (it has {:.1} GiB)",
            mem_total as f64 / (1u64 << 30) as f64
        ));
    }
    why
}

/// MemTotal from /proc/meminfo, in bytes
pub fn mem_total(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kb| kb * 1024)
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

/// When the ticket stops being good, seconds (its `check if time($t), $t <
/// <n>`), unverified: enough to say so before a disk is wiped for it.
pub fn ticket_expires(ticket: &str) -> Option<u64> {
    const CHECK: &str = "check if time($t), $t < ";
    let b = biscuit_auth::UnverifiedBiscuit::from_base64(ticket.trim()).ok()?;
    let src = b.print_block_source(0).ok()?;
    let i = src.find(CHECK)? + CHECK.len();
    let n: String = src[i..].chars().take_while(char::is_ascii_digit).collect();
    n.parse().ok()
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

/// How far a `nix build` is, from its `--log-format internal-json` lines
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Build {
    pub built: u64,
    pub to_build: u64,
    pub fetched: u64,
    pub to_fetch: u64,
    /// what it started building last
    pub building: Option<String>,
    // activity id -> nix's kind of it, for the kinds counted here
    #[serde(skip)]
    kinds: HashMap<u64, u64>,
}

// nix's ActivityType and ResultType
const COPY_PATHS: u64 = 103;
const BUILDS: u64 = 104;
const BUILD: u64 = 105;
const PROGRESS: u64 = 105;

impl Build {
    /// One line the build wrote to stderr. What is for a person comes back:
    /// a line that is not nix's, or nix's error. The rest is counted or dropped.
    pub fn line(&mut self, line: &str) -> Option<String> {
        let Some(json) = line.strip_prefix("@nix ") else {
            return Some(line.to_string());
        };
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let kind = v.get("type").and_then(|t| t.as_u64());
        let id = v.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
        match v.get("action")?.as_str()? {
            "msg" if v.get("level")?.as_u64()? == 0 => {
                return Some(plain(v.get("msg")?.as_str()?));
            }
            "start" => {
                let kind = kind?;
                if [COPY_PATHS, BUILDS].contains(&kind) {
                    self.kinds.insert(id, kind);
                }
                if kind == BUILD {
                    self.building = v.get("fields")?.get(0)?.as_str().map(drv_name);
                }
            }
            "result" if kind == Some(PROGRESS) => {
                let f = v.get("fields")?.as_array()?;
                let (done, all) = (f.first()?.as_u64()?, f.get(1)?.as_u64()?);
                match self.kinds.get(&id) {
                    Some(&BUILDS) => (self.built, self.to_build) = (done, all),
                    Some(&COPY_PATHS) => (self.fetched, self.to_fetch) = (done, all),
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    /// How much of the work is done, 0..1: a build counted as many fetches,
    /// since it is what the time goes on
    pub fn share(&self) -> f64 {
        const WEIGHT: u64 = 20;
        let all = self.to_build * WEIGHT + self.to_fetch;
        // what it fetches to evaluate comes first, a path or two of them: not
        // the work, whose size nix does not know yet
        if self.to_build == 0 && self.to_fetch < 20 {
            return 0.0;
        }
        ((self.built * WEIGHT + self.fetched) as f64 / all as f64).min(1.0)
    }
}

fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            let _ = it.by_ref().find(|c| c.is_ascii_alphabetic());
        } else {
            out.push(c);
        }
    }
    out
}

// /nix/store/<hash>-<name>.drv -> <name>
fn drv_name(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let name = base.split_once('-').map_or(base, |(_, n)| n);
    name.strip_suffix(".drv").unwrap_or(name).to_string()
}

/// How far an install is, 0..1, from its "== " stages said and its build: the
/// build (the third) is nearly all of it
pub fn install_share(stages: usize, build: f64) -> f64 {
    match stages {
        0..=2 => 0.02 * stages as f64,
        3 => 0.05 + 0.85 * build,
        4 => 0.95,
        _ => 0.98,
    }
}

/// Seconds left of a stage `share` done after `elapsed` seconds. Nothing
/// until there is enough of it to say.
pub fn eta(elapsed: u64, share: f64) -> Option<u64> {
    (elapsed >= 60 && (0.02..1.0).contains(&share))
        .then(|| (elapsed as f64 / share * (1.0 - share)) as u64)
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
                setup_mode: false,
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
    fn a_builds_numbers_out_of_nixs_lines() {
        let mut b = Build::default();
        let said = |b: &mut Build, l: &str| b.line(l);
        // as nix 2.34 writes them
        for l in [
            r#"@nix {"action":"start","id":1,"level":4,"parent":0,"text":"","type":104}"#,
            r#"@nix {"action":"start","id":2,"level":4,"parent":0,"text":"","type":103}"#,
            r#"@nix {"action":"start","id":3,"level":5,"parent":0,"text":"copying","type":0}"#,
            r#"@nix {"action":"result","fields":[0,4,0,0],"id":1,"type":105}"#,
            r#"@nix {"action":"result","fields":[30,100,1,0],"id":2,"type":105}"#,
            // another activity's numbers are not these
            r#"@nix {"action":"result","fields":[9,9,0,0],"id":3,"type":105}"#,
            r#"@nix {"action":"start","fields":["/nix/store/abc-dd-agent-0.1.0.drv","",1,1],"id":4,"level":3,"parent":1,"text":"building","type":105}"#,
            r#"@nix {"action":"result","fields":["compiling"],"id":4,"type":101}"#,
            r#"@nix {"action":"stop","id":4}"#,
            r#"@nix {"action":"result","fields":[1,4,0,0],"id":1,"type":105}"#,
            r#"@nix {"action":"msg","level":3,"msg":"these 4 derivations will be built:"}"#,
        ] {
            assert_eq!(said(&mut b, l), None, "{l}");
        }
        assert_eq!(
            (b.built, b.to_build, b.fetched, b.to_fetch),
            (1, 4, 30, 100)
        );
        assert_eq!(b.building.as_deref(), Some("dd-agent-0.1.0"));
        assert!((b.share() - 50.0 / 180.0).abs() < 1e-9);
        // nix's error, without its colours, and anyone else's line
        assert_eq!(
            said(
                &mut b,
                r#"@nix {"action":"msg","level":0,"msg":"\u001b[31;1merror:\u001b[0m no space"}"#
            )
            .as_deref(),
            Some("error: no space")
        );
        assert_eq!(
            said(&mut b, "failed: line 9").as_deref(),
            Some("failed: line 9")
        );
        assert_eq!(said(&mut b, "@nix not json"), None);
    }

    #[test]
    fn an_installs_share_and_what_is_left() {
        assert_eq!(Build::default().share(), 0.0);
        // one path of the evaluation's, all done, is not the install nearly over
        let eval = Build {
            fetched: 1,
            to_fetch: 1,
            ..Build::default()
        };
        assert_eq!(eval.share(), 0.0);
        assert_eq!(install_share(0, 0.0), 0.0);
        assert_eq!(install_share(3, 1.0), 0.9);
        assert!(install_share(5, 0.0) < 1.0);
        assert_eq!(eta(30, 0.5), None);
        assert_eq!(eta(600, 0.01), None);
        assert_eq!(eta(600, 0.5), Some(600));
    }

    #[test]
    fn the_minimum_says_what_is_missing() {
        // an "8 GB" machine, as its MemTotal says: just over
        assert_eq!(missing(4, 7 << 30), Vec::<String>::new());
        assert_eq!(
            missing(2, 4 << 30),
            [
                "4 processor threads (it has 2)",
                "8 GiB of memory (it has 4.0 GiB)"
            ]
        );
        assert_eq!(
            mem_total("MemFree: 1 kB\nMemTotal:       16384 kB\n"),
            Some(16 << 20)
        );
        assert_eq!(mem_total("nothing"), None);
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
        assert_eq!(ticket_expires("not a ticket"), None);
    }

    #[test]
    fn when_a_ticket_stops_being_good() {
        // as client/auth mint_join_ticket makes one
        let kp = biscuit_auth::KeyPair::new();
        let t = biscuit_auth::Biscuit::builder()
            .fact("purpose(\"join\")")
            .unwrap()
            .check("check if time($t), $t < 1791800000")
            .unwrap()
            .check("check if operation(\"join\")")
            .unwrap()
            .build(&kp)
            .unwrap()
            .to_base64()
            .unwrap();
        assert_eq!(ticket_expires(&t), Some(1791800000));
    }
}
