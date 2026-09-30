// SPDX-License-Identifier: Apache-2.0
//! The JSON objects of the on-bucket format (format §3, §5–§8), as serde types.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{DriveId, ObjectId, ShardHash, Timestamp, VersionId};

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------------------------------------
// Content (format §5)

/// The most bytes the data extents of one content descriptor may hold (format §5).
pub const MAX_DATA_BYTES: usize = 4096;

/// One run of content: a whole shard, zeros that are not stored, or bytes held in the descriptor
/// itself (a data extent, format §5).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Extent {
    Shard { s: ShardHash, n: u64 },
    Zero { z: u64 },
    Data {
        #[serde(with = "data")]
        d: Bytes,
    },
}

impl Extent {
    pub fn len(&self) -> u64 {
        match self {
            Extent::Shard { n, .. } => *n,
            Extent::Zero { z } => *z,
            Extent::Data { d } => d.len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn shard(&self) -> Option<ShardHash> {
        match self {
            Extent::Shard { s, .. } => Some(*s),
            Extent::Zero { .. } | Extent::Data { .. } => None,
        }
    }
}

/// How many bytes the data extents among `extents` hold.
pub fn data_len(extents: &[Extent]) -> usize {
    extents.iter().map(|e| if let Extent::Data { d } = e { d.len() } else { 0 }).sum()
}

/// A data extent in JSON: standard base64 with padding, of at least one byte (format §5).
mod data {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use bytes::Bytes;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(d))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Bytes, D::Error> {
        let text = std::borrow::Cow::<str>::deserialize(de)?;
        let d = STANDARD.decode(text.as_bytes()).map_err(D::Error::custom)?;
        if d.is_empty() || d.len() > super::MAX_DATA_BYTES {
            return Err(D::Error::custom(format!("a data extent holds 1 to {} bytes, not {}", super::MAX_DATA_BYTES, d.len())));
        }
        Ok(Bytes::from(d))
    }
}

/// How a file version's content is described: inline extents, or a manifest tree.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(untagged, try_from = "Described")]
pub enum ContentDescriptor {
    Inline { extents: Vec<Extent> },
    Tree { root: ShardHash, size: u64 },
}

/// A content descriptor as parsed, before its data extents are checked against the limit.
#[derive(Deserialize)]
#[serde(untagged)]
enum Described {
    Inline { extents: Vec<Extent> },
    Tree { root: ShardHash, size: u64 },
}

impl TryFrom<Described> for ContentDescriptor {
    type Error = String;

    fn try_from(d: Described) -> Result<Self, String> {
        match d {
            Described::Inline { extents } => {
                let n = data_len(&extents);
                if n > MAX_DATA_BYTES {
                    return Err(format!("the data extents of a content descriptor hold {n} bytes, more than {MAX_DATA_BYTES}"));
                }
                Ok(ContentDescriptor::Inline { extents })
            }
            Described::Tree { root, size } => Ok(ContentDescriptor::Tree { root, size }),
        }
    }
}

impl ContentDescriptor {
    pub fn empty() -> Self {
        ContentDescriptor::Inline { extents: Vec::new() }
    }

    pub fn size(&self) -> u64 {
        match self {
            ContentDescriptor::Inline { extents } => extents.iter().map(Extent::len).sum(),
            ContentDescriptor::Tree { size, .. } => *size,
        }
    }

    /// Whether it holds any data extents (format §5), which checkpoints never carry (§8.2).
    pub fn has_data(&self) -> bool {
        matches!(self, ContentDescriptor::Inline { extents } if extents.iter().any(|e| matches!(e, Extent::Data { .. })))
    }

    /// A strong validator derived from the content description (format §7.7): identical
    /// descriptions give identical ETags, and any change to the bytes changes it. A data extent
    /// counts as the shard extent holding the same bytes, so where the bytes live never shows.
    pub fn etag(&self) -> String {
        let mut h = Sha256::new();
        match self {
            ContentDescriptor::Inline { extents } => {
                h.update(b"voidfs-inline\0");
                for e in extents {
                    match e {
                        Extent::Shard { s, n } => {
                            h.update(*b"s");
                            h.update(s.0);
                            h.update(n.to_be_bytes());
                        }
                        Extent::Zero { z } => {
                            h.update(*b"z");
                            h.update(z.to_be_bytes());
                        }
                        Extent::Data { d } => {
                            h.update(*b"s");
                            h.update(Sha256::digest(d));
                            h.update((d.len() as u64).to_be_bytes());
                        }
                    }
                }
            }
            ContentDescriptor::Tree { root, size } => {
                h.update(b"voidfs-tree\0");
                h.update(root.0);
                h.update(size.to_be_bytes());
            }
        }
        format!("\"{}\"", hex::encode(h.finalize()))
    }
}

/// A manifest-tree page (format §5.1).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "Page")]
pub enum ManifestPage {
    Leaf { extents: Vec<Extent> },
    Node { children: Vec<PageRef> },
}

/// A manifest page as parsed, before its extents are checked.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Page {
    Leaf { extents: Vec<Extent> },
    Node { children: Vec<PageRef> },
}

impl TryFrom<Page> for ManifestPage {
    type Error = &'static str;

    fn try_from(p: Page) -> Result<Self, Self::Error> {
        match p {
            Page::Leaf { extents } if data_len(&extents) > 0 => Err("a manifest page holds a data extent"),
            Page::Leaf { extents } => Ok(ManifestPage::Leaf { extents }),
            Page::Node { children } => Ok(ManifestPage::Node { children }),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PageRef {
    pub page: ShardHash,
    pub size: u64,
}

// ---------------------------------------------------------------------------------------------
// Descriptors (format §3, §6)

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Features {
    #[serde(default)]
    pub compatible: Vec<String>,
    #[serde(default)]
    pub incompatible: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Chunking {
    pub algorithm: String,
    pub min: u32,
    pub avg: u32,
    pub max: u32,
}

impl Default for Chunking {
    fn default() -> Self {
        Chunking { algorithm: "fastcdc-2020".into(), min: 262_144, avg: 2_097_152, max: 16_777_216 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommitGuard {
    CreateIfAbsent,
    External,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PoolDescriptor {
    pub format: u32,
    pub pool_id: String,
    pub created: Timestamp,
    pub features: Features,
    pub chunking: Chunking,
    pub hash: String,
    pub commit_guard: CommitGuard,
}

/// The incompatible features this implementation understands (none yet).
pub const KNOWN_INCOMPATIBLE_FEATURES: &[&str] = &[];

impl PoolDescriptor {
    /// Refuse pools this reader cannot read correctly (format §3, §3.1).
    pub fn check_readable(&self) -> Result<(), String> {
        if self.format != crate::FORMAT_VERSION {
            return Err(format!("pool format {} is not supported", self.format));
        }
        if self.hash != "sha256" {
            return Err(format!("pool hash {:?} is not supported", self.hash));
        }
        let unknown: Vec<_> = self
            .features
            .incompatible
            .iter()
            .filter(|f| !KNOWN_INCOMPATIBLE_FEATURES.contains(&f.as_str()))
            .collect();
        if unknown.is_empty() { Ok(()) } else { Err(format!("pool uses unsupported features {unknown:?}")) }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ForkOf {
    pub drive_id: DriveId,
    pub seq: u64,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DriveDescriptor {
    pub format: u32,
    pub drive_id: DriveId,
    pub created: Timestamp,
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_of: Option<ForkOf>,
}

// ---------------------------------------------------------------------------------------------
// Objects (format §7.7)

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Folder,
    Symlink,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Attrs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// Name to base64 value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub xattrs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

/// Total decoded size of extended attributes allowed per object (protocol §7).
pub const MAX_XATTR_BYTES: usize = 64 * 1024;

/// The current record of an object (format §7.7), plus the version that is its head.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ObjectRecord {
    pub oid: ObjectId,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ContentDescriptor>,
    pub size: u64,
    pub etag: String,
    #[serde(default)]
    pub attrs: Attrs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<VersionId>,
    pub head: VersionId,
    /// When the head version was committed.
    pub time: Timestamp,
}

// ---------------------------------------------------------------------------------------------
// Commits (format §7)

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Put,
    Write,
    Copy,
    Restore,
    Rename,
    Attrs,
    Delete,
    Other,
}

impl Op {
    pub fn as_str(&self) -> &'static str {
        match self {
            Op::Put => "put",
            Op::Write => "write",
            Op::Copy => "copy",
            Op::Restore => "restore",
            Op::Rename => "rename",
            Op::Attrs => "attrs",
            Op::Delete => "delete",
            Op::Other => "other",
        }
    }

    /// Versions that change content; the default history listing shows only these
    /// (protocol §4.4).
    pub fn follows_content(&self) -> bool {
        !matches!(self, Op::Rename | Op::Attrs | Op::Delete)
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Actor {
    pub kind: String,
    pub id: String,
}

impl Actor {
    pub fn system() -> Self {
        Actor { kind: "system".into(), id: "voidfs".into() }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CreateChange {
    pub oid: ObjectId,
    pub parent: ObjectId,
    pub name: String,
    pub kind: Kind,
}

/// Replaces the given members of an object's record; absent members keep their values.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SetChange {
    pub oid: ObjectId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ContentDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Attrs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<VersionId>,
}

impl SetChange {
    pub fn new(oid: ObjectId) -> Self {
        SetChange { oid, content: None, size: None, etag: None, attrs: None, target: None, restored_from: None }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MoveChange {
    pub oid: ObjectId,
    pub parent: ObjectId,
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RemoveChange {
    pub oid: ObjectId,
    #[serde(default, skip_serializing_if = "is_false")]
    pub recursive: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // Changes are short-lived and few per transaction.
pub enum Change {
    Create(CreateChange),
    Set(SetChange),
    Move(MoveChange),
    Remove(RemoveChange),
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Txn {
    pub target: ObjectId,
    pub op: Op,
    pub actor: Actor,
    pub changes: Vec<Change>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Commit {
    pub format: u32,
    pub seq: u64,
    pub time: Timestamp,
    pub authority: String,
    pub txns: Vec<Txn>,
}

// ---------------------------------------------------------------------------------------------
// Checkpoint rows (format §8.2)

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HistoryRow {
    pub oid: ObjectId,
    pub version: VersionId,
    pub time: Timestamp,
    pub op: Op,
    pub size: u64,
    pub etag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ContentDescriptor>,
    #[serde(default)]
    pub attrs: Attrs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<VersionId>,
    pub actor: Actor,
}

/// A namespace entry (format §8.2 `entries` table).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EntryRow {
    pub parent: ObjectId,
    pub name: String,
    pub oid: ObjectId,
    pub kind: Kind,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RemovedRow {
    pub key: String,
    pub oid: ObjectId,
    /// The delete version.
    pub version: VersionId,
    /// The object's last version before it was deleted.
    pub last_version: VersionId,
    pub time: Timestamp,
    pub kind: Kind,
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extents_use_the_compact_spec_encoding() {
        let h = ShardHash::of(b"x");
        let d = ContentDescriptor::Inline { extents: vec![Extent::Shard { s: h, n: 1 }, Extent::Zero { z: 4096 }] };
        let j = serde_json::to_string(&d).unwrap();
        assert_eq!(j, format!(r#"{{"extents":[{{"s":"{h}","n":1}},{{"z":4096}}]}}"#));
        assert_eq!(serde_json::from_str::<ContentDescriptor>(&j).unwrap(), d);
        let t = ContentDescriptor::Tree { root: h, size: 10 };
        assert_eq!(serde_json::from_str::<ContentDescriptor>(&serde_json::to_string(&t).unwrap()).unwrap(), t);
    }

    #[test]
    fn data_extents_are_standard_base64() {
        let d = ContentDescriptor::Inline { extents: vec![Extent::Data { d: Bytes::from_static(b"hello, world\n") }, Extent::Zero { z: 2 }] };
        let j = serde_json::to_string(&d).unwrap();
        assert_eq!(j, r#"{"extents":[{"d":"aGVsbG8sIHdvcmxkCg=="},{"z":2}]}"#);
        assert_eq!(serde_json::from_str::<ContentDescriptor>(&j).unwrap(), d);
        assert_eq!(d.size(), 15);
        assert!(d.has_data());
        assert_eq!(d.clone(), serde_json::from_value(serde_json::to_value(&d).unwrap()).unwrap(), "through a value, as in a commit");
    }

    /// Readers reject a descriptor with an invalid data extent (format §5): not base64, empty,
    /// or over 4,096 bytes in all.
    #[test]
    fn invalid_data_extents_are_refused() {
        let parse = |j: &str| serde_json::from_str::<ContentDescriptor>(j);
        let d = |n: usize| format!(r#"{{"d":"{}"}}"#, base64::Engine::encode(&base64::engine::general_purpose::STANDARD, vec![7u8; n]));
        assert!(parse(&format!(r#"{{"extents":[{}]}}"#, d(MAX_DATA_BYTES))).is_ok());
        assert!(parse(&format!(r#"{{"extents":[{},{{"z":1}},{}]}}"#, d(2048), d(2048))).is_ok());
        for bad in [
            r#"{"extents":[{"d":""}]}"#.to_owned(),
            r#"{"extents":[{"d":"aGk"}]}"#.to_owned(),
            r#"{"extents":[{"d":"aGk=\n"}]}"#.to_owned(),
            r#"{"extents":[{"d":"a-_k"}]}"#.to_owned(),
            r#"{"extents":[{"d":7}]}"#.to_owned(),
            format!(r#"{{"extents":[{}]}}"#, d(MAX_DATA_BYTES + 1)),
            format!(r#"{{"extents":[{},{}]}}"#, d(2048), d(2049)),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
        // Wherever a descriptor is parsed: a commit's change, and a checkpoint's rows.
        let change = format!(r#"{{"set":{{"oid":"root","content":{{"extents":[{},{}]}}}}}}"#, d(4000), d(97));
        assert!(serde_json::from_str::<Change>(&change).is_err());
        assert!(serde_json::from_str::<Change>(&change.replace(&d(97), &d(96))).is_ok());
        // Never in a manifest page.
        let h = ShardHash::of(b"x");
        assert!(serde_json::from_str::<ManifestPage>(&format!(r#"{{"kind":"leaf","extents":[{{"s":"{h}","n":1}}]}}"#)).is_ok());
        assert!(serde_json::from_str::<ManifestPage>(&format!(r#"{{"kind":"leaf","extents":[{{"s":"{h}","n":1}},{}]}}"#, d(1))).is_err());
    }

    /// A data extent contributes to the ETag what the shard extent with the same bytes does
    /// (format §7.7). The value is pinned: a change to it would change every ETag.
    #[test]
    fn a_data_extent_has_the_etag_of_the_same_bytes_in_a_shard() {
        let bytes = Bytes::from_static(b"hello, world\n");
        let data = ContentDescriptor::Inline { extents: vec![Extent::Data { d: bytes.clone() }] };
        let shard = ContentDescriptor::Inline { extents: vec![Extent::Shard { s: ShardHash::of(&bytes), n: 13 }] };
        assert_eq!(data.etag(), shard.etag());
        // sha256("voidfs-inline\0" || "s" || sha256(bytes) || u64be(13)), computed apart from this code.
        assert_eq!(data.etag(), "\"6b71c1b094b82c72ac0e6fecdd8f05fe2ef1b7a3299ba3038000581335fe8f01\"");
        let mixed = |e: Extent| ContentDescriptor::Inline { extents: vec![Extent::Zero { z: 3 }, e, Extent::Shard { s: ShardHash::of(b"x"), n: 1 }] };
        assert_eq!(mixed(Extent::Data { d: bytes.clone() }).etag(), mixed(Extent::Shard { s: ShardHash::of(&bytes), n: 13 }).etag());
        assert_ne!(data.etag(), ContentDescriptor::Inline { extents: vec![Extent::Data { d: Bytes::from_static(b"hello, world!") }] }.etag());
    }

    #[test]
    fn changes_are_externally_tagged() {
        let c = Change::Remove(RemoveChange { oid: ObjectId::root(), recursive: false });
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"remove":{"oid":"root"}}"#);
        let page = ManifestPage::Leaf { extents: vec![Extent::Zero { z: 1 }] };
        assert_eq!(serde_json::to_string(&page).unwrap(), r#"{"kind":"leaf","extents":[{"z":1}]}"#);
    }

    #[test]
    fn etags_follow_content_not_form() {
        let a = ContentDescriptor::Inline { extents: vec![Extent::Zero { z: 3 }] };
        let b = ContentDescriptor::Inline { extents: vec![Extent::Zero { z: 4 }] };
        assert_eq!(a.etag(), a.clone().etag());
        assert_ne!(a.etag(), b.etag());
        assert!(a.etag().starts_with('"') && a.etag().ends_with('"'));
    }

    #[test]
    fn pools_with_unknown_incompatible_features_are_refused() {
        let mut p = PoolDescriptor {
            format: 1,
            pool_id: "p-1".into(),
            created: Timestamp::now(),
            features: Features { compatible: vec!["future-hint".into()], incompatible: vec![] },
            chunking: Chunking::default(),
            hash: "sha256".into(),
            commit_guard: CommitGuard::CreateIfAbsent,
        };
        assert!(p.check_readable().is_ok());
        p.features.incompatible.push("encryption".into());
        assert!(p.check_readable().is_err());
    }
}
