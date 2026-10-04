// SPDX-License-Identifier: Apache-2.0
//! The write journal and the upload queue (step 4, item 3).
//!
//! A change is durable on the Mac when [`Queue`]'s call for it returns: its bytes are in a file
//! of the journal's, synced, and its entry is in the state database, synced. The queue then
//! publishes it in the background:
//! - one key's changes in order, and a folder's after what was under it; other keys at once, up
//!   to 16 (SpaceFS's default);
//! - a put with the writes after it as one put, writes alone as one patch;
//! - each guarded by the version it was based on, with the `412` rule ([`crate::publish`]);
//! - pausable and cancellable for everything, a drive, a batch or one entry, and resumed after a
//!   restart, a multipart upload with the parts it had;
//! - within an upload bandwidth limit that applies at once ([`Queue::set_bandwidth`]);
//! - a large file that replaces a version the drive holds most of as a direct upload, which
//!   sends only the shards the pool lacks, straight to the bucket (protocol §4.11).
//!
//! Imports ([`Queue::import`]) are files of the user's, read where they are when they publish,
//! with their modification time, permission bits and extended attributes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, Semaphore};
use voidfs_sdk::{Bandwidth, Client};

use crate::connectivity::{Connectivity, Link};
use crate::error::Result;
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
    /// Entries that were being sent when the client last stopped.
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
    /// Set once the publisher has stopped and let go of the queue.
    stopped: Arc<(Mutex<bool>, Notify)>,
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

/// Whether entry `a`, earlier, must finish before `b` may start: they touch a key in common,
/// or one is a folder the other is in.
fn depends(a: &Entry, b: &Entry) -> bool {
    a.drive == b.drive && a.keys().any(|ka| b.keys().any(|kb| ka == kb || under(ka, kb) || under(kb, ka)))
}

impl Queue {
    /// Opens the journal in `store`, and starts publishing what it holds. `client` should not
    /// have an upload bandwidth limit of its own: the queue sets its.
    pub async fn open(store: Arc<Store>, client: Client, cfg: QueueConfig) -> Result<Queue> {
        let dir = store.dir().join("journal");
        let (s2, d2) = (store.clone(), dir.clone());
        let (entries, paused_all, drives, batches, bw, state_id) = blocking(move || {
            std::fs::create_dir_all(&d2)?;
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
                let ids: Vec<EntryId> = st.pending.values().filter(|e| e.state == State::Failed && in_scope(e, &scope)).map(|e| e.id).collect();
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
        let this = self.clone();
        blocking(move || {
            let mut st = this.st();
            let mut gone: BTreeSet<EntryId> = st.pending.values().filter(|e| in_scope(e, &scope)).map(|e| e.id).collect();
            loop {
                let more: Vec<EntryId> = st.pending.values().filter(|e| !gone.contains(&e.id) && matches!(e.base, StoredBase::Entry(b) if gone.contains(&b))).map(|e| e.id).collect();
                if more.is_empty() {
                    break;
                }
                gone.extend(more);
            }
            for id in gone {
                if let Some(r) = st.running.get(&id).cloned() {
                    // The publish stops and records the cancel itself.
                    st.cancelled.insert(id);
                    r.stop.signal(Why::Cancel);
                    continue;
                }
                if let Some(mut e) = st.pending.remove(&id) {
                    e.state = State::Cancelled;
                    this.0.store.with(|c| journal::update(c, &e))?;
                    if e.staged
                        && let Some(p) = &e.source
                    {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
            st.idle = false;
            Ok(())
        })
        .await?;
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
                c.execute("DELETE FROM entries WHERE state IN ('done', 'cancelled')", [])?;
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
                    let may_have_landed = st.may_have_landed.remove(&run[0].id);
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
            if taken.contains(id) || st.running.contains_key(id) || st.paused(e) {
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
            // A put takes the writes after it, writes take the writes after them.
            if matches!(e.op, Op::Put | Op::Write | Op::Truncate) && (e.op != Op::Put || e.size < self.0.cfg.multipart_from) {
                let mut body = e.size;
                for next in &ids[i + 1..] {
                    let n = &st.pending[next];
                    if !depends(e, n) {
                        continue;
                    }
                    let fits = n.drive == e.drive && n.key == e.key && matches!(n.op, Op::Write | Op::Truncate) && !st.paused(n) && n.state == State::Queued && n.batch == e.batch;
                    // A patch's truncate goes last: writes after it start a run of their own.
                    let after_truncate = e.op != Op::Put && run.last().is_some_and(|l: &Entry| l.op == Op::Truncate);
                    if !fits || after_truncate || body + n.size > 64 * MIB || run.len() >= 10_000 {
                        break;
                    }
                    body += n.size;
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
    fn guard(&self, base: &StoredBase) -> Result<Guard> {
        let mut base = base.clone();
        loop {
            match base {
                StoredBase::Any => return Ok(Guard::None),
                StoredBase::Absent => return Ok(Guard::Absent),
                StoredBase::Version(v) => return Ok(Guard::Version(v)),
                StoredBase::Entry(id) => match self.0.store.with(|c| journal::get(c, id))? {
                    Some(e) if e.state == State::Done => return Ok(e.version.map_or(Guard::None, Guard::Version)),
                    // Cancelled: whatever it was based on.
                    Some(e) => base = e.base,
                    None => return Ok(Guard::None),
                },
            }
        }
    }

    async fn run(&self, run: Vec<Entry>, r: Arc<Running>, may_have_landed: bool) {
        let this = self.clone();
        let base = run[0].base.clone();
        let upload_id = Arc::new(Mutex::new(run[0].upload_id.clone()));
        let outcome = match blocking(move || this.guard(&base)).await {
            Ok(guard) => {
                let ctx = Ctx {
                    client: self.0.client.clone(),
                    store: self.0.store.clone(),
                    state_id: self.0.state_id.clone(),
                    part_size: self.0.cfg.part_size,
                    multipart_from: self.0.cfg.multipart_from,
                    parts_at_once: self.0.cfg.parts_at_once,
                    direct_from: self.0.cfg.direct_from,
                    memory: self.0.memory.clone(),
                    memory_kib: self.0.memory_kib,
                    stop: r.stop.clone(),
                    sent: r.sent.clone(),
                    may_have_landed,
                    upload_id: upload_id.clone(),
                };
                publish::publish(&ctx, &run, guard).await
            }
            Err(e) => Outcome::from(e),
        };
        let outcome = match (outcome, r.stop.why()) {
            // A cancel that came as the publish finished is too late: it was published.
            (o @ Outcome::Done { .. }, _) => o,
            (_, Some(w)) => Outcome::Stopped(w),
            (o, None) => o,
        };
        let this = self.clone();
        let sent = r.sent.load(Ordering::Relaxed);
        let upload_id = upload_id.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let _ = blocking(move || this.finish(run, outcome, sent, upload_id)).await;
        self.0.wake.notify_one();
        self.0.changed.notify_waiters();
    }

    /// Records how a run ended.
    fn finish(&self, run: Vec<Entry>, outcome: Outcome, sent: u64, upload_id: Option<String>) -> Result<()> {
        let mut st = self.st();
        st.runs -= 1;
        for e in &run {
            st.running.remove(&e.id);
        }
        let mut finished = Vec::new();
        let closed = st.closed;
        for (i, e) in run.iter().enumerate() {
            let cancelled = st.cancelled.remove(&e.id);
            let Some(p) = st.pending.get_mut(&e.id) else { continue };
            if i == 0 {
                p.upload_id = upload_id.clone();
            }
            match &outcome {
                Outcome::Done { version, conflict } => {
                    p.state = State::Done;
                    p.version = version.clone();
                    p.sent = p.size;
                    p.error = None;
                    if i == 0 {
                        p.conflict = conflict.clone();
                    }
                }
                // The rest of a failed run are tried again when its first is.
                Outcome::Failed { error, .. } if i == 0 => {
                    p.state = State::Failed;
                    p.sent = sent.min(p.size);
                    p.error = Some(error.clone());
                }
                Outcome::Stopped(Why::Cancel) if cancelled => p.state = State::Cancelled,
                // Stopped by a pause, by closing, or by a cancel of another entry of the run: it
                // goes again. A request dropped part way may have landed; stopped by closing, it
                // is stored as uploading, so that the next open checks.
                _ => {
                    p.state = if closed { State::Uploading } else { State::Queued };
                    p.sent = if i == 0 { sent.min(p.size) } else { 0 };
                }
            }
            let p = p.clone();
            self.0.store.with(|c| journal::update(c, &p))?;
            if p.state == State::Queued && matches!(outcome, Outcome::Stopped(_)) {
                st.may_have_landed.insert(p.id);
            }
            if p.state.finished() {
                finished.push(p);
            }
        }
        if let Outcome::Failed { transient, .. } = &outcome {
            let id = run[0].id;
            let n = st.attempts.entry(id).or_insert(0);
            *n += 1;
            if *transient {
                let d = Duration::from_secs(1u64 << (*n).min(10)).min(self.0.cfg.retry_max);
                st.retry_at.insert(id, Instant::now() + d);
            }
        }
        for e in finished {
            st.pending.remove(&e.id);
            st.attempts.remove(&e.id);
            st.retry_at.remove(&e.id);
            if e.staged
                && let Some(p) = &e.source
            {
                let _ = std::fs::remove_file(p);
            }
            if e.state == State::Cancelled
                && let Some(id) = &e.upload_id
            {
                // Best effort: an upload left open is the garbage collector's.
                let (client, drive, key, id) = (self.0.client.clone(), e.drive.clone(), e.key.clone(), id.clone());
                tokio::spawn(async move {
                    let _ = client.abort_multipart_upload(&drive, &key, &id).await;
                });
            }
        }
        Ok(())
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
}
