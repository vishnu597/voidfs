// SPDX-License-Identifier: Apache-2.0
//! The voidfs SDK for Rust: the official AWS SDK for S3, plus typed calls for the `x-voidfs-*`
//! extensions of the voidfs protocol (`spec/protocol.md`): writes at an offset, patches, inserts
//! and removals, renames, history and rollback, forks, attributes, listings with attributes and
//! the change feed.
//!
//! ```no_run
//! # async fn demo() -> voidfs_sdk::Result<()> {
//! use voidfs_sdk::{Client, Config, Edit, Preconditions, ReadOptions, WriteOptions};
//!
//! let client = Client::new(Config {
//!     endpoint: "http://127.0.0.1:9000".into(),
//!     access_key_id: std::env::var("VOIDFS_ACCESS_KEY_ID").unwrap(),
//!     secret_access_key: std::env::var("VOIDFS_SECRET_ACCESS_KEY").unwrap(),
//!     ..Default::default()
//! })?;
//! client.create_drive("footage", Default::default()).await?;
//! let v1 = client.put_object("footage", "cut.txt", "hello world", Default::default()).await?;
//! // pwrite, guarded by the version it read
//! client.write_at("footage", "cut.txt", 6, "WORLD", WriteOptions { if_version: Some(v1.version_id.clone()), ..Default::default() }).await?;
//! client.patch("footage", "cut.txt", &[Edit::new(0, "H"), Edit::new(11, "!")], Default::default()).await?;
//! // insert bytes, shifting what follows; guarded, so it is safe to retry
//! let head = client.head_object("footage", "cut.txt", Default::default()).await?;
//! client.insert("footage", "cut.txt", 5, ",", Preconditions::if_version(head.version_id)).await?;
//! // history and rollback
//! let history = client.list_versions("footage", "cut.txt", false).await?;
//! let first = client.get_object("footage", "cut.txt", ReadOptions { version_id: Some(history[0].version_id.clone()), ..Default::default() }).await?;
//! client.restore_version("footage", "cut.txt", &v1.version_id, Default::default()).await?;
//! client.rename("footage", "cut.txt", "final/cut.txt", Default::default()).await?;
//! client.fork_drive("footage", "footage-experiment").await?;
//! // any standard S3 call
//! client.s3().head_object().bucket("footage").key("final/cut.txt").send().await?;
//! # let _ = first;
//! # Ok(()) }
//! ```
//!
//! **Direct uploads** (protocol §4.11): [`Client::put_object_direct`] sends only the shards of a
//! body the drive's pool lacks, straight to the bucket, and falls back to a put; opt-in, as
//! SpaceFS's Rust SDK's are, or for every large `put_object` with [`Config::direct_uploads`].
//!
//! **Storage credentials** (protocol §5.5): [`Client::storage_credentials`] gives read-only
//! credentials to a drive's storage, and [`Client::storage`] reads with them, straight from the
//! bucket: the pool's descriptor, the drive's checkpoints and log, its pages and shards.
//!
//! **Errors.** Every call fails with [`Error`]: [`Error::status`], [`Error::code`] and, on a
//! `412`, [`Error::current_version_id`], the version to re-read from before retrying. The errors
//! of [`Client::s3`]'s calls convert into it, with `?`.
//!
//! **Retries.** Extension calls are attempted up to [`Config::max_attempts`] times on `429`,
//! `500`, `502`, `503` and `504`, timeouts and broken connections, with jittered backoff and
//! `Retry-After` honoured. An unguarded insert or removal ([`Client::splice`]), and an unguarded
//! rename, are sent again only after a failure to connect, which cannot have reached the server:
//! a second attempt after the first applied would insert or remove twice. Pass `if_version` to
//! make them safe to retry; the second attempt of one that landed then fails with `412`. A `412`
//! is never retried.

pub mod bandwidth;
pub mod direct;
mod client;
mod error;
mod feed;
mod retry;
pub mod sign;
mod storage;
mod types;

/// The AWS SDK for S3 this crate is built on, for the types of [`Client::s3`]'s calls.
pub use aws_sdk_s3;
pub use bandwidth::Bandwidth;
pub use client::{Client, Config, DEFAULT_ENDPOINT, DEFAULT_REGION, ObjectStream, Observation, Observe};
pub use direct::{DIRECT_MIN_BYTES, PlannedShard, UploadPlan};
pub use error::{Error, Result, ServiceError};
pub use feed::{ChangeWatch, ChangeWatchEvent};
pub use storage::Storage;
pub use types::*;

/// Edits per patch (protocol §7).
pub const MAX_PATCH_EDITS: usize = voidfs_core::patch::MAX_EDITS as usize;
