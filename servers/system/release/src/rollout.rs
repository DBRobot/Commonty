//! What the agent reads of a staged rollout (docs/dev/rollout.md): the
//! record the control plane agreed (servers/system/control/state/rollout.go),
//! and how a soak's samples compare with the box before its switch. The
//! rules here are the Go state machine's, read back; the agent decides
//! nothing the record does not say.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Report {
    #[serde(default)]
    pub took: i64,
    #[serde(default)]
    pub good: i64,
    #[serde(default)]
    pub bad: i64,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rollout {
    pub counter: u64,
    pub digest: String,
    #[serde(default)]
    pub waves: Vec<Vec<String>>,
    #[serde(default)]
    pub admitted: usize,
    #[serde(default)]
    pub since: i64,
    #[serde(default)]
    pub reports: BTreeMap<String, Report>,
    pub status: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub excused: Vec<String>,
}

/// `/rollout` on the box's own dd-control: the record and the agreed time
#[derive(Debug, Deserialize)]
pub struct Answer {
    pub rollout: Option<Rollout>,
    #[serde(default)]
    pub time: i64,
}

impl Rollout {
    pub fn wave_of(&self, name: &str) -> Option<usize> {
        self.waves.iter().position(|w| w.iter().any(|b| b == name))
    }

    /// may this box run the release now: its wave admitted and the
    /// rollout not stopped; a box in no wave once every wave has passed
    pub fn may(&self, name: &str) -> bool {
        if self.status == "bad" {
            return false;
        }
        match self.wave_of(name) {
            None => self.status == "done",
            Some(k) => k < self.admitted,
        }
    }

    /// its wave has passed: a stop no longer rolls it back
    pub fn proven(&self, name: &str) -> bool {
        if self.status == "done" {
            return true;
        }
        matches!(self.wave_of(name), Some(k) if k + 1 < self.admitted)
    }
}

/// What the record names a release by: sha256 hex of its canonical payload
/// (what the signature covers), then the signature's bytes - the Go
/// state's Digest. Two writings of one signed release have one digest, so
/// a box that hands in a re-written file cannot make the record name
/// something no agent fetched.
pub fn digest(raw: &str) -> crate::Result<String> {
    use base64::Engine as _;
    use sha2::Digest as _;
    let v: serde_json::Value = serde_json::from_str(raw)?;
    let sig = v["signature"]
        .as_str()
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .ok_or(crate::Error::Signature)?;
    let mut h = sha2::Sha256::new();
    h.update(crate::canonical_v2(&v["payload"])?);
    h.update(&sig);
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The names a record may give the release fetched as `raw`: its digest,
/// and sha256 of the file itself, which is what a validator from before
/// the digest named it by. That one only ever matches the exact bytes
/// fetched here, so it cannot be steered by another box; it is for the
/// rollout of the release that brings the digest, begun by validators that
/// did not know it yet.
pub fn digests(raw: &str) -> crate::Result<Vec<String>> {
    use sha2::Digest as _;
    let file: String = sha2::Sha256::digest(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(vec![digest(raw)?, file])
}

/// What the agent does with a release, by the record
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    /// no record of this release yet: ask for one
    Begin,
    /// not this box's turn; why
    Held(String),
    /// the rollout stopped; why
    Stopped(String),
    /// this box's wave is admitted
    Go,
}

pub fn gate(ans: &Answer, counter: u64, digests: &[String], name: &str) -> Gate {
    let r = match &ans.rollout {
        Some(r) if r.counter == counter => r,
        Some(r) if r.counter > counter => {
            return Gate::Held(format!(
                "release {counter} is behind the rollout of {}",
                r.counter
            ));
        }
        _ => return Gate::Begin,
    };
    if !digests.contains(&r.digest) {
        return Gate::Stopped(format!(
            "the rollout of release {counter} is of another file than the one fetched here"
        ));
    }
    if r.status == "bad" {
        return Gate::Stopped(format!("release {counter} stopped: {}", r.why));
    }
    if r.may(name) {
        return Gate::Go;
    }
    let k = r.admitted.saturating_sub(1);
    let wave = r.waves.get(k).cloned().unwrap_or_default();
    let took: Vec<i64> = wave
        .iter()
        .filter_map(|b| r.reports.get(b).map(|x| x.took))
        .filter(|t| *t > 0)
        .collect();
    let had = match took.iter().min() {
        Some(t) if ans.time > *t => format!("has had it {} min", (ans.time - t) / 60_000),
        _ => "has not taken it yet".into(),
    };
    Gate::Held(format!(
        "release {counter} waits for wave {} ({}), which {had}",
        k + 1,
        wave.join(", ")
    ))
}

/// What a soak compares against: the box just before its switch
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Baseline {
    /// service -> NRestarts
    pub restarts: BTreeMap<String, u64>,
    /// scrape targets of the box's own Prometheus that were down; None:
    /// no Prometheus answered, so metrics are not judged
    pub down: Option<Vec<String>>,
    /// fleet-alert conditions already true (pool, disks)
    pub alerts: Option<Vec<String>>,
}

/// A crash loop never shows as failed: a service that restarted more than
/// `limit` times since the baseline. A count lower than before was reset
/// (the unit was started again by the switch), and is all since then.
pub fn restarted(
    base: &BTreeMap<String, u64>,
    now: &BTreeMap<String, u64>,
    limit: u64,
) -> Vec<String> {
    now.iter()
        .filter_map(|(u, n)| {
            let b = base.get(u).copied().unwrap_or(0);
            let since = if *n >= b { n - b } else { *n };
            (since > limit).then(|| format!("{u} restarted {since} times"))
        })
        .collect()
}

/// what is in `now` and was not in `before`
pub fn newly(before: &Option<Vec<String>>, now: &Option<Vec<String>>) -> Vec<String> {
    match (before, now) {
        (Some(b), Some(n)) => n.iter().filter(|x| !b.contains(x)).cloned().collect(),
        _ => vec![],
    }
}

/// How many samples in a row each check has failed, kept between runs. A
/// check fails the soak at `need` in a row; one passing sample clears it.
pub fn streak(prev: &BTreeMap<String, u32>, failing: &[String]) -> BTreeMap<String, u32> {
    failing
        .iter()
        .map(|c| (c.clone(), prev.get(c).copied().unwrap_or(0) + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(json: &str) -> Answer {
        serde_json::from_str(json).unwrap()
    }

    // the Go state's TestDigestVector: one file, one digest in both
    const VECTOR: &str = r#"{"payload":{"boxes":{"node1":{"nar_hash":"sha256-x","path":"/nix/store/x-node1"}},"counter":3,"issued":1,"rev":"r"},"signature":"XPJ3sq51slhHO4CBqoPxLlGwAp6BKkm8tlanu+NBO+EEwOPkP9Li2DG9gIDgtsDnZE7x0PKs33zqlV4DRvoGCA==","signer":"O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik="}"#;
    const VECTOR_DIGEST: &str = "161eba379dbae48c31d29f4e371761f4182bfe8a57d208ffbdc64816cc6fabef";

    #[test]
    fn digest_is_the_go_states_and_ignores_how_the_file_is_written() {
        assert_eq!(digest(VECTOR).unwrap(), VECTOR_DIGEST);
        let v: serde_json::Value = serde_json::from_str(VECTOR).unwrap();
        let pretty = serde_json::to_string_pretty(&v).unwrap();
        let reordered = format!(
            "\n{{ \"signer\": {},\n  \"signature\": {}, \"payload\":\t{} }}",
            v["signer"],
            v["signature"],
            serde_json::to_string_pretty(&v["payload"]).unwrap()
        );
        assert_ne!(pretty, VECTOR);
        assert_eq!(digest(&pretty).unwrap(), VECTOR_DIGEST);
        assert_eq!(digest(&reordered).unwrap(), VECTOR_DIGEST);
        // the file's own hash, for a record begun before the digest
        assert_eq!(digests(&pretty).unwrap()[0], VECTOR_DIGEST);
        assert_ne!(digests(&pretty).unwrap()[1], digests(VECTOR).unwrap()[1]);
        // another signature is another release
        let forged = VECTOR.replace("XPJ3", "XPJ4");
        assert_ne!(digest(&forged).unwrap(), VECTOR_DIGEST);
    }

    const TWO: &str = r#"{"rollout":{"counter":7,"digest":"d","waves":[["node2"],["node1"]],"admitted":1,"since":1000,"reports":{"node2":{"took":2000}},"status":"soaking"},"height":5,"time":722000}"#;

    #[test]
    fn wave_one_goes_and_the_rest_are_held() {
        let a = record(TWO);
        assert_eq!(gate(&a, 7, &["d".into()], "node2"), Gate::Go);
        assert_eq!(
            gate(&a, 7, &["d".into()], "node1"),
            Gate::Held("release 7 waits for wave 1 (node2), which has had it 12 min".into())
        );
        // a box in no wave (not this fleet's control config) waits for all
        assert!(matches!(
            gate(&a, 7, &["d".into()], "home-controller"),
            Gate::Held(_)
        ));
    }

    #[test]
    fn no_record_or_an_older_one_begins() {
        let none = record(r#"{"rollout":null,"time":1}"#);
        assert_eq!(gate(&none, 7, &["d".into()], "node1"), Gate::Begin);
        assert_eq!(gate(&record(TWO), 8, &["d".into()], "node1"), Gate::Begin);
        assert!(matches!(
            gate(&record(TWO), 6, &["d".into()], "node1"),
            Gate::Held(_)
        ));
    }

    #[test]
    fn another_file_or_a_stop_is_never_taken() {
        assert!(matches!(
            gate(&record(TWO), 7, &["x".into()], "node2"),
            Gate::Stopped(_)
        ));
        let bad = TWO.replace(
            r#""status":"soaking""#,
            r#""status":"bad","why":"node2: probe""#,
        );
        assert_eq!(
            gate(&record(&bad), 7, &["d".into()], "node2"),
            Gate::Stopped("release 7 stopped: node2: probe".into())
        );
    }

    #[test]
    fn proven_is_a_passed_wave() {
        let mut r = record(TWO).rollout.unwrap();
        assert!(!r.proven("node2"));
        r.admitted = 2;
        r.status = "good".into();
        assert!(r.proven("node2") && !r.proven("node1") && r.may("node1"));
        r.status = "done".into();
        assert!(r.proven("node1") && r.may("home-controller"));
    }

    #[test]
    fn restarts_count_from_the_baseline_or_from_a_reset() {
        let base = BTreeMap::from([("a".to_string(), 10), ("b".to_string(), 5)]);
        let now = BTreeMap::from([
            ("a".to_string(), 12),
            ("b".to_string(), 4),
            ("c".to_string(), 9),
        ]);
        assert_eq!(
            restarted(&base, &now, 3),
            vec!["b restarted 4 times", "c restarted 9 times"]
        );
    }

    #[test]
    fn a_check_fails_on_its_third_sample_in_a_row() {
        let one = streak(&BTreeMap::new(), &["site".into()]);
        let two = streak(&one, &["site".into()]);
        assert_eq!(two["site"], 2);
        assert!(streak(&two, &[]).is_empty());
        assert_eq!(
            newly(&Some(vec!["a".into()]), &Some(vec!["a".into(), "b".into()])),
            vec!["b"]
        );
        assert!(newly(&None, &Some(vec!["b".into()])).is_empty());
    }
}
