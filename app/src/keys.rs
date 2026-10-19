//! One route for the keys a page of the member's asks for, wherever the
//! app serves it (its window, or a browser it let in), and the way to
//! Photos for a page that holds Photos' password.
//!
//! The pages that ask are the app's own copies (web/_dd), on one origin:
//! Files, Mail, Movies & TV, Chat and Settings. What that origin may hold is
//! the library's key, the mail key and Photos' password. Passwords' key
//! never: it goes to Passwords' own page, on its own origin, alone. Who
//! may ask at all is decided before this (site.rs: the launch secret in the
//! window; local.rs: `admit`, a browser the member let in).
//!
//! A page here may only ask its own origin (PAGE_POLICY), so museum and
//! the store are reached by way of it: /_dd/ente/<path> goes to museum, and
//! /_dd/ente-store/<host>/<path> to the store at one of Photos' own hosts.
//! Neither carries anything of this device's: the page's own Photos session
//! (X-Auth-Token) or a presigned address is what lets it in.

use serde::Deserialize;
use tauri::{AppHandle, Runtime};

type Fail = (u16, String);

/// POST /_dd/app/keys `{services: ["library", "mail", "photos"]}`: each
/// asked for, or `{error}` for one this device cannot give yet
pub(crate) async fn keys<R: Runtime>(
    app: &AppHandle<R>,
    body: &[u8],
) -> Result<serde_json::Value, Fail> {
    #[derive(Deserialize)]
    struct Ask {
        services: Vec<String>,
    }
    let ask: Ask = serde_json::from_slice(body)
        .map_err(|_| (400, "say which services: {\"services\": [...]}".to_string()))?;
    let mut out = serde_json::Map::new();
    for s in ask.services {
        let got = match s.as_str() {
            "library" => crate::site::library_value(app)
                .await
                .and_then(|v| v.ok_or_else(|| "no library opens here".to_string())),
            "mail" => crate::mail::key(app, false).await.map_err(|(_, why)| why),
            "photos" => crate::photos::current(app)
                .map(|pw| serde_json::json!({ "pw": pw }))
                .ok_or_else(|| {
                    "Photos does not open on this device yet: open Photos once in the app"
                        .to_string()
                }),
            "vault" => return Err((403, "Passwords' key goes to Passwords alone".into())),
            _ => return Err((400, format!("no service called {s}"))),
        };
        out.insert(
            s,
            got.unwrap_or_else(|why| serde_json::json!({ "error": why })),
        );
    }
    Ok(serde_json::Value::Object(out))
}

/// Where a page's /_dd/ente/... or /_dd/ente-store/... goes, if anywhere:
/// museum, or the store at one of Photos' own hosts
pub(crate) fn ente_target(path: &str, query: Option<&str>, domain: &str) -> Option<String> {
    let url = if let Some(rest) = path.strip_prefix("/_dd/ente/") {
        format!("https://api.{domain}/{rest}")
    } else {
        let rest = path.strip_prefix("/_dd/ente-store/")?;
        let (host, p) = rest.split_once('/')?;
        if !store_host(host, domain) {
            return None;
        }
        format!("https://{host}/{p}")
    };
    // a dot segment, written plainly or escaped, would leave the path given
    let lower = path.to_ascii_lowercase();
    if lower.split('/').any(|s| s == "." || s == "..") || lower.contains("%2e") {
        return None;
    }
    Some(match query {
        Some(q) => format!("{url}?{q}"),
        None => url,
    })
}

fn store_host(host: &str, domain: &str) -> bool {
    host == format!("photos-store.{domain}") || host == format!("s3.{domain}")
}

/// museum sends a download to the store: the page follows it by way of
/// its own origin too
pub(crate) fn ente_location(location: &str, domain: &str) -> Option<String> {
    let rest = location.strip_prefix("https://")?;
    let host = rest.split(['/', '?']).next()?;
    store_host(host, domain).then(|| format!("/_dd/ente-store/{rest}"))
}

/// What museum or the store answered, for the page's server to send on
pub(crate) struct Answer {
    pub status: u16,
    pub kind: Option<String>,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

/// A page's call to museum or the store, sent on with only the headers
/// Ente's own calls carry: the page's Photos session, never this device's.
/// A redirect to the store comes back as one to this origin's way there;
/// nothing a page would run comes back at all.
pub(crate) async fn ente_send(
    method: &str,
    path: &str,
    query: Option<&str>,
    headers: &tauri::http::HeaderMap,
    body: Vec<u8>,
) -> Result<Answer, Fail> {
    let domain = crate::account::domain();
    let target = ente_target(path, query, &domain)
        .ok_or((404, "not one of Photos' addresses".to_string()))?;
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| (405, "no such method".to_string()))?;
    crate::local::network_ready().await;
    let mut out = crate::local::http()
        .map_err(|e| (502, e))?
        .request(method.clone(), &target);
    for h in [
        "content-type",
        "content-md5",
        "range",
        "accept",
        "x-auth-token",
        "x-client-package",
        "x-client-version",
    ] {
        if let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()) {
            out = out.header(h, v);
        }
    }
    if !body.is_empty() {
        out = out.body(body);
    }
    let r = crate::local::send(out)
        .await
        .map_err(|e| (502, crate::local::why(&e)))?;
    let status = r.status().as_u16();
    if r.status().is_redirection() {
        let location = r
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .and_then(|l| ente_location(l, &domain))
            .ok_or((502, "Photos sent the page somewhere else".to_string()))?;
        return Ok(Answer {
            status,
            kind: None,
            location: Some(location),
            body: Vec::new(),
        });
    }
    let kind = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if !crate::site::data_from_box(
        method.as_str(),
        status,
        &kind.clone().unwrap_or_default().to_ascii_lowercase(),
    ) {
        return Err((
            502,
            "Photos answered with code; the app runs only its own".into(),
        ));
    }
    let body = r.bytes().await.map_err(|e| (502, e.to_string()))?.to_vec();
    Ok(Answer {
        status,
        kind,
        location: None,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_reaches_museum_and_photos_store_and_nothing_else() {
        let d = "example.org";
        assert_eq!(
            ente_target("/_dd/ente/files/upload-urls", Some("count=2"), d).as_deref(),
            Some("https://api.example.org/files/upload-urls?count=2")
        );
        assert_eq!(
            ente_target(
                "/_dd/ente-store/photos-store.example.org/ente/k",
                Some("X-Amz=1"),
                d
            )
            .as_deref(),
            Some("https://photos-store.example.org/ente/k?X-Amz=1")
        );
        for bad in [
            "/_dd/ente-store/evil.example/x",
            "/_dd/ente-store/photos-store.example.org.evil.example/x",
            "/_dd/ente-store/home.example.org/_dd/app/keys",
            "/_dd/ente/../_dd/app/keys",
            "/_dd/ente/%2e%2e/_dd/app/keys",
            "/_dd/ente-storex/photos-store.example.org/x",
        ] {
            assert_eq!(ente_target(bad, None, d), None, "{bad}");
        }
    }

    #[test]
    fn a_download_redirect_stays_on_the_page_origin() {
        let d = "example.org";
        assert_eq!(
            ente_location("https://photos-store.example.org/ente/k?sig=1", d).as_deref(),
            Some("/_dd/ente-store/photos-store.example.org/ente/k?sig=1")
        );
        assert_eq!(ente_location("https://evil.example/x", d), None);
    }
}
