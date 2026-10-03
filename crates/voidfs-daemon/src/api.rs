// SPDX-License-Identifier: Apache-2.0
//! What the daemon's socket speaks: HTTP/1.1 with JSON bodies, under `/v1/`. The Mac app speaks
//! the same.
//!
//! - `GET /v1/status`: [`Status`], the one page `void status` prints.
//! - `GET /v1/info`: [`Info`], the build, the journal's unpublished bytes, and whether a restart
//!   is safe, as SpaceFS's `daemon info` reports.
//! - `POST /v1/stop`: answers `202` and stops: what is uploading stops at its next request, and
//!   goes on from the journal when the daemon starts again.
//! - `POST /v1/uploads`: [`NewBatch`], files and folders of the user's to upload as one batch;
//!   answers [`Queued`].
//! - `GET /v1/uploads[?all=true][&batch=<id>]`: [`UploadList`], what is not yet published (or
//!   everything, with `all`), and every batch (or one).
//! - `GET /v1/uploads/watch[?all=true][&batch=<id>]`: the same as a stream, one JSON document a
//!   line, each second.
//! - `POST /v1/uploads/pause`, `…/resume` and `…/cancel`: a [`Scope`]; answer [`Affected`].
//! - `PUT /v1/uploads/limit`: [`Limit`], the upload bandwidth limit, at once.
//! - `POST /v1/uploads/clear`: forgets finished uploads; answers [`Cleared`].
//!
//! An error is `{"error": {"code", "message"}}`, with a 4xx or 5xx status.

use serde::{Deserialize, Serialize};

/// The build a daemon runs, as `void version` prints it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Build {
    pub version: String,
    pub commit: String,
    pub commit_date: String,
}

impl std::fmt::Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}, {})", self.version, self.commit, self.commit_date)
    }
}

/// The daemon itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Daemon {
    pub running: bool,
    pub pid: u32,
    #[serde(flatten)]
    pub build: Build,
    pub socket: String,
    pub state_dir: String,
    /// RFC 3339.
    pub started: String,
    pub uptime_secs: u64,
    /// The server it publishes to, and the key it signs with.
    pub endpoint: String,
    pub access_key_id: String,
}

/// Whether the server can be reached, from what the daemon's requests get.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Link {
    Online,
    /// The server answers, but with errors or slowly.
    Degraded,
    /// Requests get no answer.
    Offline,
}

impl From<voidfs_client::Link> for Link {
    fn from(l: voidfs_client::Link) -> Link {
        match l {
            voidfs_client::Link::Online => Link::Online,
            voidfs_client::Link::Degraded => Link::Degraded,
            voidfs_client::Link::Offline => Link::Offline,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub endpoint: String,
    pub link: Link,
}

/// The upload queue, counted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Uploads {
    pub uploading: u64,
    pub queued: u64,
    /// Waiting for a resume after a failure the queue doesn't retry by itself.
    pub failed: u64,
    /// Not finished and held by a pause, of the queue, a drive, a batch or the entry.
    pub paused_items: u64,
    /// Entries not yet published, and the bytes they have left to send.
    pub unpublished: u64,
    pub unpublished_bytes: u64,
    /// Everything is paused.
    pub paused: bool,
    pub paused_drives: Vec<String>,
    /// Bytes a second; `None` is unlimited.
    pub bandwidth: Option<u64>,
    /// Bytes a second the uploads went at, over the last few seconds.
    #[serde(default)]
    pub rate: u64,
}

impl Uploads {
    pub fn of(s: &voidfs_client::Status) -> Uploads {
        use voidfs_client::State;
        let open = || s.items.iter().filter(|i| !i.state.finished());
        let count = |state: State| open().filter(|i| i.state == state).count() as u64;
        Uploads {
            uploading: count(State::Uploading),
            queued: count(State::Queued),
            failed: count(State::Failed),
            paused_items: open().filter(|i| i.paused).count() as u64,
            unpublished: s.unpublished,
            unpublished_bytes: s.unpublished_bytes,
            paused: s.paused,
            paused_drives: s.paused_drives.clone(),
            bandwidth: s.bandwidth,
            rate: 0,
        }
    }
}

/// The block cache.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cache {
    pub disk_bytes: u64,
    pub max_bytes: u64,
    pub pinned_bytes: u64,
    pub memory_bytes: u64,
    pub memory_max_bytes: u64,
}

/// `GET /v1/status`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub daemon: Daemon,
    pub connection: Connection,
    pub uploads: Uploads,
    pub cache: Cache,
}

/// The journal, for a restart.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    /// Changes not yet published, and the bytes they have left to send. All of it is on disk.
    pub unpublished: u64,
    pub unpublished_bytes: u64,
    /// Being sent now: after a restart a put is sent again, and a multipart upload goes on from
    /// the parts the server has.
    pub uploading: u64,
    /// Imports not yet published, read from where they are when they go up.
    pub imports: u64,
}

/// `GET /v1/info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    #[serde(flatten)]
    pub build: Build,
    pub pid: u32,
    pub uptime_secs: u64,
    pub socket: String,
    pub state_dir: String,
    /// Nothing would be lost by restarting now: no change is held only in memory.
    pub restart_safe: bool,
    /// Bytes of changes held only in memory. Every change the journal takes is on disk before
    /// its call returns; step 5's mount may hold writes before it journals them.
    pub memory_only_bytes: u64,
    pub journal: Journal,
}

impl Journal {
    pub fn of(s: &voidfs_client::Status) -> Journal {
        let open = || s.items.iter().filter(|i| !i.state.finished());
        Journal {
            unpublished: s.unpublished,
            unpublished_bytes: s.unpublished_bytes,
            uploading: open().filter(|i| i.state == voidfs_client::State::Uploading).count() as u64,
            imports: open().filter(|i| i.batch.is_some()).count() as u64,
        }
    }
}

/// A file or folder to upload: where it is, by absolute path, and the key it goes to. A folder
/// is made, with its attributes; what is in it is named by files of its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewFile {
    pub path: String,
    pub key: String,
}

/// `POST /v1/uploads`: one batch, into one drive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewBatch {
    pub label: String,
    /// By alias or id.
    pub drive: String,
    pub files: Vec<NewFile>,
}

/// What `POST /v1/uploads` queued.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queued {
    pub batch: i64,
    /// The drive's alias, as the server names it.
    pub drive: String,
    pub items: u64,
    pub bytes: u64,
}

/// `GET /v1/uploads`: the queue's items and batches, with how fast it goes. `unpublished` and
/// the rate are the whole queue's, whatever the filter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadList {
    #[serde(flatten)]
    pub queue: voidfs_client::Status,
    /// Bytes a second, over the last few seconds.
    pub rate: u64,
    /// Seconds until what is unpublished is sent, at that rate.
    pub eta_secs: Option<u64>,
}

/// What a pause, a resume or a cancel applies to: `"all"`, `{"drive": …}`, `{"batch": …}` or
/// `{"entry": …}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    All,
    Drive(String),
    Batch(i64),
    Entry(i64),
}

/// How many unfinished uploads a pause, a resume or a cancel applied to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Affected {
    pub items: u64,
}

/// `PUT /v1/uploads/limit`, and its answer: bytes a second, `None` (or 0) for unlimited.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limit {
    pub bytes_per_second: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cleared {
    pub cleared: u64,
}

/// `POST /v1/stop`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopping {
    pub stopping: bool,
    pub pid: u32,
}

/// The `error` member of an error's body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}
