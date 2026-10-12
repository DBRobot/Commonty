//! Who is a member of the fleet, as the release says: fleet/members.json
//! (each member's id, sha256 of their root, what name each id is, and who
//! is revoked). It comes with the controller's system, so it follows its
//! releases; no box is asked. Only the root the release lists as a name's
//! current one counts: an entry from before a recovery may be signed by a
//! key in a thief's hands, and one let in only by an invite waits for a
//! release to list it.

use identity::SignedEntry;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Members {
    members: Vec<String>,
    revoked: Vec<String>,
    names: BTreeMap<String, String>,
}

impl Members {
    pub fn parse(members_json: &str) -> anyhow::Result<Members> {
        let v: serde_json::Value = serde_json::from_str(members_json)?;
        let ids = |k: &str| -> Vec<String> {
            v[k].as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        };
        Ok(Members {
            members: ids("members"),
            revoked: ids("revoked"),
            names: serde_json::from_value(v["names"].clone()).unwrap_or_default(),
        })
    }

    /// the release lists this root as this name's, and none of its roots
    /// is revoked (identity::anchored)
    pub fn has(&self, e: &SignedEntry) -> bool {
        identity::anchored(e, &self.members, &self.revoked, &self.names).is_ok()
    }

    /// a list naming each of these entries' roots as theirs (tests)
    #[cfg(test)]
    pub fn listing(entries: &[&SignedEntry]) -> String {
        let ids: Vec<String> = entries
            .iter()
            .map(|e| identity::member_id(&e.entry.root))
            .collect();
        let names: BTreeMap<String, String> = entries
            .iter()
            .zip(&ids)
            .map(|(e, id)| (id.clone(), identity::name_tag(id, &e.entry.name)))
            .collect();
        serde_json::json!({ "members": ids, "revoked": [], "names": names }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::signed_entry;

    #[test]
    fn a_member_and_nobody_else() {
        let (ann, _) = signed_entry("ann");
        let (eve, _) = signed_entry("ann");
        let m = Members::parse(&Members::listing(&[&ann])).unwrap();
        assert!(m.has(&ann));
        assert!(!m.has(&eve), "the same name, another root");
        let id = identity::member_id(&ann.entry.root);
        let mut gone: serde_json::Value = serde_json::from_str(&Members::listing(&[&ann])).unwrap();
        gone["revoked"] = serde_json::json!([id]);
        assert!(!Members::parse(&gone.to_string()).unwrap().has(&ann));
        // listed, but not as ann's: a root from before a recovery
        let (newer, _) = signed_entry("ann");
        let mut moved: serde_json::Value =
            serde_json::from_str(&Members::listing(&[&newer])).unwrap();
        moved["members"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(id));
        assert!(!Members::parse(&moved.to_string()).unwrap().has(&ann));
        assert!(Members::parse(&moved.to_string()).unwrap().has(&newer));
    }
}
