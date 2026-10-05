// SPDX-License-Identifier: Apache-2.0
//! The adapter-independent namespace. Remote names remain byte-exact; equivalent Unicode
//! lookups are accepted only when unambiguous. Mutations and open handles follow in later slices.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use unicode_normalization::UnicodeNormalization;
use voidfs_sdk::{Client, FolderEntry, Kind};

use crate::{Connectivity, Error, Invalidation, Link, Store};

pub type Ino = u64;
pub type Result<T> = std::result::Result<T, FsError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync { Saved, Pending, Saving, Conflict, Error }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    pub ino: Ino,
    pub kind: Kind,
    pub size: u64,
    pub mtime: SystemTime,
    pub mode: u32,
    pub generation: u64,
    pub sync: Sync,
    pub object_id: Option<String>,
    pub version_id: Option<String>,
    pub etag: Option<String>,
    pub has_xattrs: bool,
    pub target: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    #[error("no such name")]
    NotFound,
    #[error("name already exists")]
    Exists,
    #[error("directory is not empty")]
    NotEmpty,
    #[error("is a directory")]
    IsDir,
    #[error("not a directory")]
    NotDir,
    #[error("no space left")]
    NoSpace,
    #[error("read-only filesystem")]
    ReadOnly,
    #[error("unsupported operation or object kind")]
    Unsupported,
    #[error("stale inode")]
    Stale,
    #[error("the server can't be reached")]
    Offline,
    #[error("permission denied")]
    Permission,
    #[error("invalid name")]
    InvalidName,
    #[error("ambiguous Unicode name")]
    Ambiguous,
    #[error("namespace changed during enumeration; retry")]
    Again,
    #[error("{0}")]
    Io(String),
}

impl FsError {
    /// Native errno constants, including platforms where ESTALE/EOPNOTSUPP differ.
    pub fn errno(&self) -> i32 {
        match self {
            Self::NotFound => libc::ENOENT, Self::Exists => libc::EEXIST, Self::NotEmpty => libc::ENOTEMPTY,
            Self::IsDir => libc::EISDIR, Self::NotDir => libc::ENOTDIR, Self::NoSpace => libc::ENOSPC,
            Self::ReadOnly => libc::EROFS, Self::Unsupported => libc::EOPNOTSUPP, Self::Stale => libc::ESTALE,
            Self::Offline => libc::ENETDOWN, Self::Permission => libc::EACCES, Self::InvalidName => libc::EINVAL,
            Self::Ambiguous => libc::EILSEQ, Self::Again => libc::EAGAIN, Self::Io(_) => libc::EIO,
        }
    }
}

impl From<Error> for FsError {
    fn from(e: Error) -> Self {
        match e {
            Error::Offline => Self::Offline,
            Error::Fetch(e) => sdk_error(&e),
            Error::Io(e) if e.raw_os_error() == Some(libc::ENOSPC) => Self::NoSpace,
            Error::Db(e) if matches!(e.as_ref(), rusqlite::Error::SqliteFailure(err, _) if err.code == rusqlite::ErrorCode::DiskFull) => Self::NoSpace,
            _ => Self::Io(e.to_string()),
        }
    }
}

fn sdk_error(e: &voidfs_sdk::Error) -> FsError {
    match e.status() {
        Some(404) => FsError::NotFound, Some(401 | 403) => FsError::Permission,
        Some(501) => FsError::Unsupported, Some(409 | 412) => FsError::Again,
        _ => match e { voidfs_sdk::Error::Transport { .. } => FsError::Offline, _ => FsError::Io(e.to_string()) },
    }
}

struct Node { ino: Ino, entry: FolderEntry, generation: u64, sync: Sync }

impl Node {
    fn attr(&self) -> Result<Attr> {
        let mtime = match &self.entry.mtime {
            Some(t) => chrono::DateTime::parse_from_rfc3339(t).map(SystemTime::from).map_err(|e| FsError::Io(e.to_string()))?,
            None => UNIX_EPOCH,
        };
        let mode = match &self.entry.mode {
            Some(m) => u32::from_str_radix(m, 8).map_err(|e| FsError::Io(e.to_string()))?,
            None => if self.entry.kind == Kind::Folder { 0o755 } else { 0o644 },
        };
        Ok(Attr { ino: self.ino, kind: self.entry.kind, size: self.entry.size.unwrap_or(0), mtime, mode,
            generation: self.generation, sync: self.sync, object_id: (!self.entry.object_id.is_empty()).then(|| self.entry.object_id.clone()),
            version_id: self.entry.version_id.clone(), etag: self.entry.etag.clone(), has_xattrs: self.entry.has_xattrs, target: self.entry.target.clone() })
    }
}

fn decode_json(s: &str) -> rusqlite::Result<FolderEntry> {
    serde_json::from_str(s).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}

fn node(c: &Connection, drive: &str, ino: Ino) -> rusqlite::Result<Option<Node>> {
    c.query_row("SELECT attrs, generation, sync FROM mount_inodes WHERE ino=?1 AND drive=?2", params![ino, drive], |r| {
        let sync: String = r.get(2)?;
        let sync = match sync.as_str() {
            "saved" => Sync::Saved, "pending" => Sync::Pending, "saving" => Sync::Saving, "conflict" => Sync::Conflict, "error" => Sync::Error,
            _ => return Err(rusqlite::Error::InvalidQuery),
        };
        Ok(Node { ino, entry: decode_json(&r.get::<_, String>(0)?)?, generation: r.get(1)?, sync })
    }).optional()
}

fn names(c: &Connection, parent: Ino) -> rusqlite::Result<Vec<(String, Ino)>> {
    let mut q = c.prepare("SELECT name, ino FROM mount_overlay WHERE parent=?1 AND ino IS NOT NULL
        UNION ALL SELECT name, ino FROM mount_names n WHERE parent=?1 AND NOT EXISTS
        (SELECT 1 FROM mount_overlay o WHERE o.parent=n.parent AND o.name=n.name) ORDER BY name")?;
    q.query_map([parent], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

fn parent(c: &Connection, ino: Ino) -> rusqlite::Result<Option<(Ino, String)>> {
    c.query_row("SELECT parent, name FROM mount_overlay WHERE ino=?1 UNION ALL
        SELECT parent, name FROM mount_names n WHERE ino=?1 AND NOT EXISTS
        (SELECT 1 FROM mount_overlay o WHERE o.parent=n.parent AND o.name=n.name) LIMIT 1", [ino], |r| Ok((r.get(0)?, r.get(1)?))).optional()
}

fn chain(c: &Connection, drive: &str, root: Ino, ino: Ino) -> rusqlite::Result<Option<Vec<(Ino, String)>>> {
    let mut at = ino;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    while at != root {
        if !seen.insert(at) || node(c, drive, at)?.is_none() { return Ok(None); }
        let Some((p, name)) = parent(c, at)? else { return Ok(None); };
        out.push((at, name));
        at = p;
    }
    out.push((root, String::new()));
    out.reverse();
    Ok(Some(out))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && name != "." && name != ".." && !name.contains(['/', '\0'])
}

/// One drive's persistent namespace. The caller supplies its feed invalidations; opening a
/// session distrusts persisted metadata online and retains complete snapshots for offline use.
/// `drive` must be a stable canonical drive identifier, rather than a reusable display alias.
/// This slice serves metadata only, and makes no remote mutations.
pub struct Session {
    store: Arc<Store>, client: Client, drive: String, root: Ino, connectivity: Connectivity,
    refresh: tokio::sync::Mutex<()>,
}

impl Session {
    pub async fn new(store: Arc<Store>, client: Client, drive: &str, connectivity: Connectivity) -> Result<Self> {
        let d = drive.to_owned();
        let s = store.clone();
        let root = tokio::task::spawn_blocking(move || s.with(|c| {
            let tx = c.transaction()?;
            let root = match tx.query_row("SELECT ino FROM mount_roots WHERE drive=?1", [&d], |r| r.get::<_, Ino>(0)).optional()? {
                Some(ino) => ino,
                None => {
                    let entry = FolderEntry { name: String::new(), kind: Kind::Folder, object_id: String::new(), version_id: None,
                        size: None, etag: None, mtime: None, mode: Some("0755".into()), has_xattrs: false, target: None };
                    tx.execute("INSERT INTO mount_inodes(drive, attrs) VALUES (?1, ?2)", params![d, serde_json::to_string(&entry).expect("JSON")])?;
                    let ino = tx.last_insert_rowid() as Ino;
                    tx.execute("INSERT INTO mount_roots(drive, ino) VALUES (?1, ?2)", params![d, ino])?;
                    tx.execute("INSERT INTO mount_dirs(ino) VALUES (?1)", [ino])?;
                    ino
                }
            };
            tx.execute("UPDATE mount_dirs SET listed=0, generation=generation+1 WHERE ino IN (SELECT ino FROM mount_inodes WHERE drive=?1)", [&d])?;
            tx.commit()?;
            Ok(root)
        })).await.map_err(Error::from)??;
        Ok(Self { store, client, drive: drive.to_owned(), root, connectivity, refresh: tokio::sync::Mutex::new(()) })
    }

    pub fn root(&self) -> Ino { self.root }

    async fn db<T: Send + 'static>(&self, f: impl FnOnce(&mut Connection, &str, Ino) -> rusqlite::Result<T> + Send + 'static) -> Result<T> {
        let s = self.store.clone();
        let d = self.drive.clone();
        let root = self.root;
        Ok(tokio::task::spawn_blocking(move || s.with(|c| f(c, &d, root))).await.map_err(Error::from)??)
    }

    async fn ancestors(&self, ino: Ino) -> Result<()> {
        if ino == 0 || ino > i64::MAX as u64 { return Err(FsError::Stale); }
        let path = self.db(move |c, d, root| chain(c, d, root, ino)).await?.ok_or(FsError::Stale)?;
        for (p, _) in path.iter().take(path.len() - 1) { self.refresh_dir(*p).await?; }
        if self.db(move |c, d, root| chain(c, d, root, ino)).await?.is_none() { return Err(FsError::Stale); }
        Ok(())
    }

    pub async fn lookup(&self, parent: Ino, name: &str) -> Result<Attr> {
        if !valid_name(name) { return Err(FsError::InvalidName); }
        self.ancestors(parent).await?;
        self.refresh_dir(parent).await?;
        let name = name.to_owned();
        let children = self.db(move |c, _, _| names(c, parent)).await?;
        let normalized: String = name.nfc().collect();
        let candidates: Vec<Ino> = children.iter().filter(|(n, _)| n.nfc().eq(normalized.chars())).map(|(_, ino)| *ino).collect();
        if candidates.len() > 1 { return Err(FsError::Ambiguous); }
        let ino = candidates.first().copied().ok_or(if self.connectivity.link() == Link::Offline { FsError::Offline } else { FsError::NotFound })?;
        self.cached_attr(ino).await
    }

    pub async fn getattr(&self, ino: Ino) -> Result<Attr> {
        self.ancestors(ino).await?;
        self.cached_attr(ino).await
    }

    async fn cached_attr(&self, ino: Ino) -> Result<Attr> {
        self.db(move |c, d, _| node(c, d, ino)).await?.ok_or(FsError::Stale)?.attr()
    }

    /// Names are byte-exact, case-sensitive and ordered by UTF-8 bytes. `after` is an exclusive
    /// name cursor within the current generation; adapters restart enumeration after invalidation.
    pub async fn readdir(&self, dir: Ino, after: Option<&str>, limit: usize) -> Result<Vec<(String, Attr)>> {
        self.ancestors(dir).await?;
        self.refresh_dir(dir).await?;
        let after = after.map(str::to_owned);
        let rows = self.db(move |c, d, _| {
            names(c, dir)?.into_iter().filter(|(n, _)| after.as_ref().is_none_or(|a| n > a)).take(limit)
                .map(|(name, ino)| Ok((name, node(c, d, ino)?.ok_or(rusqlite::Error::InvalidQuery)?))).collect::<rusqlite::Result<Vec<_>>>()
        }).await?;
        rows.into_iter().map(|(name, n)| Ok((name, n.attr()?))).collect()
    }

    async fn refresh_dir(&self, dir: Ino) -> Result<()> {
        let _refresh = self.refresh.lock().await;
        for _ in 0..4 {
            let state = self.db(move |c, d, root| {
                let Some(n) = node(c, d, dir)? else { return Ok(None); };
                let Some(path) = chain(c, d, root, dir)? else { return Ok(None); };
                let (generation, listed, seq): (u64, bool, Option<u64>) = c.query_row("SELECT generation, listed, seq FROM mount_dirs WHERE ino=?1", [dir], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?.unwrap_or((0, false, None));
                let key = path.into_iter().skip(1).map(|(_, n)| format!("{n}/")).collect::<String>();
                Ok(Some((n.entry.kind, key, generation, listed, seq)))
            }).await?.ok_or(FsError::Stale)?;
            let (kind, key, generation, listed, seq) = state;
            if kind != Kind::Folder { return Err(FsError::NotDir); }
            if listed { return Ok(()); }
            if self.connectivity.link() == Link::Offline {
                return if seq.is_some() { Ok(()) } else { Err(FsError::Offline) };
            }
            let Some((entries, seq)) = self.listing(&key).await? else { continue; };
            let accepted = self.db(move |c, d, root| {
                let tx = c.transaction()?;
                let current: u64 = tx.query_row("SELECT generation FROM mount_dirs WHERE ino=?1", [dir], |r| r.get(0))?;
                let latest: u64 = tx.query_row("SELECT seq FROM mount_roots WHERE drive=?1", [d], |r| r.get(0))?;
                if current != generation || seq < latest { return Ok(false); }
                let ancestors = chain(&tx, d, root, dir)?.ok_or(rusqlite::Error::InvalidQuery)?;
                for e in &entries {
                    if let Some(ino) = tx.query_row("SELECT ino FROM mount_inodes WHERE drive=?1 AND object_id=?2", params![d, e.object_id], |r| r.get::<_, Ino>(0)).optional()?
                        && (ancestors.iter().any(|(a, _)| *a == ino) || node(&tx, d, ino)?.is_none_or(|n| n.entry.kind != e.kind))
                    { return Err(rusqlite::Error::InvalidQuery); }
                }
                tx.execute("DELETE FROM mount_names WHERE parent=?1", [dir])?;
                for e in entries {
                    let json = serde_json::to_string(&e).expect("JSON");
                    tx.execute("INSERT INTO mount_inodes(drive, object_id, attrs) VALUES (?1, ?2, ?3)
                        ON CONFLICT(drive, object_id) DO UPDATE SET generation=generation+(attrs<>excluded.attrs), attrs=excluded.attrs
                        WHERE sync='saved'", params![d, e.object_id, json])?;
                    let ino: Ino = tx.query_row("SELECT ino FROM mount_inodes WHERE drive=?1 AND object_id=?2", params![d, e.object_id], |r| r.get(0))?;
                    tx.execute("DELETE FROM mount_names WHERE ino=?1", [ino])?;
                    tx.execute("INSERT INTO mount_names VALUES (?1, ?2, ?3)", params![dir, e.name.trim_end_matches('/'), ino])?;
                    if e.kind == Kind::Folder { tx.execute("INSERT OR IGNORE INTO mount_dirs(ino) VALUES (?1)", [ino])?; }
                }
                tx.execute("UPDATE mount_dirs SET listed=1, seq=?2, generation=generation+1 WHERE ino=?1", params![dir, seq])?;
                tx.execute("UPDATE mount_inodes SET generation=generation+1 WHERE ino=?1", [dir])?;
                tx.execute("UPDATE mount_roots SET seq=?2 WHERE drive=?1", params![d, seq])?;
                tx.commit()?;
                Ok(true)
            }).await?;
            if accepted { return Ok(()); }
        }
        Err(FsError::Again)
    }

    async fn listing(&self, key: &str) -> Result<Option<(Vec<FolderEntry>, u64)>> {
        let first = self.client.list_folder_page(&self.drive, key, None).await.map_err(|e| sdk_error(&e))?;
        if first.prefix != key { return Err(FsError::Io("listing returned the wrong prefix".into())); }
        let seq = first.seq;
        let mut entries = first.entries;
        let mut next = first.next_continuation_token;
        let mut tokens = HashSet::new();
        while let Some(token) = next {
            if !tokens.insert(token.clone()) { return Err(FsError::Io("listing repeated its continuation token".into())); }
            let page = self.client.list_folder_page(&self.drive, key, Some(&token)).await.map_err(|e| sdk_error(&e))?;
            if page.prefix != key { return Err(FsError::Io("listing returned the wrong prefix".into())); }
            if page.seq != seq { return Ok(None); }
            entries.extend(page.entries);
            next = page.next_continuation_token;
        }
        let mut names = HashSet::new();
        let mut ids = HashSet::new();
        for e in &entries {
            if e.kind == Kind::Unknown { return Err(FsError::Unsupported); }
            if e.kind != Kind::Folder && (e.size.is_none() || e.version_id.as_ref().is_none_or(String::is_empty) || e.etag.as_ref().is_none_or(String::is_empty))
                || e.kind == Kind::Symlink && e.target.is_none()
            { return Err(FsError::Io("incomplete object attributes".into())); }
            if !valid_name(e.name.trim_end_matches('/')) || e.name.ends_with('/') != (e.kind == Kind::Folder)
                || e.name.ends_with("//") || e.object_id.is_empty() || !names.insert(e.name.trim_end_matches('/')) || !ids.insert(&e.object_id)
            { return Err(FsError::Io("invalid or duplicate directory entry".into())); }
            Node { ino: 0, entry: e.clone(), generation: 0, sync: Sync::Saved }.attr()?;
        }
        Ok(Some((entries, seq)))
    }

    /// Persists invalidation and returns known affected inodes, including containing directories.
    /// This does not take the refresh mutex: an in-flight listing must lose its generation race.
    pub async fn invalidate(&self, changes: &[Invalidation]) -> Result<Vec<Ino>> {
        let changes = changes.to_vec();
        self.db(move |c, d, root| {
            let tx = c.transaction()?;
            let mut q = tx.prepare("SELECT ino FROM mount_inodes WHERE drive=?1")?;
            let inodes = q.query_map([d], |r| r.get::<_, Ino>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            drop(q);
            let mut affected = BTreeSet::new();
            for ino in inodes {
                let n = node(&tx, d, ino)?.ok_or(rusqlite::Error::InvalidQuery)?;
                let path = chain(&tx, d, root, ino)?;
                let key = path.map(|p| {
                    let mut key = p.into_iter().skip(1).map(|(_, n)| n).collect::<Vec<_>>().join("/");
                    if n.entry.kind == Kind::Folder && !key.is_empty() { key.push('/'); }
                    key
                });
                if changes.iter().any(|change| match change {
                    Invalidation::All => true,
                    Invalidation::Object(k) => key.as_ref().is_some_and(|p| p == k || n.entry.kind == Kind::Folder && k.starts_with(p)),
                    Invalidation::Subtree(k) => key.as_ref().is_some_and(|p| p.starts_with(k) || n.entry.kind == Kind::Folder && k.starts_with(p)),
                }) {
                    affected.insert(ino);
                    tx.execute("UPDATE mount_dirs SET listed=0, generation=generation+1 WHERE ino=?1", [ino])?;
                    tx.execute("UPDATE mount_inodes SET generation=generation+1 WHERE ino=?1", [ino])?;
                }
            }
            tx.commit()?;
            Ok(affected.into_iter().collect())
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidfs_sdk::Config;

    fn entry(name: &str, id: &str, size: u64) -> FolderEntry {
        FolderEntry { name: name.into(), kind: Kind::File, object_id: id.into(), version_id: Some("1.0".into()),
            size: Some(size), etag: Some("\"etag\"".into()), mtime: None, mode: Some("0600".into()), has_xattrs: false, target: None }
    }

    #[tokio::test]
    async fn local_names_and_tombstones_override_the_remote_snapshot_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let conn = Connectivity::default();
        let client = Client::new(Config { endpoint: "http://localhost:9".into(), access_key_id: "test".into(), secret_access_key: "test".into(), ..Default::default() }).unwrap();
        let s = Session::new(store.clone(), client.clone(), "drv", conn.clone()).await.unwrap();
        let root = s.root();
        let (remote, local) = store.with(|c| {
            for (id, size) in [("remote", 10), ("local", 20)] {
                c.execute("INSERT INTO mount_inodes(drive, object_id, attrs, sync) VALUES ('drv', ?1, ?2, ?3)",
                    params![id, serde_json::to_string(&entry("name", id, size)).unwrap(), if id == "local" { "pending" } else { "saved" }])?;
            }
            let local = c.last_insert_rowid() as Ino;
            let remote = local - 1;
            c.execute("INSERT INTO mount_names VALUES (?1, 'name', ?2)", params![root, remote])?;
            c.execute("INSERT INTO mount_overlay VALUES (?1, 'name', ?2)", params![root, local])?;
            c.execute("INSERT INTO mount_overlay VALUES (?1, 'gone', NULL)", [root])?;
            let gone = serde_json::to_string(&entry("gone", "gone", 30)).unwrap();
            c.execute("INSERT INTO mount_inodes(drive, object_id, attrs) VALUES ('drv', 'gone', ?1)", [gone])?;
            c.execute("INSERT INTO mount_names VALUES (?1, 'gone', ?2)", params![root, c.last_insert_rowid()])?;
            c.execute("UPDATE mount_dirs SET listed=1, seq=1 WHERE ino=?1", [root])?;
            Ok((remote, local))
        }).unwrap();
        let rows = s.readdir(root, None, 100).await.unwrap();
        assert_eq!(rows.iter().map(|(n, a)| (n.as_str(), a.ino, a.size, a.sync)).collect::<Vec<_>>(), [("name", local, 20, Sync::Pending)]);
        assert_eq!(s.lookup(root, "name").await.unwrap().ino, local);
        assert_eq!(s.lookup(root, "gone").await.unwrap_err(), FsError::NotFound);
        assert_eq!(s.getattr(remote).await.unwrap_err(), FsError::Stale);
        drop(s);
        drop(store);
        for _ in 0..3 { conn.unanswered(); }
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let s = Session::new(store, client, "drv", conn).await.unwrap();
        assert_eq!(s.root(), root);
        assert_eq!(s.readdir(root, None, 100).await.unwrap(), rows);
        assert_eq!(s.lookup(root, "name").await.unwrap().size, 20);
    }

    #[test]
    fn filesystem_errors_use_native_errno_and_preserve_disk_full() {
        for (e, errno) in [(FsError::NotFound, libc::ENOENT), (FsError::Exists, libc::EEXIST), (FsError::NotEmpty, libc::ENOTEMPTY),
            (FsError::IsDir, libc::EISDIR), (FsError::NotDir, libc::ENOTDIR), (FsError::NoSpace, libc::ENOSPC),
            (FsError::ReadOnly, libc::EROFS), (FsError::Unsupported, libc::EOPNOTSUPP), (FsError::Stale, libc::ESTALE),
            (FsError::Offline, libc::ENETDOWN), (FsError::Permission, libc::EACCES), (FsError::InvalidName, libc::EINVAL),
            (FsError::Ambiguous, libc::EILSEQ), (FsError::Again, libc::EAGAIN), (FsError::Io("test".into()), libc::EIO)] {
            assert_eq!(e.errno(), errno);
        }
        assert_eq!(FsError::from(Error::from(std::io::Error::from_raw_os_error(libc::ENOSPC))), FsError::NoSpace);
        assert_eq!(FsError::from(Error::Offline), FsError::Offline);
        for (status, expected) in [(404, FsError::NotFound), (403, FsError::Permission), (501, FsError::Unsupported), (412, FsError::Again)] {
            let e = voidfs_sdk::Error::Service(voidfs_sdk::ServiceError { status, code: "test".into(), message: String::new(), request_id: None, current_version_id: None, retry_after: None });
            assert_eq!(sdk_error(&e), expected);
        }
    }
}
