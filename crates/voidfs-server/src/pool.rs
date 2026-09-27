// SPDX-License-Identifier: Apache-2.0
//! A pool (format §2–§3) and its drives: loading them from the bucket, committing to their
//! logs, checkpoints, forks, deletion, and the change feed (protocol §5.6).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use voidfs_core::chunk::{Params, Shard};
use voidfs_core::ids::{DriveId, ObjectId, ShardHash, Timestamp, VersionId};
use voidfs_core::manifest::{self, Page};
use voidfs_core::model::{
    Chunking, Commit, CommitGuard, ContentDescriptor, DriveDescriptor, EntryRow, Extent, Features, ForkOf, HistoryRow, Kind, ObjectRecord,
    Op, PoolDescriptor, RemovedRow, Txn,
};
use voidfs_core::ops::OpError;
use voidfs_core::state::{DriveState, Rows};

/// Commits between checkpoints (format §8.4).
const CHECKPOINT_EVERY: u64 = 1000;
/// Rows per checkpoint segment.
const SEGMENT_ROWS: usize = 4096;
/// Change-feed batches kept in memory per drive.
const FEED_KEEP: usize = 10_000;

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

fn segments<T: Serialize>(table: &str, rows: &[T], key: impl Fn(&T) -> String, pages: &mut Vec<Page>) -> Vec<SegmentRef> {
    rows.chunks(SEGMENT_ROWS)
        .map(|chunk| {
            let seg = SegmentOut { kind: "segment", table, rows: chunk };
            let bytes = Bytes::from(serde_json::to_vec(&seg).expect("rows serialize"));
            let page = ShardHash::of(&bytes);
            pages.push(Page { hash: page, bytes });
            SegmentRef { page, first: key(&chunk[0]), last: key(chunk.last().unwrap()), count: chunk.len() }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Drives

pub struct Drive {
    pub id: DriveId,
    pub alias: String,
    pub desc: DriveDescriptor,
    state: RwLock<Arc<DriveState>>,
    commit_lock: tokio::sync::Mutex<()>,
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
    fn new(desc: DriveDescriptor, state: DriveState) -> Drive {
        let (notify, _) = tokio::sync::watch::channel(state.seq());
        let floor = state.seq();
        Drive {
            id: desc.drive_id.clone(),
            alias: desc.alias.clone(),
            desc,
            state: RwLock::new(Arc::new(state)),
            commit_lock: tokio::sync::Mutex::new(()),
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
    shards: moka::sync::Cache<ShardHash, Bytes>,
    pages: moka::sync::Cache<ShardHash, Bytes>,
    drives: RwLock<HashMap<DriveId, Arc<Drive>>>,
    aliases: RwLock<HashMap<String, DriveId>>,
    deleted: RwLock<HashMap<String, DriveId>>,
    authority: String,
}

impl Pool {
    /// Opens the pool in `store`, creating it if the store is empty.
    pub async fn open(store: crate::store::Store, cache_bytes: u64) -> anyhow::Result<Arc<Pool>> {
        let desc = match store.get("voidfs.json").await? {
            Some(b) => serde_json::from_slice::<PoolDescriptor>(&b).context("reading voidfs.json")?,
            None => {
                let d = PoolDescriptor {
                    format: voidfs_core::FORMAT_VERSION,
                    pool_id: format!("p-{}", uuid::Uuid::new_v4()),
                    created: Timestamp::now(),
                    features: Features { compatible: vec![], incompatible: vec![] },
                    chunking: Chunking::default(),
                    hash: "sha256".into(),
                    commit_guard: CommitGuard::CreateIfAbsent,
                };
                if !store.put_new("voidfs.json", Bytes::from(serde_json::to_vec_pretty(&d)?)).await? {
                    return Box::pin(Pool::open(store, cache_bytes)).await;
                }
                d
            }
        };
        desc.check_readable().map_err(|e| anyhow!(e))?;
        let params = Params::from_pool(&desc.chunking)?;
        let weigh = |_: &ShardHash, v: &Bytes| v.len().try_into().unwrap_or(u32::MAX);
        let pool = Arc::new(Pool {
            store,
            desc,
            params,
            shards: moka::sync::Cache::builder().weigher(weigh).max_capacity(cache_bytes).build(),
            pages: moka::sync::Cache::builder().weigher(weigh).max_capacity(cache_bytes / 8 + 1).build(),
            drives: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            deleted: RwLock::new(HashMap::new()),
            authority: format!("a-{}", uuid::Uuid::new_v4()),
        });
        for dir in pool.store.list_dirs("drives/").await? {
            let Ok(id) = dir.parse::<DriveId>() else { continue };
            match pool.load_drive(&id).await {
                Ok(Some(d)) => {
                    let deleted = pool.store.exists(&format!("drives/{id}/deleted.json")).await?;
                    pool.register(d, deleted);
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
        Ok(pool)
    }

    fn register(&self, d: Drive, deleted: bool) {
        let d = Arc::new(d);
        if deleted {
            self.deleted.write().unwrap().insert(d.alias.clone(), d.id.clone());
        } else {
            self.aliases.write().unwrap().insert(d.alias.clone(), d.id.clone());
        }
        self.drives.write().unwrap().insert(d.id.clone(), d);
    }

    async fn load_drive(&self, id: &DriveId) -> anyhow::Result<Option<Drive>> {
        let Some(b) = self.store.get(&format!("drives/{id}/drive.json")).await? else { return Ok(None) };
        let desc: DriveDescriptor = serde_json::from_slice(&b).context("reading drive.json")?;
        let mut state = match self.latest_checkpoint(id).await? {
            Some(s) => s,
            None if desc.fork_of.is_some() => bail!("fork {id} has no checkpoint"),
            None => DriveState::empty(),
        };
        let drive_feed = self.replay(id, &mut state).await?;
        let d = Drive::new(desc, state);
        for b in drive_feed {
            d.push_feed(b);
        }
        Ok(Some(d))
    }

    /// Applies every commit after `state.seq()` and returns their feed batches.
    async fn replay(&self, id: &DriveId, state: &mut DriveState) -> anyhow::Result<Vec<FeedBatch>> {
        let mut out = Vec::new();
        let dir = format!("drives/{id}/log/");
        let after = format!("{:020}.json", state.seq());
        for name in self.store.list_files(&dir, Some(&after)).await? {
            let bytes = self.store.get(&format!("{dir}{name}")).await?.ok_or_else(|| anyhow!("log entry {name} vanished"))?;
            let commit: Commit = serde_json::from_slice(&bytes).with_context(|| format!("parsing {name}"))?;
            if commit.seq != state.seq() + 1 {
                break; // a gap: stop at it (format §8.4)
            }
            let next = state.apply(&commit).with_context(|| format!("applying {name}"))?;
            out.push(feed_for(&commit, state, &next));
            *state = next;
        }
        Ok(out)
    }

    async fn latest_checkpoint(&self, id: &DriveId) -> anyhow::Result<Option<DriveState>> {
        let dir = format!("drives/{id}/checkpoints/");
        let Some(name) = self.store.list_files(&dir, None).await?.into_iter().rfind(|n| n.ends_with(".json")) else {
            return Ok(None);
        };
        self.load_checkpoint(&format!("{dir}{name}")).await.map(Some)
    }

    /// Reads a checkpoint index and its segments into a state (format §8).
    async fn load_checkpoint(&self, path: &str) -> anyhow::Result<DriveState> {
        let bytes = self.store.get(path).await?.ok_or_else(|| anyhow!("checkpoint {path} is missing"))?;
        let idx: CheckpointIndex = serde_json::from_slice(&bytes).with_context(|| format!("parsing {path}"))?;
        let rows = Rows {
            entries: self.read_segments::<EntryRow>(&idx.tables.entries).await?,
            objects: self.read_segments::<ObjectRecord>(&idx.tables.objects).await?,
            history: self.read_segments::<HistoryRow>(&idx.tables.history).await?,
            removed: self.read_segments::<RemovedRow>(&idx.tables.removed).await?,
        };
        Ok(DriveState::from_rows(idx.seq, idx.time, rows)?)
    }

    async fn read_segments<T: for<'de> Deserialize<'de>>(&self, refs: &[SegmentRef]) -> anyhow::Result<Vec<T>> {
        let mut out = Vec::new();
        for r in refs {
            let bytes = self.page(&r.page).await?;
            let seg: Segment<T> = serde_json::from_slice(&bytes)?;
            out.extend(seg.rows);
        }
        Ok(out)
    }

    async fn write_checkpoint(&self, id: &DriveId, state: &DriveState) -> anyhow::Result<()> {
        let rows = state.rows();
        let mut pages = Vec::new();
        let tables = CheckpointTables {
            entries: segments("entries", &rows.entries, |e| format!("{}/{}{}", e.parent, e.name, if e.kind == Kind::Folder { "/" } else { "" }), &mut pages),
            objects: segments("objects", &rows.objects, |o| o.oid.to_string(), &mut pages),
            history: segments("history", &rows.history, |h| format!("{}@{:020}.{}", h.oid, h.version.seq, h.version.idx), &mut pages),
            removed: segments("removed", &rows.removed, |r| format!("{}@{}", r.key, r.oid), &mut pages),
        };
        self.write_pages(&pages).await?;
        let idx = CheckpointIndex {
            format: 1,
            seq: state.seq(),
            time: state.time(),
            tables,
            stats: serde_json::json!({ "objects": rows.objects.len(), "bytes": state.live_bytes() }),
        };
        let name = format!("drives/{id}/checkpoints/{:020}.json", state.seq());
        self.store.put(&name, Bytes::from(serde_json::to_vec(&idx)?)).await?;
        self.store.put(&format!("drives/{id}/_last_checkpoint"), Bytes::from(format!("{{\"seq\":{}}}", state.seq()))).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------------------------
    // Shards and pages

    pub async fn shard(&self, h: &ShardHash) -> anyhow::Result<Bytes> {
        if let Some(b) = self.shards.get(h) {
            return Ok(b);
        }
        let b = self.store.get(&format!("shards/{}", h.object_path())).await?.ok_or_else(|| anyhow!("shard {h} is missing"))?;
        if ShardHash::of(&b) != *h {
            bail!("shard {h} is corrupt");
        }
        self.shards.insert(*h, b.clone());
        Ok(b)
    }

    pub async fn page(&self, h: &ShardHash) -> anyhow::Result<Bytes> {
        if let Some(b) = self.pages.get(h) {
            return Ok(b);
        }
        let b = self.store.get(&format!("pages/{}", h.object_path())).await?.ok_or_else(|| anyhow!("page {h} is missing"))?;
        self.pages.insert(*h, b.clone());
        Ok(b)
    }

    pub async fn write_shards(&self, shards: &[Shard]) -> anyhow::Result<()> {
        let tasks = shards.iter().filter(|s| !self.shards.contains_key(&s.hash)).map(|s| async move {
            self.store.put(&format!("shards/{}", s.hash.object_path()), s.bytes.clone()).await?;
            self.shards.insert(s.hash, s.bytes.clone());
            anyhow::Ok(())
        });
        for r in futures::future::join_all(tasks).await {
            r?;
        }
        Ok(())
    }

    pub async fn write_pages(&self, pages: &[Page]) -> anyhow::Result<()> {
        let tasks = pages.iter().filter(|p| !self.pages.contains_key(&p.hash)).map(|p| async move {
            self.store.put(&format!("pages/{}", p.hash.object_path()), p.bytes.clone()).await?;
            self.pages.insert(p.hash, p.bytes.clone());
            anyhow::Ok(())
        });
        for r in futures::future::join_all(tasks).await {
            r?;
        }
        Ok(())
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
        let (state, fork_of) = match source {
            None => (DriveState::empty(), None),
            Some(src) => {
                // Hold the source's commit lock so the fork includes every acknowledged write.
                let _g = src.commit_lock.lock().await;
                let s = src.snapshot().fork();
                self.write_checkpoint(&id, &s).await?;
                let seq = s.seq();
                (s, Some(ForkOf { drive_id: src.id.clone(), seq }))
            }
        };
        let desc = DriveDescriptor { format: 1, drive_id: id.clone(), created: Timestamp::now(), alias: alias.to_owned(), fork_of };
        if !self.store.put_new(&format!("drives/{id}/drive.json"), Bytes::from(serde_json::to_vec_pretty(&desc).map_err(anyhow::Error::from)?)).await? {
            return Err(CreateError::Other(anyhow!("drive id collision")));
        }
        if self.alias_taken(alias) {
            let _ = self.store.delete_prefix(&format!("drives/{id}/")).await;
            return Err(CreateError::Exists);
        }
        if let Some(src) = source {
            src.forks.write().unwrap().push(id.clone());
        }
        self.register(Drive::new(desc, state), false);
        Ok(self.drive_by_id(&id).unwrap())
    }

    pub async fn soft_delete(&self, d: &Drive) -> anyhow::Result<()> {
        let marker = serde_json::json!({ "time": Timestamp::now(), "actor": { "kind": "system", "id": "voidfs" } });
        self.store.put(&format!("drives/{}/deleted.json", d.id), Bytes::from(marker.to_string())).await?;
        self.aliases.write().unwrap().remove(&d.alias);
        self.deleted.write().unwrap().insert(d.alias.clone(), d.id.clone());
        Ok(())
    }

    pub async fn undelete(&self, alias: &str) -> Result<Arc<Drive>, CreateError> {
        let id = self.deleted.read().unwrap().get(alias).cloned().ok_or(CreateError::NotFound)?;
        if self.alias_taken(alias) {
            return Err(CreateError::Exists);
        }
        self.store.delete(&format!("drives/{id}/deleted.json")).await?;
        self.deleted.write().unwrap().remove(alias);
        self.aliases.write().unwrap().insert(alias.to_owned(), id.clone());
        Ok(self.drive_by_id(&id).unwrap())
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
        let d = self.drive_by_id(&id).unwrap();
        self.store.delete_prefix(&format!("drives/{id}/")).await?;
        self.aliases.write().unwrap().remove(&d.alias);
        self.deleted.write().unwrap().remove(&d.alias);
        self.drives.write().unwrap().remove(&id);
        for other in self.drives.read().unwrap().values() {
            other.forks.write().unwrap().retain(|f| *f != id);
        }
        Ok(true)
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
        let _g = d.commit_lock.lock().await;
        let cur = d.snapshot();
        let txn = plan(&cur)?;
        let now = Timestamp::now();
        let time = cur.time().map_or(now, |t| t.max(now));
        let commit = Commit { format: 1, seq: cur.seq() + 1, time, authority: self.authority.clone(), txns: vec![txn] };
        let next = cur.apply(&commit).map_err(|e| CommitError::Other(anyhow!("planned transaction does not apply: {e}")))?;
        let bytes = Bytes::from(serde_json::to_vec(&commit).map_err(anyhow::Error::from)?);
        if !self.store.put_new(&log_path(&d.id, commit.seq), bytes).await? {
            // Another authority wrote this sequence number: catch up and ask for a new plan.
            let mut s = (*cur).clone();
            let batches = self.replay(&d.id, &mut s).await?;
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
        if commit.seq.is_multiple_of(CHECKPOINT_EVERY)
            && let Err(e) = self.write_checkpoint(&d.id, &next).await {
                tracing::warn!("checkpoint of {} at {} failed: {e:#}", d.id, commit.seq);
            }
        Ok((VersionId::new(commit.seq, 0), next))
    }

    /// The drive's state at instant `t`, rebuilt from its checkpoints and log.
    pub async fn state_at(&self, d: &Drive, t: Timestamp) -> anyhow::Result<DriveState> {
        let mut state = DriveState::empty();
        if let Some(fork) = &d.desc.fork_of {
            // A fork's namespace before its own log exists only as its first checkpoint, so its
            // point-in-time window starts at the fork point (format §8.5, §9).
            state = self.load_checkpoint(&format!("drives/{}/checkpoints/{:020}.json", d.id, fork.seq)).await?;
            if state.time().is_some_and(|ct| ct > t) {
                bail!("that instant is before this fork was made");
            }
        }
        let dir = format!("drives/{}/log/", d.id);
        let after = format!("{:020}.json", state.seq());
        for name in self.store.list_files(&dir, Some(&after)).await? {
            let bytes = self.store.get(&format!("{dir}{name}")).await?.ok_or_else(|| anyhow!("log entry vanished"))?;
            let commit: Commit = serde_json::from_slice(&bytes)?;
            if commit.time > t || commit.seq != state.seq() + 1 {
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
    use voidfs_core::model::{Actor, Attrs};
    use voidfs_core::names::Key;
    use voidfs_core::ops::{self, Precondition};

    use super::*;
    use crate::store::Store;

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
}
