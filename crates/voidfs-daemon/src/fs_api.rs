// SPDX-License-Identifier: Apache-2.0
//! Bounded filesystem calls and metadata invalidations on the daemon's Unix socket.

use serde::{Deserialize, Serialize};
use voidfs_client::mount::{Attr, Capabilities};

pub const VERSION: u32 = 1;
pub const MAX_IO: u64 = 8 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 256;
pub const MAX_HANDLES: usize = 1024;
pub const MAX_SESSIONS: usize = 256;
pub const MAX_CALLS: usize = 32;
pub const MAX_WATCHES: usize = 64;
pub const MAX_JSON: usize = 64 * 1024;
pub const MAX_RESPONSE: usize = 1024 * 1024;
pub const EVENT_CAPACITY: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewSession { pub version: u32, pub drive: String, pub read_only: bool }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub version: u32,
    pub id: String,
    pub drive: String,
    pub root: u64,
    pub generation: u32,
    pub metadata_generation: u64,
    pub read_only: bool,
    pub max_io: u64,
    pub max_entries: usize,
    /// What the drive's core supports, for the adapter to advertise. Added within version 1:
    /// earlier readers ignore it, and this crate's client requires it.
    pub capabilities: Capabilities,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lookup { pub parent: u64, pub name: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inode { pub ino: u64 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadDir { pub ino: u64, pub after: Option<String>, pub limit: usize }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Open { pub ino: u64, pub write: bool }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened { pub fh: u64, pub attr: Attr }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Handle { pub fh: u64 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Truncate { pub fh: u64, pub size: u64 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Written { pub written: usize }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Empty {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadDirReply { pub entries: Vec<(String, Attr)>, pub generation: u64 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link { pub target: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "key", rename_all = "camelCase")]
pub enum Invalidation { Object(String), Subtree(String), All }

impl From<voidfs_client::Invalidation> for Invalidation {
    fn from(value: voidfs_client::Invalidation) -> Self {
        match value {
            voidfs_client::Invalidation::Object(key) => Self::Object(key),
            voidfs_client::Invalidation::Subtree(key) => Self::Subtree(key),
            voidfs_client::Invalidation::All => Self::All,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidationEvent {
    pub generation: u64,
    pub seq: u64,
    pub resync: bool,
    pub invalidations: Vec<Invalidation>,
    pub inodes: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsError { pub code: String, pub message: String, pub errno: i32 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsErrorBody { pub error: FsError }
