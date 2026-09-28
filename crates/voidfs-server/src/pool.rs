// SPDX-License-Identifier: Apache-2.0
//! A pool (format §2–§3) and its drives: loading them from the bucket, committing to their
//! logs, checkpoints, forks, deletion, and the change feed (protocol §5.6).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use futures::StreamExt;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use voidfs_core::chunk::{Params, Shard};
use voidfs_core::ids::{DriveId, ObjectId, ShardHash, Timestamp, VersionId};
use voidfs_core::manifest::{self, Page};
use voidfs_core::model::{
    Chunking, Commit, CommitGuard, ContentDescriptor, DriveDescriptor, Extent, Features, ForkOf, Kind, Op, PoolDescriptor, Txn,
};
use voidfs_core::ops::OpError;
use voidfs_core::state::{DriveState, Rows};

use crate::clock::Clock;
use crate::gc::guard::{self, Guard};
use crate::probe;

/// A checkpoint is due this many commits after the last one (format §8.4)...
const CHECKPOINT_EVERY: u64 = 1000;
/// ...or after this many bytes of log, whichever comes first.
const CHECKPOINT_LOG_BYTES: u64 = 16 << 20;
/// Rows in a checkpoint segment, except that a table's last may have fewer (format §8.3).
const SEGMENT_MIN_ROWS: usize = 256;
const SEGMENT_MAX_ROWS: usize = 8192;
/// Between those bounds, a segment ends at a row whose key hash has this many low zero bits...
const SEGMENT_CUT_BITS: u32 = 12;
/// ...or, if it reaches the maximum first, at the last of its rows whose key hash has this many.
const SEGMENT_FALLBACK_BITS: u32 = 10;
/// Checkpoint segments fetched at once when loading (each is a round trip to the bucket).
const PAGE_FETCH_PARALLELISM: usize = 32;
/// A checkpoint lists the previous one's pages without storing them again only if that one's
/// index was seen this recently; otherwise it reads the index again first (format §12.4,
/// option 1).
const REUSE_WITHOUT_REREAD: Duration = Duration::from_secs(6 * 3600);
/// Change-feed batches kept in memory per drive.
const FEED_KEEP: usize = 10_000;
/// Log entries fetched at once when replaying (each is a round trip to the bucket).
const LOG_FETCH_PARALLELISM: usize = 32;
/// Drives loaded at once when a pool opens.
const DRIVE_LOAD_PARALLELISM: usize = 8;
/// Shards and pages remembered as safe to reference without uploading (format §12.4).
const REUSE_CAPACITY: usize = 1 << 20;

fn log_path(id: &DriveId, seq: u64) -> String {
    format!("drives/{id}/log/{seq:020}.json")
}

// ---------------------------------------------------------------------------------------------
// Change feed

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedChange {
    pub op: &'static str,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_key: Option<String>,
    pub object_id: ObjectId,
    pub version_id: VersionId,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub struct FeedBatch {
    pub seq: u64,
    pub time: Timestamp,
    pub changes: Vec<FeedChange>,
}

fn feed_for(commit: &Commit, before: &DriveState, after: &DriveState) -> FeedBatch {
    let mut changes = Vec::new();
    for (i, txn) in commit.txns.iter().enumerate() {
        let v = VersionId::new(commit.seq, i as u32);
        for c in &txn.changes {
            if let voidfs_core::model::Change::Create(c) = c
                && c.oid != txn.target && c.kind == Kind::Folder && before.record(&c.oid).is_none()
                    && let Some(key) = after.key_of(&c.oid) {
                        changes.push(FeedChange { op: "create", key, from_key: None, object_id: c.oid.clone(), version_id: v, kind: Kind::Folder });
                    }
        }
        let kind = after.kind(&txn.target).unwrap_or(Kind::File);
        let (key, from_key) = match txn.op {
            Op::Delete => (before.key_of(&txn.target), None),
            Op::Rename => (after.key_of(&txn.target), before.key_of(&txn.target)),
            _ => (after.key_of(&txn.target), None),
        };
        if let Some(key) = key {
            changes.push(FeedChange { op: txn.op.as_str(), key, from_key, object_id: txn.target.clone(), version_id: v, kind });
        }
    }
    FeedBatch { seq: commit.seq, time: commit.time, changes }
}

// ---------------------------------------------------------------------------------------------
// Checkpoints (format §8)

#[derive(Serialize, Deserialize)]
struct SegmentRef {
    page: ShardHash,
    first: String,
    last: String,
    count: usize,
}

#[derive(Serialize, Deserialize)]
struct CheckpointTables {
    entries: Vec<SegmentRef>,
    objects: Vec<SegmentRef>,
    history: Vec<SegmentRef>,
    #[serde(default)]
    removed: Vec<SegmentRef>,
}

impl CheckpointTables {
    /// Every page the index lists.
    fn pages(&self) -> HashSet<ShardHash> {
        self.entries.iter().chain(&self.objects).chain(&self.history).chain(&self.removed).map(|s| s.page).collect()
    }
}

#[derive(Serialize, Deserialize)]
struct CheckpointIndex {
    format: u32,
    seq: u64,
    #[serde(default)]
    time: Option<Timestamp>,
    tables: CheckpointTables,
    stats: serde_json::Value,
}

#[derive(Deserialize)]
struct Segment<T> {
    rows: Vec<T>,
}

#[derive(Serialize)]
struct SegmentOut<'a, T> {
    kind: &'static str,
    table: &'a str,
    rows: &'a [T],
}

/// How many low zero bits the SHA-256 of a row's key has (format §8.3), up to 16. The key is
/// encoded as in §8.2, and the digest read as a big-endian number.
fn key_zeros(key: &str) -> u32 {
    let h = ShardHash::of(key.as_bytes()).0;
    u16::from_be_bytes([h[30], h[31]]).trailing_zeros()
}

/// Where a table's segments end (format §8.3). Once a segment has [`SEGMENT_MIN_ROWS`], it ends
/// after the first row with [`SEGMENT_CUT_BITS`] [`key_zeros`]. If it reaches
/// [`SEGMENT_MAX_ROWS`] first, it ends after the last of its rows with
/// [`SEGMENT_FALLBACK_BITS`], or at the maximum if none has them.
///
/// A cut depends only on the rows near it and on where its segment began, so a row inserted or
/// removed rewrites the segment it falls in, and rarely any other. Cutting at the maximum would
/// not: every later segment would shift until the next cut by key. About one segment in seven
/// reaches the maximum; the fallback leaves about one in 2,300 to end there.
/// Returns the end (exclusive) of each segment.
fn segment_ends<T>(rows: &[T], key: impl Fn(&T) -> String) -> Vec<usize> {
    let mut ends = Vec::new();
    let (mut start, mut i, mut fallback) = (0, 0, None);
    while i < rows.len() {
        let n = i + 1 - start;
        let zeros = if n >= SEGMENT_MIN_ROWS { key_zeros(&key(&rows[i])) } else { 0 };
        if zeros >= SEGMENT_FALLBACK_BITS {
            fallback = Some(i + 1);
        }
        if zeros >= SEGMENT_CUT_BITS || n == SEGMENT_MAX_ROWS {
            let end = fallback.unwrap_or(i + 1);
            ends.push(end);
            (start, i, fallback) = (end, end, None);
        } else {
            i += 1;
        }
    }
    if start < rows.len() {
        ends.push(rows.len());
    }
    ends
}

fn segments<T: Serialize>(table: &str, rows: &[T], key: impl Fn(&T) -> String, pages: &mut Vec<Page>) -> Vec<SegmentRef> {
    let mut start = 0;
    segment_ends(rows, &key)
        .into_iter()
        .map(|end| {
            let chunk = &rows[start..end];
            start = end;
            let seg = SegmentOut { kind: "segment", table, rows: chunk };
            let bytes = Bytes::from(serde_json::to_vec(&seg).expect("rows serialize"));
            let page = ShardHash::of(&bytes);
            pages.push(Page { hash: page, bytes });
            SegmentRef { page, first: key(&chunk[0]), last: key(chunk.last().unwrap()), count: chunk.len() }
        })
        .collect()
}

/// A checkpoint index this server wrote or read. The drive's next checkpoint lists the same page
/// for every segment that has not changed (format §8.3), and need not store it again: this index
/// referenced it when it was seen, which is the first of the checks in format §12.4.
struct Checkpointed {
    index: String,
    pages: HashSet<ShardHash>,
    /// When the index was last seen in the bucket, on the monotonic clock: the check's `r`.
    seen: Duration,
}

/// What a drive's commit lock guards besides its log: when the next checkpoint is due (format
/// §8.4), and what the last one listed.
#[derive(Default)]
struct Cadence {
    /// Commits since the last checkpoint, or since an attempt at one failed.
    commits: u64,
    /// Their size as stored.
    bytes: u64,
    last: Option<Checkpointed>,
}

impl Cadence {
    fn add(&mut self, bytes: usize) {
        self.commits += 1;
        self.bytes += bytes as u64;
    }

    fn due(&self) -> bool {
        self.commits >= CHECKPOINT_EVERY || self.bytes >= CHECKPOINT_LOG_BYTES
    }
}

/// The rows of the next `n` segments that `pages` yields.
async fn rows_of<T: for<'de> Deserialize<'de>>(pages: &mut (impl futures::Stream<Item = anyhow::Result<Bytes>> + Unpin), n: usize) -> anyhow::Result<Vec<T>> {
    let mut out = Vec::new();
    for _ in 0..n {
        let bytes = pages.next().await.ok_or_else(|| anyhow!("a checkpoint segment went missing"))??;
        out.extend(serde_json::from_slice::<Segment<T>>(&bytes)?.rows);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Drives

pub struct Drive {
    pub id: DriveId,
    pub alias: String,
    pub desc: DriveDescriptor,
    state: RwLock<Arc<DriveState>>,
    commit_lock: tokio::sync::Mutex<Cadence>,
    feed: RwLock<VecDeque<FeedBatch>>,
    /// The oldest seq the feed can still report changes after.
    feed_floor: RwLock<u64>,
    notify: tokio::sync::watch::Sender<u64>,
    pub forks: RwLock<Vec<DriveId>>,
}

/// Why a commit did not happen.
#[derive(Debug)]
pub enum CommitError {
    /// The request is invalid against the current state.
    Op(OpError),
    /// Another writer changed what this commit was based on; plan again.
    Retry,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for CommitError {
    fn from(e: anyhow::Error) -> Self {
        CommitError::Other(e)
    }
}

impl From<OpError> for CommitError {
    fn from(e: OpError) -> Self {
        CommitError::Op(e)
    }
}

impl Drive {
    fn new(desc: DriveDescriptor, state: DriveState, cadence: Cadence) -> Drive {
        let (notify, _) = tokio::sync::watch::channel(state.seq());
        let floor = state.seq();
        Drive {
            id: desc.drive_id.clone(),
            alias: desc.alias.clone(),
            desc,
            state: RwLock::new(Arc::new(state)),
            commit_lock: tokio::sync::Mutex::new(cadence),
            feed: RwLock::new(VecDeque::new()),
            feed_floor: RwLock::new(floor),
            notify,
            forks: RwLock::new(Vec::new()),
        }
    }

    pub fn snapshot(&self) -> Arc<DriveState> {
        self.state.read().unwrap().clone()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.notify.subscribe()
    }

    /// Changes after `since`, or `None` if they are no longer held.
    pub fn changes_since(&self, since: u64) -> Option<Vec<FeedBatch>> {
        if since < *self.feed_floor.read().unwrap() {
            return None;
        }
        Some(self.feed.read().unwrap().iter().filter(|b| b.seq > since).cloned().collect())
    }

    fn push_feed(&self, batch: FeedBatch) {
        let mut feed = self.feed.write().unwrap();
        feed.push_back(batch);
        while feed.len() > FEED_KEEP {
            if let Some(old) = feed.pop_front() {
                *self.feed_floor.write().unwrap() = old.seq;
            }
        }
    }

    fn install(&self, state: DriveState, batch: FeedBatch) {
        let seq = state.seq();
        *self.state.write().unwrap() = Arc::new(state);
        self.push_feed(batch);
        let _ = self.notify.send(seq);
    }
}

// ---------------------------------------------------------------------------------------------
// Pool

pub struct Pool {
    pub store: crate::store::Store,
    pub desc: PoolDescriptor,
    pub params: Params,
    pub clock: Clock,
    /// Which shards and pages a commit may reference without uploading them (format §12.4).
    pub guard: Guard,
    shards: moka::sync::Cache<ShardHash, Bytes>,
    pages: moka::sync::Cache<ShardHash, Bytes>,
    drives: RwLock<HashMap<DriveId, Arc<Drive>>>,
    aliases: RwLock<HashMap<String, DriveId>>,
    deleted: RwLock<HashMap<String, DriveId>>,
    /// Serializes changes to `drives`, `aliases` and `deleted`. Never held across an await, and
    /// no other of their locks is held while taking it.
    registry: std::sync::Mutex<()>,
    authority: String,
}

impl Pool {
    /// Opens the pool in `store`, creating it if the store is empty.
    #[cfg_attr(not(test), allow(dead_code))]
    pub async fn open(store: crate::store::Store, cache_bytes: u64) -> anyhow::Result<Arc<Pool>> {
        Pool::open_with(store, cache_bytes, Clock::System).await
    }

    /// [`Pool::open`] with a given clock. A system clock also starts a task that re-reads
    /// `gc/pending.json` every minute.
    #[cfg_attr(not(test), allow(dead_code))]
    pub async fn open_with(store: crate::store::Store, cache_bytes: u64, clock: Clock) -> anyhow::Result<Arc<Pool>> {
        Pool::open_as(store, cache_bytes, clock, CommitGuard::CreateIfAbsent).await
    }

    /// [`Pool::open_with`] for a server started with `--commit-guard guard`. A new pool is
    /// created with that guard, and an existing one must have it. With create-if-absent, the
    /// store must first be seen to honour it (format §7.2): a pool it has just created in a store
    /// that does not is removed again.
    pub async fn open_as(store: crate::store::Store, cache_bytes: u64, clock: Clock, guard: CommitGuard) -> anyhow::Result<Arc<Pool>> {
        let (desc, bytes, created) = match store.get(probe::DESCRIPTOR).await? {
            Some(b) => (serde_json::from_slice::<PoolDescriptor>(&b).context("reading voidfs.json")?, b, false),
            None => {
                let d = PoolDescriptor {
                    format: voidfs_core::FORMAT_VERSION,
                    pool_id: format!("p-{}", uuid::Uuid::new_v4()),
                    created: clock.now(),
                    features: Features { compatible: vec![], incompatible: vec![] },
                    chunking: Chunking::default(),
                    hash: "sha256".into(),
                    commit_guard: guard,
                };
                let b = Bytes::from(serde_json::to_vec_pretty(&d)?);
                let created = match guard {
                    CommitGuard::CreateIfAbsent => store.put_new(probe::DESCRIPTOR, b.clone()).await.context("creating voidfs.json with a create-if-absent write (If-None-Match: *)")?,
                    // Only this server writes the pool (§7.3), and it has just seen no descriptor.
                    CommitGuard::External => {
                        store.put(probe::DESCRIPTOR, b.clone()).await?;
                        true
                    }
                };
                if !created {
                    return Box::pin(Pool::open_as(store, cache_bytes, clock, guard)).await;
                }
                (d, b, true)
            }
        };
        desc.check_readable().map_err(|e| anyhow!(e))?;
        if let Some(why) = probe::guard_mismatch(desc.commit_guard, guard) {
            bail!("{why}");
        }
        if guard == CommitGuard::CreateIfAbsent {
            let checked = probe::create_if_absent(&store, probe::DESCRIPTOR, bytes).await;
            if let Some(why) = checked.refusal() {
                if created && checked == probe::Conditional::Ignored {
                    // Nothing can have been written to it: any other server would refuse too.
                    let _ = store.delete(probe::DESCRIPTOR).await;
                    bail!("{why}. The pool this server had just created was removed. If this is the only server that will ever write the pool, start it with --commit-guard external (format §7.3)");
                }
                bail!("{why}. This pool relies on them, so this server will not write it");
            }
        }
        let params = Params::from_pool(&desc.chunking)?;
        let weigh = |_: &ShardHash, v: &Bytes| v.len().try_into().unwrap_or(u32::MAX);
        let pool = Arc::new(Pool {
            store,
            desc,
            params,
            clock,
            guard: Guard::new(REUSE_CAPACITY),
            shards: moka::sync::Cache::builder().weigher(weigh).max_capacity(cache_bytes).build(),
            pages: moka::sync::Cache::builder().weigher(weigh).max_capacity(cache_bytes / 8 + 1).build(),
            drives: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            registry: std::sync::Mutex::new(()),
            deleted: RwLock::new(HashMap::new()),
            authority: format!("a-{}", uuid::Uuid::new_v4()),
        });
        let ids: Vec<DriveId> = pool.store.list_dirs("drives/").await?.iter().filter_map(|d| d.parse().ok()).collect();
        let loads = futures::stream::iter(ids)
            .map(|id| {
                let pool = &pool;
                async move {
                    let loaded = pool.load_drive(&id).await;
                    let deleted = pool.store.exists(&format!("drives/{id}/deleted.json")).await;
                    (id, loaded, deleted)
                }
            })
            .buffer_unordered(DRIVE_LOAD_PARALLELISM)
            .collect::<Vec<_>>()
            .await;
        for (id, loaded, deleted) in loads {
            match loaded {
                Ok(Some(d)) => {
                    pool.register(d, deleted?);
                }
                Ok(None) => {}
                Err(e) => tracing::error!("drive {id} could not be loaded: {e:#}"),
            }
        }
        let forks: Vec<(DriveId, DriveId)> = pool
            .drives
            .read()
            .unwrap()
            .values()
            .filter_map(|d| d.desc.fork_of.as_ref().map(|f| (f.drive_id.clone(), d.id.clone())))
            .collect();
        for (parent, child) in forks {
            if let Some(p) = pool.drives.read().unwrap().get(&parent) {
                p.forks.write().unwrap().push(child);
            }
        }
        if let Err(e) = pool.guard.refresh(&pool.store, &pool.clock).await {
            tracing::warn!("{e:#}; writes will fail until it can be read");
        }
        if !pool.clock.is_manual() {
            let weak = Arc::downgrade(&pool);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(guard::REFRESH_EVERY).await;
                    let Some(pool) = weak.upgrade() else { break };
                    if let Err(e) = pool.guard.refresh(&pool.store, &pool.clock).await {
                        tracing::warn!("re-reading {}: {e:#}", crate::gc::PENDING);
                    }
                }
            });
        }
        Ok(pool)
    }

    /// Creates an object only if it does not exist yet, as the pool's commit guard does it, and
    /// returns `false` if it did (format §7.2, §7.3).
    pub async fn create(&self, path: &str, bytes: Bytes) -> anyhow::Result<bool> {
        match self.desc.commit_guard {
            CommitGuard::CreateIfAbsent => self.store.put_new(path, bytes).await,
            // Only this server writes the pool, so nothing can create the object between the
            // check and the write. The check alone would not be enough otherwise.
            CommitGuard::External => {
                if self.store.exists(path).await? {
                    return Ok(false);
                }
                self.store.put(path, bytes).await?;
                Ok(true)
            }
        }
    }

    fn register(&self, d: Drive, deleted: bool) -> Arc<Drive> {
        let d = Arc::new(d);
        let _r = self.registry.lock().unwrap();
        if deleted {
            self.deleted.write().unwrap().insert(d.alias.clone(), d.id.clone());
        } else {
            self.aliases.write().unwrap().insert(d.alias.clone(), d.id.clone());
        }
        self.drives.write().unwrap().insert(d.id.clone(), d.clone());
        d
    }

    async fn load_drive(&self, id: &DriveId) -> anyhow::Result<Option<Drive>> {
        let Some(b) = self.store.get(&format!("drives/{id}/drive.json")).await? else { return Ok(None) };
        let desc: DriveDescriptor = serde_json::from_slice(&b).context("reading drive.json")?;
        let mut cadence = Cadence::default();
        let mut state = match self.latest_checkpoint(id).await? {
            Some((s, c)) => {
                cadence.last = Some(c);
                s
            }
            None if desc.fork_of.is_some() => bail!("fork {id} has no checkpoint"),
            None => DriveState::empty(),
        };
        let drive_feed = self.replay(id, &mut state, &mut cadence).await?;
        let d = Drive::new(desc, state, cadence);
        for b in drive_feed {
            d.push_feed(b);
        }
        Ok(Some(d))
    }

    /// The drive's commits after `seq`, in order, each with its size as stored. Fetched
    /// concurrently, since each is a round trip to the bucket; stops at the first gap (format
    /// §8.4).
    async fn commits_after(&self, id: &DriveId, seq: u64) -> anyhow::Result<Vec<(Commit, usize)>> {
        let dir = format!("drives/{id}/log/");
        let names = self.store.list_files(&dir, Some(&format!("{seq:020}.json"))).await?;
        let mut stream = futures::stream::iter(names)
            .map(|name| {
                let path = format!("{dir}{name}");
                async move {
                    let bytes = self.store.get(&path).await?.ok_or_else(|| anyhow!("log entry {path} vanished"))?;
                    let c = serde_json::from_slice::<Commit>(&bytes).with_context(|| format!("parsing {path}"))?;
                    anyhow::Ok((c, bytes.len()))
                }
            })
            .buffered(LOG_FETCH_PARALLELISM);
        let mut out = Vec::new();
        let mut expect = seq + 1;
        while let Some(c) = stream.next().await {
            let (c, len) = c?;
            if c.seq != expect {
                break;
            }
            expect += 1;
            out.push((c, len));
        }
        Ok(out)
    }

    /// Applies every commit after `state.seq()`, counts them towards the next checkpoint, and
    /// returns their feed batches.
    async fn replay(&self, id: &DriveId, state: &mut DriveState, cadence: &mut Cadence) -> anyhow::Result<Vec<FeedBatch>> {
        let mut out = Vec::new();
        for (commit, len) in self.commits_after(id, state.seq()).await? {
            let name = log_path(id, commit.seq);
            if commit.seq != state.seq() + 1 {
                break; // a gap: stop at it (format §8.4)
            }
            let next = state.apply(&commit).with_context(|| format!("applying {name}"))?;
            out.push(feed_for(&commit, state, &next));
            *state = next;
            cadence.add(len);
        }
        Ok(out)
    }

    async fn latest_checkpoint(&self, id: &DriveId) -> anyhow::Result<Option<(DriveState, Checkpointed)>> {
        let dir = format!("drives/{id}/checkpoints/");
        let Some(name) = self.store.list_files(&dir, None).await?.into_iter().rfind(|n| n.ends_with(".json")) else {
            return Ok(None);
        };
        self.load_checkpoint(&format!("{dir}{name}")).await.map(Some)
    }

    /// Reads a checkpoint index and its segments into a state (format §8).
    async fn load_checkpoint(&self, path: &str) -> anyhow::Result<(DriveState, Checkpointed)> {
        let seen = self.clock.mono();
        let bytes = self.store.get(path).await?.ok_or_else(|| anyhow!("checkpoint {path} is missing"))?;
        let idx: CheckpointIndex = serde_json::from_slice(&bytes).with_context(|| format!("parsing {path}"))?;
        let t = &idx.tables;
        // Each segment is a round trip to the bucket: fetch them concurrently, and in order.
        let hashes: Vec<ShardHash> = t.entries.iter().chain(&t.objects).chain(&t.history).chain(&t.removed).map(|r| r.page).collect();
        let mut pages = std::pin::pin!(futures::stream::iter(hashes).map(|h| async move { self.page(&h).await }).buffered(PAGE_FETCH_PARALLELISM));
        let rows = Rows {
            entries: rows_of(&mut pages, t.entries.len()).await?,
            objects: rows_of(&mut pages, t.objects.len()).await?,
            history: rows_of(&mut pages, t.history.len()).await?,
            removed: rows_of(&mut pages, t.removed.len()).await?,
        };
        let state = DriveState::from_rows(idx.seq, idx.time, rows)?;
        Ok((state, Checkpointed { index: path.to_owned(), pages: t.pages(), seen }))
    }

    /// Writes a checkpoint of `state` (format §8) and returns it. Pages that `reuse` lists are
    /// not stored again; the others go through the garbage-collection guard like any page.
    async fn write_checkpoint(&self, id: &DriveId, state: &DriveState, reuse: Option<&Checkpointed>) -> anyhow::Result<Checkpointed> {
        let rows = state.rows();
        let mut pages = Vec::new();
        let tables = CheckpointTables {
            entries: segments("entries", &rows.entries, |e| format!("{}/{}{}", e.parent, e.name, if e.kind == Kind::Folder { "/" } else { "" }), &mut pages),
            objects: segments("objects", &rows.objects, |o| o.oid.to_string(), &mut pages),
            history: segments("history", &rows.history, |h| format!("{}@{:020}.{}", h.oid, h.version.seq, h.version.idx), &mut pages),
            removed: segments("removed", &rows.removed, |r| format!("{}@{}", r.key, r.oid), &mut pages),
        };
        let listed = tables.pages();
        let (reused, new): (Vec<Page>, Vec<Page>) = pages.into_iter().partition(|p| reuse.is_some_and(|r| r.pages.contains(&p.hash)));
        self.write_pages(&new).await?;
        if let Some(r) = reuse
            && !reused.is_empty()
            && self.clock.mono().saturating_sub(r.seen) > guard::COMMIT_WITHIN
        {
            bail!("the previous checkpoint was seen too long ago to vouch for its pages");
        }
        let idx = CheckpointIndex {
            format: 1,
            seq: state.seq(),
            time: state.time(),
            tables,
            stats: serde_json::json!({ "objects": rows.objects.len(), "bytes": state.live_bytes() }),
        };
        let name = format!("drives/{id}/checkpoints/{:020}.json", state.seq());
        self.store.put(&name, Bytes::from(serde_json::to_vec(&idx)?)).await?;
        let seen = self.clock.mono();
        self.store.put(&format!("drives/{id}/_last_checkpoint"), Bytes::from(format!("{{\"seq\":{}}}", state.seq()))).await?;
        Ok(Checkpointed { index: name, pages: listed, seen })
    }

    /// Makes `last` fit for a new checkpoint to list its pages without storing them: reads its
    /// index again if it was seen more than [`REUSE_WITHOUT_REREAD`] ago, and forgets it if that
    /// read fails.
    async fn renew(&self, last: &mut Option<Checkpointed>) {
        let stale = last.as_ref().filter(|c| self.clock.mono().saturating_sub(c.seen) > REUSE_WITHOUT_REREAD).map(|c| c.index.clone());
        if let Some(index) = stale {
            let seen = self.clock.mono();
            let got = self.store.get(&index).await;
            *last = match got {
                Ok(Some(b)) => serde_json::from_slice::<CheckpointIndex>(&b).ok().map(|idx| Checkpointed { pages: idx.tables.pages(), index, seen }),
                _ => None,
            };
        }
    }

    /// Checkpoints `state`, the drive's newest, and starts counting towards the next one. A
    /// failure is logged, and the next attempt waits a whole interval.
    async fn checkpoint(&self, id: &DriveId, state: &DriveState, cadence: &mut Cadence) {
        cadence.commits = 0;
        cadence.bytes = 0;
        self.renew(&mut cadence.last).await;
        let written = self.write_checkpoint(id, state, cadence.last.as_ref()).await;
        match written {
            Ok(c) => cadence.last = Some(c),
            Err(e) => tracing::warn!("checkpoint of {id} at {} failed: {e:#}", state.seq()),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Shards and pages

    pub async fn shard(&self, h: &ShardHash) -> anyhow::Result<Bytes> {
        if let Some(b) = self.shards.get(h) {
            return Ok(b);
        }
        let generation = self.guard.generation();
        let b = self.store.get(&guard::Kind::Shard.path(h)).await?.ok_or_else(|| anyhow!("shard {h} is missing"))?;
        if ShardHash::of(&b) != *h {
            bail!("shard {h} is corrupt");
        }
        self.guard.confirmed(*h, generation);
        self.shards.insert(*h, b.clone());
        Ok(b)
    }

    pub async fn page(&self, h: &ShardHash) -> anyhow::Result<Bytes> {
        if let Some(b) = self.pages.get(h) {
            return Ok(b);
        }
        let generation = self.guard.generation();
        let b = self.store.get(&guard::Kind::Page.path(h)).await?.ok_or_else(|| anyhow!("page {h} is missing"))?;
        self.guard.confirmed(*h, generation);
        self.pages.insert(*h, b.clone());
        Ok(b)
    }

    /// Stores shards so that a commit may reference them (format §7.4, §12.4). Shards already
    /// checked are not uploaded again; holding a shard's bytes in the cache is not a check.
    pub async fn write_shards(&self, shards: &[Shard]) -> anyhow::Result<()> {
        let items: Vec<(ShardHash, Bytes)> = shards.iter().map(|s| (s.hash, s.bytes.clone())).collect();
        self.guard.admit(&self.store, &self.clock, guard::Kind::Shard, &items).await?;
        for s in shards {
            self.shards.insert(s.hash, s.bytes.clone());
        }
        Ok(())
    }

    /// [`Pool::write_shards`] for manifest pages and checkpoint segments.
    pub async fn write_pages(&self, pages: &[Page]) -> anyhow::Result<()> {
        let items: Vec<(ShardHash, Bytes)> = pages.iter().map(|p| (p.hash, p.bytes.clone())).collect();
        self.guard.admit(&self.store, &self.clock, guard::Kind::Page, &items).await?;
        for p in pages {
            self.pages.insert(p.hash, p.bytes.clone());
        }
        Ok(())
    }

    /// Drops a deleted shard or page from the caches.
    pub fn forget(&self, h: &ShardHash) {
        self.shards.invalidate(h);
        self.pages.invalidate(h);
    }

    /// The full extent list of a content descriptor.
    pub async fn extents(&self, desc: &ContentDescriptor) -> anyhow::Result<Vec<Extent>> {
        if let ContentDescriptor::Tree { .. } = desc {
            // Prefetch the tree's pages; flatten then works from the cache.
            let mut pending = vec![match desc {
                ContentDescriptor::Tree { root, .. } => *root,
                _ => unreachable!(),
            }];
            while let Some(h) = pending.pop() {
                let bytes = self.page(&h).await?;
                if let Ok(voidfs_core::model::ManifestPage::Node { children }) = serde_json::from_slice(&bytes) {
                    pending.extend(children.iter().map(|c| c.page));
                }
            }
        }
        Ok(manifest::flatten(desc, &mut |h| self.pages.get(h))?)
    }

    /// Fetches the given shards for an edit.
    pub async fn fetch(&self, hashes: &[ShardHash]) -> anyhow::Result<HashMap<ShardHash, Bytes>> {
        let tasks = hashes.iter().map(|h| async move { Ok::<_, anyhow::Error>((*h, self.shard(h).await?)) });
        futures::future::join_all(tasks).await.into_iter().collect()
    }

    /// Describes content, storing any manifest pages it needs.
    pub async fn describe(&self, extents: Vec<Extent>) -> anyhow::Result<ContentDescriptor> {
        let (desc, pages) = manifest::describe(extents);
        self.write_pages(&pages).await?;
        Ok(desc)
    }

    // -----------------------------------------------------------------------------------------
    // Drives

    /// Finds a live drive by alias or id.
    pub fn drive(&self, name: &str) -> Option<Arc<Drive>> {
        let id = match name.parse::<DriveId>() {
            Ok(id) if !self.deleted.read().unwrap().values().any(|d| *d == id) => id,
            Ok(_) => return None,
            Err(_) => self.aliases.read().unwrap().get(name)?.clone(),
        };
        self.drives.read().unwrap().get(&id).cloned()
    }

    pub fn drive_by_id(&self, id: &DriveId) -> Option<Arc<Drive>> {
        self.drives.read().unwrap().get(id).cloned()
    }

    pub fn list_drives(&self) -> Vec<Arc<Drive>> {
        let aliases = self.aliases.read().unwrap();
        let drives = self.drives.read().unwrap();
        let mut out: Vec<_> = aliases.values().filter_map(|id| drives.get(id).cloned()).collect();
        out.sort_by(|a, b| a.alias.cmp(&b.alias));
        out
    }

    fn alias_taken(&self, alias: &str) -> bool {
        self.aliases.read().unwrap().contains_key(alias)
    }

    /// Creates a drive, or a fork of `source` (protocol §5.1, §5.2, format §6, §9).
    pub async fn create_drive(&self, alias: &str, source: Option<&Arc<Drive>>) -> Result<Arc<Drive>, CreateError> {
        if self.alias_taken(alias) {
            return Err(CreateError::Exists);
        }
        let id = DriveId::generate();
        let mut cadence = Cadence::default();
        let (state, fork_of, deadline) = match source {
            None => (DriveState::empty(), None, None),
            Some(src) => {
                // Hold the source's commit lock so the fork includes every acknowledged write.
                let mut src_cadence = src.commit_lock.lock().await;
                // The fork references content through the source's state, so the source must
                // still be a drive (format §12.4): a hard delete forgets it before deleting it.
                if self.drive_by_id(&src.id).is_none() {
                    return Err(CreateError::NotFound);
                }
                let s = src.snapshot().fork();
                // The fork's first checkpoint shares every segment that is unchanged since the
                // source's last one (format §9). That one's index vouches for them until 12 hours
                // after it was seen (§12.4), so the fork must exist by then.
                self.renew(&mut src_cadence.last).await;
                let deadline = src_cadence.last.as_ref().map(|r| r.seen + guard::COMMIT_WITHIN);
                cadence.last = Some(self.write_checkpoint(&id, &s, src_cadence.last.as_ref()).await?);
                let seq = s.seq();
                (s, Some(ForkOf { drive_id: src.id.clone(), seq }), deadline)
            }
        };
        let desc = DriveDescriptor { format: 1, drive_id: id.clone(), created: self.clock.now(), alias: alias.to_owned(), fork_of };
        if deadline.is_some_and(|t| self.clock.mono() > t) {
            let _ = self.store.delete_prefix(&format!("drives/{id}/")).await;
            return Err(CreateError::Other(anyhow!("creating the fork took too long; try again")));
        }
        if !self.create(&format!("drives/{id}/drive.json"), Bytes::from(serde_json::to_vec_pretty(&desc).map_err(anyhow::Error::from)?)).await? {
            return Err(CreateError::Other(anyhow!("drive id collision")));
        }
        // The fork's checkpoint is a garbage-collection root from now on (format §12).
        if let Some(c) = &mut cadence.last {
            c.seen = self.clock.mono();
        }
        if self.alias_taken(alias) {
            let _ = self.store.delete_prefix(&format!("drives/{id}/")).await;
            return Err(CreateError::Exists);
        }
        if let Some(src) = source {
            src.forks.write().unwrap().push(id.clone());
        }
        Ok(self.register(Drive::new(desc, state, cadence), false))
    }

    pub async fn soft_delete(&self, d: &Drive) -> anyhow::Result<()> {
        let marker = serde_json::json!({ "time": self.clock.now(), "actor": { "kind": "system", "id": "voidfs" } });
        self.store.put(&format!("drives/{}/deleted.json", d.id), Bytes::from(marker.to_string())).await?;
        let _r = self.registry.lock().unwrap();
        if !self.drives.read().unwrap().contains_key(&d.id) {
            return Ok(());
        }
        let mut aliases = self.aliases.write().unwrap();
        if aliases.get(&d.alias) == Some(&d.id) {
            aliases.remove(&d.alias);
        }
        drop(aliases);
        self.deleted.write().unwrap().insert(d.alias.clone(), d.id.clone());
        Ok(())
    }

    pub async fn undelete(&self, alias: &str) -> Result<Arc<Drive>, CreateError> {
        let id = self.deleted.read().unwrap().get(alias).cloned().ok_or(CreateError::NotFound)?;
        if self.alias_taken(alias) {
            return Err(CreateError::Exists);
        }
        // Garbage collection may have hard-deleted it since this server loaded it (format §10).
        if !self.store.exists(&format!("drives/{id}/drive.json")).await? {
            self.forget_drive(&id);
            return Err(CreateError::NotFound);
        }
        self.store.delete(&format!("drives/{id}/deleted.json")).await?;
        let _r = self.registry.lock().unwrap();
        // A hard delete may have removed it meanwhile, or another drive taken the alias.
        let d = self.drive_by_id(&id).ok_or(CreateError::NotFound)?;
        if self.alias_taken(alias) {
            return Err(CreateError::Exists);
        }
        self.deleted.write().unwrap().remove(alias);
        self.aliases.write().unwrap().insert(alias.to_owned(), id);
        Ok(d)
    }

    /// Permanently deletes a drive, live or soft-deleted. Its forks keep working: each has its
    /// own checkpoint (format §9).
    pub async fn hard_delete(&self, name: &str) -> anyhow::Result<bool> {
        let id = match self.drive(name) {
            Some(d) => d.id.clone(),
            None => match self.deleted.read().unwrap().get(name) {
                Some(id) => id.clone(),
                None => return Ok(false),
            },
        };
        self.hard_delete_id(&id).await?;
        Ok(true)
    }

    /// Permanently deletes a drive by id, whether or not this server has loaded it.
    ///
    /// The drive is forgotten first, so that no new request can reference content through it,
    /// and commits and forks already running finish before `drive.json` goes, which is when the
    /// drive stops being a garbage-collection root. If the deletion fails part way, the server
    /// no longer serves the drive, and what is left in the bucket is loaded again on restart
    /// only if `drive.json` survived.
    pub async fn hard_delete_id(&self, id: &DriveId) -> anyhow::Result<()> {
        let d = self.drive_by_id(id);
        self.forget_drive(id);
        let _g = match &d {
            Some(d) => Some(d.commit_lock.lock().await),
            None => None,
        };
        self.store.delete(&format!("drives/{id}/drive.json")).await?;
        self.store.delete_prefix(&format!("drives/{id}/")).await?;
        Ok(())
    }

    fn forget_drive(&self, id: &DriveId) {
        let _r = self.registry.lock().unwrap();
        let removed = self.drives.write().unwrap().remove(id);
        if let Some(d) = removed {
            let mut aliases = self.aliases.write().unwrap();
            if aliases.get(&d.alias) == Some(id) {
                aliases.remove(&d.alias);
            }
            drop(aliases);
            let mut deleted = self.deleted.write().unwrap();
            if deleted.get(&d.alias) == Some(id) {
                deleted.remove(&d.alias);
            }
        }
        for other in self.drives.read().unwrap().values() {
            other.forks.write().unwrap().retain(|f| f != id);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Commits

    /// Plans a transaction against the drive's current state and commits it (format §7).
    /// `plan` runs under the drive's commit lock, so preconditions it checks are sound.
    /// Shards and pages the transaction references must already be stored.
    pub async fn commit(
        &self,
        d: &Drive,
        plan: impl FnOnce(&DriveState) -> Result<Txn, CommitError>,
    ) -> Result<(VersionId, Arc<DriveState>), CommitError> {
        let mut cadence = d.commit_lock.lock().await;
        let cur = d.snapshot();
        let txn = plan(&cur)?;
        let now = self.clock.now();
        let time = cur.time().map_or(now, |t| t.max(now));
        let commit = Commit { format: 1, seq: cur.seq() + 1, time, authority: self.authority.clone(), txns: vec![txn] };
        let next = cur.apply(&commit).map_err(|e| CommitError::Other(anyhow!("planned transaction does not apply: {e}")))?;
        let bytes = Bytes::from(serde_json::to_vec(&commit).map_err(anyhow::Error::from)?);
        let len = bytes.len();
        if !self.create(&log_path(&d.id, commit.seq), bytes).await? {
            // Another authority wrote this sequence number: catch up and ask for a new plan.
            let mut s = (*cur).clone();
            let batches = self.replay(&d.id, &mut s, &mut cadence).await?;
            let mut last = None;
            for b in batches {
                last = Some(b.clone());
                d.push_feed(b);
            }
            *d.state.write().unwrap() = Arc::new(s);
            if let Some(b) = last {
                let _ = d.notify.send(b.seq);
            }
            return Err(CommitError::Retry);
        }
        let batch = feed_for(&commit, &cur, &next);
        let next = Arc::new(next);
        d.install((*next).clone(), batch);
        cadence.add(len);
        if cadence.due() {
            self.checkpoint(&d.id, &next, &mut cadence).await;
        }
        Ok((VersionId::new(commit.seq, 0), next))
    }

    /// The drive's state at instant `t`, rebuilt from its checkpoints and log.
    pub async fn state_at(&self, d: &Drive, t: Timestamp) -> anyhow::Result<DriveState> {
        let mut state = DriveState::empty();
        if let Some(fork) = &d.desc.fork_of {
            // A fork's namespace before its own log exists only as its first checkpoint, so its
            // point-in-time window starts at the fork point (format §8.5, §9).
            state = self.load_checkpoint(&format!("drives/{}/checkpoints/{:020}.json", d.id, fork.seq)).await?.0;
            if state.time().is_some_and(|ct| ct > t) {
                bail!("that instant is before this fork was made");
            }
        }
        for (commit, _) in self.commits_after(&d.id, state.seq()).await? {
            if commit.time > t {
                break;
            }
            state = state.apply(&commit)?;
        }
        Ok(state)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    #[error("the drive already exists")]
    Exists,
    #[error("no such drive")]
    NotFound,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::FutureExt;
    use voidfs_core::model::{Actor, Attrs, Change, CreateChange, RemoveChange, SetChange};
    use voidfs_core::names::Key;
    use voidfs_core::ops::{self, Precondition};

    use super::*;
    use crate::store::{Fault, MemOp, MemStore, Store};

    const HOUR: Duration = Duration::from_secs(3600);

    async fn put(pool: &Pool, d: &Drive, key: &str, data: &'static [u8]) -> VersionId {
        let e = voidfs_core::content::from_bytes(&Bytes::from_static(data), pool.params);
        pool.write_shards(&e.new_shards).await.unwrap();
        let desc = pool.describe(e.extents).await.unwrap();
        let key = key.to_owned();
        pool.commit(d, |s| Ok(ops::put(s, &key, desc, Attrs::default(), Op::Put, &Precondition::default(), &Actor::system())?))
            .await
            .unwrap()
            .0
    }

    async fn read(pool: &Pool, d: &Drive, key: &str) -> Option<Vec<u8>> {
        let s = d.snapshot();
        let r = s.record(&s.lookup(&Key::parse(key).unwrap())?)?.clone();
        let mut out = Vec::new();
        for e in pool.extents(r.content.as_ref().unwrap()).await.unwrap() {
            match e {
                Extent::Shard { s, .. } => out.extend_from_slice(&pool.shard(&s).await.unwrap()),
                Extent::Zero { z } => out.resize(out.len() + z as usize, 0),
            }
        }
        Some(out)
    }

    #[tokio::test]
    async fn drives_forks_and_checkpoints_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("voidfs-pool-{}", uuid::Uuid::new_v4()));
        let store = Store::local(&dir).unwrap();
        {
            let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
            let a = pool.create_drive("alpha", None).await.unwrap();
            let v1 = put(&pool, &a, "docs/a.txt", b"first").await;
            // Enough commits to cross the automatic checkpoint, so reload uses checkpoint + tail.
            for _ in 0..CHECKPOINT_EVERY {
                put(&pool, &a, "counter", b"tick").await;
            }
            assert!(store.exists(&format!("drives/{}/checkpoints/{:020}.json", a.id, CHECKPOINT_EVERY)).await.unwrap());
            let b = pool.create_drive("beta", Some(&a)).await.unwrap();
            put(&pool, &b, "docs/a.txt", b"forked").await;
            put(&pool, &a, "docs/only-alpha.txt", b"alpha").await;
            let c = pool.create_drive("gamma", None).await.unwrap();
            pool.soft_delete(&c).await.unwrap();
            assert_eq!(read(&pool, &b, "docs/only-alpha.txt").await, None);
            assert_eq!(b.snapshot().version(&v1).map(|r| r.size), Some(5));
        }
        {
            let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
            let a = pool.drive("alpha").unwrap();
            let b = pool.drive("beta").unwrap();
            assert!(pool.drive("gamma").is_none(), "soft-deleted drives stay deleted");
            assert_eq!(read(&pool, &a, "docs/a.txt").await.unwrap(), b"first");
            assert_eq!(read(&pool, &a, "docs/only-alpha.txt").await.unwrap(), b"alpha");
            assert_eq!(read(&pool, &b, "docs/a.txt").await.unwrap(), b"forked");
            assert_eq!(a.snapshot().seq(), CHECKPOINT_EVERY + 2);
            assert_eq!(*a.forks.read().unwrap(), vec![b.id.clone()]);
            assert!(pool.hard_delete("alpha").await.unwrap());
            assert!(pool.undelete("gamma").await.is_ok());
        }
        {
            let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
            assert!(pool.drive("alpha").is_none());
            assert!(pool.drive("gamma").is_some());
            let b = pool.drive("beta").unwrap();
            assert_eq!(read(&pool, &b, "docs/a.txt").await.unwrap(), b"forked");
            // History from before the fork is still readable after the parent is gone.
            let s = b.snapshot();
            let oid = s.lookup(&Key::parse("docs/a.txt").unwrap()).unwrap();
            assert_eq!(s.history(&oid).unwrap().len(), 2);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_second_authority_cannot_fork_history() {
        let store = Store::memory().unwrap();
        let p1 = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d1 = p1.create_drive("shared", None).await.unwrap();
        let p2 = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d2 = p2.drive("shared").unwrap();
        put(&p1, &d1, "x", b"from one").await;
        // p2 has not seen p1's commit: its first attempt must be refused, then it catches up.
        let e = voidfs_core::content::from_bytes(&Bytes::from_static(b"from two"), p2.params);
        p2.write_shards(&e.new_shards).await.unwrap();
        let desc = p2.describe(e.extents).await.unwrap();
        let first = p2
            .commit(&d2, |s| Ok(ops::put(s, "y", desc.clone(), Attrs::default(), Op::Put, &Precondition::default(), &Actor::system())?))
            .await;
        assert!(matches!(first, Err(CommitError::Retry)));
        assert_eq!(d2.snapshot().seq(), 1, "caught up with the other authority's commit");
        let (v, _) = p2
            .commit(&d2, |s| Ok(ops::put(s, "y", desc, Attrs::default(), Op::Put, &Precondition::default(), &Actor::system())?))
            .await
            .unwrap();
        assert_eq!(v, VersionId::new(2, 0));
    }

    // -----------------------------------------------------------------------------------------
    // Commit guards (format §7.2, §7.3)

    /// Makes every create-if-absent write to `mem` behave as `fault` says.
    fn on_put_new(mem: &MemStore, fault: Fault) {
        mem.set_hook(Some(Arc::new(move |op, _| futures::future::ready(if op == MemOp::PutNew { fault } else { Fault::None }).boxed())));
    }

    #[tokio::test]
    async fn a_pool_does_not_open_where_create_if_absent_is_not_honoured() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let store = Store::Mem(mem.clone());
        let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
        put(&pool, &pool.create_drive("d", None).await.unwrap(), "x", b"x").await;
        let before = mem.peek("voidfs.json").unwrap();
        on_put_new(&mem, Fault::Unconditional);
        let e = Pool::open(store.clone(), 1 << 20).await.err().unwrap();
        assert!(format!("{e:#}").contains("ignores create-if-absent"), "{e:#}");
        assert_eq!(mem.peek("voidfs.json").unwrap(), before, "the check rewrote the same bytes");
        // A check that fails tells nothing, so it refuses too.
        on_put_new(&mem, Fault::Fail);
        let e = Pool::open(store.clone(), 1 << 20).await.err().unwrap();
        assert!(format!("{e:#}").contains("could not confirm"), "{e:#}");
        assert_eq!(mem.peek("voidfs.json").unwrap(), before);
        mem.set_hook(None);
        let pool = Pool::open(store, 1 << 20).await.unwrap();
        assert!(pool.drive("d").is_some());
    }

    #[tokio::test]
    async fn a_pool_just_created_where_create_if_absent_is_ignored_is_removed() {
        let mem = Arc::new(MemStore::new(Clock::System));
        on_put_new(&mem, Fault::Unconditional);
        let e = Pool::open(Store::Mem(mem.clone()), 1 << 20).await.err().unwrap();
        assert!(format!("{e:#}").contains("--commit-guard external"), "{e:#}");
        assert!(mem.peek("voidfs.json").is_none());
    }

    #[tokio::test]
    async fn the_external_guard_checks_before_each_write() {
        // The store rejects conditional writes outright; the external guard sends none.
        let mem = Arc::new(MemStore::new(Clock::System));
        on_put_new(&mem, Fault::Fail);
        let store = Store::Mem(mem.clone());
        let p1 = Pool::open_as(store.clone(), 1 << 20, Clock::System, CommitGuard::External).await.unwrap();
        let desc: PoolDescriptor = serde_json::from_slice(&mem.peek("voidfs.json").unwrap()).unwrap();
        assert_eq!(desc.commit_guard, CommitGuard::External);
        let d1 = p1.create_drive("shared", None).await.unwrap();
        // Two servers on one external pool break its rule, but show that the check before each
        // write finds a commit already there.
        let p2 = Pool::open_as(store.clone(), 1 << 20, Clock::System, CommitGuard::External).await.unwrap();
        let d2 = p2.drive("shared").unwrap();
        put(&p1, &d1, "x", b"from one").await;
        let e = voidfs_core::content::from_bytes(&Bytes::from_static(b"from two"), p2.params);
        p2.write_shards(&e.new_shards).await.unwrap();
        let desc = p2.describe(e.extents).await.unwrap();
        let plan = |s: &DriveState| Ok(ops::put(s, "y", desc.clone(), Attrs::default(), Op::Put, &Precondition::default(), &Actor::system())?);
        assert!(matches!(p2.commit(&d2, plan).await, Err(CommitError::Retry)));
        assert_eq!(p2.commit(&d2, plan).await.unwrap().0, VersionId::new(2, 0));
        // A pool keeps its guard, whichever it has.
        let e = Pool::open(store, 1 << 20).await.err().unwrap();
        assert!(format!("{e:#}").contains("start it with --commit-guard external"), "{e:#}");
        let other = Store::memory().unwrap();
        Pool::open(other.clone(), 1 << 20).await.unwrap();
        let e = Pool::open_as(other, 1 << 20, Clock::System, CommitGuard::External).await.err().unwrap();
        assert!(format!("{e:#}").contains("without --commit-guard external"), "{e:#}");
    }

    // -----------------------------------------------------------------------------------------
    // Checkpoint segments and cadence

    /// A file at the root. Its id and name come from `i`, so that checkpoints come out the same
    /// on every run.
    fn file(i: usize) -> Txn {
        let oid: ObjectId = format!("o-{i:026}").parse().unwrap();
        let mut set = SetChange::new(oid.clone());
        set.content = Some(ContentDescriptor::Inline { extents: vec![Extent::Shard { s: ShardHash::of(&i.to_be_bytes()), n: 1 }] });
        let create = Change::Create(CreateChange { oid: oid.clone(), parent: ObjectId::root(), name: format!("f{i:06}"), kind: Kind::File });
        Txn { target: oid, op: Op::Put, actor: Actor::system(), changes: vec![create, Change::Set(set)] }
    }

    fn removal(i: usize) -> Txn {
        let oid: ObjectId = format!("o-{i:026}").parse().unwrap();
        Txn { target: oid.clone(), op: Op::Delete, actor: Actor::system(), changes: vec![Change::Remove(RemoveChange { oid, recursive: false })] }
    }

    fn commit(seq: u64, txns: Vec<Txn>) -> Commit {
        Commit { format: 1, seq, time: "2026-09-28T00:00:00Z".parse().unwrap(), authority: "test".into(), txns }
    }

    /// A drive `big` of `n` files, the even-numbered ones, checkpointed at seq 1, and a server
    /// that has just started on it, so has checked none of the checkpoint's pages itself.
    async fn big_drive(n: usize, clock: Clock) -> (Arc<MemStore>, Arc<Pool>, Arc<Drive>, DriveState) {
        let mem = Arc::new(MemStore::new(clock.clone()));
        let store = Store::Mem(mem.clone());
        let state = DriveState::empty().apply(&commit(1, (0..n).map(|i| file(2 * i)).collect())).unwrap();
        {
            let pool = Pool::open_with(store.clone(), 64 << 20, clock.clone()).await.unwrap();
            let d = pool.create_drive("big", None).await.unwrap();
            pool.write_checkpoint(&d.id, &state, None).await.unwrap();
        }
        let pool = Pool::open_with(store, 64 << 20, clock).await.unwrap();
        let d = pool.drive("big").unwrap();
        (mem, pool, d, state)
    }

    /// Counts the requests to `mem` that `which` picks, from now on.
    fn count(mem: &MemStore, which: impl Fn(MemOp, &str) -> bool + Send + Sync + 'static) -> Arc<AtomicUsize> {
        let n = Arc::new(AtomicUsize::new(0));
        let counter = n.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            if which(op, path) {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            futures::future::ready(Fault::None).boxed()
        })));
        n
    }

    fn page_puts(op: MemOp, path: &str) -> bool {
        op == MemOp::Put && path.starts_with("pages/")
    }

    async fn tables(pool: &Pool, c: &Checkpointed) -> CheckpointTables {
        let b = pool.store.get(&c.index).await.unwrap().unwrap();
        serde_json::from_slice::<CheckpointIndex>(&b).unwrap().tables
    }

    /// How many pages each table of `b` lists that the same table of `a` does not.
    fn new_pages(a: &CheckpointTables, b: &CheckpointTables) -> [usize; 4] {
        let new = |x: &[SegmentRef], y: &[SegmentRef]| y.iter().filter(|s| x.iter().all(|o| o.page != s.page)).count();
        [new(&a.entries, &b.entries), new(&a.objects, &b.objects), new(&a.history, &b.history), new(&a.removed, &b.removed)]
    }

    async fn checkpoints(store: &Store, d: &Drive) -> Vec<u64> {
        let names = store.list_files(&format!("drives/{}/checkpoints/", d.id), None).await.unwrap();
        names.iter().filter_map(|n| n.strip_suffix(".json")?.parse().ok()).collect()
    }

    #[test]
    fn segments_end_where_the_keys_say_within_the_bounds() {
        let keys: Vec<String> = (0..200_000).map(|i| format!("o-{i:026}")).collect();
        let zeros: Vec<u32> = keys.iter().map(|k| key_zeros(k)).collect();
        let ends = segment_ends(&keys, String::clone);
        assert_eq!(ends.last(), Some(&keys.len()));
        let (mut start, mut by_key, mut fallback) = (0, 0, 0);
        for &end in &ends[..ends.len() - 1] {
            let n = end - start;
            assert!((SEGMENT_MIN_ROWS..=SEGMENT_MAX_ROWS).contains(&n), "a segment of {n} rows");
            // The rows that could end it: from the minimum to the maximum.
            let window = &zeros[start + SEGMENT_MIN_ROWS - 1..(start + SEGMENT_MAX_ROWS).min(keys.len())];
            match window.iter().position(|&z| z >= SEGMENT_CUT_BITS) {
                Some(p) => {
                    assert_eq!(end, start + SEGMENT_MIN_ROWS + p, "the first cut by key");
                    by_key += 1;
                }
                None => {
                    let last = window.iter().rposition(|&z| z >= SEGMENT_FALLBACK_BITS);
                    assert_eq!(end, start + last.map_or(SEGMENT_MAX_ROWS, |p| SEGMENT_MIN_ROWS + p), "the last fallback");
                    fallback += 1;
                }
            }
            start = end;
        }
        assert!(by_key > 0 && fallback > 0, "{by_key} segments cut by key, {fallback} at a fallback");
        // Keys that never cut end every segment at the maximum.
        let flat: Vec<String> = keys.iter().filter(|k| key_zeros(k) < SEGMENT_FALLBACK_BITS).take(20_000).cloned().collect();
        assert_eq!(segment_ends(&flat, String::clone), [SEGMENT_MAX_ROWS, 2 * SEGMENT_MAX_ROWS, 20_000]);
        assert_eq!(segment_ends(&keys[..SEGMENT_MIN_ROWS], String::clone), [SEGMENT_MIN_ROWS]);
        assert!(segment_ends(&keys[..0], String::clone).is_empty());
    }

    /// A row inserted or removed rewrites one or two segments of each table it touches, and a
    /// server that has just started stores only those: the previous checkpoint vouches for the
    /// rest (format §8.3, §12.4 option 1).
    #[tokio::test]
    async fn a_checkpoint_stores_only_the_segments_that_changed() {
        let (mem, pool, d, mut state) = big_drive(30_000, Clock::System).await;
        assert_eq!(d.snapshot().rows(), state.rows(), "a checkpoint loads as the state it was written from");
        let mut cadence = d.commit_lock.lock().await;
        let mut prev = tables(&pool, cadence.last.as_ref().unwrap()).await;
        for t in [&prev.entries, &prev.objects, &prev.history] {
            assert!(t.len() >= 4, "only {} segments", t.len());
        }
        let puts = count(&mem, page_puts);
        // A file inserted in the middle of every table, then one removed.
        for (seq, txn, removed) in [(2, file(30_001), 0), (3, removal(14_000), 1)] {
            state = state.apply(&commit(seq, vec![txn])).unwrap();
            puts.store(0, Ordering::SeqCst);
            pool.renew(&mut cadence.last).await;
            let written = pool.write_checkpoint(&d.id, &state, cadence.last.as_ref()).await.unwrap();
            let next = tables(&pool, &written).await;
            let new = new_pages(&prev, &next);
            assert!(new[..3].iter().all(|n| (1..=2).contains(n)) && new[3] == removed, "seq {seq}: new pages per table {new:?}");
            assert_eq!(puts.load(Ordering::SeqCst), new.iter().sum::<usize>(), "seq {seq}: only new pages are stored");
            cadence.last = Some(written);
            prev = next;
        }
        drop(cadence);
        mem.set_hook(None);
        let again = Pool::open(pool.store.clone(), 64 << 20).await.unwrap();
        let d = again.drive("big").unwrap();
        assert_eq!(d.snapshot().rows(), state.rows(), "with every table in use");
        // Without the previous checkpoint, a server that has just started stores every page.
        let puts = count(&mem, page_puts);
        let all = again.write_checkpoint(&d.id, &state, None).await.unwrap();
        assert_eq!(puts.load(Ordering::SeqCst), all.pages.len());
    }

    /// A checkpoint comes after 16 MiB of log when that comes before 1,000 commits, and the next
    /// is due 1,000 commits after it, counted across a restart (format §8.4).
    #[tokio::test]
    async fn checkpoints_follow_the_log_since_the_last_one() {
        let store = Store::memory().unwrap();
        let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d = pool.create_drive("log", None).await.unwrap();
        // Commits of about 80 KB: a file of 1,000 extents, all of one shard.
        let e = voidfs_core::content::from_bytes(&Bytes::from_static(b"x"), pool.params);
        pool.write_shards(&e.new_shards).await.unwrap();
        let big = pool.describe(vec![e.extents[0]; 1000]).await.unwrap();
        let (mut logged, mut last) = (0, 0);
        while checkpoints(&store, &d).await.is_empty() {
            let (v, _) = pool
                .commit(&d, |s| Ok(ops::put(s, "big", big.clone(), Attrs::default(), Op::Put, &Precondition::default(), &Actor::system())?))
                .await
                .unwrap();
            last = store.get(&log_path(&d.id, v.seq)).await.unwrap().unwrap().len() as u64;
            logged += last;
            assert!(v.seq < CHECKPOINT_EVERY, "no checkpoint after {logged} bytes of log");
        }
        assert!(logged >= CHECKPOINT_LOG_BYTES && logged - last < CHECKPOINT_LOG_BYTES, "a checkpoint after {logged} bytes of log");
        let first = d.snapshot().seq();
        assert_eq!(checkpoints(&store, &d).await, [first]);
        for _ in 0..600 {
            put(&pool, &d, "small", b"tick").await;
        }
        drop((pool, d));
        let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d = pool.drive("log").unwrap();
        while d.snapshot().seq() < first + CHECKPOINT_EVERY - 1 {
            put(&pool, &d, "small", b"tick").await;
        }
        assert_eq!(checkpoints(&store, &d).await, [first], "counted from the last checkpoint, not from seq 0");
        put(&pool, &d, "small", b"tick").await;
        assert_eq!(checkpoints(&store, &d).await, [first, first + CHECKPOINT_EVERY]);
    }

    /// A fork's first checkpoint lists its source's pages wherever their rows are the same, and
    /// stores none of those (format §9).
    #[tokio::test]
    async fn a_fork_shares_its_sources_segments() {
        let (mem, pool, src, _) = big_drive(20_000, Clock::System).await;
        let puts = count(&mem, page_puts);
        let a = pool.create_drive("fork-a", Some(&src)).await.unwrap();
        assert_eq!(puts.load(Ordering::SeqCst), 0, "the source has a checkpoint at the fork point");
        let source = tables(&pool, src.commit_lock.lock().await.last.as_ref().unwrap()).await;
        let forked = tables(&pool, a.commit_lock.lock().await.last.as_ref().unwrap()).await;
        assert_eq!(new_pages(&source, &forked), [0; 4]);
        // After a commit to the source, a fork stores only the segments that commit changed.
        put(&pool, &src, "f010001", b"new").await;
        puts.store(0, Ordering::SeqCst);
        let b = pool.create_drive("fork-b", Some(&src)).await.unwrap();
        let forked = tables(&pool, b.commit_lock.lock().await.last.as_ref().unwrap()).await;
        let new = new_pages(&source, &forked);
        assert!(new[..3].iter().all(|n| (1..=2).contains(n)) && new[3] == 0, "new pages per table {new:?}");
        assert_eq!(puts.load(Ordering::SeqCst), new.iter().sum::<usize>());
    }

    /// The previous checkpoint vouches for its pages until 12 hours after its index was seen: a
    /// server reads the index again once that was 6 hours ago, and gives up on a checkpoint that
    /// would be written after the 12 (format §12.4).
    #[tokio::test]
    async fn an_old_previous_checkpoint_is_read_again() {
        let clock = Clock::manual("2026-09-28T00:00:00Z".parse().unwrap());
        let (mem, pool, d, state) = big_drive(2_000, clock.clone()).await;
        let mut cadence = d.commit_lock.lock().await;
        let index = cadence.last.as_ref().unwrap().index.clone();
        let reads = {
            let index = index.clone();
            count(&mem, move |op, path| op == MemOp::Get && path == index)
        };
        clock.advance(REUSE_WITHOUT_REREAD - Duration::from_secs(1));
        pool.renew(&mut cadence.last).await;
        assert!(cadence.last.is_some());
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        clock.advance(2 * HOUR);
        pool.renew(&mut cadence.last).await;
        assert_eq!((reads.load(Ordering::SeqCst), cadence.last.as_ref().unwrap().seen), (1, clock.mono()));
        clock.advance(13 * HOUR);
        let stale = cadence.last.take().unwrap();
        assert!(pool.write_checkpoint(&d.id, &state, Some(&stale)).await.is_err(), "written too long after the read it relies on");
        // An index that has gone vouches for nothing.
        pool.store.delete(&index).await.unwrap();
        let mut gone = Some(stale);
        pool.renew(&mut gone).await;
        assert!(gone.is_none());
    }
}
