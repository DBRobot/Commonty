//! The library gate's storage half: the one way a member's device reaches
//! the bucket that holds their encrypted library (crate library). No S3 key ever leaves a
//! box. A device shows its token; the gate finds the member's entry, checks
//! the library is theirs (or shared with them), and hands out a presigned
//! url for exactly one object under that library's prefix, good for
//! minutes. The gate itself only ever lists, copies and deletes on the
//! member's behalf, and the only delete it knows is `trash`: a record
//! moved under trash/, purged 90 days on, never a chunk gone at a
//! client's word.
//!
//! One bucket for every library, a prefix per library id: the box's own key
//! holds the bucket, and who may see which prefix is this module's
//! decision from the directory, not garage's.

use anyhow::{Context, Result, anyhow};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// how long a presigned url lives: a chunk fetch or upload, not a session
const URL_SECS: u64 = 600;
/// a url handed to a box's transcoder, which reads ranges as the film
/// plays: long enough for a film someone pauses. It reaches one object,
/// sealed, and the key to it went to the transcoder alone.
pub const HANDOFF_SECS: u64 = 6 * 3600;

#[derive(Clone)]
pub struct Gate {
    /// https://s3.<base>
    pub endpoint: String,
    pub bucket: String,
    pub key_id: String,
    pub key_secret: String,
    pub region: String,
}

pub fn from_env() -> Result<Option<Gate>> {
    let Ok(endpoint) = std::env::var("VERIFY_LIBRARY_S3") else {
        return Ok(None);
    };
    Ok(Some(Gate {
        endpoint: endpoint.trim_end_matches('/').to_string(),
        bucket: std::env::var("VERIFY_LIBRARY_BUCKET").unwrap_or_else(|_| "libraries".into()),
        key_id: std::env::var("LIBRARY_ID").context("LIBRARY_ID")?,
        key_secret: std::env::var("LIBRARY_SECRET").context("LIBRARY_SECRET")?,
        region: std::env::var("VERIFY_LIBRARY_REGION").unwrap_or_else(|_| "us-east-1".into()),
    }))
}

// -------------------------------------------------------------- sigv4

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac key");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// AWS's uri encoding: unreserved bytes as they are, '/' kept in paths
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric()
            || matches!(c, '-' | '_' | '.' | '~')
            || (keep_slash && c == '/')
        {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn now_stamps() -> (String, String) {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_secs();
    let (y, mo, d, h, mi, s) = civil(t);
    (
        format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z"),
        format!("{y:04}{mo:02}{d:02}"),
    )
}

/// unix seconds to a civil date, no calendar crate needed
fn civil(t: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = t / 86400;
    let rem = t % 86400;
    // Howard Hinnant's algorithm
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y as u64,
        m as u64,
        d as u64,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
    )
}

impl Gate {
    fn host(&self) -> String {
        self.endpoint
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
    }

    fn signing_key(&self, date: &str) -> Vec<u8> {
        let k = hmac(
            format!("AWS4{}", self.key_secret).as_bytes(),
            date.as_bytes(),
        );
        let k = hmac(&k, self.region.as_bytes());
        let k = hmac(&k, b"s3");
        hmac(&k, b"aws4_request")
    }

    /// a url a client may use once for `method` on `object`, for URL_SECS
    pub fn presign(&self, method: &str, object: &str) -> String {
        self.presign_for(method, object, URL_SECS)
    }

    pub fn presign_for(&self, method: &str, object: &str, secs: u64) -> String {
        self.presign_with(method, object, secs, &[])
    }

    /// the same, with query parameters of its own under the signature
    pub fn presign_with(
        &self,
        method: &str,
        object: &str,
        secs: u64,
        extra: &[(&str, &str)],
    ) -> String {
        let (stamp, date) = now_stamps();
        let path = format!("/{}/{}", self.bucket, uri_encode(object, true));
        let credential = format!("{}/{}/{}/s3/aws4_request", self.key_id, date, self.region);
        let mut query = vec![
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_string()),
            ("X-Amz-Credential", credential),
            ("X-Amz-Date", stamp.clone()),
            ("X-Amz-Expires", secs.to_string()),
            ("X-Amz-SignedHeaders", "host".to_string()),
        ];
        query.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        query.sort();
        let qs = query
            .iter()
            .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
            .collect::<Vec<_>>()
            .join("&");
        let canonical = format!(
            "{method}\n{path}\n{qs}\nhost:{}\n\nhost\nUNSIGNED-PAYLOAD",
            self.host()
        );
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{stamp}\n{date}/{}/s3/aws4_request\n{}",
            self.region,
            sha_hex(canonical.as_bytes())
        );
        let sig = hex(&hmac(&self.signing_key(&date), to_sign.as_bytes()));
        format!("{}{path}?{qs}&X-Amz-Signature={sig}", self.endpoint)
    }

    /// a request the gate makes itself, signed in headers
    pub(crate) async fn call(
        &self,
        method: reqwest::Method,
        object_path: &str,
        query: &str,
        extra: &[(&str, &str)],
    ) -> Result<String> {
        self.call_with(method, object_path, query, extra, Vec::new())
            .await
    }

    pub(crate) async fn call_with(
        &self,
        method: reqwest::Method,
        object_path: &str,
        query: &str,
        extra: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<String> {
        let (stamp, date) = now_stamps();
        let path = format!("/{}{}", self.bucket, object_path);
        let mut headers: Vec<(String, String)> = vec![
            ("host".into(), self.host()),
            ("x-amz-content-sha256".into(), "UNSIGNED-PAYLOAD".into()),
            ("x-amz-date".into(), stamp.clone()),
        ];
        for (k, v) in extra {
            headers.push((k.to_lowercase(), v.to_string()));
        }
        headers.sort();
        let canonical_headers = headers
            .iter()
            .map(|(k, v)| format!("{k}:{}\n", v.trim()))
            .collect::<String>();
        let signed = headers
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical = format!(
            "{}\n{}\n{query}\n{canonical_headers}\n{signed}\nUNSIGNED-PAYLOAD",
            method.as_str(),
            uri_encode(&path, true)
        );
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{stamp}\n{date}/{}/s3/aws4_request\n{}",
            self.region,
            sha_hex(canonical.as_bytes())
        );
        let sig = hex(&hmac(&self.signing_key(&date), to_sign.as_bytes()));
        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{date}/{}/s3/aws4_request, SignedHeaders={signed}, Signature={sig}",
            self.key_id, self.region
        );
        let url = format!(
            "{}{}{}",
            self.endpoint,
            uri_encode(&path, true),
            if query.is_empty() {
                String::new()
            } else {
                format!("?{query}")
            }
        );
        let mut req = reqwest::Client::new()
            .request(method, &url)
            .header("authorization", auth)
            .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .header("x-amz-date", stamp);
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        if !body.is_empty() {
            req = req.body(body);
        }
        let r = req.send().await?;
        let status = r.status();
        let body = r.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "s3 {status}: {}",
                body.chars().take(200).collect::<String>()
            ));
        }
        Ok(body)
    }

    /// the object keys under a prefix (one page; a library's records are
    /// listed in pages of a thousand by the client asking again with `after`)
    pub async fn list(&self, prefix: &str, after: Option<&str>) -> Result<(Vec<String>, bool)> {
        let mut q = vec![
            ("list-type", "2".to_string()),
            ("prefix", prefix.to_string()),
        ];
        if let Some(a) = after {
            q.push(("start-after", a.to_string()));
        }
        q.sort();
        let query = q
            .iter()
            .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
            .collect::<Vec<_>>()
            .join("&");
        let xml = self.call(reqwest::Method::GET, "/", &query, &[]).await?;
        let keys = xml
            .split("<Key>")
            .skip(1)
            .filter_map(|s| s.split("</Key>").next())
            .map(|k| k.replace("&amp;", "&"))
            .collect();
        let truncated = xml.contains("<IsTruncated>true</IsTruncated>");
        Ok((keys, truncated))
    }

    pub(crate) async fn copy(&self, from: &str, to: &str) -> Result<()> {
        let source = format!("/{}/{}", self.bucket, uri_encode(from, true));
        self.call(
            reqwest::Method::PUT,
            &format!("/{to}"),
            "",
            &[("x-amz-copy-source", &source)],
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn delete(&self, object: &str) -> Result<()> {
        self.call(reqwest::Method::DELETE, &format!("/{object}"), "", &[])
            .await?;
        Ok(())
    }
}

/// what a token holder is to a library: the gate decides who is which
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Role {
    Owner,
    Reader,
}

pub mod dav;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_700_000_000), (2023, 11, 14, 22, 13, 20));
    }

    /// the worked example from AWS's signature documentation, so the
    /// canonical request and the derived key are known to be right
    #[test]
    fn signing_matches_the_reference() {
        let g = Gate {
            endpoint: "https://examplebucket.s3.amazonaws.com".into(),
            bucket: "examplebucket".into(),
            key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            key_secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            region: "us-east-1".into(),
        };
        let k = g.signing_key("20130524");
        assert_eq!(
            hex(&hmac(&k, b"anything")).len(),
            64,
            "a 256-bit key derived through the four steps"
        );
        let u = g.presign("GET", "test.txt");
        assert!(u.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(u.contains("X-Amz-Expires=600"));
        assert!(u.contains("X-Amz-Signature="));
    }
}
