// SPDX-License-Identifier: Apache-2.0
//! Request parsing and response helpers shared by the handlers.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use http::{HeaderMap, Method};
use http_body_util::BodyExt;
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};
use voidfs_core::ids::{Timestamp, VersionId};
use voidfs_core::model::{Attrs, Kind};
use voidfs_core::ops::{AttrsPatch, Precondition};

use super::S3Error;
use super::App;
use crate::pool::Drive;
use crate::sigv4::{Authenticated, Payload, Scope};

pub struct Ctx {
    pub method: Method,
    pub bucket: Option<String>,
    pub key: Option<String>,
    pub query: Query,
    pub headers: HeaderMap,
    pub auth: Authenticated,
}

impl Ctx {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn require(&self, scope: Scope) -> Result<(), S3Error> {
        if self.auth.key.scope >= scope {
            Ok(())
        } else {
            Err(S3Error::denied(format!("this key's scope does not allow {} requests here", self.method)))
        }
    }

    pub fn actor(&self) -> voidfs_core::model::Actor {
        voidfs_core::model::Actor { kind: "key".into(), id: self.auth.key.id.clone() }
    }

    pub fn bucket(&self) -> &str {
        self.bucket.as_deref().unwrap_or_default()
    }

    pub fn key(&self) -> &str {
        self.key.as_deref().unwrap_or_default()
    }

    /// The live drive this request names, if this key may reach it.
    pub fn drive(&self, app: &App) -> Result<Arc<Drive>, S3Error> {
        self.drive_named(app, self.bucket())
    }

    pub fn drive_named(&self, app: &App, name: &str) -> Result<Arc<Drive>, S3Error> {
        let d = app.pool.drive(name).ok_or_else(S3Error::no_bucket)?;
        if !self.auth.key.reaches(&d.alias, d.id.as_str()) {
            return Err(S3Error::no_bucket());
        }
        Ok(d)
    }

    /// Request preconditions for writes (protocol §4.0).
    pub fn precondition(&self) -> Precondition {
        Precondition {
            // A version id that does not parse can never match: use one that cannot exist.
            if_version: self.header("x-voidfs-if-version").map(|v| v.parse().unwrap_or(VersionId::new(u64::MAX, u32::MAX))),
            if_match: self.header("if-match").map(str::to_owned),
            if_none_match_any: self.header("if-none-match").is_some_and(|v| v.trim() == "*"),
        }
    }

    /// `x-voidfs-mtime` and `x-voidfs-mode` as an attribute patch.
    pub fn attrs_patch(&self) -> Result<AttrsPatch, S3Error> {
        let mut p = AttrsPatch::default();
        if let Some(t) = self.header("x-voidfs-mtime") {
            p.mtime = Some(t.parse::<Timestamp>().map_err(|_| S3Error::invalid("x-voidfs-mtime is not an RFC 3339 timestamp"))?);
        }
        if let Some(m) = self.header("x-voidfs-mode") {
            p.mode = Some(parse_mode(m)?);
        }
        Ok(p)
    }

    /// Attributes for a new object from the request headers.
    pub fn attrs_for_put(&self) -> Result<Attrs, S3Error> {
        let mut a = Attrs::default();
        if let Some(ct) = self.header("content-type") {
            a.content_type = Some(ct.to_owned());
        }
        for (name, value) in &self.headers {
            if let Some(m) = name.as_str().strip_prefix("x-amz-meta-") {
                a.meta.insert(m.to_owned(), value.to_str().unwrap_or_default().to_owned());
            }
        }
        let p = self.attrs_patch()?;
        a.mtime = p.mtime;
        a.mode = p.mode;
        Ok(a)
    }

    pub fn max_keys(&self) -> Result<usize, S3Error> {
        match self.query.get("max-keys") {
            None => Ok(1000),
            Some(v) => v.parse::<usize>().map(|n| n.min(1000)).map_err(|_| S3Error::invalid("max-keys must be a number")),
        }
    }
}

pub fn parse_mode(m: &str) -> Result<u32, S3Error> {
    u32::from_str_radix(m, 8).ok().filter(|&v| v <= 0o7777).ok_or_else(|| S3Error::invalid(format!("mode {m:?} is not octal permission bits")))
}

pub fn format_mode(mode: Option<u32>, kind: Kind) -> String {
    format!("{:04o}", mode.unwrap_or(if kind == Kind::Folder { 0o755 } else { 0o644 }))
}

/// Decoded query parameters. A bare `name` has an empty value.
pub struct Query(pub BTreeMap<String, String>);

impl Query {
    pub fn parse(raw: &str) -> Query {
        let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
        Query(
            raw.split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    (decode(k), decode(v))
                })
                .collect(),
        )
    }

    pub fn has(&self, k: &str) -> bool {
        self.0.contains_key(k)
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(String::as_str)
    }

    /// Names that are not in `known`, ignoring presigned-URL parameters and response overrides.
    pub fn unknown(&self, known: &[&str]) -> Option<&str> {
        self.0
            .keys()
            .map(String::as_str)
            .find(|k| !known.contains(k) && !k.starts_with("X-Amz-") && !k.starts_with("x-amz-") && !k.starts_with("response-"))
    }
}

/// Splits a request path into a drive name and a key, both decoded.
pub fn split_path(path: &str) -> Result<(Option<String>, Option<String>), S3Error> {
    let decode = |s: &str| {
        percent_decode_str(s).decode_utf8().map(|c| c.into_owned()).map_err(|_| S3Error::invalid("the path is not valid UTF-8"))
    };
    let rest = path.strip_prefix('/').unwrap_or(path);
    if rest.is_empty() {
        return Ok((None, None));
    }
    match rest.split_once('/') {
        None => Ok((Some(decode(rest)?), None)),
        Some((b, "")) => Ok((Some(decode(b)?), None)),
        Some((b, k)) => Ok((Some(decode(b)?), Some(decode(k)?))),
    }
}

pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// `Last-Modified` form.
pub fn http_date(t: Timestamp) -> String {
    t.datetime().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// ISO 8601 with milliseconds, as S3 listings use.
pub fn iso(t: Timestamp) -> String {
    t.datetime().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub fn xml(status: u16, body: String) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/xml")
        .body(Body::from(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{body}")))
        .unwrap()
}

pub fn json(value: &serde_json::Value) -> Response {
    Response::builder().status(200).header("content-type", "application/json").body(Body::from(value.to_string())).unwrap()
}

pub fn empty(status: u16) -> http::response::Builder {
    Response::builder().status(status)
}

pub const S3_NS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

// ---------------------------------------------------------------------------------------------
// Bodies

/// Reads a request body: decodes `aws-chunked`, enforces a size limit, and checks the signed
/// payload hash and any `x-amz-checksum-*` header or trailer before the caller commits.
pub struct BodyReader {
    body: Body,
    hasher: Option<(Sha256, [u8; 32])>,
    chunked: Option<super::chunked::Decoder>,
    pending: std::collections::VecDeque<Bytes>,
    /// A checksum the client promised, as a header or a trailer: (name, running digest, value).
    checksum: Option<(String, super::chunked::Checksum, Option<String>)>,
    decoded_length: Option<u64>,
    read: u64,
    limit: u64,
}

impl BodyReader {
    pub fn new(body: Body, ctx: &Ctx, limit: u64) -> Result<Self, S3Error> {
        let mut hasher = None;
        let mut chunked = None;
        match &ctx.auth.payload {
            Payload::Sha256(h) => hasher = Some((Sha256::new(), *h)),
            Payload::Unsigned => {}
            Payload::Streaming { signed, .. } => {
                let signing = signed.then(|| super::chunked::Signing {
                    key: ctx.auth.signing_key,
                    amz_date: ctx.auth.amz_date.clone(),
                    scope: ctx.auth.scope.clone(),
                    seed_signature: ctx.auth.seed_signature.clone(),
                });
                chunked = Some(super::chunked::Decoder::new(signing));
            }
        }
        // A checksum named by x-amz-trailer arrives at the end; one in a header is known now.
        let mut checksum = None;
        if let Some(name) = ctx.header("x-amz-trailer")
            && let Some(c) = super::chunked::Checksum::for_header(name.trim()) {
                checksum = Some((name.trim().to_ascii_lowercase(), c, None));
            }
        for (name, value) in &ctx.headers {
            if let Some(c) = super::chunked::Checksum::for_header(name.as_str()) {
                checksum = Some((name.as_str().to_owned(), c, value.to_str().ok().map(str::to_owned)));
            }
        }
        let decoded_length = ctx.header("x-amz-decoded-content-length").and_then(|v| v.parse().ok());
        Ok(BodyReader { body, hasher, chunked, pending: Default::default(), checksum, decoded_length, read: 0, limit })
    }

    /// The next piece of content, or `None` at the end.
    pub async fn next(&mut self) -> Result<Option<Bytes>, S3Error> {
        loop {
            if let Some(b) = self.pending.pop_front() {
                self.read += b.len() as u64;
                if self.read > self.limit {
                    return Err(S3Error::new(413, "EntityTooLarge", format!("the body exceeds {} bytes", self.limit)));
                }
                if let Some((_, c, _)) = &mut self.checksum {
                    c.update(&b);
                }
                return Ok(Some(b));
            }
            let Some(frame) = self.body.frame().await else { return Ok(None) };
            let frame = frame.map_err(|e| S3Error::new(400, "IncompleteBody", e.to_string()))?;
            let Ok(data) = frame.into_data() else { continue };
            if let Some((h, _)) = &mut self.hasher {
                h.update(&data);
            }
            match &mut self.chunked {
                Some(d) => {
                    let out = d.feed(&data).map_err(chunk_error)?;
                    self.pending.extend(out.into_iter().filter(|b| !b.is_empty()));
                }
                None if !data.is_empty() => self.pending.push_back(data),
                None => {}
            }
        }
    }

    /// Call after the last piece: fails if the body does not match what was signed or promised.
    pub fn finish(self) -> Result<(), S3Error> {
        if let Some((h, want)) = self.hasher {
            let got: [u8; 32] = h.finalize().into();
            if got != want {
                return Err(S3Error::new(400, "XAmzContentSHA256Mismatch", "the body does not match x-amz-content-sha256"));
            }
        }
        let mut trailers = Vec::new();
        if let Some(d) = self.chunked {
            let (t, n) = d.finish().map_err(chunk_error)?;
            if self.decoded_length.is_some_and(|want| want != n) {
                return Err(S3Error::new(400, "IncompleteBody", "the body does not match x-amz-decoded-content-length"));
            }
            trailers = t;
        }
        if let Some((name, c, header_value)) = self.checksum {
            let promised = header_value.or_else(|| trailers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone()));
            if let Some(want) = promised
                && c.finish() != want {
                    return Err(S3Error::new(400, "BadDigest", format!("the {name} checksum does not match the body")));
                }
        }
        Ok(())
    }

    pub async fn read_all(mut self) -> Result<Bytes, S3Error> {
        let mut buf = bytes::BytesMut::new();
        while let Some(d) = self.next().await? {
            buf.extend_from_slice(&d);
        }
        self.finish()?;
        Ok(buf.freeze())
    }
}

fn chunk_error(e: super::chunked::ChunkError) -> S3Error {
    use super::chunked::ChunkError::*;
    match e {
        Signature => S3Error::new(403, "SignatureDoesNotMatch", e.to_string()),
        Malformed(_) | Incomplete => S3Error::new(400, "IncompleteBody", e.to_string()),
    }
}

/// Largest extension body (protocol §7).
pub const MAX_EXTENSION_BODY: u64 = 64 * 1024 * 1024;
/// Largest single PutObject (protocol §7).
pub const MAX_PUT: u64 = 5 * 1024 * 1024 * 1024;
/// Largest small request body we buffer (XML, JSON).
pub const MAX_SMALL_BODY: u64 = 4 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_and_decode() {
        assert_eq!(split_path("/").unwrap(), (None, None));
        assert_eq!(split_path("/d").unwrap(), (Some("d".into()), None));
        assert_eq!(split_path("/d/").unwrap(), (Some("d".into()), None));
        assert_eq!(split_path("/d/a%20b/%C3%A9/").unwrap(), (Some("d".into()), Some("a b/é/".into())));
        assert!(split_path("/d/%FF").is_err());
    }

    #[test]
    fn queries_decode() {
        let q = Query::parse("x-voidfs-write&versionId=1.0&prefix=sp%20ace%2B");
        assert!(q.has("x-voidfs-write"));
        assert_eq!(q.get("versionId"), Some("1.0"));
        assert_eq!(q.get("prefix"), Some("sp ace+"));
        assert_eq!(q.unknown(&["x-voidfs-write", "versionId", "prefix"]), None);
        assert_eq!(Query::parse("policy").unknown(&["versions"]), Some("policy"));
    }

    #[test]
    fn modes() {
        assert_eq!(parse_mode("0600").unwrap(), 0o600);
        assert_eq!(parse_mode("755").unwrap(), 0o755);
        assert!(parse_mode("0999").is_err());
        assert_eq!(format_mode(None, Kind::Folder), "0755");
        assert_eq!(format_mode(Some(0o600), Kind::File), "0600");
    }
}
