// SPDX-License-Identifier: Apache-2.0
//! The write journal and the upload queue (step 4, item 3).
//!
//! A change is durable on the Mac when [`Queue`]'s call for it returns: its bytes are in a file
//! of the journal's, synced, and its entry is in the state database, synced. The queue then
//! publishes it in the background:
//! - one key's changes in order, and a folder's after what was under it; other keys at once, up
//!   to 16 (SpaceFS's default);
//! - a put with the writes after it as one put, writes alone as one patch;
//! - each guarded by the version it was based on, with the `412` rule ([`crate::publish`]); mount
//!   edits preserve the competing remote version for reconciliation;
//! - pausable and cancellable for everything, a drive, a batch or one entry, and resumed after a
//!   restart, a multipart upload with the parts it had;
//! - within an upload bandwidth limit that applies at once ([`Queue::set_bandwidth`]);
//! - a large file that replaces a version the drive holds most of as a direct upload, which
//!   sends only the shards the pool lacks, straight to the bucket (protocol §4.11).
//!
//! Imports ([`Queue::import`]) are files of the user's, read where they are when they publish,
//! with their modification time, permission bits and extended attributes.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, Semaphore};
use unicode_normalization::UnicodeNormalization;
use voidfs_sdk::{Bandwidth, Client};

use crate::connectivity::{Connectivity, Link};
use crate::error::{Error, Result};
use crate::journal::{self, Attrs, Base, BatchId, Entry, EntryId, Op, Stamp, State, StoredBase};
use crate::publish::{self, Ctx, Guard, Outcome, Stop, Why};
use crate::store::Store;

const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct QueueConfig {
    /// Entries published at once.
    pub uploads: usize,
    pub part_size: u64,
    /// Files from this size go up in parts.
    pub multipart_from: u64,
    /// Parts of one file at once.
    pub parts_at_once: usize,
    /// Files from this size that replace a version of the drive's are planned as direct uploads
    /// (protocol §4.11), and sent that way if the drive holds at least half of them already.
    /// `u64::MAX` turns direct uploads off.
    pub direct_from: u64,
    /// Request bodies held in memory at once, across all uploads.
    pub memory_bytes: u64,
    /// The longest wait before a failed publish is tried again.
    pub retry_max: Duration,
    /// While it says the server can't be reached, nothing is published; the queue goes on once
    /// it can be.
    pub connectivity: Option<Connectivity>,
}

impl Default for QueueConfig {
    fn default() -> QueueConfig {
        QueueConfig {
            uploads: 16,
            part_size: 16 * MIB,
            multipart_from: 64 * MIB,
            parts_at_once: 4,
            direct_from: voidfs_sdk::DIRECT_MIN_BYTES as u64,
            memory_bytes: 256 * MIB,
            retry_max: Duration::from_secs(60),
            connectivity: None,
        }
    }
}

/// What a pause, a resume or a cancel applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Drive(String),
    Batch(BatchId),
    Entry(EntryId),
}

/// A file of the user's to upload, or a folder to make (an empty one, which no file's key would
/// make), with the folder's attributes.
#[derive(Clone, Debug)]
pub struct Import {
    pub path: PathBuf,
    pub drive: String,
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: EntryId,
    pub drive: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_key: Option<String>,
    pub op: Op,
    pub state: State,
    pub paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchId>,
    pub size: u64,
    pub sent: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The version a `412` said was there, which this one was published over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BatchStatus {
    pub id: BatchId,
    pub label: String,
    pub items: u64,
    pub done: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub bytes: u64,
    pub sent: u64,
    pub paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub items: Vec<Item>,
    pub batches: Vec<BatchStatus>,
    pub paused: bool,
    pub paused_drives: Vec<String>,
    /// Bytes a second; `None` is unlimited.
    pub bandwidth: Option<u64>,
    /// Entries not yet published, and their bytes.
    pub unpublished: u64,
    pub unpublished_bytes: u64,
}

struct Running {
    stop: Stop,
    sent: Arc<AtomicU64>,
}

#[derive(Default)]
struct QState {
    /// Entries neither done nor cancelled, by id.
    pending: BTreeMap<EntryId, Entry>,
    /// The runs being published, by every entry in them.
    running: HashMap<EntryId, Arc<Running>>,
    runs: usize,
    retry_at: HashMap<EntryId, Instant>,
    attempts: HashMap<EntryId, u32>,
    /// Entries that were being sent when the client last stopped, or whose answer was lost.
    may_have_landed: HashSet<EntryId>,
    paused_all: bool,
    paused_drives: BTreeSet<String>,
    paused_batches: BTreeSet<BatchId>,
    /// Entries cancelled while they were being published.
    cancelled: HashSet<EntryId>,
    /// The publisher found nothing to start and nothing to retry, and nothing is running.
    idle: bool,
    /// ... because the server couldn't be reached.
    idle_offline: bool,
    closed: bool,
}

impl QState {
    fn paused(&self, e: &Entry) -> bool {
        self.paused_all || e.paused || self.paused_drives.contains(&e.drive) || e.batch.is_some_and(|b| self.paused_batches.contains(&b))
    }
}

struct Inner {
    store: Arc<Store>,
    client: Client,
    bandwidth: Arc<Bandwidth>,
    cfg: QueueConfig,
    /// The journal's own copies of bytes.
    dir: PathBuf,
    state_id: String,
    st: Mutex<QState>,
    wake: Notify,
    changed: Notify,
    memory: Arc<Semaphore>,
    memory_kib: u32,
    /// Whether the server offers direct uploads, as last found out.
    direct: Arc<publish::Offered>,
    /// Set once the publisher has stopped and let go of the queue.
    stopped: Arc<(Mutex<bool>, Notify)>,
    mount_publishers: Mutex<HashMap<String, std::sync::Weak<crate::mount::data::Staged>>>,
    mount_registrations: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

/// The journal and the upload queue. Cloning shares it.
#[derive(Clone)]
pub struct Queue(Arc<Inner>);

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await?
}

/// What the journal in `store` holds unpublished, and the bytes it has left to send, without
/// opening the queue: for a status while no daemon runs.
pub fn unpublished(store: &Store) -> Result<(u64, u64)> {
    let entries = store.with(|c| journal::unfinished(c))?;
    Ok((entries.len() as u64, entries.iter().map(|e| e.size.saturating_sub(e.sent)).sum()))
}

/// Is `key` the folder `prefix` (ending in `/`) or inside it?
fn under(key: &str, prefix: &str) -> bool {
    prefix.ends_with('/') && key.starts_with(prefix)
}

/// Whether entry `a`, earlier, must finish before `b` may start: they touch the same name
/// (including a file replaced by a folder), or one is a folder the other is in. Mount
/// dependencies include equivalent Unicode spellings; ordinary queue keys remain byte-exact.
pub(crate) fn depends(a: &Entry, b: &Entry) -> bool {
    a.drive == b.drive && a.keys().any(|ka| b.keys().any(|kb| {
        let mount = a.mount || b.mount;
        let ka = if mount { Cow::Owned(ka.nfc().collect::<String>()) } else { Cow::Borrowed(ka) };
        let kb = if mount { Cow::Owned(kb.nfc().collect::<String>()) } else { Cow::Borrowed(kb) };
        ka.trim_end_matches('/') == kb.trim_end_matches('/') || under(&ka, &kb) || under(&kb, &ka)
    }))
}

/// Paths the state recorded for its own files (the journal's copies, retained conflict snapshots,
/// staging files) name them under the directory as it was opened then. Reached by another path
/// since (moved, or through a link), they are found again by name, as staging files always were:
/// otherwise startup would take the journal's copies of unpublished bytes for strays.
fn reroot(tx: &rusqlite::Transaction<'_>, dir: &Path) -> rusqlite::Result<()> {
    let here = |sub: &str, recorded: &str| Path::new(recorded).file_name().map(|name| dir.join(sub).join(name).to_string_lossy().into_owned());
    let sources = tx.prepare("SELECT id, source FROM entries WHERE staged=1 AND source IS NOT NULL AND state NOT IN ('done', 'cancelled')")?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, source) in sources {
        if let Some(path) = here("journal", &source).filter(|path| *path != source) { tx.execute("UPDATE entries SET source=?2 WHERE id=?1", rusqlite::params![id, path])?; }
    }
    for (table, column, sub) in [("mount_conflicts", "local_path", "mount-conflicts"), ("mount_conflicts", "remote_path", "mount-conflicts"), ("mount_staged", "path", "mount-stage")] {
        let rows = tx.prepare(&format!("SELECT ino, {column} FROM {table} WHERE {column} IS NOT NULL"))?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (ino, recorded) in rows {
            if let Some(path) = here(sub, &recorded).filter(|path| *path != recorded) { tx.execute(&format!("UPDATE {table} SET {column}=?2 WHERE ino=?1"), rusqlite::params![ino, path])?; }
        }
    }
    Ok(())
}

impl Queue {
    /// Opens the journal in `store`, and starts publishing what it holds. `client` should not
    /// have an upload bandwidth limit of its own: the queue sets its.
    pub async fn open(store: Arc<Store>, client: Client, cfg: QueueConfig) -> Result<Queue> {
        let dir = store.dir().join("journal");
        let (s2, d2) = (store.clone(), dir.clone());
        let (entries, paused_all, drives, batches, bw, state_id) = blocking(move || {
            std::fs::create_dir_all(&d2)?;
            s2.with(|c| {
                let tx = c.transaction()?;
                reroot(&tx, s2.dir())?;
                crate::mount::publication::recover_overlays(&tx)?;
                let ids = tx.prepare("SELECT e.id FROM entries e JOIN mount_inodes n ON n.entry_id=e.id AND n.ino=e.mount_ino
                    WHERE e.mount=1 AND e.state='done' AND e.version IS NOT NULL AND e.published_version IS NULL AND n.sync<>'saved'")?
                    .query_map([], |r| r.get::<_, EntryId>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                for id in ids {
                    let mut e = journal::get(&tx, id)?.ok_or(rusqlite::Error::InvalidQuery)?;
                    e.published_version = e.version.clone();
                    let mut key = e.to_key.clone().unwrap_or_else(|| e.key.clone());
                    if e.op == Op::Folder && !key.ends_with('/') { key.push('/'); }
                    e.published_key = Some(key); e.published_attrs = false; e.state = State::Queued;
                    journal::update(&tx, &e)?;
                }
                tx.commit()
            })?;
            let entries = s2.with(|c| journal::unfinished(c))?;
            let drives: Vec<String> = s2.with(|c| c.prepare("SELECT drive FROM paused_drives")?.query_map([], |r| r.get(0))?.collect())?;
            let batches: Vec<BatchId> = s2.with(|c| c.prepare("SELECT id FROM batches WHERE paused = 1")?.query_map([], |r| r.get(0))?.collect())?;
            let paused_all = s2.meta("paused")?.as_deref() == Some("1");
            let bw = s2.meta("bandwidth")?.and_then(|b| b.parse::<u64>().ok());
            // Staged bytes no unfinished entry names were left by a call that didn't return, or
            // by a finished entry's cleanup that didn't happen.
            let keep: HashSet<PathBuf> = entries.iter().filter(|e| e.staged).filter_map(|e| e.source.clone()).collect();
            for f in std::fs::read_dir(&d2)? {
                let f = f?.path();
                if !keep.contains(&f) {
                    let _ = std::fs::remove_file(&f);
                }
            }
            // Competing snapshots no conflict recorded were captured by a run that stopped.
            let conflicts = s2.dir().join("mount-conflicts");
            if conflicts.is_dir() {
                let kept: HashSet<std::ffi::OsString> = s2.with(|c| c.prepare("SELECT local_path FROM mount_conflicts WHERE local_path IS NOT NULL
                    UNION ALL SELECT remote_path FROM mount_conflicts WHERE remote_path IS NOT NULL")?
                    .query_map([], |r| r.get::<_, String>(0))?.filter_map(|p| p.map(|p| PathBuf::from(p).file_name().map(|n| n.to_owned())).transpose()).collect())?;
                for f in std::fs::read_dir(&conflicts)? {
                    let f = f?.path();
                    if f.is_file() && f.file_name().is_some_and(|n| !kept.contains(n)) {
                        let _ = std::fs::remove_file(&f);
                    }
                }
            }
            Ok((entries, paused_all, drives, batches, bw, s2.id()?))
        })
        .await?;
        let bandwidth = Arc::new(Bandwidth::new(bw));
        let mut config = client.config().clone();
        config.upload_bandwidth = Some(bandwidth.clone());
        let client = Client::new(config)?;
        let mut st = QState { paused_all, paused_drives: drives.into_iter().collect(), paused_batches: batches.into_iter().collect(), ..Default::default() };
        for mut e in entries {
            if e.state == State::Uploading {
                st.may_have_landed.insert(e.id);
                e.state = State::Queued;
            }
            st.pending.insert(e.id, e);
        }
        let memory_kib = (cfg.memory_bytes / 1024).clamp(1, u32::MAX as u64 / 2) as u32;
        let inner = Inner {
            memory: Arc::new(Semaphore::new(memory_kib as usize)),
            memory_kib,
            direct: Arc::default(),
            store,
            client,
            bandwidth,
            cfg,
            dir,
            state_id,
            st: Mutex::new(st),
            wake: Notify::new(),
            changed: Notify::new(),
            stopped: Arc::new((Mutex::new(false), Notify::new())),
            mount_publishers: Mutex::new(HashMap::new()),
            mount_registrations: Mutex::new(HashMap::new()),
        };
        let q = Queue(Arc::new(inner));
        if let Some(conn) = &q.0.cfg.connectivity {
            // The publisher looks again whenever the link changes; this lets go with the queue.
            let mut rx = conn.watch();
            let weak = Arc::downgrade(&q.0);
            tokio::spawn(async move {
                while rx.changed().await.is_ok() {
                    match weak.upgrade() {
                        Some(inner) => inner.wake.notify_one(),
                        None => return,
                    }
                }
            });
        }
        let (q2, stopped) = (q.clone(), q.0.stopped.clone());
        tokio::spawn(async move {
            q2.publisher().await;
            drop(q2);
            *stopped.0.lock().unwrap_or_else(|p| p.into_inner()) = true;
            stopped.1.notify_waiters();
        });
        Ok(q)
    }

    fn st(&self) -> std::sync::MutexGuard<'_, QState> {
        self.0.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    // -----------------------------------------------------------------------------------------
    // The journal: each call returns once its change is durable.

    pub(crate) fn uses_store(&self, store: &Arc<Store>) -> bool {
        Arc::ptr_eq(&self.0.store, store)
    }

    pub(crate) fn register_mount_publisher(&self, drive: &str, publisher: std::sync::Weak<crate::mount::data::Staged>) {
        let mut publishers = self.0.mount_publishers.lock().unwrap_or_else(|p| p.into_inner());
        publishers.retain(|_, publisher| publisher.strong_count() > 0);
        publishers.insert(drive.to_owned(), publisher);
    }

    fn mount_publisher(&self, drive: &str) -> Option<Arc<crate::mount::data::Staged>> {
        self.0.mount_publishers.lock().unwrap_or_else(|p| p.into_inner()).get(drive).and_then(std::sync::Weak::upgrade)
    }

    pub(crate) async fn mount_registration(&self, drive: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self.0.mount_registrations.lock().unwrap_or_else(|p| p.into_inner());
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = gates.get(drive).and_then(std::sync::Weak::upgrade) { gate } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(drive.to_owned(), Arc::downgrade(&gate));
                gate
            }
        };
        gate.lock_owned().await
    }

    /// Freezes one range of a mount's staging file into bytes the journal owns. The caller
    /// holds the inode's mutation lock until its entries adopt this path in mount_transaction.
    pub(crate) async fn mount_copy(&self, source: PathBuf, offset: u64, length: u64) -> crate::mount::Result<PathBuf> {
        let dir = self.0.dir.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Seek, SeekFrom, Write};
            let end = offset.checked_add(length).ok_or(crate::mount::FsError::InvalidArgument)?;
            let mut src = std::fs::File::open(&source)?;
            let source_size = src.metadata()?.len();
            if source_size < end {
                return Err(crate::mount::FsError::Io("a mount snapshot range is past the staging file".into()));
            }
            src.seek(SeekFrom::Start(offset))?;
            let mut raw = [0u8; 12];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut raw)?;
            let path = dir.join(hex::encode(raw));
            let result = (|| {
                let dest = if offset == 0 && length == source_size {
                    if std::fs::copy(&source, &path)? != length {
                        return Err(crate::mount::FsError::Io("a mount snapshot source changed size".into()));
                    }
                    std::fs::File::open(&path)?
                } else {
                    let mut dest = std::fs::File::options().create_new(true).write(true).open(&path)?;
                    let mut left = length;
                    let mut buf = vec![0u8; (length.min(MIB) as usize).max(1)];
                    while left != 0 {
                        let n = left.min(buf.len() as u64) as usize;
                        src.read_exact(&mut buf[..n])?;
                        dest.write_all(&buf[..n])?;
                        left -= n as u64;
                    }
                    dest
                };
                dest.sync_all()?;
                std::fs::File::open(&dir)?.sync_all()?;
                Ok::<_, crate::mount::FsError>(())
            })();
            if let Err(err) = result {
                let _ = std::fs::remove_file(&path);
                return Err(err);
            }
            Ok(path)
        }).await.map_err(crate::Error::from).map_err(crate::mount::FsError::from)?
    }

    /// Makes namespace changes and their journal entries durable together. Holding the queue
    /// lock across the commit keeps the publisher from observing only half of a local edit.
    pub(crate) async fn mount_transaction<T: Send + 'static>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> crate::mount::Result<(T, Vec<Entry>)> + Send + 'static,
    ) -> crate::mount::Result<T> {
        let this = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut st = this.st();
            if st.closed {
                return Err(crate::mount::FsError::Io("the upload queue is closed".into()));
            }
            let committed = this.0.store.with(|c| {
                let tx = c.transaction()?;
                let (value, mut entries) = match f(&tx) {
                    Ok(result) => result,
                    Err(err) => return Ok(Err(err)),
                };
                for i in 0..entries.len() {
                    let (earlier, rest) = entries.split_at_mut(i);
                    let e = &mut rest[0];
                    e.mount = true;
                    // A new inode must check absence even when an older inode occupied this
                    // key. Its ordering dependency is independent of its version guard.
                    if !matches!(e.base, StoredBase::Absent | StoredBase::Entry(_)) {
                        let prev = earlier.iter().rev().chain(st.pending.values().rev()).find(|p| {
                            p.drive == e.drive && (p.to_key.as_deref() == Some(&e.key) || p.key == e.key && p.op != Op::Delete && p.op != Op::Rename)
                        });
                        if let Some(prev) = prev {
                            e.base = StoredBase::Entry(prev.id);
                        }
                    }
                    journal::insert(&tx, e)?;
                    if let Some(ino) = e.mount_ino {
                        let updated = tx.execute("UPDATE mount_inodes SET entry_id=?2 WHERE ino=?1 AND drive=?3", rusqlite::params![ino, e.id, e.drive])?;
                        if updated != 1 {
                            return Ok(Err(crate::mount::FsError::Stale));
                        }
                    }
                    crate::mount::publication::stamp(&tx, e)?;
                }
                tx.commit()?;
                Ok(Ok((value, entries)))
            }).map_err(crate::mount::FsError::from)??;
            let (value, entries) = committed;
            for e in entries {
                st.pending.insert(e.id, e);
            }
            st.idle = false;
            Ok(value)
        }).await.map_err(crate::Error::from).map_err(crate::mount::FsError::from)?;
        if result.is_ok() {
            self.0.wake.notify_one();
            self.0.changed.notify_waiters();
        }
        result
    }

    /// The whole file.
    pub async fn put(&self, drive: &str, key: &str, data: Bytes, base: Base, attrs: Attrs) -> Result<EntryId> {
        let mut e = Entry::new(drive, key, Op::Put, base.into());
        e.size = data.len() as u64;
        e.attrs = attrs;
        self.stage(&mut e, data).await?;
        self.journal(e).await
    }

    /// Bytes at an offset.
    pub async fn write(&self, drive: &str, key: &str, offset: u64, data: Bytes, base: Base) -> Result<EntryId> {
        let mut e = Entry::new(drive, key, Op::Write, base.into());
        e.offset = offset;
        e.size = data.len() as u64;
        self.stage(&mut e, data).await?;
        self.journal(e).await
    }

    pub async fn truncate(&self, drive: &str, key: &str, size: u64, base: Base) -> Result<EntryId> {
        let mut e = Entry::new(drive, key, Op::Truncate, base.into());
        e.length = size;
        self.journal(e).await
    }

    pub async fn rename(&self, drive: &str, from: &str, to: &str, replace: bool, base: Base) -> Result<EntryId> {
        let mut e = Entry::new(drive, from, Op::Rename, base.into());
        e.to_key = Some(to.to_owned());
        e.replace = replace;
        self.journal(e).await
    }

    pub async fn delete(&self, drive: &str, key: &str, base: Base) -> Result<EntryId> {
        self.journal(Entry::new(drive, key, Op::Delete, base.into())).await
    }

    pub async fn folder(&self, drive: &str, key: &str, attrs: Attrs) -> Result<EntryId> {
        let mut e = Entry::new(drive, key, Op::Folder, StoredBase::Any);
        e.attrs = attrs;
        self.journal(e).await
    }

    pub async fn set_attrs(&self, drive: &str, key: &str, attrs: Attrs, base: Base) -> Result<EntryId> {
        let mut e = Entry::new(drive, key, Op::Attrs, base.into());
        e.attrs = attrs;
        self.journal(e).await
    }

    /// Writes `data` to a file of the journal's and syncs it, and the folder.
    async fn stage(&self, e: &mut Entry, data: Bytes) -> Result<()> {
        let dir = self.0.dir.clone();
        let path = blocking(move || {
            use std::io::Write;
            let mut raw = [0u8; 12];
            use std::io::Read;
            std::fs::File::open("/dev/urandom")?.read_exact(&mut raw)?;
            let path = dir.join(hex::encode(raw));
            let mut f = std::fs::File::create(&path)?;
            f.write_all(&data)?;
            f.sync_all()?;
            std::fs::File::open(&dir)?.sync_all()?;
            Ok(path)
        })
        .await?;
        e.source = Some(path);
        e.staged = true;
        Ok(())
    }

    /// Records `e`, based on the last unfinished change it depends on if there is one, and
    /// starts publishing it.
    async fn journal(&self, mut e: Entry) -> Result<EntryId> {
        let this = self.clone();
        // If it isn't recorded, its staged bytes go at the next open.
        let id = blocking(move || {
            let mut st = this.st();
            if let Some(prev) = st.pending.values().rev().find(|p| p.drive == e.drive && (p.key == e.key || p.to_key.as_deref() == Some(&e.key))) {
                e.base = StoredBase::Entry(prev.id);
            }
            this.0.store.with(|c| journal::insert(c, &mut e))?;
            let id = e.id;
            st.pending.insert(id, e);
            st.idle = false;
            Ok(id)
        })
        .await;
        self.0.wake.notify_one();
        id
    }

    /// Queues files of the user's as one batch, read where they are when they go up.
    pub async fn import(&self, label: &str, files: Vec<Import>) -> Result<BatchId> {
        let this = self.clone();
        let label = label.to_owned();
        let id = blocking(move || {
            let mut entries = Vec::with_capacity(files.len());
            for f in &files {
                let meta = std::fs::metadata(&f.path)?;
                if meta.is_dir() {
                    let key = if f.key.ends_with('/') { f.key.clone() } else { format!("{}/", f.key) };
                    let mut e = Entry::new(&f.drive, &key, Op::Folder, StoredBase::Any);
                    e.attrs = attrs_of(&f.path, &meta);
                    entries.push(e);
                    continue;
                }
                let mut e = Entry::new(&f.drive, &f.key, Op::Put, StoredBase::Any);
                e.source = Some(f.path.clone());
                e.stamp = Some(Stamp::of(&meta));
                e.size = meta.len();
                e.attrs = attrs_of(&f.path, &meta);
                entries.push(e);
            }
            let mut st = this.st();
            let batch = this.0.store.with(|c| {
                let tx = c.transaction()?;
                tx.execute("INSERT INTO batches(label, created) VALUES (?1, ?2)", rusqlite::params![label, journal::now_ms()])?;
                let batch = tx.last_insert_rowid();
                for e in &mut entries {
                    e.batch = Some(batch);
                    if let Some(prev) = st.pending.values().rev().find(|p| p.drive == e.drive && (p.key == e.key || p.to_key.as_deref() == Some(&e.key))) {
                        e.base = StoredBase::Entry(prev.id);
                    }
                    journal::insert(&tx, e)?;
                }
                tx.commit()?;
                Ok(batch)
            })?;
            for e in entries {
                st.pending.insert(e.id, e);
            }
            st.idle = false;
            Ok(batch)
        })
        .await?;
        self.0.wake.notify_one();
        Ok(id)
    }

    // -----------------------------------------------------------------------------------------
    // Control

    /// Pauses: what is uploading stops at its next request and goes on from there when resumed.
    pub async fn pause(&self, scope: Scope) -> Result<()> {
        self.set_paused(scope, true).await
    }

    /// Resumes, and tries a failed entry again at once.
    pub async fn resume(&self, scope: Scope) -> Result<()> {
        self.set_paused(scope, false).await
    }

    async fn set_paused(&self, scope: Scope, paused: bool) -> Result<()> {
        let this = self.clone();
        blocking(move || {
            let mut st = this.st();
            let s = &this.0.store;
            match &scope {
                Scope::All => {
                    st.paused_all = paused;
                    s.set_meta("paused", if paused { "1" } else { "0" })?;
                }
                Scope::Drive(d) => {
                    if paused {
                        st.paused_drives.insert(d.clone());
                        s.with(|c| c.execute("INSERT OR IGNORE INTO paused_drives(drive) VALUES (?1)", [d]).map(drop))?;
                    } else {
                        st.paused_drives.remove(d);
                        s.with(|c| c.execute("DELETE FROM paused_drives WHERE drive = ?1", [d]).map(drop))?;
                    }
                }
                Scope::Batch(b) => {
                    if paused {
                        st.paused_batches.insert(*b);
                    } else {
                        st.paused_batches.remove(b);
                    }
                    s.with(|c| c.execute("UPDATE batches SET paused = ?2 WHERE id = ?1", rusqlite::params![b, paused]).map(drop))?;
                }
                Scope::Entry(id) => {
                    if let Some(e) = st.pending.get_mut(id) {
                        e.paused = paused;
                        let e = e.clone();
                        s.with(|c| journal::update(c, &e))?;
                    }
                }
            }
            if !paused {
                // A resume tries the scope's failed entries again, at once.
                let blocked = s.with(|c| c.prepare("SELECT ino FROM mount_conflict_blockers UNION SELECT ino FROM mount_conflicts")?.query_map([], |r| r.get::<_, u64>(0))?.collect::<rusqlite::Result<HashSet<_>>>())?;
                let ids: Vec<EntryId> = st.pending.values().filter(|e| e.state == State::Failed && !(e.mount && (e.conflict.is_some() || e.mount_ino.is_some_and(|ino| blocked.contains(&ino)))) && in_scope(e, &scope)).map(|e| e.id).collect();
                for id in ids {
                    st.retry_at.remove(&id);
                    st.attempts.remove(&id);
                    if let Some(e) = st.pending.get_mut(&id) {
                        e.state = State::Queued;
                        e.error = None;
                        let e = e.clone();
                        s.with(|c| journal::update(c, &e))?;
                    }
                }
            }
            st.idle = false;
            if paused {
                for r in st.pending.values().filter(|e| in_scope(e, &scope)).filter_map(|e| st.running.get(&e.id)) {
                    r.stop.signal(Why::Pause);
                }
            }
            Ok(())
        })
        .await?;
        self.0.wake.notify_one();
        self.0.changed.notify_waiters();
        Ok(())
    }

    /// Cancels: nothing in the scope is published from now on, and what was uploading stops. A
    /// change made on top of a cancelled one is cancelled with it.
    pub async fn cancel(&self, scope: Scope) -> Result<()> {
        let entries = {
            let st = self.st();
            let mut gone: BTreeSet<EntryId> = st.pending.values().filter(|e| in_scope(e, &scope)).map(|e| e.id).collect();
            loop {
                let more: Vec<EntryId> = st.pending.values().filter(|e| !gone.contains(&e.id) && matches!(e.base, StoredBase::Entry(b) if gone.contains(&b))).map(|e| e.id).collect();
                if more.is_empty() { break; }
                gone.extend(more);
            }
            gone.into_iter().filter_map(|id| st.pending.get(&id).cloned()).collect::<Vec<_>>()
        };
        let drives: BTreeSet<String> = entries.iter().filter(|e| e.mount).map(|e| e.drive.clone()).collect();
        let mut prepared = Vec::new();
        let mut registrations = Vec::new();
        for drive in drives {
            registrations.push(self.mount_registration(&drive).await);
            if let Some(publisher) = self.mount_publisher(&drive) {
                let run = entries.iter().filter(|e| e.drive == drive).cloned().collect::<Vec<_>>();
                prepared.push((run.iter().filter_map(|e| e.mount_ino).collect::<HashSet<_>>(), publisher.prepare_publication(&run).await));
            }
        }
        let this = self.clone();
        let reports = blocking(move || {
            let mut st = this.st();
            let mut cancelled = Vec::new();
            for entry in entries {
                if let Some(r) = st.running.get(&entry.id).cloned() {
                    st.cancelled.insert(entry.id);
                    r.stop.signal(Why::Cancel);
                } else if let Some(e) = st.pending.get(&entry.id) {
                    if e.published_version.is_some() { continue; }
                    let mut e = e.clone(); e.state = State::Cancelled;
                    cancelled.push(e);
                }
            }
            let reports = this.0.store.with(|c| {
                let tx = c.transaction()?;
                for e in &cancelled { journal::update(&tx, e)?; }
                let mut drives = BTreeMap::<String, Vec<Entry>>::new();
                for e in cancelled.iter().filter(|e| e.mount) { drives.entry(e.drive.clone()).or_default().push(e.clone()); }
                let mut reports = Vec::new();
                for run in drives.into_values() {
                    reports.extend(crate::mount::publication::reconcile(&tx, &run, &crate::mount::publication::Completion::Cancelled).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?);
                }
                tx.commit()?;
                Ok(reports)
            })?;
            for e in cancelled {
                st.pending.remove(&e.id); st.attempts.remove(&e.id); st.retry_at.remove(&e.id);
                if e.staged && let Some(p) = &e.source { let _ = std::fs::remove_file(p); }
            }
            st.idle = false;
            Ok(reports)
        }).await?;
        for (inodes, prepared) in prepared {
            let selected = reports.iter().filter(|r| inodes.contains(&r.ino)).cloned().collect();
            prepared.apply(selected);
        }
        drop(registrations);
        self.0.wake.notify_one();
        self.0.changed.notify_waiters();
        Ok(())
    }

    /// The upload bandwidth limit, in bytes a second (`None` for unlimited). It applies to the
    /// next piece of every upload, and is kept across restarts.
    pub async fn set_bandwidth(&self, bytes_per_second: Option<u64>) -> Result<()> {
        self.0.bandwidth.set(bytes_per_second);
        let store = self.0.store.clone();
        blocking(move || store.set_meta("bandwidth", &bytes_per_second.unwrap_or(0).to_string())).await
    }

    /// Bytes the queue's requests have sent since it opened, sent again or not: for a rate.
    pub fn bytes_sent(&self) -> u64 {
        self.0.bandwidth.taken()
    }

    /// Forgets entries that are done or cancelled (SpaceFS's Clear, for Recent).
    pub async fn clear_finished(&self) -> Result<()> {
        let store = self.0.store.clone();
        blocking(move || {
            store.with(|c| {
                // Retain every base reachable from unfinished changes: they still need the
                // version a completed predecessor received, including after a restart.
                c.execute("WITH RECURSIVE needed(id) AS (
                    SELECT id FROM entries WHERE state NOT IN ('done', 'cancelled')
                    UNION SELECT entry_id FROM mount_inodes WHERE entry_id IS NOT NULL
                    UNION SELECT entry_id FROM mount_conflicts
                    UNION SELECT CAST(substr(e.base, 3) AS INTEGER) FROM entries e JOIN needed n ON e.id=n.id WHERE e.base LIKE 'e:%')
                    DELETE FROM entries WHERE state IN ('done', 'cancelled') AND id NOT IN (SELECT id FROM needed)", [])?;
                c.execute("DELETE FROM batches WHERE id NOT IN (SELECT batch FROM entries WHERE batch IS NOT NULL)", []).map(drop)
            })
        })
        .await
    }

    pub async fn status(&self) -> Result<Status> {
        let this = self.clone();
        blocking(move || {
            let entries = this.0.store.with(|c| journal::all(c))?;
            let labels: HashMap<BatchId, (String, bool)> =
                this.0.store.with(|c| c.prepare("SELECT id, label, paused FROM batches")?.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect())?;
            let st = this.st();
            let mut items = Vec::with_capacity(entries.len());
            for e in entries {
                // The live view of what is pending, the stored one of what isn't.
                let e = st.pending.get(&e.id).cloned().unwrap_or(e);
                let sent = st.running.get(&e.id).map_or(e.sent, |r| r.sent.load(Ordering::Relaxed));
                let state = if st.running.contains_key(&e.id) { State::Uploading } else { e.state };
                items.push(Item {
                    id: e.id,
                    paused: st.paused(&e) && !state.finished(),
                    drive: e.drive,
                    key: e.key,
                    to_key: e.to_key,
                    op: e.op,
                    state,
                    batch: e.batch,
                    size: e.size,
                    sent,
                    version: e.version,
                    conflict: e.conflict,
                    error: e.error,
                });
            }
            let mut batches: BTreeMap<BatchId, BatchStatus> = BTreeMap::new();
            for i in &items {
                let Some(b) = i.batch else { continue };
                let (label, paused) = labels.get(&b).cloned().unwrap_or_default();
                let s = batches.entry(b).or_insert(BatchStatus { id: b, label, items: 0, done: 0, failed: 0, cancelled: 0, bytes: 0, sent: 0, paused });
                s.items += 1;
                s.bytes += i.size;
                s.sent += if i.state == State::Done { i.size } else { i.sent };
                match i.state {
                    State::Done => s.done += 1,
                    State::Failed => s.failed += 1,
                    State::Cancelled => s.cancelled += 1,
                    _ => {}
                }
            }
            let unpublished: Vec<&Item> = items.iter().filter(|i| !i.state.finished()).collect();
            Ok(Status {
                unpublished: unpublished.len() as u64,
                unpublished_bytes: unpublished.iter().map(|i| i.size.saturating_sub(i.sent)).sum(),
                batches: batches.into_values().collect(),
                paused: st.paused_all,
                paused_drives: st.paused_drives.iter().cloned().collect(),
                bandwidth: this.0.bandwidth.get(),
                items,
            })
        })
        .await
    }

    /// Waits until nothing is left that could be published now: everything is finished, paused,
    /// waiting on something paused, or failed and waiting for a resume.
    pub async fn settle(&self) {
        loop {
            let n = self.0.changed.notified();
            let online = self.0.cfg.connectivity.as_ref().is_none_or(|c| c.link() != Link::Offline);
            {
                let st = self.st();
                // Idle because offline is idle no longer once the server is back.
                if st.idle && !(st.idle_offline && online) {
                    return;
                }
            }
            n.await;
        }
    }

    /// Stops publishing: what is uploading stops at its next request, as a pause does, but
    /// nothing is marked paused. Returns once the publisher has stopped and released the state.
    pub async fn close(&self) {
        {
            let mut st = self.st();
            st.closed = true;
            for r in st.running.values() {
                r.stop.signal(Why::Pause);
            }
        }
        self.0.wake.notify_one();
        let stopped = self.0.stopped.clone();
        loop {
            let n = stopped.1.notified();
            if *stopped.0.lock().unwrap_or_else(|p| p.into_inner()) {
                return;
            }
            n.await;
        }
    }

    // -----------------------------------------------------------------------------------------
    // The publisher

    async fn publisher(&self) {
        // The runs' tasks: the publisher stops only once each has let go of the queue, and of the
        // store with it, so that whoever closed the queue can open the state directory again.
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            while tasks.try_join_next().is_some() {}
            let wait = {
                let mut st = self.st();
                if st.closed && st.runs == 0 {
                    break;
                }
                let offline = self.0.cfg.connectivity.as_ref().is_some_and(|c| c.link() == Link::Offline);
                let (runs, wait) = if st.closed || offline { (Vec::new(), None) } else { self.select(&mut st) };
                st.idle = st.runs == 0 && runs.is_empty() && wait.is_none();
                st.idle_offline = offline;
                if st.idle {
                    self.0.changed.notify_waiters();
                }
                for run in runs {
                    let r = Arc::new(Running { stop: Stop::default(), sent: Arc::new(AtomicU64::new(0)) });
                    for e in &run {
                        st.running.insert(e.id, r.clone());
                    }
                    st.runs += 1;
                    // Kept until an attempt finishes the entry: one that fails may not have found out.
                    let may_have_landed = st.may_have_landed.contains(&run[0].id);
                    let this = self.clone();
                    tasks.spawn(async move { this.run(run, r, may_have_landed).await });
                }
                wait
            };
            let notified = self.0.wake.notified();
            match wait {
                Some(d) => {
                    let _ = tokio::time::timeout(d, notified).await;
                }
                None => notified.await,
            }
        }
        while tasks.join_next().await.is_some() {}
    }

    /// The runs that may start now, and how long until a failed one may be tried again.
    fn select(&self, st: &mut QState) -> (Vec<Vec<Entry>>, Option<Duration>) {
        let blocked = match self.0.store.with(|c| c.prepare("SELECT ino FROM mount_conflict_blockers UNION SELECT ino FROM mount_conflicts")?.query_map([], |r| r.get::<_, u64>(0))?.collect::<rusqlite::Result<HashSet<_>>>()) {
            Ok(blocked) => blocked,
            Err(_) => return (Vec::new(), Some(Duration::from_secs(1))),
        };
        let now = Instant::now();
        let mut free = self.0.cfg.uploads.saturating_sub(st.runs);
        let mut runs = Vec::new();
        let mut wait: Option<Duration> = None;
        let mut taken: HashSet<EntryId> = HashSet::new();
        let ids: Vec<EntryId> = st.pending.keys().copied().collect();
        for (i, id) in ids.iter().enumerate() {
            if free == 0 {
                break;
            }
            let e = &st.pending[id];
            if taken.contains(id) || st.running.contains_key(id) || st.paused(e) || e.mount && e.mount_ino.is_some_and(|ino| blocked.contains(&ino)) {
                continue;
            }
            if e.state == State::Failed {
                match st.retry_at.get(id) {
                    Some(t) if *t > now => {
                        let d = *t - now;
                        wait = Some(wait.map_or(d, |w: Duration| w.min(d)));
                        continue;
                    }
                    Some(_) => {}
                    None => continue,
                }
            }
            // Anything earlier it depends on must be finished first.
            if ids[..i].iter().any(|p| depends(&st.pending[p], e)) {
                continue;
            }
            let mut run = vec![e.clone()];
            taken.insert(*id);
            // A put takes the writes after it, writes take the writes after them. An entry that
            // may have landed goes alone: its attempt may have carried fewer entries than are
            // queued now, and recognizing what landed must not take the rest as published.
            let alone = st.may_have_landed.contains(id);
            if !alone && matches!(e.op, Op::Put | Op::Write | Op::Truncate) && (e.op != Op::Put || e.size < self.0.cfg.multipart_from) {
                let patch = e.op != Op::Put;
                let mut body = e.size.saturating_add(if patch { 12 + if e.op == Op::Write { 16 } else { 0 } } else { 0 });
                let mut logical = e.size;
                for next in &ids[i + 1..] {
                    let n = &st.pending[next];
                    if !depends(e, n) {
                        continue;
                    }
                    let fits = n.drive == e.drive && n.key == e.key && matches!(n.op, Op::Write | Op::Truncate) && !st.paused(n) && n.state == State::Queued && n.batch == e.batch && n.mount == e.mount && n.published_version == e.published_version && n.published_attrs == e.published_attrs;
                    // A patch's truncate goes last: writes after it start a run of their own.
                    let after_truncate = e.op != Op::Put && run.last().is_some_and(|l: &Entry| l.op == Op::Truncate);
                    let added = n.size.saturating_add(if patch && n.op == Op::Write { 16 } else { 0 });
                    let next_size = match n.op { Op::Write => logical.max(n.offset.saturating_add(n.size)), Op::Truncate => n.length, _ => logical };
                    if !fits || after_truncate || body.saturating_add(added) > 64 * MIB || !patch && next_size > 64 * MIB || run.len() >= 10_000 {
                        break;
                    }
                    body += added;
                    logical = next_size;
                    taken.insert(n.id);
                    run.push(n.clone());
                }
            }
            for e in &run {
                if let Some(p) = st.pending.get_mut(&e.id) {
                    p.state = State::Uploading;
                }
            }
            runs.push(run);
            free -= 1;
        }
        (runs, wait)
    }

    /// The guard of an entry: the version its base is, or got.
    fn guard(&self, base: &StoredBase, mount: bool) -> Result<Guard> {
        let mut base = base.clone();
        let mut seen = HashSet::new();
        loop {
            match base {
                StoredBase::Any => return Ok(Guard::None),
                StoredBase::Absent => return Ok(Guard::Absent),
                StoredBase::Version(v) => return Ok(Guard::Version(v)),
                StoredBase::Entry(id) => match self.0.store.with(|c| journal::get(c, id))? {
                    _ if !seen.insert(id) => return Err(crate::Error::Invalid("a journal base contains a cycle".into())),
                    Some(e) if mount && e.state == State::Done && e.version.is_none() => return Err(crate::Error::Invalid("the mount edit's base has no known published version".into())),
                    Some(e) if e.state == State::Done => return Ok(e.version.map_or(Guard::None, Guard::Version)),
                    Some(_) if mount => return Err(crate::Error::Invalid("the mount edit's predecessor was not published".into())),
                    // Cancelled: whatever it was based on.
                    Some(e) => base = e.base,
                    None if mount => return Err(crate::Error::Invalid("the mount edit's base is missing".into())),
                    None => return Ok(Guard::None),
                },
            }
        }
    }

    async fn object(&self, e: &Entry, key: &str, version: Option<String>, published: bool) -> Result<crate::mount::publication::Object> {
        let meta = self.0.client.head_object(&e.drive, key, voidfs_sdk::ReadOptions { version_id: version.clone(), ..Default::default() }).await?;
        if version.as_ref().is_some_and(|v| *v != meta.version_id) {
            return Err(Error::Changed { key: key.to_owned(), expected: version.unwrap_or_default(), got: meta.version_id });
        }
        let attrs = self.0.client.attributes(&e.drive, key, voidfs_sdk::ReadOptions { version_id: Some(meta.version_id.clone()), ..Default::default() }).await?;
        if meta.object_id.as_deref() != Some(attrs.object_id.as_str()) || attrs.object_id.is_empty() || attrs.version_id.as_deref() != Some(meta.version_id.as_str()) {
            return Err(Error::Invalid("the published object's identity or attributes did not match its exact version".into()));
        }
        let object = crate::mount::publication::Object { drive: e.drive.clone(), key: key.to_owned(), meta, attrs, content: None };
        crate::mount::publication::validate(&object).map_err(|e| Error::Invalid(e.to_string()))?;
        if published {
            let expected = if let Some(ino) = e.mount_ino {
                let store = self.0.store.clone(); let drive = e.drive.clone();
                let json = blocking(move || store.with(|c| c.query_row("SELECT attrs FROM mount_inodes WHERE ino=?1 AND drive=?2", rusqlite::params![ino, drive], |r| r.get::<_, String>(0)))).await?;
                serde_json::from_str::<voidfs_sdk::FolderEntry>(&json).map_err(|e| Error::Invalid(e.to_string()))?.kind
            } else if e.op == Op::Folder || e.key.ends_with('/') { voidfs_sdk::Kind::Folder } else { voidfs_sdk::Kind::File };
            if object.meta.kind != expected { return Err(Error::Invalid("the published object's kind differs from the local edit".into())); }
        }
        Ok(object)
    }

    async fn competing(&self, e: &Entry, key: &str, version: Option<String>, opposite_kind: bool) -> Result<crate::mount::publication::Object> {
        match self.object(e, key, version.clone(), false).await {
            Err(Error::Fetch(error)) if opposite_kind && error.status() == Some(404) && !key.is_empty() => {
                let alternative = if key.ends_with('/') { key.trim_end_matches('/').to_owned() } else { format!("{key}/") };
                self.object(e, &alternative, version, false).await
            }
            result => result,
        }
    }

    fn metadata_retryable(error: &Error) -> bool {
        match error {
            Error::Fetch(error) => matches!(&**error, voidfs_sdk::Error::Transport { .. }) || error.status().is_some_and(|s| s >= 500 || s == 429),
            Error::Io(_) | Error::Db(_) => true,
            _ => false,
        }
    }

    fn permanent_completion_error(error: &Error) -> bool {
        let Error::Db(database) = error else { return false; };
        let rusqlite::Error::ToSqlConversionFailure(cause) = database.as_ref() else { return false; };
        cause.downcast_ref::<crate::mount::FsError>().is_some_and(|error| !matches!(error, crate::mount::FsError::Io(message) if message.starts_with("state database:")))
    }

    fn acknowledge(&self, run: &[Entry], version: &str, key: &str, attrs: bool) -> Result<()> {
        let mut st = self.st();
        let mut entries = Vec::new();
        for e in run {
            if let Some(p) = st.pending.get(&e.id) {
                let mut p = p.clone();
                p.published_version = Some(version.to_owned());
                p.published_key = Some(key.to_owned());
                p.published_attrs = attrs;
                entries.push(p);
            }
        }
        self.0.store.with(|c| {
            let tx = c.transaction()?;
            for e in &entries { journal::update(&tx, e)?; }
            tx.commit()?;
            Ok(())
        })?;
        crate::kill::point("publish.acknowledged");
        for e in entries { st.pending.insert(e.id, e); }
        Ok(())
    }

    async fn run(&self, mut run: Vec<Entry>, r: Arc<Running>, may_have_landed: bool) {
        let this = self.clone();
        let base = run[0].base.clone();
        let mount = run[0].mount;
        let upload_id = Arc::new(Mutex::new(run[0].upload_id.clone()));
        let (ack_queue, ack_run) = (self.clone(), run.clone());
        let ctx = Ctx {
            client: self.0.client.clone(), store: self.0.store.clone(), state_id: self.0.state_id.clone(),
            part_size: self.0.cfg.part_size, multipart_from: self.0.cfg.multipart_from, parts_at_once: self.0.cfg.parts_at_once,
            direct_from: self.0.cfg.direct_from, memory: self.0.memory.clone(), memory_kib: self.0.memory_kib,
            offered: self.0.direct.clone(), stop: r.stop.clone(), sent: r.sent.clone(), may_have_landed, upload_id: upload_id.clone(),
            acknowledge: mount.then(|| Arc::new(move |version: &str, key: &str, attrs| ack_queue.acknowledge(&ack_run, version, key, attrs)) as Arc<publish::Acknowledge>),
        };
        let outcome = if let Some(version) = &run[0].published_version {
            if run[0].published_attrs { publish::resume_attrs(&ctx, &run, version.clone()).await } else { Outcome::Done { version: Some(version.clone()), conflict: None } }
        } else { match blocking(move || this.guard(&base, mount)).await {
            Ok(guard) => publish::publish(&ctx, &run, guard).await,
            Err(e) => Outcome::from(e),
        }};
        if mount {
            let (store, ids) = (self.0.store.clone(), run.iter().map(|e| e.id).collect::<Vec<_>>());
            if let Ok(recorded) = blocking(move || store.with(|c| ids.into_iter().map(|id| journal::get(c, id)).collect::<rusqlite::Result<Vec<_>>>())).await {
                for (entry, recorded) in run.iter_mut().zip(recorded) {
                    if let Some(recorded) = recorded && recorded.published_version.is_some() {
                        entry.published_version = recorded.published_version; entry.published_key = recorded.published_key; entry.published_attrs = recorded.published_attrs;
                    }
                }
            }
        }
        let mut outcome = match (outcome, r.stop.why()) {
            (o @ (Outcome::Done { .. } | Outcome::Conflict { .. }), _) => o,
            (_, Some(Why::Cancel)) if run[0].published_version.is_some() => Outcome::Failed { error: "published content awaits reconciliation after cancellation".into(), transient: false },
            (_, Some(w)) => Outcome::Stopped(w),
            (o, None) => o,
        };
        if mount && run[0].op == Op::Delete && matches!(outcome, Outcome::Done { version: None, .. }) {
            match self.0.client.head_object(&run[0].drive, &run[0].key, voidfs_sdk::ReadOptions::default()).await {
                Ok(meta) => outcome = Outcome::Conflict { status: 412, current_version: Some(meta.version_id), error: "the guarded delete left its remote object in place".into() },
                Err(error) if error.status() == Some(404) => {},
                Err(error) => {
                    let error = Error::from(error);
                    outcome = Outcome::Failed { transient: Self::metadata_retryable(&error), error: format!("confirming the guarded delete: {error}") };
                }
            }
        }
        let mut completion = None;
        if mount {
            match &outcome {
                Outcome::Done { version, .. } => {
                    let key = run[0].published_key.clone().or_else(|| run[0].to_key.clone()).unwrap_or_else(|| run[0].key.clone());
                    let key = if run[0].op == Op::Folder && !key.ends_with('/') { format!("{key}/") } else { key };
                    if let Some(version) = version.clone() {
                        for e in &mut run { e.published_version = Some(version.clone()); e.published_key = Some(key.clone()); }
                        let (this, entries, v, k) = (self.clone(), run.clone(), version.clone(), key.clone());
                        if let Err(e) = blocking(move || this.acknowledge(&entries, &v, &k, false)).await {
                            outcome = Outcome::Failed { error: format!("recording the published version: {e}"), transient: true };
                        } else if run[0].op != Op::Delete {
                            match self.object(&run[0], &key, Some(version.clone()), true).await {
                                Ok(object) => completion = Some(crate::mount::publication::Completion::Success { version: Some(version), object: Some(object) }),
                                Err(e) => {
                                    let transient = Self::metadata_retryable(&e);
                                    outcome = Outcome::Failed { error: format!("reconciling the published version: {e}"), transient };
                                }
                            }
                        } else {
                            completion = Some(crate::mount::publication::Completion::Success { version: Some(version), object: None });
                        }
                    } else if run[0].op == Op::Delete {
                        completion = Some(crate::mount::publication::Completion::Success { version: None, object: None });
                    } else {
                        outcome = Outcome::Failed { error: "the mount mutation returned no published version".into(), transient: false };
                    }
                }
                Outcome::Conflict { status, current_version, error } => {
                    let key = if *status == 409 && run[0].op == Op::Rename { run[0].to_key.as_deref().unwrap_or(&run[0].key) } else { &run[0].key };
                    let key = if run[0].op == Op::Folder && !key.ends_with('/') { format!("{key}/") } else { key.to_owned() };
                    let mut detail = error.clone();
                    let mut remote = None;
                    let mut normalized = *status;
                    if *status != 404 {
                        match self.competing(&run[0], &key, current_version.clone(), *status == 409).await {
                            Ok(object) => match crate::mount::publication::capture_remote(&self.0.client, &self.0.store, object.clone()).await {
                                Ok(object) => remote = Some(object),
                                Err(e) => { remote = Some(object); detail.push_str(&format!("; retaining the remote version: {e}")); },
                            },
                            Err(Error::Fetch(e)) if *status == 412 && current_version.is_none() && e.status() == Some(404) => normalized = 404,
                            Err(e) => detail.push_str(&format!("; reading the remote version: {e}")),
                        }
                    }
                    completion = Some(crate::mount::publication::Completion::Conflict { status: normalized, current_version: current_version.clone(), remote, error: detail, local: HashMap::new() });
                }
                _ => {}
            }
            if completion.is_none() {
                completion = match &outcome {
                    Outcome::Failed { error, .. } | Outcome::Rejected { error, .. } => Some(crate::mount::publication::Completion::Error { error: error.clone() }),
                    Outcome::Stopped(Why::Cancel) => Some(crate::mount::publication::Completion::Cancelled),
                    _ => None,
                };
            }
        }
        let registration = if mount { Some(self.mount_registration(&run[0].drive).await) } else { None };
        let related = {
            let st = self.st();
            let mut entries = Vec::new();
            let mut ids: HashSet<EntryId> = run.iter().map(|e| e.id).collect();
            loop {
                let more = st.pending.values().filter(|e| !ids.contains(&e.id) && (matches!(e.base, StoredBase::Entry(id) if ids.contains(&id)) || entries.iter().chain(run.iter()).any(|prior| prior.id < e.id && depends(prior, e)))).cloned().collect::<Vec<_>>();
                if more.is_empty() { break; }
                for e in more { ids.insert(e.id); entries.push(e); }
            }
            entries.extend(run.iter().cloned());
            entries
        };
        let prepared = match self.mount_publisher(&run[0].drive) {
            Some(publisher) if mount => Some(publisher.prepare_publication(&related).await),
            _ => None,
        };
        let connectivity = self.0.cfg.connectivity.clone().unwrap_or_default();
        if let Some(completion) = &mut completion && let Err(e) = crate::mount::publication::capture_local(&self.0.client, &self.0.store, &run, completion, &connectivity).await
            && let crate::mount::publication::Completion::Conflict { error, .. } = completion {
            error.push_str(&format!("; retaining local bytes: {e}"));
        }
        let captured = match &completion {
            Some(crate::mount::publication::Completion::Conflict { remote, local, .. }) =>
                remote.iter().filter_map(|o| o.content.clone()).chain(local.values().filter_map(|c| c.path.clone())).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        if !captured.is_empty() { crate::kill::point("conflict.captured"); }
        let this = self.clone();
        let sent = r.sent.load(Ordering::Relaxed);
        let upload_id = upload_id.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let backup = run.clone();
        let fallback_upload = upload_id.clone();
        let finished = blocking(move || this.finish(run, outcome, completion, sent, upload_id)).await;
        let finished = match finished {
            Err(error) if Self::permanent_completion_error(&error) => {
                let message = format!("reconciling the acknowledged publication: {error}");
                let (this, run) = (self.clone(), backup.clone());
                blocking(move || this.finish(run, Outcome::Failed { error: message.clone(), transient: false }, Some(crate::mount::publication::Completion::Error { error: message }), sent, fallback_upload)).await
            }
            result => result,
        };
        if !captured.is_empty() {
            // Snapshots a conflict didn't record: its completion failed, or an earlier one kept its own.
            let store = self.0.store.clone();
            let _ = blocking(move || store.with(|c| {
                let mut q = c.prepare("SELECT 1 FROM mount_conflicts WHERE local_path=?1 OR remote_path=?1")?;
                for path in captured {
                    if !q.exists([path.to_string_lossy()])? { let _ = std::fs::remove_file(&path); }
                }
                Ok(())
            })).await;
        }
        match finished {
            Ok(reports) => if let Some(prepared) = prepared { prepared.apply(reports); },
            Err(e) => {
                eprintln!("voidfs publication reconciliation: {e}");
                let mut st = self.st();
                for entry in backup {
                    st.running.remove(&entry.id);
                    if let Some(p) = st.pending.get_mut(&entry.id) {
                        p.state = State::Failed;
                        p.error = Some(format!("recording publication completion: {e}"));
                        p.published_version = entry.published_version;
                        p.published_key = entry.published_key;
                        p.published_attrs = entry.published_attrs;
                    }
                    st.retry_at.insert(entry.id, Instant::now() + Duration::from_secs(1));
                }
            }
        }
        drop(registration);
        self.st().runs -= 1;
        self.0.wake.notify_one();
        self.0.changed.notify_waiters();
    }

    /// Journal completion and durable mount state become visible together.
    fn finish(&self, run: Vec<Entry>, outcome: Outcome, completion: Option<crate::mount::publication::Completion>, sent: u64, upload_id: Option<String>) -> Result<Vec<crate::mount::publication::Report>> {
        let mut st = self.st();
        let mut entries = Vec::new();
        let closed = st.closed;
        for (i, e) in run.iter().enumerate() {
            let cancelled = st.cancelled.contains(&e.id);
            let Some(p) = st.pending.get(&e.id) else { continue };
            let mut p = p.clone();
            if i == 0 { p.upload_id = upload_id.clone(); }
            p.published_version = e.published_version.clone().or(p.published_version);
            p.published_key = e.published_key.clone().or(p.published_key);
            if e.published_version.is_some() { p.published_attrs = e.published_attrs; }
            match &outcome {
                Outcome::Done { version, conflict } => {
                    p.state = State::Done; p.version = version.clone(); p.sent = p.size; p.error = None;
                    if i == 0 { p.conflict = conflict.clone(); }
                }
                Outcome::Conflict { current_version, error, .. } => {
                    p.state = State::Failed; p.conflict = Some(current_version.clone().unwrap_or_else(|| "?".into())); p.error = Some(error.clone());
                    if i == 0 { p.sent = sent.min(p.size); }
                }
                Outcome::Failed { error, .. } | Outcome::Rejected { error, .. } if i == 0 => {
                    p.state = State::Failed; p.sent = sent.min(p.size); p.error = Some(error.clone());
                }
                Outcome::Stopped(Why::Cancel) if cancelled => p.state = State::Cancelled,
                _ => {
                    p.state = if closed { State::Uploading } else { State::Queued };
                    p.sent = if i == 0 { sent.min(p.size) } else { 0 };
                }
            }
            entries.push(p);
        }
        let reports = self.0.store.with(|c| {
            let tx = c.transaction()?;
            for e in &entries { journal::update(&tx, e)?; }
            let reports = if let Some(completion) = &completion {
                let reconciled: Vec<Entry> = entries.iter().filter(|e| !matches!(completion, crate::mount::publication::Completion::Cancelled) || e.state == State::Cancelled).cloned().collect();
                crate::mount::publication::reconcile(&tx, &reconciled, completion).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
            } else { Vec::new() };
            tx.commit()?;
            Ok(reports)
        })?;
        crate::kill::point("reconcile.committed");
        for e in &run { st.running.remove(&e.id); st.cancelled.remove(&e.id); }
        if let Outcome::Failed { transient, .. } = &outcome {
            let id = run[0].id;
            // A timeout or a lost connection may have followed a request that landed.
            if *transient { st.may_have_landed.insert(id); }
            let n = st.attempts.entry(id).or_insert(0);
            *n += 1;
            if *transient {
                let d = Duration::from_secs(1u64 << (*n).min(10)).min(self.0.cfg.retry_max);
                st.retry_at.insert(id, Instant::now() + d);
            } else { st.retry_at.remove(&id); }
        } else if matches!(outcome, Outcome::Conflict { .. }) {
            for e in &run { st.retry_at.remove(&e.id); }
        }
        for e in entries {
            if e.state == State::Queued && matches!(outcome, Outcome::Stopped(_)) { st.may_have_landed.insert(e.id); }
            if !e.state.finished() { st.pending.insert(e.id, e); continue; }
            st.pending.remove(&e.id); st.attempts.remove(&e.id); st.retry_at.remove(&e.id); st.may_have_landed.remove(&e.id);
            if e.staged && let Some(p) = &e.source { let _ = std::fs::remove_file(p); }
            if e.state == State::Cancelled && let Some(id) = &e.upload_id {
                let (client, drive, key, id) = (self.0.client.clone(), e.drive.clone(), e.key.clone(), id.clone());
                tokio::spawn(async move { let _ = client.abort_multipart_upload(&drive, &key, &id).await; });
            }
        }
        Ok(reports)
    }

    /// The client the queue publishes with (its bandwidth limit included).
    pub fn client(&self) -> &Client {
        &self.0.client
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.0.store
    }
}

fn in_scope(e: &Entry, scope: &Scope) -> bool {
    match scope {
        Scope::All => true,
        Scope::Drive(d) => e.drive == *d,
        Scope::Batch(b) => e.batch == Some(*b),
        Scope::Entry(id) => e.id == *id,
    }
}

/// What an imported file brings: its modification time, permission bits and extended
/// attributes, but the ones the system keeps for itself.
fn attrs_of(path: &Path, meta: &std::fs::Metadata) -> Attrs {
    use std::os::unix::fs::PermissionsExt;
    let mtime = meta.modified().ok().map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Micros, true));
    let mut xattrs = BTreeMap::new();
    if let Ok(names) = xattr::list(path) {
        for name in names {
            let name = name.to_string_lossy().into_owned();
            if matches!(name.as_str(), "com.apple.quarantine" | "com.apple.provenance") || name.starts_with("security.") || name.starts_with("system.") {
                continue;
            }
            if let Ok(Some(v)) = xattr::get(path, &name) {
                xattrs.insert(name, v);
            }
        }
    }
    Attrs { mtime, mode: Some(meta.permissions().mode() & 0o7777), xattrs, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, to: Option<&str>) -> Entry {
        let mut e = Entry::new("d", key, if to.is_some() { Op::Rename } else { Op::Put }, StoredBase::Any);
        e.to_key = to.map(str::to_owned);
        e
    }

    #[test]
    fn changes_wait_for_earlier_ones_on_their_keys_and_folders() {
        let put = entry("a/x", None);
        assert!(depends(&put, &entry("a/x", None)), "the same key");
        assert!(!depends(&put, &entry("a/y", None)), "another key");
        assert!(depends(&put, &entry("a/", Some("b/"))), "a folder rename after a change inside it");
        assert!(depends(&entry("a/", Some("b/")), &entry("b/z", None)), "a change inside the folder's new name");
        assert!(depends(&entry("x", Some("y")), &entry("y", None)), "a rename's destination");
        assert!(!depends(&entry("a", None), &entry("ab", None)), "a file is not a folder");
        let mut other = entry("a/x", None);
        other.drive = "e".into();
        assert!(!depends(&put, &other), "another drive");
    }

    fn client(endpoint: &str) -> Client {
        Client::new(voidfs_sdk::Config { endpoint: endpoint.to_owned(), access_key_id: voidfs_server::test_server::ADMIN_KEY_ID.into(),
            secret_access_key: voidfs_server::test_server::ADMIN_SECRET.into(), ..Default::default() }).unwrap()
    }

    async fn paused(dir: &Path) -> Queue {
        let store = Arc::new(Store::open(dir).unwrap());
        store.set_meta("paused", "1").unwrap();
        Queue::open(store, client("http://127.0.0.1:1"), QueueConfig::default()).await.unwrap()
    }

    fn local_inode(tx: &rusqlite::Transaction<'_>) -> crate::mount::Result<u64> {
        let attrs = voidfs_sdk::FolderEntry { name: "file".into(), kind: voidfs_sdk::Kind::File, object_id: String::new(), version_id: None,
            size: Some(0), etag: None, mtime: None, mode: None, has_xattrs: false, target: None };
        tx.execute("INSERT INTO mount_inodes(drive, attrs) VALUES ('d', ?1)", [serde_json::to_string(&attrs).unwrap()]).map_err(crate::Error::from)?;
        Ok(tx.last_insert_rowid() as u64)
    }

    #[tokio::test]
    async fn journal_copies_survive_a_state_directory_reached_by_another_path() {
        let parent = tempfile::tempdir().unwrap();
        let real = parent.path().join("state");
        let staging = tempfile::tempdir().unwrap();
        let source = staging.path().join("file");
        std::fs::write(&source, b"snapshot").unwrap();
        let q = paused(&real).await;
        let frozen = q.mount_copy(source, 0, 8).await.unwrap();
        let name = frozen.file_name().unwrap().to_owned();
        let snapshots = real.join("mount-conflicts");
        let (local, remote) = (snapshots.join("local-1"), snapshots.join("remote-1"));
        q.mount_transaction(move |tx| {
            let mut e = Entry::new("d", "file", Op::Put, StoredBase::Absent);
            let ino = local_inode(tx)?;
            e.mount_ino = Some(ino);
            (e.source, e.staged, e.size) = (Some(frozen), true, 8);
            tx.execute("INSERT INTO mount_conflicts(ino, entry_id, local_path, remote_path) VALUES (?1, 1, ?2, ?3)",
                rusqlite::params![ino, local.to_str(), remote.to_str()]).map_err(crate::Error::from)?;
            Ok(((), vec![e]))
        }).await.unwrap();
        q.close().await;
        drop(q);
        // Through a link, then moved: the same state, recorded under another path.
        let link = parent.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let moved = parent.path().join("moved");
        for (open, rename) in [(link.clone(), None), (moved.clone(), Some((&real, &moved)))] {
            if let Some((from, to)) = rename { std::fs::rename(from, to).unwrap(); }
            let q = paused(&open).await;
            let entry = q.store().with(|c| journal::unfinished(c)).unwrap().pop().expect("the entry is still queued");
            assert_eq!(entry.source.as_deref(), Some(open.join("journal").join(&name).as_path()), "recorded where this state now is");
            assert_eq!(std::fs::read(entry.source.unwrap()).unwrap(), b"snapshot", "queue startup keeps the journal's copy");
            let paths: (String, String) = q.store().with(|c| c.query_row("SELECT local_path, remote_path FROM mount_conflicts", [], |r| Ok((r.get(0)?, r.get(1)?)))).unwrap();
            let snapshots = open.join("mount-conflicts");
            assert_eq!(paths, (snapshots.join("local-1").to_string_lossy().into_owned(), snapshots.join("remote-1").to_string_lossy().into_owned()),
                "retained snapshots are found where this state now is");
            q.close().await;
        }
    }

    #[tokio::test]
    async fn mount_snapshot_is_frozen_and_owned_by_the_journal_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let source = staging.path().join("file");
        std::fs::write(&source, b"snapshot").unwrap();
        let q = paused(dir.path()).await;
        let frozen = q.mount_copy(source.clone(), 0, 8).await.unwrap();
        assert_eq!(frozen.parent().unwrap(), dir.path().join("journal"));
        assert_eq!(std::fs::read(&frozen).unwrap(), b"snapshot");
        std::fs::write(&source, b"replacement bytes").unwrap();
        assert_eq!(std::fs::read(&frozen).unwrap(), b"snapshot", "later staging writes do not change the queued source");
        let path = frozen.clone();
        q.mount_transaction(move |tx| {
            let mut e = Entry::new("d", "file", Op::Put, StoredBase::Absent);
            e.mount_ino = Some(local_inode(tx)?);
            e.source = Some(path);
            e.staged = true;
            e.size = 8;
            Ok(((), vec![e]))
        }).await.unwrap();
        q.close().await;
        drop(q);
        let q = paused(dir.path()).await;
        let entry = q.store().with(|c| journal::all(c)).unwrap().pop().unwrap();
        assert_eq!(entry.source.as_ref(), Some(&frozen));
        assert!(entry.staged);
        assert_eq!(std::fs::read(&frozen).unwrap(), b"snapshot", "queue startup keeps adopted source bytes");
        q.cancel(Scope::All).await.unwrap();
        assert!(!frozen.exists(), "the journal cleans up its frozen source after cancellation");
        assert!(source.exists(), "the mount still owns its mutable staging file");
        q.close().await;
    }

    #[tokio::test]
    async fn mount_snapshot_copies_exact_ranges_larger_than_its_buffer() {
        use std::os::unix::fs::FileExt;
        let dir = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let source = staging.path().join("sparse");
        let file = std::fs::File::create(&source).unwrap();
        file.set_len(64 * MIB).unwrap();
        let bytes: Vec<u8> = (0..3 * MIB + 17).map(|n| (n % 251) as u8).collect();
        file.write_all_at(&bytes, 17 * MIB + 3).unwrap();
        let q = paused(dir.path()).await;
        let frozen = q.mount_copy(source, 17 * MIB + 3, bytes.len() as u64).await.unwrap();
        assert_eq!(std::fs::metadata(&frozen).unwrap().len(), bytes.len() as u64);
        assert_eq!(std::fs::read(&frozen).unwrap(), bytes);
        q.close().await;
        drop(q);
        let q = paused(dir.path()).await;
        assert!(!frozen.exists(), "an unadopted snapshot is removed at queue startup");
        q.close().await;
    }

    #[tokio::test]
    async fn mount_snapshot_rejects_short_and_overflowing_ranges_without_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let source = staging.path().join("short");
        std::fs::write(&source, b"data").unwrap();
        let q = paused(dir.path()).await;
        assert!(q.mount_copy(source.clone(), 3, 2).await.is_err());
        assert_eq!(q.mount_copy(source.clone(), u64::MAX, 1).await, Err(crate::mount::FsError::InvalidArgument));
        assert_eq!(std::fs::read_dir(dir.path().join("journal")).unwrap().count(), 0);
        let frozen = q.mount_copy(source, 4, 0).await.unwrap();
        assert_eq!(std::fs::metadata(frozen).unwrap().len(), 0);
        q.close().await;
    }

    #[tokio::test]
    async fn mount_patch_coalescing_counts_encoded_body_at_size_boundary() {
        for (last, expected) in [(8 * MIB, 7), (8 * MIB - 12 - 8 * 16, 8)] {
            let dir = tempfile::tempdir().unwrap();
            let q = paused(dir.path()).await;
            q.mount_transaction(move |_| {
                let entries = (0..8).map(|i| {
                    let mut e = Entry::new("d", "file", Op::Write, if i == 0 { StoredBase::Version("base".into()) } else { StoredBase::Any });
                    e.offset = i * 8 * MIB;
                    e.size = if i == 7 { last } else { 8 * MIB };
                    e.length = e.size;
                    e
                }).collect();
                Ok(((), entries))
            }).await.unwrap();
            let mut state = QState { pending: q.st().pending.clone(), ..Default::default() };
            let (runs, retry) = q.select(&mut state);
            assert!(retry.is_none());
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].len(), expected, "VFSP headers consume the same request-body budget as edit bytes");
            let encoded = 12 + runs[0].iter().map(|e| e.size + 16).sum::<u64>();
            assert!(encoded <= voidfs_core::patch::MAX_BODY as u64);
            if expected == 8 { assert_eq!(encoded, voidfs_core::patch::MAX_BODY as u64); }
            else {
                let last = state.pending.values().last().unwrap();
                assert_eq!(last.state, State::Queued);
                assert_eq!(last.base, StoredBase::Entry(runs[0].last().unwrap().id), "the next run is guarded by its predecessor's published version");
            }
            q.close().await;
        }
    }

    #[tokio::test]
    async fn mount_sparse_changes_do_not_expand_a_coalesced_put_past_body_budget() {
        for op in [Op::Write, Op::Truncate] {
            for (size, expected) in [(1024 * MIB, 1), (64 * MIB, 2)] {
                let dir = tempfile::tempdir().unwrap();
                let q = paused(dir.path()).await;
                q.mount_transaction(move |_| {
                    let create = Entry::new("d", "file", Op::Put, StoredBase::Absent);
                    let mut edit = Entry::new("d", "file", op, StoredBase::Any);
                    if op == Op::Write { edit.offset = size - 1; edit.size = 1; }
                    else { edit.length = size; }
                    Ok(((), vec![create, edit]))
                }).await.unwrap();
                let mut state = QState { pending: q.st().pending.clone(), ..Default::default() };
                let (runs, retry) = q.select(&mut state);
                assert!(retry.is_none());
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].len(), expected, "a sparse edit's logical size bounds the coalesced put's allocation");
                if expected == 1 {
                    let create = runs[0][0].id;
                    let edit = state.pending.values().last().unwrap();
                    assert_eq!(edit.state, State::Queued);
                    assert_eq!(edit.base, StoredBase::Entry(create));
                    state.pending.remove(&create);
                    let (runs, _) = q.select(&mut state);
                    assert_eq!(runs.len(), 1);
                    assert_eq!(runs[0].len(), 1);
                    assert_eq!(runs[0][0].op, op, "large gaps stay server-side instead of becoming a put body");
                }
                q.close().await;
            }
        }
    }

    #[tokio::test]
    async fn mount_edits_commit_namespace_journal_and_lineage_together_and_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let ino;
        let first;
        {
            let q = paused(dir.path()).await;
            assert!(q.uses_store(q.store()));
            let other_dir = tempfile::tempdir().unwrap();
            assert!(!q.uses_store(&Arc::new(Store::open(other_dir.path()).unwrap())));
            ino = q.mount_transaction(|tx| {
                let ino = local_inode(tx)?;
                let mut create = Entry::new("d", "file", Op::Put, StoredBase::Absent);
                create.mount_ino = Some(ino);
                let mut attrs = Entry::new("d", "file", Op::Attrs, StoredBase::Any);
                attrs.mount_ino = Some(ino);
                Ok((ino, vec![create, attrs]))
            }).await.unwrap();
            let entries = q.store().with(|c| journal::all(c)).unwrap();
            assert_eq!(entries.len(), 2);
            first = entries[0].id;
            assert_eq!(entries[0].base, StoredBase::Absent);
            assert_eq!(entries[1].base, StoredBase::Entry(first));
            assert!(entries.iter().all(|e| e.mount && e.mount_ino == Some(ino) && e.source.is_none()));
            assert_eq!(q.st().pending.len(), 2);
            let last: EntryId = q.store().with(|c| c.query_row("SELECT entry_id FROM mount_inodes WHERE ino=?1", [ino], |r| r.get(0))).unwrap();
            assert_eq!(last, entries[1].id);
            q.close().await;
        }
        let q = paused(dir.path()).await;
        let entries = q.store().with(|c| journal::all(c)).unwrap();
        assert_eq!(entries[0].id, first);
        assert_eq!(entries[1].base, StoredBase::Entry(first));
        assert_eq!(entries[1].mount_ino, Some(ino));
        assert_eq!(q.status().await.unwrap().unpublished, 2);
        q.close().await;
    }

    #[tokio::test]
    async fn mount_transaction_rolls_back_semantic_and_journal_failures_without_queue_entries() {
        let dir = tempfile::tempdir().unwrap();
        let q = paused(dir.path()).await;
        let result: crate::mount::Result<()> = q.mount_transaction(|tx| {
            local_inode(tx)?;
            Err(crate::mount::FsError::Exists)
        }).await;
        assert_eq!(result, Err(crate::mount::FsError::Exists));
        q.store().with(|c| c.execute_batch("CREATE TRIGGER reject_journal BEFORE INSERT ON entries WHEN NEW.key='refuse' BEGIN SELECT RAISE(ABORT, 'injected journal failure'); END")).unwrap();
        let result: crate::mount::Result<()> = q.mount_transaction(|tx| {
            let ino = local_inode(tx)?;
            let mut first = Entry::new("d", "accepted", Op::Put, StoredBase::Absent);
            first.mount_ino = Some(ino);
            let mut second = Entry::new("d", "refuse", Op::Folder, StoredBase::Absent);
            second.mount_ino = Some(ino);
            Ok(((), vec![first, second]))
        }).await;
        assert!(result.is_err());
        assert_eq!(q.store().with(|c| c.query_row("SELECT count(*) FROM mount_inodes", [], |r| r.get::<_, u64>(0))).unwrap(), 0);
        assert!(q.store().with(|c| journal::all(c)).unwrap().is_empty());
        assert!(q.st().pending.is_empty());
        let result: crate::mount::Result<()> = q.mount_transaction(|tx| {
            local_inode(tx)?;
            let mut e = Entry::new("d", "missing-inode", Op::Put, StoredBase::Absent);
            e.mount_ino = Some(u64::MAX >> 1);
            Ok(((), vec![e]))
        }).await;
        assert_eq!(result, Err(crate::mount::FsError::Stale));
        assert_eq!(q.store().with(|c| c.query_row("SELECT count(*) FROM mount_inodes", [], |r| r.get::<_, u64>(0))).unwrap(), 0);
        assert!(q.store().with(|c| journal::all(c)).unwrap().is_empty());
        q.close().await;
        assert_eq!(q.mount_transaction(|_| Ok(((), Vec::new()))).await, Err(crate::mount::FsError::Io("the upload queue is closed".into())));
    }

    #[tokio::test]
    async fn mount_opposite_kind_recreations_wait_for_previous_occupants_to_leave() {
        let dir = tempfile::tempdir().unwrap();
        let q = paused(dir.path()).await;
        q.mount_transaction(|_| {
            let mut renamed = Entry::new("d", "renamed", Op::Rename, StoredBase::Version("base".into()));
            renamed.to_key = Some("destination".into());
            let mut replacement = Entry::new("d", "replace-source", Op::Rename, StoredBase::Version("base".into()));
            replacement.to_key = Some("café".into());
            let mut folder_rename = Entry::new("d", "re\u{301}pertoire/", Op::Rename, StoredBase::Version("base".into()));
            folder_rename.to_key = Some("de\u{301}place\u{301}/".into());
            Ok(((), vec![Entry::new("d", "file", Op::Delete, StoredBase::Version("base".into())),
                Entry::new("d", "file/", Op::Folder, StoredBase::Absent),
                Entry::new("d", "folder/", Op::Delete, StoredBase::Version("base".into())),
                Entry::new("d", "folder", Op::Put, StoredBase::Absent),
                renamed, Entry::new("d", "renamed/", Op::Folder, StoredBase::Absent),
                Entry::new("d", "cafe\u{301}", Op::Delete, StoredBase::Version("base".into())), replacement,
                folder_rename, Entry::new("d", "répertoire/source-child", Op::Delete, StoredBase::Version("base".into())),
                Entry::new("d", "déplacé/destination-child", Op::Put, StoredBase::Absent)]))
        }).await.unwrap();
        // Inspect the actual scheduler with a running queue's durable entries, without
        // allowing the background publisher to race the deterministic selection.
        let mut state = QState { pending: q.st().pending.clone(), ..Default::default() };
        let (runs, retry) = q.select(&mut state);
        let selected: Vec<_> = runs.iter().map(|run| (run[0].key.as_str(), run[0].op)).collect();
        assert_eq!(selected, [("file", Op::Delete), ("folder/", Op::Delete), ("renamed", Op::Rename),
            ("cafe\u{301}", Op::Delete), ("re\u{301}pertoire/", Op::Rename)]);
        assert!(retry.is_none());
        assert!(state.pending.values().filter(|e| matches!(e.op, Op::Put | Op::Folder)).all(|e| e.state == State::Queued && e.base == StoredBase::Absent));
        let cli_nfd = Entry::new("d", "cafe\u{301}/", Op::Delete, StoredBase::Any);
        let mut cli_nfc = Entry::new("d", "café/child", Op::Put, StoredBase::Any);
        assert!(!depends(&cli_nfd, &cli_nfc), "CLI-only dependencies retain raw Unicode spellings");
        cli_nfc.mount = true;
        assert!(depends(&cli_nfd, &cli_nfc), "a mount child conservatively waits for an equivalent CLI ancestor");
        q.close().await;
    }

    #[tokio::test]
    async fn mount_new_inodes_keep_absence_guards_and_existing_lineage_survives_clear_finished() {
        let dir = tempfile::tempdir().unwrap();
        let q = paused(dir.path()).await;
        q.delete("d", "file", Base::Version("old".into())).await.unwrap();
        q.mount_transaction(|tx| {
            let ino = local_inode(tx)?;
            let mut e = Entry::new("d", "file", Op::Put, StoredBase::Absent);
            e.mount_ino = Some(ino);
            Ok(((), vec![e]))
        }).await.unwrap();
        let create = q.store().with(|c| journal::all(c)).unwrap().pop().unwrap();
        assert_eq!(create.base, StoredBase::Absent, "a recreated inode is not based on the deleted inode");
        q.store().with(|c| {
            let mut e = create.clone();
            e.state = State::Done;
            e.version = Some("new".into());
            journal::update(c, &e)
        }).unwrap();
        q.clear_finished().await.unwrap();
        assert_eq!(q.guard(&StoredBase::Entry(create.id), true).unwrap(), Guard::Version("new".into()));
        let ino = create.mount_ino.unwrap();
        q.mount_transaction(move |_| {
            let mut e = Entry::new("d", "file", Op::Rename, StoredBase::Entry(create.id));
            e.mount_ino = Some(ino);
            e.to_key = Some("moved".into());
            Ok(((), vec![e]))
        }).await.unwrap();
        let rename = q.store().with(|c| journal::all(c)).unwrap().pop().unwrap();
        q.mount_transaction(|tx| {
            let ino = local_inode(tx)?;
            let mut e = Entry::new("d", "file", Op::Put, StoredBase::Absent);
            e.mount_ino = Some(ino);
            Ok(((), vec![e]))
        }).await.unwrap();
        let recreated = q.store().with(|c| journal::all(c)).unwrap().pop().unwrap();
        assert_eq!(recreated.base, StoredBase::Absent, "the renamed inode's version does not guard its replacement");
        assert!(depends(&rename, &recreated));
        q.mount_transaction(move |_| {
            let mut e = Entry::new("d", "moved", Op::Attrs, StoredBase::Entry(rename.id));
            e.mount_ino = Some(ino);
            Ok(((), vec![e]))
        }).await.unwrap();
        let attrs = q.store().with(|c| journal::all(c)).unwrap().pop().unwrap();
        assert_eq!(attrs.base, StoredBase::Entry(rename.id));
        assert!(depends(&rename, &attrs), "a rename's destination waits for its source publish");
        q.store().with(|c| {
            let mut e = rename.clone();
            e.state = State::Done;
            e.version = Some("renamed".into());
            journal::update(c, &e)
        }).unwrap();
        q.clear_finished().await.unwrap();
        assert_eq!(q.guard(&attrs.base, true).unwrap(), Guard::Version("renamed".into()));
        assert!(q.store().with(|c| journal::get(c, create.id)).unwrap().is_some(), "transitive lineage remains available");
        assert!(q.guard(&StoredBase::Entry(i64::MAX), true).is_err(), "missing lineage never silently removes a guard");
        q.store().with(|c| {
            let mut e = rename.clone();
            e.state = State::Done;
            e.version = None;
            journal::update(c, &e)
        }).unwrap();
        assert!(q.guard(&attrs.base, true).is_err(), "a recovered rename without a known version never silently removes a guard");
        q.close().await;
    }

    #[tokio::test]
    async fn empty_mount_files_publish_and_mount_conflicts_preserve_competing_objects() {
        let server = voidfs_server::test_server::TestServer::start().await.unwrap();
        let client = client(&server.endpoint);
        client.create_drive("drv", Default::default()).await.unwrap();
        let mut stale = Vec::new();
        for key in ["rename", "delete", "attrs"] {
            let base = client.put_object("drv", key, "before", Default::default()).await.unwrap().version_id;
            client.put_object("drv", key, "competing", Default::default()).await.unwrap();
            stale.push((key.to_owned(), base));
        }
        client.put_object("drv", "occupied", "competing", Default::default()).await.unwrap();
        client.put_object("drv", "folder/", Bytes::new(), Default::default()).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        store.set_meta("paused", "1").unwrap();
        store.with(|c| journal::insert(c, &mut Entry::new("drv", "missing-cli-source", Op::Put, StoredBase::Any))).unwrap();
        let q = Queue::open(store, client.clone(), QueueConfig::default()).await.unwrap();
        q.mount_transaction(move |_| {
            let mut entries = vec![Entry::new("drv", "empty", Op::Put, StoredBase::Absent),
                Entry::new("drv", "occupied", Op::Put, StoredBase::Absent), Entry::new("drv", "folder/", Op::Folder, StoredBase::Absent)];
            let mut missing = Entry::new("drv", "missing-mount-source", Op::Put, StoredBase::Absent);
            missing.size = 1;
            entries.push(missing);
            for (key, version) in stale {
                let op = match key.as_str() { "rename" => Op::Rename, "delete" => Op::Delete, _ => Op::Attrs };
                let mut e = Entry::new("drv", &key, op, StoredBase::Version(version));
                if op == Op::Rename { e.to_key = Some("renamed".into()); }
                if op == Op::Attrs { e.attrs.mode = Some(0o600); }
                entries.push(e);
            }
            Ok(((), entries))
        }).await.unwrap();
        q.resume(Scope::All).await.unwrap();
        q.settle().await;
        let status = q.status().await.unwrap();
        assert_eq!(status.items.iter().find(|i| i.key == "empty").unwrap().state, State::Done);
        assert_eq!(client.get_object("drv", "empty", Default::default()).await.unwrap().body, Bytes::new());
        assert!(status.items.iter().filter(|i| i.key != "empty").all(|i| i.state == State::Failed), "{:?}", status.items);
        for key in ["occupied", "folder/", "rename", "delete", "attrs"] {
            assert!(status.items.iter().find(|i| i.key == key).unwrap().conflict.is_some(), "a mount guard rejection is a durable structured conflict");
        }
        q.resume(Scope::All).await.unwrap();
        q.settle().await;
        assert!(q.status().await.unwrap().items.iter().filter(|i| i.conflict.is_some()).all(|i| i.state == State::Failed), "ordinary resume never resolves mount conflicts");
        for key in ["occupied", "rename", "delete", "attrs"] {
            assert_eq!(client.get_object("drv", key, Default::default()).await.unwrap().body.as_ref(), b"competing");
        }
        assert_eq!(client.head_object("drv", "renamed", Default::default()).await.unwrap_err().status(), Some(404));
        for key in ["missing-cli-source", "missing-mount-source"] {
            assert_eq!(client.head_object("drv", key, Default::default()).await.unwrap_err().status(), Some(404), "missing bytes never become an empty object");
        }
        assert_eq!(client.list_versions("drv", "folder/", false).await.unwrap().len(), 1, "guarded mkdir does not replace a competing folder");
        q.close().await;
    }

    #[tokio::test]
    async fn mount_completion_rolls_back_a_partial_run_without_cleaning_sources_or_ram() {
        let dir = tempfile::tempdir().unwrap();
        let q = paused(dir.path()).await;
        let a = q.mount_copy({ let p = dir.path().join("a"); std::fs::write(&p, "aa").unwrap(); p }, 0, 2).await.unwrap();
        let b = q.mount_copy({ let p = dir.path().join("b"); std::fs::write(&p, "bb").unwrap(); p }, 0, 2).await.unwrap();
        let sources = [a.clone(), b.clone()];
        q.mount_transaction(move |_| {
            let mut first = Entry::new("drive", "file", Op::Write, StoredBase::Version("base".into()));
            first.source = Some(a); first.staged = true; first.size = 2;
            let mut second = Entry::new("drive", "file", Op::Write, StoredBase::Any);
            second.source = Some(b); second.staged = true; second.size = 2; second.offset = 2;
            Ok(((), vec![first, second]))
        }).await.unwrap();
        let run = q.st().pending.values().cloned().collect::<Vec<_>>();
        q.store().with(|c| c.execute_batch(&format!("CREATE TEMP TRIGGER refuse_completion BEFORE UPDATE OF state ON entries WHEN NEW.id={} AND NEW.state='done' BEGIN SELECT RAISE(ABORT, 'completion failed'); END", run[1].id))).unwrap();
        assert!(q.finish(run.clone(), Outcome::Done { version: Some("published".into()), conflict: None }, None, 4, None).is_err());
        assert!(q.store().with(|c| journal::all(c)).unwrap().iter().all(|e| e.state == State::Queued), "the first journal update must roll back with the second");
        assert!(q.st().pending.values().all(|e| e.state == State::Queued), "RAM cannot advance ahead of the failed transaction");
        assert!(sources.iter().all(|p| p.exists()), "frozen sources remain available until completion commits");
        q.store().with(|c| c.execute_batch("DROP TRIGGER refuse_completion")).unwrap();
        q.finish(run, Outcome::Done { version: Some("published".into()), conflict: None }, None, 4, None).unwrap();
        assert!(q.store().with(|c| journal::all(c)).unwrap().iter().all(|e| e.state == State::Done));
        assert!(sources.iter().all(|p| !p.exists()));
        q.close().await;
    }

    #[tokio::test]
    async fn acknowledged_mount_publication_restarts_without_repeating_it_or_absorbing_later_writes() {
        let server = voidfs_server::test_server::TestServer::start().await.unwrap();
        let client = client(&server.endpoint);
        client.create_drive("drive", Default::default()).await.unwrap();
        let base = client.put_object("drive", "file", "before", Default::default()).await.unwrap().version_id;
        let published = client.put_object("drive", "file", "published", Default::default()).await.unwrap().version_id;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        store.set_meta("paused", "1").unwrap();
        let q = Queue::open(store.clone(), client.clone(), QueueConfig::default()).await.unwrap();
        let source = dir.path().join("snapshot"); std::fs::write(&source, "wrong").unwrap();
        let frozen = q.mount_copy(source, 0, 5).await.unwrap();
        q.mount_transaction(move |_| {
            let mut write = Entry::new("drive", "file", Op::Write, StoredBase::Version(base));
            write.source = Some(frozen); write.staged = true; write.size = 5;
            let mut truncate = Entry::new("drive", "file", Op::Truncate, StoredBase::Any); truncate.length = 9;
            Ok(((), vec![write, truncate]))
        }).await.unwrap();
        let first = q.st().pending.values().cloned().collect::<Vec<_>>();
        q.acknowledge(&first, &published, "file", false).unwrap();
        let source = dir.path().join("later"); std::fs::write(&source, "!").unwrap();
        let frozen = q.mount_copy(source, 0, 1).await.unwrap();
        let predecessor = first.last().unwrap().id;
        q.mount_transaction(move |_| {
            let mut write = Entry::new("drive", "file", Op::Write, StoredBase::Entry(predecessor));
            write.source = Some(frozen); write.staged = true; write.size = 1; write.offset = 9;
            Ok(((), vec![write]))
        }).await.unwrap();
        q.close().await; drop(q); drop(store);
        let q = Queue::open(Arc::new(Store::open(dir.path()).unwrap()), client.clone(), QueueConfig::default()).await.unwrap();
        q.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
        let status = q.status().await.unwrap();
        assert!(status.items.iter().all(|e| e.state == State::Done), "{:?}", status.items);
        assert!(status.items[..2].iter().all(|e| e.version.as_deref() == Some(published.as_str())), "the acknowledged run binds the version it already produced");
        assert_ne!(status.items[2].version.as_deref(), Some(published.as_str()), "later writes receive their own guarded publication");
        assert_eq!(client.get_object("drive", "file", Default::default()).await.unwrap().body.as_ref(), b"published!");
        assert_eq!(client.list_versions("drive", "file", true).await.unwrap().len(), 3, "the acknowledged run is never sent again");
        q.close().await;
    }

    #[tokio::test]
    async fn mount_guards_reject_cancelled_predecessors_without_falling_back_to_their_base() {
        let dir = tempfile::tempdir().unwrap();
        let q = paused(dir.path()).await;
        let mut e = Entry::new("drive", "file", Op::Write, StoredBase::Version("old-base".into()));
        e.mount = true; e.state = State::Cancelled;
        q.store().with(|c| journal::insert(c, &mut e)).unwrap();
        assert!(q.guard(&StoredBase::Entry(e.id), true).is_err(), "a skipped local predecessor changes the byte view and cannot retain its old guard");
        assert_eq!(q.guard(&StoredBase::Entry(e.id), false).unwrap(), Guard::Version("old-base".into()), "CLI cancellation policy is unchanged");
        q.close().await;
    }


    async fn partial_xattrs(already_applied: bool, foreign_content: bool) -> (tempfile::TempDir, Queue, Client, String, voidfs_server::test_server::TestServer) {
        let server = voidfs_server::test_server::TestServer::start().await.unwrap();
        let client = client(&server.endpoint);
        client.create_drive("drive", Default::default()).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap()); store.set_meta("paused", "1").unwrap();
        let q = Queue::open(store.clone(), client.clone(), QueueConfig::default()).await.unwrap();
        let source = dir.path().join("source"); std::fs::write(&source, "local").unwrap();
        let frozen = q.mount_copy(source, 0, 5).await.unwrap();
        q.mount_transaction(move |_| {
            let mut e = Entry::new("drive", "file", Op::Put, StoredBase::Absent);
            e.source = Some(frozen); e.staged = true; e.size = 5; e.attrs.xattrs.insert("user.test".into(), b"wanted".to_vec());
            Ok(((), vec![e]))
        }).await.unwrap();
        let run = q.st().pending.values().cloned().collect::<Vec<_>>();
        let version = client.put_object("drive", "file", "local", voidfs_sdk::PutOptions {
            metadata: std::collections::BTreeMap::from([(publish::MARKER.to_owned(), format!("{}.{}", q.0.state_id, run[0].id))]), ..Default::default()
        }).await.unwrap().version_id;
        q.acknowledge(&run, &version, "file", true).unwrap();
        let mut current = version.clone();
        if already_applied {
            current = client.set_attributes("drive", "file", voidfs_sdk::AttributesUpdate {
                set_xattrs: std::collections::BTreeMap::from([("user.test".into(), Bytes::from_static(b"wanted"))]), ..Default::default()
            }, voidfs_sdk::Preconditions::if_version(current)).await.unwrap().version_id;
        }
        if foreign_content {
            current = client.write_at("drive", "file", 0, "REMOTE", voidfs_sdk::WriteOptions { size: Some(6), if_version: Some(current), ..Default::default() }).await.unwrap().version_id;
        }
        q.close().await; drop(q); drop(store);
        let q = Queue::open(Arc::new(Store::open(dir.path()).unwrap()), client.clone(), QueueConfig::default()).await.unwrap();
        (dir, q, client, current, server)
    }

    #[tokio::test]
    async fn partial_mount_put_restarts_at_xattrs_and_recognizes_an_already_landed_attribute_version() {
        for already_applied in [false, true] {
            let (_dir, q, client, _, _server) = partial_xattrs(already_applied, false).await;
            q.resume(Scope::All).await.unwrap();
            tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
            assert_eq!(q.status().await.unwrap().items[0].state, State::Done, "a data put's acknowledged version must resume its attributes independently");
            let attrs = client.attributes("drive", "file", Default::default()).await.unwrap();
            assert_eq!(attrs.xattrs.get("user.test").map(String::as_str), Some("d2FudGVk"));
            assert_eq!(client.get_object("drive", "file", Default::default()).await.unwrap().body.as_ref(), b"local");
            assert_eq!(client.list_versions("drive", "file", true).await.unwrap().len(), 2, "an already-landed attrs response cannot make a second attrs version");
            assert!(q.store().with(|c| journal::all(c)).unwrap().iter().all(|e| !e.published_attrs));
            q.close().await;
        }
    }

    #[tokio::test]
    async fn partial_mount_put_never_accepts_foreign_content_with_matching_xattrs_as_its_own() {
        let (_dir, q, client, current, _server) = partial_xattrs(true, true).await;
        q.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
        let status = q.status().await.unwrap();
        assert_eq!(status.items[0].state, State::Failed);
        assert_eq!(status.items[0].conflict.as_deref(), Some(current.as_str()), "matching xattrs do not identify a content-changing commit as the put's own attrs request");
        assert_eq!(client.get_object("drive", "file", Default::default()).await.unwrap().body.as_ref(), b"REMOTE");
        assert_eq!(client.list_versions("drive", "file", true).await.unwrap().len(), 3);
        q.close().await;
    }


    #[tokio::test]
    async fn legacy_done_mount_entries_recover_identity_and_owned_overlays_without_republishing() {
        let server = voidfs_server::test_server::TestServer::start().await.unwrap();
        let client = client(&server.endpoint); client.create_drive("drive", Default::default()).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap()); store.set_meta("paused", "1").unwrap();
        let q = Queue::open(store.clone(), client.clone(), QueueConfig::default()).await.unwrap();
        let cache = crate::Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone())), Default::default()).await.unwrap();
        let session = crate::mount::Session::new_writable_with_config(store.clone(), client.clone(), cache.clone(), q.clone(), "drive", Default::default(), crate::mount::StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() }).await.unwrap();
        session.readdir(session.root(), None, 100).await.unwrap();
        let ino = session.create(session.root(), "file", 0o640).await.unwrap().ino;
        let fh = session.open(ino, true).await.unwrap();
        session.write(fh, 0, Bytes::from_static(b"local")).await.unwrap(); session.fsync(fh).await.unwrap();
        let published = client.put_object("drive", "file", "local", voidfs_sdk::PutOptions { mode: Some(0o640), ..Default::default() }).await.unwrap().version_id;
        store.with(|c| {
            for mut e in journal::all(c)? { e.state = State::Done; e.version = Some(published.clone()); journal::update(c, &e)?; }
            c.execute("DELETE FROM mount_overlay_publications", [])?;
            Ok(())
        }).unwrap();
        session.close(fh).await.unwrap(); drop(session); drop(cache); q.close().await; drop(q); drop(store);
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let q = Queue::open(store.clone(), client.clone(), QueueConfig::default()).await.unwrap();
        let cache = crate::Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone())), Default::default()).await.unwrap();
        let session = crate::mount::Session::new_writable_with_config(store.clone(), client.clone(), cache, q.clone(), "drive", Default::default(), crate::mount::StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() }).await.unwrap();
        q.resume(Scope::All).await.unwrap(); tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
        let attr = session.lookup(session.root(), "file").await.unwrap();
        assert_eq!(attr.ino, ino, "upgrading cannot replace the local inode");
        assert_eq!(attr.sync, crate::mount::Sync::Saved, "already-uploaded legacy files need reconciliation rather than another upload");
        assert_eq!(attr.version_id.as_deref(), Some(published.as_str()));
        assert!(attr.object_id.is_some());
        assert_eq!(store.with(|c| c.query_row("SELECT count(*) FROM mount_overlay", [], |r| r.get::<_, i64>(0))).unwrap(), 0);
        assert_eq!(store.with(|c| c.query_row("SELECT count(*) FROM mount_staged", [], |r| r.get::<_, i64>(0))).unwrap(), 0);
        assert_eq!(client.list_versions("drive", "file", true).await.unwrap().len(), 1);
        assert_eq!(q.status().await.unwrap().unpublished, 0);
        q.close().await;
    }


    #[tokio::test]
    async fn an_ambiguous_mount_put_marker_never_accepts_a_foreign_edit_as_its_publication() {
        let (_dir, q, client, _, _server) = partial_xattrs(false, false).await;
        {
            let mut st = q.st();
            for e in st.pending.values_mut() {
                e.published_version = None; e.published_key = None; e.published_attrs = false;
                q.store().with(|c| journal::update(c, e)).unwrap();
            }
        }
        let current = client.write_at("drive", "file", 0, "REMOTE", voidfs_sdk::WriteOptions { size: Some(6), ..Default::default() }).await.unwrap().version_id;
        q.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
        let status = q.status().await.unwrap();
        assert_eq!(status.items[0].state, State::Failed);
        assert_eq!(status.items[0].conflict.as_deref(), Some(current.as_str()), "an inherited put marker does not identify the current bytes as the lost put");
        assert_eq!(client.get_object("drive", "file", Default::default()).await.unwrap().body.as_ref(), b"REMOTE");
        assert_eq!(client.list_versions("drive", "file", true).await.unwrap().len(), 2);
        assert!(q.store().with(|c| journal::all(c)).unwrap()[0].source.as_ref().unwrap().exists());
        q.close().await;
    }


    /// A mount put guarded by `base` whose run was uploading when the client stopped, with the
    /// versions the server holds: its own put's, after someone else's edit if `foreign_first`.
    async fn lost_mount_put(foreign_first: bool) -> (tempfile::TempDir, Queue, Client, Vec<String>, voidfs_server::test_server::TestServer) {
        let server = voidfs_server::test_server::TestServer::start().await.unwrap();
        let client = client(&server.endpoint);
        client.create_drive("drive", Default::default()).await.unwrap();
        let mut versions = vec![client.put_object("drive", "file", "base", Default::default()).await.unwrap().version_id];
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap()); store.set_meta("paused", "1").unwrap();
        let q = Queue::open(store.clone(), client.clone(), QueueConfig::default()).await.unwrap();
        let source = dir.path().join("source"); std::fs::write(&source, "local").unwrap();
        let frozen = q.mount_copy(source, 0, 5).await.unwrap();
        let base = versions[0].clone();
        q.mount_transaction(move |_| {
            let mut e = Entry::new("drive", "file", Op::Put, StoredBase::Version(base));
            e.source = Some(frozen); e.staged = true; e.size = 5;
            Ok(((), vec![e]))
        }).await.unwrap();
        let id = q.st().pending.values().next().unwrap().id;
        let marker = format!("{}.{id}", q.0.state_id);
        if foreign_first {
            let foreign = voidfs_sdk::PutOptions { if_version: versions.last().cloned(), ..Default::default() };
            versions.push(client.put_object("drive", "file", "foreign", foreign).await.unwrap().version_id);
        }
        let own = voidfs_sdk::PutOptions { if_version: versions.last().cloned(), metadata: std::collections::BTreeMap::from([(publish::MARKER.to_owned(), marker)]), ..Default::default() };
        versions.push(client.put_object("drive", "file", "local", own).await.unwrap().version_id);
        store.with(|c| c.execute("UPDATE entries SET state='uploading'", [])).unwrap();
        q.close().await; drop(q); drop(store);
        let q = Queue::open(Arc::new(Store::open(dir.path()).unwrap()), client.clone(), QueueConfig::default()).await.unwrap();
        q.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
        (dir, q, client, versions, server)
    }

    #[tokio::test]
    async fn a_lost_mount_put_is_known_by_its_marker_on_the_version_right_after_its_guard() {
        let (_dir, q, client, versions, _server) = lost_mount_put(false).await;
        let item = q.status().await.unwrap().items.remove(0);
        assert_eq!((item.state, item.version.as_deref(), item.conflict.as_deref()), (State::Done, Some(versions[1].as_str()), None), "{item:?}");
        assert_eq!(client.list_versions("drive", "file", true).await.unwrap().len(), 2, "the put that landed is not sent again");
        q.close().await;
    }

    #[tokio::test]
    async fn a_marked_version_after_someone_elses_edit_is_not_the_lost_put() {
        let (_dir, q, client, versions, _server) = lost_mount_put(true).await;
        let item = q.status().await.unwrap().items.remove(0);
        assert_eq!((item.state, item.conflict.as_deref()), (State::Failed, Some(versions[2].as_str())), "{item:?}");
        assert_eq!(client.list_versions("drive", "file", true).await.unwrap().len(), 3);
        q.close().await;
    }

    async fn publication_session() -> (tempfile::TempDir, Queue, Client, crate::mount::Session, voidfs_server::test_server::TestServer) {
        let server = voidfs_server::test_server::TestServer::start().await.unwrap();
        let client = client(&server.endpoint); client.create_drive("drive", Default::default()).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap()); store.set_meta("paused", "1").unwrap();
        let q = Queue::open(store.clone(), client.clone(), QueueConfig::default()).await.unwrap();
        let cache = crate::Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone())), Default::default()).await.unwrap();
        let session = crate::mount::Session::new_writable_with_config(store, client.clone(), cache, q.clone(), "drive", Default::default(), crate::mount::StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() }).await.unwrap();
        (dir, q, client, session, server)
    }

    #[tokio::test]
    async fn a_guarded_folder_delete_that_keeps_new_children_is_a_conflict_and_retains_its_overlay() {
        let (_dir, q, client, session, _server) = publication_session().await;
        client.put_object("drive", "folder/", Bytes::new(), Default::default()).await.unwrap();
        session.readdir(session.root(), None, 100).await.unwrap();
        let ino = session.lookup(session.root(), "folder").await.unwrap().ino;
        session.readdir(ino, None, 100).await.unwrap();
        session.rmdir(session.root(), "folder").await.unwrap();
        client.put_object("drive", "folder/child", "foreign", Default::default()).await.unwrap();
        q.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
        let status = q.status().await.unwrap();
        assert_eq!(status.items[0].state, State::Failed, "a no-op delete response cannot acknowledge removal of an extant folder");
        assert!(status.items[0].conflict.is_some());
        assert!(matches!(session.lookup(session.root(), "folder").await, Err(crate::mount::FsError::NotFound)), "the local deletion tombstone remains visible during conflict");
        assert_eq!(client.get_object("drive", "folder/child", Default::default()).await.unwrap().body.as_ref(), b"foreign");
        let conflict = session.conflict(ino).await.unwrap().unwrap();
        assert_eq!(conflict.remote.unwrap().kind, voidfs_sdk::Kind::Folder);
        assert!(!conflict.remote_missing);
        assert!(conflict.remote_retained);
        q.close().await;
    }

    #[tokio::test]
    async fn a_missing_guarded_base_is_a_retained_absence_conflict_without_a_capture_error() {
        let server = voidfs_server::test_server::TestServer::start().await.unwrap();
        let client = client(&server.endpoint); client.create_drive("drive", Default::default()).await.unwrap();
        client.put_object("drive", "file", "before", Default::default()).await.unwrap();
        let rejected = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback({
            let rejected = rejected.clone(); let upstream = server.endpoint.clone(); let http = reqwest::Client::new();
            move |request: axum::extract::Request| {
                let (rejected, upstream, http) = (rejected.clone(), upstream.clone(), http.clone());
                async move {
                    if matches!(request.method().as_str(), "PUT" | "POST") {
                        rejected.fetch_add(1, Ordering::SeqCst);
                        return axum::response::Response::builder().status(412).header("content-type", "application/xml")
                            .body(axum::body::Body::from("<Error><Code>PreconditionFailed</Code><Message>the guarded source was removed</Message><RequestId>missing</RequestId></Error>")).unwrap();
                    }
                    let (parts, body) = request.into_parts();
                    let head = parts.method == axum::http::Method::HEAD;
                    let mut forwarded = http.request(parts.method, format!("{}{}", upstream, parts.uri.path_and_query().unwrap()));
                    for (name, value) in &parts.headers { forwarded = forwarded.header(name, value); }
                    let response = forwarded.body(axum::body::to_bytes(body, usize::MAX).await.unwrap()).send().await.unwrap();
                    let mut returned = axum::response::Response::builder().status(response.status());
                    for (name, value) in response.headers() {
                        if !matches!(name.as_str(), "transfer-encoding" | "connection") && (head || name.as_str() != "content-length") { returned = returned.header(name, value); }
                    }
                    returned.body(axum::body::Body::from(response.bytes().await.unwrap())).unwrap()
                }
            }
        });
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap()); store.set_meta("paused", "1").unwrap();
        let q = Queue::open(store.clone(), self::client(&endpoint), QueueConfig::default()).await.unwrap();
        let cache = crate::Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone())), Default::default()).await.unwrap();
        let session = crate::mount::Session::new_writable_with_config(store, client.clone(), cache, q.clone(), "drive", Default::default(), crate::mount::StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() }).await.unwrap();
        session.readdir(session.root(), None, 100).await.unwrap();
        let ino = session.lookup(session.root(), "file").await.unwrap().ino;
        let fh = session.open(ino, true).await.unwrap();
        session.write(fh, 0, Bytes::from_static(b"LOCAL!")).await.unwrap(); session.fsync(fh).await.unwrap();
        client.delete_object("drive", "file", Default::default()).await.unwrap();
        q.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
        assert_eq!(rejected.load(Ordering::SeqCst), 1, "the publication received a guarded 412 without a current version");
        let conflict = session.conflict(ino).await.unwrap().unwrap();
        assert!(conflict.remote_missing, "412 without a current version followed by HEAD 404 represents explicit remote absence");
        assert!(conflict.remote_retained && conflict.local_retained);
        assert!(conflict.remote.is_none());
        assert!(conflict.error.is_none(), "remote absence requires no downloadable competing bytes: {:?}", conflict.error);
        assert_eq!(session.read(fh, 0, 6).await.unwrap().as_ref(), b"LOCAL!");
        assert_eq!(q.status().await.unwrap().items[0].state, State::Failed);
        session.close(fh).await.unwrap(); q.close().await; task.abort();
    }

    #[tokio::test]
    async fn consistent_wrong_kind_metadata_is_rejected_before_publication_reconciliation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback(|| async {
            axum::response::Response::builder().status(200).header("x-amz-version-id", "version").header("x-voidfs-object-id", "object")
                .header("x-voidfs-kind", "folder").header("etag", "etag").body(axum::body::Body::from(serde_json::to_vec(&serde_json::json!({
                    "objectId": "object", "kind": "folder", "versionId": "version", "mode": "0644"
                })).unwrap())).unwrap()
        });
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap()); store.set_meta("paused", "1").unwrap();
        let q = Queue::open(store.clone(), client(&endpoint), Default::default()).await.unwrap();
        let ino = store.with(|c| { let tx = c.transaction()?; let ino = local_inode(&tx).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?; tx.commit()?; Ok(ino) }).unwrap();
        let mut entry = Entry::new("d", "file", Op::Attrs, StoredBase::Version("base".into())); entry.mount = true; entry.mount_ino = Some(ino);
        assert!(matches!(q.object(&entry, "file", Some("version".into()), true).await, Err(Error::Invalid(message)) if message.contains("kind")), "HEAD and attrs agreeing on the wrong kind still cannot redefine a local file");
        q.close().await; task.abort();
    }

    #[tokio::test]
    async fn an_acknowledged_publication_identity_collision_records_terminal_error_and_keeps_its_source() {
        let server = voidfs_server::test_server::TestServer::start().await.unwrap();
        let client = client(&server.endpoint); client.create_drive("drive", Default::default()).await.unwrap();
        let base = client.put_object("drive", "file", "before", Default::default()).await.unwrap().version_id;
        let object = client.head_object("drive", "file", Default::default()).await.unwrap().object_id.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap()); store.set_meta("paused", "1").unwrap();
        let q = Queue::open(store.clone(), client.clone(), Default::default()).await.unwrap();
        let ino = store.with(|c| {
            let tx = c.transaction()?;
            let ino = local_inode(&tx).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            tx.execute("UPDATE mount_inodes SET drive='drive' WHERE ino=?1", [ino])?;
            let attrs = voidfs_sdk::FolderEntry { name: "other".into(), kind: voidfs_sdk::Kind::File, object_id: object.clone(), version_id: Some(base.clone()),
                size: Some(6), etag: None, mtime: None, mode: None, has_xattrs: false, target: None };
            tx.execute("INSERT INTO mount_inodes(drive, object_id, attrs, sync) VALUES ('drive', ?1, ?2, 'pending')", rusqlite::params![object, serde_json::to_string(&attrs).unwrap()])?;
            tx.commit()?; Ok(ino)
        }).unwrap();
        let source = dir.path().join("source"); std::fs::write(&source, "LOCAL!").unwrap();
        let frozen = q.mount_copy(source, 0, 6).await.unwrap(); let retained = frozen.clone();
        q.mount_transaction(move |_| {
            let mut e = Entry::new("drive", "file", Op::Write, StoredBase::Version(base));
            e.source = Some(frozen); e.staged = true; e.size = 6; e.mount_ino = Some(ino);
            Ok(((), vec![e]))
        }).await.unwrap();
        q.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), q.settle()).await.expect("a permanent identity collision cannot keep retrying reconciliation forever");
        let status = q.status().await.unwrap();
        assert_eq!(status.items[0].state, State::Failed);
        assert!(status.items[0].error.as_deref().unwrap_or_default().contains("pending local inode"));
        assert_eq!(store.with(|c| c.query_row("SELECT sync FROM mount_inodes WHERE ino=?1", [ino], |r| r.get::<_, String>(0))).unwrap(), "error");
        assert_eq!(store.with(|c| c.query_row("SELECT object_id FROM mount_inodes WHERE ino=?1", [ino], |r| r.get::<_, Option<String>>(0))).unwrap(), None, "the failed binding rolls back before the error is recorded");
        let recorded = store.with(|c| journal::all(c)).unwrap();
        assert_eq!(recorded[0].published_version, Some(client.head_object("drive", "file", Default::default()).await.unwrap().version_id));
        assert!(retained.exists());
        assert!(!q.st().retry_at.contains_key(&recorded[0].id));
        assert_eq!(client.list_versions("drive", "file", true).await.unwrap().len(), 2);
        q.close().await;
    }


    #[tokio::test]
    async fn a_conflict_keeps_exact_remote_metadata_when_its_sparse_bytes_cannot_be_reserved() {
        let (_dir, q, client, session, _server) = publication_session().await;
        let put = client.put_object("drive", "file", "before", Default::default()).await.unwrap();
        let attrs = client.set_attributes("drive", "file", voidfs_sdk::AttributesUpdate {
            set_xattrs: std::collections::BTreeMap::from([("user.tag".into(), Bytes::from_static(b"remote metadata"))]), ..Default::default()
        }, voidfs_sdk::Preconditions::if_version(put.version_id)).await.unwrap();
        session.readdir(session.root(), None, 100).await.unwrap();
        let ino = session.lookup(session.root(), "file").await.unwrap().ino;
        let fh = session.open(ino, true).await.unwrap();
        session.write(fh, 0, Bytes::from_static(b"LOCAL!")).await.unwrap(); session.fsync(fh).await.unwrap();
        let size = 1u64 << 50;
        let remote = client.truncate("drive", "file", size, voidfs_sdk::Preconditions::if_version(attrs.version_id)).await.unwrap();
        q.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), q.settle()).await.unwrap();
        let conflict = session.conflict(ino).await.unwrap().unwrap();
        let retained = conflict.remote.as_ref().expect("byte admission failure cannot discard the exact remote identity and metadata");
        assert_eq!(retained.size, size);
        assert_eq!(retained.version_id.as_deref(), Some(remote.version_id.as_str()));
        assert_eq!(conflict.remote_attrs.as_ref().unwrap().xattrs.get("user.tag").map(String::as_str), Some("cmVtb3RlIG1ldGFkYXRh"));
        assert!(!conflict.remote_missing && !conflict.remote_retained);
        assert!(conflict.local_retained && conflict.error.is_some());
        assert_eq!(session.read_conflict(ino, crate::mount::ConflictSide::Local, 0, 6).await.unwrap().as_ref(), b"LOCAL!");
        assert!(matches!(session.read_conflict(ino, crate::mount::ConflictSide::Remote, 0, 1).await, Err(crate::mount::FsError::Io(_))));
        session.close(fh).await.unwrap(); q.close().await;
    }

}
