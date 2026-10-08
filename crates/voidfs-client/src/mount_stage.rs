// SPDX-License-Identifier: Apache-2.0
//! Append bytes before committing their logical ranges, so an interrupted overwrite preserves
//! every previously acknowledged range. Gaps retain the bound remote base or read as zero.

use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::*;

pub(super) mod mtime {
    use std::time::SystemTime;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &SystemTime, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let value = chrono::DateTime::<chrono::Utc>::from(*value).to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
        serializer.serialize_str(&value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<SystemTime, D::Error> {
        let value = String::deserialize(deserializer)?;
        chrono::DateTime::parse_from_rfc3339(&value).map(SystemTime::from).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug)]
pub struct StagingConfig {
    pub min_free_bytes: u64,
    pub free_space: Option<fn(&Path) -> io::Result<u64>>,
    pub quiet_period: Option<Duration>,
    /// A flush rewrites a staging file without its overwritten bytes once they exceed both this
    /// and the bytes still in use.
    pub compact_garbage: u64,
}

impl Default for StagingConfig {
    fn default() -> Self {
        Self { min_free_bytes: 256 * 1024 * 1024, free_space: None, quiet_period: Some(Duration::from_secs(2)), compact_garbage: 64 * 1024 * 1024 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Extent { pub start: u64, pub end: u64, pub physical: u64 }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub base: Attr,
    pub key: String,
    pub size: u64,
    pub remote_size: u64,
    pub extents: Vec<Extent>,
    pub dirty: Vec<(u64, u64)>,
    pub reset: Option<u64>,
    pub revision: u64,
    pub flushed: u64,
}

/// A retired stage's bytes belong to no record any more: its file goes with the last handle
/// that still reads it.
#[derive(Debug)]
pub(crate) struct Stage { pub path: PathBuf, pub record: Record, pub retired: bool }

impl Drop for Stage {
    fn drop(&mut self) {
        if self.retired { let _ = std::fs::remove_file(&self.path); }
    }
}

fn io_error(error: io::Error) -> FsError { Error::from(error).into() }

fn bounds(offset: u64, len: u64) -> Result<u64> {
    offset.checked_add(len).filter(|end| *end <= i64::MAX as u64).ok_or(FsError::InvalidArgument)
}

fn admit(path: &Path, bytes: u64, record: &Record, cfg: &StagingConfig) -> Result<()> {
    let parent = path.parent().ok_or_else(|| FsError::Io("staging path has no parent".into()))?;
    let free = match cfg.free_space { Some(f) => f(parent), None => fs4::available_space(parent) }.map_err(io_error)?;
    let metadata = serde_json::to_vec(record).map_err(|e| FsError::Io(e.to_string()))?.len() as u64;
    let metadata = metadata.div_ceil(4096).checked_mul(8192).and_then(|n| n.checked_add(16 * 1024)).ok_or(FsError::NoSpace)?;
    let need = bytes.checked_add(metadata).and_then(|n| n.checked_add(cfg.min_free_bytes)).ok_or(FsError::NoSpace)?;
    if free < need { return Err(FsError::NoSpace); }
    Ok(())
}

fn overwrite(extents: &mut Vec<Extent>, start: u64, end: u64, physical: u64) {
    let mut next = Vec::with_capacity(extents.len() + 2);
    for extent in extents.drain(..) {
        if extent.end <= start || extent.start >= end { next.push(extent); continue; }
        if extent.start < start { next.push(Extent { start: extent.start, end: start, physical: extent.physical }); }
        if extent.end > end { next.push(Extent { start: end, end: extent.end, physical: extent.physical + end - extent.start }); }
    }
    next.push(Extent { start, end, physical });
    next.sort_unstable_by_key(|extent| extent.start);
    for extent in next {
        if let Some(last) = extents.last_mut() && last.end == extent.start && last.physical + last.end - last.start == extent.physical {
            last.end = extent.end;
        } else { extents.push(extent); }
    }
}

fn dirty(ranges: &mut Vec<(u64, u64)>, start: u64, end: u64) {
    ranges.push((start, end));
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges.drain(..) {
        if let Some(last) = merged.last_mut() && start <= last.1 { last.1 = last.1.max(end); }
        else { merged.push((start, end)); }
    }
    *ranges = merged;
}

pub(crate) fn read_range(path: &Path, physical: u64, len: u64) -> Result<Vec<u8>> {
    bounds(physical, len)?;
    let length = usize::try_from(len).map_err(|_| FsError::InvalidArgument)?;
    let mut bytes = vec![0; length];
    File::open(path).map_err(io_error)?.read_exact_at(&mut bytes, physical).map_err(io_error)?;
    Ok(bytes)
}

/// Inodes whose stage can't be used, and why.
pub(crate) type Damaged = Vec<(Ino, String)>;

/// The drive's stages, and the inodes whose stage can't be used because its record and file
/// disagree: a power failure can lose bytes a write acknowledged before any fsync, while a later
/// commit made their record durable. The rest of the drive stays usable.
pub(crate) fn load(store: &Store, drive: &str) -> Result<(HashMap<Ino, Stage>, Damaged)> {
    let rows = store.with(|c| {
        let mut q = c.prepare("SELECT s.ino, s.path, s.record FROM mount_staged s JOIN mount_inodes n ON n.ino=s.ino WHERE n.drive=?1")?;
        q.query_map([drive], |r| Ok((r.get::<_, Ino>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()
    })?;
    let (mut stages, mut damaged) = (HashMap::new(), Vec::new());
    for (ino, path, json) in rows {
        match check(store, ino, &path, &json) {
            Ok(stage) => { stages.insert(ino, stage); }
            Err(error) => damaged.push((ino, error.to_string())),
        }
    }
    Ok((stages, damaged))
}

fn check(store: &Store, ino: Ino, path: &str, json: &str) -> Result<Stage> {
    let record: Record = serde_json::from_str(json).map_err(|e| FsError::Io(e.to_string()))?;
    // By name in this state directory, which may be reached by another spelling than recorded.
    let name = Path::new(path).file_name().ok_or_else(|| FsError::Io("invalid staged file path".into()))?;
    let path = store.dir().join("mount-stage").join(name);
    if record.base.ino != ino || record.remote_size > record.base.size || record.remote_size > record.size || record.flushed > record.revision {
        return Err(FsError::Io("invalid staged file record".into()));
    }
    bounds(record.size, 0)?;
    let length = std::fs::metadata(&path).map_err(io_error)?.len();
    let mut previous = 0;
    for extent in &record.extents {
        if extent.start < previous || extent.start >= extent.end || extent.end > record.size || bounds(extent.physical, extent.end - extent.start)? > length {
            return Err(FsError::Io("staged bytes are missing from their file".into()));
        }
        previous = extent.end;
    }
    for &(start, end) in &record.dirty {
        if start >= end || end > record.size { return Err(FsError::Io("invalid staged dirty range".into())); }
    }
    Ok(Stage { path, record, retired: false })
}

#[cfg(test)]
pub(crate) fn load_all(store: &Store, drive: &str) -> Result<HashMap<Ino, Stage>> {
    let (stages, damaged) = load(store, drive)?;
    match damaged.into_iter().next() { Some((_, error)) => Err(FsError::Io(error)), None => Ok(stages) }
}

/// Removes the drive's staging files that no record names: bytes a crash left between creating,
/// compacting or publishing a stage and recording that, and assemblies of an interrupted flush.
/// Only the drive's writer calls it, before loading its stages, so none of them is open.
pub(crate) fn collect(store: &Store, drive: &str) -> Result<usize> {
    let dir = store.dir().join("mount-stage");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io_error(e)),
    };
    let paths = entries.map(|entry| entry.map(|entry| entry.path())).collect::<io::Result<Vec<_>>>().map_err(io_error)?;
    let removed = store.with(|c| {
        // By name: the state directory may be reached by another spelling than when recorded.
        let kept = c.prepare("SELECT path FROM mount_staged")?.query_map([], |r| r.get::<_, String>(0))?
            .map(|path| path.map(|path| PathBuf::from(path).file_name().map(|name| name.to_owned()))).collect::<rusqlite::Result<HashSet<_>>>()?;
        let mut ours = c.prepare("SELECT 1 FROM mount_inodes WHERE ino=?1 AND drive=?2")?;
        let mut removed = 0;
        for path in paths {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
            let orphan = name.starts_with("snapshot-") || match name.split('-').next().and_then(|ino| ino.parse::<Ino>().ok()) {
                Some(ino) => !kept.contains(&Some(std::ffi::OsString::from(name))) && ours.exists(params![ino, drive])?,
                None => false,
            };
            if orphan && path.is_file() && std::fs::remove_file(&path).is_ok() { removed += 1; }
        }
        Ok(removed)
    })?;
    Ok(removed)
}

impl Stage {
    pub fn new(store: &Store, base: Attr, key: String) -> Result<Self> {
        bounds(base.size, 0)?;
        let dir = store.dir().join("mount-stage");
        std::fs::create_dir_all(&dir).map_err(io_error)?;
        let mut random = [0u8; 12];
        File::open("/dev/urandom").map_err(io_error)?.read_exact(&mut random).map_err(io_error)?;
        let path = dir.join(format!("{}-{}", base.ino, hex::encode(random)));
        File::options().create_new(true).write(true).open(&path).map_err(io_error)?;
        let size = base.size;
        let record = Record { base, key, size, remote_size: size, extents: Vec::new(), dirty: Vec::new(), reset: None, revision: 0, flushed: 0 };
        Ok(Self { path, record, retired: false })
    }

    fn attr(store: &Store, drive: &str, ino: Ino) -> Result<Attr> {
        store.with_normal(|c| Ok(node(c, drive, ino)?.ok_or(FsError::Stale).and_then(|n| n.attr())))?
    }

    fn persist(&self, store: &Store, drive: &str, ino: Ino, next: &Record) -> Result<Attr> {
        let path = self.path.to_str().ok_or_else(|| FsError::Io("staging path is not UTF-8".into()))?;
        let record = serde_json::to_string(next).map_err(|e| FsError::Io(e.to_string()))?;
        store.with_normal(|c| {
            let tx = c.transaction()?;
            let result: Result<Attr> = (|| {
                let mut n = node(&tx, drive, ino)?.ok_or(FsError::Stale)?;
                if n.entry.kind != Kind::File { return Err(FsError::IsDir); }
                n.entry.size = Some(next.size);
                n.entry.mtime = Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true));
                n.generation = n.generation.checked_add(1).filter(|g| *g <= i64::MAX as u64).ok_or(FsError::InvalidArgument)?;
                n.sync = publication::pending(&tx, ino)?;
                save_node(&tx, &n)?;
                tx.execute("INSERT INTO mount_staged(ino, path, record) VALUES (?1, ?2, ?3)
                    ON CONFLICT(ino) DO UPDATE SET path=excluded.path, record=excluded.record", params![ino, path, record])?;
                n.attr()
            })();
            if result.is_ok() { tx.commit()?; }
            Ok(result)
        })?
    }

    pub fn append(&mut self, store: &Store, drive: &str, ino: Ino, offset: u64, data: &[u8], cfg: &StagingConfig) -> Result<Attr> {
        let end = bounds(offset, data.len() as u64)?;
        if data.is_empty() { return Self::attr(store, drive, ino); }
        if self.record.base.ino != ino { return Err(FsError::Stale); }
        let _admission = store.staging();
        let file = File::options().write(true).open(&self.path).map_err(io_error)?;
        let physical = file.metadata().map_err(io_error)?.len();
        bounds(physical, data.len() as u64)?;
        let mut next = self.record.clone();
        overwrite(&mut next.extents, offset, end, physical);
        dirty(&mut next.dirty, offset, end);
        next.size = next.size.max(end);
        next.revision = next.revision.checked_add(1).filter(|r| *r <= i64::MAX as u64).ok_or(FsError::InvalidArgument)?;
        admit(&self.path, data.len() as u64, &next, cfg)?;
        if let Err(e) = file.write_all_at(data, physical) {
            let _ = file.set_len(physical);
            return Err(io_error(e));
        }
        crate::kill::point("stage.appended");
        match self.persist(store, drive, ino, &next) {
            Ok(attr) => { crate::kill::point("stage.recorded"); self.record = next; Ok(attr) },
            Err(e) => Err(e),
        }
    }

    pub fn truncate(&mut self, store: &Store, drive: &str, ino: Ino, size: u64, cfg: &StagingConfig) -> Result<Attr> {
        bounds(size, 0)?;
        if self.record.base.ino != ino { return Err(FsError::Stale); }
        if size == self.record.size { return Self::attr(store, drive, ino); }
        let _admission = store.staging();
        admit(&self.path, 0, &self.record, cfg)?;
        let mut next = self.record.clone();
        if size < next.size {
            next.remote_size = next.remote_size.min(size);
            next.extents.retain(|extent| extent.start < size);
            for extent in &mut next.extents { extent.end = extent.end.min(size); }
            next.dirty.retain(|(start, _)| *start < size);
            for (_, end) in &mut next.dirty { *end = (*end).min(size); }
            next.reset = Some(next.reset.map_or(size, |previous| previous.min(size)));
        }
        next.size = size;
        next.revision = next.revision.checked_add(1).filter(|r| *r <= i64::MAX as u64).ok_or(FsError::InvalidArgument)?;
        let attr = self.persist(store, drive, ino, &next)?;
        self.record = next;
        Ok(attr)
    }

    pub fn sync(&self) -> Result<()> {
        File::options().write(true).open(&self.path).map_err(io_error)?.sync_all().map_err(io_error)?;
        let dir = self.path.parent().ok_or_else(|| FsError::Io("staging path has no parent".into()))?;
        File::open(dir).map_err(io_error)?.sync_all().map_err(io_error)?;
        File::open(dir.parent().ok_or_else(|| FsError::Io("staging directory has no parent".into()))?).map_err(io_error)?.sync_all().map_err(io_error)
    }

    /// The staging file's bytes no extent uses any more, and the bytes extents use.
    pub fn garbage(&self) -> Result<(u64, u64)> {
        let length = std::fs::metadata(&self.path).map_err(io_error)?.len();
        let live = self.record.extents.iter().map(|extent| extent.end - extent.start).sum::<u64>();
        Ok((length.saturating_sub(live), live))
    }

    /// Copies the bytes extents use to a new file, makes it the stage's durably, then removes the
    /// old one. A crash leaves one of the two unrecorded, which [`collect`] removes. Returns
    /// whether it compacted: a stage not yet recorded, or recorded at another file after an
    /// uncertain commit, is left as it is.
    pub fn compact(&mut self, store: &Store, ino: Ino, cfg: &StagingConfig) -> Result<bool> {
        let dir = self.path.parent().ok_or_else(|| FsError::Io("staging path has no parent".into()))?.to_owned();
        let (_, live) = self.garbage()?;
        admit(&self.path, live, &self.record, cfg)?;
        let mut random = [0u8; 12];
        File::open("/dev/urandom").map_err(io_error)?.read_exact(&mut random).map_err(io_error)?;
        let path = dir.join(format!("{ino}-{}", hex::encode(random)));
        let mut record = self.record.clone();
        let written = (|| {
            let source = File::open(&self.path).map_err(io_error)?;
            let dest = File::options().create_new(true).write(true).open(&path).map_err(io_error)?;
            let mut buffer = vec![0u8; 1024 * 1024];
            let (mut at, mut extents) = (0u64, Vec::<Extent>::with_capacity(record.extents.len()));
            for extent in &record.extents {
                let (physical, mut from, mut left) = (at, extent.physical, extent.end - extent.start);
                while left > 0 {
                    let n = left.min(buffer.len() as u64) as usize;
                    source.read_exact_at(&mut buffer[..n], from).map_err(io_error)?;
                    dest.write_all_at(&buffer[..n], at).map_err(io_error)?;
                    (from, at, left) = (from + n as u64, at + n as u64, left - n as u64);
                }
                match extents.last_mut() {
                    Some(last) if last.end == extent.start && last.physical + last.end - last.start == physical => last.end = extent.end,
                    _ => extents.push(Extent { start: extent.start, end: extent.end, physical }),
                }
            }
            dest.sync_all().map_err(io_error)?;
            File::open(&dir).map_err(io_error)?.sync_all().map_err(io_error)?;
            record.extents = extents;
            serde_json::to_string(&record).map_err(|e| FsError::Io(e.to_string()))
        })();
        let json = match written { Ok(json) => json, Err(e) => { let _ = std::fs::remove_file(&path); return Err(e); } };
        crate::kill::point("compact.written");
        let (old, new) = (self.path.to_str(), path.to_str());
        let (Some(old), Some(new)) = (old, new) else { let _ = std::fs::remove_file(&path); return Err(FsError::Io("staging path is not UTF-8".into())); };
        // An error here may follow a commit: both files stay until the next writer collects one.
        let updated = store.with(|c| c.execute("UPDATE mount_staged SET path=?3, record=?4 WHERE ino=?1 AND path=?2", params![ino, old, new, json]))?;
        if updated != 1 { let _ = std::fs::remove_file(&path); return Ok(false); }
        crate::kill::point("compact.committed");
        let old = std::mem::replace(&mut self.path, path);
        self.record = record;
        let _ = std::fs::remove_file(old);
        Ok(true)
    }

    /// Reads an immutable byte range referred to by an extent's physical offset.
    #[cfg(test)]
    pub fn read_local(&self, physical: u64, len: u64) -> Result<Vec<u8>> {
        read_range(&self.path, physical, len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plenty(_: &Path) -> io::Result<u64> { Ok(u64::MAX) }
    fn empty(_: &Path) -> io::Result<u64> { Ok(0) }
    fn config() -> StagingConfig { StagingConfig { min_free_bytes: 0, free_space: Some(plenty), quiet_period: None, ..Default::default() } }

    fn inode(store: &Store, size: u64) -> Attr {
        store.with(|c| {
            let entry = FolderEntry { name: "file".into(), kind: Kind::File, object_id: "object".into(), version_id: Some("version".into()),
                size: Some(size), etag: Some("etag".into()), mtime: None, mode: None, has_xattrs: false, target: None };
            c.execute("INSERT INTO mount_inodes(drive, attrs) VALUES ('drive', ?1)", [serde_json::to_string(&entry).unwrap()])?;
            Ok(node(c, "drive", c.last_insert_rowid() as u64)?.unwrap().attr().unwrap())
        }).unwrap()
    }

    fn local(stage: &Stage) -> Vec<u8> {
        let mut bytes = vec![0; stage.record.size as usize];
        for extent in &stage.record.extents {
            bytes[extent.start as usize..extent.end as usize].copy_from_slice(&stage.read_local(extent.physical, extent.end - extent.start).unwrap());
        }
        bytes
    }

    #[test]
    fn overlapping_writes_split_then_merge_extents_and_restart_keeps_acknowledged_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let old_mtime = UNIX_EPOCH - Duration::from_secs(365 * 86400);
        let mut attr = inode(&store, 0);
        attr.mtime = old_mtime;
        let ino = attr.ino;
        let mut stage = Stage::new(&store, attr, "file".into()).unwrap();
        let initial = stage.append(&store, "drive", ino, 0, b"abcdefgh", &config()).unwrap();
        assert_eq!(initial.size, 8);
        assert_eq!(initial.sync, Sync::Pending);
        stage.append(&store, "drive", ino, 2, b"XX", &config()).unwrap();
        stage.append(&store, "drive", ino, 3, b"YZ", &config()).unwrap();
        assert_eq!(local(&stage), b"abXYZfgh");
        assert_eq!(stage.record.dirty, [(0, 8)]);
        assert_eq!(stage.record.revision, 3);
        assert_eq!(stage.record.base.size, 0);
        stage.sync().unwrap();
        drop(stage);
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let stages = load_all(&store, "drive").unwrap();
        assert_eq!(local(&stages[&ino]), b"abXYZfgh");
        assert_eq!(stages[&ino].record.base.mtime, old_mtime);
        assert!(load_all(&store, "other-drive").unwrap().is_empty());
    }

    #[test]
    fn contiguous_appends_use_one_extent_without_allocating_a_logical_hole() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let attr = inode(&store, 0);
        let ino = attr.ino;
        let mut stage = Stage::new(&store, attr, "file".into()).unwrap();
        stage.append(&store, "drive", ino, 1024 * 1024 * 1024, b"ab", &config()).unwrap();
        stage.append(&store, "drive", ino, 1024 * 1024 * 1024 + 2, b"cd", &config()).unwrap();
        assert_eq!(stage.record.extents, [Extent { start: 1024 * 1024 * 1024, end: 1024 * 1024 * 1024 + 4, physical: 0 }]);
        assert_eq!(std::fs::metadata(&stage.path).unwrap().len(), 4);
        assert_eq!(stage.read_local(0, 4).unwrap(), b"abcd");
    }

    #[test]
    fn failed_metadata_commit_preserves_previous_acknowledgements_and_runtime_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let attr = inode(&store, 0);
        let ino = attr.ino;
        let mut stage = Stage::new(&store, attr, "file".into()).unwrap();
        let accepted = stage.append(&store, "drive", ino, 0, b"accepted", &config()).unwrap();
        let before = serde_json::to_string(&stage.record).unwrap();
        store.with(|c| c.execute_batch("CREATE TRIGGER refuse_staged BEFORE INSERT ON mount_staged BEGIN SELECT RAISE(ABORT, 'test failure'); END")).unwrap();
        assert!(matches!(stage.append(&store, "drive", ino, 0, b"damaging", &config()), Err(FsError::Io(_))));
        assert_eq!(local(&stage), b"accepted");
        assert_eq!(serde_json::to_string(&stage.record).unwrap(), before);
        assert_eq!(Stage::attr(&store, "drive", ino).unwrap(), accepted);
        assert_eq!(std::fs::metadata(&stage.path).unwrap().len(), 16);
        let reloaded = load_all(&store, "drive").unwrap();
        assert_eq!(local(&reloaded[&ino]), b"accepted");
    }

    #[test]
    fn shrinking_and_regrowing_never_restore_clipped_local_or_remote_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let attr = inode(&store, 12);
        let ino = attr.ino;
        let mut stage = Stage::new(&store, attr, "file".into()).unwrap();
        stage.append(&store, "drive", ino, 8, b"abcd", &config()).unwrap();
        stage.truncate(&store, "drive", ino, 10, &config()).unwrap();
        assert_eq!(stage.record.extents, [Extent { start: 8, end: 10, physical: 0 }]);
        stage.truncate(&store, "drive", ino, 4, &config()).unwrap();
        stage.truncate(&store, "drive", ino, 12, &config()).unwrap();
        assert!(stage.record.extents.is_empty());
        assert!(stage.record.dirty.is_empty());
        assert_eq!(stage.record.remote_size, 4);
        assert_eq!(stage.record.reset, Some(4));
        stage.append(&store, "drive", ino, 7, b"z", &config()).unwrap();
        assert_eq!(stage.record.remote_size, 4);
        assert_eq!(stage.record.reset, Some(4));
        assert_eq!(local(&stage), b"\0\0\0\0\0\0\0z\0\0\0\0");
        let reloaded = load_all(&store, "drive").unwrap();
        assert_eq!(reloaded[&ino].record.remote_size, 4);
        assert_eq!(local(&reloaded[&ino]), local(&stage));
    }

    #[test]
    fn low_space_and_invalid_ranges_fail_before_mutating_any_bytes_or_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let attr = inode(&store, 0);
        let ino = attr.ino;
        let mut stage = Stage::new(&store, attr.clone(), "file".into()).unwrap();
        let cfg = StagingConfig { free_space: Some(empty), ..config() };
        assert_eq!(stage.append(&store, "drive", ino, 0, b"new", &cfg), Err(FsError::NoSpace));
        assert_eq!(stage.truncate(&store, "drive", ino, 9, &cfg), Err(FsError::NoSpace));
        assert_eq!(stage.append(&store, "drive", ino, i64::MAX as u64, b"new", &config()), Err(FsError::InvalidArgument));
        assert_eq!(stage.truncate(&store, "drive", ino, u64::MAX, &config()), Err(FsError::InvalidArgument));
        assert_eq!(stage.append(&store, "drive", ino, 0, b"", &cfg).unwrap(), attr);
        assert_eq!(stage.truncate(&store, "drive", ino, 0, &cfg).unwrap(), attr);
        assert_eq!(stage.record.revision, 0);
        assert_eq!(std::fs::metadata(&stage.path).unwrap().len(), 0);
        assert!(load_all(&store, "drive").unwrap().is_empty());
    }
}
