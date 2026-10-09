// SPDX-License-Identifier: Apache-2.0
//! What the calls take and give back. Options are `Default`, so build them with
//! `..Default::default()`; they gain fields over time. Timestamps stay the strings the server sent
//! (RFC 3339 with microseconds), so that they can be passed back unchanged as `as_of` (§4.4).

use std::collections::BTreeMap;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// What an object is (protocol §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Folder,
    Symlink,
    /// A kind this SDK does not know yet.
    #[serde(other)]
    Unknown,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Folder => "folder",
            Kind::Symlink => "symlink",
            Kind::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(s: &str) -> Kind {
        match s {
            "file" => Kind::File,
            "folder" => Kind::Folder,
            "symlink" => Kind::Symlink,
            _ => Kind::Unknown,
        }
    }
}

/// Conditions a mutation applies only under (protocol §4.0). Of two racing guarded writers,
/// exactly one succeeds; the other gets `412` with the current version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preconditions {
    /// The object's current version must be this one.
    pub if_version: Option<String>,
    /// The object's current ETag must be this one.
    pub if_match: Option<String>,
}

impl Preconditions {
    pub fn if_version(v: impl Into<String>) -> Preconditions {
        Preconditions { if_version: Some(v.into()), if_match: None }
    }

    pub fn is_empty(&self) -> bool {
        self.if_version.is_none() && self.if_match.is_none()
    }
}

#[derive(Clone, Debug, Default)]
pub struct PutOptions {
    pub content_type: Option<String>,
    pub if_version: Option<String>,
    pub if_match: Option<String>,
    /// `If-None-Match: *`: only if nothing is at the key.
    pub if_none_match_any: bool,
    /// The modification time to record, RFC 3339. Default: the commit time.
    pub mtime: Option<String>,
    /// POSIX permission bits, for example `0o644`.
    pub mode: Option<u32>,
    /// User metadata, sent as `x-amz-meta-<name>`.
    pub metadata: BTreeMap<String, String>,
}

/// Options of the offset write (§4.1) and the patch (§4.2).
#[derive(Clone, Debug, Default)]
pub struct WriteOptions {
    /// The object's size afterwards: larger extends it with zeros, smaller truncates it.
    pub size: Option<u64>,
    pub if_version: Option<String>,
    pub if_match: Option<String>,
    pub mtime: Option<String>,
    pub mode: Option<u32>,
    /// User metadata entries the new version sets, sent as `x-amz-meta-<name>`; the others keep
    /// their values. A server from before protocol revision 9 ignores them.
    pub metadata: BTreeMap<String, String>,
}

/// Which version a read serves: the current one by default, or `version_id`, or the one current
/// at `as_of` (§4.5). Not both.
#[derive(Clone, Debug, Default)]
pub struct ReadOptions {
    pub version_id: Option<String>,
    /// RFC 3339; pass a `last_modified` from the history unchanged.
    pub as_of: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RenameOptions {
    /// Let a file replace an existing file at the destination (POSIX `rename`), the atomic save.
    pub replace: bool,
    /// Preconditions on the source.
    pub if_version: Option<String>,
    pub if_match: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CreateDrive {
    /// A human name. Default: the alias.
    pub display_name: Option<String>,
}

/// One edit of a patch (§4.2): `data` written at `offset`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub offset: u64,
    pub data: Bytes,
}

impl Edit {
    pub fn new(offset: u64, data: impl Into<Bytes>) -> Edit {
        Edit { offset, data: data.into() }
    }
}

/// What a mutation answers: the version it made, and the object's ETag and size afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub version_id: String,
    pub etag: Option<String>,
    pub size: Option<u64>,
}

/// A part of an open multipart upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadedPart {
    pub number: u32,
    pub etag: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreResult {
    /// The new current version.
    pub version_id: String,
    /// The version it copies, for a file.
    pub restored_from: Option<String>,
    pub etag: Option<String>,
    pub size: Option<u64>,
}

/// An object's headers, from GET or HEAD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectMeta {
    pub version_id: String,
    pub etag: String,
    /// The whole object's size, also for a range.
    pub size: u64,
    pub kind: Kind,
    pub object_id: Option<String>,
    pub content_type: Option<String>,
    /// HTTP date of the version.
    pub last_modified: Option<String>,
    /// RFC 3339.
    pub mtime: Option<String>,
    /// Octal, for example `0644`.
    pub mode: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Object {
    pub meta: ObjectMeta,
    pub body: Bytes,
}

/// A drive as `ListBuckets` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DriveSummary {
    pub name: String,
    pub created: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateDriveResult {
    pub drive_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkResult {
    pub drive_id: String,
    pub source_id: Option<String>,
    pub fork_point: Option<String>,
}

/// `?x-voidfs-drive` (§5.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub drive_id: String,
    pub alias: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub fork_of: Option<ForkOf>,
    #[serde(default)]
    pub forks: Vec<String>,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub usage_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkOf {
    pub drive_id: String,
    #[serde(default)]
    pub fork_point: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

/// `?x-voidfs-credentials` (§5.5): read-only storage credentials for one drive.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCredentials {
    pub drive_id: String,
    pub access_generation: u64,
    pub storage: StorageLocation,
    #[serde(default)]
    pub storage_budget: Option<StorageBudget>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageLocation {
    pub backend: String,
    pub bucket: String,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub force_path_style: bool,
    #[serde(default)]
    pub readable: Vec<String>,
    pub credentials: TemporaryCredentials,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemporaryCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default)]
    pub session_token: Option<String>,
    pub expires_at: String,
}

impl std::fmt::Debug for TemporaryCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemporaryCredentials").field("access_key_id", &self.access_key_id).field("expires_at", &self.expires_at).finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageBudget {
    #[serde(default)]
    pub limit_bytes: Option<u64>,
    #[serde(default)]
    pub used_bytes: Option<u64>,
}

/// One version in an object's history (§4.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub version_id: String,
    pub is_latest: bool,
    pub size: u64,
    pub etag: String,
    /// With microseconds; pass it unchanged as `as_of`.
    pub last_modified: String,
    /// `put`, `write`, `copy`, `restore`, `rename`, `attrs` or `other`.
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<String>,
}

/// `?x-voidfs-attrs` (§4.8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attributes {
    pub object_id: String,
    pub kind: Kind,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub mtime: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    /// Extended attributes, their values base64.
    #[serde(default)]
    pub xattrs: BTreeMap<String, String>,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

/// A change of attributes, as one `attrs` version (§4.8). What is left `None` or empty stays.
#[derive(Clone, Debug, Default)]
pub struct AttributesUpdate {
    pub mtime: Option<String>,
    pub mode: Option<u32>,
    /// Extended attributes to set, with their raw values.
    pub set_xattrs: BTreeMap<String, Bytes>,
    pub remove_xattrs: Vec<String>,
    pub flags: Option<Vec<String>>,
    pub content_type: Option<String>,
}

/// One page of a folder's children with attributes (§4.9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderPage {
    pub prefix: String,
    /// The drive position the page reflects: resume the change feed from it.
    pub seq: u64,
    pub entries: Vec<FolderEntry>,
    #[serde(default)]
    pub next_continuation_token: Option<String>,
}

/// A folder's children, every page (§4.9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderListing {
    pub prefix: String,
    /// The first page's position, so that resuming the change feed from it misses nothing that
    /// changed while the later pages were read.
    pub seq: u64,
    pub entries: Vec<FolderEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    /// The child's name; a folder's ends in `/`.
    pub name: String,
    pub kind: Kind,
    pub object_id: String,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default)]
    pub mtime: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub has_xattrs: bool,
    /// A symbolic link's target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// One page of objects deleted and still retained (§4.10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedPage {
    pub deleted: Vec<DeletedEntry>,
    #[serde(default)]
    pub next_continuation_token: Option<String>,
}

/// An object deleted and still retained (§4.10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedEntry {
    pub key: String,
    pub object_id: String,
    pub deleted_at: String,
    /// Restore this version at the key to bring the object back.
    pub last_version_id: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
}

/// A long poll of the change feed (§5.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    /// The last position included: the next `since`.
    pub seq: u64,
    pub changes: Vec<Change>,
    /// Whether more changes are waiting.
    #[serde(default)]
    pub more: bool,
}

/// The changes of one drive position, as the event stream delivers them (§5.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeBatch {
    pub seq: u64,
    #[serde(default)]
    pub time: Option<String>,
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// The version's operation (§4.4), `create` for a folder a write made, or `delete`.
    pub op: String,
    pub key: String,
    /// For a rename, where it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_key: Option<String>,
    pub object_id: String,
    pub version_id: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
}

/// An object as `ListObjectsV2` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectSummary {
    pub key: String,
    pub size: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// A listing, every page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ListResult {
    pub objects: Vec<ObjectSummary>,
    pub common_prefixes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ListOptions {
    pub prefix: Option<String>,
    /// Only `/` is supported (protocol §3).
    pub delimiter: Option<String>,
}
