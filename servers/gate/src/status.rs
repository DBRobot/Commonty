//! Whether the services members use are working, for the public uptime
//! page: uptime alone, no release or box details. Each box notes its own services every minute (dd_service_up,
//! modules/observe/uptime.nix); this reads those notes from thanos on this
//! box and answers per service. What it answers names no box, address,
//! place or person: the labels are this file's, never a box's words.

use std::time::{Duration, Instant};

use serde::Serialize;

/// the services, in the page's order, with the words it shows; a service
/// a box reports that is not here is not shown
pub const SERVICES: &[(&str, &str)] = &[
    ("site", "Website and sign-in"),
    ("files", "Files"),
    ("movies", "Movies & TV"),
    ("photos", "Photos"),
    ("passwords", "Passwords"),
    ("chat", "Chat"),
    ("games", "Games"),
    ("git", "Git"),
];

/// how many days of history the page draws
pub const DAYS: u64 = 90;
const DAY: u64 = 86_400;
/// the history's windows: each one is the share of its quarter hour up
const STEP: u64 = 900;
/// another box's newest notes reach thanos with its next block, up to two
/// hours late: windows this recent after a service's last note are unknown,
/// not down
const LATE: u64 = 3 * 3600;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Service {
    pub id: &'static str,
    pub label: &'static str,
    pub up: bool,
    /// when it last became what it is now, as far as the notes go back
    pub since: Option<u64>,
    /// each day's share up, oldest first, from `start`; null where nothing
    /// was measured
    pub days: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub services: Vec<Service>,
    pub checked: u64,
    /// the first day's midnight (UTC)
    pub start: u64,
}

/// one series per service: (time, value), oldest first
type Series = Vec<(String, Vec<(u64, f64)>)>;

#[derive(Default)]
pub struct Cache {
    now: tokio::sync::Mutex<Option<(Instant, Status)>>,
    history: tokio::sync::Mutex<Option<(Instant, Series)>>,
}

/// the status is worked out again at most this often, whoever asks
const KEEP: Duration = Duration::from_secs(60);
const KEEP_HISTORY: Duration = Duration::from_secs(600);

fn unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn known(id: &str) -> Option<(&'static str, &'static str)> {
    SERVICES.iter().copied().find(|(k, _)| *k == id)
}

async fn ask(thanos: &str, path: &str, params: &[(&str, String)]) -> Option<serde_json::Value> {
    let r = crate::http()
        .get(format!("{}/api/v1/{path}", thanos.trim_end_matches('/')))
        .timeout(Duration::from_secs(20))
        .query(params)
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    let v: serde_json::Value = r.json().await.ok()?;
    (v["status"] == "success").then(|| v["data"]["result"].clone())
}

fn number(v: &serde_json::Value) -> Option<(u64, f64)> {
    let t = v[0].as_f64()?;
    let x = v[1].as_str()?.parse::<f64>().ok()?;
    (t >= 0.0 && x.is_finite()).then_some((t as u64, x))
}

/// which boxes may say whether each service works: those the release
/// builds with it (VERIFY_SERVICE_BOXES). Of those, a placed service is
/// noted only by the box the plan runs it on (modules/observe/uptime.nix).
pub type Speakers = std::collections::BTreeMap<String, Vec<String>>;

/// A vector or matrix answer, one series per service and box, as one
/// series per service: only services we know, only from a box the release
/// builds with that service, and the worse of those boxes at each moment.
/// Another box's word for a service it is not built with counts for nothing.
fn series(result: &serde_json::Value, speakers: &Speakers) -> Series {
    let mut by: std::collections::BTreeMap<&'static str, std::collections::BTreeMap<u64, f64>> =
        Default::default();
    for s in result.as_array().into_iter().flatten() {
        let Some((id, _)) = s["metric"]["service"].as_str().and_then(known) else {
            continue;
        };
        let from = s["metric"]["box"].as_str().unwrap_or_default();
        if !speakers
            .get(id)
            .is_some_and(|b| b.iter().any(|b| b == from))
        {
            continue;
        }
        let points: Vec<(u64, f64)> = match s["values"].as_array() {
            Some(vs) => vs.iter().filter_map(number).collect(),
            None => number(&s["value"]).into_iter().collect(),
        };
        let all = by.entry(id).or_default();
        for (t, v) in points {
            all.entry(t).and_modify(|w| *w = w.min(v)).or_insert(v);
        }
    }
    by.into_iter()
        .map(|(id, p)| (id.to_string(), p.into_iter().collect()))
        .collect()
}

const NOW: &str = "min by (service, box) (dd_service_up)";
/// the last few hours by the minute, for when a change happened
const RECENT: u64 = 6 * 3600;
const MINUTE: u64 = 60;
const WINDOWS: &str = "min by (service, box) (avg_over_time(dd_service_up[15m]))";

/// the first midnight (UTC) the history covers, for `now`
pub fn first_day(now: u64) -> u64 {
    now / DAY * DAY - (DAYS - 1) * DAY
}

/// Each day's share up, from the quarter hours' shares (a point at t is the
/// quarter hour before t). Before a service was first noted there is
/// nothing to say; after it, a quarter hour with no note counts as down
/// (its box, or the box's measuring, was off), except the last few hours
/// after its newest note, which may only be late.
pub fn days(points: &[(u64, f64)], start: u64, now: u64) -> Vec<Option<f64>> {
    let now = now / STEP * STEP;
    let Some(first) = points.first().map(|p| p.0) else {
        return vec![None; DAYS as usize];
    };
    let last = points.last().map(|p| p.0).unwrap_or(first);
    let until = if now.saturating_sub(last) <= LATE {
        last
    } else {
        now
    };
    (0..DAYS)
        .map(|d| {
            let lo = start + d * DAY;
            let hi = (lo + DAY).min(until);
            // the windows ending in (lo, hi] that come after the first note
            let from = lo.max(first.saturating_sub(STEP));
            if hi <= from {
                return None;
            }
            let expected = (hi - from) / STEP;
            if expected == 0 {
                return None;
            }
            let got: f64 = points
                .iter()
                .filter(|(t, _)| *t > from && *t <= hi)
                .map(|(_, v)| v.clamp(0.0, 1.0))
                .sum();
            Some((got / expected as f64).clamp(0.0, 1.0))
        })
        .collect()
}

/// When a service became what it is now: the end of the last window, of
/// `step` seconds each, that was otherwise. A window partly down breaks a
/// run of up, partly up a run of down, and a missing one a run of up.
/// None: no window was otherwise, as far as the notes go back.
pub fn since(up: bool, windows: &[(u64, f64)], step: u64) -> Option<u64> {
    let mut next: Option<u64> = None;
    for &(t, v) in windows.iter().rev() {
        let same = if up { v >= 1.0 } else { v <= 0.0 };
        let gap = next.is_some_and(|n| n - t > step + step / 2);
        if !same || (gap && up) {
            return Some(next.map_or(t, |n| n - step));
        }
        next = Some(t);
    }
    None
}

impl Cache {
    async fn windows(&self, thanos: &str, speakers: &Speakers, now: u64) -> Option<Series> {
        let mut h = self.history.lock().await;
        if let Some((at, s)) = h.as_ref()
            && at.elapsed() < KEEP_HISTORY
        {
            return Some(s.clone());
        }
        let start = first_day(now) + STEP;
        let end = now / STEP * STEP;
        let got = ask(
            thanos,
            "query_range",
            &[
                ("query", WINDOWS.to_string()),
                ("start", start.to_string()),
                ("end", end.to_string()),
                ("step", STEP.to_string()),
                ("partial_response", "true".into()),
            ],
        )
        .await
        .map(|r| series(&r, speakers));
        match got {
            Some(s) => {
                *h = Some((Instant::now(), s.clone()));
                Some(s)
            }
            None => h.as_ref().map(|(_, s)| s.clone()),
        }
    }

    /// what the page shows, worked out at most once a minute; the last good
    /// answer while thanos does not answer; None when there never was one
    pub async fn status(&self, thanos: Option<&str>, speakers: &Speakers) -> Option<Status> {
        let mut g = self.now.lock().await;
        if let Some((at, s)) = g.as_ref()
            && at.elapsed() < KEEP
        {
            return Some(s.clone());
        }
        let fresh = match thanos {
            Some(t) => self.work_out(t, speakers).await,
            None => None,
        };
        if let Some(s) = fresh {
            *g = Some((Instant::now(), s));
        }
        g.as_ref().map(|(_, s)| s.clone())
    }

    async fn work_out(&self, thanos: &str, speakers: &Speakers) -> Option<Status> {
        let now = unix();
        let end = now / MINUTE * MINUTE;
        let now_q = [
            ("query", NOW.to_string()),
            ("partial_response", "true".into()),
        ];
        let recent_q = [
            ("query", NOW.to_string()),
            ("start", (end - RECENT).to_string()),
            ("end", end.to_string()),
            ("step", MINUTE.to_string()),
            ("partial_response", "true".into()),
        ];
        let (current, recent, windows) = tokio::join!(
            ask(thanos, "query", &now_q),
            ask(thanos, "query_range", &recent_q),
            self.windows(thanos, speakers, now),
        );
        let recent = recent.map(|r| series(&r, speakers)).unwrap_or_default();
        Some(assemble(
            &series(&current?, speakers),
            &recent,
            &windows?,
            now,
        ))
    }
}

/// the page's answer from the notes: every service noted in the history or
/// now, in our order. When it changed comes from the last hours by the
/// minute, else from the quarter hours.
pub fn assemble<'a>(
    current: &'a Series,
    recent: &'a Series,
    windows: &'a Series,
    now: u64,
) -> Status {
    let start = first_day(now);
    let services = SERVICES
        .iter()
        .filter_map(|&(id, label)| {
            let now_v = current
                .iter()
                .find(|(k, _)| k == id)
                .and_then(|(_, p)| p.last().map(|x| x.1));
            let of = |s: &'a Series| {
                s.iter()
                    .find(|(k, _)| k == id)
                    .map(|(_, p)| p.as_slice())
                    .unwrap_or_default()
            };
            let (w, r) = (of(windows), of(recent));
            if now_v.is_none() && w.is_empty() {
                return None;
            }
            // a service noted before and silent now is not working
            let up = now_v.is_some_and(|v| v >= 1.0);
            Some(Service {
                id,
                label,
                up,
                since: since(up, r, MINUTE)
                    .or_else(|| since(up, w, STEP))
                    .or_else(|| w.first().map(|p| p.0 - STEP)),
                days: days(w, start, now),
            })
        })
        .collect();
    Status {
        services,
        checked: now,
        start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // midday, UTC
    const NOW: u64 = 1_791_035_600;

    fn quarter_hours(from: u64, to: u64, v: f64) -> Vec<(u64, f64)> {
        (from / STEP + 1..=to / STEP)
            .map(|i| (i * STEP, v))
            .collect()
    }

    #[test]
    fn a_day_up_throughout_is_all_up_and_a_day_before_any_note_is_unknown() {
        let start = first_day(NOW);
        let pts = quarter_hours(start + 10 * DAY, NOW, 1.0);
        let d = days(&pts, start, NOW);
        assert_eq!(d.len(), DAYS as usize);
        assert!(d[..10].iter().all(Option::is_none), "{d:?}");
        assert!(d[10..].iter().all(|x| *x == Some(1.0)), "{d:?}");
    }

    #[test]
    fn a_missing_quarter_hour_counts_as_down_and_a_half_down_one_as_half() {
        let start = first_day(NOW);
        let lo = start + 5 * DAY;
        let mut pts = quarter_hours(lo - DAY, NOW, 1.0);
        // day 5: 96 quarter hours, 4 missing and 2 half up
        pts.retain(|(t, _)| !(*t > lo && *t <= lo + 4 * STEP));
        for p in pts
            .iter_mut()
            .filter(|(t, _)| *t > lo + 10 * STEP && *t <= lo + 12 * STEP)
        {
            p.1 = 0.5;
        }
        let d = days(&pts, start, NOW);
        let got = d[5].unwrap();
        assert!((got - 91.0 / 96.0).abs() < 1e-9, "{got}");
        assert_eq!(d[4], Some(1.0));
        assert_eq!(d[6], Some(1.0));
    }

    #[test]
    fn todays_share_is_of_the_day_so_far_and_late_notes_are_not_down() {
        let start = first_day(NOW);
        let today = NOW / DAY * DAY;
        // notes stop an hour ago: another box's block not in yet
        let pts = quarter_hours(start, NOW - 3600, 1.0);
        let d = days(&pts, start, NOW);
        assert_eq!(*d.last().unwrap(), Some(1.0));
        // stopped five hours ago: that is down
        let pts = quarter_hours(start, NOW - 5 * 3600, 1.0);
        let d = days(&pts, start, NOW);
        let so_far = (NOW / STEP * STEP - today) / STEP;
        let up = (NOW - 5 * 3600) / STEP * STEP;
        let want = ((up - today) / STEP) as f64 / so_far as f64;
        assert!((d.last().unwrap().unwrap() - want).abs() < 1e-9);
    }

    #[test]
    fn since_is_after_the_last_window_that_was_otherwise() {
        let mut pts = quarter_hours(NOW - DAY, NOW, 1.0);
        let n = pts.len();
        pts[n - 10].1 = 0.7;
        assert_eq!(since(true, &pts, STEP), Some(pts[n - 10].0));
        // down now, down for the last 3 windows
        for p in &mut pts[n - 3..] {
            p.1 = 0.0;
        }
        assert_eq!(since(false, &pts, STEP), Some(pts[n - 4].0));
        // never otherwise: nothing to say
        let pts = quarter_hours(NOW - DAY, NOW, 1.0);
        assert_eq!(since(true, &pts, STEP), None);
        // a gap in the notes breaks a run of up
        let mut pts = quarter_hours(NOW - DAY, NOW, 1.0);
        pts.remove(pts.len() - 5);
        assert_eq!(since(true, &pts, STEP), Some(pts[pts.len() - 4].0 - STEP));
    }

    #[test]
    fn a_service_silent_now_is_down_and_one_never_noted_is_not_listed() {
        let windows = vec![("photos".to_string(), quarter_hours(NOW - DAY, NOW, 1.0))];
        let current = vec![("files".to_string(), vec![(NOW, 1.0)])];
        let s = assemble(&current, &Series::new(), &windows, NOW);
        let ids: Vec<_> = s.services.iter().map(|x| x.id).collect();
        assert_eq!(ids, ["files", "photos"]);
        assert!(s.services[0].up);
        assert!(!s.services[1].up);
    }

    #[test]
    fn only_services_we_name_are_read_and_their_labels_are_ours() {
        let r = serde_json::json!([
            {"metric": {"service": "photos", "box": "node1"}, "value": [NOW, "1"]},
            {"metric": {"service": "<b>node2</b>", "box": "node1"}, "value": [NOW, "1"]},
            {"metric": {"service": "git", "box": "node1"}, "values": [[NOW - 900, "1"], [NOW, "0"]]}
        ]);
        let s = series(&r, &speakers(&[("photos", "node1"), ("git", "node1")]));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], ("git".into(), vec![(NOW - 900, 1.0), (NOW, 0.0)]));
        let out = serde_json::to_string(&assemble(&s, &s, &s, NOW)).unwrap();
        assert!(!out.contains("node"), "{out}");
    }

    fn speakers(pairs: &[(&str, &str)]) -> Speakers {
        let mut out = Speakers::new();
        for (id, b) in pairs {
            out.entry(id.to_string()).or_default().push(b.to_string());
        }
        out
    }

    /// A box the release does not build with a service changes nothing about it,
    /// whatever it says, up or down; the boxes that do run it count, the
    /// worse of them at each moment.
    #[test]
    fn only_the_boxes_that_run_a_service_speak_for_it() {
        let who = speakers(&[
            ("photos", "node1"),
            ("movies", "node1"),
            ("movies", "node3"),
        ]);
        let honest = serde_json::json!([
            {"metric": {"service": "photos", "box": "node1"}, "values": [[NOW - 900, "1"], [NOW, "1"]]},
            {"metric": {"service": "movies", "box": "node1"}, "values": [[NOW - 900, "1"], [NOW, "1"]]},
            {"metric": {"service": "movies", "box": "node3"}, "values": [[NOW - 900, "1"], [NOW, "0.5"]]}
        ]);
        let want = series(&honest, &who);
        assert_eq!(
            want[0],
            ("movies".into(), vec![(NOW - 900, 1.0), (NOW, 0.5)])
        );
        for lie in ["0", "1"] {
            let mut told = honest.as_array().unwrap().clone();
            told.push(serde_json::json!({"metric": {"service": "photos", "box": "node2"}, "values": [[NOW - 900, lie], [NOW, lie]]}));
            told.push(serde_json::json!({"metric": {"service": "movies", "box": "node2"}, "values": [[NOW, lie]]}));
            told.push(serde_json::json!({"metric": {"service": "git", "box": "node2"}, "values": [[NOW, lie]]}));
            // no box label at all is no box that runs it either
            told.push(serde_json::json!({"metric": {"service": "photos"}, "values": [[NOW, lie]]}));
            let got = series(&serde_json::Value::Array(told), &who);
            assert_eq!(got, want, "a box not built with it said {lie}");
            assert_eq!(
                assemble(&got, &got, &got, NOW).services,
                assemble(&want, &want, &want, NOW).services
            );
        }
        // and with nobody named, nobody speaks
        assert!(series(&honest, &Speakers::new()).is_empty());
    }
}
