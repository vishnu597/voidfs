// SPDX-License-Identifier: Apache-2.0
//! A pool (format §2–§3) and its drives: loading them from the bucket, committing to their
//! logs, checkpoints, forks, deletion, and the change feed (protocol §5.6).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

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
use crate::metrics::PoolMetrics;
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
/// Transactions in one commit, at most (format §7.1)...
const BATCH_TXNS: usize = 256;
/// ...and their size as encoded, unless the first alone is larger, so that log entries stay
/// quick to replay.
const BATCH_BYTES: usize = 1 << 20;
/// How many times a batch is planned again after another authority wrote the sequence number it
/// was for (format §7.2), before its transactions are handed back to be retried.
const BATCH_ATTEMPTS: u32 = 8;
/// The longest a transaction waits for the log. What it references was checked for garbage
/// collection before it was queued, and must be committed within 12 hours of that check (format
/// §12.4), of which a request body may take 6.
const QUEUE_MAX: Duration = Duration::from_secs(3600);

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

/// Adds the feed changes of `txn`, version `v`, given the drive's state just before and just
/// after it.
fn feed_of(txn: &Txn, v: VersionId, before: &DriveState, after: &DriveState, changes: &mut Vec<FeedChange>) {
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

/// Applies `commit` a transaction at a time, so that each one's feed changes name keys as they
/// were just before and after it, not after the whole commit.
fn apply_logged(state: &DriveState, commit: &Commit) -> Result<(DriveState, FeedBatch), voidfs_core::state::StateError> {
    let mut changes = Vec::new();
    let mut next: Option<DriveState> = None;
    for (i, txn) in commit.txns.iter().enumerate() {
        let before = next.as_ref().unwrap_or(state);
        let after = before.apply_txn_of(commit.seq, i as u32, commit.time, txn)?;
        feed_of(txn, VersionId::new(commit.seq, i as u32), before, &after, &mut changes);
        next = Some(after);
    }
    let next = match next {
        Some(n) => n,
        None => state.apply(commit)?,
    };
    Ok((next, FeedBatch { seq: commit.seq, time: commit.time, changes }))
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
/// §8.4).
#[derive(Default)]
struct Cadence {
    /// Commits since the last checkpoint was started.
    commits: u64,
    /// Their size as stored.
    bytes: u64,
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

/// A transaction's plan: it checks the transaction's preconditions against the state it is given
/// and returns the transaction.
type Plan = Box<dyn FnMut(&DriveState) -> Result<Txn, CommitError> + Send>;

/// What a commit answers: the transaction's version, and the drive's state just after it.
pub type Committed = Result<(VersionId, Arc<DriveState>), CommitError>;

/// A mutation waiting for its drive's log.
struct Waiting {
    plan: Plan,
    reply: tokio::sync::oneshot::Sender<Committed>,
    /// When it was queued, on the monotonic clock.
    since: Duration,
}

pub struct Drive {
    pub id: DriveId,
    pub alias: String,
    pub desc: DriveDescriptor,
    state: RwLock<Arc<DriveState>>,
    /// Held while a batch is committed, and by anything that must see every acknowledged write.
    commit_lock: tokio::sync::Mutex<Cadence>,
    /// The drive's last checkpoint, whose pages the next one lists without storing them again.
    /// Held while a checkpoint is written, so that there is one at a time. Whatever takes both
    /// locks takes the commit lock first.
    checkpoint: Arc<tokio::sync::Mutex<Option<Checkpointed>>>,
    /// Mutations waiting for the next commit, in the order they arrived.
    queue: std::sync::Mutex<VecDeque<Waiting>>,
    /// Whether a task is draining `queue`: there is at most one. Set and cleared only while
    /// `queue` is locked.
    draining: std::sync::atomic::AtomicBool,
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
    /// Not committed, and worth trying again: what the plan was based on has changed, another
    /// authority kept winning the log, or the log was too busy for too long.
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
    fn new(desc: DriveDescriptor, state: Arc<DriveState>, cadence: Cadence, last: Option<Checkpointed>) -> Drive {
        let (notify, _) = tokio::sync::watch::channel(state.seq());
        let floor = state.seq();
        Drive {
            id: desc.drive_id.clone(),
            alias: desc.alias.clone(),
            desc,
            state: RwLock::new(state),
            commit_lock: tokio::sync::Mutex::new(cadence),
            checkpoint: Arc::new(tokio::sync::Mutex::new(last)),
            queue: std::sync::Mutex::new(VecDeque::new()),
            draining: std::sync::atomic::AtomicBool::new(false),
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

    fn install(&self, state: Arc<DriveState>, batch: FeedBatch) {
        let seq = state.seq();
        *self.state.write().unwrap() = state;
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
    pub metrics: PoolMetrics,
}

/// What a cache keeps of `b`: a copy holding only these bytes. A slice keeps its whole buffer
/// alive, and a shard cut by the chunker is a slice of a buffer of up to about 32 MiB.
fn cached(b: &Bytes) -> Bytes {
    Bytes::copy_from_slice(b)
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
        let page_bytes = cache_bytes / 8 + 1;
        let metrics = PoolMetrics::new(cache_bytes, page_bytes);
        // Least recently used first: shards and pages never change, so the ones used last are the
        // ones to keep. moka's default (TinyLFU) admits an entry only if it was used more often
        // than everything it would evict, so a full cache turned every new shard away.
        let cache = |bytes, evictions: prometheus::IntCounter| {
            moka::sync::Cache::builder()
                .weigher(|_: &ShardHash, v: &Bytes| v.len().try_into().unwrap_or(u32::MAX))
                .max_capacity(bytes)
                .eviction_policy(moka::policy::EvictionPolicy::lru())
                .eviction_listener(move |_, _, cause| {
                    if cause == moka::notification::RemovalCause::Size {
                        evictions.inc();
                    }
                })
                .build()
        };
        let pool = Arc::new(Pool {
            store,
            desc,
            params,
            clock,
            guard: Guard::new(REUSE_CAPACITY),
            shards: cache(cache_bytes, metrics.shards.evictions.clone()),
            pages: cache(page_bytes, metrics.pages.evictions.clone()),
            drives: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            registry: std::sync::Mutex::new(()),
            deleted: RwLock::new(HashMap::new()),
            authority: format!("a-{}", uuid::Uuid::new_v4()),
            metrics,
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
        let (mut state, last) = match self.latest_checkpoint(id).await? {
            Some((s, c)) => (s, Some(c)),
            None if desc.fork_of.is_some() => bail!("fork {id} has no checkpoint"),
            None => (DriveState::empty(), None),
        };
        let from = state.seq();
        let drive_feed = self.replay(id, &mut state, &mut cadence).await?;
        let d = Drive::new(desc, Arc::new(state), cadence, last);
        // The feed holds every commit replayed, so it can report changes after where the replay
        // started.
        *d.feed_floor.write().unwrap() = from;
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
            let (next, batch) = apply_logged(state, &commit).with_context(|| format!("applying {name}"))?;
            out.push(batch);
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

    /// Writes a checkpoint of `state` (format §8) and returns it. `state` was the drive's own at
    /// `seen`, which vouches for the content its rows reference (format §12.4, option 1). Pages
    /// that `reuse` lists are not stored again; the others go through the garbage-collection
    /// guard like any page.
    async fn write_checkpoint(&self, id: &DriveId, state: Arc<DriveState>, seen: Duration, reuse: Option<&Checkpointed>) -> anyhow::Result<Checkpointed> {
        let seq = state.seq();
        let time = state.time();
        // Copying the rows out, encoding and hashing them takes hundreds of milliseconds of CPU
        // for a drive of 600,000 rows: not on an async worker.
        let (tables, pages, stats) = tokio::task::spawn_blocking(move || {
            let rows = state.rows();
            let mut pages = Vec::new();
            let tables = CheckpointTables {
                entries: segments("entries", &rows.entries, |e| format!("{}/{}{}", e.parent, e.name, if e.kind == Kind::Folder { "/" } else { "" }), &mut pages),
                objects: segments("objects", &rows.objects, |o| o.oid.to_string(), &mut pages),
                history: segments("history", &rows.history, |h| format!("{}@{:020}.{}", h.oid, h.version.seq, h.version.idx), &mut pages),
                removed: segments("removed", &rows.removed, |r| format!("{}@{}", r.key, r.oid), &mut pages),
            };
            (tables, pages, serde_json::json!({ "objects": rows.objects.len(), "bytes": state.live_bytes() }))
        })
        .await?;
        let listed = tables.pages();
        let (reused, new): (Vec<Page>, Vec<Page>) = pages.into_iter().partition(|p| reuse.is_some_and(|r| r.pages.contains(&p.hash)));
        self.write_pages(&new).await?;
        let now = self.clock.mono();
        if now.saturating_sub(seen) > guard::COMMIT_WITHIN {
            bail!("the state was the drive's too long ago to vouch for the content it references");
        }
        if let Some(r) = reuse
            && !reused.is_empty()
            && now.saturating_sub(r.seen) > guard::COMMIT_WITHIN
        {
            bail!("the previous checkpoint was seen too long ago to vouch for its pages");
        }
        let idx = CheckpointIndex { format: 1, seq, time, tables, stats };
        let name = format!("drives/{id}/checkpoints/{seq:020}.json");
        self.store.put(&name, Bytes::from(serde_json::to_vec(&idx)?)).await?;
        let seen = self.clock.mono();
        self.store.put(&format!("drives/{id}/_last_checkpoint"), Bytes::from(format!("{{\"seq\":{seq}}}"))).await?;
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

    /// Checkpoints `state`, which was the drive's newest at `seen`, after `last`, and makes it
    /// the last. A failure is logged, and the next attempt waits a whole interval.
    async fn checkpoint(&self, id: &DriveId, state: Arc<DriveState>, seen: Duration, last: &mut Option<Checkpointed>) {
        let started = Instant::now();
        let seq = state.seq();
        self.renew(last).await;
        let written = self.write_checkpoint(id, state, seen, last.as_ref()).await;
        self.metrics.checkpoint_write.observe(started.elapsed().as_secs_f64());
        match written {
            Ok(c) => {
                self.metrics.checkpoints_written.inc();
                *last = Some(c);
            }
            Err(e) => {
                self.metrics.checkpoints_failed.inc();
                tracing::warn!("checkpoint of {id} at {seq} failed: {e:#}");
            }
        }
    }

    /// Waits for the checkpoints being written in the background.
    pub async fn finish_checkpoints(&self) {
        let drives: Vec<Arc<Drive>> = self.drives.read().unwrap().values().cloned().collect();
        for d in drives {
            drop(d.checkpoint.lock().await);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Shards and pages

    pub async fn shard(&self, h: &ShardHash) -> anyhow::Result<Bytes> {
        if let Some(b) = self.shards.get(h) {
            self.metrics.shards.hits.inc();
            return Ok(b);
        }
        self.metrics.shards.misses.inc();
        let generation = self.guard.generation();
        let b = self.store.get(&guard::Kind::Shard.path(h)).await?.ok_or_else(|| anyhow!("shard {h} is missing"))?;
        if ShardHash::of(&b) != *h {
            bail!("shard {h} is corrupt");
        }
        self.guard.confirmed(*h, generation);
        self.shards.insert(*h, cached(&b));
        Ok(b)
    }

    pub async fn page(&self, h: &ShardHash) -> anyhow::Result<Bytes> {
        if let Some(b) = self.pages.get(h) {
            self.metrics.pages.hits.inc();
            return Ok(b);
        }
        self.metrics.pages.misses.inc();
        let generation = self.guard.generation();
        let b = self.store.get(&guard::Kind::Page.path(h)).await?.ok_or_else(|| anyhow!("page {h} is missing"))?;
        self.guard.confirmed(*h, generation);
        self.pages.insert(*h, cached(&b));
        Ok(b)
    }

    /// Stores shards so that a commit may reference them (format §7.4, §12.4). Shards already
    /// checked are not uploaded again; holding a shard's bytes in the cache is not a check.
    pub async fn write_shards(&self, shards: &[Shard]) -> anyhow::Result<()> {
        let items: Vec<(ShardHash, Bytes)> = shards.iter().map(|s| (s.hash, s.bytes.clone())).collect();
        self.guard.admit(&self.store, &self.clock, guard::Kind::Shard, &items).await?;
        for s in shards {
            self.shards.insert(s.hash, cached(&s.bytes));
        }
        Ok(())
    }

    /// [`Pool::write_shards`] for manifest pages and checkpoint segments.
    pub async fn write_pages(&self, pages: &[Page]) -> anyhow::Result<()> {
        let items: Vec<(ShardHash, Bytes)> = pages.iter().map(|p| (p.hash, p.bytes.clone())).collect();
        self.guard.admit(&self.store, &self.clock, guard::Kind::Page, &items).await?;
        for p in pages {
            self.pages.insert(p.hash, cached(&p.bytes));
        }
        Ok(())
    }

    /// Drops a deleted shard or page from the caches.
    pub fn forget(&self, h: &ShardHash) {
        self.shards.invalidate(h);
        self.pages.invalidate(h);
    }

    /// The pool's metrics, with what they report as of now: the caches' sizes, the drives, and
    /// the phase of garbage collection as last read.
    pub fn gather_metrics(&self) -> Vec<prometheus::proto::MetricFamily> {
        let m = &self.metrics;
        for (cache, series) in [(&self.shards, &m.shards), (&self.pages, &m.pages)] {
            // Counts are kept up to date lazily; this settles them first.
            cache.run_pending_tasks();
            series.bytes.set(i64::try_from(cache.weighted_size()).unwrap_or(i64::MAX));
            series.entries.set(i64::try_from(cache.entry_count()).unwrap_or(i64::MAX));
        }
        m.drives_live.set(self.aliases.read().unwrap().len() as i64);
        m.drives_deleted.set(self.deleted.read().unwrap().len() as i64);
        let phase = self.guard.phase().map(|p| match p {
            Some(crate::gc::Phase::Marking) => 1,
            Some(crate::gc::Phase::Waiting) => 2,
            Some(crate::gc::Phase::Deleting) => 3,
            None => 0,
        });
        for (i, g) in m.gc_phase.iter().enumerate() {
            g.set(i64::from(phase == Some(i)));
        }
        m.gather()
    }

    /// The full extent list of a content descriptor.
    pub async fn extents(&self, desc: &ContentDescriptor) -> anyhow::Result<Vec<Extent>> {
        // Fetch the tree's pages, then flatten from what was fetched: the cache may already have
        // evicted some of them.
        let mut pages = HashMap::new();
        if let ContentDescriptor::Tree { root, .. } = desc {
            let mut pending = vec![*root];
            while let Some(h) = pending.pop() {
                let bytes = self.page(&h).await?;
                if let Ok(voidfs_core::model::ManifestPage::Node { children }) = serde_json::from_slice(&bytes) {
                    pending.extend(children.iter().map(|c| c.page));
                }
                pages.insert(h, bytes);
            }
        }
        Ok(manifest::flatten(desc, &mut |h| pages.get(h).cloned())?)
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
        let mut last = None;
        let (state, fork_of, deadline) = match source {
            None => (Arc::new(DriveState::empty()), None, None),
            Some(src) => {
                // Hold the source's commit lock so the fork includes every acknowledged write,
                // and its checkpoint lock so that its last checkpoint is not still being written.
                let _committing = src.commit_lock.lock().await;
                let mut src_last = src.checkpoint.lock().await;
                // The fork references content through the source's state, so the source must
                // still be a drive (format §12.4): a hard delete forgets it before deleting it.
                if self.drive_by_id(&src.id).is_none() {
                    return Err(CreateError::NotFound);
                }
                let seen = self.clock.mono();
                let s = Arc::new(src.snapshot().fork());
                // The fork's first checkpoint shares every segment that is unchanged since the
                // source's last one (format §9). That one's index vouches for them until 12 hours
                // after it was seen, and the source's state for the content it references until
                // 12 hours after `seen` (§12.4), so the fork must exist by then.
                self.renew(&mut src_last).await;
                let vouched = src_last.as_ref().map_or(seen, |r| r.seen.min(seen));
                last = Some(self.write_checkpoint(&id, s.clone(), seen, src_last.as_ref()).await?);
                let seq = s.seq();
                (s, Some(ForkOf { drive_id: src.id.clone(), seq }), Some(vouched + guard::COMMIT_WITHIN))
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
        if let Some(c) = &mut last {
            c.seen = self.clock.mono();
        }
        if self.alias_taken(alias) {
            let _ = self.store.delete_prefix(&format!("drives/{id}/")).await;
            return Err(CreateError::Exists);
        }
        if let Some(src) = source {
            src.forks.write().unwrap().push(id.clone());
        }
        Ok(self.register(Drive::new(desc, state, Cadence::default(), last), false))
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
    /// and commits, forks and a checkpoint already running finish before `drive.json` goes, which
    /// is when the drive stops being a garbage-collection root. If the deletion fails part way,
    /// the server no longer serves the drive, and what is left in the bucket is loaded again on
    /// restart only if `drive.json` survived.
    pub async fn hard_delete_id(&self, id: &DriveId) -> anyhow::Result<()> {
        let d = self.drive_by_id(id);
        self.forget_drive(id);
        let _g = match &d {
            Some(d) => {
                let committing = d.commit_lock.lock().await;
                Some((committing, d.checkpoint.lock().await))
            }
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

    /// Commits a transaction to the drive's log (format §7), in one log entry with whatever else
    /// is waiting: the transactions that queue up while an entry is being written go into the next
    /// one together.
    ///
    /// `plan` checks the transaction's preconditions against the state it is given, the drive's
    /// state plus the transactions before it in the same commit, and returns the transaction. It
    /// may run more than once, and only its last run counts: again after another authority wrote
    /// the sequence number the commit was for (format §7.2), and in the next commit if this one is
    /// full. Shards and pages the transaction references must already be stored.
    ///
    /// Answers the transaction's version and the drive's state just after it, which has the
    /// commit's earlier transactions in it and not its later ones.
    pub async fn commit(self: &Arc<Self>, d: &Arc<Drive>, plan: impl FnMut(&DriveState) -> Result<Txn, CommitError> + Send + 'static) -> Committed {
        self.commit_all(d, [plan]).await.pop().expect("one answer per plan")
    }

    /// [`Pool::commit`] for several transactions, queued together and in order, so that each is
    /// planned after the ones before it and they share log entries. Answers in the same order.
    pub async fn commit_all<P>(self: &Arc<Self>, d: &Arc<Drive>, plans: impl IntoIterator<Item = P>) -> Vec<Committed>
    where
        P: FnMut(&DriveState) -> Result<Txn, CommitError> + Send + 'static,
    {
        let plans: Vec<P> = plans.into_iter().collect();
        let since = self.clock.mono();
        let (answers, start): (Vec<_>, bool) = {
            let mut q = d.queue.lock().unwrap();
            let answers = plans
                .into_iter()
                .map(|plan| {
                    let (reply, answer) = tokio::sync::oneshot::channel();
                    q.push_back(Waiting { plan: Box::new(plan), reply, since });
                    answer
                })
                .collect();
            (answers, !d.draining.swap(true, std::sync::atomic::Ordering::Relaxed))
        };
        // A task of its own, so that a request that goes away cannot stop a commit that others
        // are waiting on; and only one, so that what else waits for the commit lock waits for one
        // batch, not for a task per mutation queued ahead of it.
        if start {
            tokio::spawn(self.clone().drain(d.clone()));
        }
        let mut out = Vec::with_capacity(answers.len());
        for answer in answers {
            out.push(answer.await.unwrap_or_else(|_| Err(CommitError::Other(anyhow!("the commit was abandoned")))));
        }
        out
    }

    /// Commits what is waiting on `d`, a batch at a time, until nothing is. A mutation starts one
    /// of these unless one is running, and it takes everything waiting each time it holds the
    /// commit lock, so a batch is what queued up while the one before it was being written.
    async fn drain(self: Arc<Self>, d: Arc<Drive>) {
        use std::sync::atomic::Ordering;
        /// Lets the next mutation start another task if this one panics.
        struct Unwinding<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Unwinding<'_> {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    self.0.store(false, Ordering::Relaxed);
                }
            }
        }
        let _unwinding = Unwinding(&d.draining);
        loop {
            // Taken for each batch, so that forks and deletions of the drive get their turn.
            let mut cadence = d.commit_lock.lock().await;
            let batch: Vec<Waiting> = {
                let mut q = d.queue.lock().unwrap();
                if q.is_empty() {
                    // Under the queue's lock, so that a mutation queued after this starts a task.
                    d.draining.store(false, Ordering::Relaxed);
                    return;
                }
                let n = q.len().min(BATCH_TXNS);
                q.drain(..n).collect()
            };
            let rest = self.commit_batch(&d, &mut cadence, batch).await;
            let mut q = d.queue.lock().unwrap();
            for w in rest.into_iter().rev() {
                q.push_front(w);
            }
        }
    }

    /// Plans `batch` in order, each transaction against the state the ones before it leave,
    /// writes those that plan as one commit, and answers each mutation. Returns the mutations that
    /// did not fit, to wait for the next commit.
    ///
    /// A mutation is answered only once the commit is written, unless its plan failed against
    /// the drive's state as installed, before any transaction of the batch: a failure may rest
    /// on an earlier transaction that is never written.
    async fn commit_batch(self: &Arc<Self>, d: &Drive, cadence: &mut Cadence, batch: Vec<Waiting>) -> Vec<Waiting> {
        let now = self.clock.mono();
        let (mut batch, late): (Vec<Waiting>, Vec<Waiting>) = batch.into_iter().partition(|w| now.saturating_sub(w.since) <= QUEUE_MAX);
        for w in late {
            let _ = w.reply.send(Err(CommitError::Retry));
        }
        let mut attempts = 0;
        loop {
            attempts += 1;
            let cur = d.snapshot();
            let now = self.clock.now();
            let (seq, time) = (cur.seq() + 1, cur.time().map_or(now, |t| t.max(now)));
            let mut state = cur.clone();
            let (mut txns, mut bytes, mut changes) = (Vec::new(), 0, Vec::new());
            let mut held: Vec<(Waiting, Committed)> = Vec::new();
            let mut rest = Vec::new();
            let mut waiting = batch.into_iter();
            while let Some(mut w) = waiting.next() {
                let planned = (w.plan)(&state).and_then(|txn| {
                    let len = serde_json::to_vec(&txn).map_err(anyhow::Error::from)?.len();
                    Ok((txn, len))
                });
                let (txn, len) = match planned {
                    Ok(t) => t,
                    Err(e) if txns.is_empty() => {
                        let _ = w.reply.send(Err(e));
                        continue;
                    }
                    Err(e) => {
                        held.push((w, Err(e)));
                        continue;
                    }
                };
                if !txns.is_empty() && bytes + len > BATCH_BYTES {
                    rest.push(w);
                    rest.extend(waiting);
                    break;
                }
                let v = VersionId::new(seq, txns.len() as u32);
                match state.apply_txn_of(seq, v.idx, time, &txn) {
                    Ok(next) => {
                        let next = Arc::new(next);
                        feed_of(&txn, v, &state, &next, &mut changes);
                        held.push((w, Ok((v, next.clone()))));
                        state = next;
                        bytes += len;
                        txns.push(txn);
                    }
                    Err(e) => {
                        let e = CommitError::Other(anyhow!("planned transaction does not apply: {e}"));
                        if txns.is_empty() {
                            let _ = w.reply.send(Err(e));
                        } else {
                            held.push((w, Err(e)));
                        }
                    }
                }
            }
            if txns.is_empty() {
                return rest;
            }
            let n = txns.len();
            let commit = Commit { format: 1, seq, time, authority: self.authority.clone(), txns };
            let written = match serde_json::to_vec(&commit) {
                Ok(bytes) => {
                    let len = bytes.len();
                    let started = Instant::now();
                    let created = self.create(&log_path(&d.id, seq), Bytes::from(bytes)).await;
                    self.metrics.log_write.observe(started.elapsed().as_secs_f64());
                    created.map(|created| created.then_some(len))
                }
                Err(e) => Err(e.into()),
            };
            match written {
                Ok(Some(len)) => {
                    self.metrics.commits_written.inc();
                    self.metrics.batch.observe(n as f64);
                    d.install(state.clone(), FeedBatch { seq, time, changes });
                    cadence.add(len);
                    // Nothing ties a checkpoint to the commit that made it due (format §8), so it
                    // is written in the background, from the state just installed. If the last
                    // one is still being written, the next commit tries again.
                    if cadence.due()
                        && let Ok(mut last) = d.checkpoint.clone().try_lock_owned()
                    {
                        *cadence = Cadence::default();
                        let (pool, id, seen) = (self.clone(), d.id.clone(), self.clock.mono());
                        tokio::spawn(async move { pool.checkpoint(&id, state, seen, &mut last).await });
                    }
                    for (w, answer) in held {
                        let _ = w.reply.send(answer);
                    }
                    return rest;
                }
                // Another authority wrote this sequence number: catch up, and plan everything
                // again against what it wrote (format §7.2).
                Ok(None) => {
                    self.metrics.commits_lost.inc();
                    let failed = match self.catch_up(d, &cur, cadence).await {
                        Err(e) => Some(format!("catching up with the log: {e:#}")),
                        Ok(()) if attempts >= BATCH_ATTEMPTS => None,
                        Ok(()) => {
                            batch = held.into_iter().map(|(w, _)| w).chain(rest).collect();
                            continue;
                        }
                    };
                    for (w, _) in held {
                        let _ = w.reply.send(Err(failed.as_ref().map_or(CommitError::Retry, |e| CommitError::Other(anyhow!("{e}")))));
                    }
                    return rest;
                }
                // Whether it was written is not known, so none of it may be reported as done.
                Err(e) => {
                    self.metrics.commits_failed.inc();
                    for (w, _) in held {
                        let _ = w.reply.send(Err(CommitError::Other(anyhow!("writing log entry {seq}: {e:#}"))));
                    }
                    return rest;
                }
            }
        }
    }

    /// Applies the commits another authority wrote after `cur`, the drive's state, and installs
    /// the result.
    async fn catch_up(&self, d: &Drive, cur: &DriveState, cadence: &mut Cadence) -> anyhow::Result<()> {
        let mut s = cur.clone();
        let batches = self.replay(&d.id, &mut s, cadence).await?;
        let mut last = None;
        for b in batches {
            last = Some(b.seq);
            d.push_feed(b);
        }
        *d.state.write().unwrap() = Arc::new(s);
        if let Some(seq) = last {
            let _ = d.notify.send(seq);
        }
        Ok(())
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
    use voidfs_core::ops::{self, AttrsPatch, OpError, Precondition};

    use super::*;
    use crate::store::{Fault, MemOp, MemStore, Store};

    const HOUR: Duration = Duration::from_secs(3600);

    async fn put(pool: &Arc<Pool>, d: &Arc<Drive>, key: &str, data: &'static [u8]) -> VersionId {
        let desc = content(pool, data).await;
        pool.commit(d, put_plan(key, &desc, Precondition::default(), &Arc::default())).await.unwrap().0
    }

    /// Stores `data` and describes it.
    async fn content(pool: &Pool, data: &'static [u8]) -> ContentDescriptor {
        let e = voidfs_core::content::from_bytes(&Bytes::from_static(data), pool.params);
        pool.write_shards(&e.new_shards).await.unwrap();
        pool.describe(e.extents).await.unwrap()
    }

    type TestPlan = Box<dyn FnMut(&DriveState) -> Result<Txn, CommitError> + Send>;

    /// Puts `desc` at `key` under `pre`, counting its runs in `runs`.
    fn put_plan(key: &str, desc: &ContentDescriptor, pre: Precondition, runs: &Arc<AtomicUsize>) -> TestPlan {
        let (key, desc, runs) = (key.to_owned(), desc.clone(), runs.clone());
        Box::new(move |s| {
            runs.fetch_add(1, Ordering::SeqCst);
            Ok(ops::put(s, &key, desc.clone(), Attrs::default(), Op::Put, &pre, &Actor::system())?)
        })
    }

    fn rename_plan(src: &str, dst: &str) -> TestPlan {
        let (src, dst) = (src.to_owned(), dst.to_owned());
        Box::new(move |s| Ok(ops::rename(s, &src, &dst, false, &AttrsPatch::default(), &Precondition::default(), &Actor::system())?))
    }

    fn delete_plan(key: &str) -> TestPlan {
        let key = key.to_owned();
        Box::new(move |s| ops::delete(s, &key, &Precondition::default(), &Actor::system())?.ok_or(CommitError::Op(OpError::NoSuchKey)))
    }

    fn absent() -> Precondition {
        Precondition { if_none_match_any: true, ..Default::default() }
    }

    /// Commits `plans` together: they queue up while the log is busy, then go into as few
    /// commits as the caps allow. Answers in their order.
    async fn batch(pool: &Arc<Pool>, d: &Arc<Drive>, plans: Vec<TestPlan>) -> Vec<Committed> {
        let busy = d.commit_lock.lock().await;
        let n = plans.len();
        let tasks: Vec<_> = plans
            .into_iter()
            .map(|p| {
                let (pool, d) = (pool.clone(), d.clone());
                tokio::spawn(async move { pool.commit(&d, p).await })
            })
            .collect();
        while d.queue.lock().unwrap().len() < n {
            tokio::task::yield_now().await;
        }
        drop(busy);
        futures::future::join_all(tasks).await.into_iter().map(Result::unwrap).collect()
    }

    /// Waits for the checkpoint `d` is writing in the background, if any.
    async fn settle(d: &Drive) {
        drop(d.checkpoint.lock().await);
    }

    fn version(a: &Committed) -> VersionId {
        a.as_ref().map(|(v, _)| *v).unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// The drive's log, as stored.
    async fn log(store: &Store, d: &Drive) -> Vec<Commit> {
        let dir = format!("drives/{}/log/", d.id);
        let mut out = Vec::new();
        for name in store.list_files(&dir, None).await.unwrap() {
            out.push(serde_json::from_slice(&store.get(&format!("{dir}{name}")).await.unwrap().unwrap()).unwrap());
        }
        out
    }

    async fn read(pool: &Pool, d: &Drive, key: &str) -> Option<Vec<u8>> {
        let s = d.snapshot();
        let r = s.record(&s.lookup(&Key::parse(key).unwrap())?)?.clone();
        let mut out = Vec::new();
        for e in pool.extents(r.content.as_ref().unwrap()).await.unwrap() {
            match e {
                Extent::Shard { s, .. } => out.extend_from_slice(&pool.shard(&s).await.unwrap()),
                Extent::Zero { z } => out.resize(out.len() + z as usize, 0),
                Extent::Data { d } => out.extend_from_slice(&d),
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
            settle(&a).await;
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

    /// `p2` has not seen `p1`'s commit, so its batch claims seq 1 and loses the race. Nothing it
    /// planned is reported: it catches up and plans the batch again against `p1`'s commit, where
    /// a precondition that held before no longer does (format §7.2).
    async fn lose_a_race(p1: &Arc<Pool>, p2: &Arc<Pool>) {
        let (d1, d2) = (p1.drive("shared").unwrap(), p2.drive("shared").unwrap());
        put(p1, &d1, "x", b"from one").await;
        let two = content(p2, b"from two").await;
        let runs = Arc::new(AtomicUsize::new(0));
        let answers = batch(p2, &d2, vec![
            put_plan("y", &two, absent(), &runs),
            put_plan("x", &two, absent(), &runs),
            put_plan("z", &two, Precondition::default(), &runs),
        ])
        .await;
        assert_eq!(runs.load(Ordering::SeqCst), 6, "every plan ran again after catching up");
        assert_eq!(version(&answers[0]), VersionId::new(2, 0));
        assert!(matches!(answers[1], Err(CommitError::Op(OpError::PreconditionFailed { current: Some(v) })) if v == VersionId::new(1, 0)));
        assert_eq!(version(&answers[2]), VersionId::new(2, 1));
        assert_eq!(d2.snapshot().seq(), 2);
        let log = log(&p2.store, &d2).await;
        assert_eq!(log.iter().map(|c| (c.seq, c.authority.as_str(), c.txns.len())).collect::<Vec<_>>(), [(1, p1.authority.as_str(), 1), (2, p2.authority.as_str(), 2)]);
        let fresh = Pool::open_as(p2.store.clone(), 1 << 20, Clock::System, p2.desc.commit_guard).await.unwrap();
        let d = fresh.drive("shared").unwrap();
        for (key, data) in [("x", &b"from one"[..]), ("y", b"from two"), ("z", b"from two")] {
            assert_eq!(read(&fresh, &d, key).await.as_deref(), Some(data), "{key}");
        }
    }

    #[tokio::test]
    async fn a_second_authority_cannot_fork_history() {
        let store = Store::memory().unwrap();
        let p1 = Pool::open(store.clone(), 1 << 20).await.unwrap();
        p1.create_drive("shared", None).await.unwrap();
        let p2 = Pool::open(store.clone(), 1 << 20).await.unwrap();
        lose_a_race(&p1, &p2).await;
    }

    /// A batch that keeps losing is handed back to be retried, and nothing it planned is in the
    /// log.
    #[tokio::test]
    async fn a_batch_that_keeps_losing_is_handed_back() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let store = Store::mem(mem.clone());
        let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let one = content(&pool, b"one").await;
        // Another authority writes each sequence number just before this one tries to.
        let rival = store.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            let won = (op == MemOp::PutNew && path.contains("/log/")).then(|| {
                let seq: u64 = path.rsplit('/').next().and_then(|n| n.strip_suffix(".json")).unwrap().parse().unwrap();
                (path.to_owned(), Bytes::from(serde_json::to_vec(&commit(seq, vec![file(seq as usize)])).unwrap()))
            });
            let rival = rival.clone();
            async move {
                if let Some((path, bytes)) = won {
                    rival.put(&path, bytes).await.unwrap();
                }
                Fault::None
            }
            .boxed()
        })));
        let runs = Arc::new(AtomicUsize::new(0));
        let answers = batch(&pool, &d, vec![put_plan("a", &one, Precondition::default(), &runs), put_plan("b", &one, Precondition::default(), &runs)]).await;
        mem.set_hook(None);
        assert!(answers.iter().all(|a| matches!(a, Err(CommitError::Retry))));
        assert_eq!(runs.load(Ordering::SeqCst), 2 * BATCH_ATTEMPTS as usize);
        assert_eq!(d.snapshot().seq(), BATCH_ATTEMPTS as u64, "caught up with each of the rival's commits");
        assert!(log(&store, &d).await.iter().all(|c| c.authority == "test"));
        assert_eq!(version(&pool.commit(&d, put_plan("a", &one, Precondition::default(), &runs)).await), VersionId::new(BATCH_ATTEMPTS as u64 + 1, 0));
    }

    /// Mutations that wait together go into one commit, each planned against the state the ones
    /// before it leave, and each answered with the drive's state just after it (format §7.1).
    #[tokio::test]
    async fn a_batch_plans_each_transaction_after_the_ones_before_it() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let (one, two) = (content(&pool, b"one").await, content(&pool, b"two").await);
        let entries = count(&mem, |op, path| op == MemOp::PutNew && path.contains("/log/"));
        let runs = Arc::new(AtomicUsize::new(0));
        // Against the drive as it is, only the first would succeed.
        let answers = batch(&pool, &d, vec![
            put_plan("a", &one, absent(), &runs),
            put_plan("a", &two, Precondition { if_match: Some(one.etag()), ..Default::default() }, &runs),
            rename_plan("a", "b"),
        ])
        .await;
        assert_eq!(answers.iter().map(version).collect::<Vec<_>>(), [VersionId::new(1, 0), VersionId::new(1, 1), VersionId::new(1, 2)]);
        assert_eq!(entries.load(Ordering::SeqCst), 1, "one log entry");
        let etag = |a: &Committed, key: &str| {
            let s = &a.as_ref().unwrap().1;
            s.lookup(&Key::parse(key).unwrap()).and_then(|o| s.record(&o)).map(|r| r.etag.clone())
        };
        assert_eq!(etag(&answers[0], "a"), Some(one.etag()));
        assert_eq!(etag(&answers[1], "a"), Some(two.etag()));
        assert_eq!((etag(&answers[2], "a"), etag(&answers[2], "b")), (None, Some(two.etag())));
        assert_eq!(d.snapshot().rows(), answers[2].as_ref().unwrap().1.rows());
        mem.set_hook(None);
        let again = Pool::open(pool.store.clone(), 1 << 20).await.unwrap();
        assert_eq!(again.drive("d").unwrap().snapshot().rows(), d.snapshot().rows(), "the log entry replays to the same state");
    }

    /// A transaction that fails gets its own error, and the rest of its batch commits without
    /// it. A failure that rests on an earlier transaction of the batch is reported only once that
    /// transaction is written.
    #[tokio::test]
    async fn a_failed_transaction_leaves_the_rest_of_its_batch() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let one = content(&pool, b"one").await;
        let runs = Arc::new(AtomicUsize::new(0));
        let plans = || {
            vec![
                delete_plan("missing"),
                put_plan("a", &one, absent(), &runs),
                put_plan("a", &one, absent(), &runs),
                put_plan("b", &one, Precondition::default(), &runs),
            ]
        };
        // While the log cannot be written, only the failure planned against the drive as it is
        // is reported as such.
        on_put_new(&mem, Fault::Fail);
        let answers = batch(&pool, &d, plans()).await;
        assert!(matches!(answers[0], Err(CommitError::Op(OpError::NoSuchKey))));
        assert!(answers[1..].iter().all(|a| matches!(a, Err(CommitError::Other(_)))));
        assert_eq!(d.snapshot().seq(), 0);
        mem.set_hook(None);
        let answers = batch(&pool, &d, plans()).await;
        assert!(matches!(answers[0], Err(CommitError::Op(OpError::NoSuchKey))));
        assert_eq!(version(&answers[1]), VersionId::new(1, 0));
        assert!(matches!(answers[2], Err(CommitError::Op(OpError::PreconditionFailed { current: Some(v) })) if v == VersionId::new(1, 0)));
        assert_eq!(version(&answers[3]), VersionId::new(1, 1));
        let s = d.snapshot();
        let a = s.lookup(&Key::parse("a").unwrap()).unwrap();
        assert_eq!(s.history(&a).unwrap().iter().map(|r| r.version).collect::<Vec<_>>(), [VersionId::new(1, 0)]);
    }

    /// A batched commit's feed has each transaction's changes in order, with keys as they were
    /// just before and after that transaction, and each transaction is a version in its object's
    /// history. A server that replays the log reports the same.
    #[tokio::test]
    async fn a_batched_commit_feeds_and_records_each_transaction() {
        let pool = Pool::open(Store::memory().unwrap(), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let one = content(&pool, b"one").await;
        let answers = batch(&pool, &d, vec![put_plan("f/a", &one, Precondition::default(), &Arc::default()), rename_plan("f/a", "g/b"), delete_plan("g/b")]).await;
        let v = |i| VersionId::new(1, i);
        assert_eq!(answers.iter().map(version).collect::<Vec<_>>(), [v(0), v(1), v(2)]);
        let feed = |batches: Vec<FeedBatch>| {
            batches.iter().flat_map(|b| b.changes.iter().map(|c| (b.seq, c.op, c.key.clone(), c.from_key.clone(), c.version_id))).collect::<Vec<_>>()
        };
        let s = |k: &str| k.to_owned();
        let expected = vec![
            (1, "create", s("f/"), None, v(0)),
            (1, "put", s("f/a"), None, v(0)),
            (1, "create", s("g/"), None, v(1)),
            (1, "rename", s("g/b"), Some(s("f/a")), v(1)),
            (1, "delete", s("g/b"), None, v(2)),
        ];
        assert_eq!(feed(d.changes_since(0).unwrap()), expected);
        let oid = answers[0].as_ref().unwrap().1.lookup(&Key::parse("f/a").unwrap()).unwrap();
        let history = d.snapshot().history(&oid).unwrap().iter().map(|r| (r.version, r.op)).collect::<Vec<_>>();
        assert_eq!(history, [(v(0), Op::Put), (v(1), Op::Rename), (v(2), Op::Delete)]);
        let replayed = pool.replay(&d.id, &mut DriveState::empty(), &mut Cadence::default()).await.unwrap();
        assert_eq!(feed(replayed), expected, "replayed from the log");
    }

    /// A commit holds at most [`BATCH_TXNS`] transactions and, unless its first alone is larger,
    /// [`BATCH_BYTES`] of them. The rest wait for the next.
    #[tokio::test]
    async fn batches_are_capped() {
        let store = Store::memory().unwrap();
        let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let one = content(&pool, b"x").await;
        let runs = Arc::new(AtomicUsize::new(0));
        let plans = (0..BATCH_TXNS + 10).map(|i| put_plan(&format!("k{i}"), &one, Precondition::default(), &runs)).collect();
        let versions: Vec<VersionId> = batch(&pool, &d, plans).await.iter().map(version).collect();
        let expected: Vec<VersionId> = (0..BATCH_TXNS as u32).map(|i| VersionId::new(1, i)).chain((0..10).map(|i| VersionId::new(2, i))).collect();
        assert_eq!(versions, expected);
        // Transactions of about 80 KB: a file of 1,000 extents, all of one shard.
        let e = voidfs_core::content::from_bytes(&Bytes::from_static(b"x"), pool.params);
        let big = pool.describe(vec![e.extents[0].clone(); 1000]).await.unwrap();
        let plans = (0..30).map(|i| put_plan(&format!("big{i}"), &big, Precondition::default(), &runs)).collect();
        let answers = batch(&pool, &d, plans).await;
        let seqs: HashSet<u64> = answers.iter().map(|a| version(a).seq).collect();
        let commits: Vec<Commit> = log(&store, &d).await.into_iter().filter(|c| seqs.contains(&c.seq)).collect();
        assert!(commits.len() >= 3, "{} commits", commits.len());
        assert_eq!(commits.iter().map(|c| c.txns.len()).sum::<usize>(), 30);
        for c in &commits {
            let size: usize = c.txns.iter().map(|t| serde_json::to_vec(t).unwrap().len()).sum();
            assert!(c.txns.len() > 1 && size <= BATCH_BYTES, "seq {}: {} transactions, {size} bytes", c.seq, c.txns.len());
        }
        // A transaction larger than the cap commits, alone.
        let huge = ContentDescriptor::Inline { extents: vec![e.extents[0].clone(); 20_000] };
        let answers = batch(&pool, &d, vec![put_plan("huge", &huge, Precondition::default(), &runs), put_plan("small", &one, Precondition::default(), &runs)]).await;
        let first = version(&answers[0]);
        assert_eq!(version(&answers[1]), VersionId::new(first.seq + 1, 0));
        assert!(store.get(&log_path(&d.id, first.seq)).await.unwrap().unwrap().len() > BATCH_BYTES);
    }

    /// Transactions queued together are planned in their order, share log entries up to the
    /// cap, and each gets its own answer.
    #[tokio::test]
    async fn transactions_committed_together_keep_their_order() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let one = content(&pool, b"one").await;
        let entries = count(&mem, |op, path| op == MemOp::PutNew && path.contains("/log/"));
        // The folder can go only once the file in it has.
        let answers = pool.commit_all(&d, [put_plan("a/x", &one, Precondition::default(), &Arc::default()), delete_plan("a/x"), delete_plan("a/")]).await;
        assert_eq!(answers.iter().map(version).collect::<Vec<_>>(), [VersionId::new(1, 0), VersionId::new(1, 1), VersionId::new(1, 2)]);
        assert!(d.snapshot().lookup(&Key::parse("a/").unwrap()).is_none());
        let runs = Arc::new(AtomicUsize::new(0));
        let answers = pool.commit_all(&d, (0..BATCH_TXNS + 1).map(|i| put_plan(&format!("k{i}"), &one, Precondition::default(), &runs))).await;
        assert_eq!(answers.iter().map(version).collect::<Vec<_>>(), (0..BATCH_TXNS as u32).map(|i| VersionId::new(2, i)).chain([VersionId::new(3, 0)]).collect::<Vec<_>>());
        assert_eq!(entries.load(Ordering::SeqCst), 3);
        mem.set_hook(None);
    }

    /// A mutation that waited longer than [`QUEUE_MAX`] for the log is handed back, not committed
    /// so long after the garbage-collection checks it relied on (format §12.4).
    #[tokio::test]
    async fn a_mutation_that_waited_too_long_is_handed_back() {
        let clock = Clock::manual("2026-09-28T00:00:00Z".parse().unwrap());
        let pool = Pool::open_with(Store::memory().unwrap(), 1 << 20, clock.clone()).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let one = content(&pool, b"one").await;
        let runs = Arc::new(AtomicUsize::new(0));
        let busy = d.commit_lock.lock().await;
        let late = tokio::spawn({
            let (pool, d, plan) = (pool.clone(), d.clone(), put_plan("a", &one, Precondition::default(), &runs));
            async move { pool.commit(&d, plan).await.map(|(v, _)| v) }
        });
        while d.queue.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        clock.advance(QUEUE_MAX + Duration::from_secs(1));
        drop(busy);
        assert!(matches!(late.await.unwrap(), Err(CommitError::Retry)));
        assert_eq!(runs.load(Ordering::SeqCst), 0, "never planned");
        assert_eq!(version(&pool.commit(&d, put_plan("a", &one, Precondition::default(), &runs)).await), VersionId::new(1, 0));
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
        let store = Store::mem(mem.clone());
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
        let e = Pool::open(Store::mem(mem.clone()), 1 << 20).await.err().unwrap();
        assert!(format!("{e:#}").contains("--commit-guard external"), "{e:#}");
        assert!(mem.peek("voidfs.json").is_none());
    }

    #[tokio::test]
    async fn the_external_guard_checks_before_each_write() {
        // The store rejects conditional writes outright; the external guard sends none.
        let mem = Arc::new(MemStore::new(Clock::System));
        on_put_new(&mem, Fault::Fail);
        let store = Store::mem(mem.clone());
        let p1 = Pool::open_as(store.clone(), 1 << 20, Clock::System, CommitGuard::External).await.unwrap();
        let desc: PoolDescriptor = serde_json::from_slice(&mem.peek("voidfs.json").unwrap()).unwrap();
        assert_eq!(desc.commit_guard, CommitGuard::External);
        p1.create_drive("shared", None).await.unwrap();
        // Two servers on one external pool break its rule, but show that the check before each
        // write finds a commit already there.
        let p2 = Pool::open_as(store.clone(), 1 << 20, Clock::System, CommitGuard::External).await.unwrap();
        lose_a_race(&p1, &p2).await;
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
        let store = Store::mem(mem.clone());
        let state = DriveState::empty().apply(&commit(1, (0..n).map(|i| file(2 * i)).collect())).unwrap();
        {
            let pool = Pool::open_with(store.clone(), 64 << 20, clock.clone()).await.unwrap();
            let d = pool.create_drive("big", None).await.unwrap();
            pool.write_checkpoint(&d.id, Arc::new(state.clone()), pool.clock.mono(), None).await.unwrap();
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
        let mut last = d.checkpoint.lock().await;
        let mut prev = tables(&pool, last.as_ref().unwrap()).await;
        for t in [&prev.entries, &prev.objects, &prev.history] {
            assert!(t.len() >= 4, "only {} segments", t.len());
        }
        let puts = count(&mem, page_puts);
        // A file inserted in the middle of every table, then one removed.
        for (seq, txn, removed) in [(2, file(30_001), 0), (3, removal(14_000), 1)] {
            state = state.apply(&commit(seq, vec![txn])).unwrap();
            puts.store(0, Ordering::SeqCst);
            pool.renew(&mut last).await;
            let written = pool.write_checkpoint(&d.id, Arc::new(state.clone()), pool.clock.mono(), last.as_ref()).await.unwrap();
            let next = tables(&pool, &written).await;
            let new = new_pages(&prev, &next);
            assert!(new[..3].iter().all(|n| (1..=2).contains(n)) && new[3] == removed, "seq {seq}: new pages per table {new:?}");
            assert_eq!(puts.load(Ordering::SeqCst), new.iter().sum::<usize>(), "seq {seq}: only new pages are stored");
            *last = Some(written);
            prev = next;
        }
        drop(last);
        mem.set_hook(None);
        let again = Pool::open(pool.store.clone(), 64 << 20).await.unwrap();
        let d = again.drive("big").unwrap();
        assert_eq!(d.snapshot().rows(), state.rows(), "with every table in use");
        // Without the previous checkpoint, a server that has just started stores every page.
        let puts = count(&mem, page_puts);
        let all = again.write_checkpoint(&d.id, Arc::new(state), again.clock.mono(), None).await.unwrap();
        assert_eq!(puts.load(Ordering::SeqCst), all.pages.len());
    }

    /// After a restart, the feed reports the changes replayed from the log since the last
    /// checkpoint, and no earlier ones (protocol §5.6).
    #[tokio::test]
    async fn a_reloaded_drive_reports_the_changes_it_replayed() {
        let store = Store::memory().unwrap();
        let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        for key in ["a", "b", "c"] {
            put(&pool, &d, key, b"x").await;
        }
        let keys = |d: &Drive, since| d.changes_since(since).map(|bs| bs.iter().flat_map(|b| b.changes.iter().map(|c| c.key.clone())).collect::<Vec<_>>());
        let again = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let reloaded = again.drive("d").unwrap();
        assert_eq!(keys(&reloaded, 0), Some(vec!["a".to_owned(), "b".into(), "c".into()]));
        assert_eq!(keys(&reloaded, 2), Some(vec!["c".to_owned()]));
        // With a checkpoint at seq 3, the log is replayed from there.
        pool.write_checkpoint(&d.id, d.snapshot(), pool.clock.mono(), None).await.unwrap();
        for key in ["d", "e"] {
            put(&pool, &d, key, b"x").await;
        }
        let again = Pool::open(store, 1 << 20).await.unwrap();
        let reloaded = again.drive("d").unwrap();
        assert_eq!(keys(&reloaded, 3), Some(vec!["d".to_owned(), "e".into()]));
        assert_eq!(keys(&reloaded, 2), None, "before the checkpoint");
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
        let big = pool.describe(vec![e.extents[0].clone(); 1000]).await.unwrap();
        let (mut logged, mut last) = (0, 0);
        while checkpoints(&store, &d).await.is_empty() {
            let v = version(&pool.commit(&d, put_plan("big", &big, Precondition::default(), &Arc::default())).await);
            settle(&d).await;
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
        settle(&d).await;
        assert_eq!(checkpoints(&store, &d).await, [first], "counted from the last checkpoint, not from seq 0");
        put(&pool, &d, "small", b"tick").await;
        settle(&d).await;
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
        let source = tables(&pool, src.checkpoint.lock().await.as_ref().unwrap()).await;
        let forked = tables(&pool, a.checkpoint.lock().await.as_ref().unwrap()).await;
        assert_eq!(new_pages(&source, &forked), [0; 4]);
        // After a commit to the source, a fork stores only the segments that commit changed.
        put(&pool, &src, "f010001", b"new").await;
        puts.store(0, Ordering::SeqCst);
        let b = pool.create_drive("fork-b", Some(&src)).await.unwrap();
        let forked = tables(&pool, b.checkpoint.lock().await.as_ref().unwrap()).await;
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
        let mut last = d.checkpoint.lock().await;
        let index = last.as_ref().unwrap().index.clone();
        let reads = {
            let index = index.clone();
            count(&mem, move |op, path| op == MemOp::Get && path == index)
        };
        clock.advance(REUSE_WITHOUT_REREAD - Duration::from_secs(1));
        pool.renew(&mut last).await;
        assert!(last.is_some());
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        clock.advance(2 * HOUR);
        pool.renew(&mut last).await;
        assert_eq!((reads.load(Ordering::SeqCst), last.as_ref().unwrap().seen), (1, clock.mono()));
        clock.advance(13 * HOUR);
        let stale = last.take().unwrap();
        assert!(pool.write_checkpoint(&d.id, Arc::new(state), clock.mono(), Some(&stale)).await.is_err(), "written too long after the read it relies on");
        // An index that has gone vouches for nothing.
        pool.store.delete(&index).await.unwrap();
        let mut gone = Some(stale);
        pool.renew(&mut gone).await;
        assert!(gone.is_none());
    }

    // -----------------------------------------------------------------------------------------
    // Checkpoints in the background

    /// Requests to a [`MemStore`] held back by [`hold`].
    struct Held {
        release: tokio::sync::watch::Sender<bool>,
        /// Requests held so far.
        held: Arc<AtomicUsize>,
    }

    /// Holds the requests to `mem` that `which` picks until [`Held::release`] says `true`.
    fn hold(mem: &MemStore, which: impl Fn(MemOp, &str) -> bool + Send + Sync + 'static) -> Held {
        let (release, released) = tokio::sync::watch::channel(false);
        let held = Arc::new(AtomicUsize::new(0));
        let h = held.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            if !which(op, path) {
                return futures::future::ready(Fault::None).boxed();
            }
            h.fetch_add(1, Ordering::SeqCst);
            let mut released = released.clone();
            async move {
                let _ = released.wait_for(|r| *r).await;
                Fault::None
            }
            .boxed()
        })));
        Held { release, held }
    }

    fn index_puts(op: MemOp, path: &str) -> bool {
        op == MemOp::Put && path.contains("/checkpoints/")
    }

    /// Waits until `n` has reached `at_least`.
    async fn reached(n: &AtomicUsize, at_least: usize) {
        while n.load(Ordering::SeqCst) < at_least {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Commits `n` puts of `desc` to `d`, one after another; returns the last one's version.
    async fn ticks(pool: &Arc<Pool>, d: &Arc<Drive>, desc: &ContentDescriptor, n: u64) -> VersionId {
        let mut v = VersionId::new(0, 0);
        for _ in 0..n {
            v = version(&pool.commit(d, put_plan("tick", desc, Precondition::default(), &Arc::default())).await);
        }
        v
    }

    /// Nothing ties a checkpoint to the commit that made it due (format §8): the commit is
    /// answered, and later ones are written, while the checkpoint is. There is one at a time: a
    /// drive due again meanwhile starts the next with its first commit after.
    #[tokio::test]
    async fn checkpoints_do_not_hold_up_commits() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let tick = content(&pool, b"tick").await;
        let h = hold(&mem, index_puts);
        let limit = Duration::from_secs(60);
        tokio::time::timeout(limit, ticks(&pool, &d, &tick, CHECKPOINT_EVERY + 5)).await.expect("commits go on while a checkpoint is written");
        reached(&h.held, 1).await;
        // Due again, while the first is still being written.
        tokio::time::timeout(limit, ticks(&pool, &d, &tick, CHECKPOINT_EVERY)).await.unwrap();
        assert_eq!(h.held.load(Ordering::SeqCst), 1, "one checkpoint at a time");
        h.release.send(true).unwrap();
        settle(&d).await;
        assert_eq!(checkpoints(&pool.store, &d).await, [CHECKPOINT_EVERY]);
        let v = ticks(&pool, &d, &tick, 1).await;
        settle(&d).await;
        assert_eq!(checkpoints(&pool.store, &d).await, [CHECKPOINT_EVERY, v.seq]);
        assert_eq!(pool.metrics.checkpoints_written.get(), 2);
    }

    /// A hard delete waits for the checkpoint being written, so that nothing of the drive is
    /// written after it has gone.
    #[tokio::test]
    async fn a_hard_delete_waits_for_a_checkpoint_being_written() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let tick = content(&pool, b"tick").await;
        let h = hold(&mem, index_puts);
        ticks(&pool, &d, &tick, CHECKPOINT_EVERY).await;
        reached(&h.held, 1).await;
        let deleting = tokio::spawn({
            let pool = pool.clone();
            async move { pool.hard_delete("d").await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!deleting.is_finished(), "the hard delete waits for the checkpoint");
        h.release.send(true).unwrap();
        assert!(deleting.await.unwrap());
        assert!(pool.store.list_recursive(&format!("drives/{}/", d.id)).await.unwrap().is_empty(), "nothing of the drive is left");
    }

    /// A fork waits for its source's checkpoint being written, so that it starts from that one
    /// (format §9).
    #[tokio::test]
    async fn a_fork_waits_for_a_checkpoint_being_written() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let tick = content(&pool, b"tick").await;
        let index = format!("drives/{}/checkpoints/", d.id);
        let h = hold(&mem, move |op, path| index_puts(op, path) && path.starts_with(&index));
        ticks(&pool, &d, &tick, CHECKPOINT_EVERY).await;
        reached(&h.held, 1).await;
        let forking = tokio::spawn({
            let (pool, d) = (pool.clone(), d.clone());
            async move { pool.create_drive("fork", Some(&d)).await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!forking.is_finished(), "the fork waits for the checkpoint");
        h.release.send(true).unwrap();
        let fork = forking.await.unwrap();
        assert_eq!(checkpoints(&pool.store, &d).await, [CHECKPOINT_EVERY]);
        assert_eq!(checkpoints(&pool.store, &fork).await, [CHECKPOINT_EVERY]);
    }

    /// A checkpoint's rows reference content on the strength of its state having been the
    /// drive's (format §12.4, option 1), so it is written within 12 hours of that. One held up
    /// longer fails, and the next is due a whole interval later.
    #[tokio::test]
    async fn a_checkpoint_of_a_state_seen_too_long_ago_is_not_written() {
        let clock = Clock::manual("2026-09-28T00:00:00Z".parse().unwrap());
        let mem = Arc::new(MemStore::new(clock.clone()));
        let pool = Pool::open_with(Store::mem(mem.clone()), 1 << 20, clock.clone()).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let tick = content(&pool, b"tick").await;
        let h = hold(&mem, page_puts);
        ticks(&pool, &d, &tick, CHECKPOINT_EVERY).await;
        reached(&h.held, 1).await;
        clock.advance(13 * HOUR);
        h.release.send(true).unwrap();
        settle(&d).await;
        assert!(checkpoints(&pool.store, &d).await.is_empty());
        assert_eq!((pool.metrics.checkpoints_written.get(), pool.metrics.checkpoints_failed.get()), (0, 1));
        ticks(&pool, &d, &tick, CHECKPOINT_EVERY - 1).await;
        settle(&d).await;
        assert!(checkpoints(&pool.store, &d).await.is_empty(), "a failure waits a whole interval");
        ticks(&pool, &d, &tick, 1).await;
        settle(&d).await;
        assert_eq!(checkpoints(&pool.store, &d).await, [2 * CHECKPOINT_EVERY]);
    }

    /// One task drains a drive's queue at a time, so whatever else waits for the commit lock (a
    /// fork, or a hard delete) waits for the batch being written, not for a line of tasks that grows with every mutation. With a task
    /// per mutation, under eight clients writing steadily, it waited 1.5 s, then 9, then 56.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn steady_commits_do_not_starve_the_commit_lock() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        let one = content(&pool, b"one").await;
        mem.set_hook(Some(Arc::new(|op, path| {
            let slow = op == MemOp::PutNew && path.contains("/log/");
            async move {
                if slow {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Fault::None
            }
            .boxed()
        })));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let clients: Vec<_> = (0..8)
            .map(|c| {
                let (pool, d, one, stop) = (pool.clone(), d.clone(), one.clone(), stop.clone());
                tokio::spawn(async move {
                    let mut i = 0;
                    while !stop.load(Ordering::SeqCst) {
                        pool.commit(&d, put_plan(&format!("c{c}-{i}"), &one, Precondition::default(), &Arc::default())).await.unwrap();
                        i += 1;
                    }
                    i
                })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(300)).await;
        for _ in 0..5 {
            let started = Instant::now();
            drop(tokio::time::timeout(Duration::from_secs(5), d.commit_lock.lock()).await.expect("the commit lock is not starved"));
            assert!(started.elapsed() < Duration::from_millis(500), "waited {:?}", started.elapsed());
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        stop.store(true, Ordering::SeqCst);
        let mut done = 0;
        for c in clients {
            done += c.await.unwrap();
        }
        assert!(done > 100, "only {done} commits");
    }

    // -----------------------------------------------------------------------------------------
    // Caches

    /// `n` different objects of `size` bytes, with their hashes.
    fn blobs(tag: &str, n: usize, size: usize) -> Vec<(ShardHash, Bytes)> {
        (0..n)
            .map(|i| {
                let mut b = format!("{tag} {i} ").into_bytes();
                b.resize(size, b'.');
                (ShardHash::of(&b), Bytes::from(b))
            })
            .collect()
    }

    async fn write_as(pool: &Pool, kind: guard::Kind, items: &[(ShardHash, Bytes)]) {
        match kind {
            guard::Kind::Shard => pool.write_shards(&items.iter().map(|(hash, bytes)| Shard { hash: *hash, bytes: bytes.clone() }).collect::<Vec<_>>()).await,
            guard::Kind::Page => pool.write_pages(&items.iter().map(|(hash, bytes)| Page { hash: *hash, bytes: bytes.clone() }).collect::<Vec<_>>()).await,
        }
        .unwrap();
    }

    async fn read_as(pool: &Pool, kind: guard::Kind, h: &ShardHash) -> Bytes {
        match kind {
            guard::Kind::Shard => pool.shard(h).await,
            guard::Kind::Page => pool.page(h).await,
        }
        .unwrap()
    }

    /// A cache full of objects read many times still takes in new ones, written or read, and
    /// serves them from then on. Under moka's default policy, it turned them away.
    #[tokio::test]
    async fn a_full_cache_keeps_new_shards_and_pages() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        // Sixteen objects fill each cache; the page cache holds an eighth of the shard cache.
        for (kind, cache, series, size) in [(guard::Kind::Shard, &pool.shards, &pool.metrics.shards, 64 << 10), (guard::Kind::Page, &pool.pages, &pool.metrics.pages, 8 << 10)] {
            let old = blobs(&format!("{kind:?} old"), 16, size);
            write_as(&pool, kind, &old).await;
            cache.run_pending_tasks();
            for _ in 0..20 {
                for (h, _) in &old {
                    read_as(&pool, kind, h).await;
                }
            }
            cache.run_pending_tasks();
            let gets = count(&mem, |op, path| op == MemOp::Get && (path.starts_with("shards/") || path.starts_with("pages/")));
            let [written, read] = &blobs(&format!("{kind:?} new"), 2, size)[..] else { unreachable!() };
            write_as(&pool, kind, std::slice::from_ref(written)).await;
            cache.run_pending_tasks();
            assert_eq!(read_as(&pool, kind, &written.0).await, written.1);
            assert_eq!(gets.load(Ordering::SeqCst), 0, "{kind:?}: one written is kept");
            assert_ne!(cache.get(&written.0).unwrap().as_ptr(), written.1.as_ptr(), "{kind:?}: a copy, not a slice of the writer's buffer");
            pool.store.put(&kind.path(&read.0), read.1.clone()).await.unwrap();
            for _ in 0..3 {
                assert_eq!(read_as(&pool, kind, &read.0).await, read.1);
                cache.run_pending_tasks();
            }
            assert_eq!(read_as(&pool, kind, &written.0).await, written.1);
            assert_eq!(gets.load(Ordering::SeqCst), 1, "{kind:?}: one read is fetched once, then kept");
            assert!(cache.weighted_size() <= cache.policy().max_capacity().unwrap(), "{kind:?}: the cache stays within its bytes");
            // A deleted object is dropped.
            pool.forget(&written.0);
            assert_eq!(read_as(&pool, kind, &written.0).await, written.1);
            assert_eq!(gets.load(Ordering::SeqCst), 2, "{kind:?}: one forgotten is fetched again");
            cache.run_pending_tasks();
            assert_eq!(series.evictions.get(), 2, "{kind:?}: the two new objects each evicted an old one; forgetting one is not an eviction");
            assert_eq!(series.misses.get(), 2, "{kind:?}: a miss for each fetch");
            assert_eq!(series.hits.get(), 16 * 20 + 1 + 2 + 1, "{kind:?}: a hit for every other read");
        }
    }

    /// Reading a manifest tree doesn't rely on the page cache keeping the pages it fetched.
    #[tokio::test]
    async fn a_tree_reads_whatever_the_page_cache_keeps() {
        let mem = Arc::new(MemStore::new(Clock::System));
        // The page cache holds one byte, so each page goes at the next housekeeping.
        let pool = Pool::open(Store::mem(mem.clone()), 0).await.unwrap();
        let extents: Vec<Extent> = (0..3_000u32).map(|i| Extent::Shard { s: ShardHash::of(&i.to_be_bytes()), n: 1 }).collect();
        let desc = pool.describe(extents.clone()).await.unwrap();
        assert!(matches!(desc, ContentDescriptor::Tree { .. }));
        pool.pages.invalidate_all();
        let weak = Arc::downgrade(&pool);
        mem.set_hook(Some(Arc::new(move |op, path| {
            if op == MemOp::Get && path.starts_with("pages/")
                && let Some(pool) = weak.upgrade()
            {
                pool.pages.run_pending_tasks();
            }
            futures::future::ready(Fault::None).boxed()
        })));
        assert_eq!(pool.extents(&desc).await.unwrap(), extents);
    }

    // -----------------------------------------------------------------------------------------
    // Metrics

    /// A put, then two reads of what it wrote: both hit the shard cache. A server that has just
    /// started misses once, reading the shard from the bucket, and then hits.
    #[tokio::test]
    async fn metrics_count_cache_hits_and_bucket_requests() {
        use crate::metrics::{BucketOp, encode, sample};
        let store = Store::memory().unwrap();
        let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d = pool.create_drive("counted", None).await.unwrap();
        put(&pool, &d, "a", b"hello").await;
        assert_eq!(read(&pool, &d, "a").await.unwrap(), b"hello");
        assert_eq!(read(&pool, &d, "a").await.unwrap(), b"hello");
        let text = encode(pool.gather_metrics());
        assert_eq!(sample(&text, r#"voidfs_cache_hits_total{cache="shard"}"#), Some(2.0));
        assert_eq!(sample(&text, r#"voidfs_cache_misses_total{cache="shard"}"#), Some(0.0));
        assert_eq!(sample(&text, r#"voidfs_cache_entries{cache="shard"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_cache_bytes{cache="shard"}"#), Some(5.0));
        assert_eq!(sample(&text, r#"voidfs_drives{state="live"}"#), Some(1.0));
        let fresh = Pool::open(store.clone(), 1 << 20).await.unwrap();
        let d = fresh.drive("counted").unwrap();
        let gets = store.metrics.requests(BucketOp::Get);
        assert_eq!(read(&fresh, &d, "a").await.unwrap(), b"hello");
        assert_eq!(read(&fresh, &d, "a").await.unwrap(), b"hello");
        assert_eq!(store.metrics.requests(BucketOp::Get), gets + 1, "one read from the bucket");
        let text = encode(fresh.gather_metrics());
        assert_eq!(sample(&text, r#"voidfs_cache_misses_total{cache="shard"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_cache_hits_total{cache="shard"}"#), Some(1.0));
        let text = encode(store.metrics.gather());
        assert_eq!(sample(&text, r#"voidfs_bucket_requests_total{op="get"}"#), Some((gets + 1) as f64));
        assert_eq!(sample(&text, r#"voidfs_bucket_request_errors_total{op="get"}"#), Some(0.0));
        assert!(sample(&text, r#"voidfs_bucket_requests_total{op="put_new"}"#).unwrap() >= 2.0, "voidfs.json, drive.json and the log entry");
    }

    /// Each log entry's transactions go into the batch histogram; a lost race is counted too.
    #[tokio::test]
    async fn metrics_count_transactions_per_log_entry() {
        use crate::metrics::{encode, sample};
        let pool = Pool::open(Store::memory().unwrap(), 1 << 20).await.unwrap();
        let d = pool.create_drive("batched", None).await.unwrap();
        put(&pool, &d, "one", b"1").await;
        let desc = content(&pool, b"2").await;
        let runs = Arc::new(AtomicUsize::new(0));
        let plans = (0..3).map(|i| put_plan(&format!("k{i}"), &desc, Precondition::default(), &runs)).collect();
        assert!(batch(&pool, &d, plans).await.iter().all(|a| a.is_ok()));
        let text = encode(pool.gather_metrics());
        assert_eq!(sample(&text, r#"voidfs_commits_total{outcome="written"}"#), Some(2.0));
        assert_eq!(sample(&text, "voidfs_commit_transactions_count"), Some(2.0));
        assert_eq!(sample(&text, "voidfs_commit_transactions_sum"), Some(4.0));
        assert_eq!(sample(&text, r#"voidfs_commit_transactions_bucket{le="1"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_commit_transactions_bucket{le="2"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_commit_transactions_bucket{le="4"}"#), Some(2.0));
        assert_eq!(sample(&text, "voidfs_commit_log_write_seconds_count"), Some(2.0));
        // Another server writes the next entry first.
        let other = Pool::open(pool.store.clone(), 1 << 20).await.unwrap();
        put(&other, &other.drive("batched").unwrap(), "elsewhere", b"3").await;
        put(&pool, &d, "after", b"4").await;
        let text = encode(pool.gather_metrics());
        assert_eq!(sample(&text, r#"voidfs_commits_total{outcome="lost_race"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_commits_total{outcome="written"}"#), Some(3.0));
    }
}
