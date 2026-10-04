// SPDX-License-Identifier: Apache-2.0
//! The bucket capability probe: whether a bucket gives the format what it needs, checked when a
//! server starts and reported in full by `voidfs-server probe`.
//!
//! What decides whether a pool may be written is create-if-absent (format §7.2). OpenDAL's
//! capability flags describe its S3 driver, not the endpoint behind it, so the only way to know
//! is to try. [`create_if_absent`] creates `voidfs.json` again with its exact bytes, which must
//! fail because the object exists. That stores nothing new under the pool's root (§2): a bucket
//! that honours the condition refuses the write, and one that ignores it rewrites the same bytes.
//!
//! The rest reads the bucket's configuration, which OpenDAL does not expose, with signed requests
//! of its own ([`Bucket`]). A lifecycle rule that would delete the pool's objects, or move them
//! where they can't be read, stops a server from starting. Versioning, which keeps deleted
//! objects billed, is a warning. Presigned URLs, CORS, object lock, modification times and the
//! bucket's clock are only reported. What a presigned PUT binds decides whether a server offers
//! direct uploads ([`presigned_puts`]), and what minted credentials reach whether it offers storage
//! credentials ([`crate::credentials::check`]); it checks both when it starts.
//!
//! Some things can't be checked without new objects, which the format does not allow under the
//! root: that create-if-absent holds when writers race, and that reads and listings see writes at
//! once. The conformance suite and the store's ignored tests check those against a bucket.

use std::fmt;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use opendal::Buffer;
use opendal::raw::HttpClient;
use reqsign_aws_v4::{Credential, DefaultCredentialProvider, RequestSigner, StaticCredentialProvider};
use reqsign_core::{OsEnv, ProvideCredentialChain, Signer};
use voidfs_core::model::{CommitGuard, PoolDescriptor};

use crate::store::Store;

/// The pool descriptor (format §3), which the create-if-absent check creates again.
pub const DESCRIPTOR: &str = "voidfs.json";

/// Storage classes whose objects can't be read until they are restored.
const COLD: &[&str] = &["GLACIER", "DEEP_ARCHIVE"];

/// What stays as it is in a key in a URL: unreserved characters and `/`.
const KEY: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~').remove(b'/');

// ---------------------------------------------------------------------------------------------
// Create-if-absent

/// What creating an existing object again did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conditional {
    /// It failed because the object exists: the store honours create-if-absent.
    Honoured,
    /// It succeeded: the store ignored the condition and overwrote the object.
    Ignored,
    /// It failed some other way, so nothing is known.
    Failed(String),
}

/// Creates `path` again with `bytes`, which must be its current content, to see whether the
/// store refuses (format §7.2). A store that ignores the condition rewrites the same bytes.
pub async fn create_if_absent(store: &Store, path: &str, bytes: Bytes) -> Conditional {
    match store.put_new(path, bytes).await {
        Ok(false) => Conditional::Honoured,
        Ok(true) => Conditional::Ignored,
        Err(e) => Conditional::Failed(format!("{e:#}")),
    }
}

impl Conditional {
    /// Why a pool that relies on create-if-absent must not be written here, if it must not.
    pub fn refusal(&self) -> Option<String> {
        match self {
            Conditional::Honoured => None,
            Conditional::Ignored => Some(
                "the bucket ignores create-if-absent writes (If-None-Match: *): creating voidfs.json again succeeded, where it must fail. \
                 Two servers could then both write the same commit, and a drive's history would fork without an error (format §7.2)"
                    .into(),
            ),
            Conditional::Failed(e) => Some(format!("could not confirm that the bucket honours create-if-absent writes (If-None-Match: *): creating voidfs.json again failed: {e}")),
        }
    }
}

impl fmt::Display for Conditional {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Conditional::Honoured => write!(f, "honoured: creating voidfs.json again was refused"),
            Conditional::Ignored => write!(f, "IGNORED: creating voidfs.json again succeeded (it rewrote the same bytes)"),
            Conditional::Failed(e) => write!(f, "unknown: creating voidfs.json again failed: {e}"),
        }
    }
}

/// Why a server started with `--commit-guard wanted` must not write a pool that declares
/// `declared`, if it must not. A pool keeps its guard for life (format §3).
pub fn guard_mismatch(declared: CommitGuard, wanted: CommitGuard) -> Option<String> {
    match (declared, wanted) {
        (CommitGuard::External, CommitGuard::CreateIfAbsent) => Some(
            "this pool uses the external commit guard (format §7.3): at most one server may write it. If this server is that one, start it with --commit-guard external".into(),
        ),
        (CommitGuard::CreateIfAbsent, CommitGuard::External) => Some(
            "this pool uses the create-if-absent commit guard (format §7.2), and a pool keeps its guard for life; start the server without --commit-guard external".into(),
        ),
        _ => None,
    }
}

pub fn guard_name(g: CommitGuard) -> &'static str {
    match g {
        CommitGuard::CreateIfAbsent => "create-if-absent",
        CommitGuard::External => "external",
    }
}

// ---------------------------------------------------------------------------------------------
// The bucket's configuration

/// Which service a bucket is on, judged from its endpoint, for advice that differs between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Aws,
    R2,
    Other,
}

/// Signed requests for a bucket's own configuration (lifecycle rules, versioning and so on),
/// which OpenDAL does not make. Credentials come from the same sources as the store's.
pub struct Bucket {
    http: HttpClient,
    signer: Signer<Credential>,
    /// The same credentials, for STS (storage credentials, protocol §5.5).
    sts_signer: Signer<Credential>,
    name: String,
    region: String,
    /// The endpoint, as a URL.
    base: String,
    /// The bucket's URL, path-style, as OpenDAL addresses it.
    url: String,
    /// The pool's root as a key prefix: empty, or ending in `/`.
    prefix: String,
    provider: Provider,
}

/// Lets reqsign fetch credentials (instance metadata, STS) through OpenDAL's HTTP client.
#[derive(Clone, Debug)]
struct ViaOpendal(HttpClient);

impl reqsign_core::HttpSend for ViaOpendal {
    async fn http_send(&self, req: http::Request<Bytes>) -> reqsign_core::Result<http::Response<Bytes>> {
        let (parts, body) = req.into_parts();
        let resp = self.0.send(http::Request::from_parts(parts, Buffer::from(body))).await.map_err(|e| reqsign_core::Error::unexpected(e.to_string()))?;
        let (parts, body) = resp.into_parts();
        Ok(http::Response::from_parts(parts, body.to_bytes()))
    }
}

/// One of the bucket's configuration documents, or why it could not be read.
#[derive(Debug)]
enum Doc {
    Found(String),
    /// The bucket has none.
    Absent,
    Unchecked(Unchecked),
}

/// Why a check has no result.
#[derive(Debug)]
enum Unchecked {
    /// The service does not have the feature, so the bucket can't have it set either.
    Unsupported,
    Failed(String),
}

impl fmt::Display for Unchecked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unchecked::Unsupported => write!(f, "the service does not support it"),
            Unchecked::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// A reply to a [`Bucket`] request.
struct Reply {
    doc: Doc,
    /// How far the bucket's clock (its `Date` header) was ahead of this machine's when the reply
    /// arrived, in whole seconds.
    skew: Option<i64>,
}

impl Bucket {
    /// `prefix` is the pool's root in the bucket, as in `s3:<bucket>/<prefix>`. The other
    /// arguments are as for [`Store::s3`].
    pub fn new(bucket: &str, prefix: &str, endpoint: Option<&str>, region: &str, credentials: Option<(&str, &str)>) -> anyhow::Result<Bucket> {
        let http = HttpClient::new()?;
        let ctx = reqsign_core::Context::new().with_file_read(reqsign_file_read_tokio::TokioFileRead).with_http_send(ViaOpendal(http.clone())).with_env(OsEnv);
        let chain = || match credentials {
            Some((id, secret)) => ProvideCredentialChain::new().push(StaticCredentialProvider::new(id, secret)),
            None => ProvideCredentialChain::new().push(DefaultCredentialProvider::builder().build()),
        };
        let signer = Signer::new(ctx.clone(), chain(), RequestSigner::new("s3", region));
        let sts_signer = Signer::new(ctx, chain(), RequestSigner::new("sts", region));
        let base = match endpoint {
            Some(e) if e.starts_with("http://") || e.starts_with("https://") => e.trim_end_matches('/').to_owned(),
            Some(e) => format!("https://{}", e.trim_end_matches('/')),
            None => format!("https://s3.{region}.amazonaws.com"),
        };
        let host = base.split("://").nth(1).unwrap_or_default().split(['/', ':']).next().unwrap_or_default().to_ascii_lowercase();
        let provider = if host.ends_with(".r2.cloudflarestorage.com") {
            Provider::R2
        } else if host.ends_with(".amazonaws.com") {
            Provider::Aws
        } else {
            Provider::Other
        };
        let prefix = prefix.trim_matches('/');
        Ok(Bucket {
            http,
            signer,
            sts_signer,
            name: bucket.to_owned(),
            region: region.to_owned(),
            url: format!("{base}/{bucket}"),
            base,
            prefix: if prefix.is_empty() { String::new() } else { format!("{prefix}/") },
            provider,
        })
    }

    /// Where storage credentials for the pool reach (protocol §5.5).
    pub fn location(&self) -> crate::credentials::Location {
        crate::credentials::Location { bucket: self.name.clone(), root: self.prefix.clone(), region: self.region.clone(), endpoint: self.base.clone() }
    }

    /// What mints storage credentials through STS for this bucket, or why its service can't: AWS's
    /// STS needs a role to assume, and R2 has none. Elsewhere STS is tried on the bucket's own
    /// endpoint, as MinIO serves it.
    pub fn sts(&self, opts: &crate::credentials::StsOptions) -> Result<crate::credentials::Sts, String> {
        let endpoint = match (&opts.endpoint, self.provider, &opts.role) {
            (_, Provider::R2, _) => return Err("R2 mints temporary credentials through Cloudflare's API with an API token, which this server does not use yet".into()),
            (None, Provider::Aws, None) => return Err("AWS STS needs a role to assume (--storage-credentials-role)".into()),
            (Some(e), _, _) => e.trim_end_matches('/').to_owned(),
            (None, Provider::Aws, _) => format!("https://sts.{}.amazonaws.com", self.region),
            (None, Provider::Other, _) => self.base.clone(),
        };
        Ok(crate::credentials::Sts { http: self.http.clone(), signer: self.sts_signer.clone(), endpoint: format!("{endpoint}/"), role: opts.role.clone(), bucket: self.name.clone(), root: self.prefix.clone() })
    }

    /// The HTTP client its requests go through.
    pub fn http(&self) -> &HttpClient {
        &self.http
    }

    /// Where the pool is, in words.
    fn scope(&self) -> String {
        if self.prefix.is_empty() { "the bucket".into() } else { format!("the pool's prefix {:?}", self.prefix) }
    }

    /// Reads the configuration document `what` (for example `lifecycle`).
    async fn read(&self, what: &str) -> Reply {
        let sent = Utc::now();
        let resp = async {
            let (mut parts, ()) = http::Request::get(format!("{}?{what}", self.url)).header("x-amz-content-sha256", reqsign_aws_v4::EMPTY_STRING_SHA256).body(())?.into_parts();
            self.signer.sign(&mut parts, None).await.map_err(|e| anyhow!("signing the request: {e}"))?;
            anyhow::Ok(self.http.send(http::Request::from_parts(parts, Buffer::new())).await?)
        }
        .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => return Reply { doc: Doc::Unchecked(Unchecked::Failed(format!("{e:#}"))), skew: None },
        };
        let arrived = Utc::now();
        let date = resp.headers().get(http::header::DATE).and_then(|v| v.to_str().ok()).and_then(|v| DateTime::parse_from_rfc2822(v).ok());
        // The Date header has whole seconds: compare it with the middle of the round trip.
        let skew = date.map(|d| (d.with_timezone(&Utc) - (sent + (arrived - sent) / 2)).num_milliseconds() as f64 / 1000.0).map(|s| s.round() as i64);
        let status = resp.status().as_u16();
        let text = String::from_utf8_lossy(&resp.into_body().to_bytes()).into_owned();
        let code = error_code(&text);
        let doc = match (status, code.as_deref()) {
            (200, _) => Doc::Found(text),
            (404, Some("NoSuchLifecycleConfiguration" | "NoSuchCORSConfiguration" | "ObjectLockConfigurationNotFoundError")) => Doc::Absent,
            (403, c) => Doc::Unchecked(Unchecked::Failed(format!("these credentials may not read it ({})", c.unwrap_or("HTTP 403")))),
            (501, _) | (_, Some("NotImplemented")) => Doc::Unchecked(Unchecked::Unsupported),
            (s, c) => Doc::Unchecked(Unchecked::Failed(format!("HTTP {s}{}", c.map(|c| format!(" {c}")).unwrap_or_default()))),
        };
        Reply { doc, skew }
    }

    /// What the bucket's lifecycle rules mean for the pool, or why they could not be read.
    async fn lifecycle(&self) -> Result<Lifecycle, Unchecked> {
        match self.read("lifecycle").await.doc {
            Doc::Found(xml) => parse_lifecycle(&xml).map(|rules| assess(&rules, &self.prefix)).map_err(|e| Unchecked::Failed(format!("they could not be parsed ({e:#})"))),
            Doc::Absent => Ok(Lifecycle::default()),
            Doc::Unchecked(u) => Err(u),
        }
    }

    /// A PUT of `path` (relative to the pool's root), presigned with `headers` among the signed
    /// ones, path-style as OpenDAL addresses the bucket.
    pub async fn presign_put(&self, path: &str, headers: &[(String, String)], expires: Duration) -> anyhow::Result<crate::direct::Presigned> {
        let key = format!("{}{path}", self.prefix);
        let url = format!("{}/{}", self.url, percent_encoding::utf8_percent_encode(&key, KEY));
        let mut b = http::Request::put(url);
        for (k, v) in headers {
            b = b.header(k, v);
        }
        let (mut parts, ()) = b.body(())?.into_parts();
        self.signer.sign(&mut parts, Some(expires)).await.map_err(|e| anyhow!("presigning a PUT of {path}: {e}"))?;
        let headers = parts.headers.iter().filter(|(k, _)| *k != http::header::HOST).map(|(k, v)| Ok((k.as_str().to_owned(), v.to_str()?.to_owned()))).collect::<anyhow::Result<_>>()?;
        Ok(crate::direct::Presigned { url: parts.uri.to_string(), headers })
    }

    async fn versioning(&self) -> Result<Versioning, Unchecked> {
        match self.read("versioning").await.doc {
            Doc::Found(xml) => parse_versioning(&xml).map_err(|e| Unchecked::Failed(format!("it could not be parsed ({e:#})"))),
            Doc::Absent => Ok(Versioning::Off),
            Doc::Unchecked(u) => Err(u),
        }
    }
}

/// The `Code` of an S3 error document.
pub(crate) fn error_code(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let code = doc.root_element().children().find(|n| n.has_tag_name("Code"))?.text()?;
    Some(code.trim().to_owned())
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text<'a>(n: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(n, name).and_then(|c| c.text()).map(str::trim)
}

/// A lifecycle rule, as far as the pool is concerned.
#[derive(Debug, Default, PartialEq)]
struct Rule {
    id: String,
    enabled: bool,
    /// The key prefix it applies to: empty for the whole bucket.
    prefix: String,
    /// It applies only to objects with certain tags, and voidfs tags none.
    tagged: bool,
    /// Its size condition, in words.
    size: Option<String>,
    /// When it expires current objects, in words ("after 30 days").
    expires: Option<String>,
    /// Where it moves current objects, and when.
    transitions: Vec<(String, String)>,
    noncurrent_expires: bool,
}

fn when(n: roxmltree::Node) -> String {
    match (text(n, "Days"), text(n, "Date")) {
        (Some("1"), _) => "after 1 day".into(),
        (Some(d), _) => format!("after {d} days"),
        (None, Some(d)) => format!("on {d}"),
        (None, None) => "at some point".into(),
    }
}

fn parse_lifecycle(xml: &str) -> anyhow::Result<Vec<Rule>> {
    let doc = roxmltree::Document::parse(xml)?;
    let mut rules = Vec::new();
    for r in doc.root_element().children().filter(|n| n.has_tag_name("Rule")) {
        let mut rule = Rule { id: text(r, "ID").unwrap_or_default().to_owned(), enabled: text(r, "Status") == Some("Enabled"), ..Rule::default() };
        // A filter is a prefix, a tag, a size, or an `And` of them; older rules have a bare
        // `Prefix`. No filter at all means the whole bucket.
        let filter = child(r, "Filter").map(|f| child(f, "And").unwrap_or(f));
        if let Some(f) = filter {
            rule.prefix = text(f, "Prefix").unwrap_or_default().to_owned();
            rule.tagged = child(f, "Tag").is_some();
            let size: Vec<String> = [("ObjectSizeGreaterThan", "larger"), ("ObjectSizeLessThan", "smaller")]
                .into_iter()
                .filter_map(|(el, word)| text(f, el).map(|n| format!("{word} than {n} bytes")))
                .collect();
            if !size.is_empty() {
                rule.size = Some(format!("(only objects {})", size.join(" and ")));
            }
        } else {
            rule.prefix = text(r, "Prefix").unwrap_or_default().to_owned();
        }
        if let Some(e) = child(r, "Expiration")
            && (text(e, "Days").is_some() || text(e, "Date").is_some())
        {
            rule.expires = Some(when(e));
        }
        for t in r.children().filter(|n| n.has_tag_name("Transition")) {
            rule.transitions.push((text(t, "StorageClass").unwrap_or("an unnamed storage class").to_owned(), when(t)));
        }
        rule.noncurrent_expires = child(r, "NoncurrentVersionExpiration").is_some();
        rules.push(rule);
    }
    Ok(rules)
}

/// What a bucket's lifecycle rules mean for the pool.
#[derive(Debug, Default, PartialEq)]
struct Lifecycle {
    /// Rules that would delete the pool's objects, or make them unreadable.
    fatal: Vec<String>,
    /// Rules that change what the pool's objects cost.
    warnings: Vec<String>,
    /// Rules in the bucket, and how many of them apply to the pool's objects.
    rules: usize,
    covering: usize,
    /// Some rule expires noncurrent versions of the pool's objects.
    noncurrent_expire: bool,
}

/// Which of `rules` touch objects under `prefix` (the pool's root), and how.
fn assess(rules: &[Rule], prefix: &str) -> Lifecycle {
    let mut out = Lifecycle { rules: rules.len(), ..Lifecycle::default() };
    for r in rules {
        let overlaps = r.prefix.starts_with(prefix) || prefix.starts_with(&r.prefix);
        if !r.enabled || r.tagged || !overlaps {
            continue;
        }
        out.covering += 1;
        let name = if r.id.is_empty() { "a lifecycle rule".to_owned() } else { format!("lifecycle rule {:?}", r.id) };
        let mut what = if r.prefix.is_empty() { "every object in the bucket".to_owned() } else { format!("objects under {:?}", r.prefix) };
        if let Some(s) = &r.size {
            what = format!("{what} {s}");
        }
        if let Some(w) = &r.expires {
            out.fatal.push(format!("{name} deletes {what} {w}"));
        }
        for (class, w) in &r.transitions {
            if COLD.contains(&class.as_str()) {
                out.fatal.push(format!("{name} moves {what} to {class} {w}, where they can't be read until restored"));
            } else {
                out.warnings.push(format!("{name} moves {what} to {class} {w}: reads may cost more, and objects deleted soon after may be billed for a minimum period"));
            }
        }
        out.noncurrent_expire |= r.noncurrent_expires;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Versioning {
    Off,
    Enabled,
    Suspended,
}

fn parse_versioning(xml: &str) -> anyhow::Result<Versioning> {
    if xml.trim().is_empty() {
        return Ok(Versioning::Off);
    }
    let doc = roxmltree::Document::parse(xml)?;
    Ok(match text(doc.root_element(), "Status") {
        Some("Enabled") => Versioning::Enabled,
        Some("Suspended") => Versioning::Suspended,
        _ => Versioning::Off,
    })
}

/// Why versioning costs money here, if it does.
fn versioning_warning(v: Versioning, noncurrent_expire: bool) -> Option<&'static str> {
    match v {
        Versioning::Enabled if !noncurrent_expire => Some(
            "bucket versioning is on, and no lifecycle rule expires old versions of the pool's objects: everything voidfs deletes, \
             such as garbage-collected shards, stays stored and billed",
        ),
        Versioning::Suspended if !noncurrent_expire => Some("bucket versioning is suspended: old versions kept while it was on stay stored and billed"),
        _ => None,
    }
}

fn describe_cors(xml: &str) -> anyhow::Result<String> {
    let doc = roxmltree::Document::parse(xml)?;
    let rules: Vec<_> = doc.root_element().children().filter(|n| n.has_tag_name("CORSRule")).collect();
    let mut origins: Vec<&str> = Vec::new();
    for r in &rules {
        if r.children().any(|m| m.has_tag_name("AllowedMethod") && m.text().map(str::trim) == Some("PUT")) {
            origins.extend(r.children().filter(|o| o.has_tag_name("AllowedOrigin")).filter_map(|o| o.text()).map(str::trim));
        }
    }
    let n = rules.len();
    Ok(if origins.is_empty() {
        format!("{n} rule(s), none allowing PUT: uploads from a browser will need one")
    } else {
        format!("{n} rule(s); PUT allowed from {}", origins.join(", "))
    })
}

fn describe_object_lock(xml: &str) -> anyhow::Result<String> {
    let doc = roxmltree::Document::parse(xml)?;
    let root = doc.root_element();
    if text(root, "ObjectLockEnabled") != Some("Enabled") {
        return Ok("off".into());
    }
    let retention = child(root, "Rule").and_then(|r| child(r, "DefaultRetention"));
    Ok(match retention {
        Some(d) => {
            let period = text(d, "Days").map(|n| format!("{n} days")).or_else(|| text(d, "Years").map(|n| format!("{n} years"))).unwrap_or_default();
            format!("on, keeping new objects {} for {period}: garbage collection can't reclaim them until then", text(d, "Mode").unwrap_or("locked"))
        }
        None => "on, with no default retention".into(),
    })
}

// ---------------------------------------------------------------------------------------------
// At start, and in full

/// The checks of the bucket's configuration that a server makes before it opens its pool. Fails
/// if a lifecycle rule would delete the pool's objects or make them unreadable; logs what would
/// only cost money, and what could not be checked.
pub async fn check_bucket(b: &Bucket) -> anyhow::Result<()> {
    let (lifecycle, versioning) = tokio::join!(b.lifecycle(), b.versioning());
    let noncurrent_expire = match &lifecycle {
        Ok(l) => {
            for w in &l.warnings {
                tracing::warn!("{w}");
            }
            if !l.fatal.is_empty() {
                bail!("{}. voidfs keeps objects for as long as anything refers to them; remove the rule, or scope it away from {}", l.fatal.join("; "), b.scope());
            }
            l.noncurrent_expire
        }
        Err(Unchecked::Unsupported) => {
            tracing::info!("the bucket's lifecycle rules were not checked: the service does not support them");
            false
        }
        Err(Unchecked::Failed(why)) => {
            tracing::warn!("the bucket's lifecycle rules were not checked: {why}. Make sure none deletes or archives objects under {}", b.scope());
            false
        }
    };
    match versioning {
        Ok(v) => {
            if let Some(w) = versioning_warning(v, noncurrent_expire) {
                tracing::warn!("{w}");
            }
        }
        Err(why) => tracing::info!("the bucket's versioning was not checked: {why}"),
    }
    Ok(())
}

/// What `voidfs-server probe` found: a line per check, and whether a server would start.
#[derive(Default)]
pub struct Report {
    rows: Vec<(&'static str, String)>,
    /// Why a server started with the same options would refuse to open the pool.
    pub refusals: Vec<String>,
}

impl Report {
    fn row(&mut self, check: &'static str, result: impl Into<String>) {
        self.rows.push((check, result.into()));
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (check, result) in &self.rows {
            writeln!(f, "{check:<21} {result}")?;
        }
        writeln!(f)?;
        if self.refusals.is_empty() {
            return write!(f, "A server started with these options would open the pool.");
        }
        write!(f, "A server started with these options would refuse to open the pool:")?;
        for r in &self.refusals {
            write!(f, "\n- {r}")?;
        }
        Ok(())
    }
}

/// Every check, for a server that would be started with `--commit-guard guard` and `sts`. It
/// changes nothing a pool holds: the writes are the create-if-absent check's, which a bucket that
/// honours the condition refuses, and PUTs of [`PROBE_SHARD`], a valid shard, presigned and with
/// minted storage credentials.
pub async fn report(store: &Store, bucket: Option<&Bucket>, guard: CommitGuard, sts: &crate::credentials::StsOptions) -> anyhow::Result<Report> {
    let mut r = Report::default();
    let descriptor = store.get(DESCRIPTOR).await?;
    let pool_exists = descriptor.is_some();
    match descriptor {
        None => {
            r.row("pool", format!("none here yet; a server would create one with the {} commit guard", guard_name(guard)));
            r.row(
                "create-if-absent",
                match guard {
                    CommitGuard::CreateIfAbsent => "not checked, as there is no voidfs.json to create again. A server checks it right after creating the pool, and removes the pool if the bucket ignores it",
                    CommitGuard::External => "not checked, as there is no voidfs.json to create again",
                },
            );
        }
        Some(bytes) => {
            let desc: PoolDescriptor = serde_json::from_slice(&bytes).context("reading voidfs.json")?;
            r.row("pool", format!("{}, commit guard {}", desc.pool_id, guard_name(desc.commit_guard)));
            if let Err(e) = desc.check_readable() {
                r.refusals.push(e);
            }
            let c = create_if_absent(store, DESCRIPTOR, bytes).await;
            r.row("create-if-absent", c.to_string());
            if let Some(why) = guard_mismatch(desc.commit_guard, guard) {
                r.refusals.push(why);
            } else if desc.commit_guard == CommitGuard::CreateIfAbsent
                && let Some(why) = c.refusal()
            {
                r.refusals.push(why);
            }
            r.row("modification times", modification_times(store).await);
        }
    }
    let Some(b) = bucket else {
        r.row("bucket settings", "not applicable: the store is not a bucket");
        return Ok(r);
    };
    let (lifecycle, versioning, cors, lock, presigned) = tokio::join!(b.lifecycle(), b.versioning(), b.read("cors"), b.read("object-lock"), presigned(store, b));
    let mut noncurrent_expire = false;
    match lifecycle {
        Ok(l) => {
            noncurrent_expire = l.noncurrent_expire;
            r.row("lifecycle rules", format!("{} rule(s), {} applying to the pool's objects", l.rules, l.covering));
            for w in l.fatal.iter().chain(&l.warnings) {
                r.row("", w.clone());
            }
            if !l.fatal.is_empty() {
                r.refusals.push(format!("{}; remove the rule, or scope it away from {}", l.fatal.join("; "), b.scope()));
            }
        }
        Err(why) => r.row("lifecycle rules", format!("not checked: {why}")),
    }
    r.row(
        "versioning",
        match versioning {
            Ok(v) => format!("{}{}", format!("{v:?}").to_lowercase(), versioning_warning(v, noncurrent_expire).map(|w| format!(": {w}")).unwrap_or_default()),
            Err(why) => format!("not checked: {why}"),
        },
    );
    r.row(
        "object lock",
        match lock.doc {
            Doc::Found(xml) => describe_object_lock(&xml).unwrap_or_else(|e| format!("could not be parsed ({e:#})")),
            Doc::Absent => "off".into(),
            Doc::Unchecked(why) => format!("not checked: {why}"),
        },
    );
    r.row(
        "CORS",
        match cors.doc {
            Doc::Found(xml) => describe_cors(&xml).unwrap_or_else(|e| format!("could not be parsed ({e:#})")),
            Doc::Absent => "no rules: uploads from a browser will need one".into(),
            Doc::Unchecked(why) => format!("not checked: {why}"),
        },
    );
    r.row("presigned URLs", presigned);
    r.row(
        "presigned PUTs",
        if pool_exists { presigned_puts(b, &b.http).await.to_string() } else { "not checked, as there is no pool here yet; a server checks them when it starts".into() },
    );
    r.row(
        "storage credentials",
        match (b.sts(sts), pool_exists) {
            (Err(why), _) => format!("not offered: {why}"),
            (Ok(_), false) => "not checked, as there is no pool here yet; a server checks them when it starts".into(),
            (Ok(mint), true) => crate::credentials::check(&mint, &b.location(), &b.http).await.to_string(),
        },
    );
    r.row(
        "bucket clock",
        match cors.skew.or(lock.skew) {
            Some(s) if s.abs() <= 2 => "agrees with this machine's to within 2 s".into(),
            Some(s) => format!("{} s {} this machine's{}", s.abs(), if s > 0 { "ahead of" } else { "behind" }, if s.abs() > 300 { ": signed requests fail beyond 15 minutes" } else { "" }),
            None => "unknown: no reply had a Date header".into(),
        },
    );
    r.row(
        "not probed",
        "whether create-if-absent holds when writers race, and whether reads and listings see writes at once: both need new objects. \
         Run the conformance suite, and `cargo test -p voidfs-server -- --ignored` with the VOIDFS_S3_* variables, against this bucket",
    );
    Ok(r)
}

/// Whether a HEAD and a listing give modification times, which garbage collection needs (format
/// §12).
async fn modification_times(store: &Store) -> String {
    let Some(op) = store.operator() else { return "kept by this store".into() };
    let head = match store.modified(DESCRIPTOR).await {
        Ok(Some(_)) => "HEAD gives them".to_owned(),
        Ok(None) => "HEAD found no voidfs.json".to_owned(),
        Err(e) => format!("HEAD DOES NOT, so garbage collection will fail ({e:#})"),
    };
    let listed = match op.list("/").await {
        Ok(entries) => match entries.iter().find(|e| e.path() == DESCRIPTOR) {
            Some(e) if e.metadata().last_modified().is_some() => "listings give them".to_owned(),
            Some(_) => "LISTINGS DO NOT, so garbage collection will never delete anything".to_owned(),
            None => "a listing did not show voidfs.json".to_owned(),
        },
        Err(e) => format!("listing failed: {e}"),
    };
    format!("{head}; {listed}")
}

/// Whether the bucket accepts a presigned request: a HEAD of `voidfs.json`, which reads nothing
/// and is answered whether or not the object exists.
async fn presigned(store: &Store, b: &Bucket) -> String {
    let Some(op) = store.operator() else { return "not applicable".into() };
    let answer = async {
        let p = op.presign_stat(DESCRIPTOR, Duration::from_secs(300)).await?;
        let mut req = http::Request::builder().method(p.method().clone()).uri(p.uri().clone());
        for (k, v) in p.header() {
            req = req.header(k, v);
        }
        anyhow::Ok(b.http.send(req.body(Buffer::new())?).await?.status().as_u16())
    };
    match answer.await {
        Ok(200 | 404) => "accepted: a presigned HEAD of voidfs.json was answered".into(),
        Ok(403) => "REJECTED: a presigned HEAD of voidfs.json was refused (HTTP 403)".into(),
        Ok(s) => format!("unknown: a presigned HEAD of voidfs.json was answered with HTTP {s}"),
        Err(e) => format!("unknown: {e:#}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Presigned PUTs, for direct uploads (protocol §4.11)

impl crate::direct::Presign for Bucket {
    fn put<'a>(&'a self, path: &'a str, headers: &'a [(String, String)], expires: Duration) -> futures::future::BoxFuture<'a, anyhow::Result<crate::direct::Presigned>> {
        Box::pin(self.presign_put(path, headers, expires))
    }
}

/// The bytes of the shard the checks upload. It is a valid shard, stored under its own hash, so a
/// store that ignores what a URL binds stores nothing a pool must not hold (format §2, §4), and
/// garbage collection removes it.
pub const PROBE_SHARD: &[u8] = b"voidfs: a presigned upload, checked\n";

/// What a store did with something a presigned PUT binds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// It refused the PUT that broke it.
    Enforced,
    /// It accepted the PUT that broke it.
    Ignored,
    /// It answers that it does not implement it (`501`).
    Unsupported,
    Unknown(String),
}

/// What the store does with presigned PUTs (protocol §4.11, §9).
#[derive(Clone, Debug)]
pub struct PresignedPuts {
    /// A PUT of a valid shard with its own checksum. `None` when the store accepted it.
    pub refused: Option<String>,
    /// A PUT whose bytes do not match the signed `x-amz-checksum-sha256`.
    pub checksum: Check,
    /// A PUT sent without the signed checksum header.
    pub signed_header: Check,
    /// A PUT with the signed `If-None-Match: *` of an object that exists.
    pub if_none_match: Check,
}

impl PresignedPuts {
    /// Whether a URL keeps a client from storing bytes under a hash they don't match, which
    /// direct uploads need (protocol §9).
    pub fn binds_checksums(&self) -> bool {
        self.refused.is_none() && self.checksum == Check::Enforced && self.signed_header == Check::Enforced
    }
}

impl fmt::Display for PresignedPuts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(why) = &self.refused {
            return write!(f, "REFUSED: a presigned PUT of a valid shard {why}. Direct uploads are not offered");
        }
        let said = |c: &Check, refused: &str| match c {
            Check::Enforced => format!("refused ({refused})"),
            Check::Ignored => "ACCEPTED".to_owned(),
            Check::Unsupported => "not implemented (HTTP 501)".to_owned(),
            Check::Unknown(why) => format!("unknown ({why})"),
        };
        write!(
            f,
            "a wrong x-amz-checksum-sha256 was {}; a PUT without the signed checksum was {}; If-None-Match on an existing object was {}. ",
            said(&self.checksum, "HTTP 400"),
            said(&self.signed_header, "HTTP 400 or 403"),
            said(&self.if_none_match, "HTTP 412")
        )?;
        match (self.binds_checksums(), self.if_none_match == Check::Enforced) {
            (true, true) => write!(f, "Direct uploads are offered, binding both"),
            (true, false) => write!(f, "Direct uploads are offered, binding checksums only"),
            (false, _) => write!(f, "Direct uploads are NOT offered: the store must refuse bytes that don't match a URL's checksum"),
        }
    }
}

/// Finds out what a store enforces on the PUTs it presigns, with four PUTs of
/// [`PROBE_SHARD`] at its own path: one that must be accepted, then one with another checksum,
/// one without the checksum header, and one with `If-None-Match: *`, which must be refused.
pub async fn presigned_puts(presign: &dyn crate::direct::Presign, http: &HttpClient) -> PresignedPuts {
    let h = voidfs_core::ids::ShardHash::of(PROBE_SHARD);
    let path = format!("shards/{}", h.object_path());
    let sum = ("x-amz-checksum-sha256".to_owned(), crate::direct::checksum(&h));
    let wrong = ("x-amz-checksum-sha256".to_owned(), crate::direct::checksum(&voidfs_core::ids::ShardHash::of(b"other bytes")));
    let inm = ("if-none-match".to_owned(), "*".to_owned());
    let ttl = Duration::from_secs(300);
    // Presigns with `signed` and sends with `sent`: the status and the error code.
    let put = async |signed: &[(String, String)], sent: &[(String, String)]| -> Result<(u16, String), String> {
        let p = presign.put(&path, signed, ttl).await.map_err(|e| format!("{e:#}"))?;
        let mut req = http::Request::put(&p.url);
        for (k, v) in &p.headers {
            if sent.iter().any(|(s, _)| s == k) || !signed.iter().any(|(s, _)| s == k) {
                req = req.header(k, v);
            }
        }
        let resp = http.send(req.body(Buffer::from(Bytes::from_static(PROBE_SHARD))).map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let code = error_code(&String::from_utf8_lossy(&resp.into_body().to_bytes())).unwrap_or_default();
        Ok((status, code))
    };
    let judge = |r: Result<(u16, String), String>, refusals: &[u16]| match r {
        Ok((s, _)) if refusals.contains(&s) => Check::Enforced,
        Ok((200..=299, _)) => Check::Ignored,
        Ok((501, _)) => Check::Unsupported,
        Ok((s, code)) => Check::Unknown(format!("HTTP {s}{}", if code.is_empty() { String::new() } else { format!(" {code}") })),
        Err(e) => Check::Unknown(e),
    };
    let refused = match put(std::slice::from_ref(&sum), std::slice::from_ref(&sum)).await {
        Ok((200..=299, _)) => None,
        Ok((s, code)) => Some(format!("was answered with HTTP {s}{}", if code.is_empty() { String::new() } else { format!(" {code}") })),
        Err(e) => Some(format!("failed: {e}")),
    };
    if refused.is_some() {
        return PresignedPuts { refused, checksum: Check::Unknown("not tried".into()), signed_header: Check::Unknown("not tried".into()), if_none_match: Check::Unknown("not tried".into()) };
    }
    let checksum = judge(put(std::slice::from_ref(&wrong), std::slice::from_ref(&wrong)).await, &[400]);
    // AWS, R2 and versitygw answer 403; MinIO 400 AccessDenied.
    let signed_header = judge(put(std::slice::from_ref(&sum), &[]).await, &[400, 403]);
    let both = [sum.clone(), inm];
    let if_none_match = judge(put(&both, &both).await, &[412]);
    PresignedPuts { refused, checksum, signed_header, if_none_match }
}

/// Direct uploads through `presign`, if the store behind it binds a URL's checksum: what a server
/// offers, after the checks it makes when it starts.
pub async fn offer(presign: Box<dyn crate::direct::Presign>, http: &HttpClient) -> (PresignedPuts, Option<std::sync::Arc<crate::direct::Direct>>) {
    let checks = presigned_puts(presign.as_ref(), http).await;
    let direct = checks.binds_checksums().then(|| std::sync::Arc::new(crate::direct::Direct::new(presign, checks.if_none_match == Check::Enforced)));
    (checks, direct)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::FutureExt;

    use super::*;
    use crate::clock::Clock;
    use crate::store::{Fault, MemOp, MemStore};

    fn mem() -> (Arc<MemStore>, Store) {
        let m = Arc::new(MemStore::new(Clock::System));
        (m.clone(), Store::mem(m))
    }

    fn fault_on_put_new(m: &MemStore, fault: Fault) {
        m.set_hook(Some(Arc::new(move |op, _| futures::future::ready(if op == MemOp::PutNew { fault } else { Fault::None }).boxed())));
    }

    #[tokio::test]
    async fn create_if_absent_outcomes() {
        let (m, store) = mem();
        let bytes = Bytes::from_static(b"{\"format\":1}");
        store.put(DESCRIPTOR, bytes.clone()).await.unwrap();
        assert_eq!(create_if_absent(&store, DESCRIPTOR, bytes.clone()).await, Conditional::Honoured);
        fault_on_put_new(&m, Fault::Unconditional);
        assert_eq!(create_if_absent(&store, DESCRIPTOR, bytes.clone()).await, Conditional::Ignored);
        assert_eq!(m.peek(DESCRIPTOR).unwrap(), bytes, "an ignored condition rewrites the same bytes");
        fault_on_put_new(&m, Fault::Fail);
        assert!(matches!(create_if_absent(&store, DESCRIPTOR, bytes).await, Conditional::Failed(_)));
        assert!(Conditional::Honoured.refusal().is_none());
        assert!(Conditional::Ignored.refusal().unwrap().contains("ignores create-if-absent"));
        assert!(Conditional::Failed("x".into()).refusal().unwrap().contains("could not confirm"));
    }

    #[test]
    fn a_pool_keeps_its_guard() {
        use CommitGuard::*;
        assert!(guard_mismatch(CreateIfAbsent, CreateIfAbsent).is_none());
        assert!(guard_mismatch(External, External).is_none());
        assert!(guard_mismatch(External, CreateIfAbsent).unwrap().contains("--commit-guard external"));
        assert!(guard_mismatch(CreateIfAbsent, External).unwrap().contains("without --commit-guard external"));
    }

    const LIFECYCLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LifecycleConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Rule><ID>tmp</ID><Filter><Prefix>tmp/</Prefix></Filter><Status>Enabled</Status><Expiration><Days>1</Days></Expiration></Rule>
  <Rule><ID>everything</ID><Filter></Filter><Status>Disabled</Status><Expiration><Days>30</Days></Expiration></Rule>
  <Rule><ID>tagged</ID><Filter><And><Prefix>pool/</Prefix><Tag><Key>k</Key><Value>v</Value></Tag></And></Filter><Status>Enabled</Status><Expiration><Days>3</Days></Expiration></Rule>
  <Rule><ID>cold</ID><Filter><And><Prefix>pool/shards/</Prefix><ObjectSizeGreaterThan>1048576</ObjectSizeGreaterThan></And></Filter><Status>Enabled</Status>
    <Transition><Days>30</Days><StorageClass>STANDARD_IA</StorageClass></Transition>
    <Transition><Days>90</Days><StorageClass>DEEP_ARCHIVE</StorageClass></Transition></Rule>
  <Rule><ID>old</ID><Prefix>pool</Prefix><Status>Enabled</Status><NoncurrentVersionExpiration><NoncurrentDays>7</NoncurrentDays></NoncurrentVersionExpiration>
    <Expiration><ExpiredObjectDeleteMarker>true</ExpiredObjectDeleteMarker></Expiration></Rule>
  <Rule><ID>uploads</ID><Status>Enabled</Status><AbortIncompleteMultipartUpload><DaysAfterInitiation>7</DaysAfterInitiation></AbortIncompleteMultipartUpload></Rule>
</LifecycleConfiguration>"#;

    #[test]
    fn lifecycle_rules_are_parsed() {
        let rules = parse_lifecycle(LIFECYCLE).unwrap();
        assert_eq!(rules.len(), 6);
        assert_eq!(rules[0], Rule { id: "tmp".into(), enabled: true, prefix: "tmp/".into(), expires: Some("after 1 day".into()), ..Rule::default() });
        assert!(!rules[1].enabled && rules[1].prefix.is_empty());
        assert!(rules[2].tagged);
        assert_eq!(rules[3].size.as_deref(), Some("(only objects larger than 1048576 bytes)"));
        assert_eq!(rules[3].transitions, [("STANDARD_IA".into(), "after 30 days".into()), ("DEEP_ARCHIVE".into(), "after 90 days".into())]);
        assert_eq!((rules[4].prefix.as_str(), rules[4].noncurrent_expires, &rules[4].expires), ("pool", true, &None), "a delete-marker rule expires nothing");
        assert!(rules[5].prefix.is_empty() && rules[5].expires.is_none() && rules[5].transitions.is_empty());
    }

    #[test]
    fn lifecycle_rules_are_judged_by_what_they_do_to_the_pool() {
        let rules = parse_lifecycle(LIFECYCLE).unwrap();
        let l = assess(&rules, "pool/");
        assert_eq!((l.rules, l.covering), (6, 3), "cold, old and uploads cover pool/; tmp is elsewhere, everything is disabled, tagged needs tags");
        assert_eq!(l.fatal.len(), 1);
        assert!(l.fatal[0].contains("\"cold\" moves objects under \"pool/shards/\" (only objects larger than 1048576 bytes) to DEEP_ARCHIVE after 90 days"), "{:?}", l.fatal);
        assert_eq!(l.warnings.len(), 1);
        assert!(l.warnings[0].contains("STANDARD_IA"));
        assert!(l.noncurrent_expire);
        // A pool at the root of the bucket is covered by every rule with a prefix.
        let l = assess(&rules, "");
        assert_eq!(l.covering, 4);
        assert!(l.fatal.iter().any(|f| f.contains("\"tmp\" deletes objects under \"tmp/\" after 1 day")));
        // A pool elsewhere is covered only by the rule without one.
        let l = assess(&rules, "other/");
        assert_eq!((l.covering, l.fatal.len(), l.noncurrent_expire), (1, 0, false));
        // An expiry on a date, over the whole bucket.
        let dated = r#"<LifecycleConfiguration><Rule><Status>Enabled</Status><Expiration><Date>2027-01-01T00:00:00Z</Date></Expiration></Rule></LifecycleConfiguration>"#;
        let l = assess(&parse_lifecycle(dated).unwrap(), "pool/");
        assert_eq!(l.fatal, ["a lifecycle rule deletes every object in the bucket on 2027-01-01T00:00:00Z"]);
    }

    #[test]
    fn bucket_settings_are_parsed() {
        let v = |s: &str| parse_versioning(s).unwrap();
        assert_eq!(v(r#"<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>Enabled</Status></VersioningConfiguration>"#), Versioning::Enabled);
        assert_eq!(v("<VersioningConfiguration><Status>Suspended</Status><MfaDelete>Disabled</MfaDelete></VersioningConfiguration>"), Versioning::Suspended);
        assert_eq!(v(r#"<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"/>"#), Versioning::Off);
        assert_eq!(v(""), Versioning::Off);
        assert!(versioning_warning(Versioning::Enabled, false).is_some());
        assert!(versioning_warning(Versioning::Enabled, true).is_none());
        assert!(versioning_warning(Versioning::Off, false).is_none());

        let cors = r#"<CORSConfiguration><CORSRule><AllowedOrigin>https://app.example</AllowedOrigin><AllowedMethod>GET</AllowedMethod><AllowedMethod>PUT</AllowedMethod></CORSRule>
            <CORSRule><AllowedOrigin>*</AllowedOrigin><AllowedMethod>GET</AllowedMethod></CORSRule></CORSConfiguration>"#;
        assert_eq!(describe_cors(cors).unwrap(), "2 rule(s); PUT allowed from https://app.example");
        assert!(describe_cors("<CORSConfiguration><CORSRule><AllowedOrigin>*</AllowedOrigin><AllowedMethod>GET</AllowedMethod></CORSRule></CORSConfiguration>").unwrap().contains("none allowing PUT"));

        let lock = "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled><Rule><DefaultRetention><Mode>GOVERNANCE</Mode><Days>30</Days></DefaultRetention></Rule></ObjectLockConfiguration>";
        assert!(describe_object_lock(lock).unwrap().starts_with("on, keeping new objects GOVERNANCE for 30 days"));
        assert_eq!(describe_object_lock("<ObjectLockConfiguration/>").unwrap(), "off");

        assert_eq!(error_code("<Error><Code>NoSuchLifecycleConfiguration</Code><Message>m</Message></Error>").as_deref(), Some("NoSuchLifecycleConfiguration"));
        assert_eq!(error_code("not xml"), None);
    }

    #[test]
    fn bucket_urls_and_providers() {
        let b = Bucket::new("b", "/pool/", Some("https://acct.r2.cloudflarestorage.com/"), "auto", Some(("id", "secret"))).unwrap();
        assert_eq!((b.url.as_str(), b.prefix.as_str(), b.provider), ("https://acct.r2.cloudflarestorage.com/b", "pool/", Provider::R2));
        let b = Bucket::new("b", "", None, "eu-west-1", None).unwrap();
        assert_eq!((b.url.as_str(), b.prefix.as_str(), b.provider), ("https://s3.eu-west-1.amazonaws.com/b", "", Provider::Aws));
        let b = Bucket::new("b", "a/b", Some("127.0.0.1:7070"), "us-east-1", Some(("id", "secret"))).unwrap();
        assert_eq!((b.url.as_str(), b.prefix.as_str(), b.provider), ("https://127.0.0.1:7070/b", "a/b/", Provider::Other));
    }

    #[test]
    fn storage_credentials_are_minted_where_the_service_can() {
        use crate::credentials::{Location, StsOptions};
        let opts = |role: Option<&str>, endpoint: Option<&str>| StsOptions { role: role.map(str::to_owned), endpoint: endpoint.map(str::to_owned) };
        let minio = Bucket::new("b", "pool", Some("http://127.0.0.1:9000/"), "us-east-1", Some(("id", "secret"))).unwrap();
        assert_eq!(minio.location(), Location { bucket: "b".into(), root: "pool/".into(), region: "us-east-1".into(), endpoint: "http://127.0.0.1:9000".into() });
        let sts = minio.sts(&opts(None, None)).unwrap();
        assert_eq!((sts.endpoint.as_str(), sts.role.as_deref(), sts.bucket.as_str(), sts.root.as_str()), ("http://127.0.0.1:9000/", None, "b", "pool/"), "MinIO's STS is on its own endpoint");
        let aws = Bucket::new("b", "", None, "eu-west-1", None).unwrap();
        assert!(aws.sts(&opts(None, None)).err().unwrap().contains("--storage-credentials-role"));
        let sts = aws.sts(&opts(Some("arn:aws:iam::1:role/r"), None)).unwrap();
        assert_eq!((sts.endpoint.as_str(), sts.role.as_deref()), ("https://sts.eu-west-1.amazonaws.com/", Some("arn:aws:iam::1:role/r")));
        assert_eq!(aws.location().endpoint, "https://s3.eu-west-1.amazonaws.com");
        assert_eq!(aws.sts(&opts(Some("arn:aws:iam::1:role/r"), Some("https://sts.example/"))).unwrap().endpoint, "https://sts.example/");
        let r2 = Bucket::new("b", "pool", Some("https://acct.r2.cloudflarestorage.com"), "auto", Some(("id", "secret"))).unwrap();
        assert!(r2.sts(&opts(None, None)).err().unwrap().contains("Cloudflare"));
    }

    #[tokio::test]
    async fn the_report_says_whether_a_server_would_start() {
        let (m, store) = mem();
        let r = report(&store, None, CommitGuard::CreateIfAbsent, &Default::default()).await.unwrap();
        assert!(r.refusals.is_empty());
        assert!(r.to_string().contains("none here yet"), "{r}");
        let desc = serde_json::json!({ "format": 1, "pool_id": "p-1", "created": "2026-09-28T00:00:00Z", "features": { "compatible": [], "incompatible": [] },
            "chunking": { "algorithm": "fastcdc-2020", "min": 262144, "avg": 2097152, "max": 16777216 }, "hash": "sha256", "commit_guard": "create-if-absent" });
        store.put(DESCRIPTOR, Bytes::from(desc.to_string())).await.unwrap();
        let r = report(&store, None, CommitGuard::CreateIfAbsent, &Default::default()).await.unwrap();
        assert!(r.refusals.is_empty(), "{r}");
        assert!(r.to_string().contains("honoured"));
        assert!(r.to_string().ends_with("would open the pool."));
        let r = report(&store, None, CommitGuard::External, &Default::default()).await.unwrap();
        assert_eq!(r.refusals.len(), 1, "{r}");
        fault_on_put_new(&m, Fault::Unconditional);
        let r = report(&store, None, CommitGuard::CreateIfAbsent, &Default::default()).await.unwrap();
        assert!(r.to_string().contains("IGNORED"));
        assert!(r.refusals[0].contains("ignores create-if-absent"), "{r}");
    }
}
