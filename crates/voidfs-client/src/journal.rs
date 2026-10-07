// SPDX-License-Identifier: Apache-2.0
//! The write journal's entries (step 4, item 3): local changes, durable on the Mac before they
//! are published, kept in the state database with their bytes in files beside it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

pub type EntryId = i64;
pub type BatchId = i64;

/// What a change does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    /// The whole file, from staged bytes, an import, or a new empty mount file.
    Put,
    /// Bytes at an offset.
    Write,
    /// A new size.
    Truncate,
    Rename,
    Delete,
    /// A folder, made.
    Folder,
    /// Attributes only.
    Attrs,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Put => "put",
            Op::Write => "write",
            Op::Truncate => "truncate",
            Op::Rename => "rename",
            Op::Delete => "delete",
            Op::Folder => "folder",
            Op::Attrs => "attrs",
        }
    }

    fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "put" => Op::Put,
            "write" => Op::Write,
            "truncate" => Op::Truncate,
            "rename" => Op::Rename,
            "delete" => Op::Delete,
            "folder" => Op::Folder,
            "attrs" => Op::Attrs,
            _ => return None,
        })
    }
}

/// What the caller's change was based on, which its publish is guarded by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Base {
    /// Nothing: published unguarded.
    Any,
    /// Nothing was at the key: a new file.
    Absent,
    /// The object as it was at this version.
    Version(String),
}

/// A base as stored: a change made on top of another change not yet published is based on
/// whatever version that one gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StoredBase {
    Any,
    Absent,
    Version(String),
    Entry(EntryId),
}

impl StoredBase {
    fn encode(&self) -> String {
        match self {
            StoredBase::Any => "any".into(),
            StoredBase::Absent => "absent".into(),
            StoredBase::Version(v) => format!("v:{v}"),
            StoredBase::Entry(e) => format!("e:{e}"),
        }
    }

    fn decode(s: &str) -> StoredBase {
        if let Some(v) = s.strip_prefix("v:") {
            StoredBase::Version(v.to_owned())
        } else if let Some(e) = s.strip_prefix("e:").and_then(|e| e.parse().ok()) {
            StoredBase::Entry(e)
        } else if s == "absent" {
            StoredBase::Absent
        } else {
            StoredBase::Any
        }
    }
}

impl From<Base> for StoredBase {
    fn from(b: Base) -> StoredBase {
        match b {
            Base::Any => StoredBase::Any,
            Base::Absent => StoredBase::Absent,
            Base::Version(v) => StoredBase::Version(v),
        }
    }
}

/// Attributes a change carries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attrs {
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Extended attributes to set, with their raw values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub xattrs: BTreeMap<String, Vec<u8>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_xattrs: Vec<String>,
}

impl Attrs {
    pub fn is_empty(&self) -> bool {
        *self == Attrs::default()
    }
}

/// Where an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Queued,
    Uploading,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Uploading => "uploading",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> State {
        match s {
            "uploading" => State::Uploading,
            "done" => State::Done,
            "failed" => State::Failed,
            "cancelled" => State::Cancelled,
            _ => State::Queued,
        }
    }

    pub fn finished(self) -> bool {
        matches!(self, State::Done | State::Cancelled)
    }
}

/// The source file of an import as it was when queued: a file that changes is read again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub size: u64,
    pub mtime_ns: i128,
}

impl Stamp {
    pub fn of(meta: &std::fs::Metadata) -> Stamp {
        let mtime_ns = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i128);
        Stamp { size: meta.len(), mtime_ns }
    }

    fn encode(&self) -> String {
        format!("{}:{}", self.size, self.mtime_ns)
    }

    fn decode(s: &str) -> Option<Stamp> {
        let (a, b) = s.split_once(':')?;
        Some(Stamp { size: a.parse().ok()?, mtime_ns: b.parse().ok()? })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub id: EntryId,
    pub drive: String,
    pub key: String,
    pub op: Op,
    pub base: StoredBase,
    /// The bytes of a put or a write; absent only for a new empty mount put.
    pub source: Option<PathBuf>,
    /// Whether `source` is the journal's own copy, deleted once the entry is finished.
    pub staged: bool,
    pub stamp: Option<Stamp>,
    /// A write's offset.
    pub offset: u64,
    /// A write's length, a truncate's size.
    pub length: u64,
    pub to_key: Option<String>,
    pub replace: bool,
    pub attrs: Attrs,
    pub batch: Option<BatchId>,
    pub state: State,
    pub paused: bool,
    pub sent: u64,
    /// Bytes to send.
    pub size: u64,
    pub version: Option<String>,
    /// A successful mount mutation awaiting exact-version reconciliation, never resent.
    pub published_version: Option<String>,
    pub published_key: Option<String>,
    pub published_attrs: bool,
    pub conflict: Option<String>,
    pub error: Option<String>,
    pub upload_id: Option<String>,
    pub created: i64,
    /// Mount edits preserve a competing remote version instead of retrying without a guard.
    pub mount: bool,
    /// The local inode whose next mutation uses this entry's published version.
    pub mount_ino: Option<u64>,
}

impl Entry {
    pub fn new(drive: &str, key: &str, op: Op, base: StoredBase) -> Entry {
        Entry {
            id: 0,
            drive: drive.to_owned(),
            key: key.to_owned(),
            op,
            base,
            source: None,
            staged: false,
            stamp: None,
            offset: 0,
            length: 0,
            to_key: None,
            replace: false,
            attrs: Attrs::default(),
            batch: None,
            state: State::Queued,
            paused: false,
            sent: 0,
            size: 0,
            version: None,
            published_version: None,
            published_key: None,
            published_attrs: false,
            conflict: None,
            error: None,
            upload_id: None,
            created: now_ms(),
            mount: false,
            mount_ino: None,
        }
    }

    /// The keys it touches: its own, and a rename's destination.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.key.as_str()).chain(self.to_key.as_deref())
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

const COLUMNS: &str = "id, drive, key, op, base, source, staged, stamp, pos, length, to_key, overwrite, attrs, batch, state, paused, sent, size, version, conflict, error, upload_id, created, mount, mount_ino, published_version, published_key, published_attrs";

fn from_row(r: &Row) -> rusqlite::Result<Entry> {
    let attrs: Option<String> = r.get(12)?;
    let stamp: Option<String> = r.get(7)?;
    Ok(Entry {
        id: r.get(0)?,
        drive: r.get(1)?,
        key: r.get(2)?,
        op: Op::parse(&r.get::<_, String>(3)?).unwrap_or(Op::Put),
        base: StoredBase::decode(&r.get::<_, String>(4)?),
        source: r.get::<_, Option<String>>(5)?.map(PathBuf::from),
        staged: r.get(6)?,
        stamp: stamp.as_deref().and_then(Stamp::decode),
        offset: r.get::<_, i64>(8)? as u64,
        length: r.get::<_, i64>(9)? as u64,
        to_key: r.get(10)?,
        replace: r.get(11)?,
        attrs: attrs.and_then(|a| serde_json::from_str(&a).ok()).unwrap_or_default(),
        batch: r.get(13)?,
        state: State::parse(&r.get::<_, String>(14)?),
        paused: r.get(15)?,
        sent: r.get::<_, i64>(16)? as u64,
        size: r.get::<_, i64>(17)? as u64,
        version: r.get(18)?,
        conflict: r.get(19)?,
        error: r.get(20)?,
        upload_id: r.get(21)?,
        created: r.get(22)?,
        mount: r.get(23)?,
        mount_ino: r.get(24)?,
        published_version: r.get(25)?,
        published_key: r.get(26)?,
        published_attrs: r.get(27)?,
    })
}

/// Inserts `e`, giving it its id.
pub(crate) fn insert(c: &Connection, e: &mut Entry) -> rusqlite::Result<()> {
    let attrs = (!e.attrs.is_empty()).then(|| serde_json::to_string(&e.attrs).unwrap_or_default());
    c.execute(
        "INSERT INTO entries(drive, key, op, base, source, staged, stamp, pos, length, to_key, overwrite, attrs, batch, state, paused, sent, size, created, mount, mount_ino)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0, ?16, ?17, ?18, ?19)",
        params![
            e.drive,
            e.key,
            e.op.as_str(),
            e.base.encode(),
            e.source.as_ref().map(|p| p.to_string_lossy().into_owned()),
            e.staged,
            e.stamp.map(|s| s.encode()),
            e.offset as i64,
            e.length as i64,
            e.to_key,
            e.replace,
            attrs,
            e.batch,
            e.state.as_str(),
            e.paused,
            e.size as i64,
            e.created,
            e.mount,
            e.mount_ino,
        ],
    )?;
    e.id = c.last_insert_rowid();
    Ok(())
}

/// Writes the fields that change as an entry is published.
pub(crate) fn update(c: &Connection, e: &Entry) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE entries SET state = ?2, paused = ?3, sent = ?4, size = ?5, version = ?6, conflict = ?7, error = ?8, upload_id = ?9, stamp = ?10, base = ?11, published_version=?12, published_key=?13, published_attrs=?14 WHERE id = ?1",
        params![e.id, e.state.as_str(), e.paused, e.sent as i64, e.size as i64, e.version, e.conflict, e.error, e.upload_id, e.stamp.map(|s| s.encode()), e.base.encode(), e.published_version, e.published_key, e.published_attrs],
    )?;
    Ok(())
}

pub(crate) fn unfinished(c: &Connection) -> rusqlite::Result<Vec<Entry>> {
    let mut q = c.prepare(&format!("SELECT {COLUMNS} FROM entries WHERE state NOT IN ('done', 'cancelled') ORDER BY id"))?;
    q.query_map([], from_row)?.collect()
}

pub(crate) fn all(c: &Connection) -> rusqlite::Result<Vec<Entry>> {
    let mut q = c.prepare(&format!("SELECT {COLUMNS} FROM entries ORDER BY id"))?;
    q.query_map([], from_row)?.collect()
}

pub(crate) fn get(c: &Connection, id: EntryId) -> rusqlite::Result<Option<Entry>> {
    c.query_row(&format!("SELECT {COLUMNS} FROM entries WHERE id = ?1"), [id], from_row).optional()
}

/// A multipart upload's finished parts: number, ETag and size.
pub(crate) fn parts(c: &Connection, id: EntryId) -> rusqlite::Result<Vec<(u32, String, u64)>> {
    let mut q = c.prepare("SELECT number, etag, size FROM parts WHERE entry = ?1 ORDER BY number")?;
    q.query_map([id], |r| Ok((r.get::<_, i64>(0)? as u32, r.get(1)?, r.get::<_, i64>(2)? as u64)))?.collect()
}

pub(crate) fn add_part(c: &Connection, id: EntryId, number: u32, etag: &str, size: u64) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO parts(entry, number, etag, size) VALUES (?1, ?2, ?3, ?4)", params![id, number as i64, etag, size as i64])?;
    Ok(())
}

pub(crate) fn clear_parts(c: &Connection, id: EntryId) -> rusqlite::Result<()> {
    c.execute("DELETE FROM parts WHERE entry = ?1", [id])?;
    Ok(())
}
