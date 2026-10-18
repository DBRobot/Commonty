//! The TPM makers' roots a release ships (fleet/tpm-roots,
//! docs/dev/attestation.md): the control plane chains every box's EK
//! certificate to them, so a file there that is not a CA certificate in
//! date, or not under its maker's vendor id, stops the release.

use anyhow::{Result, bail};
use std::collections::BTreeMap;
use x509_parser::pem::Pem;

/// The roots at a commit, by path under fleet/tpm-roots.
pub fn at(root: &std::path::Path, rev: &str) -> Result<BTreeMap<String, String>> {
    let git = |args: &[&str]| -> Result<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()?;
        if !out.status.success() {
            bail!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let mut files = BTreeMap::new();
    for path in git(&["ls-tree", "-r", "--name-only", rev, "fleet/tpm-roots"])?.lines() {
        let rel = path.trim_start_matches("fleet/tpm-roots/");
        if rel != "README.md" {
            files.insert(rel.to_owned(), git(&["show", &format!("{rev}:{path}")])?);
        }
    }
    Ok(files)
}

/// Each file `<vendor id>/<name>.pem`, its certificates CAs in date at
/// `now` (unix seconds); and at least one.
pub fn check(files: &BTreeMap<String, String>, now: i64) -> Result<()> {
    if files.is_empty() {
        bail!("fleet/tpm-roots is empty: no box could attest");
    }
    for (path, text) in files {
        let Some((vendor, name)) = path.split_once('/') else {
            bail!("fleet/tpm-roots/{path}: not in a maker's folder");
        };
        if vendor.len() != 8
            || !vendor
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
        {
            bail!(
                "fleet/tpm-roots/{path}: {vendor} is not a TCG vendor id (8 hex digits, upper case)"
            );
        }
        if !name.ends_with(".pem") || name.contains('/') {
            bail!("fleet/tpm-roots/{path}: only .pem files, one level down");
        }
        let mut n = 0;
        for pem in Pem::iter_from_buffer(text.as_bytes()) {
            let pem = match pem {
                Ok(p) => p,
                Err(e) => bail!("fleet/tpm-roots/{path}: {e}"),
            };
            let cert = match pem.parse_x509() {
                Ok(c) => c,
                Err(e) => bail!("fleet/tpm-roots/{path}: not a certificate: {e}"),
            };
            if !cert.is_ca() {
                bail!("fleet/tpm-roots/{path}: not a CA certificate");
            }
            let v = cert.validity();
            if now < v.not_before.timestamp() || now > v.not_after.timestamp() {
                bail!(
                    "fleet/tpm-roots/{path}: not in date ({} to {})",
                    v.not_before,
                    v.not_after
                );
            }
            n += 1;
        }
        if n == 0 {
            bail!("fleet/tpm-roots/{path}: no certificate in it");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTEL: &str = include_str!("tpm_roots/intel-root.pem");
    // node1's EK certificate: a TPM's own, not a CA
    const LEAF: &str = include_str!("tpm_roots/node1-ek.pem");
    const OCT_2026: i64 = 1_792_195_200;

    fn one(path: &str, text: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(path.to_owned(), text.to_owned())])
    }

    fn refused(files: &BTreeMap<String, String>, now: i64, part: &str) {
        match check(files, now) {
            Ok(()) => panic!("passed; want {part:?}"),
            Err(e) => assert!(e.to_string().contains(part), "{e}"),
        }
    }

    #[test]
    fn a_makers_root_in_date_passes() {
        check(&one("494E5443/intel-ondie-ca-root.pem", INTEL), OCT_2026).unwrap();
    }

    #[test]
    fn a_root_out_of_date_stops_the_release() {
        // Intel's runs to the end of 2049
        refused(
            &one("494E5443/intel.pem", INTEL),
            2_600_000_000,
            "not in date",
        );
        refused(&one("494E5443/intel.pem", INTEL), 0, "not in date");
    }

    #[test]
    fn a_tpms_own_certificate_is_no_root() {
        refused(&one("494E5443/node1.pem", LEAF), OCT_2026, "not a CA");
    }

    #[test]
    fn a_root_must_sit_under_its_vendor_id() {
        refused(
            &one("intel/intel.pem", INTEL),
            OCT_2026,
            "not a TCG vendor id",
        );
        refused(
            &one("494e5443/intel.pem", INTEL),
            OCT_2026,
            "not a TCG vendor id",
        );
        refused(
            &one("intel.pem", INTEL),
            OCT_2026,
            "not in a maker's folder",
        );
        refused(&one("494E5443/intel.der", INTEL), OCT_2026, "only .pem");
    }

    #[test]
    fn junk_and_nothing_stop_the_release() {
        refused(
            &one("494E5443/x.pem", "not a certificate"),
            OCT_2026,
            "no certificate",
        );
        let bad = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
        refused(&one("494E5443/x.pem", bad), OCT_2026, "not a certificate");
        refused(&BTreeMap::new(), OCT_2026, "empty");
    }
}
