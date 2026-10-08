// SPDX-License-Identifier: Apache-2.0
//! The adapter-independent namespace. Remote names remain byte-exact; equivalent Unicode
//! lookups are accepted only when unambiguous. Open handles read immutable remote snapshots.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use unicode_normalization::UnicodeNormalization;
use voidfs_sdk::{Client, FolderEntry, Kind};

use bytes::Bytes;

use crate::{Cache, Connectivity, Content, Error, Invalidation, Link, Reader, Store};

#[path = "mount_mutations.rs"]
mod mutations;
#[path = "mount_xattrs.rs"]
mod xattrs;
#[path = "mount_stage.rs"]
mod stage;
#[path = "mount_data.rs"]
pub(crate) mod data;
#[path = "mount_notify.rs"]
mod notify;
#[path = "mount_publish.rs"]
pub(crate) mod publication;
pub use mutations::RenameMode;
pub use xattrs::XattrMode;
pub use stage::StagingConfig;
pub use notify::LocalChange;
pub use publication::{Conflict, ConflictSide};

pub type Ino = u64;
pub type Fh = u64;
pub type Result<T> = std::result::Result<T, FsError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sync { Saved, Pending, Saving, Conflict, Error }

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attr {
    pub ino: Ino,
    pub kind: Kind,
    pub size: u64,
    #[serde(with = "stage::mtime")]
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
    #[error("bad file handle")]
    BadHandle,
    #[error("the server can't be reached")]
    Offline,
    #[error("permission denied")]
    Permission,
    #[error("invalid name")]
    InvalidName,
    #[error("invalid argument")]
    InvalidArgument,
    #[error("extended attribute not found")]
    NoAttr,
    #[error("extended attributes exceed the size limit")]
    TooLarge,
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
            Self::InvalidArgument => libc::EINVAL, Self::TooLarge => libc::E2BIG,
            Self::NoAttr => {
                #[cfg(target_os = "macos")] { libc::ENOATTR }
                #[cfg(not(target_os = "macos"))] { libc::ENODATA }
            },
            Self::Ambiguous => libc::EILSEQ, Self::Again => libc::EAGAIN, Self::BadHandle => libc::EBADF, Self::Io(_) => libc::EIO,
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

impl From<rusqlite::Error> for FsError {
    fn from(e: rusqlite::Error) -> Self { Error::from(e).into() }
}

impl From<std::io::Error> for FsError {
    fn from(e: std::io::Error) -> Self { Error::from(e).into() }
}

fn sdk_error(e: &voidfs_sdk::Error) -> FsError {
    match e.status() {
        Some(404) => FsError::NotFound, Some(401 | 403) => FsError::Permission,
        Some(501) => FsError::Unsupported, Some(409 | 412) => FsError::Again,
        _ => match e { voidfs_sdk::Error::Transport { .. } => FsError::Offline, _ => FsError::Io(e.to_string()) },
    }
}

struct Node { ino: Ino, entry: FolderEntry, generation: u64, sync: Sync, entry_id: Option<i64>, remote_key: Option<String> }

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
    c.query_row("SELECT attrs, generation, sync, entry_id, remote_key FROM mount_inodes WHERE ino=?1 AND drive=?2", params![ino, drive], |r| {
        let sync: String = r.get(2)?;
        let sync = match sync.as_str() {
            "saved" => Sync::Saved, "pending" => Sync::Pending, "saving" => Sync::Saving, "conflict" => Sync::Conflict, "error" => Sync::Error,
            _ => return Err(rusqlite::Error::InvalidQuery),
        };
        Ok(Node { ino, entry: decode_json(&r.get::<_, String>(0)?)?, generation: r.get(1)?, sync, entry_id: r.get(3)?, remote_key: r.get(4)? })
    }).optional()
}

fn names(c: &Connection, parent: Ino) -> rusqlite::Result<Vec<(String, Ino)>> {
    let mut q = c.prepare("SELECT name, ino FROM mount_overlay WHERE parent=?1 AND ino IS NOT NULL
        UNION ALL SELECT name, ino FROM mount_names n WHERE parent=?1 AND NOT EXISTS
        (SELECT 1 FROM mount_overlay o WHERE o.parent=n.parent AND o.name=n.name) ORDER BY name")?;
    q.query_map([parent], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

const EQUIVALENT_NAMES: &str = "SELECT ino FROM mount_overlay INDEXED BY mount_overlay_nfc WHERE parent=?1 AND nfc=?2 AND ino IS NOT NULL
        UNION ALL SELECT ino FROM mount_names n INDEXED BY mount_names_nfc WHERE parent=?1 AND nfc=?2 AND NOT EXISTS
        (SELECT 1 FROM mount_overlay o WHERE o.parent=n.parent AND o.name=n.name) LIMIT 2";

fn equivalent_names(c: &Connection, parent: Ino, nfc: &str) -> rusqlite::Result<Vec<Ino>> {
    let mut q = c.prepare(EQUIVALENT_NAMES)?;
    q.query_map(params![parent, nfc], |r| r.get(0))?.collect()
}

const NAMED_CHILD: &str = "SELECT ino FROM mount_overlay WHERE parent=?1 AND name=?2
        UNION ALL SELECT ino FROM mount_names n WHERE parent=?1 AND name=?2 AND NOT EXISTS
        (SELECT 1 FROM mount_overlay o WHERE o.parent=n.parent AND o.name=n.name) LIMIT 1";

fn named_child(c: &Connection, parent: Ino, name: &str) -> rusqlite::Result<Option<Ino>> {
    c.query_row(NAMED_CHILD,
        params![parent, name], |r| r.get::<_, Option<Ino>>(0)).optional().map(Option::flatten)
}

const DESCENDANTS: &str = "WITH RECURSIVE children(ino) AS (
        SELECT ?1 UNION SELECT o.ino FROM mount_overlay o JOIN children p ON o.parent=p.ino WHERE o.ino IS NOT NULL
        UNION SELECT n.ino FROM mount_names n JOIN children p ON n.parent=p.ino WHERE NOT EXISTS
        (SELECT 1 FROM mount_overlay o WHERE o.parent=n.parent AND o.name=n.name)) SELECT ino FROM children";

fn descendants(c: &Connection, ino: Ino) -> rusqlite::Result<Vec<Ino>> {
    let mut q = c.prepare(DESCENDANTS)?;
    q.query_map([ino], |r| r.get(0))?.collect()
}

fn affected_inodes(c: &Connection, drive: &str, root: Ino, changes: &[Invalidation]) -> rusqlite::Result<BTreeSet<Ino>> {
    let mut affected = BTreeSet::new();
    for change in changes {
        let (key, subtree) = match change {
            Invalidation::All => {
                let mut q = c.prepare("SELECT ino FROM mount_inodes WHERE drive=?1")?;
                affected.extend(q.query_map([drive], |r| r.get::<_, Ino>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
                continue;
            }
            Invalidation::Object(key) => (key, false), Invalidation::Subtree(key) => (key, true),
        };
        let mut bases = vec![key.clone()];
        bases.extend(key.match_indices('/').map(|(end, _)| key[..=end].to_owned()));
        let prefix = if key.is_empty() { String::new() } else { format!("{}/", key.trim_end_matches('/')) };
        let end = prefix.strip_suffix('/').map(|p| format!("{p}0"));
        let mut q = c.prepare("SELECT ino FROM mount_inodes INDEXED BY mount_pending_remote_key
            WHERE drive=?1 AND sync<>'saved' AND remote_key IN (SELECT value FROM json_each(?2))
            UNION SELECT ino FROM mount_inodes INDEXED BY mount_pending_remote_key
            WHERE drive=?1 AND sync<>'saved' AND ?3 AND remote_key>=?4 AND (?5 IS NULL OR remote_key<?5)")?;
        let origins = q.query_map(params![drive, serde_json::to_string(&bases).expect("JSON"), subtree, prefix, end], |r| r.get::<_, Ino>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for ino in origins {
            affected.extend(descendants(c, ino)?);
            if let Some(path) = chain(c, drive, root, ino)? { affected.extend(path.into_iter().map(|(at, _)| at)); }
        }
        let mut at = root;
        affected.insert(root);
        let mut components = key.split('/').peekable();
        while let Some(name) = components.next() {
            if name.is_empty() {
                if components.peek().is_none() && subtree { affected.extend(descendants(c, at)?); }
                break;
            }
            let Some(ino) = named_child(c, at, name)? else { break; };
            let directory = c.query_row("SELECT 1 FROM mount_dirs WHERE ino=?1", [ino], |_| Ok(())).optional()?.is_some();
            if components.peek().is_none() {
                if !directory { affected.insert(ino); }
                break;
            }
            if !directory { break; }
            affected.insert(ino);
            at = ino;
        }
    }
    Ok(affected)
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

fn mutation_node(c: &Connection, drive: &str, root: Ino, ino: Ino, offline: bool) -> Result<(Node, String)> {
    if ino == 0 || ino > i64::MAX as u64 { return Err(FsError::Stale); }
    let path = chain(c, drive, root, ino)?.ok_or(FsError::Stale)?;
    for (dir, _) in path.iter().take(path.len() - 1) { mutation_listing(c, drive, root, *dir, offline)?; }
    let n = node(c, drive, ino)?.ok_or(FsError::Stale)?;
    let mut key = path.into_iter().skip(1).map(|(_, name)| name).collect::<Vec<_>>().join("/");
    if n.entry.kind == Kind::Folder && !key.is_empty() { key.push('/'); }
    Ok((n, key))
}

fn mutation_listing(c: &Connection, drive: &str, root: Ino, dir: Ino, offline: bool) -> Result<()> {
    let n = node(c, drive, dir)?.ok_or(FsError::Stale)?;
    if n.entry.kind != Kind::Folder { return Err(FsError::NotDir); }
    let (listed, seq): (bool, Option<u64>) = c.query_row("SELECT listed, seq FROM mount_dirs WHERE ino=?1", [dir], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let local = dir != root && n.entry.object_id.is_empty();
    if offline && seq.is_none() && !local { return Err(FsError::Offline); }
    if !offline && !listed { return Err(FsError::Again); }
    Ok(())
}

fn mutation_base(n: &Node) -> Result<crate::journal::StoredBase> {
    if let Some(id) = n.entry_id { return Ok(crate::journal::StoredBase::Entry(id)); }
    n.entry.version_id.clone().filter(|s| !s.is_empty()).map(crate::journal::StoredBase::Version).ok_or(FsError::Again)
}

fn save_node(c: &Connection, n: &Node) -> Result<()> {
    let sync = match n.sync { Sync::Saved => "saved", Sync::Pending => "pending", Sync::Saving => "saving", Sync::Conflict => "conflict", Sync::Error => "error" };
    c.execute("UPDATE mount_inodes SET attrs=?2, generation=?3, sync=?4 WHERE ino=?1", params![n.ino, serde_json::to_string(&n.entry).expect("JSON"), n.generation, sync])?;
    Ok(())
}

fn remote_path(c: &Connection, drive: &str, root: Ino, ino: Ino) -> Result<String> {
    let path = chain(c, drive, root, ino)?.ok_or(FsError::Stale)?;
    for (index, (at, _)) in path.iter().enumerate().rev() {
        if let Some(n) = node(c, drive, *at)? && let Some(key) = n.remote_key {
            let suffix = path.iter().skip(index + 1).map(|(_, name)| name.as_str()).collect::<Vec<_>>().join("/");
            let mut key = if suffix.is_empty() { key } else { format!("{}{suffix}", key) };
            if node(c, drive, ino)?.is_some_and(|n| n.entry.kind == Kind::Folder) && !key.ends_with('/') { key.push('/'); }
            return Ok(key);
        }
    }
    let mut key = path.into_iter().skip(1).map(|(_, name)| name).collect::<Vec<_>>().join("/");
    if !key.is_empty() && node(c, drive, ino)?.is_some_and(|n| n.entry.kind == Kind::Folder) { key.push('/'); }
    Ok(key)
}

struct Handle {
    attr: Attr, reader: tokio::sync::Mutex<Reader>, write: bool, closed: std::sync::atomic::AtomicBool,
    frozen: Mutex<Vec<Arc<data::File>>>, published: Mutex<Option<(Attr, String)>>, local_attr: Mutex<Option<Attr>>,
}
struct ReadResolution { budget: crate::mount_resolve::Budget, remaining: Duration, failed: HashSet<String> }
impl Default for ReadResolution {
    fn default() -> Self { Self { budget: Default::default(), remaining: Duration::from_secs(2), failed: HashSet::new() } }
}
struct Closing<'a> { handle: &'a Handle, complete: bool }
impl Drop for Closing<'_> {
    fn drop(&mut self) { if !self.complete { self.handle.closed.store(false, std::sync::atomic::Ordering::Release); } }
}
struct Handles { next: u64, entries: std::collections::HashMap<Fh, Arc<Handle>> }

/// One drive's persistent namespace. The caller supplies its feed invalidations; opening a
/// session distrusts persisted metadata online and retains complete snapshots for offline use.
/// `drive` must be a stable canonical drive identifier, rather than a reusable display alias.
/// Opens bind immutable remote versions. Writable namespaces durably queue guarded mutations.
pub struct Session {
    store: Arc<Store>, client: Client, drive: String, root: Ino, connectivity: Connectivity,
    cache: Cache, generation: u32, handles: Mutex<Handles>,
    refresh: Mutex<HashMap<Ino, Weak<tokio::sync::Mutex<()>>>>,
    queue: Option<crate::Queue>,
    data: Arc<data::Staged>,
    local: Arc<notify::Observer>,
}

impl Session {
    pub async fn new(store: Arc<Store>, client: Client, cache: Cache, drive: &str, connectivity: Connectivity) -> Result<Self> {
        let lease = store.mount_session(drive, false).ok_or(FsError::Again)?;
        Self::new_inner(store, client, cache, drive, connectivity, lease).await
    }

    async fn new_inner(store: Arc<Store>, client: Client, cache: Cache, drive: &str, connectivity: Connectivity, lease: Arc<crate::store::MountLease>) -> Result<Self> {
        let d = drive.to_owned();
        let s = store.clone();
        let (root, generation) = tokio::task::spawn_blocking(move || s.with(|c| {
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
            let generation: u32 = tx.query_row("INSERT INTO meta(key, value) VALUES ('mount_session', '1')
                ON CONFLICT(key) DO UPDATE SET value=CAST(value AS INTEGER)+1 WHERE CAST(value AS INTEGER)<4294967295
                RETURNING value", [], |r| r.get::<_, String>(0)?.parse().map_err(|_| rusqlite::Error::InvalidQuery))?;
            tx.commit()?;
            Ok((root, generation))
        })).await.map_err(Error::from)??;
        let local = Arc::new(notify::Observer::default());
        let data = data::Staged::load(store.clone(), drive.to_owned(), root, None, StagingConfig::default(), lease, local.clone()).await?;
        Ok(Self { store, client, drive: drive.to_owned(), root, connectivity, cache, generation,
            handles: Mutex::new(Handles { next: 1, entries: Default::default() }), refresh: Mutex::new(HashMap::new()), queue: None, data, local })
    }

    /// A writable session on the daemon's queue and state store. One writer owns each drive;
    /// adapters share that session so their handles see the same local bytes.
    pub async fn new_writable(store: Arc<Store>, client: Client, cache: Cache, queue: crate::Queue, drive: &str, connectivity: Connectivity) -> Result<Self> {
        Self::new_writable_with_config(store, client, cache, queue, drive, connectivity, StagingConfig::default()).await
    }

    pub async fn new_writable_with_config(store: Arc<Store>, client: Client, cache: Cache, queue: crate::Queue, drive: &str, connectivity: Connectivity, cfg: StagingConfig) -> Result<Self> {
        if !queue.uses_store(&store) { return Err(FsError::InvalidArgument); }
        let lease = store.mount_session(drive, true).ok_or(FsError::Again)?;
        let mut session = Self::new_inner(store, client, cache, drive, connectivity, lease.clone()).await?;
        let registration = queue.mount_registration(drive).await;
        session.data = data::Staged::load(session.store.clone(), drive.to_owned(), session.root, Some(queue.clone()), cfg, lease, session.local.clone()).await?;
        queue.register_mount_publisher(drive, Arc::downgrade(&session.data));
        drop(registration);
        session.queue = Some(queue);
        session.data.restart_timers();
        Ok(session)
    }

    pub fn root(&self) -> Ino { self.root }

    /// Durable session epoch for adapters' reconnect handshakes. Handles use its high 32 bits.
    pub fn generation(&self) -> u32 { self.generation }

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
        let normalized: String = name.nfc().collect();
        let candidates = self.db(move |c, _, _| equivalent_names(c, parent, &normalized)).await?;
        if candidates.len() > 1 { return Err(FsError::Ambiguous); }
        let ino = candidates.first().copied().ok_or(FsError::NotFound)?;
        self.cached_attr(ino).await
    }

    pub async fn getattr(&self, ino: Ino) -> Result<Attr> {
        self.ancestors(ino).await?;
        self.cached_attr(ino).await
    }

    async fn cached_attr(&self, ino: Ino) -> Result<Attr> {
        self.db(move |c, d, _| node(c, d, ino)).await?.ok_or(FsError::Stale)?.attr()
    }

    async fn prepare_mutation(&self, ino: Ino) -> Result<()> {
        for _ in 0..4 {
            self.ancestors(ino).await?;
            let offline = self.connectivity.link() == Link::Offline;
            let state = self.db(move |c, d, root| {
                let tx = c.transaction()?;
                let state: Result<(Node, String)> = (|| {
                    let (mut n, key) = mutation_node(&tx, d, root, ino, offline)?;
                    if n.remote_key.is_none() && !n.entry.object_id.is_empty() {
                        let remote = remote_path(&tx, d, root, ino)?;
                        tx.execute("UPDATE mount_inodes SET remote_key=?2, generation=generation+1 WHERE ino=?1", params![ino, remote])?;
                        n.remote_key = Some(remote);
                        n.generation += 1;
                    }
                    Ok((n, key))
                })();
                if state.is_ok() { tx.commit()?; }
                Ok(state)
            }).await??;
            let (n, key) = state;
            if ino == self.root || n.entry_id.is_some() || n.entry.version_id.as_ref().is_some_and(|v| !v.is_empty()) { return Ok(()); }
            if offline { return Err(FsError::Offline); }
            let base_key = n.remote_key.as_deref().unwrap_or(&key);
            let attrs = match self.client.attributes(&self.drive, base_key, Default::default()).await {
                Err(e) if e.status() == Some(404) && base_key != key => self.client.attributes(&self.drive, &key, Default::default()).await,
                result => result,
            }.map_err(|e| sdk_error(&e))?;
            if attrs.object_id != n.entry.object_id || attrs.kind != n.entry.kind { return Err(FsError::Again); }
            let Some(version) = attrs.version_id.filter(|s| !s.is_empty()) else { return Err(FsError::Unsupported); };
            let accepted = self.db(move |c, d, root| {
                let tx = c.transaction()?;
                let state = (|| {
                    let (mut current, current_key) = mutation_node(&tx, d, root, ino, offline)?;
                    if current.generation != n.generation || current_key != key { return Err(FsError::Again); }
                    current.entry.version_id = Some(version);
                    current.entry.mtime = attrs.mtime;
                    current.entry.mode = attrs.mode;
                    current.entry.has_xattrs = !attrs.xattrs.is_empty();
                    current.generation += 1;
                    current.attr()?;
                    save_node(&tx, &current)?;
                    Ok(())
                })();
                if state.is_ok() { tx.commit()?; }
                Ok(state)
            }).await?;
            match accepted { Err(FsError::Again) => continue, other => return other }
        }
        Err(FsError::Again)
    }

    /// Binds attributes and content from one accepted namespace snapshot. IDs are never reused.
    pub async fn open(&self, ino: Ino, write: bool) -> Result<Fh> {
        if write {
            if self.queue.is_none() { return Err(FsError::ReadOnly); }
            self.prepare_mutation(ino).await?;
        }
        for _ in 0..4 {
            self.ancestors(ino).await?;
            let _opening = self.data.opening().await;
            let offline = self.connectivity.link() == Link::Offline;
            let snapshot = self.db(move |c, d, root| {
                let tx = c.transaction()?;
                let Some(path) = chain(&tx, d, root, ino)? else { return Ok(Err(FsError::Stale)); };
                for (dir, _) in path.iter().take(path.len() - 1) {
                    let (listed, seq): (bool, Option<u64>) = tx.query_row("SELECT listed, seq FROM mount_dirs WHERE ino=?1", [dir], |r| Ok((r.get(0)?, r.get(1)?)))?;
                    if offline && seq.is_none() && (*dir == root || node(&tx, d, *dir)?.is_none_or(|n| !n.entry.object_id.is_empty())) { return Ok(Err(FsError::Offline)); }
                    if !offline && !listed { return Ok(Err(FsError::Again)); }
                }
                let n = node(&tx, d, ino)?.ok_or(rusqlite::Error::InvalidQuery)?;
                let key = remote_path(&tx, d, root, ino).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                let persisted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM mount_staged WHERE ino=?1)", [ino], |r| r.get(0))?;
                Ok(Ok((n, key, persisted)))
            }).await?;
            let (n, key, persisted) = match snapshot { Err(FsError::Again) => continue, other => other? };
            let attr = n.attr()?;
            match attr.kind { Kind::Folder => return Err(FsError::IsDir), Kind::File => {}, _ => return Err(FsError::Unsupported) }
            self.data.retire_saved(ino, &attr, persisted).await;
            let (base, key, published) = match self.data.file(ino) { Some(file) => { let state = file.state.lock().await; (state.record.base.clone(), state.record.key.clone(), Some(file.guard.lock().unwrap_or_else(|p| p.into_inner()).clone())) }, None => (attr.clone(), key, None) };
            let local = base.object_id.is_none() && n.sync != Sync::Saved;
            let version_id = if local { format!("local:{}", attr.ino) } else { base.version_id.clone().filter(|s| !s.is_empty()).ok_or_else(|| FsError::Io("missing snapshot version".into()))? };
            let etag = if local { format!("local:{}", attr.ino) } else { base.etag.clone().filter(|s| !s.is_empty()).ok_or_else(|| FsError::Io("missing snapshot ETag".into()))? };
            let content = Content { drive: self.drive.clone(), key, version_id, etag, size: base.size };
            let reader = self.cache.reader(content).with_connectivity(self.connectivity.clone());
            let mut handles = self.handles.lock().unwrap_or_else(|p| p.into_inner());
            if handles.next > u32::MAX as u64 { return Err(FsError::Io("file handle space exhausted".into())); }
            let fh = (u64::from(self.generation) << 32) | handles.next;
            handles.next += 1;
            let handle = Arc::new(Handle { attr: base, reader: tokio::sync::Mutex::new(reader), write, closed: std::sync::atomic::AtomicBool::new(false), frozen: Mutex::new(Vec::new()), published: Mutex::new(published), local_attr: Mutex::new(None) });
            self.data.register(&handle);
            handles.entries.insert(fh, handle);
            return Ok(fh);
        }
        Err(FsError::Again)
    }

    fn check_handle(&self, fh: Fh) -> Result<()> {
        let generation = fh >> 32;
        if generation > 0 && generation < u64::from(self.generation) { return Err(FsError::Stale); }
        if generation != u64::from(self.generation) { return Err(FsError::BadHandle); }
        Ok(())
    }

    fn handle(&self, fh: Fh) -> Result<Arc<Handle>> {
        self.check_handle(fh)?;
        self.handles.lock().unwrap_or_else(|p| p.into_inner()).entries.get(&fh).cloned().ok_or(FsError::BadHandle)
    }

    /// The attributes bound at open, independent of later namespace changes.
    pub fn handle_attr(&self, fh: Fh) -> Result<Attr> {
        let handle = self.handle(fh)?;
        let mut attr = handle.attr.clone();
        let local = match self.data.file(attr.ino) {
            Some(file) => Some(file.attr.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            None => handle.local_attr.lock().unwrap_or_else(|p| p.into_inner()).clone().or_else(|| handle.frozen.lock().unwrap_or_else(|p| p.into_inner()).last().map(|file| file.attr.lock().unwrap_or_else(|p| p.into_inner()).clone())),
        };
        if let Some(local) = local {
            attr.size = local.size;
            attr.mtime = local.mtime;
            attr.generation = local.generation;
            attr.sync = local.sync;
        }
        Ok(attr)
    }

    async fn snapshot_key(&self, attr: &Attr, failed: &HashSet<String>, budget: &mut crate::mount_resolve::Budget) -> Result<String> {
        let id = attr.object_id.as_deref().ok_or(FsError::Stale)?;
        match self.ancestors(attr.ino).await {
            Ok(()) => {
                let ino = attr.ino;
                let id = id.to_owned();
                let key = self.db(move |c, d, root| {
                    let tx = c.transaction()?;
                    let Some(path) = chain(&tx, d, root, ino)? else { return Ok(None); };
                    for (dir, _) in path.iter().take(path.len() - 1) {
                        let listed: bool = tx.query_row("SELECT listed FROM mount_dirs WHERE ino=?1", [dir], |r| r.get(0))?;
                        if !listed { return Ok(None); }
                    }
                    if node(&tx, d, ino)?.is_none_or(|n| n.entry.object_id != id) { return Ok(None); }
                    Ok(Some(path.into_iter().skip(1).map(|(_, name)| name).collect::<Vec<_>>().join("/")))
                }).await?;
                if let Some(key) = key && !failed.contains(&key) { return Ok(key); }
            }
            Err(FsError::Stale | FsError::NotFound | FsError::Again) => {},
            Err(e) => return Err(e),
        }
        let key = crate::mount_resolve::locate(&self.client, &self.drive, id, &self.connectivity, budget).await?.ok_or(FsError::Stale)?;
        if failed.contains(&key) { return Err(FsError::Stale); }
        Ok(key)
    }

    pub async fn read(&self, fh: Fh, offset: u64, len: u64) -> Result<Bytes> {
        let handle = self.handle(fh)?;
        let mut resolution = ReadResolution::default();
        let opening = self.data.opening().await;
        let frozen = handle.frozen.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let file = self.data.file(handle.attr.ino);
        let state = match file { Some(file) => Some(file.state.clone().lock_owned().await), None => None };
        drop(opening);
        if state.is_some() || !frozen.is_empty() {
            let mut view = data::View::new(handle.attr.size);
            for file in &frozen { view.overlay(&*file.state.lock().await); }
            if let Some(state) = &state { view.overlay(state); }
            if offset >= view.size || len == 0 { return Ok(Bytes::new()); }
            let len = len.min(view.size - offset);
            let mut body = vec![0u8; usize::try_from(len).map_err(|_| FsError::InvalidArgument)?];
            let end = offset + len;
            let mut at = offset;
            for extent in &view.extents {
                if extent.end <= offset || extent.start >= end { continue; }
                let start = extent.start.max(offset);
                if at < start { self.read_gap(&handle, at, start.min(view.remote_size), offset, &mut body, &mut resolution).await?; }
                let stop = extent.end.min(end);
                let path = extent.path.clone();
                let physical = extent.physical + start - extent.start;
                let bytes = tokio::task::spawn_blocking(move || stage::read_range(&path, physical, stop - start)).await.map_err(Error::from)??;
                body[(start - offset) as usize..(stop - offset) as usize].copy_from_slice(&bytes);
                at = stop;
            }
            if at < end { self.read_gap(&handle, at, end.min(view.remote_size), offset, &mut body, &mut resolution).await?; }
            return Ok(Bytes::from(body));
        }
        self.read_remote(&handle, offset, len, &mut resolution).await
    }

    async fn read_gap(&self, handle: &Handle, start: u64, end: u64, offset: u64, body: &mut [u8], resolution: &mut ReadResolution) -> Result<()> {
        let end = end.min(handle.attr.size);
        if start < end {
            let bytes = self.read_remote(handle, start, end - start, resolution).await?;
            body[(start - offset) as usize..(end - offset) as usize].copy_from_slice(&bytes);
        }
        Ok(())
    }

    async fn read_remote(&self, handle: &Handle, offset: u64, len: u64, resolution: &mut ReadResolution) -> Result<Bytes> {
        let mut reader = handle.reader.lock().await;
        if offset >= handle.attr.size || len == 0 { return Ok(Bytes::new()); }
        let len = len.min(handle.attr.size - offset);
        for _ in 0..4 {
            match reader.read(offset, len).await {
                Ok(bytes) => return Ok(bytes),
                Err(e) => {
                    let missing = matches!(&e, Error::Fetch(e) if e.status() == Some(404));
                    if missing || matches!(e, Error::Changed { .. }) {
                        if !resolution.failed.insert(reader.content().key.clone()) { return Err(if missing { FsError::Stale } else { e.into() }); }
                        let start = tokio::time::Instant::now();
                        let key = tokio::time::timeout(resolution.remaining, self.snapshot_key(&handle.attr, &resolution.failed, &mut resolution.budget)).await.map_err(|_| FsError::Again)?;
                        resolution.remaining = resolution.remaining.saturating_sub(start.elapsed());
                        match key {
                            Ok(key) => { reader.relocate(key); continue; }
                            Err(error) if missing || error == FsError::Again => return Err(error),
                            Err(_) => {},
                        }
                        eprintln!("voidfs mount snapshot read: {e}");
                    }
                    return Err(e.into());
                }
            }
        }
        Err(FsError::Again)
    }

    /// A read already in progress retains its handle; later calls see EBADF.
    pub async fn close(&self, fh: Fh) -> Result<()> {
        let handle = self.handle(fh)?;
        if handle.closed.swap(true, std::sync::atomic::Ordering::AcqRel) { return Err(FsError::BadHandle); }
        let mut closing = Closing { handle: &handle, complete: false };
        if handle.write && let Some(file) = self.data.file(handle.attr.ino) {
            self.data.flush(handle.attr.ino, file).await?;
        }
        self.handles.lock().unwrap_or_else(|p| p.into_inner()).entries.remove(&fh).ok_or(FsError::BadHandle)?;
        closing.complete = true;
        Ok(())
    }

    pub async fn readlink(&self, ino: Ino) -> Result<String> {
        let attr = self.getattr(ino).await?;
        if attr.kind != Kind::Symlink { return Err(FsError::InvalidName); }
        attr.target.ok_or_else(|| FsError::Io("missing symlink target".into()))
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
        let lock = {
            let mut locks = self.refresh.lock().unwrap_or_else(|p| p.into_inner());
            locks.retain(|_, lock| lock.strong_count() > 0);
            let lock = locks.get(&dir).and_then(Weak::upgrade).unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
            locks.insert(dir, Arc::downgrade(&lock));
            lock
        };
        let _refresh = lock.lock().await;
        for _ in 0..4 {
            let state = self.db(move |c, d, root| {
                let Some(n) = node(c, d, dir)? else { return Ok(None); };
                let Some(path) = chain(c, d, root, dir)? else { return Ok(None); };
                let (generation, listed, seq): (u64, bool, Option<u64>) = c.query_row("SELECT generation, listed, seq FROM mount_dirs WHERE ino=?1", [dir], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?.unwrap_or((0, false, None));
                let key = path.into_iter().skip(1).map(|(_, n)| format!("{n}/")).collect::<String>();
                let remote = remote_path(c, d, root, dir).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(Some((n.entry.kind, key, remote, generation, listed, seq, n.entry.object_id, n.sync)))
            }).await?.ok_or(FsError::Stale)?;
            let (kind, key, mut remote, generation, listed, seq, object_id, sync) = state;
            if kind != Kind::Folder { return Err(FsError::NotDir); }
            if listed { return Ok(()); }
            if dir != self.root && object_id.is_empty() {
                let accepted = self.db(move |c, _, _| c.execute("UPDATE mount_dirs SET listed=1, generation=generation+1 WHERE ino=?1 AND generation=?2", params![dir, generation])).await?;
                if accepted == 1 { return Ok(()); }
                continue;
            }
            if self.connectivity.link() == Link::Offline {
                return if seq.is_some() { Ok(()) } else { Err(FsError::Offline) };
            }
            let bound_version = if sync != Sync::Saved && dir != self.root {
                let matches = |attrs: &voidfs_sdk::Attributes| attrs.kind == Kind::Folder && attrs.object_id == object_id;
                let attrs = match self.client.attributes(&self.drive, &remote, Default::default()).await {
                    Ok(attrs) if matches(&attrs) => attrs,
                    result if remote != key && (result.as_ref().is_ok_and(|attrs| !matches(attrs)) || result.as_ref().is_err_and(|e| e.status() == Some(404))) => {
                        let attrs = self.client.attributes(&self.drive, &key, Default::default()).await.map_err(|e| sdk_error(&e))?;
                        if !matches(&attrs) { return Err(FsError::Again); }
                        remote = key.clone();
                        attrs
                    }
                    Ok(_) => return Err(FsError::Again),
                    Err(e) => return Err(sdk_error(&e)),
                };
                Some(attrs.version_id)
            } else { None };
            let listing = match self.listing(&remote).await {
                Err(FsError::NotFound) if remote != key => { remote = key.clone(); self.listing(&remote).await },
                result => result,
            }?;
            let Some((entries, seq)) = listing else { continue; };
            if let Some(version) = bound_version {
                match self.client.attributes(&self.drive, &remote, Default::default()).await {
                    Ok(attrs) if attrs.kind == Kind::Folder && attrs.object_id == object_id && attrs.version_id == version => {},
                    Ok(_) => continue,
                    Err(e) if e.status() == Some(404) => continue,
                    Err(e) => return Err(sdk_error(&e)),
                }
            }
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
                    let remote_key = format!("{remote}{}", e.name);
                    tx.execute("INSERT INTO mount_inodes(drive, object_id, attrs, remote_key) VALUES (?1, ?2, ?3, ?4)
                        ON CONFLICT(drive, object_id) DO UPDATE SET generation=generation+(attrs<>excluded.attrs OR remote_key IS NOT excluded.remote_key), attrs=excluded.attrs, remote_key=excluded.remote_key
                        WHERE sync='saved'", params![d, e.object_id, json, remote_key])?;
                    let ino: Ino = tx.query_row("SELECT ino FROM mount_inodes WHERE drive=?1 AND object_id=?2", params![d, e.object_id], |r| r.get(0))?;
                    tx.execute("DELETE FROM mount_names WHERE ino=?1", [ino])?;
                    let name = e.name.trim_end_matches('/');
                    let nfc: String = name.nfc().collect();
                    tx.execute("INSERT INTO mount_names(parent, name, ino, nfc) VALUES (?1, ?2, ?3, ?4)", params![dir, name, ino, nfc])?;
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
            Node { ino: 0, entry: e.clone(), generation: 0, sync: Sync::Saved, entry_id: None, remote_key: None }.attr()?;
        }
        Ok(Some((entries, seq)))
    }

    /// Persists invalidation and returns known affected inodes, including containing directories.
    /// This does not take the refresh mutex: an in-flight listing must lose its generation race.
    pub async fn invalidate(&self, changes: &[Invalidation]) -> Result<Vec<Ino>> {
        let changes = changes.to_vec();
        self.db(move |c, d, root| {
            let tx = c.transaction()?;
            let affected = affected_inodes(&tx, d, root, &changes)?;
            for ino in &affected {
                tx.execute("UPDATE mount_dirs SET listed=0, generation=generation+1 WHERE ino=?1", [ino])?;
                tx.execute("UPDATE mount_inodes SET generation=generation+1 WHERE ino=?1", [ino])?;
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

    #[test]
    fn namespace_queries_search_the_name_and_parent_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.with(|c| {
            let mut q = c.prepare(&format!("EXPLAIN QUERY PLAN {EQUIVALENT_NAMES}"))?;
            let plan = q.query_map(params![1u64, "café"], |r| r.get::<_, String>(3))?.collect::<rusqlite::Result<Vec<_>>>()?.join("\n");
            assert!(plan.contains("mount_names_nfc (parent=? AND nfc=?)"), "{plan}");
            assert!(plan.contains("mount_overlay_nfc (parent=? AND nfc=?)"), "{plan}");
            let mut q = c.prepare(&format!("EXPLAIN QUERY PLAN {NAMED_CHILD}"))?;
            let plan = q.query_map(params![1u64, "file"], |r| r.get::<_, String>(3))?.collect::<rusqlite::Result<Vec<_>>>()?.join("\n");
            assert!(plan.contains("SEARCH n USING PRIMARY KEY (parent=? AND name=?)"), "{plan}");
            let mut q = c.prepare(&format!("EXPLAIN QUERY PLAN {DESCENDANTS}"))?;
            let plan = q.query_map([1u64], |r| r.get::<_, String>(3))?.collect::<rusqlite::Result<Vec<_>>>()?.join("\n");
            assert!(plan.contains("SEARCH n USING PRIMARY KEY (parent=?)"), "{plan}");
            Ok(())
        }).unwrap();
    }

    #[test]
    fn indexed_equivalent_lookup_keeps_byte_exact_overlay_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.with(|c| {
            c.execute("INSERT INTO mount_inodes(ino, drive, attrs) VALUES (1, 'drv', '{}'), (2, 'drv', '{}'), (3, 'drv', '{}'), (4, 'drv', '{}')", [])?;
            c.execute("INSERT INTO mount_names VALUES (1, 'café', 2, 'café'), (1, ?1, 3, 'café')", ["cafe\u{301}"])?;
            assert_eq!(equivalent_names(c, 1, "café")?.len(), 2);
            c.execute("INSERT INTO mount_overlay VALUES (1, 'café', NULL, 'café')", [])?;
            assert_eq!(equivalent_names(c, 1, "café")?, vec![3]);
            assert_eq!(named_child(c, 1, "café")?, None);
            c.execute("UPDATE mount_overlay SET ino=4 WHERE parent=1 AND name='café'", [])?;
            let mut matches = equivalent_names(c, 1, "café")?; matches.sort_unstable();
            assert_eq!(matches, vec![3, 4], "a local spelling collides with a distinct equivalent remote spelling");
            c.execute("INSERT INTO mount_overlay VALUES (1, ?1, NULL, 'café')", ["cafe\u{301}"])?;
            assert_eq!(equivalent_names(c, 1, "café")?, vec![4]);
            Ok(())
        }).unwrap();
    }

    fn legacy_affected(c: &Connection, root: Ino, key: &str) -> rusqlite::Result<BTreeSet<Ino>> {
        let mut q = c.prepare("SELECT ino FROM mount_inodes WHERE drive='drv'")?;
        let inodes = q.query_map([], |r| r.get::<_, Ino>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(q);
        let mut affected = BTreeSet::new();
        for ino in inodes {
            let n = node(c, "drv", ino)?.ok_or(rusqlite::Error::InvalidQuery)?;
            let path = chain(c, "drv", root, ino)?;
            let full_key = path.map(|p| {
                let mut key = p.into_iter().skip(1).map(|(_, n)| n).collect::<Vec<_>>().join("/");
                if n.entry.kind == Kind::Folder && !key.is_empty() { key.push('/'); }
                key
            });
            if full_key.as_ref().is_some_and(|p| p == key || n.entry.kind == Kind::Folder && key.starts_with(p)) { affected.insert(ino); }
        }
        Ok(affected)
    }

    #[test]
    #[ignore = "reproducible PR measurement: cargo test -p voidfs-client --lib namespace_measurement_10k_and_100k -- --ignored --nocapture"]
    fn namespace_measurement_10k_and_100k() {
        fn median(mut samples: Vec<std::time::Duration>) -> f64 { samples.sort_unstable(); samples[samples.len() / 2].as_secs_f64() * 1000.0 }
        for count in [10_000, 100_000] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path()).unwrap();
            store.with(|c| {
                let tx = c.transaction()?;
                let mut folder = entry("", "", 0); folder.kind = Kind::Folder;
                tx.execute("INSERT INTO mount_inodes(ino, drive, attrs) VALUES (1, 'drv', ?1)", [serde_json::to_string(&folder).unwrap()])?;
                tx.execute("INSERT INTO mount_dirs(ino, listed, seq) VALUES (1, 1, 1)", [])?;
                for i in 0..count {
                    let name = format!("file-{i:06}");
                    tx.execute("INSERT INTO mount_inodes(ino, drive, object_id, attrs) VALUES (?1, 'drv', ?2, ?3)",
                        params![i + 2, name, serde_json::to_string(&entry(&name, &name, 8)).unwrap()])?;
                    tx.execute("INSERT INTO mount_names VALUES (1, ?1, ?2, ?1)", params![name, i + 2])?;
                }
                tx.commit()?;
                let key = format!("file-{:06}", count - 1);
                let change = [Invalidation::Object(key.clone())];
                let expected = BTreeSet::from([1, count as u64 + 1]);
                let mut invalidation_before = Vec::new(); let mut invalidation_after = Vec::new();
                let mut lookup_before = Vec::new(); let mut lookup_after = Vec::new();
                for _ in 0..3 {
                    let start = std::time::Instant::now();
                    let tx = c.transaction()?;
                    let affected = legacy_affected(&tx, 1, &key)?;
                    assert_eq!(affected, expected);
                    for ino in affected {
                        tx.execute("UPDATE mount_dirs SET listed=0, generation=generation+1 WHERE ino=?1", [ino])?;
                        tx.execute("UPDATE mount_inodes SET generation=generation+1 WHERE ino=?1", [ino])?;
                    }
                    tx.commit()?;
                    invalidation_before.push(start.elapsed());
                    let start = std::time::Instant::now();
                    let tx = c.transaction()?;
                    let affected = affected_inodes(&tx, "drv", 1, &change)?;
                    assert_eq!(affected, expected);
                    for ino in affected {
                        tx.execute("UPDATE mount_dirs SET listed=0, generation=generation+1 WHERE ino=?1", [ino])?;
                        tx.execute("UPDATE mount_inodes SET generation=generation+1 WHERE ino=?1", [ino])?;
                    }
                    tx.commit()?;
                    invalidation_after.push(start.elapsed());
                    let start = std::time::Instant::now();
                    let normalized: String = key.nfc().collect();
                    let candidates = names(c, 1)?.iter().filter(|(n, _)| n.nfc().eq(normalized.chars())).map(|(_, ino)| *ino).collect::<Vec<_>>();
                    assert_eq!(candidates, vec![count as u64 + 1]);
                    lookup_before.push(start.elapsed());
                    let start = std::time::Instant::now();
                    assert_eq!(equivalent_names(c, 1, &normalized)?, candidates);
                    lookup_after.push(start.elapsed());
                }
                println!("entries={count} invalidation_queries_before={} after=7 invalidation_ms_before={:.3} after={:.3} lookup_ms_before={:.3} after={:.3}",
                    3 * count + 6, median(invalidation_before), median(invalidation_after), median(lookup_before), median(lookup_after));
                Ok(())
            }).unwrap();
        }
    }

    #[tokio::test]
    async fn local_names_and_tombstones_override_the_remote_snapshot_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let conn = Connectivity::default();
        let client = Client::new(Config { endpoint: "http://localhost:9".into(), access_key_id: "test".into(), secret_access_key: "test".into(), ..Default::default() }).unwrap();
        let cache = Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone()).with_connectivity(conn.clone())),
            crate::CacheConfig { min_free_bytes: 0, ..Default::default() }).await.unwrap();
        let s = Session::new(store.clone(), client.clone(), cache, "drv", conn.clone()).await.unwrap();
        let root = s.root();
        let (remote, local) = store.with(|c| {
            for (id, size) in [("remote", 10), ("local", 20)] {
                c.execute("INSERT INTO mount_inodes(drive, object_id, attrs, sync) VALUES ('drv', ?1, ?2, ?3)",
                    params![id, serde_json::to_string(&entry("name", id, size)).unwrap(), if id == "local" { "pending" } else { "saved" }])?;
            }
            let local = c.last_insert_rowid() as Ino;
            let remote = local - 1;
            c.execute("INSERT INTO mount_names VALUES (?1, 'name', ?2, 'name')", params![root, remote])?;
            c.execute("INSERT INTO mount_overlay VALUES (?1, 'name', ?2, 'name')", params![root, local])?;
            c.execute("INSERT INTO mount_overlay VALUES (?1, 'gone', NULL, 'gone')", [root])?;
            let gone = serde_json::to_string(&entry("gone", "gone", 30)).unwrap();
            c.execute("INSERT INTO mount_inodes(drive, object_id, attrs) VALUES ('drv', 'gone', ?1)", [gone])?;
            c.execute("INSERT INTO mount_names VALUES (?1, 'gone', ?2, 'gone')", params![root, c.last_insert_rowid()])?;
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
        let cache = Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone()).with_connectivity(conn.clone())),
            crate::CacheConfig { min_free_bytes: 0, ..Default::default() }).await.unwrap();
        let s = Session::new(store, client, cache, "drv", conn).await.unwrap();
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
