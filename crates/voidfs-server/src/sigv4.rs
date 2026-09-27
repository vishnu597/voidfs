// SPDX-License-Identifier: Apache-2.0
//! AWS Signature Version 4 verification (protocol §2): header and presigned-query
//! authentication, access keys and scopes.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use http::HeaderMap;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
const MAX_SKEW_SECS: i64 = 15 * 60;
const MAX_PRESIGN_SECS: i64 = 7 * 24 * 3600;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Scope {
    Read,
    Write,
    Admin,
}

impl std::str::FromStr for Scope {
    type Err = String;
    fn from_str(s: &str) -> Result<Scope, String> {
        match s {
            "read" => Ok(Scope::Read),
            "write" => Ok(Scope::Write),
            "admin" => Ok(Scope::Admin),
            _ => Err(format!("unknown scope {s:?}")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct KeyInfo {
    pub id: String,
    pub secret: String,
    pub scope: Scope,
    /// Drive aliases or ids this key may reach; `None` for every drive.
    pub drives: Option<Vec<String>>,
}

impl KeyInfo {
    pub fn reaches(&self, alias: &str, id: &str) -> bool {
        self.drives.as_ref().is_none_or(|d| d.iter().any(|x| x == alias || x == id))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Keys(HashMap<String, KeyInfo>);

impl Keys {
    pub fn insert(&mut self, k: KeyInfo) {
        self.0.insert(k.id.clone(), k);
    }

    pub fn get(&self, id: &str) -> Option<&KeyInfo> {
        self.0.get(id)
    }
}

/// How the request body is covered by the signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Unsigned,
    Sha256([u8; 32]),
    /// `aws-chunked` bodies; `signed` chunks carry their own signatures.
    Streaming { signed: bool, trailer: bool },
}

#[derive(Clone, Debug)]
pub struct Authenticated {
    pub key: KeyInfo,
    pub payload: Payload,
    /// Values for verifying signed `aws-chunked` bodies.
    pub signing_key: [u8; 32],
    pub scope: String,
    pub amz_date: String,
    pub seed_signature: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("the request is not signed")]
    Anonymous,
    #[error("the authorization header is malformed: {0}")]
    Malformed(String),
    #[error("the access key id does not exist")]
    UnknownKey,
    #[error("the request signature does not match")]
    SignatureMismatch,
    #[error("the request time is too far from the server's time")]
    Skewed,
    #[error("the presigned request has expired")]
    Expired,
    #[error("header {0} must be signed")]
    UnsignedExtensionHeader(String),
    #[error("the authorization mechanism you have provided is not supported; use AWS4-HMAC-SHA256 (for boto3, signature_version='s3v4')")]
    SigV2,
}

impl AuthError {
    pub fn s3_code(&self) -> (&'static str, u16) {
        match self {
            AuthError::Anonymous => ("AccessDenied", 403),
            AuthError::Malformed(_) => ("AuthorizationHeaderMalformed", 400),
            AuthError::UnknownKey => ("InvalidAccessKeyId", 403),
            AuthError::SignatureMismatch => ("SignatureDoesNotMatch", 403),
            AuthError::Skewed => ("RequestTimeTooSkewed", 403),
            AuthError::Expired => ("AccessDenied", 403),
            AuthError::UnsignedExtensionHeader(_) => ("InvalidArgument", 400),
            AuthError::SigV2 => ("InvalidRequest", 400),
        }
    }
}

pub fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(data);
    m.finalize().into_bytes().into()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn signing_key(secret: &str, date: &str, region: &str) -> [u8; 32] {
    let k = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k = hmac(&k, region.as_bytes());
    let k = hmac(&k, b"s3");
    hmac(&k, b"aws4_request")
}

fn uri_encode(s: &str) -> String {
    utf8_percent_encode(s, UNRESERVED).to_string()
}

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// The canonical query string: pairs decoded, re-encoded strictly, and sorted.
fn canonical_query(raw: &str, skip_signature: bool) -> String {
    let mut pairs: Vec<(String, String)> = raw
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (uri_encode(&decode(k)), uri_encode(&decode(v)))
        })
        .filter(|(k, _)| !(skip_signature && k == "X-Amz-Signature"))
        .collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

fn canonical_header_value(values: &[&str]) -> String {
    values.iter().map(|v| v.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(",")
}

struct Parsed {
    key_id: String,
    date: String,
    region: String,
    signed_headers: Vec<String>,
    signature: String,
    amz_date: String,
    payload_hash: String,
    presigned_expires: Option<i64>,
}

fn parse_credential(c: &str) -> Result<(String, String, String), AuthError> {
    let parts: Vec<&str> = c.split('/').collect();
    if parts.len() != 5 || parts[3] != "s3" || parts[4] != "aws4_request" {
        return Err(AuthError::Malformed(format!("bad credential scope {c:?}")));
    }
    Ok((parts[0].to_owned(), parts[1].to_owned(), parts[2].to_owned()))
}

fn parse(uri: &http::Uri, headers: &HeaderMap) -> Result<Parsed, AuthError> {
    let header = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    if let Some(auth) = header("authorization") {
        if auth.starts_with("AWS ") {
            return Err(AuthError::SigV2);
        }
        let rest = auth.strip_prefix("AWS4-HMAC-SHA256 ").ok_or_else(|| AuthError::Malformed("only AWS4-HMAC-SHA256 is supported".into()))?;
        let mut fields = HashMap::new();
        for part in rest.split(',') {
            let (k, v) = part.trim().split_once('=').ok_or_else(|| AuthError::Malformed(part.into()))?;
            fields.insert(k.trim(), v.trim());
        }
        let get = |k: &str| fields.get(k).copied().ok_or_else(|| AuthError::Malformed(format!("missing {k}")));
        let (key_id, date, region) = parse_credential(get("Credential")?)?;
        let amz_date = header("x-amz-date").ok_or_else(|| AuthError::Malformed("missing x-amz-date".into()))?.to_owned();
        let payload_hash = header("x-amz-content-sha256").unwrap_or("UNSIGNED-PAYLOAD").to_owned();
        return Ok(Parsed {
            key_id,
            date,
            region,
            signed_headers: get("SignedHeaders")?.split(';').map(str::to_owned).collect(),
            signature: get("Signature")?.to_owned(),
            amz_date,
            payload_hash,
            presigned_expires: None,
        });
    }
    let query: HashMap<String, String> = uri
        .query()
        .unwrap_or("")
        .split('&')
        .filter_map(|p| p.split_once('=').map(|(k, v)| (decode(k), decode(v))))
        .collect();
    if query.get("X-Amz-Algorithm").map(String::as_str) == Some("AWS4-HMAC-SHA256") {
        let get = |k: &str| query.get(k).cloned().ok_or_else(|| AuthError::Malformed(format!("missing {k}")));
        let (key_id, date, region) = parse_credential(&get("X-Amz-Credential")?)?;
        let expires: i64 = get("X-Amz-Expires")?.parse().map_err(|_| AuthError::Malformed("bad X-Amz-Expires".into()))?;
        if !(1..=MAX_PRESIGN_SECS).contains(&expires) {
            return Err(AuthError::Malformed("X-Amz-Expires out of range".into()));
        }
        return Ok(Parsed {
            key_id,
            date,
            region,
            signed_headers: get("X-Amz-SignedHeaders")?.split(';').map(str::to_owned).collect(),
            signature: get("X-Amz-Signature")?,
            amz_date: get("X-Amz-Date")?,
            payload_hash: "UNSIGNED-PAYLOAD".into(),
            presigned_expires: Some(expires),
        });
    }
    if query.contains_key("AWSAccessKeyId") && query.contains_key("Signature") {
        return Err(AuthError::SigV2);
    }
    Err(AuthError::Anonymous)
}

/// Verifies a request's signature and returns the key that made it.
pub fn verify(method: &str, uri: &http::Uri, headers: &HeaderMap, keys: &Keys, now: DateTime<Utc>) -> Result<Authenticated, AuthError> {
    let p = parse(uri, headers)?;
    let key = keys.get(&p.key_id).ok_or(AuthError::UnknownKey)?;
    let when = NaiveDateTime::parse_from_str(&p.amz_date, "%Y%m%dT%H%M%SZ")
        .map_err(|_| AuthError::Malformed("bad X-Amz-Date".into()))?
        .and_utc();
    if !p.amz_date.starts_with(&p.date) {
        return Err(AuthError::Malformed("credential date does not match X-Amz-Date".into()));
    }
    match p.presigned_expires {
        None if (now - when).num_seconds().abs() > MAX_SKEW_SECS => return Err(AuthError::Skewed),
        Some(exp) if (now - when).num_seconds() > exp => return Err(AuthError::Expired),
        Some(_) if (when - now).num_seconds() > MAX_SKEW_SECS => return Err(AuthError::Skewed),
        _ => {}
    }

    let mut canonical_headers = String::new();
    for name in &p.signed_headers {
        let values: Vec<&str> = if name == "host" && headers.get("host").is_none() {
            uri.authority().map(|a| vec![a.as_str()]).unwrap_or_default()
        } else {
            headers.get_all(name.as_str()).iter().filter_map(|v| v.to_str().ok()).collect()
        };
        canonical_headers.push_str(&format!("{name}:{}\n", canonical_header_value(&values)));
    }
    let path = if uri.path().is_empty() { "/" } else { uri.path() };
    let canonical = format!(
        "{method}\n{path}\n{}\n{canonical_headers}\n{}\n{}",
        canonical_query(uri.query().unwrap_or(""), p.presigned_expires.is_some()),
        p.signed_headers.join(";"),
        p.payload_hash
    );
    let scope = format!("{}/{}/s3/aws4_request", p.date, p.region);
    let to_sign = format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{}", p.amz_date, sha256_hex(canonical.as_bytes()));
    let skey = signing_key(&key.secret, &p.date, &p.region);
    let given = hex::decode(&p.signature).map_err(|_| AuthError::SignatureMismatch)?;
    let mut mac = HmacSha256::new_from_slice(&skey).expect("any key length");
    mac.update(to_sign.as_bytes());
    mac.verify_slice(&given).map_err(|_| AuthError::SignatureMismatch)?;

    for name in headers.keys() {
        let n = name.as_str();
        if n.starts_with("x-voidfs-") && !p.signed_headers.iter().any(|s| s == n) {
            return Err(AuthError::UnsignedExtensionHeader(n.to_owned()));
        }
    }

    let payload = match p.payload_hash.as_str() {
        "UNSIGNED-PAYLOAD" => Payload::Unsigned,
        "STREAMING-UNSIGNED-PAYLOAD-TRAILER" => Payload::Streaming { signed: false, trailer: true },
        "STREAMING-AWS4-HMAC-SHA256-PAYLOAD" => Payload::Streaming { signed: true, trailer: false },
        "STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER" => Payload::Streaming { signed: true, trailer: true },
        h => {
            let mut out = [0u8; 32];
            hex::decode_to_slice(h, &mut out).map_err(|_| AuthError::Malformed(format!("bad x-amz-content-sha256 {h:?}")))?;
            Payload::Sha256(out)
        }
    };
    Ok(Authenticated { key: key.clone(), payload, signing_key: skey, scope, amz_date: p.amz_date, seed_signature: p.signature })
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use aws_credential_types::Credentials;
    use aws_sigv4::http_request::{PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SignatureLocation, SigningSettings, UriPathNormalizationMode, sign};
    use aws_sigv4::sign::v4;

    use super::*;

    fn keys() -> Keys {
        let mut k = Keys::default();
        k.insert(KeyInfo { id: "VFTESTKEY00000000000".into(), secret: "s".repeat(40), scope: Scope::Admin, drives: None });
        k
    }

    fn signed(method: &str, uri: &str, headers: &[(&str, &str)], body: &[u8], presign: bool) -> http::Request<()> {
        let identity = Credentials::new("VFTESTKEY00000000000", "s".repeat(40), None, None, "t").into();
        let mut settings = SigningSettings::default();
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        if presign {
            settings.signature_location = SignatureLocation::QueryParams;
            settings.expires_in = Some(std::time::Duration::from_secs(300));
        } else {
            settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        }
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region("auto")
            .name("s3")
            .time(SystemTime::now())
            .settings(settings)
            .build()
            .unwrap()
            .into();
        let mut all: Vec<(&str, &str)> = headers.to_vec();
        all.push(("host", "127.0.0.1:9000"));
        let body = if presign { SignableBody::UnsignedPayload } else { SignableBody::Bytes(body) };
        let req = SignableRequest::new(method, uri, all.clone().into_iter(), body).unwrap();
        let (ins, _) = sign(req, &params).unwrap().into_parts();
        let mut b = http::Request::builder().method(method).uri(uri);
        for (k, v) in all {
            b = b.header(k, v);
        }
        let mut r = b.body(()).unwrap();
        ins.apply_to_request_http1x(&mut r);
        r
    }

    fn check(r: &http::Request<()>) -> Result<Authenticated, AuthError> {
        verify(r.method().as_str(), r.uri(), r.headers(), &keys(), Utc::now())
    }

    #[test]
    fn accepts_what_the_aws_signer_produces() {
        let uri = "http://127.0.0.1:9000/drive/sp%20ace%2Bplus%25/%C3%A9t%C3%A9.txt?x-voidfs-write&versionId=1.0&list-type=2";
        let r = signed("PUT", uri, &[("x-voidfs-offset", "6"), ("content-type", "text/plain")], b"WORLD", false);
        let a = check(&r).unwrap();
        assert_eq!(a.key.scope, Scope::Admin);
        assert_eq!(a.payload, Payload::Sha256(Sha256::digest(b"WORLD").into()));
    }

    #[test]
    fn accepts_presigned_urls() {
        let r = signed("GET", "http://127.0.0.1:9000/drive/a.txt?versionId=3.0", &[], b"", true);
        assert!(r.uri().query().unwrap().contains("X-Amz-Signature"));
        let a = check(&r).unwrap();
        assert_eq!(a.payload, Payload::Unsigned);
    }

    #[test]
    fn rejects_tampering_unknown_keys_and_unsigned_extension_headers() {
        let r = signed("PUT", "http://127.0.0.1:9000/drive/a.txt?x-voidfs-write", &[("x-voidfs-offset", "6")], b"x", false);
        let mut tampered = r.clone();
        tampered.headers_mut().insert("x-voidfs-offset", "7".parse().unwrap());
        assert_eq!(check(&tampered).unwrap_err(), AuthError::SignatureMismatch);
        let moved: http::Uri = "http://127.0.0.1:9000/drive/b.txt?x-voidfs-write".parse().unwrap();
        assert_eq!(verify("PUT", &moved, r.headers(), &keys(), Utc::now()).unwrap_err(), AuthError::SignatureMismatch);
        let mut extra = r.clone();
        extra.headers_mut().insert("x-voidfs-size", "1".parse().unwrap());
        assert_eq!(check(&extra).unwrap_err(), AuthError::UnsignedExtensionHeader("x-voidfs-size".into()));
        assert_eq!(verify("PUT", r.uri(), r.headers(), &Keys::default(), Utc::now()).unwrap_err(), AuthError::UnknownKey);
        let later = Utc::now() + chrono::Duration::hours(1);
        assert_eq!(verify("PUT", r.uri(), r.headers(), &keys(), later).unwrap_err(), AuthError::Skewed);
        let bare: http::Uri = "http://127.0.0.1:9000/drive/a.txt".parse().unwrap();
        assert_eq!(verify("GET", &bare, &HeaderMap::new(), &keys(), Utc::now()).unwrap_err(), AuthError::Anonymous);
        let v2: http::Uri = "http://127.0.0.1:9000/drive/a.txt?AWSAccessKeyId=VF&Signature=x&Expires=1".parse().unwrap();
        assert_eq!(verify("GET", &v2, &HeaderMap::new(), &keys(), Utc::now()).unwrap_err(), AuthError::SigV2);
    }
}
