//! /_dd/releases.json: the newest release notes, for the assistant's "what's
//! new" (servers/web-shared/guide.js). They are the forge's releases that
//! `dd release publish` writes (r<N>, named "N · words", one "- " line per
//! change), read as anybody from the same forge and repository the release's
//! member list comes from: a public repository's release page, nothing more.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// how many releases a page is told of
const FEW: usize = 5;

pub struct Notes {
    url: String,
    /// the list as last read, and when: a good one for ten minutes, a
    /// failure for one; a good one is kept to answer through a failure
    seen: Mutex<Option<(Instant, bool, Value)>>,
}

impl Notes {
    /// From the member list's address on the forge
    /// (https://<forge>/<owner>/<repo>/raw/branch/releases/members.json):
    /// that repository's releases in the forge's API. None for any other shape.
    pub fn from_member_list(url: &str) -> Option<Notes> {
        let u = url::Url::parse(url).ok()?;
        if u.scheme() != "https" {
            return None;
        }
        let repo = u
            .path()
            .strip_prefix('/')?
            .strip_suffix("/raw/branch/releases/members.json")?;
        let (owner, name) = repo.split_once('/')?;
        let fine = |s: &str| {
            !s.is_empty()
                && !s.starts_with('.')
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        };
        if !fine(owner) || !fine(name) {
            return None;
        }
        Some(Notes {
            url: format!(
                "{}://{}/api/v1/repos/{owner}/{name}/releases?limit={FEW}",
                u.scheme(),
                u.host_str()?
            ),
            seen: Mutex::new(None),
        })
    }

    /// {"releases": [{n, at, title, notes}]}, newest first; None when the
    /// forge could not be read and nothing was read before
    pub async fn get(&self) -> Option<Value> {
        let kept = self.seen.lock().unwrap().clone();
        if let Some((at, good, v)) = &kept {
            let fresh = if *good { 600 } else { 60 };
            if at.elapsed() < Duration::from_secs(fresh) {
                return good.then(|| v.clone());
            }
        }
        let read = async {
            super::http()
                .get(&self.url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .json::<Value>()
                .await
                .ok()
        }
        .await;
        match read.as_ref().and_then(Value::as_array) {
            Some(list) => {
                let v = json!({ "releases": shape(list) });
                *self.seen.lock().unwrap() = Some((Instant::now(), true, v.clone()));
                Some(v)
            }
            None => {
                eprintln!("release notes: {} could not be read", self.url);
                // the last good list, still; only its age is reset
                let last = kept_good(&kept);
                *self.seen.lock().unwrap() = Some((
                    Instant::now(),
                    last.is_some(),
                    last.clone().unwrap_or(Value::Null),
                ));
                last
            }
        }
    }
}

fn kept_good(kept: &Option<(Instant, bool, Value)>) -> Option<Value> {
    kept.as_ref()
        .filter(|(_, good, _)| *good)
        .map(|(_, _, v)| v.clone())
}

/// The forge's releases as the page reads them: published ones tagged
/// r<N>, newest number first
fn shape(list: &[Value]) -> Vec<Value> {
    let mut out: Vec<(u64, Value)> = list
        .iter()
        .filter(|r| r["draft"] != true && r["prerelease"] != true)
        .filter_map(|r| {
            let n: u64 = r["tag_name"].as_str()?.strip_prefix('r')?.parse().ok()?;
            let name = r["name"].as_str().unwrap_or("");
            let title = name
                .split_once(" · ")
                .map(|(_, t)| t.trim())
                .filter(|t| !t.is_empty());
            let notes: Vec<&str> = r["body"]
                .as_str()
                .unwrap_or("")
                .lines()
                .filter_map(|l| l.trim().strip_prefix("- "))
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(6)
                .collect();
            let at = r["published_at"]
                .as_str()
                .or(r["created_at"].as_str())
                .and_then(unix);
            Some((
                n,
                json!({ "n": n, "at": at, "title": title, "notes": notes }),
            ))
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.0));
    out.into_iter().take(FEW).map(|(_, v)| v).collect()
}

/// RFC 3339 as the forge writes it (2026-10-21T16:25:25-04:00, or Z), in
/// seconds since 1970
fn unix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // fractions of a second, then the offset
    let mut i = 19;
    if b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    let offset = match s.get(i..)? {
        "Z" | "z" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) && &o[3..4] == ":" => {
            let v = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if o.starts_with('-') { -v } else { v }
        }
        _ => return None,
    };
    // days from 1970-01-01 to the date (Howard Hinnant's days_from_civil)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forge_and_repository_are_the_member_lists() {
        let n = Notes::from_member_list(
            "https://git.commonty.org/david/commonty/raw/branch/releases/members.json",
        )
        .unwrap();
        assert_eq!(
            n.url,
            "https://git.commonty.org/api/v1/repos/david/commonty/releases?limit=5"
        );
        for bad in [
            "",
            "http://git.commonty.org/david/commonty/raw/branch/releases/members.json",
            "https://git.commonty.org/david/commonty/raw/branch/main/members.json",
            "https://git.commonty.org/david/../raw/branch/releases/members.json",
            "https://git.commonty.org/a/b/c/raw/branch/releases/members.json",
        ] {
            assert!(Notes::from_member_list(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn times_are_read_with_their_offset() {
        assert_eq!(unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix("2026-10-21T20:25:25Z"), Some(1_792_614_325));
        assert_eq!(unix("2026-10-21T16:25:25-04:00"), Some(1_792_614_325));
        assert_eq!(unix("2026-10-21T20:25:25.123+00:00"), Some(1_792_614_325));
        assert_eq!(unix("2026-10-21 20:25:25"), None);
        assert_eq!(unix("2026-13-21T20:25:25Z"), None);
    }

    #[test]
    fn releases_are_shaped_newest_first() {
        let forge = json!([
            {"tag_name": "r311", "name": "311 · 2 changes", "draft": false, "prerelease": false,
             "published_at": "2026-10-20T10:00:00Z",
             "body": "Release 311, signed on the laptop and pulled by the boxes.\n\n### Fixes\n- a thing (#380)\n- another (#381)\n"},
            {"tag_name": "r312", "name": "312 · links stay in their tab", "published_at": "2026-10-21T10:00:00Z",
             "body": "Release 312.\n\n### Fixes\n- links stay in their tab (#391)\n"},
            {"tag_name": "r313", "name": "313", "draft": true, "body": ""},
            {"tag_name": "v0.1.60", "name": "the app", "body": "- not a box release"},
        ]);
        let got = shape(forge.as_array().unwrap());
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["n"], 312);
        assert_eq!(got[0]["title"], "links stay in their tab");
        assert_eq!(got[0]["notes"], json!(["links stay in their tab (#391)"]));
        assert_eq!(got[0]["at"], unix("2026-10-21T10:00:00Z").unwrap());
        assert_eq!(got[1]["n"], 311);
        assert_eq!(got[1]["notes"].as_array().unwrap().len(), 2);
    }
}
