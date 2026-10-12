//! Build orders: the laptop names which boxes build what, signs that with
//! the release key, and reads back what they built from the agreed log.
//!
//! A box acts only on a signed order that names it (dd-attest --order), and
//! its answer is signed by its host key, so neither the relay, the forge nor
//! the box the log is read through can put words in its mouth. Two builders
//! per job, as far apart as the fleet allows: a different owner, house and
//! region where there is one (docs/dev/app-release.md).

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use sha2::Digest as _;

/// builders per job
pub const BUILDERS: usize = 2;

/// What a build needs of the box that runs it (`passthru.needs` in the flake)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Need {
    pub kvm: bool,
    pub ram_gb: u64,
    /// an aarch64 build (the home controller's card): the runner boxes run
    /// aarch64 under emulation (roles/runner.nix)
    pub aarch64: bool,
}

impl Need {
    pub fn from_json(v: &serde_json::Value) -> Need {
        Need {
            kvm: v["kvm"].as_bool().unwrap_or(false),
            ram_gb: v["ramGB"].as_u64().unwrap_or(0),
            aarch64: v["aarch64"].as_bool().unwrap_or(false),
        }
    }
}

/// A box as the picker sees it: where it is and whose it is (boxes.json),
/// and what it said in its last heartbeat (the agreed state)
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    pub name: String,
    pub owner: Option<String>,
    pub site: Option<String>,
    pub region: Option<String>,
    pub alive: bool,
    pub ram_bytes: Option<f64>,
    pub kvm: Option<bool>,
    /// a runner, which runs aarch64 under emulation
    pub aarch64: bool,
}

impl Candidate {
    /// What a box says counts against it; what it has not said yet (a box
    /// on a dd-control from before the fact) does not.
    fn can(&self, need: &Need) -> bool {
        self.alive
            && self
                .ram_bytes
                .is_none_or(|b| b >= (need.ram_gb as f64) * 1e9 * 0.9)
            && (!need.kvm || self.kvm != Some(false))
            && (!need.aarch64 || self.aarch64)
    }
}

/// The boxes from boxes.json, their owners (fleet/boxes/<box>/box.json) and
/// the agreed state (dd-control's /state)
pub fn candidates(
    boxes: &serde_json::Value,
    owners: &BTreeMap<String, String>,
    state: &serde_json::Value,
) -> Vec<Candidate> {
    let now = state["time"].as_i64().unwrap_or(0);
    // dd-control's own rule (state.Alive), with its default window
    let gone_after = 90_000;
    let mut out = Vec::new();
    for (name, b) in boxes.as_object().into_iter().flatten() {
        let s = &state["boxes"][name];
        let field = |k: &str| b[k].as_str().map(str::to_string);
        out.push(Candidate {
            name: name.clone(),
            owner: owners.get(name).cloned(),
            site: field("siteId"),
            region: field("regionId"),
            alive: s["lastSeen"]
                .as_i64()
                .is_some_and(|seen| now - seen <= gone_after),
            ram_bytes: s["facts"]["ram_bytes"].as_f64(),
            kvm: s["facts"]["kvm"].as_f64().map(|v| v >= 1.0),
            aarch64: b["roles"]
                .as_array()
                .is_some_and(|r| r.iter().any(|x| x.as_str() == Some("runner"))),
        });
    }
    out
}

/// How far apart two boxes are: one for each of owner, house and region
/// that both state and that differ. Unknown is never apart.
pub fn separation(a: &Candidate, b: &Candidate) -> u8 {
    let apart = |x: &Option<String>, y: &Option<String>| match (x, y) {
        (Some(x), Some(y)) => x != y,
        _ => false,
    };
    apart(&a.owner, &b.owner) as u8
        + apart(&a.site, &b.site) as u8
        + apart(&a.region, &b.region) as u8
}

/// a box's place in line for one output: rendezvous hashing, so work
/// spreads as boxes join and an output lands on the same boxes each time
fn rank(path: &str, name: &str) -> [u8; 32] {
    sha2::Sha256::digest(format!("{path}\0{name}").as_bytes()).into()
}

/// The builders for one output: the first in line that can, then each next
/// the one furthest from those already chosen (ties: next in line). Also
/// how far apart the chosen are at the least, 0 to 3: 3 is the rule met,
/// anything less is the fleet not yet allowing it, said in the manifest.
pub fn pick(
    path: &str,
    need: &Need,
    boxes: &[Candidate],
    k: usize,
    exclude: &[String],
) -> Result<(Vec<String>, u8)> {
    let mut line: Vec<&Candidate> = boxes
        .iter()
        .filter(|c| c.can(need) && !exclude.contains(&c.name))
        .collect();
    line.sort_by_key(|c| rank(path, &c.name));
    ensure!(
        line.len() >= k,
        "{} box(es) can build {path} ({need:?}); {k} needed",
        line.len()
    );
    let mut chosen: Vec<&Candidate> = vec![line.remove(0)];
    while chosen.len() < k {
        let (i, _) = line
            .iter()
            .enumerate()
            .max_by_key(|(i, c)| {
                let near = chosen.iter().map(|o| separation(o, c)).min().unwrap_or(0);
                (near, std::cmp::Reverse(*i))
            })
            .expect("checked above");
        chosen.push(line.remove(i));
    }
    let mut apart = 3;
    for (i, a) in chosen.iter().enumerate() {
        for b in &chosen[i + 1..] {
            apart = apart.min(separation(a, b));
        }
    }
    Ok((chosen.iter().map(|c| c.name.clone()).collect(), apart))
}

/// One output the order asks for
#[derive(Clone, Debug)]
pub struct Job {
    pub name: String,
    pub attr: String,
    pub version: Option<String>,
    pub part: Option<String>,
    pub path: String,
    pub builders: Vec<String>,
}

pub fn order_payload(id: &str, flake: &str, rev: &str, jobs: &[Job]) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "issued": release::now(),
        "flake": flake,
        "rev": rev,
        "jobs": jobs.iter().map(|j| serde_json::json!({
            "name": j.name,
            "attr": j.attr,
            "version": j.version,
            "part": j.part,
            "path": j.path,
            "builders": j.builders,
        })).collect::<Vec<_>>(),
    })
}

/// What a box said about one job, once its signature checks out
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    Built { closure: String, nar: String },
    Failed(String),
}

/// One item of the log, checked: the box the log says sent it is the box
/// whose host key signed the statement, and the statement is about this
/// order, at this commit, for a job of it at the path it names.
/// `verify(attester, statement, signature)` checks the host key's
/// signature. None: not about this order.
pub fn read_item(
    item: &serde_json::Value,
    id: &str,
    flake: &str,
    rev: &str,
    jobs: &[Job],
    verify: &dyn Fn(&str, &str, &str) -> Result<()>,
) -> Result<Option<(String, String, Said)>> {
    let from = item["box"].as_str().context("a result names its box")?;
    let inner = &item["item"];
    let statement = inner["statement"].as_str().context("no statement")?;
    let signature = inner["signature"].as_str().context("no signature")?;
    let st: serde_json::Value = serde_json::from_str(statement)?;
    if st["order"] != id {
        return Ok(None);
    }
    ensure!(
        st["v"] == 2 && st["kind"] == "built",
        "{from}: not a build result"
    );
    ensure!(
        st["attester"] == from,
        "{from} sent a result in another box's name"
    );
    ensure!(
        st["flake"] == flake && st["rev"] == rev,
        "{from}: a result about another commit"
    );
    let name = st["job"].as_str().context("a result names its job")?;
    let job = jobs
        .iter()
        .find(|j| j.name == name)
        .with_context(|| format!("{from}: no job {name} in this order"))?;
    ensure!(
        job.builders.iter().any(|b| b == from),
        "{from} was not asked to build {name}"
    );
    ensure!(
        st["path"] == job.path.as_str(),
        "{from}: {name} at another path"
    );
    verify(from, statement, signature)?;
    let said = match st["error"].as_str() {
        Some(e) => Said::Failed(e.to_string()),
        None => Said::Built {
            closure: st["closure"].as_str().context("no closure")?.to_string(),
            nar: st["narHash"].as_str().context("no narHash")?.to_string(),
        },
    };
    Ok(Some((name.to_string(), from.to_string(), said)))
}

/// Where the order stands: per job, the builders that answered and what
/// they said. Done when every job has all its builders built and agreeing.
pub enum Standing {
    Waiting(Vec<String>),
    Agreed(BTreeMap<String, (String, String)>),
}

pub fn standing(jobs: &[Job], said: &BTreeMap<(String, String), Said>) -> Result<Standing> {
    let mut agreed = BTreeMap::new();
    let mut waiting = Vec::new();
    for job in jobs {
        let mut first: Option<(String, String)> = None;
        for b in &job.builders {
            match said.get(&(job.name.clone(), b.clone())) {
                None => waiting.push(format!("{} on {b}", job.name)),
                Some(Said::Failed(e)) => anyhow::bail!("{b} could not build {}: {e}", job.name),
                Some(Said::Built { closure, nar }) => match &first {
                    None => first = Some((closure.clone(), nar.clone())),
                    Some(f) => ensure!(
                        *f == (closure.clone(), nar.clone()),
                        "{}: the builders made different bytes. Nothing is signed; check it with nix build --rebuild",
                        job.name
                    ),
                },
            }
        }
        if let Some(f) = first
            && !waiting
                .iter()
                .any(|w| w.starts_with(&format!("{} on", job.name)))
        {
            agreed.insert(job.name.clone(), f);
        }
    }
    Ok(if waiting.is_empty() {
        Standing::Agreed(agreed)
    } else {
        Standing::Waiting(waiting)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(name: &str, owner: &str, site: &str, region: &str) -> Candidate {
        Candidate {
            name: name.into(),
            owner: Some(owner.into()),
            site: Some(site.into()),
            region: Some(region.into()),
            alive: true,
            ram_bytes: Some(32e9),
            kvm: Some(true),
            aarch64: false,
        }
    }

    #[test]
    fn builders_are_as_far_apart_as_the_fleet_allows() {
        let fleet = vec![
            cand("a1", "ann", "s1", "r1"),
            cand("a2", "ann", "s1", "r1"),
            cand("b1", "bob", "s2", "r2"),
            cand("c1", "cat", "s3", "r1"),
        ];
        for path in ["/nix/store/x", "/nix/store/y", "/nix/store/z"] {
            let (chosen, apart) = pick(path, &Need::default(), &fleet, 2, &[]).unwrap();
            assert_eq!(chosen.len(), 2);
            assert!(apart >= 2, "{path}: {chosen:?} only {apart} apart");
            // never two of ann's boxes in one house together
            assert!(!(chosen.contains(&"a1".into()) && chosen.contains(&"a2".into())));
        }
        // today's fleet: one owner, one house, one region - the best there
        // is, and said to be no further apart than that
        let today = vec![cand("node1", "d", "s", "r"), cand("node2", "d", "s", "r")];
        let (chosen, apart) = pick("/nix/store/x", &Need::default(), &today, 2, &[]).unwrap();
        assert_eq!(chosen.len(), 2);
        assert_eq!(apart, 0);
    }

    #[test]
    fn a_build_goes_only_where_it_can_run() {
        let mut small = cand("small", "ann", "s1", "r1");
        small.ram_bytes = Some(8e9);
        let mut novm = cand("novm", "bob", "s2", "r2");
        novm.kvm = Some(false);
        let mut off = cand("off", "cat", "s3", "r3");
        off.alive = false;
        let big = cand("big", "dan", "s4", "r4");
        let mut quiet = cand("quiet", "eve", "s5", "r5");
        quiet.kvm = None; // a dd-control from before the kvm fact
        let fleet = vec![small, novm, off, big, quiet];
        let need = Need {
            kvm: true,
            ram_gb: 16,
            aarch64: false,
        };
        let (chosen, _) = pick("/nix/store/x", &need, &fleet, 2, &[]).unwrap();
        let mut chosen = chosen;
        chosen.sort();
        assert_eq!(chosen, ["big", "quiet"]);
        // a box that went silent is passed over the next time
        assert!(pick("/nix/store/x", &need, &fleet, 2, &["quiet".into()]).is_err());
    }

    #[test]
    fn an_aarch64_build_goes_only_to_a_runner() {
        let mut runner = cand("runner", "ann", "s1", "r1");
        runner.aarch64 = true;
        let mut other = cand("other", "bob", "s2", "r2");
        other.aarch64 = true;
        let plain = cand("plain", "cat", "s3", "r3");
        let need = Need {
            aarch64: true,
            ..Need::default()
        };
        let (mut chosen, _) = pick("/nix/store/x", &need, &[runner, other, plain], 2, &[]).unwrap();
        chosen.sort();
        assert_eq!(chosen, ["other", "runner"]);
        assert!(Need::from_json(&serde_json::json!({"aarch64": true})).aarch64);
    }

    #[test]
    fn work_spreads_and_stays_put() {
        let fleet: Vec<_> = (0..8)
            .map(|i| cand(&format!("n{i}"), &format!("o{i}"), &format!("s{i}"), "r"))
            .collect();
        let first = |p: &str| pick(p, &Need::default(), &fleet, 2, &[]).unwrap().0[0].clone();
        let mut firsts: Vec<String> = (0..40).map(|i| first(&format!("/nix/store/{i}"))).collect();
        assert_eq!(
            first("/nix/store/7"),
            first("/nix/store/7"),
            "the same output, the same boxes"
        );
        firsts.sort();
        firsts.dedup();
        assert!(firsts.len() >= 5, "40 outputs began on only {firsts:?}");
    }

    fn job() -> Job {
        Job {
            name: "app-linux".into(),
            attr: "legacyPackages.x86_64-linux.appRelease".into(),
            version: Some("1.2.3".into()),
            part: Some("linux".into()),
            path: "/nix/store/p".into(),
            builders: vec!["node1".into(), "node3".into()],
        }
    }

    fn item(from: &str, attester: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut st = serde_json::json!({
            "v": 2, "kind": "built", "attester": attester, "flake": "f", "rev": "r",
            "order": "o1", "job": "app-linux", "path": "/nix/store/p",
            "closure": "c", "narHash": "n",
        });
        for (k, v) in extra.as_object().unwrap() {
            st[k] = v.clone();
        }
        serde_json::json!({ "box": from, "item": { "statement": st.to_string(), "signature": "sig" } })
    }

    #[test]
    fn a_result_counts_only_for_its_box_its_order_and_its_job() {
        let jobs = [job()];
        let ok = |_: &str, _: &str, s: &str| -> Result<()> {
            ensure!(s == "sig", "bad signature");
            Ok(())
        };
        let read = |i| read_item(&i, "o1", "f", "r", &jobs, &ok);
        let got = read(item("node1", "node1", serde_json::json!({})))
            .unwrap()
            .unwrap();
        assert_eq!(
            got.2,
            Said::Built {
                closure: "c".into(),
                nar: "n".into()
            }
        );
        // another order's result is not this one's business
        assert!(
            read(item("node1", "node1", serde_json::json!({"order": "o0"})))
                .unwrap()
                .is_none()
        );
        // in another box's name, about another commit, from a box not asked,
        // at another path: refused
        assert!(read(item("node1", "node3", serde_json::json!({}))).is_err());
        assert!(read(item("node1", "node1", serde_json::json!({"rev": "x"}))).is_err());
        assert!(read(item("node2", "node2", serde_json::json!({}))).is_err());
        assert!(
            read(item(
                "node1",
                "node1",
                serde_json::json!({"path": "/nix/store/q"})
            ))
            .is_err()
        );
        let failed = read(item(
            "node1",
            "node1",
            serde_json::json!({"error": "out of disk"}),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(failed.2, Said::Failed("out of disk".into()));
    }

    #[test]
    fn an_order_is_done_only_when_its_builders_agree() {
        let jobs = [job()];
        let built = |c: &str| Said::Built {
            closure: c.into(),
            nar: "n".into(),
        };
        let mut said = BTreeMap::new();
        said.insert(("app-linux".into(), "node1".into()), built("c"));
        assert!(
            matches!(standing(&jobs, &said).unwrap(), Standing::Waiting(w) if w == ["app-linux on node3"])
        );
        said.insert(("app-linux".into(), "node3".into()), built("c"));
        assert!(
            matches!(standing(&jobs, &said).unwrap(), Standing::Agreed(a) if a["app-linux"] == ("c".into(), "n".into()))
        );
        said.insert(("app-linux".into(), "node3".into()), built("other"));
        assert!(standing(&jobs, &said).is_err(), "two builders, two answers");
        said.insert(
            ("app-linux".into(), "node3".into()),
            Said::Failed("no".into()),
        );
        assert!(standing(&jobs, &said).is_err());
    }
}
