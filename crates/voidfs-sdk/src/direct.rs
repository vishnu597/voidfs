// SPDX-License-Identifier: Apache-2.0
//! Direct uploads (protocol §4.11): the client cuts content into shards, asks the server which
//! of them its pool lacks, PUTs those straight to the bucket with the URLs the server presigned,
//! and commits. It pays when the drive holds most of the content already, such as a large file
//! changed in one place; new bytes are usually as fast through an ordinary put, which doesn't
//! wait on the bucket. Opt-in, as SpaceFS's Rust SDK's is: [`Client::put_object_direct`], or
//! [`crate::Config::direct_uploads`] for [`Client::put_object`].

use std::collections::BTreeMap;

use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use http::Method;
use serde::Deserialize;
use voidfs_core::chunk::{self, Params};
use voidfs_core::ids::ShardHash;

use crate::client::{Client, Req, paced};
use crate::error::service_error;
use crate::retry::{self, Replay};
use crate::sign::object_path;
use crate::types::{PutOptions, WriteResult};
use crate::{Error, Result};

/// Bodies of this size and more go direct when [`crate::Config::direct_uploads`] is on, as
/// SpaceFS's do.
pub const DIRECT_MIN_BYTES: usize = 8 << 20;
/// The most shards a plan may list (protocol §7).
pub const MAX_SHARDS: usize = 4096;
/// Shards a direct upload sends at once.
pub const SHARD_UPLOADS: usize = 8;

/// What a plan answered (protocol §4.11).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadPlan {
    pub token: String,
    pub expires_seconds: u64,
    /// How many of the listed shards (each counted once) the pool holds already.
    pub held: u64,
    /// The shards to PUT, each once.
    pub upload: Vec<PlannedShard>,
}

/// A shard to PUT, where, and with which headers.
#[derive(Clone, Debug, Deserialize)]
pub struct PlannedShard {
    /// Its SHA-256, in hexadecimal.
    pub hash: String,
    pub length: u64,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

/// Content cut into shards as format §4.1 says, with a default `voidfs.json`'s sizes: each
/// shard's hash and length, in order. Shards are cut the same wherever the content is, so a
/// drive that holds a version of it holds most of these.
pub fn shards_of(content: &Bytes) -> Vec<(ShardHash, u64)> {
    chunk::shards(content, Params::DEFAULT).iter().map(|s| (s.hash, s.bytes.len() as u64)).collect()
}

/// Whether a failure is the server's answer about the object, which stands, or a reason to put
/// the ordinary way (protocol §4.11).
fn stands(e: &Error) -> bool {
    matches!(e.status(), Some(409 | 412))
}

impl Client {
    /// Asks which of `shards` (the content, in order) the drive's pool holds for `key`, and for
    /// URLs to PUT the rest to. `501` where the server doesn't offer direct uploads.
    pub async fn plan_upload(&self, drive: &str, key: &str, shards: &[(ShardHash, u64)]) -> Result<UploadPlan> {
        if key.is_empty() {
            return Err(Error::Invalid("key is empty".into()));
        }
        let list: Vec<_> = shards.iter().map(|(h, n)| serde_json::json!({ "hash": h.to_hex(), "length": n })).collect();
        let body = serde_json::to_vec(&serde_json::json!({ "shards": list })).map_err(|e| Error::Invalid(e.to_string()))?;
        let req = Req::new(Method::POST, object_path(drive, key)).query("x-voidfs-upload-plan", "").header("content-type", "application/json").body(body);
        let r = self.execute(req).await?;
        serde_json::from_slice(&r.body).map_err(|e| Error::decode(format!("{e}: {}", String::from_utf8_lossy(&r.body[..r.body.len().min(200)]))))
    }

    /// PUTs one shard's bytes to the URL its plan gave, with exactly its headers, within this
    /// client's upload bandwidth. A `412` is the bucket saying it has the shard already, which is
    /// as good. Retried as an idempotent request.
    pub async fn upload_shard(&self, shard: &PlannedShard, bytes: Bytes) -> Result<()> {
        if bytes.len() as u64 != shard.length {
            return Err(Error::Invalid(format!("shard {} is {} bytes, not {}", shard.hash, bytes.len(), shard.length)));
        }
        let mut attempt = 1;
        loop {
            let mut req = self.http().put(&shard.url);
            for (k, v) in &shard.headers {
                req = req.header(k, v);
            }
            req = match &self.config().upload_bandwidth {
                Some(bw) => req.header(http::header::CONTENT_LENGTH, bytes.len()).body(reqwest::Body::wrap_stream(paced(bytes.clone(), bw.clone()))),
                None => req.body(bytes.clone()),
            };
            let r = match req.send().await {
                Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 412 => return Ok(()),
                Ok(resp) => {
                    let (status, headers) = (resp.status().as_u16(), resp.headers().clone());
                    let body = resp.bytes().await.unwrap_or_default();
                    Error::Service(service_error(status, &headers, &body))
                }
                Err(e) => Error::from(e),
            };
            if attempt < self.config().max_attempts.max(1) && retry::may_retry(&r, Replay::Safe) {
                tokio::time::sleep(retry::delay(attempt, &r)).await;
                attempt += 1;
                continue;
            }
            return Err(r);
        }
    }

    /// Commits a planned upload as a version of `key`: a put of `shards` in order, whose content
    /// has the SHA-256 `content_sha256` (hexadecimal), with the token of their plan. `opts` are a
    /// put's: preconditions, content type, metadata, mtime and mode.
    pub async fn commit_upload(&self, drive: &str, key: &str, token: &str, shards: &[(ShardHash, u64)], content_sha256: &str, opts: PutOptions) -> Result<WriteResult> {
        if key.is_empty() {
            return Err(Error::Invalid("key is empty".into()));
        }
        let size: u64 = shards.iter().map(|(_, n)| n).sum();
        let list: Vec<_> = shards.iter().map(|(h, n)| serde_json::json!({ "hash": h.to_hex(), "length": n })).collect();
        let body = serde_json::json!({ "token": token, "size": size, "contentSha256": content_sha256, "shards": list });
        let mut req = Req::new(Method::PUT, object_path(drive, key))
            .query("x-voidfs-upload-commit", "")
            .header("content-type", "application/json")
            .header_opt("x-voidfs-content-type", opts.content_type)
            .guard(opts.if_version.as_ref(), opts.if_match.as_ref())
            .header_opt("if-none-match", opts.if_none_match_any.then_some("*"))
            .attrs(opts.mtime.as_ref(), opts.mode)
            .body(serde_json::to_vec(&body).map_err(|e| Error::Invalid(e.to_string()))?);
        for (k, v) in &opts.metadata {
            req = req.header(&format!("x-amz-meta-{k}"), v.clone());
        }
        self.execute(req).await?.write_result()
    }

    /// Sends the shards a plan lists, cut from `content`, [`SHARD_UPLOADS`] at once.
    pub async fn upload_planned(&self, plan: &UploadPlan, content: &Bytes) -> Result<()> {
        let mut by_hash = std::collections::HashMap::new();
        for s in chunk::shards(content, Params::DEFAULT) {
            by_hash.insert(s.hash.to_hex(), s.bytes);
        }
        let mut sends = Vec::with_capacity(plan.upload.len());
        for p in &plan.upload {
            let bytes = by_hash.get(&p.hash).cloned().ok_or_else(|| Error::Invalid(format!("the plan lists shard {}, which the content does not have", p.hash)))?;
            let (this, p) = (self.clone(), p.clone());
            sends.push(async move { this.upload_shard(&p, bytes).await });
        }
        futures::stream::iter(sends).buffer_unordered(SHARD_UPLOADS).try_collect::<Vec<()>>().await?;
        Ok(())
    }

    /// Writes a whole object as [`Client::put_object`] does, sending only the shards the drive's
    /// pool lacks, straight to the bucket (protocol §4.11). It puts the ordinary way if the body is
    /// under [`DIRECT_MIN_BYTES`] or has more than [`MAX_SHARDS`] shards, and if any step fails with
    /// anything but `409` or `412`, which are answers about the object and stand: a server that
    /// doesn't offer direct uploads, a bucket it can't reach, a plan whose token expired.
    pub async fn put_object_direct(&self, drive: &str, key: &str, body: impl Into<Bytes>, opts: PutOptions) -> Result<WriteResult> {
        let body: Bytes = body.into();
        if body.len() < DIRECT_MIN_BYTES {
            return self.put_plain(drive, key, body, opts).await;
        }
        let shards = shards_of(&body);
        if shards.len() > MAX_SHARDS {
            return self.put_plain(drive, key, body, opts).await;
        }
        let attempt = async {
            let plan = self.plan_upload(drive, key, &shards).await?;
            self.upload_planned(&plan, &body).await?;
            self.commit_upload(drive, key, &plan.token, &shards, &ShardHash::of(&body).to_hex(), opts.clone()).await
        };
        match attempt.await {
            Ok(w) => Ok(w),
            Err(e) if stands(&e) => Err(e),
            Err(_) => self.put_plain(drive, key, body, opts).await,
        }
    }
}
