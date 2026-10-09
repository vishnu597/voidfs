// SPDX-License-Identifier: Apache-2.0
//! Bounded filesystem calls and metadata invalidations on the daemon's Unix socket.

use serde::{Deserialize, Serialize};
use voidfs_client::mount::{Attr, Capabilities, Conflict, ConflictSide, RenameMode, XattrMode};

pub const VERSION: u32 = 1;
pub const MAX_IO: u64 = 8 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 256;
pub const MAX_HANDLES: usize = 1024;
pub const MAX_SESSIONS: usize = 256;
pub const MAX_CALLS: usize = 32;
pub const MAX_WATCHES: usize = 64;
pub const MAX_JSON: usize = 64 * 1024;
pub const MAX_RESPONSE: usize = 1024 * 1024;
/// An object's extended attribute names and values together, as `Capabilities::max_xattr_bytes` says.
pub const MAX_XATTR: usize = 64 * 1024;
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

/// `create` and `mkdir`: an exclusive new name with permission bits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Create { pub parent: u64, pub name: String, pub mode: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenameHow { Replace, Exclusive, Swap }

impl From<RenameHow> for RenameMode {
    fn from(how: RenameHow) -> Self {
        match how { RenameHow::Replace => Self::Replace, RenameHow::Exclusive => Self::Exclusive, RenameHow::Swap => Self::Swap }
    }
}

impl From<RenameMode> for RenameHow {
    fn from(how: RenameMode) -> Self {
        match how { RenameMode::Replace => Self::Replace, RenameMode::Exclusive => Self::Exclusive, RenameMode::Swap => Self::Swap }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rename { pub from_parent: u64, pub from_name: String, pub to_parent: u64, pub to_name: String, pub how: RenameHow }

/// `link` and `clone_file`: a second name for `ino`. Both are refused (`Capabilities`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkTo { pub ino: u64, pub parent: u64, pub name: String }

/// Absent or `null` leaves a value as it is. `mtime` is RFC 3339, as `Attr` spells it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAttr { pub ino: u64, #[serde(default)] pub mode: Option<u32>, #[serde(default)] pub mtime: Option<String> }

/// `removexattr` as JSON; `getxattr` as the query of a binary answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Xattr { pub ino: u64, pub name: String }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum XattrHow { Set, Create, Replace }

impl From<XattrHow> for XattrMode {
    fn from(how: XattrHow) -> Self {
        match how { XattrHow::Set => Self::Set, XattrHow::Create => Self::Create, XattrHow::Replace => Self::Replace }
    }
}

impl From<XattrMode> for XattrHow {
    fn from(how: XattrMode) -> Self {
        match how { XattrMode::Set => Self::Set, XattrMode::Create => Self::Create, XattrMode::Replace => Self::Replace }
    }
}

/// The query of `PUT …/setxattr`, whose body is the raw value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetXattr { pub ino: u64, pub name: String, pub how: XattrHow }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XattrNames { pub names: Vec<String> }

/// The retained versions of a conflicted save, or `null`. `Conflict` keeps its Rust serde shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictReply { pub conflict: Option<Conflict> }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Side { Local, Remote }

impl From<Side> for ConflictSide {
    fn from(side: Side) -> Self { match side { Side::Local => Self::Local, Side::Remote => Self::Remote } }
}

impl From<ConflictSide> for Side {
    fn from(side: ConflictSide) -> Self { match side { ConflictSide::Local => Self::Local, ConflictSide::Remote => Self::Remote } }
}

/// The query of `GET …/read_conflict`, whose answer is raw bytes of one retained version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadConflict { pub ino: u64, pub side: Side, pub offset: u64, pub length: u64 }

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
