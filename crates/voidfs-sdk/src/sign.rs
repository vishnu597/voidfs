// SPDX-License-Identifier: Apache-2.0
//! Signing requests with AWS Signature Version 4 (protocol §2), as the client and the conformance
//! runner send them.

use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings, UriPathNormalizationMode, sign};
use aws_sigv4::sign::v4;
use bytes::Bytes;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::Error;

/// RFC 3986 unreserved characters stay; everything else is encoded, as SigV4 requires of a query.
pub const QUERY: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
/// The same, but `/` stays: a key in a path.
pub const PATH: &AsciiSet = &QUERY.remove(b'/');

/// Signs requests for one endpoint with one access key.
#[derive(Clone)]
pub struct Signer {
    endpoint: String,
    authority: String,
    key_id: String,
    secret: String,
    region: String,
    /// Send `/<drive>/<key>` as `/<key>` to host `<drive>.<domain>` (virtual-host addressing),
    /// still connecting to the endpoint. `/` goes to `<domain>` itself.
    pub virtual_host: Option<String>,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer").field("endpoint", &self.endpoint).field("key_id", &self.key_id).field("region", &self.region).field("virtual_host", &self.virtual_host).finish_non_exhaustive()
    }
}

impl Signer {
    /// `endpoint` is `http(s)://host[:port]`, without a path.
    pub fn new(endpoint: &str, key_id: &str, secret: &str) -> Result<Signer, Error> {
        let endpoint = endpoint.trim_end_matches('/').to_owned();
        let url: http::Uri = endpoint.parse().map_err(|e| Error::Invalid(format!("endpoint {endpoint:?}: {e}")))?;
        if url.scheme().is_none() {
            return Err(Error::Invalid(format!("endpoint {endpoint:?} has no scheme: give http:// or https://")));
        }
        if url.path() != "/" && !url.path().is_empty() {
            return Err(Error::Invalid(format!("endpoint {endpoint:?} has a path")));
        }
        let authority = url.authority().ok_or_else(|| Error::Invalid(format!("endpoint {endpoint:?} has no host")))?.to_string();
        Ok(Signer { endpoint, authority, key_id: key_id.into(), secret: secret.into(), region: "us-east-1".into(), virtual_host: None })
    }

    /// The region in the signing scope; servers accept any (protocol §2).
    pub fn with_region(mut self, region: &str) -> Signer {
        self.region = region.into();
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// A signed request for `path` (path-style and percent-encoded: `/<drive>/<key>`) and `query`
    /// (encoded, without `?`). `headers` are signed except those named in `unsigned`, which are
    /// added after signing; `host` and the payload hash are added.
    pub fn sign(&self, method: &str, path: &str, query: &str, headers: Vec<(String, String)>, unsigned: &[String], body: Bytes) -> Result<http::Request<Bytes>, Error> {
        let (host, path) = self.address(path);
        let uri = if query.is_empty() { format!("{}{}", self.endpoint, path) } else { format!("{}{}?{}", self.endpoint, path, query) };
        let mut headers = headers;
        headers.push(("host".into(), host));
        let (signed, extra): (Vec<_>, Vec<_>) = headers.into_iter().partition(|(k, _)| !unsigned.iter().any(|u| u.eq_ignore_ascii_case(k)));

        let identity = Credentials::new(&self.key_id, &self.secret, None, None, "voidfs").into();
        let mut settings = SigningSettings::default();
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name("s3")
            .time(SystemTime::now())
            .settings(settings)
            .build()
            .map_err(|e| Error::Invalid(format!("cannot sign: {e}")))?
            .into();
        let signable = SignableRequest::new(method, &uri, signed.iter().map(|(k, v)| (k.as_str(), v.as_str())), SignableBody::Bytes(&body))
            .map_err(|e| Error::Invalid(format!("cannot sign: {e}")))?;
        let (instructions, _) = sign(signable, &params).map_err(|e| Error::Invalid(format!("cannot sign: {e}")))?.into_parts();

        let mut builder = http::Request::builder().method(method).uri(&uri);
        for (k, v) in signed.iter().chain(extra.iter()) {
            builder = builder.header(k, v);
        }
        let mut request = builder.body(body).map_err(|e| Error::Invalid(e.to_string()))?;
        instructions.apply_to_request_http1x(&mut request);
        Ok(request)
    }

    /// The host and path a path-style request is sent with.
    pub fn address(&self, path: &str) -> (String, String) {
        let Some(domain) = &self.virtual_host else { return (self.authority.clone(), path.to_owned()) };
        let port = self.authority.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map(|(_, p)| format!(":{p}")).unwrap_or_default();
        let rest = path.strip_prefix('/').unwrap_or(path);
        if rest.is_empty() {
            return (format!("{domain}{port}"), "/".into());
        }
        let (drive, key) = rest.split_once('/').unwrap_or((rest, ""));
        (format!("{drive}.{domain}{port}"), format!("/{key}"))
    }
}

/// `/<drive>`, encoded.
pub fn drive_path(drive: &str) -> String {
    format!("/{}", utf8_percent_encode(drive, QUERY))
}

/// `/<drive>/<key>`, encoded, keeping the key's `/`.
pub fn object_path(drive: &str, key: &str) -> String {
    format!("/{}/{}", utf8_percent_encode(drive, QUERY), utf8_percent_encode(key, PATH))
}

/// A query string from names and values; an empty value is sent as the bare name.
pub fn query_string<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    pairs
        .into_iter()
        .map(|(k, v)| {
            let k = utf8_percent_encode(k, QUERY).to_string();
            if v.is_empty() { k } else { format!("{k}={}", utf8_percent_encode(v, QUERY)) }
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encoded UTF-8 for a header value, as `x-voidfs-source` and `x-voidfs-display-name`
/// carry names (protocol §4.7, §5.1).
pub fn header_text(s: &str) -> String {
    utf8_percent_encode(s, PATH).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_host_addresses() {
        let mut s = Signer::new("http://127.0.0.1:9100", "a", "s").unwrap();
        assert_eq!(s.address("/d/k%20x"), ("127.0.0.1:9100".into(), "/d/k%20x".into()));
        s.virtual_host = Some("s3.localhost".into());
        assert_eq!(s.address("/"), ("s3.localhost:9100".into(), "/".into()));
        assert_eq!(s.address("/d"), ("d.s3.localhost:9100".into(), "/".into()));
        assert_eq!(s.address("/d/"), ("d.s3.localhost:9100".into(), "/".into()));
        assert_eq!(s.address("/d/a/b%20c"), ("d.s3.localhost:9100".into(), "/a/b%20c".into()));
    }

    #[test]
    fn paths_and_queries_are_encoded_once() {
        assert_eq!(object_path("d", "a b/c+d/é~"), "/d/a%20b/c%2Bd/%C3%A9~");
        assert_eq!(object_path("d", "folder/"), "/d/folder/");
        assert_eq!(query_string([("x-voidfs-write", ""), ("versionId", "1.0"), ("prefix", "a b/")]), "x-voidfs-write&versionId=1.0&prefix=a%20b%2F");
        assert_eq!(header_text("a b/ü"), "a%20b/%C3%BC");
    }

    #[test]
    fn endpoints_are_checked() {
        assert!(Signer::new("127.0.0.1:9000", "a", "s").is_err());
        assert!(Signer::new("http://127.0.0.1:9000/x", "a", "s").is_err());
        assert_eq!(Signer::new("http://h:1/", "a", "s").unwrap().endpoint(), "http://h:1");
    }

    #[test]
    fn signing_adds_host_and_payload_hash_and_leaves_unsigned_headers_out() {
        let s = Signer::new("http://h:1", "VFKEY", "secret").unwrap();
        let r = s.sign("PUT", "/d/k", "x-voidfs-write", vec![("x-voidfs-offset".into(), "3".into()), ("x-voidfs-mode".into(), "0644".into())], &["x-voidfs-mode".into()], Bytes::from_static(b"abc")).unwrap();
        assert_eq!(r.uri(), "http://h:1/d/k?x-voidfs-write");
        assert_eq!(r.headers()["host"], "h:1");
        assert_eq!(r.headers()["x-amz-content-sha256"], "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let auth = r.headers()["authorization"].to_str().unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=VFKEY/"), "{auth}");
        assert!(auth.contains("x-voidfs-offset") && !auth.contains("x-voidfs-mode"), "{auth}");
        assert_eq!(r.headers()["x-voidfs-mode"], "0644");
    }
}
