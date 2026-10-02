// SPDX-License-Identifier: Apache-2.0
//! The client: the AWS SDK for S3 for the standard calls, and signed requests for the extensions.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use base64::Engine;
use bytes::Bytes;
use http::{HeaderMap, Method};
use serde::de::DeserializeOwned;

use crate::error::{from_s3, service_error};
use crate::feed::ChangeWatch;
use crate::retry::{self, Replay};
use crate::sign::{self, Signer, drive_path, object_path};
use crate::types::*;
use crate::{Error, Result};

/// Where a client connects when nothing says otherwise: `voidfs-server`'s default address.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:9000";
/// The signing region; servers accept any (protocol §2).
pub const DEFAULT_REGION: &str = "us-east-1";

#[derive(Clone)]
pub struct Config {
    /// `http(s)://host[:port]`.
    pub endpoint: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    /// How long a request may wait for the server without hearing from it. A long poll waits
    /// this beyond its own wait.
    pub timeout: Duration,
    pub connect_timeout: Duration,
    /// Attempts per extension call, the first included (the retry rule is in [`crate::Error`]'s
    /// docs). The AWS client keeps its own.
    pub max_attempts: u32,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            endpoint: DEFAULT_ENDPOINT.into(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            region: DEFAULT_REGION.into(),
            timeout: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(10),
            max_attempts: 3,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("endpoint", &self.endpoint)
            .field("access_key_id", &self.access_key_id)
            .field("region", &self.region)
            .field("timeout", &self.timeout)
            .field("connect_timeout", &self.connect_timeout)
            .field("max_attempts", &self.max_attempts)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// From the environment: `VOIDFS_ENDPOINT` (default [`DEFAULT_ENDPOINT`]),
    /// `VOIDFS_ACCESS_KEY_ID`, `VOIDFS_SECRET_ACCESS_KEY` and `VOIDFS_REGION`. The key's names are
    /// the ones `voidfs-server` takes its admin key from.
    pub fn from_env() -> Result<Config> {
        Config::from_lookup(|n| std::env::var(n).ok())
    }

    pub(crate) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config> {
        let var = |n: &str| get(n).filter(|v| !v.is_empty());
        let need = |n: &str| var(n).ok_or_else(|| Error::Invalid(format!("{n} is not set")));
        let mut c = Config { access_key_id: need("VOIDFS_ACCESS_KEY_ID")?, secret_access_key: need("VOIDFS_SECRET_ACCESS_KEY")?, ..Config::default() };
        if let Some(e) = var("VOIDFS_ENDPOINT") {
            c.endpoint = e;
        }
        if let Some(r) = var("VOIDFS_REGION") {
            c.region = r;
        }
        Ok(c)
    }
}

/// A voidfs client. Cloning it is cheap and shares its connections.
#[derive(Clone)]
pub struct Client(Arc<Inner>);

struct Inner {
    config: Config,
    signer: Signer,
    /// For requests answered at once.
    http: reqwest::Client,
    /// For long polls and event streams, which set their own limits.
    http_wait: reqwest::Client,
    s3: aws_sdk_s3::Client,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").field("config", &self.0.config).finish_non_exhaustive()
    }
}

/// How long a request may take.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Patience {
    /// Answered at once: it fails once the server is silent for the configured timeout.
    Prompt,
    /// A long poll: the server may hold it this long, and the timeout beyond.
    Wait(Duration),
    /// An event stream, open until it breaks; its reader watches for silence.
    Stream,
}

/// A request to send, and how it may be retried.
pub(crate) struct Req {
    method: Method,
    path: String,
    query: Vec<(&'static str, String)>,
    headers: Vec<(String, String)>,
    body: Bytes,
    replay: Replay,
}

impl Req {
    pub(crate) fn new(method: Method, path: String) -> Req {
        Req { method, path, query: Vec::new(), headers: Vec::new(), body: Bytes::new(), replay: Replay::Safe }
    }

    pub(crate) fn query(mut self, name: &'static str, value: impl Into<String>) -> Req {
        self.query.push((name, value.into()));
        self
    }

    fn query_opt(self, name: &'static str, value: Option<impl Into<String>>) -> Req {
        match value {
            Some(v) => self.query(name, v),
            None => self,
        }
    }

    pub(crate) fn header(mut self, name: &str, value: impl Into<String>) -> Req {
        self.headers.push((name.to_ascii_lowercase(), value.into()));
        self
    }

    fn header_opt(self, name: &str, value: Option<impl Into<String>>) -> Req {
        match value {
            Some(v) => self.header(name, v),
            None => self,
        }
    }

    fn body(mut self, body: impl Into<Bytes>) -> Req {
        self.body = body.into();
        self
    }

    fn guard(self, if_version: Option<&String>, if_match: Option<&String>) -> Req {
        self.header_opt("x-voidfs-if-version", if_version.cloned()).header_opt("if-match", if_match.cloned())
    }

    fn attrs(self, mtime: Option<&String>, mode: Option<u32>) -> Req {
        self.header_opt("x-voidfs-mtime", mtime.cloned()).header_opt("x-voidfs-mode", mode.map(|m| format!("{m:04o}")))
    }

    /// Sent again only when it cannot have reached the server, unless a precondition guards it:
    /// then a second attempt after the first landed fails with `412` instead of applying twice.
    fn not_idempotent(mut self) -> Req {
        let guarded = self.headers.iter().any(|(k, _)| k == "x-voidfs-if-version" || k == "if-match" || k == "if-none-match");
        if !guarded {
            self.replay = Replay::OnlyIfUnsent;
        }
        self
    }

    fn query_string(&self) -> String {
        sign::query_string(self.query.iter().map(|(k, v)| (*k, v.as_str())))
    }
}

/// A successful answer, read whole.
pub(crate) struct Reply {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Reply {
    fn header(&self, name: &str) -> Option<String> {
        header(&self.headers, name)
    }

    fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| Error::decode(format!("{e}: {}", String::from_utf8_lossy(&self.body[..self.body.len().min(200)]))))
    }

    fn write_result(&self) -> Result<WriteResult> {
        Ok(WriteResult {
            version_id: self.header("x-amz-version-id").ok_or_else(|| Error::decode("no x-amz-version-id on a mutation"))?,
            etag: self.header("etag"),
            size: self.header("x-voidfs-size").and_then(|s| s.parse().ok()),
        })
    }
}

fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

fn require(what: &str, value: &str) -> Result<()> {
    if value.is_empty() { Err(Error::Invalid(format!("{what} is empty"))) } else { Ok(()) }
}

/// The object headers of a GET or HEAD (protocol §4.0).
fn object_meta(status: u16, h: &HeaderMap) -> Result<ObjectMeta> {
    let size = match (status, header(h, "content-range")) {
        (206, Some(r)) => r.rsplit_once('/').and_then(|(_, total)| total.parse().ok()).ok_or_else(|| Error::decode(format!("content-range {r:?}")))?,
        _ => header(h, "content-length").and_then(|s| s.parse().ok()).unwrap_or(0),
    };
    let metadata = h
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().strip_prefix("x-amz-meta-")?.to_owned(), v.to_str().ok()?.to_owned())))
        .collect();
    Ok(ObjectMeta {
        version_id: header(h, "x-amz-version-id").ok_or_else(|| Error::decode("no x-amz-version-id on a read"))?,
        etag: header(h, "etag").unwrap_or_default(),
        size,
        kind: header(h, "x-voidfs-kind").map_or(Kind::Unknown, |k| Kind::parse(&k)),
        object_id: header(h, "x-voidfs-object-id"),
        content_type: header(h, "content-type"),
        last_modified: header(h, "last-modified"),
        mtime: header(h, "x-voidfs-mtime"),
        mode: header(h, "x-voidfs-mode"),
        metadata,
    })
}

impl Client {
    pub fn new(config: Config) -> Result<Client> {
        if config.access_key_id.is_empty() || config.secret_access_key.is_empty() {
            return Err(Error::Invalid("an access key id and secret are required".into()));
        }
        let signer = Signer::new(&config.endpoint, &config.access_key_id, &config.secret_access_key)?.with_region(&config.region);
        let build = |read_timeout: Option<Duration>| {
            let mut b = reqwest::Client::builder().connect_timeout(config.connect_timeout);
            if let Some(t) = read_timeout {
                b = b.read_timeout(t);
            }
            b.build().map_err(|e| Error::Invalid(format!("HTTP client: {e}")))
        };
        let http = build(Some(config.timeout))?;
        let http_wait = build(None)?;
        let credentials = Credentials::new(&config.access_key_id, &config.secret_access_key, None, None, "voidfs-sdk");
        let s3 = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new(config.region.clone()))
                .credentials_provider(credentials)
                .endpoint_url(signer.endpoint())
                .force_path_style(true)
                .build(),
        );
        Ok(Client(Arc::new(Inner { config, signer, http, http_wait, s3 })))
    }

    /// A client configured by [`Config::from_env`].
    pub fn from_env() -> Result<Client> {
        Client::new(Config::from_env()?)
    }

    /// The AWS SDK client underneath (path-style, the same key), for any standard S3 call:
    /// multipart uploads, copies, tagging.
    pub fn s3(&self) -> &aws_sdk_s3::Client {
        &self.0.s3
    }

    pub fn config(&self) -> &Config {
        &self.0.config
    }

    // -----------------------------------------------------------------------------------------
    // Sending

    /// One attempt: the response once its status is a success, or the error it carries.
    async fn attempt(&self, req: &Req, patience: Patience) -> Result<reqwest::Response> {
        let signed = self.0.signer.sign(req.method.as_str(), &req.path, &req.query_string(), req.headers.clone(), &[], req.body.clone())?;
        let mut request = reqwest::Request::try_from(signed).map_err(|e| Error::Invalid(e.to_string()))?;
        let http = match patience {
            Patience::Prompt => &self.0.http,
            Patience::Wait(w) => {
                *request.timeout_mut() = w.checked_add(self.0.config.timeout);
                &self.0.http_wait
            }
            Patience::Stream => &self.0.http_wait,
        };
        let resp = http.execute(request).await?;
        let status = resp.status().as_u16();
        if status >= 300 {
            let headers = resp.headers().clone();
            let body = resp.bytes().await.unwrap_or_default();
            return Err(Error::Service(service_error(status, &headers, &body)));
        }
        Ok(resp)
    }

    /// Sends `req`, retrying as its [`Replay`] allows, and reads the whole answer.
    pub(crate) async fn execute(&self, req: Req) -> Result<Reply> {
        self.execute_with(req, Patience::Prompt).await
    }

    async fn execute_with(&self, req: Req, patience: Patience) -> Result<Reply> {
        let mut attempt = 1;
        loop {
            let r = match self.attempt(&req, patience).await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let headers = resp.headers().clone();
                    resp.bytes().await.map(|body| Reply { status, headers, body }).map_err(Error::from)
                }
                Err(e) => Err(e),
            };
            match r {
                Err(e) if attempt < self.0.config.max_attempts.max(1) && retry::may_retry(&e, req.replay) => {
                    tokio::time::sleep(retry::delay(attempt, &e)).await;
                    attempt += 1;
                }
                r => return r,
            }
        }
    }

    /// Sends `req`, retrying as its [`Replay`] allows, until the answer's headers arrive.
    pub(crate) async fn execute_streaming(&self, req: &Req, patience: Patience) -> Result<reqwest::Response> {
        let mut attempt = 1;
        loop {
            match self.attempt(req, patience).await {
                Err(e) if attempt < self.0.config.max_attempts.max(1) && retry::may_retry(&e, req.replay) => {
                    tokio::time::sleep(retry::delay(attempt, &e)).await;
                    attempt += 1;
                }
                r => return r,
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Drives (§5)

    /// Every drive the key reaches (`ListBuckets`).
    pub async fn list_drives(&self) -> Result<Vec<DriveSummary>> {
        let out = self.0.s3.list_buckets().send().await.map_err(from_s3)?;
        Ok(out
            .buckets()
            .iter()
            .map(|b| DriveSummary { name: b.name().unwrap_or_default().to_owned(), created: b.creation_date().and_then(|d| d.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime).ok()) })
            .collect())
    }

    /// Creates a drive named `name` (§5.1).
    pub async fn create_drive(&self, name: &str, opts: CreateDrive) -> Result<CreateDriveResult> {
        require("drive name", name)?;
        let req = Req::new(Method::PUT, drive_path(name)).header_opt("x-voidfs-display-name", opts.display_name.as_deref().map(sign::header_text));
        let r = self.execute(req).await?;
        Ok(CreateDriveResult { drive_id: r.header("x-voidfs-drive-id").ok_or_else(|| Error::decode("no x-voidfs-drive-id"))? })
    }

    /// Creates `name` as a copy-on-write fork of `source`'s current state (§5.2).
    pub async fn fork_drive(&self, source: &str, name: &str) -> Result<ForkResult> {
        require("source drive", source)?;
        require("drive name", name)?;
        let r = self.execute(Req::new(Method::PUT, drive_path(name)).header("x-voidfs-fork-source", source)).await?;
        Ok(ForkResult {
            drive_id: r.header("x-voidfs-drive-id").ok_or_else(|| Error::decode("no x-voidfs-drive-id"))?,
            source_id: r.header("x-voidfs-fork-source-id"),
            fork_point: r.header("x-voidfs-fork-point"),
        })
    }

    /// Soft-deletes a drive, recoverable for the retention window, or with `hard` deletes it for
    /// good (§5.3).
    pub async fn delete_drive(&self, name: &str, hard: bool) -> Result<()> {
        require("drive name", name)?;
        let req = Req::new(Method::DELETE, drive_path(name)).header_opt("x-voidfs-hard-delete", hard.then_some("true"));
        self.execute(req).await.map(drop)
    }

    /// Recovers a soft-deleted drive (§5.3).
    pub async fn undelete_drive(&self, name: &str) -> Result<CreateDriveResult> {
        require("drive name", name)?;
        let r = self.execute(Req::new(Method::POST, drive_path(name)).query("x-voidfs-undelete", "")).await?;
        Ok(CreateDriveResult { drive_id: r.header("x-voidfs-drive-id").ok_or_else(|| Error::decode("no x-voidfs-drive-id"))? })
    }

    /// The drive's id, lineage, position and size (§5.4).
    pub async fn describe_drive(&self, name: &str) -> Result<DriveInfo> {
        require("drive name", name)?;
        self.execute(Req::new(Method::GET, drive_path(name)).query("x-voidfs-drive", "")).await?.json()
    }

    /// Exchanges the key for short-lived read-only credentials to the drive's storage (§5.5).
    /// `501 NotImplemented` where the deployment can't issue them: read through the API then.
    pub async fn storage_credentials(&self, name: &str) -> Result<StorageCredentials> {
        require("drive name", name)?;
        self.execute(Req::new(Method::GET, drive_path(name)).query("x-voidfs-credentials", "")).await?.json()
    }

    // -----------------------------------------------------------------------------------------
    // Objects (§3, §4)

    /// Writes a whole object. A key ending in `/` with an empty body is a folder.
    pub async fn put_object(&self, drive: &str, key: &str, body: impl Into<Bytes>, opts: PutOptions) -> Result<WriteResult> {
        require("key", key)?;
        let mut req = Req::new(Method::PUT, object_path(drive, key))
            .body(body)
            .header_opt("content-type", opts.content_type)
            .guard(opts.if_version.as_ref(), opts.if_match.as_ref())
            .header_opt("if-none-match", opts.if_none_match_any.then_some("*"))
            .attrs(opts.mtime.as_ref(), opts.mode);
        for (k, v) in &opts.metadata {
            req = req.header(&format!("x-amz-meta-{k}"), v.clone());
        }
        self.execute(req).await?.write_result()
    }

    fn read_req(&self, method: Method, drive: &str, key: &str, opts: &ReadOptions) -> Result<Req> {
        require("key", key)?;
        if opts.version_id.is_some() && opts.as_of.is_some() {
            return Err(Error::Invalid("give a version id or as_of, not both".into()));
        }
        Ok(Req::new(method, object_path(drive, key)).query_opt("versionId", opts.version_id.clone()).header_opt("x-voidfs-as-of", opts.as_of.clone()))
    }

    /// Reads a whole object.
    pub async fn get_object(&self, drive: &str, key: &str, opts: ReadOptions) -> Result<Object> {
        let r = self.execute(self.read_req(Method::GET, drive, key, &opts)?).await?;
        Ok(Object { meta: object_meta(r.status, &r.headers)?, body: r.body })
    }

    /// Reads an object as it arrives, for objects too large to hold.
    pub async fn get_object_stream(&self, drive: &str, key: &str, opts: ReadOptions) -> Result<ObjectStream> {
        let req = self.read_req(Method::GET, drive, key, &opts)?;
        let resp = self.execute_streaming(&req, Patience::Prompt).await?;
        Ok(ObjectStream { meta: object_meta(resp.status().as_u16(), resp.headers())?, resp })
    }

    /// An object's headers.
    pub async fn head_object(&self, drive: &str, key: &str, opts: ReadOptions) -> Result<ObjectMeta> {
        let r = self.execute(self.read_req(Method::HEAD, drive, key, &opts)?).await?;
        object_meta(r.status, &r.headers)
    }

    /// Reads `length` bytes from `offset`, or to the end with `None`; fewer where the object ends.
    pub async fn read_range(&self, drive: &str, key: &str, offset: u64, length: Option<u64>, opts: ReadOptions) -> Result<Bytes> {
        if length == Some(0) {
            return Ok(Bytes::new());
        }
        Ok(self.get_range(drive, key, offset, length, opts).await?.body)
    }

    /// [`Client::read_range`] with the headers of the version read: its id and ETag, and in
    /// `size` the whole object's size. A cache checks that the bytes are of the version it asked
    /// for this way.
    pub async fn get_range(&self, drive: &str, key: &str, offset: u64, length: Option<u64>, opts: ReadOptions) -> Result<Object> {
        let range = match length {
            Some(0) => return Err(Error::Invalid("an empty range".into())),
            Some(n) => format!("bytes={offset}-{}", offset.saturating_add(n - 1)),
            None => format!("bytes={offset}-"),
        };
        let r = self.execute(self.read_req(Method::GET, drive, key, &opts)?.header("range", range)).await?;
        Ok(Object { meta: object_meta(r.status, &r.headers)?, body: r.body })
    }

    /// Deletes an object; its history stays. The version the delete made, if there was an object.
    pub async fn delete_object(&self, drive: &str, key: &str, pre: Preconditions) -> Result<Option<String>> {
        require("key", key)?;
        let r = self.execute(Req::new(Method::DELETE, object_path(drive, key)).guard(pre.if_version.as_ref(), pre.if_match.as_ref())).await?;
        Ok(r.header("x-amz-version-id"))
    }

    /// Every key and common prefix under `opts.prefix`, every page (`ListObjectsV2`).
    pub async fn list_objects(&self, drive: &str, opts: ListOptions) -> Result<ListResult> {
        let mut pages = self.0.s3.list_objects_v2().bucket(drive).set_prefix(opts.prefix).set_delimiter(opts.delimiter).into_paginator().send();
        let mut out = ListResult::default();
        while let Some(page) = pages.next().await {
            let page = page.map_err(from_s3)?;
            for o in page.contents() {
                out.objects.push(ObjectSummary {
                    key: o.key().unwrap_or_default().to_owned(),
                    size: o.size().and_then(|s| u64::try_from(s).ok()).unwrap_or(0),
                    etag: o.e_tag().map(str::to_owned),
                    last_modified: o.last_modified().and_then(|d| d.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime).ok()),
                });
            }
            out.common_prefixes.extend(page.common_prefixes().iter().filter_map(|p| p.prefix().map(str::to_owned)));
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------------------------
    // Edits (§4.1–§4.3)

    /// `pwrite`: writes `data` at `offset` as one version, creating the object with zeros before
    /// `offset` if it is absent and unguarded (§4.1).
    pub async fn write_at(&self, drive: &str, key: &str, offset: u64, data: impl Into<Bytes>, opts: WriteOptions) -> Result<WriteResult> {
        require("key", key)?;
        let req = Req::new(Method::PUT, object_path(drive, key))
            .query("x-voidfs-write", "")
            .header("x-voidfs-offset", offset.to_string())
            .header_opt("x-voidfs-size", opts.size.map(|s| s.to_string()))
            .guard(opts.if_version.as_ref(), opts.if_match.as_ref())
            .attrs(opts.mtime.as_ref(), opts.mode)
            .body(data);
        self.execute(req).await?.write_result()
    }

    /// Sets the object's size, truncating it or extending it with zeros (§4.1).
    pub async fn truncate(&self, drive: &str, key: &str, size: u64, pre: Preconditions) -> Result<WriteResult> {
        self.write_at(drive, key, 0, Bytes::new(), WriteOptions { size: Some(size), if_version: pre.if_version, if_match: pre.if_match, ..Default::default() }).await
    }

    /// Applies `edits` in order as one version; later edits win where they overlap (§4.2).
    pub async fn patch(&self, drive: &str, key: &str, edits: &[Edit], opts: WriteOptions) -> Result<WriteResult> {
        if edits.is_empty() || edits.len() > voidfs_core::patch::MAX_EDITS as usize {
            return Err(Error::Invalid(format!("a patch has 1 to {} edits, not {}", voidfs_core::patch::MAX_EDITS, edits.len())));
        }
        let borrowed: Vec<_> = edits.iter().map(|e| voidfs_core::patch::Edit { offset: e.offset, data: &e.data }).collect();
        self.raw_patch(drive, key, voidfs_core::patch::encode(&borrowed), opts).await
    }

    /// Sends an encoded `application/vnd.voidfs.patch` body as it is.
    pub async fn raw_patch(&self, drive: &str, key: &str, body: impl Into<Bytes>, opts: WriteOptions) -> Result<WriteResult> {
        require("key", key)?;
        let req = Req::new(Method::POST, object_path(drive, key))
            .query("x-voidfs-patch", "")
            .header("content-type", "application/vnd.voidfs.patch")
            .header_opt("x-voidfs-size", opts.size.map(|s| s.to_string()))
            .guard(opts.if_version.as_ref(), opts.if_match.as_ref())
            .attrs(opts.mtime.as_ref(), opts.mode)
            .body(body);
        self.execute(req).await?.write_result()
    }

    /// Removes `remove` bytes at `offset` and inserts `data` there, shifting what follows, as one
    /// version (§4.3). Not idempotent: without a precondition it is not sent again once it may
    /// have reached the server, so a failure after that leaves it unknown whether it applied. Pass
    /// `if_version` to make it safe to retry.
    pub async fn splice(&self, drive: &str, key: &str, offset: u64, remove: u64, data: impl Into<Bytes>, pre: Preconditions) -> Result<WriteResult> {
        require("key", key)?;
        let req = Req::new(Method::PUT, object_path(drive, key))
            .query("x-voidfs-splice", "")
            .header("x-voidfs-offset", offset.to_string())
            .header_opt("x-voidfs-remove", (remove > 0).then(|| remove.to_string()))
            .guard(pre.if_version.as_ref(), pre.if_match.as_ref())
            .body(data)
            .not_idempotent();
        self.execute(req).await?.write_result()
    }

    /// Inserts `data` at `offset`, shifting what follows: a splice that removes nothing.
    pub async fn insert(&self, drive: &str, key: &str, offset: u64, data: impl Into<Bytes>, pre: Preconditions) -> Result<WriteResult> {
        self.splice(drive, key, offset, 0, data, pre).await
    }

    /// Removes `length` bytes at `offset`, closing the gap: a splice that inserts nothing.
    pub async fn remove_range(&self, drive: &str, key: &str, offset: u64, length: u64, pre: Preconditions) -> Result<WriteResult> {
        self.splice(drive, key, offset, length, Bytes::new(), pre).await
    }

    /// Moves `from` to `to` in the same drive as one metadata-only version; content, history and
    /// ETag stay (§4.7). Both are files, or both folders (ending in `/`), which move with their
    /// subtree. Unguarded, it is sent again only when it cannot have reached the server.
    pub async fn rename(&self, drive: &str, from: &str, to: &str, opts: RenameOptions) -> Result<WriteResult> {
        require("source key", from)?;
        require("destination key", to)?;
        let req = Req::new(Method::PUT, object_path(drive, to))
            .query("x-voidfs-rename", "")
            .header("x-voidfs-source", sign::header_text(from))
            .header_opt("x-voidfs-replace", opts.replace.then_some("true"))
            .guard(opts.if_version.as_ref(), opts.if_match.as_ref())
            .not_idempotent();
        self.execute(req).await?.write_result()
    }

    // -----------------------------------------------------------------------------------------
    // History (§4.4–§4.6)

    /// The object's history, oldest first, every page. With `all`, renames and attribute
    /// changes too (§4.4).
    pub async fn list_versions(&self, drive: &str, key: &str, all: bool) -> Result<Vec<VersionEntry>> {
        require("key", key)?;
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Page {
            versions: Vec<VersionEntry>,
            next_continuation_token: Option<String>,
        }
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let req = Req::new(Method::GET, object_path(drive, key))
                .query("x-voidfs-versions", "")
                .query_opt("x-voidfs-all", all.then_some("true"))
                .query_opt("continuation-token", token.take());
            let page: Page = self.execute(req).await?.json()?;
            out.extend(page.versions);
            match page.next_continuation_token {
                Some(t) => token = Some(t),
                None => return Ok(out),
            }
        }
    }

    /// Makes `version_id`'s content and attributes current again, as a new version (§4.6).
    pub async fn restore_version(&self, drive: &str, key: &str, version_id: &str, pre: Preconditions) -> Result<RestoreResult> {
        require("key", key)?;
        require("version id", version_id)?;
        let req = Req::new(Method::POST, object_path(drive, key)).query("x-voidfs-restore", "").query("versionId", version_id).guard(pre.if_version.as_ref(), pre.if_match.as_ref());
        restore_result(self.execute(req).await?)
    }

    /// Restores the state at `as_of` (RFC 3339): a file's version current then, or a folder's
    /// whole subtree, as one version (§4.6).
    pub async fn restore_as_of(&self, drive: &str, key: &str, as_of: &str, pre: Preconditions) -> Result<RestoreResult> {
        require("key", key)?;
        require("as_of", as_of)?;
        let req = Req::new(Method::POST, object_path(drive, key)).query("x-voidfs-restore", "").header("x-voidfs-as-of", as_of).guard(pre.if_version.as_ref(), pre.if_match.as_ref());
        restore_result(self.execute(req).await?)
    }

    // -----------------------------------------------------------------------------------------
    // Attributes and listings (§4.8–§4.10)

    pub async fn attributes(&self, drive: &str, key: &str, opts: ReadOptions) -> Result<Attributes> {
        self.execute(self.read_req(Method::GET, drive, key, &opts)?.query("x-voidfs-attrs", "")).await?.json()
    }

    /// Changes attributes as one `attrs` version (§4.8).
    pub async fn set_attributes(&self, drive: &str, key: &str, update: AttributesUpdate, pre: Preconditions) -> Result<WriteResult> {
        require("key", key)?;
        let mut body = serde_json::Map::new();
        if let Some(t) = update.mtime {
            body.insert("mtime".into(), t.into());
        }
        if let Some(m) = update.mode {
            body.insert("mode".into(), format!("{m:04o}").into());
        }
        if !update.set_xattrs.is_empty() || !update.remove_xattrs.is_empty() {
            let set: BTreeMap<_, _> = update.set_xattrs.iter().map(|(k, v)| (k.clone(), base64::engine::general_purpose::STANDARD.encode(v))).collect();
            body.insert("xattrs".into(), serde_json::json!({ "set": set, "remove": update.remove_xattrs }));
        }
        if let Some(f) = update.flags {
            body.insert("flags".into(), f.into());
        }
        if let Some(c) = update.content_type {
            body.insert("contentType".into(), c.into());
        }
        let req = Req::new(Method::POST, object_path(drive, key))
            .query("x-voidfs-attrs", "")
            .header("content-type", "application/json")
            .guard(pre.if_version.as_ref(), pre.if_match.as_ref())
            .body(serde_json::to_vec(&body).expect("JSON"));
        self.execute(req).await?.write_result()
    }

    /// One page of a folder's children with their attributes (§4.9). `prefix` is the folder's key
    /// ending in `/`, or empty for the root.
    pub async fn list_folder_page(&self, drive: &str, prefix: &str, continuation_token: Option<&str>) -> Result<FolderPage> {
        let req = Req::new(Method::GET, drive_path(drive)).query("x-voidfs-list", "").query("prefix", prefix).query_opt("continuation-token", continuation_token.map(str::to_owned));
        self.execute(req).await?.json()
    }

    /// A folder's children with their attributes, every page (§4.9).
    pub async fn list_folder(&self, drive: &str, prefix: &str) -> Result<FolderListing> {
        let first = self.list_folder_page(drive, prefix, None).await?;
        let mut listing = FolderListing { prefix: first.prefix, seq: first.seq, entries: first.entries };
        let mut next = first.next_continuation_token;
        while let Some(token) = next {
            let page = self.list_folder_page(drive, prefix, Some(&token)).await?;
            listing.entries.extend(page.entries);
            next = page.next_continuation_token;
        }
        Ok(listing)
    }

    /// Objects deleted and still retained, under `prefix`, every page (§4.10).
    pub async fn list_deleted(&self, drive: &str, prefix: &str) -> Result<Vec<DeletedEntry>> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Page {
            deleted: Vec<DeletedEntry>,
            next_continuation_token: Option<String>,
        }
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let req = Req::new(Method::GET, drive_path(drive)).query("x-voidfs-deleted", "").query_opt("prefix", (!prefix.is_empty()).then(|| prefix.to_owned())).query_opt("continuation-token", token.take());
            let page: Page = self.execute(req).await?.json()?;
            out.extend(page.deleted);
            match page.next_continuation_token {
                Some(t) => token = Some(t),
                None => return Ok(out),
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // The change feed (§5.6)

    /// The changes after drive position `since`, waiting up to `wait` (at most 60 s) for one.
    /// `410 ChangesExpired` if `since` is older than the server holds: relist and resume from the
    /// listing's `seq`.
    pub async fn changes(&self, drive: &str, since: u64, wait: Duration) -> Result<Changes> {
        let wait = wait.min(Duration::from_secs(60));
        let req = Req::new(Method::GET, drive_path(drive)).query("x-voidfs-changes", "").query("since", since.to_string()).header("x-voidfs-wait", wait.as_secs().to_string());
        self.execute_with(req, Patience::Wait(wait)).await?.json()
    }

    /// The changes after `since` as the server publishes them (Server-Sent Events), reconnecting
    /// from the last position delivered when the stream breaks.
    pub fn watch_changes(&self, drive: &str, since: u64) -> ChangeWatch {
        ChangeWatch::new(self.clone(), drive.to_owned(), since)
    }
}

fn restore_result(r: Reply) -> Result<RestoreResult> {
    let w = r.write_result()?;
    Ok(RestoreResult { version_id: w.version_id, restored_from: r.header("x-voidfs-restored-from"), etag: w.etag, size: w.size })
}

/// An object read as it arrives.
pub struct ObjectStream {
    pub meta: ObjectMeta,
    resp: reqwest::Response,
}

impl fmt::Debug for ObjectStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObjectStream").field("meta", &self.meta).finish_non_exhaustive()
    }
}

impl ObjectStream {
    /// The next piece of the body, or `None` at its end. A broken connection is an error: read
    /// again from the offset reached, with `version_id` set to `meta.version_id`.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>> {
        Ok(self.resp.chunk().await?)
    }

    /// The rest of the body.
    pub async fn collect(mut self) -> Result<Bytes> {
        let mut out = bytes::BytesMut::new();
        while let Some(c) = self.chunk().await? {
            out.extend_from_slice(&c);
        }
        Ok(out.freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_comes_from_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |n: &str| pairs.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string());
        let c = Config::from_lookup(env(&[("VOIDFS_ACCESS_KEY_ID", "VFID"), ("VOIDFS_SECRET_ACCESS_KEY", "s")])).unwrap();
        assert_eq!((c.endpoint.as_str(), c.access_key_id.as_str(), c.region.as_str()), (DEFAULT_ENDPOINT, "VFID", DEFAULT_REGION));
        let c = Config::from_lookup(env(&[("VOIDFS_ACCESS_KEY_ID", "VFID"), ("VOIDFS_SECRET_ACCESS_KEY", "s"), ("VOIDFS_ENDPOINT", "https://v.example"), ("VOIDFS_REGION", "auto")])).unwrap();
        assert_eq!((c.endpoint.as_str(), c.region.as_str()), ("https://v.example", "auto"));
        let e = Config::from_lookup(env(&[("VOIDFS_ACCESS_KEY_ID", "VFID"), ("VOIDFS_SECRET_ACCESS_KEY", "")])).unwrap_err();
        assert_eq!(e.to_string(), "VOIDFS_SECRET_ACCESS_KEY is not set");
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let c = Config { access_key_id: "VFID".into(), secret_access_key: "hunter2hunter2".into(), ..Config::default() };
        assert!(!format!("{c:?}").contains("hunter2"));
        let client = Client::new(c).unwrap();
        assert!(!format!("{client:?}").contains("hunter2"));
        assert!(Client::new(Config::default()).is_err(), "no key");
    }

    #[test]
    fn splices_and_renames_replay_only_when_unguarded_and_unsent() {
        let r = Req::new(Method::PUT, "/d/k".into()).not_idempotent();
        assert_eq!(r.replay, Replay::OnlyIfUnsent);
        let r = Req::new(Method::PUT, "/d/k".into()).guard(Some(&"1.0".to_string()), None).not_idempotent();
        assert_eq!(r.replay, Replay::Safe);
        let r = Req::new(Method::PUT, "/d/k".into()).guard(None, Some(&"\"e\"".to_string())).not_idempotent();
        assert_eq!(r.replay, Replay::Safe);
        assert_eq!(Req::new(Method::PUT, "/d/k".into()).replay, Replay::Safe);
    }

    #[test]
    fn object_headers_are_read() {
        let mut h = HeaderMap::new();
        for (k, v) in [("x-amz-version-id", "3.0"), ("etag", "\"3.0\""), ("content-range", "bytes 2-4/10"), ("content-length", "3"), ("x-voidfs-kind", "file"), ("x-voidfs-object-id", "o-1"), ("x-voidfs-mode", "0644"), ("x-amz-meta-colour", "blue")] {
            h.insert(k, v.parse().unwrap());
        }
        let m = object_meta(206, &h).unwrap();
        assert_eq!((m.version_id.as_str(), m.size, m.kind, m.mode.as_deref()), ("3.0", 10, Kind::File, Some("0644")));
        assert_eq!(m.metadata.get("colour").map(String::as_str), Some("blue"));
        assert_eq!(object_meta(200, &h).unwrap().size, 3);
        h.remove("x-amz-version-id");
        assert!(matches!(object_meta(200, &h), Err(Error::Decode(_))));
    }
}
