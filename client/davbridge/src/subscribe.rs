//! A calendar from a link: Google's or Outlook's private address of a
//! calendar (or any .ics on the web), fetched by the computer it was added
//! on and kept as one of the member's calendars, read only. The link is in
//! that computer's own list (the app's data folder), never in the library,
//! where a member who may write a shared calendar could put one; the
//! calendar's folder has a `source` marker that makes it read only, and
//! `source-state`, each file's sha256, so a refresh writes only what
//! changed and takes away what the link dropped.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// the largest calendar a link may give
const MAX: usize = 20 << 20;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// the most events a link may hold, and the most it may come to written
/// out: each zone goes into each file that names it, so the written files
/// can be larger than what was fetched
const MAX_EVENTS: usize = 20_000;
const MAX_WRITTEN: usize = 64 << 20;

/// The time zones a calendar's lines name (TZID=...), as the zones' own
/// TZID lines write them.
fn zones_named(lines: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for l in lines.split("\r\n") {
        let head = l.split(':').next().unwrap_or("");
        for p in head.split(';').skip(1) {
            if let Some(v) = p.strip_prefix("TZID=").or_else(|| p.strip_prefix("tzid=")) {
                out.insert(v.trim_matches('"').to_string());
            }
        }
    }
    out
}

/// One calendar file per event (UID): its changed occurrences beside it,
/// and the time zones it names, as the pages and programs keep them. File
/// names are from the UID, so the same event is the same file each time.
pub fn split(ics: &str) -> Result<Vec<(String, String)>> {
    let text = ics.replace("\r\n", "\n");
    // unfolded only to find UIDs; the lines are kept as they came
    let mut head: Vec<&str> = Vec::new();
    let mut zones: BTreeMap<String, String> = BTreeMap::new();
    let mut events: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut depth = 0;
    let mut inside: Option<&str> = None;
    for line in text.lines() {
        let upper = line.to_ascii_uppercase();
        if let Some(what) = upper.strip_prefix("BEGIN:") {
            depth += 1;
            if depth == 2 {
                inside = Some(match what.trim() {
                    "VTIMEZONE" => "zone",
                    "VEVENT" | "VTODO" | "VJOURNAL" => "event",
                    _ => "other",
                });
                cur.clear();
            }
            if depth >= 2 {
                cur.push(line);
            }
            continue;
        }
        if upper.starts_with("END:") {
            if depth >= 2 {
                cur.push(line);
            }
            if depth == 2 {
                let block = cur.join("\r\n");
                match inside {
                    Some("zone") => {
                        let id = cur
                            .iter()
                            .find_map(|l| l.strip_prefix("TZID:"))
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        zones.insert(id, block);
                    }
                    Some("event") => {
                        let uid =
                            uid_of(&cur).unwrap_or_else(|| hex(&Sha256::digest(block.as_bytes())));
                        events.entry(uid).or_default().push(block);
                    }
                    _ => {}
                }
                inside = None;
            }
            depth -= 1;
            continue;
        }
        if depth == 1 && inside.is_none() {
            let u = upper.split([':', ';']).next().unwrap_or("");
            if matches!(u, "VERSION" | "PRODID" | "CALSCALE" | "METHOD")
                || u.starts_with("X-WR-TIMEZONE")
            {
                head.push(line);
            }
        } else if depth >= 2 {
            cur.push(line);
        }
    }
    let head = if head
        .iter()
        .any(|l| l.to_ascii_uppercase().starts_with("VERSION"))
    {
        head.join("\r\n")
    } else {
        format!("VERSION:2.0\r\n{}", head.join("\r\n"))
    };
    if events.len() > MAX_EVENTS {
        bail!("the calendar at the link has over {MAX_EVENTS} events");
    }
    let mut written = 0;
    let mut out = Vec::with_capacity(events.len());
    for (uid, blocks) in events {
        let name = format!("{}.ics", &hex(&Sha256::digest(uid.as_bytes()))[..32]);
        let body = blocks.join("\r\n");
        let mut f = String::from("BEGIN:VCALENDAR\r\n");
        f.push_str(head.trim_end());
        f.push_str("\r\n");
        // only the zones this event names
        for id in zones_named(&body) {
            if let Some(z) = zones.get(&id) {
                f.push_str(z);
                f.push_str("\r\n");
            }
        }
        f.push_str(&body);
        f.push_str("\r\nEND:VCALENDAR\r\n");
        written += f.len();
        if written > MAX_WRITTEN {
            bail!(
                "the calendar at the link comes to over {} MiB",
                MAX_WRITTEN >> 20
            );
        }
        out.push((name, f));
    }
    Ok(out)
}

fn uid_of(lines: &[&str]) -> Option<String> {
    // a UID folded over lines is joined back
    let mut out: Option<String> = None;
    for l in lines {
        if let Some(o) = out.as_mut() {
            if let Some(rest) = l.strip_prefix(' ').or_else(|| l.strip_prefix('\t')) {
                o.push_str(rest);
                continue;
            }
            break;
        }
        if l.to_ascii_uppercase().starts_with("UID") {
            out = l.split_once(':').map(|(_, v)| v.to_string());
        }
    }
    out.map(|u| u.trim().to_string()).filter(|u| !u.is_empty())
}

/// A link as fetched: webcal:// is https://, and nothing else but https.
pub fn link(url: &str) -> Result<String> {
    let u = url.trim();
    let u = match u
        .strip_prefix("webcal://")
        .or_else(|| u.strip_prefix("webcals://"))
    {
        Some(rest) => format!("https://{rest}"),
        None => u.to_string(),
    };
    if !u.starts_with("https://") {
        bail!("a calendar link starts with https:// or webcal://");
    }
    Ok(u)
}

/// An address the fetch may reach: on the internet, not this computer,
/// its network or the fleet's (a link is something a person typed, and a
/// calendar's reply is written where others may read it).
pub fn public(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_loopback()
                // benchmarking, and the old 6to4 relays
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || (o[0] == 192 && o[1] == 88 && o[2] == 99)
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_documentation()
                || a.is_multicast()
                || o[0] == 0
                // carrier-grade NAT, where the tailnet's addresses are
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || o[0] >= 240)
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let s = a.segments();
            // global unicast only (2000::/3), less the parts of it that
            // carry or reach another address: 6to4, Teredo, documentation
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && s[1] == 0)
                && !(s[0] == 0x2001 && s[1] == 0x0db8)
        }
    }
}

/// The link's body, at most MAX, read as it comes. Each hop's host is
/// looked up here, refused unless every address is public, and the
/// connection made to the address checked; a redirect is followed by hand,
/// five at most, checked the same way.
pub async fn fetch(url: &str) -> Result<String> {
    fetch_under(url, &INTERNET).await
}

/// What a fetch may reach: which addresses, and whether plain http is
/// taken (only a test's local server, never a link)
struct Reach {
    allows: fn(std::net::IpAddr) -> bool,
    http: bool,
}

const INTERNET: Reach = Reach {
    allows: public,
    http: false,
};

async fn fetch_under(url: &str, reach: &Reach) -> Result<String> {
    let checked = |u: &str| -> Result<String> {
        match link(u) {
            Ok(l) => Ok(l),
            Err(_) if reach.http && u.starts_with("http://") => Ok(u.to_string()),
            Err(e) => Err(e),
        }
    };
    let mut at = checked(url)?;
    let mut hops = 0;
    let mut r = loop {
        let u = reqwest::Url::parse(&at).context("not a link")?;
        let host = u.host_str().context("a link names a host")?.to_string();
        // an address in brackets is looked up without them
        let bare = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let port = u.port_or_known_default().unwrap_or(443);
        let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((bare.as_str(), port))
            .await
            .with_context(|| format!("looking up {host}"))?
            .collect();
        if addrs.is_empty() || !addrs.iter().all(|a| (reach.allows)(a.ip())) {
            bail!("{host} is not on the internet");
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            // a proxy would connect where it likes, past the check above
            .no_proxy()
            .resolve(&host, addrs[0])
            .build()?;
        // a private link is a secret: no error of the fetch names it
        let r = client
            .get(u)
            .header("accept", "text/calendar, */*")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!(e.without_url()))?;
        if r.status().is_redirection() {
            hops += 1;
            if hops > 5 {
                bail!("the link sent us on too many times");
            }
            let to = r
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .context("a redirect with nowhere to go")?;
            at = checked(reqwest::Url::parse(&at)?.join(to)?.as_str())?;
            continue;
        }
        break r;
    };
    if !r.status().is_success() {
        bail!("the link answered {}", r.status());
    }
    if r.content_length().is_some_and(|n| n as usize > MAX) {
        bail!("the calendar at the link is over {} MiB", MAX >> 20);
    }
    let mut b = Vec::new();
    while let Some(chunk) = r
        .chunk()
        .await
        .map_err(|e| anyhow::anyhow!(e.without_url()))?
    {
        if b.len() + chunk.len() > MAX {
            bail!("the calendar at the link is over {} MiB", MAX >> 20);
        }
        b.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&b).into_owned();
    if !text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_uppercase()
        .starts_with("BEGIN:VCALENDAR")
    {
        bail!("the link does not give a calendar");
    }
    Ok(text)
}

/// what to write and take away, against what was written last
pub struct Plan {
    pub write: Vec<(String, String)>,
    pub remove: Vec<String>,
    pub state: BTreeMap<String, String>,
}

pub fn plan(files: Vec<(String, String)>, before: &BTreeMap<String, String>) -> Plan {
    let mut state = BTreeMap::new();
    let mut write = Vec::new();
    for (name, text) in files {
        let h = hex(&Sha256::digest(text.as_bytes()));
        if before.get(&name) != Some(&h) {
            write.push((name.clone(), text));
        }
        state.insert(name, h);
    }
    let remove = before
        .keys()
        .filter(|n| !state.contains_key(*n))
        .cloned()
        .collect();
    Plan {
        write,
        remove,
        state,
    }
}

pub fn read_state(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(n, h)| (n.to_string(), h.to_string()))
        .collect()
}

pub fn write_state(state: &BTreeMap<String, String>) -> String {
    state.iter().map(|(n, h)| format!("{n} {h}\n")).collect()
}

/// Every calendar of the member's own libraries that has a link: fetched,
/// and what changed written. A link that fails leaves its calendar as it
/// was, said in the answer. Returns (calendar, events written, taken away)
/// or the reason, per calendar.
pub async fn refresh_all<S: crate::gate::Source>(
    store: &crate::gate::GateStore<S>,
) -> Vec<(String, Result<(usize, usize)>)> {
    let mut out = Vec::new();
    let Ok(found) = store.linked().await else {
        return out;
    };
    for (g, folder, url) in found {
        let r = async {
            let text = fetch(&url).await?;
            let before = match g.get_bytes(&format!("{folder}/source-state")).await? {
                Some((b, _)) => read_state(&String::from_utf8_lossy(&b)),
                None => BTreeMap::new(),
            };
            let p = plan(split(&text)?, &before);
            for (name, body) in &p.write {
                g.put_bytes(
                    &format!("{folder}/{name}"),
                    body.as_bytes(),
                    &crate::Version::Any,
                )
                .await
                .with_context(|| format!("writing {name}"))?;
            }
            for name in &p.remove {
                g.trash_if(&format!("{folder}/{name}"), &crate::Version::Any)
                    .await?;
            }
            // nothing changed: nothing written, so no device's trash fills
            if p.write.is_empty() && p.remove.is_empty() && p.state == before {
                return Ok((0, 0));
            }
            g.put_bytes(
                &format!("{folder}/source-state"),
                write_state(&p.state).as_bytes(),
                &crate::Version::Any,
            )
            .await?;
            Ok((p.write.len(), p.remove.len()))
        }
        .await;
        out.push((folder, r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOGLE: &str = "BEGIN:VCALENDAR\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nVERSION:2.0\r\nCALSCALE:GREGORIAN\r\nX-WR-CALNAME:Work\r\nX-WR-TIMEZONE:America/New_York\r\nBEGIN:VTIMEZONE\r\nTZID:America/New_York\r\nBEGIN:STANDARD\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nDTSTART:19701101T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nDTSTART;TZID=America/New_York:20261019T090000\r\nRRULE:FREQ=WEEKLY\r\nUID:abc123@google.com\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART;TZID=America/New_York:20261021T100000\r\nRECURRENCE-ID;TZID=America/New_York:20261021T090000\r\nUID:abc123@google.com\r\nSUMMARY:Standup (late)\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261025\r\nUID:very-long-uid-that-is-folded-over-two-lin\r\n es@example.com\r\nSUMMARY:Day off\r\nBEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn a_link_becomes_one_file_per_event_with_its_zones() {
        let files = split(GOOGLE).unwrap();
        assert_eq!(
            files.len(),
            2,
            "the series and its changed one are one file"
        );
        let standup = files.iter().find(|(_, t)| t.contains("Standup")).unwrap();
        assert!(standup.1.contains("Standup (late)"));
        assert!(standup.1.contains("BEGIN:VTIMEZONE"));
        assert!(standup.1.starts_with("BEGIN:VCALENDAR\r\nPRODID:-//Google"));
        assert!(standup.1.ends_with("END:VCALENDAR\r\n"));
        assert!(!standup.1.contains("X-WR-CALNAME"));
        let day = files.iter().find(|(_, t)| t.contains("Day off")).unwrap();
        assert!(day.1.contains("BEGIN:VALARM") && day.1.contains("END:VALARM"));
        // the name comes from the whole UID, folded or not, the same each time
        let again = split(GOOGLE).unwrap();
        assert_eq!(files, again);
        let folded_uid = "very-long-uid-that-is-folded-over-two-lines@example.com";
        assert_eq!(
            day.0,
            format!("{}.ics", &hex(&Sha256::digest(folded_uid.as_bytes()))[..32])
        );
    }

    #[test]
    fn a_zone_goes_only_where_it_is_named_and_the_whole_is_capped() {
        // a zone no event names is in no file
        let spare = GOOGLE.replace(
            "BEGIN:VTIMEZONE",
            "BEGIN:VTIMEZONE\r\nTZID:Europe/Unused\r\nEND:VTIMEZONE\r\nBEGIN:VTIMEZONE",
        );
        for (_, f) in split(&spare).unwrap() {
            assert!(!f.contains("Europe/Unused"), "{f}");
        }
        let files = split(GOOGLE).unwrap();
        let day = files.iter().find(|(_, t)| t.contains("Day off")).unwrap();
        assert!(
            !day.1.contains("BEGIN:VTIMEZONE"),
            "a day's event names no zone"
        );
        // a big zone named by many small events would be copied into each
        let big_zone = format!(
            "BEGIN:VTIMEZONE\r\nTZID:Big\r\nX-PAD:{}\r\nEND:VTIMEZONE\r\n",
            "x".repeat(1 << 20)
        );
        let mut ics = format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{big_zone}");
        for i in 0..80 {
            ics.push_str(&format!(
                "BEGIN:VEVENT\r\nUID:e{i}\r\nDTSTART;TZID=Big:20261019T090000\r\nEND:VEVENT\r\n"
            ));
        }
        ics.push_str("END:VCALENDAR\r\n");
        let e = split(&ics).unwrap_err();
        assert!(e.to_string().contains("over 64 MiB"), "{e:#}");
        // and too many events
        let mut ics = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
        for i in 0..=MAX_EVENTS {
            ics.push_str(&format!("BEGIN:VEVENT\r\nUID:n{i}\r\nEND:VEVENT\r\n"));
        }
        ics.push_str("END:VCALENDAR\r\n");
        assert!(split(&ics).is_err());
    }

    #[test]
    fn a_refresh_writes_what_changed_and_takes_away_what_went() {
        let first = plan(split(GOOGLE).unwrap(), &BTreeMap::new());
        assert_eq!((first.write.len(), first.remove.len()), (2, 0));
        let same = plan(split(GOOGLE).unwrap(), &first.state);
        assert_eq!((same.write.len(), same.remove.len()), (0, 0));
        let changed = GOOGLE.replace("SUMMARY:Day off", "SUMMARY:Day off, moved");
        let p = plan(split(&changed).unwrap(), &first.state);
        assert_eq!((p.write.len(), p.remove.len()), (1, 0));
        let only = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
        let p = plan(split(only).unwrap(), &first.state);
        assert_eq!((p.write.len(), p.remove.len()), (0, 2));
        assert_eq!(read_state(&write_state(&first.state)), first.state);
    }

    /// a server on this computer: a calendar, redirects, and calendars too
    /// big, as a link could give them
    async fn server() -> String {
        use axum::body::Body;
        use axum::response::IntoResponse as _;
        let small = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
        let app = axum::Router::new()
            .route("/cal", axum::routing::get(move || async move { small }))
            .route(
                "/here",
                axum::routing::get(|| async { axum::response::Redirect::temporary("/cal") }),
            )
            .route(
                "/away",
                axum::routing::get(|| async {
                    axum::response::Redirect::temporary("http://127.0.0.2:9/cal")
                }),
            )
            .route(
                "/big",
                axum::routing::get(|| async {
                    // no length said: the cap is only found by counting
                    let chunk = bytes_of(1 << 20);
                    let s = futures_util::stream::iter(
                        (0..30).map(move |_| Ok::<_, std::io::Error>(chunk.clone())),
                    );
                    Body::from_stream(s).into_response()
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
        format!("http://{at}")
    }

    /// a server that says its calendar is 40 MiB, written by hand: hyper
    /// would not send a length its body does not have
    async fn says_big() -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = l.accept().await {
                let mut buf = [0u8; 4096];
                let _ = c.read(&mut buf).await;
                let _ = c
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 41943040\r\n\r\nBEGIN:VCALENDAR\r\n",
                    )
                    .await;
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
        format!("http://{at}")
    }

    fn bytes_of(n: usize) -> axum::body::Bytes {
        let mut v = b"BEGIN:VCALENDAR\r\n".to_vec();
        v.resize(n, b'x');
        v.into()
    }

    /// only this computer's 127.0.0.1, as a test stands in for the internet
    const HERE: Reach = Reach {
        allows: |ip| ip == std::net::IpAddr::from([127, 0, 0, 1]),
        http: true,
    };

    #[tokio::test]
    async fn a_redirect_is_checked_like_the_link() {
        let at = server().await;
        assert!(fetch_under(&format!("{at}/cal"), &HERE).await.is_ok());
        assert!(fetch_under(&format!("{at}/here"), &HERE).await.is_ok());
        let e = fetch_under(&format!("{at}/away"), &HERE).await.unwrap_err();
        assert!(e.to_string().contains("not on the internet"), "{e:#}");
        // and the link itself, to this computer, as a link is checked
        let e = fetch(&format!("{at}/cal")).await.unwrap_err();
        assert!(e.to_string().contains("https://"), "{e:#}");
        let e = fetch("https://127.0.0.1:9/cal").await.unwrap_err();
        assert!(e.to_string().contains("not on the internet"), "{e:#}");
        let e = fetch("https://[::1]:9/cal").await.unwrap_err();
        assert!(e.to_string().contains("not on the internet"), "{e:#}");
    }

    #[tokio::test]
    async fn a_calendar_too_big_is_stopped_as_it_comes() {
        let at = server().await;
        let e = fetch_under(&format!("{at}/big"), &HERE).await.unwrap_err();
        assert!(e.to_string().contains("over 20 MiB"), "{e:#}");
        let e = fetch_under(&format!("{}/cal", says_big().await), &HERE)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("over 20 MiB"), "{e:#}");
    }

    #[test]
    fn only_the_internet_is_fetched() {
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.20",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.7",
            "0.0.0.0",
            "::1",
            "fd7a:115c:a1e0::1",
            "fe80::1",
            "::ffff:192.168.1.1",
            "198.18.0.1",
            "192.88.99.1",
            "2002:c0a8:0101::1",
            "2001:0:4136:e378::1",
            "64:ff9b::c0a8:101",
            "::c0a8:101",
            "fec0::1",
            "100::1",
            "2001:db8::1",
        ] {
            assert!(!public(bad.parse().unwrap()), "{bad}");
        }
        for good in ["142.250.80.46", "2607:f8b0:4004:c07::64", "1.1.1.1"] {
            assert!(public(good.parse().unwrap()), "{good}");
        }
    }

    #[test]
    fn links() {
        assert_eq!(
            link("webcal://p01-calendars.icloud.com/x").unwrap(),
            "https://p01-calendars.icloud.com/x"
        );
        assert_eq!(
            link(" https://calendar.google.com/calendar/ical/x/private-y/basic.ics ").unwrap(),
            "https://calendar.google.com/calendar/ical/x/private-y/basic.ics"
        );
        assert!(link("file:///etc/passwd").is_err());
        assert!(link("http://example.com/cal.ics").is_err());
        assert!(link("javascript:x").is_err());
    }
}
