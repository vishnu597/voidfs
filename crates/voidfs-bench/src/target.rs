// SPDX-License-Identifier: Apache-2.0
//! What a scenario runs against: a voidfs server, or the bare bucket underneath it.
//!
//! Both use `aws-sdk-s3` for standard S3 calls, configured the same way. voidfs's extensions
//! (`x-voidfs-*`) are sent as SigV4-signed requests, as the conformance runner sends them.
//! Every request holds one of a fixed number of permits while it runs, which stands in for
//! SpaceFS's "64 connections".

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context, anyhow, bail, ensure};
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation};
use aws_sdk_s3::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_smithy_runtime_api::http::Response as HttpResponse;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier};
use aws_sigv4::http_request::{PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings, UriPathNormalizationMode, sign};
use aws_sigv4::sign::v4;
use bytes::Bytes;
use futures::StreamExt;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, SemaphorePermit};

/// Unreserved characters stay; everything else is encoded (SigV4's rule for query values).
const QUERY: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
/// The same, keeping `/` (key paths and `x-amz-copy-source`).
const PATH: &AsciiSet = &QUERY.remove(b'/');

/// Copies in flight at once within one folder move on a bare bucket.
const MOVE_PARALLELISM: usize = 8;
/// Keys per DeleteObjects request (the S3 maximum).
const DELETE_BATCH: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flavor {
    Voidfs,
    Bare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Checksums {
    /// The SDK's default: a CRC32 on every upload, validated on download when present.
    SdkDefault,
    /// Only where S3 requires one (DeleteObjects), as SpaceFS's own SDK configures it.
    WhenRequired,
}

/// How to reach an S3 endpoint.
pub struct Endpoint {
    /// `None` means AWS S3 itself, with virtual-hosted addressing.
    pub url: Option<String>,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// Where one scenario's objects live on a target.
#[derive(Clone, Debug)]
pub struct Place {
    pub bucket: String,
    /// Prefix of every key, empty or ending in `/`.
    pub root: String,
}

impl Place {
    fn key(&self, k: &str) -> String {
        format!("{}{k}", self.root)
    }
}

/// Requests sent and payload bytes moved, as the client sees them.
#[derive(Default)]
pub struct Counters {
    requests: AtomicU64,
    up: AtomicU64,
    down: AtomicU64,
}

impl Counters {
    fn moved(&self, up: usize, down: usize) {
        self.up.fetch_add(up as u64, Ordering::Relaxed);
        self.down.fetch_add(down as u64, Ordering::Relaxed);
    }

    /// Requests, bytes up, bytes down, so far.
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (self.requests.load(Ordering::Relaxed), self.up.load(Ordering::Relaxed), self.down.load(Ordering::Relaxed))
    }
}

pub struct Target {
    pub flavor: Flavor,
    pub counters: Counters,
    /// Shown in reports; carries no secrets.
    pub describe: String,
    s3: aws_sdk_s3::Client,
    ext: Option<Ext>,
    /// voidfs: prefix of the drive made for each scenario. Bare: the bucket.
    space: String,
    /// Bare only: prefix of every key in the bucket.
    prefix: String,
    permits: Semaphore,
}

/// An SDK error as `status Code: message`, or the full chain when there was no response.
fn sdk<E, R>(e: SdkError<E, R>) -> anyhow::Error
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug + HasStatus,
{
    match e.raw_response() {
        Some(r) => anyhow!("{} {}: {}", r.status_u16(), e.code().unwrap_or("error"), e.message().unwrap_or_default()),
        None => anyhow!("{}", DisplayErrorContext(e)),
    }
}

trait HasStatus {
    fn status_u16(&self) -> u16;
}

impl HasStatus for HttpResponse {
    fn status_u16(&self) -> u16 {
        self.status().as_u16()
    }
}

fn s3_client(ep: &Endpoint, checksums: Checksums) -> aws_sdk_s3::Client {
    let credentials = Credentials::new(&ep.access_key_id, &ep.secret_access_key, None, None, "voidfs-bench");
    let mut b = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(ep.region.clone()))
        .credentials_provider(credentials)
        .force_path_style(ep.url.is_some());
    if let Some(url) = &ep.url {
        b = b.endpoint_url(url);
    }
    if checksums == Checksums::WhenRequired {
        b = b
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
    }
    aws_sdk_s3::Client::from_conf(b.build())
}

impl Target {
    /// A voidfs server. Each scenario gets a fresh drive named `<drive_prefix>-<run>-<nn>`.
    pub fn voidfs(ep: Endpoint, drive_prefix: &str, connections: usize, checksums: Checksums) -> anyhow::Result<Target> {
        let url = ep.url.clone().ok_or_else(|| anyhow!("the voidfs target needs an endpoint"))?;
        let ext = Ext::new(&url, &ep.access_key_id, &ep.secret_access_key, &ep.region)?;
        Ok(Target {
            flavor: Flavor::Voidfs,
            describe: format!("{url} (drives {drive_prefix}-*)"),
            s3: s3_client(&ep, checksums),
            counters: Counters::default(),
            ext: Some(ext),
            space: drive_prefix.to_owned(),
            prefix: String::new(),
            permits: Semaphore::new(connections),
        })
    }

    /// A bare bucket. Each scenario's keys go under `<prefix><run>/<scenario>/`.
    pub fn bare(ep: Endpoint, bucket: &str, prefix: &str, connections: usize, checksums: Checksums) -> Target {
        let at = ep.url.clone().unwrap_or_else(|| format!("AWS S3 {}", ep.region));
        Target {
            flavor: Flavor::Bare,
            describe: format!("{at}, bucket {bucket}, prefix {prefix}"),
            s3: s3_client(&ep, checksums),
            counters: Counters::default(),
            ext: None,
            space: bucket.to_owned(),
            prefix: prefix.to_owned(),
            permits: Semaphore::new(connections),
        }
    }

    pub fn name(&self) -> &'static str {
        match self.flavor {
            Flavor::Voidfs => "voidfs",
            Flavor::Bare => "bare",
        }
    }

    pub fn place(&self, run: &str, index: usize, scenario: &str) -> Place {
        match self.flavor {
            Flavor::Voidfs => Place { bucket: format!("{}-{run}-{index:02}", self.space), root: String::new() },
            Flavor::Bare => Place { bucket: self.space.clone(), root: format!("{}{run}/{scenario}/", self.prefix) },
        }
    }

    /// Every key under `root` in a bare target's bucket.
    pub fn place_at(&self, root: &str) -> Place {
        Place { bucket: self.space.clone(), root: root.to_owned() }
    }

    /// Makes a scenario's place ready: a new drive on voidfs, nothing on a bare bucket.
    pub async fn prepare(&self, p: &Place) -> anyhow::Result<()> {
        if self.flavor == Flavor::Voidfs {
            let _g = self.permit().await;
            self.s3.create_bucket().bucket(&p.bucket).send().await.map_err(sdk).with_context(|| format!("creating drive {}", p.bucket))?;
        }
        Ok(())
    }

    /// Removes everything a scenario made. On voidfs the drive is hard-deleted; its shards stay
    /// in the pool, because voidfs has no garbage collection yet.
    pub async fn destroy(&self, p: &Place) -> anyhow::Result<()> {
        match &self.ext {
            Some(ext) => {
                let _g = self.permit().await;
                let r = ext.send("DELETE", &format!("/{}", p.bucket), "", vec![("x-voidfs-hard-delete".into(), "true".into())], Bytes::new()).await?;
                ensure!(r.status < 300 || r.status == 404, "deleting drive {}: {}", p.bucket, r.brief());
                Ok(())
            }
            None => {
                let keys = self.list(p, "").await?;
                self.delete_many(p, &keys).await
            }
        }
    }

    /// Waits for a free connection slot; every request takes one.
    async fn permit(&self) -> SemaphorePermit<'_> {
        self.counters.requests.fetch_add(1, Ordering::Relaxed);
        self.permits.acquire().await.expect("the semaphore is never closed")
    }

    // -----------------------------------------------------------------------------------------
    // Standard S3

    pub async fn put(&self, p: &Place, key: &str, body: Bytes) -> anyhow::Result<()> {
        let _g = self.permit().await;
        self.counters.moved(body.len(), 0);
        self.s3.put_object().bucket(&p.bucket).key(p.key(key)).body(ByteStream::from(body)).send().await.map_err(sdk)?;
        Ok(())
    }

    /// GetObject, with the body collected into memory.
    pub async fn get(&self, p: &Place, key: &str) -> anyhow::Result<Bytes> {
        let _g = self.permit().await;
        let out = self.s3.get_object().bucket(&p.bucket).key(p.key(key)).send().await.map_err(sdk)?;
        let body = out.body.collect().await?.into_bytes();
        self.counters.moved(0, body.len());
        Ok(body)
    }

    /// GetObject into a buffer that can then be edited.
    pub async fn get_vec(&self, p: &Place, key: &str, size_hint: u64) -> anyhow::Result<Vec<u8>> {
        let _g = self.permit().await;
        let mut out = self.s3.get_object().bucket(&p.bucket).key(p.key(key)).send().await.map_err(sdk)?;
        let mut buf = Vec::with_capacity(size_hint as usize + 8192);
        while let Some(chunk) = out.body.next().await {
            buf.extend_from_slice(&chunk?);
        }
        self.counters.moved(0, buf.len());
        Ok(buf)
    }

    /// GetObject, reading the body chunk by chunk and dropping it. Returns its length.
    pub async fn stream_get(&self, p: &Place, key: &str) -> anyhow::Result<u64> {
        let _g = self.permit().await;
        let mut out = self.s3.get_object().bucket(&p.bucket).key(p.key(key)).send().await.map_err(sdk)?;
        let mut n = 0u64;
        while let Some(chunk) = out.body.next().await {
            n += chunk?.len() as u64;
        }
        self.counters.moved(0, n as usize);
        Ok(n)
    }

    pub async fn get_range(&self, p: &Place, key: &str, start: u64, len: u64) -> anyhow::Result<Bytes> {
        let _g = self.permit().await;
        let out = self
            .s3
            .get_object()
            .bucket(&p.bucket)
            .key(p.key(key))
            .range(format!("bytes={start}-{}", start + len - 1))
            .send()
            .await
            .map_err(sdk)?;
        let body = out.body.collect().await?.into_bytes();
        self.counters.moved(0, body.len());
        Ok(body)
    }

    /// HeadObject; returns the object's size.
    pub async fn head(&self, p: &Place, key: &str) -> anyhow::Result<u64> {
        let _g = self.permit().await;
        let out = self.s3.head_object().bucket(&p.bucket).key(p.key(key)).send().await.map_err(sdk)?;
        Ok(out.content_length().unwrap_or_default().try_into()?)
    }

    /// ListObjectsV2 of `prefix`, every page. Returns keys relative to the place.
    pub async fn list(&self, p: &Place, prefix: &str) -> anyhow::Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let _g = self.permit().await;
            let out = self
                .s3
                .list_objects_v2()
                .bucket(&p.bucket)
                .prefix(p.key(prefix))
                .max_keys(1000)
                .set_continuation_token(token.take())
                .send()
                .await
                .map_err(sdk)?;
            for o in out.contents() {
                if let Some(k) = o.key() {
                    keys.push(k.strip_prefix(p.root.as_str()).unwrap_or(k).to_owned());
                }
            }
            match out.next_continuation_token() {
                Some(t) if out.is_truncated() == Some(true) => token = Some(t.to_owned()),
                _ => break,
            }
        }
        Ok(keys)
    }

    pub async fn copy(&self, p: &Place, src: &str, dst: &str) -> anyhow::Result<()> {
        let _g = self.permit().await;
        let source = format!("{}/{}", p.bucket, utf8_percent_encode(&p.key(src), PATH));
        self.s3.copy_object().bucket(&p.bucket).key(p.key(dst)).copy_source(source).send().await.map_err(sdk)?;
        Ok(())
    }

    pub async fn delete(&self, p: &Place, key: &str) -> anyhow::Result<()> {
        let _g = self.permit().await;
        self.s3.delete_object().bucket(&p.bucket).key(p.key(key)).send().await.map_err(sdk)?;
        Ok(())
    }

    /// DeleteObjects, 1,000 keys per request.
    pub async fn delete_many(&self, p: &Place, keys: &[String]) -> anyhow::Result<()> {
        for batch in keys.chunks(DELETE_BATCH) {
            let ids = batch.iter().map(|k| ObjectIdentifier::builder().key(p.key(k)).build()).collect::<Result<Vec<_>, _>>()?;
            let delete = Delete::builder().set_objects(Some(ids)).quiet(true).build()?;
            let _g = self.permit().await;
            let out = self.s3.delete_objects().bucket(&p.bucket).delete(delete).send().await.map_err(sdk)?;
            if let Some(e) = out.errors().first() {
                bail!("deleting {}: {}", e.key().unwrap_or("?"), e.message().unwrap_or("?"));
            }
        }
        Ok(())
    }

    /// A multipart upload of `body` in `part`-byte parts, all uploaded at once (each still
    /// needs a permit, so at most `connections` are in flight across the whole run).
    pub async fn multipart(&self, p: &Place, key: &str, body: Bytes, part: u64) -> anyhow::Result<()> {
        let full = p.key(key);
        let id = {
            let _g = self.permit().await;
            let out = self.s3.create_multipart_upload().bucket(&p.bucket).key(&full).send().await.map_err(sdk)?;
            out.upload_id().ok_or_else(|| anyhow!("CreateMultipartUpload returned no upload id"))?.to_owned()
        };
        let uploads = body.chunks(part as usize).enumerate().map(|(i, _)| {
            let start = i * part as usize;
            let chunk = body.slice(start..(start + part as usize).min(body.len()));
            let (full, id) = (&full, &id);
            async move {
                let n = i as i32 + 1;
                let _g = self.permit().await;
                self.counters.moved(chunk.len(), 0);
                let out = self
                    .s3
                    .upload_part()
                    .bucket(&p.bucket)
                    .key(full)
                    .upload_id(id)
                    .part_number(n)
                    .body(ByteStream::from(chunk))
                    .send()
                    .await
                    .map_err(sdk)?;
                anyhow::Ok(CompletedPart::builder().part_number(n).set_e_tag(out.e_tag().map(str::to_owned)).build())
            }
        });
        let parts = match futures::future::try_join_all(uploads).await {
            Ok(parts) => parts,
            Err(e) => {
                let _g = self.permit().await;
                let _ = self.s3.abort_multipart_upload().bucket(&p.bucket).key(&full).upload_id(&id).send().await;
                return Err(e);
            }
        };
        let _g = self.permit().await;
        self.s3
            .complete_multipart_upload()
            .bucket(&p.bucket)
            .key(&full)
            .upload_id(&id)
            .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(parts)).build())
            .send()
            .await
            .map_err(sdk)?;
        Ok(())
    }

    /// Moves every object under the folder `src` to `dst` the only way a bare bucket can:
    /// list, copy each, then delete the originals. Returns how many objects moved.
    pub async fn move_prefix(&self, p: &Place, src: &str, dst: &str) -> anyhow::Result<usize> {
        let keys = self.list(p, src).await?;
        let copies = futures::stream::iter(keys.clone().into_iter().map(|k: String| {
            let to = format!("{dst}{}", &k[src.len()..]);
            async move { self.copy(p, &k, &to).await }
        }))
        .buffer_unordered(MOVE_PARALLELISM)
        .collect::<Vec<_>>()
        .await;
        copies.into_iter().collect::<anyhow::Result<Vec<()>>>()?;
        self.delete_many(p, &keys).await?;
        Ok(keys.len())
    }

    // -----------------------------------------------------------------------------------------
    // voidfs extensions (protocol §4)

    fn ext(&self) -> anyhow::Result<&Ext> {
        self.ext.as_ref().ok_or_else(|| anyhow!("extensions need a voidfs target"))
    }

    fn object_path(p: &Place, key: &str) -> String {
        format!("/{}/{}", p.bucket, utf8_percent_encode(&p.key(key), PATH))
    }

    /// Sends an object extension; returns the object's size afterwards (`x-voidfs-size`).
    async fn edit(&self, method: &str, p: &Place, key: &str, query: &str, headers: Vec<(String, String)>, body: Bytes) -> anyhow::Result<u64> {
        let _g = self.permit().await;
        self.counters.moved(body.len(), 0);
        let r = self.ext()?.send(method, &Self::object_path(p, key), query, headers, body).await?;
        ensure!(r.status == 200, "{method} ?{query} on {key}: {}", r.brief());
        let size = r.headers.get("x-voidfs-size").and_then(|v| v.to_str().ok()).ok_or_else(|| anyhow!("no x-voidfs-size in the response"))?;
        Ok(size.parse()?)
    }

    /// `?x-voidfs-write` (§4.1): `data` at `offset`, and the size afterwards if given.
    pub async fn write_at(&self, p: &Place, key: &str, offset: u64, data: Bytes, size: Option<u64>) -> anyhow::Result<u64> {
        let mut h = vec![("x-voidfs-offset".to_string(), offset.to_string())];
        if let Some(n) = size {
            h.push(("x-voidfs-size".into(), n.to_string()));
        }
        self.edit("PUT", p, key, "x-voidfs-write", h, data).await
    }

    /// `?x-voidfs-splice` (§4.3).
    pub async fn splice(&self, p: &Place, key: &str, offset: u64, remove: u64, data: Bytes) -> anyhow::Result<u64> {
        let h = vec![("x-voidfs-offset".to_string(), offset.to_string()), ("x-voidfs-remove".into(), remove.to_string())];
        self.edit("PUT", p, key, "x-voidfs-splice", h, data).await
    }

    /// `?x-voidfs-patch` (§4.2) with an encoded body.
    pub async fn patch(&self, p: &Place, key: &str, body: Bytes) -> anyhow::Result<u64> {
        let h = vec![("content-type".to_string(), "application/vnd.voidfs.patch".into())];
        self.edit("POST", p, key, "x-voidfs-patch", h, body).await
    }

    /// `?x-voidfs-rename` (§4.7): a file, or a folder with everything under it.
    pub async fn rename(&self, p: &Place, src: &str, dst: &str) -> anyhow::Result<()> {
        let h = vec![("x-voidfs-source".to_string(), utf8_percent_encode(&p.key(src), PATH).to_string())];
        let _g = self.permit().await;
        let r = self.ext()?.send("PUT", &Self::object_path(p, dst), "x-voidfs-rename", h, Bytes::new()).await?;
        ensure!(r.status == 200, "renaming {src} to {dst}: {}", r.brief());
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Signed extension requests

struct Ext {
    http: reqwest::Client,
    endpoint: String,
    authority: String,
    access_key_id: String,
    secret_access_key: String,
    region: String,
}

struct ExtResponse {
    status: u16,
    headers: http::HeaderMap,
    body: Bytes,
}

impl ExtResponse {
    fn brief(&self) -> String {
        let text: String = String::from_utf8_lossy(&self.body).chars().take(300).collect();
        format!("status {} {text}", self.status)
    }
}

impl Ext {
    fn new(endpoint: &str, access_key_id: &str, secret_access_key: &str, region: &str) -> anyhow::Result<Ext> {
        let endpoint = endpoint.trim_end_matches('/').to_owned();
        let uri: http::Uri = endpoint.parse().with_context(|| format!("endpoint {endpoint}"))?;
        let authority = uri.authority().ok_or_else(|| anyhow!("endpoint {endpoint} has no host"))?.to_string();
        let http = reqwest::Client::builder().timeout(Duration::from_secs(600)).build()?;
        Ok(Ext {
            http,
            endpoint,
            authority,
            access_key_id: access_key_id.to_owned(),
            secret_access_key: secret_access_key.to_owned(),
            region: region.to_owned(),
        })
    }

    /// Signs every header, `x-voidfs-*` included (protocol §2), and sends the request.
    async fn send(&self, method: &str, path: &str, query: &str, mut headers: Vec<(String, String)>, body: Bytes) -> anyhow::Result<ExtResponse> {
        let uri = if query.is_empty() { format!("{}{path}", self.endpoint) } else { format!("{}{path}?{query}", self.endpoint) };
        headers.push(("host".into(), self.authority.clone()));
        let identity = aws_credential_types::Credentials::new(&self.access_key_id, &self.secret_access_key, None, None, "voidfs-bench").into();
        let mut settings = SigningSettings::default();
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        let params = v4::SigningParams::builder().identity(&identity).region(&self.region).name("s3").time(SystemTime::now()).settings(settings).build()?.into();
        let signable = SignableRequest::new(method, &uri, headers.iter().map(|(k, v)| (k.as_str(), v.as_str())), SignableBody::Bytes(&body))?;
        let (instructions, _) = sign(signable, &params)?.into_parts();
        let mut builder = http::Request::builder().method(method).uri(&uri);
        for (k, v) in &headers {
            builder = builder.header(k, v);
        }
        let mut request = builder.body(body)?;
        instructions.apply_to_request_http1x(&mut request);
        let resp = self.http.execute(reqwest::Request::try_from(request)?).await.with_context(|| format!("{method} {path}?{query}"))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.bytes().await?;
        Ok(ExtResponse { status, headers, body })
    }
}

