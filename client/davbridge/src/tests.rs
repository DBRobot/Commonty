//! The bridge as the programs that use it ask: discovery, a calendar's
//! items, multiget, a query, sync from a token, saves by version, and a
//! calendar shared to be seen and not changed.

use super::*;
use axum::http::Request as HttpRequest;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

const PW: &str = "s3cret-long-enough-to-be-real";
const HOST: &str = "127.0.0.1:41664";

/// a program's Digest answer to the bridge's challenge, as Thunderbird's
fn digest(pw: &str, method: &str, uri: &str, nonce: &str, alg: &str) -> String {
    let ha1 = hash(alg, &format!("{USER}:{REALM}:{pw}"));
    let ha2 = hash(alg, &format!("{method}:{uri}"));
    let r = hash(alg, &format!("{ha1}:{nonce}:00000001:c0ffee:auth:{ha2}"));
    format!(
        "Digest username=\"{USER}\", realm=\"{REALM}\", nonce=\"{nonce}\", uri=\"{uri}\", algorithm={alg}, response=\"{r}\", qop=auth, nc=00000001, cnonce=\"c0ffee\""
    )
}

/// a challenge's nonce, as a program gets one: by asking without a password
async fn fresh(app: &axum::Router) -> String {
    let r = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("PROPFIND")
                .uri("/dav/")
                .header("host", HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let c = r.headers()["www-authenticate"]
        .to_str()
        .unwrap()
        .to_string();
    c.split("nonce=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn ask(
    app: &axum::Router,
    method: &str,
    path: &str,
    hs: &[(&str, &str)],
    body: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut r = HttpRequest::builder()
        .method(method)
        .uri(path)
        .header("host", HOST)
        .header(
            "authorization",
            digest(PW, method, path, &fresh(app).await, "SHA-256"),
        );
    for (k, v) in hs {
        r = r.header(*k, *v);
    }
    let res = app
        .clone()
        .oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// a PROPFIND of /dav/ with these headers alone
async fn raw(app: &axum::Router, hs: &[(&str, &str)]) -> Response {
    let mut r = HttpRequest::builder().method("PROPFIND").uri("/dav/");
    for (k, v) in hs {
        r = r.header(*k, *v);
    }
    app.clone()
        .oneshot(r.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn setup() -> (axum::Router, Arc<Memory>) {
    let m = Arc::new(Memory::with(Kind::Calendars, "personal", "Personal", true));
    m.add(Kind::Calendars, "lingyun~abcd1234", "Lingyun's", false);
    m.add(Kind::Contacts, "contacts", "Contacts", true);
    (router(m.clone(), PW.into(), 41664), m)
}

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e1\r\nSUMMARY:Dentist & co <x>\r\nDTSTART:20261020T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[tokio::test]
async fn a_program_without_the_password_gets_nothing() {
    let (app, _) = setup();
    let r = raw(&app, &[("host", HOST)]).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let ch: Vec<String> = r
        .headers()
        .get_all("www-authenticate")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(
        ch.iter()
            .any(|c| c.starts_with("Digest") && c.contains("algorithm=SHA-256")),
        "{ch:?}"
    );
    assert!(ch.iter().any(|c| c.contains("algorithm=MD5")), "{ch:?}");
    let n = fresh(&app).await;
    let wrong = digest("guess", "PROPFIND", "/dav/", &n, "SHA-256");
    let r = raw(&app, &[("host", HOST), ("authorization", &wrong)]).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // OPTIONS says what this is before anyone signs in, as clients expect
    let r = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("OPTIONS")
                .uri("/dav/")
                .header("host", HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        r.headers()["dav"]
            .to_str()
            .unwrap()
            .contains("calendar-access")
    );
}

#[tokio::test]
async fn only_this_computers_programs_are_answered() {
    let (app, _) = setup();
    let n = fresh(&app).await;
    let mut nc = 0;
    let mut ok = || {
        nc += 1;
        let ha1 = hash("SHA-256", &format!("{USER}:{REALM}:{PW}"));
        let ha2 = hash("SHA-256", "PROPFIND:/dav/");
        let c = format!("{nc:08x}");
        let r = hash("SHA-256", &format!("{ha1}:{n}:{c}:c0ffee:auth:{ha2}"));
        format!(
            "Digest username=\"{USER}\", realm=\"{REALM}\", nonce=\"{n}\", uri=\"/dav/\", algorithm=SHA-256, response=\"{r}\", qop=auth, nc={c}, cnonce=\"c0ffee\""
        )
    };
    let st = |hs: Vec<(&'static str, String)>| {
        let app = app.clone();
        async move {
            let hs: Vec<(&str, &str)> = hs.iter().map(|(k, v)| (*k, v.as_str())).collect();
            raw(&app, &hs).await.status()
        }
    };

    assert_eq!(
        st(vec![("host", HOST.into()), ("authorization", ok())]).await,
        StatusCode::MULTI_STATUS
    );
    assert_eq!(
        st(vec![
            ("host", "localhost:41664".into()),
            ("authorization", ok())
        ])
        .await,
        StatusCode::MULTI_STATUS
    );
    // a name someone else's DNS points here, another port, a page in a browser
    assert_eq!(
        st(vec![
            ("host", "evil.example:41664".into()),
            ("authorization", ok())
        ])
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        st(vec![
            ("host", "127.0.0.1:41665".into()),
            ("authorization", ok())
        ])
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        st(vec![
            ("host", HOST.into()),
            ("origin", "https://evil.example".into()),
            ("authorization", ok())
        ])
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        st(vec![
            ("host", HOST.into()),
            ("sec-fetch-site", "cross-site".into()),
            ("authorization", ok())
        ])
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        st(vec![
            ("host", HOST.into()),
            ("sec-fetch-site", "none".into()),
            ("authorization", ok())
        ])
        .await,
        StatusCode::MULTI_STATUS
    );
}

#[tokio::test]
async fn a_digest_answer_is_checked_whole() {
    let (app, _) = setup();
    let n = fresh(&app).await;
    let st = |auth: String| {
        let app = app.clone();
        async move { raw(&app, &[("host", HOST), ("authorization", &auth)]).await }
    };
    // MD5 too, for the programs that know no other
    assert_eq!(
        st(digest(PW, "PROPFIND", "/dav/", &n, "MD5"))
            .await
            .status(),
        StatusCode::MULTI_STATUS
    );
    // another request's answer, a made-up nonce
    assert_eq!(
        st(digest(PW, "PROPFIND", "/dav/calendars/", &n, "SHA-256"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        st(digest(
            PW,
            "PROPFIND",
            "/dav/",
            "1.00000000000000000000000000000000",
            "SHA-256"
        ))
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    // the same answer heard again (the MD5 one above): never good twice
    assert_eq!(
        st(digest(PW, "PROPFIND", "/dav/", &n, "MD5"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    // the password in the clear is no way in
    use base64::Engine as _;
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PW}"))
    );
    assert_eq!(st(basic).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_program_finds_the_calendars_and_address_books() {
    let (app, _) = setup();
    let find = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:card="urn:ietf:params:xml:ns:carddav"><d:prop><d:current-user-principal/><c:calendar-home-set/><card:addressbook-home-set/><d:resourcetype/></d:prop></d:propfind>"#;
    let (s, _, b) = ask(
        &app,
        "PROPFIND",
        "/.well-known/caldav",
        &[("depth", "0")],
        find,
    )
    .await;
    assert_eq!(s, StatusCode::MULTI_STATUS);
    assert!(
        b.contains("<D:current-user-principal><D:href>/dav/</D:href>"),
        "{b}"
    );
    assert!(
        b.contains("<C:calendar-home-set><D:href>/dav/calendars/</D:href>"),
        "{b}"
    );
    assert!(
        b.contains("<CR:addressbook-home-set><D:href>/dav/contacts/</D:href>"),
        "{b}"
    );
    let list = r#"<d:propfind xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/"><d:prop><d:displayname/><d:resourcetype/><cs:getctag/><a:calendar-color/><d:current-user-privilege-set/><d:quota-used-bytes/></d:prop></d:propfind>"#;
    let (s, _, b) = ask(&app, "PROPFIND", "/dav/calendars/", &[("depth", "1")], list).await;
    assert_eq!(s, StatusCode::MULTI_STATUS);
    assert!(
        b.contains("<D:href>/dav/calendars/personal/</D:href>"),
        "{b}"
    );
    assert!(
        b.contains("<D:href>/dav/calendars/lingyun~abcd1234/</D:href>"),
        "{b}"
    );
    assert!(
        b.contains("<D:displayname>Lingyun&apos;s</D:displayname>")
            || b.contains("<D:displayname>Lingyun's</D:displayname>"),
        "{b}"
    );
    assert!(b.contains("<C:calendar/>"), "{b}");
    assert!(b.contains("<CS:getctag>"), "{b}");
    // what it does not have, it says so (404), not nothing
    assert!(
        b.contains("HTTP/1.1 404 Not Found") && b.contains("<D:quota-used-bytes/>"),
        "{b}"
    );
    // the shared one is read only: no write privilege
    let shared = b.split("lingyun~abcd1234").nth(1).unwrap();
    let shared = shared.split("</D:response>").next().unwrap();
    assert!(
        shared.contains("<D:read/>") && !shared.contains("<D:write/>"),
        "{shared}"
    );
    let (_, _, b) = ask(&app, "PROPFIND", "/dav/contacts/", &[("depth", "1")], "").await;
    assert!(
        b.contains("<D:href>/dav/contacts/contacts/</D:href>") && b.contains("<CR:addressbook/>"),
        "{b}"
    );
}

#[tokio::test]
async fn an_event_is_made_read_and_changed_from_its_version() {
    let (app, m) = setup();
    let at = "/dav/calendars/personal/e1.ics";
    let (s, h, _) = ask(
        &app,
        "PUT",
        at,
        &[("if-none-match", "*"), ("content-type", "text/calendar")],
        EVENT,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let v1 = h["etag"].to_str().unwrap().to_string();
    // made again: it is there
    let (s, _, _) = ask(&app, "PUT", at, &[("if-none-match", "*")], EVENT).await;
    assert_eq!(s, StatusCode::PRECONDITION_FAILED);
    let (s, h, b) = ask(&app, "GET", at, &[], "").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, EVENT);
    assert_eq!(h["etag"].to_str().unwrap(), v1);
    assert!(
        h["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/calendar")
    );
    let changed = EVENT.replace("Dentist", "Dentist, moved");
    let (s, h, _) = ask(&app, "PUT", at, &[("if-match", &v1)], &changed).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let v2 = h["etag"].to_str().unwrap().to_string();
    assert_ne!(v1, v2);
    // a second program, still on version one, changes nothing
    let (s, _, _) = ask(&app, "PUT", at, &[("if-match", &v1)], EVENT).await;
    assert_eq!(s, StatusCode::PRECONDITION_FAILED);
    let (s, _, _) = ask(&app, "DELETE", at, &[("if-match", &v1)], "").await;
    assert_eq!(s, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        m.file(Kind::Calendars, "personal", "e1.ics").unwrap(),
        changed.as_bytes()
    );
    // not a calendar at all
    let (s, _, _) = ask(&app, "PUT", "/dav/calendars/personal/x.ics", &[], "hello").await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (s, _, _) = ask(&app, "DELETE", at, &[("if-match", &v2)], "").await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(m.file(Kind::Calendars, "personal", "e1.ics").is_none());
}

#[tokio::test]
async fn a_calendar_shared_to_be_seen_is_not_changed() {
    let (app, _) = setup();
    let (s, _, _) = ask(
        &app,
        "PUT",
        "/dav/calendars/lingyun~abcd1234/e.ics",
        &[],
        EVENT,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = ask(
        &app,
        "DELETE",
        "/dav/calendars/lingyun~abcd1234/e.ics",
        &[],
        "",
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn items_come_with_their_data_by_multiget_and_by_query() {
    let (app, _) = setup();
    ask(&app, "PUT", "/dav/calendars/personal/e1.ics", &[], EVENT).await;
    let todo = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:t1\r\nSUMMARY:Bins\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    ask(&app, "PUT", "/dav/calendars/personal/t1.ics", &[], todo).await;
    let (s, _, b) = ask(
        &app,
        "PROPFIND",
        "/dav/calendars/personal/",
        &[("depth", "1")],
        r#"<propfind xmlns="DAV:"><prop><getetag/></prop></propfind>"#,
    )
    .await;
    assert_eq!(s, StatusCode::MULTI_STATUS);
    assert!(
        b.contains("/dav/calendars/personal/e1.ics")
            && b.contains("/dav/calendars/personal/t1.ics"),
        "{b}"
    );
    assert!(b.contains("<D:getetag>"), "{b}");
    let multi = r#"<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><d:href>http://127.0.0.1:41664/dav/calendars/personal/e1.ics</d:href><d:href>/dav/calendars/personal/gone.ics</d:href></c:calendar-multiget>"#;
    let (s, _, b) = ask(
        &app,
        "REPORT",
        "/dav/calendars/personal/",
        &[("depth", "1")],
        multi,
    )
    .await;
    assert_eq!(s, StatusCode::MULTI_STATUS);
    // the event's text, escaped for xml
    assert!(b.contains("SUMMARY:Dentist &amp; co &lt;x&gt;"), "{b}");
    assert!(b.contains("gone.ics</D:href><D:status>HTTP/1.1 404"), "{b}");
    let query = r#"<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"/></c:comp-filter></c:filter></c:calendar-query>"#;
    let (_, _, b) = ask(&app, "REPORT", "/dav/calendars/personal/", &[], query).await;
    assert!(b.contains("t1.ics") && !b.contains("e1.ics"), "{b}");
}

#[tokio::test]
async fn a_program_syncs_from_its_token() {
    let (app, _) = setup();
    let sync = |t: &str| {
        format!(
            r#"<d:sync-collection xmlns:d="DAV:"><d:sync-token>{t}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#
        )
    };
    let at = "/dav/calendars/personal/";
    ask(&app, "PUT", "/dav/calendars/personal/a.ics", &[], EVENT).await;
    ask(&app, "PUT", "/dav/calendars/personal/b.ics", &[], EVENT).await;
    let (s, _, b) = ask(&app, "REPORT", at, &[], &sync("")).await;
    assert_eq!(s, StatusCode::MULTI_STATUS);
    assert!(b.contains("a.ics") && b.contains("b.ics"), "{b}");
    let t1 = b
        .split("<D:sync-token>")
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .to_string();
    // nothing changed: nothing but the token
    let (_, _, b) = ask(&app, "REPORT", at, &[], &sync(&t1)).await;
    assert!(!b.contains(".ics"), "{b}");
    // one changed, one gone, one new
    ask(
        &app,
        "PUT",
        "/dav/calendars/personal/a.ics",
        &[],
        &EVENT.replace("Dentist", "D"),
    )
    .await;
    ask(&app, "DELETE", "/dav/calendars/personal/b.ics", &[], "").await;
    ask(&app, "PUT", "/dav/calendars/personal/c.ics", &[], EVENT).await;
    let (_, _, b) = ask(&app, "REPORT", at, &[], &sync(&t1)).await;
    assert!(b.contains("a.ics</D:href><D:propstat>"), "{b}");
    assert!(b.contains("c.ics</D:href><D:propstat>"), "{b}");
    assert!(b.contains("b.ics</D:href><D:status>HTTP/1.1 404"), "{b}");
    // a token this bridge never gave: read it all again
    let (s, _, b) = ask(
        &app,
        "REPORT",
        at,
        &[],
        &sync("https://commonty.org/ns/sync/nope"),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(b.contains("valid-sync-token"), "{b}");
}

#[tokio::test]
async fn a_program_makes_and_names_a_calendar() {
    let (app, m) = setup();
    let mk = r#"<c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:set><d:prop><d:displayname>Work</d:displayname><a:calendar-color>#2F6FD6FF</a:calendar-color></d:prop></d:set></c:mkcalendar>"#;
    let (s, _, _) = ask(&app, "MKCALENDAR", "/dav/calendars/work/", &[], mk).await;
    assert_eq!(s, StatusCode::CREATED);
    let c = m.collections(Kind::Calendars).await.unwrap();
    let w = c.iter().find(|c| c.id == "work").unwrap();
    assert_eq!((w.name.as_str(), w.color.as_str()), ("Work", "#2F6FD6FF"));
    let (s, _, _) = ask(&app, "MKCALENDAR", "/dav/calendars/work/", &[], mk).await;
    assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED);
    let (s, _, _) = ask(&app, "MKCALENDAR", "/dav/calendars/no%20spaces/", &[], mk).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let patch = r#"<d:propertyupdate xmlns:d="DAV:" xmlns:a="http://apple.com/ns/ical/"><d:set><d:prop><d:displayname>Office</d:displayname><a:calendar-color>red; x</a:calendar-color></d:prop></d:set></d:propertyupdate>"#;
    let (s, _, _) = ask(&app, "PROPPATCH", "/dav/calendars/work/", &[], patch).await;
    assert_eq!(s, StatusCode::MULTI_STATUS);
    let c = m.collections(Kind::Calendars).await.unwrap();
    let w = c.iter().find(|c| c.id == "work").unwrap();
    // the name is taken, a colour that is not one is not
    assert_eq!((w.name.as_str(), w.color.as_str()), ("Office", "#2F6FD6FF"));
}

#[test]
fn paths_stay_inside() {
    assert_eq!(
        at("/dav/calendars/a/b.ics"),
        Some(At::Item(Kind::Calendars, "a".into(), "b.ics".into()))
    );
    assert_eq!(at("/dav/calendars/a%2Fb/c"), None);
    assert_eq!(at("/dav/calendars/../x"), None);
    assert_eq!(at("/dav/other/"), None);
    assert_eq!(
        href_at("https://h:1/dav/contacts/c/p%20q.vcf"),
        Some(At::Item(Kind::Contacts, "c".into(), "p q.vcf".into()))
    );
    assert_eq!(
        href(&At::Item(Kind::Contacts, "c".into(), "p q.vcf".into())),
        "/dav/contacts/c/p%20q.vcf"
    );
}

#[test]
fn an_old_or_another_runs_challenge_is_asked_again() {
    let secret = "pw\0run-one";
    let ask = |n: &str| {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            digest(PW, "PROPFIND", "/dav/", n, "SHA-256")
                .parse()
                .unwrap(),
        );
        signed_in(&h, "PROPFIND", "/dav/", secret, PW, now())
    };
    assert!(ask(&nonce(secret, now(), 7)).is_ok());
    // answered right, but run out: stale, so the program asks again quietly
    assert_eq!(ask(&nonce(secret, now() - 2 * NONCE_SECS, 7)), Err(true));
    // made by an earlier run of the bridge: not one of this run's
    assert_eq!(ask(&nonce("pw\0run-zero", now(), 7)), Err(false));
    // no qop, the old way with no count: refused
    let mut h = HeaderMap::new();
    let n = nonce(secret, now(), 7);
    let ha1 = hash("SHA-256", &format!("{USER}:{REALM}:{PW}"));
    let ha2 = hash("SHA-256", "PROPFIND:/dav/");
    let r = hash("SHA-256", &format!("{ha1}:{n}:{ha2}"));
    h.insert(
        header::AUTHORIZATION,
        format!("Digest username=\"{USER}\", realm=\"{REALM}\", nonce=\"{n}\", uri=\"/dav/\", algorithm=SHA-256, response=\"{r}\"")
            .parse()
            .unwrap(),
    );
    assert_eq!(
        signed_in(&h, "PROPFIND", "/dav/", secret, PW, now()),
        Err(false)
    );
}
