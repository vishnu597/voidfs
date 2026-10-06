// SPDX-License-Identifier: Apache-2.0
//! A pool (format §2–§3) and its drives: loading them from the bucket, committing to their
//! logs, checkpoints, forks, deletion, and the change feed (protocol §5.6).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use futures::future::{BoxFuture, Shared};
use futures::{FutureExt, StreamExt};
use bytes::Bytes;
use serde::Serialize;
use voidfs_core::chunk::{Params, Shard};
use voidfs_core::ids::{DriveId, ObjectId, ShardHash, Timestamp, VersionId};
use voidfs_core::manifest::{self, Page};
use voidfs_core::model::{
    Chunking, Commit, CommitGuard, ContentDescriptor, DriveDescriptor, Extent, Features, ForkOf, INLINE_DATA, KNOWN_INCOMPATIBLE_FEATURES, MULTI_OBJECT_VERSIONS, Kind, Op, PoolDescriptor,
    Txn,
};
use voidfs_core::ops::OpError;
use voidfs_core::state::{DriveState, Spilled};
use voidfs_format::{CheckpointIndex, CheckpointTables, SegmentRef, log_path};

use crate::clock::Clock;
use crate::gc::Phase;
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
/// A checkpoint lists the previous one's pages without storing them again only if that one's
/// index was seen this recently; otherwise it reads the index again first (format §12.4,
/// option 1).
const REUSE_WITHOUT_REREAD: Duration = Duration::from_secs(6 * 3600);
/// Change-feed batches kept in memory per drive.
const FEED_KEEP: usize = 10_000;
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
/// The longest a log entry is held after the one before it, for the requests that one answered to
/// come back (see [`Pool::drain`])...
const HOLD_MAX: Duration = Duration::from_millis(2);
/// ...and the share of that entry's write time it may take, at most.
const HOLD_SHARE: u32 = 4;
/// How coarse tokio's timers are. A hold shorter than this would take a tick all the same, so
/// an entry written in less than four is not held.
const TIMER_TICK: Duration = Duration::from_millis(1);

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
    // Every other object the version changed (RFC 0004): what a folder restore rolls back, brings
    // back or takes out, and what a rename replaces. What it took out comes before the target,
    // children before their folders, as the version took them out; the rest after, folders first.
    let (mut gone, mut rest) = (Vec::new(), Vec::new());
    if after.multi_object_versions() {
        for oid in after.last_changed().iter().filter(|o| **o != txn.target && before.record(o).is_some()) {
            let kind = after.kind(oid).unwrap_or(Kind::File);
            match (before.key_of(oid), after.key_of(oid)) {
                (Some(was), None) => gone.push(FeedChange { op: "delete", key: was, from_key: None, object_id: oid.clone(), version_id: v, kind }),
                (was, Some(is)) => {
                    let from_key = was.filter(|w| *w != is);
                    rest.push(FeedChange { op: txn.op.as_str(), key: is, from_key, object_id: oid.clone(), version_id: v, kind });
                }
                (None, None) => {}
            }
        }
    }
    gone.sort_by(|a, b| b.key.cmp(&a.key));
    rest.sort_by(|a, b| a.key.cmp(&b.key));
    changes.extend(gone);
    let kind = after.kind(&txn.target).unwrap_or(Kind::File);
    let (key, from_key) = match txn.op {
        Op::Delete => (before.key_of(&txn.target), None),
        Op::Rename => (after.key_of(&txn.target), before.key_of(&txn.target)),
        _ => (after.key_of(&txn.target), None),
    };
    if let Some(key) = key {
        changes.push(FeedChange { op: txn.op.as_str(), key, from_key, object_id: txn.target.clone(), version_id: v, kind });
    }
    changes.extend(rest);
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
    /// When it was queued, for holding log entries, which tokio's timers time.
    queued: Instant,
}

/// A log entry just written. The requests it answered may send their next ones at once, and the
/// next entry waits for them a little (see [`Pool::drain`]).
#[derive(Clone, Copy)]
struct Landed {
    /// When its mutations were answered.
    at: Instant,
    /// How long the next entry may wait for them.
    bound: Duration,
    /// How many mutations it answered.
    answered: usize,
}

impl Landed {
    fn for_write(at: Instant, took: Duration, answered: usize) -> Option<Self> {
        let bound = (took / HOLD_SHARE).min(HOLD_MAX);
        (bound >= TIMER_TICK).then_some(Self { at, bound, answered })
    }

    /// Whether `w` was queued while the next entry could wait for it.
    fn caught(&self, w: &Waiting) -> bool {
        w.queued > self.at && w.queued <= self.at + self.bound
    }
}

/// How many mutations queued within the bound of each recent log entry, against how many it
/// answered: each entry's are added once the sums so far have lost an eighth, so that the last
/// eight or so entries count most.
#[derive(Default)]
struct Returns {
    caught: f64,
    answered: f64,
}

impl Returns {
    fn add(&mut self, caught: usize, answered: usize) {
        self.caught = self.caught * 0.875 + caught as f64;
        self.answered = self.answered * 0.875 + answered as f64;
    }

    /// Whether holding pays: at least half as many came back as were answered, or nothing is
    /// known yet.
    fn pay(&self) -> bool {
        2.0 * self.caught >= self.answered
    }
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
    /// locks takes this one first: a checkpoint takes the commit lock at its end, to put what it
    /// spilled into the state (format §8.2).
    checkpoint: Arc<tokio::sync::Mutex<Option<Checkpointed>>>,
    /// Mutations waiting for the next commit, in the order they arrived.
    queue: std::sync::Mutex<VecDeque<Waiting>>,
    /// Whether a task is draining `queue`: there is at most one. Set and cleared only while
    /// `queue` is locked.
    draining: std::sync::atomic::AtomicBool,
    /// Told whenever a mutation is queued, for the draining task holding the next entry.
    queued: tokio::sync::Notify,
    /// Whether the requests log entries answered came back in time lately, which decides whether
    /// the draining task holds the next entry (see [`Pool::drain`]). Kept here, not in the task,
    /// because the task ends whenever the queue empties. Only that task takes it.
    returns: std::sync::Mutex<Returns>,
    feed: RwLock<VecDeque<FeedBatch>>,
    /// The oldest seq the feed can still report changes after.
    feed_floor: RwLock<u64>,
    notify: tokio::sync::watch::Sender<u64>,
    pub forks: RwLock<Vec<DriveId>>,
}

/// What [`Pool::find_shards`] found of a shard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    Held,
    Missing,
    /// In the bucket, with this length rather than the one it was listed with.
    Length(u64),
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
            queued: tokio::sync::Notify::new(),
            returns: std::sync::Mutex::new(Returns::default()),
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
    pub guard: Arc<Guard>,
    /// Whether the pool lists `inline-data`, so that content may be held in data extents
    /// (format §3.1, §5). Read when the pool opens, as the rest of `voidfs.json` is.
    pub inline_data: bool,
    /// Whether the pool lists `multi-object-versions`, so that a transaction's version is every
    /// object's it changes (format §7.1, RFC 0004). Read when the pool opens.
    pub multi_object_versions: bool,
    shards: moka::sync::Cache<ShardHash, Bytes>,
    pages: moka::sync::Cache<ShardHash, Bytes>,
    /// Shards and pages being fetched from the bucket, which reads that miss meanwhile wait for.
    shard_fetches: Fetches,
    page_fetches: Fetches,
    drives: RwLock<HashMap<DriveId, Arc<Drive>>>,
    aliases: RwLock<HashMap<String, DriveId>>,
    deleted: RwLock<HashMap<String, DriveId>>,
    /// Serializes changes to `drives`, `aliases` and `deleted`. Never held across an await, and
    /// no other of their locks is held while taking it.
    registry: std::sync::Mutex<()>,
    authority: String,
    pub metrics: PoolMetrics,
}

/// What [`Pool::drop_caches`] emptied the caches of.
#[derive(Debug)]
pub struct Dropped {
    pub shards: u64,
    pub shard_bytes: u64,
    pub pages: u64,
    pub page_bytes: u64,
}

/// What a cache keeps of `b`: a copy holding only these bytes. A slice keeps its whole buffer
/// alive, and a shard cut by the chunker is a slice of a buffer of up to about 32 MiB.
fn cached(b: &Bytes) -> Bytes {
    Bytes::copy_from_slice(b)
}

/// A fetch of a shard or page, which every read waiting for it shares.
type Fetch = Shared<BoxFuture<'static, Result<Bytes, Arc<anyhow::Error>>>>;
/// The fetches in flight, by hash. Never held across an await. A fetch's task is started with
/// this held, and is what takes the fetch out again.
type Fetches = Arc<std::sync::Mutex<HashMap<ShardHash, Fetch>>>;

/// Takes a fetch out of [`Fetches`] when its task ends, however it ends.
struct Leaving {
    fetches: Fetches,
    h: ShardHash,
}

impl Drop for Leaving {
    fn drop(&mut self) {
        self.fetches.lock().unwrap().remove(&self.h);
    }
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
        Pool::open_creating(store, cache_bytes, clock, guard, &[]).await
    }

    /// [`Pool::open_as`], creating a new pool with `features` listed in `features.incompatible`
    /// (format §3.1). An existing pool keeps its own: a feature is added to one with
    /// [`enable_feature`], once every server that writes it implements the feature.
    pub async fn open_creating(store: crate::store::Store, cache_bytes: u64, clock: Clock, guard: CommitGuard, features: &[String]) -> anyhow::Result<Arc<Pool>> {
        if let Some(f) = features.iter().find(|f| !KNOWN_INCOMPATIBLE_FEATURES.contains(&f.as_str())) {
            bail!("this server does not implement the feature {f:?}; it knows {KNOWN_INCOMPATIBLE_FEATURES:?}");
        }
        let mut incompatible: Vec<String> = Vec::new();
        for f in features {
            if !incompatible.contains(f) {
                incompatible.push(f.clone());
            }
        }
        let (desc, bytes, created) = match store.get(probe::DESCRIPTOR).await? {
            Some(b) => (serde_json::from_slice::<PoolDescriptor>(&b).context("reading voidfs.json")?, b, false),
            None => {
                let d = PoolDescriptor {
                    format: voidfs_core::FORMAT_VERSION,
                    pool_id: format!("p-{}", uuid::Uuid::new_v4()),
                    created: clock.now(),
                    features: Features { compatible: vec![], incompatible },
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
                    return Box::pin(Pool::open_creating(store, cache_bytes, clock, guard, features)).await;
                }
                (d, b, true)
            }
        };
        desc.check_readable().map_err(|e| anyhow!(e))?;
        for f in features.iter().filter(|f| !desc.has(f)) {
            tracing::warn!(
                "the pool does not list {f}, and it is not new, so --new-pool-feature does not add it. Once every server that writes the pool implements {f}, add it with `voidfs-server pool enable {f}`, then restart them"
            );
        }
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
            inline_data: desc.has(INLINE_DATA),
            multi_object_versions: desc.has(MULTI_OBJECT_VERSIONS),
            desc,
            params,
            clock,
            guard: Arc::new(Guard::new(REUSE_CAPACITY)),
            shards: cache(cache_bytes, metrics.shards.evictions.clone()),
            pages: cache(page_bytes, metrics.pages.evictions.clone()),
            shard_fetches: Fetches::default(),
            page_fetches: Fetches::default(),
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

    /// An empty drive under the pool's rule for versions.
    fn empty_state(&self) -> DriveState {
        DriveState::empty().with_multi_object_versions(self.multi_object_versions)
    }

    async fn load_drive(&self, id: &DriveId) -> anyhow::Result<Option<Drive>> {
        let Some(b) = self.store.get(&format!("drives/{id}/drive.json")).await? else { return Ok(None) };
        let desc: DriveDescriptor = serde_json::from_slice(&b).context("reading drive.json")?;
        let mut cadence = Cadence::default();
        let seen = self.clock.mono();
        let (mut state, last) = match voidfs_format::latest_checkpoint(self, id, self.multi_object_versions).await? {
            Some(c) => (c.state, Some(Checkpointed { pages: c.index.tables.pages(), index: c.path, seen })),
            None if desc.fork_of.is_some() => bail!("fork {id} has no checkpoint"),
            None => (self.empty_state(), None),
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

    /// Applies every commit after `state.seq()`, counts them towards the next checkpoint, and
    /// returns their feed batches.
    async fn replay(&self, id: &DriveId, state: &mut DriveState, cadence: &mut Cadence) -> anyhow::Result<Vec<FeedBatch>> {
        let mut out = Vec::new();
        for (commit, len) in voidfs_format::commits_after(self, id, state.seq()).await? {
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

    /// Reads a checkpoint index and its segments into a state (format §8).
    async fn load_checkpoint(&self, path: &str) -> anyhow::Result<(DriveState, Checkpointed)> {
        let seen = self.clock.mono();
        let c = voidfs_format::load_checkpoint(self, path, self.multi_object_versions).await?;
        Ok((c.state, Checkpointed { index: c.path, pages: c.index.tables.pages(), seen }))
    }

    /// Writes a checkpoint of `state` (format §8) and returns it, with what it spilled. `state`
    /// was the drive's own at `seen`, which vouches for the content its rows reference (format
    /// §12.4, option 1). Pages that `reuse` lists are not stored again; the others go through the
    /// garbage-collection guard like any page.
    ///
    /// A checkpoint holds no data extents (format §8.2): each is stored as a shard, through the
    /// guard like any shard, and its row lists the shard instead.
    async fn write_checkpoint(&self, id: &DriveId, state: Arc<DriveState>, seen: Duration, reuse: Option<&Checkpointed>) -> anyhow::Result<(Checkpointed, Spilled)> {
        let seq = state.seq();
        let time = state.time();
        // Copying the rows out, encoding and hashing them takes hundreds of milliseconds of CPU
        // for a drive of 600,000 rows: not on an async worker.
        let (tables, pages, stats, spilled) = tokio::task::spawn_blocking(move || {
            let mut rows = state.rows();
            let spilled = rows.spill();
            let mut pages = Vec::new();
            let tables = CheckpointTables {
                entries: segments("entries", &rows.entries, |e| format!("{}/{}{}", e.parent, e.name, if e.kind == Kind::Folder { "/" } else { "" }), &mut pages),
                objects: segments("objects", &rows.objects, |o| o.oid.to_string(), &mut pages),
                history: segments("history", &rows.history, |h| format!("{}@{:020}.{}", h.oid, h.version.seq, h.version.idx), &mut pages),
                removed: segments("removed", &rows.removed, |r| format!("{}@{}", r.key, r.oid), &mut pages),
            };
            (tables, pages, serde_json::json!({ "objects": rows.objects.len(), "bytes": state.live_bytes() }), spilled)
        })
        .await?;
        let listed = tables.pages();
        let (reused, new): (Vec<Page>, Vec<Page>) = pages.into_iter().partition(|p| reuse.is_some_and(|r| r.pages.contains(&p.hash)));
        self.write_shards(&spilled.shards).await?;
        self.metrics.checkpoint_spilled.inc_by(spilled.shards.len() as u64);
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
        Ok((Checkpointed { index: name, pages: listed, seen }, spilled))
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
    /// the last. A failure is logged, and the next attempt waits a whole interval. The caller
    /// holds the drive's checkpoint lock, which `last` is.
    ///
    /// Then puts what the checkpoint spilled into the drive's state, so that the state holds no
    /// more bytes in data extents than the log since this checkpoint has.
    async fn checkpoint(&self, d: &Drive, state: Arc<DriveState>, seen: Duration, last: &mut Option<Checkpointed>) {
        let started = Instant::now();
        let seq = state.seq();
        self.renew(last).await;
        let written = self.write_checkpoint(&d.id, state, seen, last.as_ref()).await;
        self.metrics.checkpoint_write.observe(started.elapsed().as_secs_f64());
        match written {
            Ok((c, spilled)) => {
                self.metrics.checkpoints_written.inc();
                *last = Some(c);
                self.swap_in(d, &spilled).await;
            }
            Err(e) => {
                self.metrics.checkpoints_failed.inc();
                tracing::warn!("checkpoint of {} at {seq} failed: {e:#}", d.id);
            }
        }
    }

    /// Puts the descriptors a checkpoint of `d` spilled into its state in place of those they
    /// replaced, wherever the state still has them (format §8.2). The checkpoint references their
    /// shards from now on, which is what a later commit or checkpoint relies on to reference them
    /// (format §12.4, option 1).
    async fn swap_in(&self, d: &Drive, spilled: &Spilled) {
        if spilled.objects.is_empty() && spilled.versions.is_empty() {
            return;
        }
        let started = Instant::now();
        let _committing = d.commit_lock.lock().await;
        let waited = started.elapsed();
        let next = d.snapshot().with_spilled(spilled);
        *d.state.write().unwrap() = Arc::new(next);
        self.metrics.checkpoint_swap.observe((started.elapsed() - waited).as_secs_f64());
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

    /// A shard, from the cache or else from the bucket, checked against its hash.
    pub async fn shard(&self, h: &ShardHash) -> anyhow::Result<Bytes> {
        self.read(guard::Kind::Shard, h).await
    }

    /// A manifest page or checkpoint segment, from the cache or else from the bucket, checked
    /// against its hash.
    pub async fn page(&self, h: &ShardHash) -> anyhow::Result<Bytes> {
        self.read(guard::Kind::Page, h).await
    }

    /// Reads a shard or page. Reads that miss it while it is being fetched wait for that fetch,
    /// so that the bucket is asked once however many miss it at once; they count as coalesced,
    /// not as misses. The fetch runs on a task of its own: a reader that goes away (a client
    /// that disconnects) leaves it to the others, and if all go it still fills the cache. A
    /// failure reaches every reader waiting, and is not kept: the next read fetches again.
    async fn read(&self, kind: guard::Kind, h: &ShardHash) -> anyhow::Result<Bytes> {
        let (cache, series, fetches) = match kind {
            guard::Kind::Shard => (&self.shards, &self.metrics.shards, &self.shard_fetches),
            guard::Kind::Page => (&self.pages, &self.metrics.pages, &self.page_fetches),
        };
        let fetch = loop {
            if let Some(b) = cache.get(h) {
                series.hits.inc();
                return Ok(b);
            }
            let mut inflight = fetches.lock().unwrap();
            if let Some(f) = inflight.get(h) {
                series.coalesced.inc();
                break f.clone();
            }
            // A fetch puts what it read in the cache before it leaves the map, so one that ended
            // since the miss above has left it there.
            if cache.contains_key(h) {
                continue;
            }
            series.misses.inc();
            let f = self.fetch_one(kind, *h, cache.clone(), fetches.clone());
            inflight.insert(*h, f.clone());
            break f;
        };
        fetch.await.map_err(|e| anyhow!("{e:#}"))
    }

    /// Starts fetching a shard or page for [`Pool::read`]. What it reads must match its hash,
    /// pages too: a checkpoint segment that parses but does not would load a drive in a state it
    /// never had. What it read is recorded as checked (format §12.4, option 2) under the view the
    /// fetch started in, whenever its readers came.
    fn fetch_one(&self, kind: guard::Kind, h: ShardHash, cache: moka::sync::Cache<ShardHash, Bytes>, fetches: Fetches) -> Fetch {
        let (store, guard) = (self.store.clone(), self.guard.clone());
        let generation = self.guard.generation();
        let task = tokio::spawn(async move {
            let _leaving = Leaving { fetches, h };
            let what = match kind {
                guard::Kind::Shard => "shard",
                guard::Kind::Page => "page",
            };
            let b = store.get(&kind.path(&h)).await?.ok_or_else(|| anyhow!("{what} {h} is missing"))?;
            if ShardHash::of(&b) != h {
                bail!("{what} {h} is corrupt");
            }
            guard.confirmed(h, generation);
            let b = cached(&b);
            cache.insert(h, b.clone());
            Ok(b)
        });
        async move { task.await.unwrap_or_else(|e| Err(anyhow!("fetching {h}: {e}"))).map_err(Arc::new) }.boxed().shared()
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

    /// Finds shards a direct upload's commit will reference without this server having uploaded
    /// them (protocol §4.11, format §12.4), each with the length it is listed with. A shard is
    /// held if a request to the bucket that started after the view of `gc/pending.json` was read
    /// finds it, with that length, and the view does not propose it (option 2); with `ask_all`
    /// false, as for a plan, one this server checked recently counts without a request. A
    /// candidate of a waiting run is fetched, checked against its hash and rewritten (option 3).
    /// One a run is deleting is reported missing; with `ask_all`, as for a commit, it fails the
    /// call instead, since it may be gone by the time the commit lands.
    pub async fn find_shards(&self, items: &[(ShardHash, u64)], ask_all: bool) -> anyhow::Result<Vec<Found>> {
        let view = self.guard.view(&self.store, &self.clock).await?;
        let generation = self.guard.generation();
        let mut out = vec![Found::Missing; items.len()];
        let (mut ask, mut rescue) = (Vec::new(), Vec::new());
        for (i, (h, _)) in items.iter().enumerate() {
            match view.proposed(h) {
                Some(Phase::Deleting) if ask_all => bail!("garbage collection is deleting shards this upload needs; try again shortly"),
                Some(Phase::Deleting) => {}
                Some(_) if self.guard.checked(h) && !ask_all => out[i] = Found::Held,
                Some(_) => rescue.push(i),
                None if self.guard.checked(h) && !ask_all => out[i] = Found::Held,
                None => ask.push(i),
            }
        }
        let mut heads = futures::stream::iter(ask).map(|i| async move { (i, self.store.length(&guard::Kind::Shard.path(&items[i].0)).await) }).buffer_unordered(guard::ADMIT_UPLOADS);
        while let Some((i, r)) = heads.next().await {
            let (h, n) = items[i];
            out[i] = match r? {
                Some(len) if len == n => {
                    self.guard.confirmed(h, generation);
                    Found::Held
                }
                Some(len) => Found::Length(len),
                None => Found::Missing,
            };
        }
        drop(heads);
        for i in rescue {
            let (h, n) = items[i];
            let bytes = match self.shard(&h).await {
                Ok(b) => b,
                Err(_) if !self.store.exists(&guard::Kind::Shard.path(&h)).await? => continue,
                Err(e) => return Err(e),
            };
            if bytes.len() as u64 != n {
                out[i] = Found::Length(bytes.len() as u64);
                continue;
            }
            self.guard.admit(&self.store, &self.clock, guard::Kind::Shard, &[(h, bytes)]).await?;
            out[i] = Found::Held;
        }
        Ok(out)
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

    /// Empties the shard and page caches, so that reads go to the bucket as they would after a
    /// restart: an operator sends SIGUSR1 for it, to measure cold reads. Nothing else is
    /// dropped. The drives' states are what the server serves from, not caches, and the guard's
    /// checks only spare writes their uploads. Reads in flight may put back what they fetch.
    pub fn drop_caches(&self) -> Dropped {
        let mut held = [(0, 0); 2];
        for (cache, held) in [&self.shards, &self.pages].into_iter().zip(&mut held) {
            cache.run_pending_tasks();
            *held = (cache.entry_count(), cache.weighted_size());
            cache.invalidate_all();
            cache.run_pending_tasks();
        }
        self.metrics.cache_drops.inc();
        Dropped { shards: held[0].0, shard_bytes: held[0].1, pages: held[1].0, page_bytes: held[1].1 }
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

    /// The full extent list of a content descriptor. Flattened from the pages fetched, as the
    /// cache may already have evicted some of them.
    pub async fn extents(&self, desc: &ContentDescriptor) -> anyhow::Result<Vec<Extent>> {
        voidfs_format::extents(self, desc).await
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
            None => (Arc::new(self.empty_state()), None, None),
            Some(src) => {
                // Hold the source's checkpoint lock so that its last checkpoint is not still
                // being written, and its commit lock so the fork includes every acknowledged
                // write.
                let mut src_last = src.checkpoint.lock().await;
                let _committing = src.commit_lock.lock().await;
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
                let (written, spilled) = self.write_checkpoint(&id, s.clone(), seen, src_last.as_ref()).await?;
                last = Some(written);
                // The fork's own checkpoint references what it spilled; the source's does not.
                let s = Arc::new(s.with_spilled(&spilled));
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
                let checkpointing = d.checkpoint.lock().await;
                Some((checkpointing, d.commit_lock.lock().await))
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
        let (since, queued) = (self.clock.mono(), Instant::now());
        let (answers, start): (Vec<_>, bool) = {
            let mut q = d.queue.lock().unwrap();
            let answers = plans
                .into_iter()
                .map(|plan| {
                    let (reply, answer) = tokio::sync::oneshot::channel();
                    q.push_back(Waiting { plan: Box::new(plan), reply, since, queued });
                    answer
                })
                .collect();
            (answers, !d.draining.swap(true, std::sync::atomic::Ordering::Relaxed))
        };
        d.queued.notify_waiters();
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
    ///
    /// Taken as soon as an entry lands, the next batch would miss the requests that entry has
    /// just answered: their clients send the next ones at once, which then wait for that batch
    /// and then their own. Clients writing together split into two groups that take turns, and
    /// each write takes two round trips. So after an entry lands, the next one waits until as
    /// many mutations have queued since as it answered, up to a quarter of its write time and at
    /// most [`HOLD_MAX`]. That is what those clients take to come back from nearby; one alone
    /// never waits, since its own next request is what the entry waits for. An entry written in
    /// less than 4 ms is not held: a timer takes a tick, 1 ms, to wait at all, and a client
    /// takes longer than a quarter of such an entry to come back.
    ///
    /// Answered clients that do not come back, as when requests arrive at their own pace or do
    /// more between writes, would cost what is waiting the bound each time. So an entry waits
    /// only while, over the last several entries, at least half as many mutations queued within
    /// their bounds as they answered, whether they were held or not ([`Returns`]). One entry
    /// alone would not do: when each answers one or two, a single request that happens to
    /// arrive in time would turn holding on. The commit lock is not held meanwhile, so forks,
    /// hard deletes and a checkpoint's swap get it between entries as before.
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
        // The entry written last, and how many mutations have been seen that queued within its
        // bound.
        let (mut last, mut caught): (Option<Landed>, usize) = (None, 0);
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
            if let Some(l) = &last {
                caught += batch.iter().filter(|w| l.caught(w)).count();
            }
            let (rest, landed) = self.commit_batch(&d, &mut cadence, batch).await;
            drop(cadence);
            {
                let mut q = d.queue.lock().unwrap();
                // Before what did not fit is put back: the batch counted it already.
                if let Some(l) = &last {
                    caught += q.iter().filter(|w| l.caught(w)).count();
                }
                for w in rest.into_iter().rev() {
                    q.push_front(w);
                }
            }
            let pay = {
                let mut r = d.returns.lock().unwrap();
                if let Some(l) = &last {
                    r.add(caught, l.answered);
                }
                r.pay()
            };
            (last, caught) = (landed, 0);
            if let Some(l) = &landed
                && pay
            {
                let started = Instant::now();
                Self::hold(&d, l).await;
                self.metrics.hold.observe(started.elapsed().as_secs_f64());
            }
        }
    }

    /// Waits until as many mutations have queued on `d` since `landed` as it answered, or a
    /// whole batch is waiting, or until its bound, which is a tick or more.
    async fn hold(d: &Drive, landed: &Landed) {
        // A timer fires up to a tick late: set so that it fires by the bound.
        let timer = tokio::time::sleep_until((landed.at + landed.bound - TIMER_TICK).into());
        tokio::pin!(timer);
        loop {
            // Made before looking, so that it hears of a mutation queued after the look.
            let queued = d.queued.notified();
            {
                let q = d.queue.lock().unwrap();
                if q.len() >= BATCH_TXNS || q.iter().filter(|w| w.queued > landed.at).count() >= landed.answered {
                    return;
                }
            }
            tokio::select! {
                _ = queued => {}
                _ = &mut timer => return,
            }
        }
    }

    /// Plans `batch` in order, each transaction against the state the ones before it leave,
    /// writes those that plan as one commit, and answers each mutation. Returns the mutations that
    /// did not fit, to wait for the next commit, and the commit if it was written.
    ///
    /// A mutation is answered only once the commit is written, unless its plan failed against
    /// the drive's state as installed, before any transaction of the batch: a failure may rest
    /// on an earlier transaction that is never written.
    async fn commit_batch(self: &Arc<Self>, d: &Arc<Drive>, cadence: &mut Cadence, batch: Vec<Waiting>) -> (Vec<Waiting>, Option<Landed>) {
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
                return (rest, None);
            }
            let n = txns.len();
            let commit = Commit { format: 1, seq, time, authority: self.authority.clone(), txns };
            let (written, took) = match serde_json::to_vec(&commit) {
                Ok(bytes) => {
                    let len = bytes.len();
                    let started = Instant::now();
                    let created = self.create(&log_path(&d.id, seq), Bytes::from(bytes)).await;
                    let took = started.elapsed();
                    self.metrics.log_write.observe(took.as_secs_f64());
                    (created.map(|created| created.then_some(len)), took)
                }
                Err(e) => (Err(e.into()), Duration::ZERO),
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
                        let (pool, d, seen) = (self.clone(), d.clone(), self.clock.mono());
                        tokio::spawn(async move { pool.checkpoint(&d, state, seen, &mut last).await });
                    }
                    let landed = Landed::for_write(Instant::now(), took, held.len());
                    for (w, answer) in held {
                        let _ = w.reply.send(answer);
                    }
                    return (rest, landed);
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
                    return (rest, None);
                }
                // Whether it was written is not known, so none of it may be reported as done.
                Err(e) => {
                    self.metrics.commits_failed.inc();
                    for (w, _) in held {
                        let _ = w.reply.send(Err(CommitError::Other(anyhow!("writing log entry {seq}: {e:#}"))));
                    }
                    return (rest, None);
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
        let mut state = self.empty_state();
        if let Some(fork) = &d.desc.fork_of {
            // A fork's namespace before its own log exists only as its first checkpoint, so its
            // point-in-time window starts at the fork point (format §8.5, §9).
            state = self.load_checkpoint(&format!("drives/{}/checkpoints/{:020}.json", d.id, fork.seq)).await?.0;
            if state.time().is_some_and(|ct| ct > t) {
                bail!("that instant is before this fork was made");
            }
        }
        for (commit, _) in voidfs_format::commits_after(self, &d.id, state.seq()).await? {
            if commit.time > t {
                break;
            }
            state = state.apply(&commit)?;
        }
        Ok(state)
    }
}

/// The server reads its pool through its caches: pages come from [`Pool::page`], so loading a
/// checkpoint shares them, and each read is checked as format §12.4 asks.
impl voidfs_format::Source for Pool {
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, anyhow::Result<Option<Bytes>>> {
        Box::pin(self.store.get(path))
    }

    fn list<'a>(&'a self, dir: &'a str, after: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<Vec<String>>> {
        Box::pin(self.store.list_files(dir, after))
    }

    fn page<'a>(&'a self, h: &'a ShardHash) -> BoxFuture<'a, anyhow::Result<Bytes>> {
        Box::pin(Pool::page(self, h))
    }
}

/// Adds `feature` to the incompatible features of the pool in `store` (format §3.1), unless it
/// already lists it; returns whether it added it. Readers that do not implement the feature refuse
/// the pool from then on, and servers use it once they have read `voidfs.json` again, when they
/// next start. Members of `voidfs.json` this server does not know are kept.
pub async fn enable_feature(store: &crate::store::Store, feature: &str) -> anyhow::Result<bool> {
    if !KNOWN_INCOMPATIBLE_FEATURES.contains(&feature) {
        bail!("this server does not implement the feature {feature:?}; it knows {KNOWN_INCOMPATIBLE_FEATURES:?}");
    }
    let Some(bytes) = store.get(probe::DESCRIPTOR).await? else {
        bail!("there is no pool here yet (no {}). A server creates one when it starts; --new-pool-feature {feature} creates it with the feature", probe::DESCRIPTOR);
    };
    let desc: PoolDescriptor = serde_json::from_slice(&bytes).context("reading voidfs.json")?;
    desc.check_readable().map_err(|e| anyhow!(e))?;
    if desc.has(feature) {
        return Ok(false);
    }
    let mut json: serde_json::Value = serde_json::from_slice(&bytes)?;
    let features = json.as_object_mut().and_then(|o| o.get_mut("features")).and_then(|f| f.as_object_mut()).ok_or_else(|| anyhow!("voidfs.json has no features object"))?;
    let listed = features.entry("incompatible").or_insert_with(|| serde_json::json!([]));
    listed.as_array_mut().ok_or_else(|| anyhow!("features.incompatible in voidfs.json is not a list"))?.push(feature.into());
    store.put(probe::DESCRIPTOR, Bytes::from(serde_json::to_vec_pretty(&json)?)).await?;
    // Another change made at the same moment would have been lost: check this one stuck.
    let again = store.get(probe::DESCRIPTOR).await?.ok_or_else(|| anyhow!("voidfs.json vanished"))?;
    let again: PoolDescriptor = serde_json::from_slice(&again).context("reading voidfs.json again")?;
    if !again.has(feature) || again.pool_id != desc.pool_id {
        bail!("voidfs.json changed while {feature} was being added; try again");
    }
    Ok(true)
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
    use voidfs_core::state::Rows;

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
            assert_eq!(b.snapshot().find_version(&voidfs_core::names::Key::parse("docs/a.txt").unwrap(), &v1).map(|r| r.size), Some(5));
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
            let (written, _) = pool.write_checkpoint(&d.id, Arc::new(state.clone()), pool.clock.mono(), last.as_ref()).await.unwrap();
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
        let (all, _) = again.write_checkpoint(&d.id, Arc::new(state), again.clock.mono(), None).await.unwrap();
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

    // -----------------------------------------------------------------------------------------
    // Content held in descriptors (format §5, §8.2)

    /// A pool that lists `inline-data`.
    async fn inline_pool(mem: &Arc<MemStore>) -> Arc<Pool> {
        let pool = Pool::open_creating(Store::mem(mem.clone()), 1 << 20, Clock::System, CommitGuard::CreateIfAbsent, &[INLINE_DATA.into()]).await.unwrap();
        assert!(pool.inline_data);
        pool
    }

    /// Content held in its descriptor.
    fn held(data: &[u8]) -> ContentDescriptor {
        ContentDescriptor::Inline { extents: voidfs_core::content::inline(data) }
    }

    /// Rows as a checkpoint lists them: rows equal after this are the same bytes with the same
    /// ETags, wherever the bytes are held.
    fn spilled(rows: &Rows) -> Rows {
        let mut r = rows.clone();
        r.spill();
        r
    }

    fn holds_data(rows: &Rows) -> bool {
        rows.objects.iter().filter_map(|r| r.content.as_ref()).chain(rows.history.iter().filter_map(|r| r.content.as_ref())).any(ContentDescriptor::has_data)
    }

    /// The rows of the checkpoint `c`, as stored.
    async fn stored_rows(pool: &Pool, c: &Checkpointed) -> Rows {
        pool.load_checkpoint(&c.index).await.unwrap().0.rows()
    }

    /// Checkpoints `d` as its commits would, and waits for it.
    async fn checkpoint_now(pool: &Arc<Pool>, d: &Arc<Drive>) {
        let mut last = d.checkpoint.clone().lock_owned().await;
        pool.checkpoint(d, d.snapshot(), pool.clock.mono(), &mut last).await;
    }

    /// A checkpoint stores each distinct small content as a shard and lists shard extents; the
    /// state in memory takes them too. A server that starts again loads the checkpoint, then
    /// replays the log after it, which has data extents again, to the same state.
    #[tokio::test]
    async fn a_checkpoint_spills_data_extents_and_the_state_takes_the_shards() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let d = pool.create_drive("d", None).await.unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        let plans = (0..40).map(|i| put_plan(&format!("f{i}"), &held(format!("file {}", i % 30).as_bytes()), Precondition::default(), &runs)).collect();
        batch(&pool, &d, plans).await;
        pool.commit(&d, put_plan("f0", &held(b"second"), Precondition::default(), &runs)).await.unwrap();
        let before = d.snapshot();
        assert!(holds_data(&before.rows()));
        let shard_puts = count(&mem, |op, path| op == MemOp::Put && path.starts_with("shards/"));
        checkpoint_now(&pool, &d).await;
        assert_eq!(shard_puts.load(Ordering::SeqCst), 31, "each distinct content once");
        assert_eq!(pool.metrics.checkpoint_spilled.get(), 31);
        for i in 0..30 {
            assert!(mem.peek(&format!("shards/{}", ShardHash::of(format!("file {i}").as_bytes()).object_path())).is_some());
        }
        let last = d.checkpoint.lock().await.as_ref().map(|c| c.index.clone()).unwrap();
        let stored = pool.load_checkpoint(&last).await.unwrap().0.rows();
        assert!(!holds_data(&stored), "a checkpoint never carries data extents");
        assert_eq!(stored, spilled(&before.rows()));
        let now = d.snapshot();
        assert_eq!(now.seq(), before.seq());
        assert_eq!(now.rows(), stored, "the state took what the checkpoint spilled");
        assert_eq!(pool.metrics.checkpoint_swap.get_sample_count(), 1);
        // More small content, in the log after the checkpoint.
        pool.commit(&d, put_plan("f1", &held(b"after"), Precondition::default(), &runs)).await.unwrap();
        mem.set_hook(None);
        let again = Pool::open(pool.store.clone(), 1 << 20).await.unwrap();
        let reloaded = again.drive("d").unwrap();
        assert_eq!(reloaded.snapshot().rows(), d.snapshot().rows(), "checkpoint, then the log");
        assert!(holds_data(&reloaded.snapshot().rows()));
        for (key, data) in [("f0", &b"second"[..]), ("f1", b"after"), ("f29", b"file 29"), ("f35", b"file 5")] {
            assert_eq!(read(&again, &reloaded, key).await.unwrap(), data, "{key}");
        }
        let oid = before.lookup(&Key::parse("f0").unwrap()).unwrap();
        let snap = reloaded.snapshot();
        let first = &snap.history(&oid).unwrap()[0];
        assert_eq!((first.etag.clone(), first.content.clone()), (held(b"file 0").etag(), Some(spilled_desc(&held(b"file 0")))));
    }

    fn spilled_desc(c: &ContentDescriptor) -> ContentDescriptor {
        let ContentDescriptor::Inline { extents } = c else { panic!() };
        ContentDescriptor::Inline { extents: voidfs_core::content::spill(extents).extents }
    }

    /// What a checkpoint spilled goes into the state as it is when the checkpoint is written,
    /// not as it was when it began: commits made meanwhile stay, and a file they changed keeps
    /// its new content.
    #[tokio::test]
    async fn the_state_takes_what_was_spilled_only_where_it_is_unchanged() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let d = pool.create_drive("d", None).await.unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        for (key, data) in [("a", &b"alpha"[..]), ("b", b"beta")] {
            pool.commit(&d, put_plan(key, &held(data), Precondition::default(), &runs)).await.unwrap();
        }
        let h = hold(&mem, index_puts);
        let checkpointing = tokio::spawn({
            let (pool, d) = (pool.clone(), d.clone());
            async move { checkpoint_now(&pool, &d).await }
        });
        reached(&h.held, 1).await;
        // Meanwhile `b` changes, and `c` is new.
        pool.commit(&d, put_plan("b", &held(b"beta 2"), Precondition::default(), &runs)).await.unwrap();
        pool.commit(&d, put_plan("c", &held(b"gamma"), Precondition::default(), &runs)).await.unwrap();
        let seq = d.snapshot().seq();
        h.release.send(true).unwrap();
        checkpointing.await.unwrap();
        let s = d.snapshot();
        assert_eq!(s.seq(), seq, "the commits made meanwhile are still there");
        let content = |key: &str| s.record(&s.lookup(&Key::parse(key).unwrap()).unwrap()).unwrap().content.clone().unwrap();
        assert_eq!(content("a"), spilled_desc(&held(b"alpha")));
        assert_eq!(content("b"), held(b"beta 2"));
        assert_eq!(content("c"), held(b"gamma"));
        let b = s.lookup(&Key::parse("b").unwrap()).unwrap();
        let versions: Vec<_> = s.history(&b).unwrap().iter().map(|r| r.content.clone().unwrap()).collect();
        assert_eq!(versions, [spilled_desc(&held(b"beta")), held(b"beta 2")]);
        mem.set_hook(None);
        let again = Pool::open(pool.store.clone(), 1 << 20).await.unwrap();
        assert_eq!(again.drive("d").unwrap().snapshot().rows(), s.rows());
    }

    /// A checkpoint that spills many shards keeps a bounded number of uploads in flight.
    #[tokio::test]
    async fn a_checkpoint_spills_a_bounded_number_at_once() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let d = pool.create_drive("d", None).await.unwrap();
        let plans = (0..200).map(|i| put_plan(&format!("f{i}"), &held(format!("{i}").as_bytes()), Precondition::default(), &Arc::default())).collect();
        batch(&pool, &d, plans).await;
        let (now, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (n, m) = (now.clone(), most.clone());
        mem.set_hook(Some(Arc::new(move |op, path| {
            if !(op == MemOp::Put && path.starts_with("shards/")) {
                return futures::future::ready(Fault::None).boxed();
            }
            let (n, m) = (n.clone(), m.clone());
            async move {
                m.fetch_max(n.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(2)).await;
                n.fetch_sub(1, Ordering::SeqCst);
                Fault::None
            }
            .boxed()
        })));
        checkpoint_now(&pool, &d).await;
        assert_eq!(pool.metrics.checkpoint_spilled.get(), 200);
        assert_eq!(most.load(Ordering::SeqCst), guard::ADMIT_UPLOADS);
    }

    /// A fork's first checkpoint spills too, and the fork's state takes what it spilled. Its
    /// source's state does not: only a drive's own checkpoints vouch for its content.
    #[tokio::test]
    async fn a_forks_first_checkpoint_spills() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let src = pool.create_drive("src", None).await.unwrap();
        pool.commit(&src, put_plan("a", &held(b"shared"), Precondition::default(), &Arc::default())).await.unwrap();
        let fork = pool.create_drive("fork", Some(&src)).await.unwrap();
        assert!(holds_data(&src.snapshot().rows()));
        assert!(!holds_data(&fork.snapshot().rows()));
        let stored = stored_rows(&pool, fork.checkpoint.lock().await.as_ref().unwrap()).await;
        assert_eq!(stored, fork.snapshot().rows());
        assert_eq!(spilled(&src.snapshot().rows()), stored);
        assert_eq!(read(&pool, &fork, "a").await.unwrap(), b"shared");
    }

    /// A checkpoint that has spilled takes the commit lock to swap, while forks and hard deletes
    /// wait for it: they take the checkpoint lock first, so neither waits for the other in turn.
    #[tokio::test]
    async fn forks_and_hard_deletes_wait_for_a_checkpoint_that_spills() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let d = pool.create_drive("d", None).await.unwrap();
        pool.commit(&d, put_plan("a", &held(b"small"), Precondition::default(), &Arc::default())).await.unwrap();
        let h = hold(&mem, index_puts);
        let checkpointing = tokio::spawn({
            let (pool, d) = (pool.clone(), d.clone());
            async move { checkpoint_now(&pool, &d).await }
        });
        reached(&h.held, 1).await;
        let forking = tokio::spawn({
            let (pool, d) = (pool.clone(), d.clone());
            async move { pool.create_drive("fork", Some(&d)).await.unwrap() }
        });
        // Commits go on while the fork waits for the checkpoint.
        tokio::time::timeout(Duration::from_secs(10), pool.commit(&d, put_plan("b", &held(b"more"), Precondition::default(), &Arc::default()))).await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!forking.is_finished());
        h.release.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(10), checkpointing).await.expect("no deadlock").unwrap();
        let fork = tokio::time::timeout(Duration::from_secs(10), forking).await.expect("no deadlock").unwrap();
        assert_eq!(read(&pool, &fork, "b").await.unwrap(), b"more");
        // Likewise a hard delete.
        let h = hold(&mem, index_puts);
        let checkpointing = tokio::spawn({
            let (pool, d) = (pool.clone(), d.clone());
            async move { checkpoint_now(&pool, &d).await }
        });
        reached(&h.held, 1).await;
        let deleting = tokio::spawn({
            let pool = pool.clone();
            async move { pool.hard_delete("d").await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!deleting.is_finished());
        h.release.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(10), checkpointing).await.expect("no deadlock").unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(10), deleting).await.expect("no deadlock").unwrap());
    }

    /// A new pool lists the features a server is started with; an existing one keeps its own,
    /// and has one added only by [`enable_feature`], which keeps what it does not know of
    /// `voidfs.json`. A pool listing a feature this server does not implement is refused.
    #[tokio::test]
    async fn features_are_added_to_new_pools_or_enabled() {
        let store = Store::memory().unwrap();
        assert!(format!("{:#}", enable_feature(&store, INLINE_DATA).await.unwrap_err()).contains("no pool here yet"));
        let open = |features: Vec<String>| {
            let store = store.clone();
            async move { Pool::open_creating(store, 1 << 20, Clock::System, CommitGuard::CreateIfAbsent, &features).await }
        };
        assert!(open(vec!["shard-zstd".into()]).await.is_err(), "not implemented");
        assert!(store.get(probe::DESCRIPTOR).await.unwrap().is_none());
        assert!(!open(vec![]).await.unwrap().inline_data);
        assert!(!open(vec![INLINE_DATA.into()]).await.unwrap().inline_data, "not a new pool");
        let mut json: serde_json::Value = serde_json::from_slice(&store.get(probe::DESCRIPTOR).await.unwrap().unwrap()).unwrap();
        json["future_member"] = serde_json::json!({ "kept": true });
        store.put(probe::DESCRIPTOR, Bytes::from(serde_json::to_vec(&json).unwrap())).await.unwrap();
        assert!(enable_feature(&store, "shard-zstd").await.is_err());
        assert!(enable_feature(&store, INLINE_DATA).await.unwrap());
        assert!(!enable_feature(&store, INLINE_DATA).await.unwrap(), "already");
        let json: serde_json::Value = serde_json::from_slice(&store.get(probe::DESCRIPTOR).await.unwrap().unwrap()).unwrap();
        assert_eq!(json["features"]["incompatible"], serde_json::json!([INLINE_DATA]));
        assert_eq!(json["future_member"], serde_json::json!({ "kept": true }));
        assert!(open(vec![]).await.unwrap().inline_data);
        let fresh = Store::memory().unwrap();
        let pool = Pool::open_creating(fresh.clone(), 1 << 20, Clock::System, CommitGuard::CreateIfAbsent, &[INLINE_DATA.into()]).await.unwrap();
        assert!(pool.inline_data);
        let desc: PoolDescriptor = serde_json::from_slice(&fresh.get(probe::DESCRIPTOR).await.unwrap().unwrap()).unwrap();
        assert_eq!(desc.features.incompatible, [INLINE_DATA]);
        // A pool with a feature this server does not implement: refused, as an older server
        // refuses one with inline-data.
        let mut json: serde_json::Value = serde_json::from_slice(&fresh.get(probe::DESCRIPTOR).await.unwrap().unwrap()).unwrap();
        json["features"]["incompatible"] = serde_json::json!([INLINE_DATA, "encryption"]);
        fresh.put(probe::DESCRIPTOR, Bytes::from(serde_json::to_vec(&json).unwrap())).await.unwrap();
        let e = Pool::open(fresh.clone(), 1 << 20).await.err().unwrap();
        assert!(format!("{e:#}").contains("unsupported features"), "{e:#}");
        assert!(enable_feature(&fresh, INLINE_DATA).await.is_err(), "nor changed");
    }

    fn restore_plan(key: &str, then: Arc<DriveState>) -> TestPlan {
        let key = key.to_owned();
        Box::new(move |s| Ok(ops::restore_subtree(s, &then, &key, &Actor::system())?))
    }

    /// The versions in the history of the object at `key`.
    fn versions(s: &DriveState, key: &str) -> Vec<VersionId> {
        s.history(&s.lookup(&Key::parse(key).unwrap()).unwrap()).unwrap().iter().map(|r| r.version).collect()
    }

    /// RFC 0004: once `multi-object-versions` is enabled, a server replays the log under its rule,
    /// so a folder restore made before is in the restored file's history; history a checkpoint
    /// recorded before stays as it was. The feed reports what a version took out, its target,
    /// then the rest.
    #[tokio::test]
    async fn multi_object_versions_apply_to_the_log_replayed_once_enabled() {
        let store = Store::memory().unwrap();
        let pool = Pool::open_creating(store.clone(), 1 << 20, Clock::System, CommitGuard::CreateIfAbsent, &[]).await.unwrap();
        assert!(!pool.multi_object_versions);
        let d = pool.create_drive("d", None).await.unwrap();
        put(&pool, &d, "c/a", b"1").await;
        let then = d.snapshot();
        put(&pool, &d, "c/a", b"2").await;
        let in_checkpoint = pool.commit(&d, restore_plan("c/", then)).await.unwrap().0;
        checkpoint_now(&pool, &d).await;
        put(&pool, &d, "l/a", b"1").await;
        let then = d.snapshot();
        put(&pool, &d, "l/a", b"2").await;
        let in_log = pool.commit(&d, restore_plan("l/", then)).await.unwrap().0;
        assert!(!versions(&d.snapshot(), "l/a").contains(&in_log), "the folder's version alone");
        drop((d, pool));

        assert!(enable_feature(&store, MULTI_OBJECT_VERSIONS).await.unwrap());
        let pool = Pool::open(store.clone(), 1 << 20).await.unwrap();
        assert!(pool.multi_object_versions);
        let d = pool.drive("d").unwrap();
        let s = d.snapshot();
        assert_eq!(versions(&s, "l/a").last(), Some(&in_log), "replayed under the rule");
        assert_eq!(s.record(&s.lookup(&Key::parse("l/a").unwrap()).unwrap()).unwrap().head, in_log);
        assert!(!versions(&s, "c/a").contains(&in_checkpoint), "as the checkpoint recorded it");

        for key in ["f/a", "f/b", "f/c"] {
            put(&pool, &d, key, b"1").await;
        }
        let then = d.snapshot();
        put(&pool, &d, "f/a", b"2").await;
        for key in ["f/b", "f/c"] {
            pool.commit(&d, delete_plan(key)).await.unwrap();
        }
        put(&pool, &d, "f/n/x", b"x").await;
        put(&pool, &d, "f/z", b"z").await;
        let since = d.snapshot().seq();
        let v = pool.commit(&d, restore_plan("f/", then)).await.unwrap().0;
        let feed = d.changes_since(since).unwrap();
        let got: Vec<(&str, &str)> = feed[0].changes.iter().map(|c| (c.op, c.key.as_str())).collect();
        // The planner names them in another order: what it takes out as it walks the folder, and
        // what it brings back after what it rolls back.
        assert_eq!(got, [
            ("delete", "f/z"),
            ("delete", "f/n/x"),
            ("delete", "f/n/"),
            ("restore", "f/"),
            ("restore", "f/a"),
            ("restore", "f/b"),
            ("restore", "f/c"),
        ]);
        assert!(feed[0].changes.iter().all(|c| c.version_id == v));
        let e = pool.create_drive("e", None).await.unwrap();
        let v = put(&pool, &e, "a/b", b"b").await;
        assert_eq!(versions(&e.snapshot(), "a/"), [v], "a new drive under the rule");
    }

    /// One task drains a drive's queue at a time, so whatever else waits for the commit lock (a
    /// fork, a hard delete, a checkpoint putting what it spilled into the state) waits for the
    /// batch being written, not for a line of tasks that grows with every mutation. With a task
    /// per mutation, under eight clients writing steadily, it waited 1.5 s, then 9, then 56.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn steady_commits_do_not_starve_the_commit_lock() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let d = pool.create_drive("d", None).await.unwrap();
        let one = held(b"one");
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
        // A checkpoint that spills puts what it spilled into the state meanwhile.
        tokio::time::timeout(Duration::from_secs(5), checkpoint_now(&pool, &d)).await.expect("the checkpoint's swap is not starved");
        assert_eq!(pool.metrics.checkpoint_swap.get_sample_count(), 1);
        stop.store(true, Ordering::SeqCst);
        let mut done = 0;
        for c in clients {
            done += c.await.unwrap();
        }
        assert!(done > 100, "only {done} commits");
    }

    // -----------------------------------------------------------------------------------------
    // Holding a log entry for the requests the one before it answered

    /// Log entries written through [`slow_log`].
    #[derive(Default)]
    struct LogWrites {
        begun: AtomicUsize,
        /// When each began and ended.
        done: std::sync::Mutex<Vec<(Instant, Instant)>>,
    }

    /// Makes each log entry written to `mem` take `delay`.
    fn slow_log(mem: &MemStore, delay: Duration) -> Arc<LogWrites> {
        let writes = Arc::new(LogWrites::default());
        let w = writes.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            if !(op == MemOp::PutNew && path.contains("/log/")) {
                return futures::future::ready(Fault::None).boxed();
            }
            let w = w.clone();
            async move {
                let began = Instant::now();
                w.begun.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                w.done.lock().unwrap().push((began, Instant::now()));
                Fault::None
            }
            .boxed()
        })));
        writes
    }

    /// What a client takes to send its next request once answered: `t`, which a timer would
    /// stretch to a millisecond.
    async fn think(t: Duration) {
        let started = Instant::now();
        while started.elapsed() < t {
            tokio::task::yield_now().await;
        }
    }

    /// Clients that write again as soon as they are answered share log entries: each entry waits
    /// for the requests the one before it answered. Otherwise they split into two groups that take
    /// turns, each write takes two entries, and the ten rounds here take twenty. A drive that had
    /// stopped holding, because the clients of its last entry did not come back, starts again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn clients_writing_together_share_log_entries() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let one = held(b"one");
        let writes = slow_log(&mem, Duration::from_millis(20));
        let (clients, rounds) = (8, 10);
        for (i, stopped) in [false, true].into_iter().enumerate() {
            let d = pool.create_drive(&format!("d{i}"), None).await.unwrap();
            if stopped {
                d.returns.lock().unwrap().add(0, clients);
                assert!(!d.returns.lock().unwrap().pay());
            }
            let before = writes.begun.load(Ordering::SeqCst);
            let tasks: Vec<_> = (0..clients)
                .map(|c| {
                    let (pool, d, one) = (pool.clone(), d.clone(), one.clone());
                    tokio::spawn(async move {
                        for r in 0..rounds {
                            pool.commit(&d, put_plan(&format!("c{c}-{r}"), &one, Precondition::default(), &Arc::default())).await.unwrap();
                            think(Duration::from_micros(300)).await;
                        }
                    })
                })
                .collect();
            for t in tasks {
                t.await.unwrap();
            }
            let entries = writes.begun.load(Ordering::SeqCst) - before;
            assert!(entries <= rounds + rounds / 2, "stopped at first: {stopped}. {entries} log entries for {rounds} rounds of {clients} writes");
        }
        assert!(pool.metrics.hold.get_sample_count() >= 2 * rounds as u64 - 4);
    }

    /// An entry written in less than 4 ms is not held: a timer would take a tick, more than a
    /// quarter of the entry.
    #[test]
    fn quick_entries_are_not_held() {
        let at = Instant::now();
        for took in [Duration::ZERO, Duration::from_nanos(1), Duration::from_millis(1), Duration::from_nanos(3_999_999)] {
            assert!(Landed::for_write(at, took, 4).is_none(), "an entry written in {took:?} cannot pay for a timer tick");
        }
        for (took, bound) in [(Duration::from_millis(4), Duration::from_millis(1)),
            (Duration::from_millis(6), Duration::from_micros(1500)), (Duration::from_millis(8), HOLD_MAX),
            (Duration::from_secs(1), HOLD_MAX)] {
            let landed = Landed::for_write(at, took, 4).expect("a timer tick fits within a quarter of the write");
            assert_eq!((landed.at, landed.bound, landed.answered), (at, bound, 4));
        }
    }

    /// A client writing alone never waits for a hold: the entry after its last one waits for its
    /// next write, which comes, and for nothing else.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_client_alone_never_waits() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let d = pool.create_drive("d", None).await.unwrap();
        let one = held(b"one");
        let writes = slow_log(&mem, Duration::from_millis(20));
        let mut waited = Vec::new();
        for i in 0..20 {
            let sent = Instant::now();
            pool.commit(&d, put_plan(&format!("k{i}"), &one, Precondition::default(), &Arc::default())).await.unwrap();
            waited.push(writes.done.lock().unwrap().last().unwrap().0 - sent);
            think(Duration::from_micros(300)).await;
        }
        // Held for more, each write would wait a millisecond at least. A busy machine delays a few
        // of them as much, so the middle one is what counts.
        waited.sort();
        assert!(waited[10] < Duration::from_millis(1), "waits before its entries were written: {waited:?}");
    }

    /// When the clients an entry answered do not come back, the next entry waits at most the
    /// bound, and then entries stop waiting: requests that arrive at their own pace do not pay
    /// for it each time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_hold_is_bounded_when_nobody_comes_back() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = inline_pool(&mem).await;
        let d = pool.create_drive("d", None).await.unwrap();
        let one = held(b"one");
        let writes = slow_log(&mem, Duration::from_millis(100));
        // One put an entry, each queued while the entry before it is written, after that entry's
        // bound; no client writes again.
        let puts = async {
            let mut tasks = Vec::new();
            for i in 0..7 {
                if i > 0 {
                    reached(&writes.begun, i).await;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let (pool, d, one) = (pool.clone(), d.clone(), one.clone());
                tasks.push(tokio::spawn(async move { pool.commit(&d, put_plan(&format!("k{i}"), &one, Precondition::default(), &Arc::default())).await.unwrap() }));
            }
            for t in tasks {
                t.await.unwrap();
            }
        };
        tokio::time::timeout(Duration::from_secs(10), puts).await.expect("the hold is bounded");
        let done = writes.done.lock().unwrap().clone();
        assert_eq!(done.len(), 7);
        let gaps: Vec<Duration> = done.windows(2).map(|w| w[1].0 - w[0].1).collect();
        assert!((HOLD_MAX - TIMER_TICK..HOLD_MAX + Duration::from_millis(10)).contains(&gaps[0]), "the first entry waits, at most the bound: {gaps:?}");
        assert!(gaps[1..].iter().sum::<Duration>() < Duration::from_millis(3), "the next ones do not: {gaps:?}");
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
        read_result(pool, kind, h).await.unwrap()
    }

    async fn read_result(pool: &Pool, kind: guard::Kind, h: &ShardHash) -> anyhow::Result<Bytes> {
        match kind {
            guard::Kind::Shard => pool.shard(h).await,
            guard::Kind::Page => pool.page(h).await,
        }
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

    /// Requests to a [`MemStore`] held back by [`gate`] until it opens, which says how they end.
    struct Gate {
        open: tokio::sync::watch::Sender<Option<Fault>>,
        /// Requests held so far.
        held: Arc<AtomicUsize>,
    }

    impl Gate {
        fn open(&self, fault: Fault) {
            self.open.send_replace(Some(fault));
        }
    }

    /// Holds the requests to `mem` that `which` picks until the gate opens.
    fn gate(mem: &MemStore, which: impl Fn(MemOp, &str) -> bool + Send + Sync + 'static) -> Gate {
        let (open, opened) = tokio::sync::watch::channel(None);
        let held = Arc::new(AtomicUsize::new(0));
        let h = held.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            if !which(op, path) {
                return futures::future::ready(Fault::None).boxed();
            }
            h.fetch_add(1, Ordering::SeqCst);
            let mut opened = opened.clone();
            async move { opened.wait_for(Option::is_some).await.map(|f| f.unwrap()).unwrap_or(Fault::None) }.boxed()
        })));
        Gate { open, held }
    }

    fn object_gets(op: MemOp, path: &str) -> bool {
        op == MemOp::Get && (path.starts_with("shards/") || path.starts_with("pages/"))
    }

    /// Waits until `done` says so, and fails the test after five seconds rather than hang.
    async fn until(what: &str, done: impl Fn() -> bool) {
        let waited = tokio::time::timeout(Duration::from_secs(5), async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        waited.await.unwrap_or_else(|_| panic!("waited five seconds for {what}"));
    }

    /// `n` reads of `h` at once, each on a task of its own.
    fn readers(pool: &Arc<Pool>, kind: guard::Kind, h: ShardHash, n: usize) -> Vec<tokio::task::JoinHandle<anyhow::Result<Bytes>>> {
        (0..n)
            .map(|_| {
                let pool = pool.clone();
                tokio::spawn(async move {
                    match kind {
                        guard::Kind::Shard => pool.shard(&h).await,
                        guard::Kind::Page => pool.page(&h).await,
                    }
                })
            })
            .collect()
    }

    fn cache_of(pool: &Pool, kind: guard::Kind) -> (&moka::sync::Cache<ShardHash, Bytes>, &crate::metrics::CacheMetrics) {
        match kind {
            guard::Kind::Shard => (&pool.shards, &pool.metrics.shards),
            guard::Kind::Page => (&pool.pages, &pool.metrics.pages),
        }
    }

    /// However many reads miss a shard or page at once, the bucket is asked for it once, and
    /// every read gets its bytes. The reads that waited count as coalesced, not as misses.
    #[tokio::test]
    async fn reads_that_miss_together_share_one_fetch() {
        use crate::metrics::{encode, sample};
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        for kind in [guard::Kind::Shard, guard::Kind::Page] {
            let (cache, series) = cache_of(&pool, kind);
            let [(h, b)] = &blobs(&format!("{kind:?}"), 1, 64 << 10)[..] else { unreachable!() };
            pool.store.put(&kind.path(h), b.clone()).await.unwrap();
            let gets = gate(&mem, object_gets);
            let reads = readers(&pool, kind, *h, 8);
            until("eight reads", || series.misses.get() + series.coalesced.get() == 8).await;
            gets.open(Fault::None);
            for r in reads {
                assert_eq!(&r.await.unwrap().unwrap(), b, "{kind:?}");
            }
            assert_eq!(gets.held.load(Ordering::SeqCst), 1, "{kind:?}: one request to the bucket");
            assert_eq!((series.hits.get(), series.misses.get(), series.coalesced.get()), (0, 1, 7), "{kind:?}");
            assert!(cache.contains_key(h), "{kind:?}");
            assert_eq!(&read_as(&pool, kind, h).await, b);
            assert_eq!(gets.held.load(Ordering::SeqCst), 1, "{kind:?}: then from memory");
            assert_eq!(series.hits.get(), 1);
        }
        let text = encode(pool.gather_metrics());
        assert_eq!(sample(&text, r#"voidfs_cache_coalesced_total{cache="shard"}"#), Some(7.0));
        assert_eq!(sample(&text, r#"voidfs_cache_coalesced_total{cache="page"}"#), Some(7.0));
    }

    /// A failed fetch fails every read waiting for it, and is not kept: the next read asks the
    /// bucket again.
    #[tokio::test]
    async fn a_failed_fetch_reaches_every_reader_and_the_next_read_tries_again() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        for kind in [guard::Kind::Shard, guard::Kind::Page] {
            let (cache, series) = cache_of(&pool, kind);
            let [(h, b)] = &blobs(&format!("{kind:?}"), 1, 64 << 10)[..] else { unreachable!() };
            pool.store.put(&kind.path(h), b.clone()).await.unwrap();
            let gets = gate(&mem, object_gets);
            let reads = readers(&pool, kind, *h, 8);
            until("eight reads", || series.misses.get() + series.coalesced.get() == 8).await;
            gets.open(Fault::Fail);
            for r in reads {
                let e = r.await.unwrap().unwrap_err();
                assert!(format!("{e:#}").contains("injected failure"), "{kind:?}: {e:#}");
            }
            assert_eq!(gets.held.load(Ordering::SeqCst), 1, "{kind:?}: one request to the bucket");
            assert!(!cache.contains_key(h), "{kind:?}: nothing kept");
            let gets = count(&mem, object_gets);
            assert_eq!(&read_as(&pool, kind, h).await, b, "{kind:?}");
            assert_eq!(gets.load(Ordering::SeqCst), 1, "{kind:?}: the next read asks again");
            assert_eq!((series.misses.get(), series.coalesced.get()), (2, 7), "{kind:?}");
        }
    }

    /// The read that started a fetch can go away, as when its client disconnects, without
    /// failing or cancelling the reads waiting for the same fetch. With every reader gone, the
    /// fetch still ends and fills the cache.
    #[tokio::test]
    async fn a_reader_that_goes_away_leaves_the_fetch_to_the_others() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let [(h, b), (lone, lone_b)] = &blobs("shard", 2, 64 << 10)[..] else { unreachable!() };
        for (h, b) in [(h, b), (lone, lone_b)] {
            pool.store.put(&guard::Kind::Shard.path(h), b.clone()).await.unwrap();
        }
        let series = &pool.metrics.shards;
        let gets = gate(&mem, object_gets);
        let first = readers(&pool, guard::Kind::Shard, *h, 1).pop().unwrap();
        until("the first read's fetch", || gets.held.load(Ordering::SeqCst) == 1).await;
        let others = readers(&pool, guard::Kind::Shard, *h, 7);
        until("seven more reads", || series.coalesced.get() == 7).await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        gets.open(Fault::None);
        for r in others {
            assert_eq!(&r.await.unwrap().unwrap(), b);
        }
        assert_eq!(gets.held.load(Ordering::SeqCst), 1);

        let gets = gate(&mem, object_gets);
        let reads = readers(&pool, guard::Kind::Shard, *lone, 3);
        until("three reads", || gets.held.load(Ordering::SeqCst) == 1 && series.coalesced.get() == 9).await;
        for r in reads {
            r.abort();
            assert!(r.await.unwrap_err().is_cancelled());
        }
        gets.open(Fault::None);
        until("the fetch to fill the cache", || pool.shards.contains_key(lone)).await;
        assert_eq!(&pool.shard(lone).await.unwrap(), lone_b);
        assert_eq!(gets.held.load(Ordering::SeqCst), 1, "fetched once, by no reader in the end");
    }

    /// A shard or page whose bytes do not match its hash is refused to every read waiting for it.
    /// It is neither cached nor taken as stored: the next read asks the bucket again, and a write
    /// of it uploads it.
    #[tokio::test]
    async fn a_corrupt_shard_or_page_is_refused_to_every_reader() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        for (kind, what) in [(guard::Kind::Shard, "shard"), (guard::Kind::Page, "page")] {
            let (cache, series) = cache_of(&pool, kind);
            let [(h, b)] = &blobs(what, 1, 64 << 10)[..] else { unreachable!() };
            let mut bad = b.to_vec();
            bad[100] ^= 1;
            pool.store.put(&kind.path(h), Bytes::from(bad)).await.unwrap();
            let gets = gate(&mem, object_gets);
            let reads = readers(&pool, kind, *h, 8);
            until("eight reads", || series.misses.get() + series.coalesced.get() == 8).await;
            gets.open(Fault::None);
            for r in reads {
                let e = r.await.unwrap().unwrap_err();
                assert!(format!("{e:#}").contains(&format!("{what} {h} is corrupt")), "{e:#}");
            }
            assert_eq!(gets.held.load(Ordering::SeqCst), 1, "{what}");
            assert!(!cache.contains_key(h), "{what}");
            let gets = count(&mem, object_gets);
            assert!(read_result(&pool, kind, h).await.is_err(), "{what}");
            assert_eq!(gets.load(Ordering::SeqCst), 1, "{what}: asked again, and refused again");
            let dir = format!("{what}s/");
            let puts = count(&mem, move |op, path| op == MemOp::Put && path.starts_with(&dir));
            write_as(&pool, kind, &[(*h, b.clone())]).await;
            assert_eq!(puts.load(Ordering::SeqCst), 1, "{what}: not taken as stored, so uploaded");
            assert_eq!(&read_as(&pool, kind, h).await, b);
        }
    }

    /// A checkpoint segment that still parses, but is not what was written, is refused: the drive
    /// is not loaded in a state it never had.
    #[tokio::test]
    async fn a_checkpoint_with_a_corrupt_segment_is_not_loaded() {
        let (mem, pool, d, _) = big_drive(300, Clock::System).await;
        let index = format!("drives/{}/checkpoints/{:020}.json", d.id, checkpoints(&pool.store, &d).await[0]);
        let idx: CheckpointIndex = serde_json::from_slice(&pool.store.get(&index).await.unwrap().unwrap()).unwrap();
        let seg = idx.tables.entries[0].page;
        let path = guard::Kind::Page.path(&seg);
        let text = String::from_utf8(mem.peek(&path).unwrap().to_vec()).unwrap();
        let changed = text.replacen("\"f000000\"", "\"f000001\"", 1);
        assert_ne!(changed, text);
        serde_json::from_str::<voidfs_format::Segment<serde_json::Value>>(&changed).expect("the segment still parses");
        pool.store.put(&path, Bytes::from(changed)).await.unwrap();
        let fresh = Pool::open(pool.store.clone(), 64 << 20).await.unwrap();
        assert!(fresh.drive("big").is_none(), "the drive is not served from a corrupt checkpoint");
        let Err(e) = fresh.load_checkpoint(&index).await else { panic!("a corrupt segment loaded") };
        assert!(format!("{e:#}").contains(&format!("page {seg} is corrupt")), "{e:#}");
    }

    /// A fetch records what it read as checked under the view of garbage collection it started
    /// in (format §12.4, option 2), whoever waited for it: if the view changed while it was in
    /// flight, a write of the shard still uploads it.
    #[tokio::test]
    async fn a_fetch_is_checked_under_the_view_it_started_in() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let [(steady, steady_b), (moved, moved_b)] = &blobs("shard", 2, 64 << 10)[..] else { unreachable!() };
        for (h, b) in [(steady, steady_b), (moved, moved_b)] {
            pool.store.put(&guard::Kind::Shard.path(h), b.clone()).await.unwrap();
        }
        let puts = |mem: &MemStore| count(mem, |op, path| op == MemOp::Put && path.starts_with("shards/"));
        // The view stays: what was read is not uploaded again.
        assert_eq!(&pool.shard(steady).await.unwrap(), steady_b);
        let n = puts(&mem);
        write_as(&pool, guard::Kind::Shard, &[(*steady, steady_b.clone())]).await;
        assert_eq!(n.load(Ordering::SeqCst), 0, "read under an unchanged view: not uploaded");

        // A run starts while the fetch is in flight, and a second read joins it after.
        let gets = gate(&mem, object_gets);
        let first = readers(&pool, guard::Kind::Shard, *moved, 1);
        until("the fetch", || gets.held.load(Ordering::SeqCst) == 1).await;
        let run = crate::gc::PendingRecord { format: 1, run: "r-1".into(), phase: crate::gc::Phase::Marking, t1: None, grace: 0, candidates: vec![] };
        pool.store.put(crate::gc::PENDING, Bytes::from(serde_json::to_vec(&run).unwrap())).await.unwrap();
        pool.guard.refresh(&pool.store, &pool.clock).await.unwrap();
        let second = readers(&pool, guard::Kind::Shard, *moved, 1);
        until("the second read", || pool.metrics.shards.coalesced.get() == 1).await;
        gets.open(Fault::None);
        for r in first.into_iter().chain(second) {
            assert_eq!(&r.await.unwrap().unwrap(), moved_b);
        }
        let n = puts(&mem);
        write_as(&pool, guard::Kind::Shard, &[(*moved, moved_b.clone())]).await;
        assert_eq!(n.load(Ordering::SeqCst), 1, "read across a change of view: uploaded");
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

    /// Dropping the caches empties both, and only them: reads then fetch every shard and page
    /// once, and return the same bytes. The drive's state stays, and so do the guard's checks.
    #[tokio::test]
    async fn dropping_the_caches_empties_them_and_reads_come_back_the_same() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        let d = pool.create_drive("cold", None).await.unwrap();
        // More extents than a descriptor holds, so that the file has manifest pages too.
        let pieces = blobs("piece", 1_100, 16);
        write_as(&pool, guard::Kind::Shard, &pieces).await;
        let desc = pool.describe(pieces.iter().map(|(h, b)| Extent::Shard { s: *h, n: b.len() as u64 }).collect()).await.unwrap();
        assert!(matches!(desc, ContentDescriptor::Tree { .. }));
        pool.commit(&d, put_plan("big", &desc, Precondition::default(), &Arc::default())).await.unwrap();
        put(&pool, &d, "small", b"hello").await;
        let want: Vec<u8> = pieces.iter().flat_map(|(_, b)| b.to_vec()).collect();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            log.lock().unwrap().push((op, path.split('/').next().unwrap_or_default().to_owned()));
            futures::future::ready(Fault::None).boxed()
        })));
        let requests = |op: MemOp, dir: &str| seen.lock().unwrap().iter().filter(|(o, d)| *o == op && (dir.is_empty() || d == dir)).count();
        assert_eq!(read(&pool, &d, "big").await.unwrap(), want);
        assert_eq!(requests(MemOp::Get, ""), 0, "warm: everything from memory");

        let dropped = pool.drop_caches();
        assert_eq!((dropped.shards, dropped.shard_bytes), (1_101, 1_100 * 16 + 5), "{dropped:?}");
        assert!(dropped.pages > 0 && dropped.page_bytes > 0, "{dropped:?}");
        for cache in [&pool.shards, &pool.pages] {
            cache.run_pending_tasks();
            assert_eq!((cache.entry_count(), cache.weighted_size()), (0, 0));
        }
        assert_eq!(pool.metrics.cache_drops.get(), 1);

        assert_eq!(read(&pool, &d, "big").await.unwrap(), want);
        assert_eq!(read(&pool, &d, "small").await.unwrap(), b"hello");
        assert_eq!(read(&pool, &d, "big").await.unwrap(), want);
        assert_eq!(requests(MemOp::Get, "shards"), 1_101, "cold: each shard fetched once");
        assert_eq!(requests(MemOp::Get, "pages") as u64, dropped.pages, "and each page");
        assert_eq!(seen.lock().unwrap().len(), 1_101 + dropped.pages as usize, "nothing else: the drive's state stays");
        write_as(&pool, guard::Kind::Shard, &pieces[..1]).await;
        assert_eq!(requests(MemOp::Put, "") + requests(MemOp::PutNew, ""), 0, "the guard's checks stay: a shard stored already is not uploaded again");
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
