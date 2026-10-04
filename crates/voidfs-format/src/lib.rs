// SPDX-License-Identifier: Apache-2.0
//! A reader of the on-bucket format (`spec/format.md`): what it takes to open a pool and its
//! drives with nothing but read access to the bucket. The pool's descriptor (§3), a drive's
//! descriptor (§6), its newest checkpoint (§8) and the commits after it (§7, §8.4), and manifest
//! trees (§5.1).
//!
//! The server loads its drives with it, through its own caches; a client reads a drive straight
//! from the bucket with storage credentials (protocol §5.5). Both hand it a [`Source`].

use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use futures::StreamExt;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use voidfs_core::ids::{DriveId, ShardHash, Timestamp};
use voidfs_core::manifest;
use voidfs_core::model::{Commit, ContentDescriptor, DriveDescriptor, Extent, MULTI_OBJECT_VERSIONS, ManifestPage, PoolDescriptor};
use voidfs_core::state::{DriveState, Rows};

/// The pool descriptor (format §3).
pub const DESCRIPTOR: &str = "voidfs.json";
/// Checkpoint segments fetched at once: each is a round trip to the bucket.
pub const PAGE_FETCH_PARALLELISM: usize = 32;
/// Commits fetched at once.
pub const LOG_FETCH_PARALLELISM: usize = 32;

/// Where a reader reads the pool from: paths are relative to the pool's root (format §2).
pub trait Source: Send + Sync {
    /// The object at `path`, or `None` if there is none.
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, anyhow::Result<Option<Bytes>>>;

    /// The names of the objects directly under `dir` (which ends in `/`), sorted, after `after` if
    /// given. Only objects, not the prefixes under it.
    fn list<'a>(&'a self, dir: &'a str, after: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<Vec<String>>>;

    /// A manifest page or checkpoint segment, checked against its hash: one that parses but does
    /// not match would load a drive in a state it never had.
    fn page<'a>(&'a self, h: &'a ShardHash) -> BoxFuture<'a, anyhow::Result<Bytes>> {
        Box::pin(async move {
            let b = self.get(&page_path(h)).await?.ok_or_else(|| anyhow!("page {h} is missing"))?;
            if ShardHash::of(&b) != *h {
                bail!("page {h} is corrupt");
            }
            Ok(b)
        })
    }
}

pub fn shard_path(h: &ShardHash) -> String {
    format!("shards/{}", h.object_path())
}

pub fn page_path(h: &ShardHash) -> String {
    format!("pages/{}", h.object_path())
}

pub fn log_path(id: &DriveId, seq: u64) -> String {
    format!("drives/{id}/log/{seq:020}.json")
}

pub fn checkpoint_path(id: &DriveId, seq: u64) -> String {
    format!("drives/{id}/checkpoints/{seq:020}.json")
}

// ---------------------------------------------------------------------------------------------
// Checkpoints (format §8)

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentRef {
    pub page: ShardHash,
    pub first: String,
    pub last: String,
    pub count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointTables {
    pub entries: Vec<SegmentRef>,
    pub objects: Vec<SegmentRef>,
    pub history: Vec<SegmentRef>,
    #[serde(default)]
    pub removed: Vec<SegmentRef>,
}

impl CheckpointTables {
    /// Every page the index lists.
    pub fn pages(&self) -> std::collections::HashSet<ShardHash> {
        self.entries.iter().chain(&self.objects).chain(&self.history).chain(&self.removed).map(|s| s.page).collect()
    }
}

/// A checkpoint index, `drives/<id>/checkpoints/<seq>.json` (format §8.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointIndex {
    pub format: u32,
    pub seq: u64,
    #[serde(default)]
    pub time: Option<Timestamp>,
    pub tables: CheckpointTables,
    pub stats: serde_json::Value,
}

/// A checkpoint segment (format §8.2), as read.
#[derive(Deserialize)]
pub struct Segment<T> {
    pub rows: Vec<T>,
}

/// A checkpoint, loaded.
pub struct Checkpoint {
    pub path: String,
    pub index: CheckpointIndex,
    pub state: DriveState,
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

/// Reads a checkpoint index and its segments into a state (format §8), under the pool's rule for
/// versions.
pub async fn load_checkpoint(src: &impl Source, path: &str, multi_object_versions: bool) -> anyhow::Result<Checkpoint> {
    let bytes = src.get(path).await?.ok_or_else(|| anyhow!("checkpoint {path} is missing"))?;
    let index: CheckpointIndex = serde_json::from_slice(&bytes).with_context(|| format!("parsing {path}"))?;
    let t = &index.tables;
    // Each segment is a round trip to the bucket: fetch them concurrently, and in order.
    let hashes: Vec<ShardHash> = t.entries.iter().chain(&t.objects).chain(&t.history).chain(&t.removed).map(|r| r.page).collect();
    let mut pages = std::pin::pin!(futures::stream::iter(hashes).map(|h| async move { src.page(&h).await }).buffered(PAGE_FETCH_PARALLELISM));
    let rows = Rows {
        entries: rows_of(&mut pages, t.entries.len()).await?,
        objects: rows_of(&mut pages, t.objects.len()).await?,
        history: rows_of(&mut pages, t.history.len()).await?,
        removed: rows_of(&mut pages, t.removed.len()).await?,
    };
    let state = DriveState::from_rows(index.seq, index.time, rows)?.with_multi_object_versions(multi_object_versions);
    Ok(Checkpoint { path: path.to_owned(), index, state })
}

/// The drive's newest checkpoint, if it has one (format §8.4).
pub async fn latest_checkpoint(src: &impl Source, id: &DriveId, multi_object_versions: bool) -> anyhow::Result<Option<Checkpoint>> {
    let dir = format!("drives/{id}/checkpoints/");
    let Some(name) = src.list(&dir, None).await?.into_iter().rfind(|n| n.ends_with(".json")) else {
        return Ok(None);
    };
    load_checkpoint(src, &format!("{dir}{name}"), multi_object_versions).await.map(Some)
}

// ---------------------------------------------------------------------------------------------
// The log (format §7)

/// The drive's commits after `seq`, in order, each with its size as stored. Fetched concurrently,
/// since each is a round trip to the bucket; stops at the first gap (format §8.4).
pub async fn commits_after(src: &impl Source, id: &DriveId, seq: u64) -> anyhow::Result<Vec<(Commit, usize)>> {
    let dir = format!("drives/{id}/log/");
    let names = src.list(&dir, Some(&format!("{seq:020}.json"))).await?;
    let mut stream = futures::stream::iter(names)
        .map(|name| {
            let path = format!("{dir}{name}");
            async move {
                let bytes = src.get(&path).await?.ok_or_else(|| anyhow!("log entry {path} vanished"))?;
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

/// Applies every commit after `state.seq()` up to the first gap, and returns how many it applied.
pub async fn catch_up(src: &impl Source, id: &DriveId, state: &mut DriveState) -> anyhow::Result<usize> {
    let commits = commits_after(src, id, state.seq()).await?;
    for (c, _) in &commits {
        *state = state.apply(c).with_context(|| format!("applying {}", log_path(id, c.seq)))?;
    }
    Ok(commits.len())
}

// ---------------------------------------------------------------------------------------------
// Pools and drives

/// The pool's descriptor, refused if this reader can't read the pool correctly (format §3, §3.1).
pub async fn open_pool(src: &impl Source) -> anyhow::Result<PoolDescriptor> {
    let bytes = src.get(DESCRIPTOR).await?.ok_or_else(|| anyhow!("there is no pool here: {DESCRIPTOR} is missing"))?;
    let desc: PoolDescriptor = serde_json::from_slice(&bytes).with_context(|| format!("reading {DESCRIPTOR}"))?;
    desc.check_readable().map_err(|e| anyhow!(e))?;
    Ok(desc)
}

/// A drive's descriptor, or `None` if the drive does not exist (format §6).
pub async fn drive(src: &impl Source, id: &DriveId) -> anyhow::Result<Option<DriveDescriptor>> {
    let Some(b) = src.get(&format!("drives/{id}/drive.json")).await? else { return Ok(None) };
    Ok(Some(serde_json::from_slice(&b).context("reading drive.json")?))
}

/// A drive's latest state (format §8.4): its newest checkpoint and every commit after it, under
/// `pool`'s rule for versions. `None` if the drive does not exist.
pub async fn load_drive(src: &impl Source, pool: &PoolDescriptor, id: &DriveId) -> anyhow::Result<Option<(DriveDescriptor, DriveState)>> {
    let Some(desc) = drive(src, id).await? else { return Ok(None) };
    let multi_object_versions = pool.has(MULTI_OBJECT_VERSIONS);
    let mut state = match latest_checkpoint(src, id, multi_object_versions).await? {
        Some(c) => c.state,
        None if desc.fork_of.is_some() => bail!("fork {id} has no checkpoint"),
        None => DriveState::empty().with_multi_object_versions(multi_object_versions),
    };
    catch_up(src, id, &mut state).await?;
    Ok(Some((desc, state)))
}

// ---------------------------------------------------------------------------------------------
// Content (format §5)

/// The full extent list of a content descriptor, reading a tree's pages (format §5.1).
pub async fn extents(src: &impl Source, desc: &ContentDescriptor) -> anyhow::Result<Vec<Extent>> {
    // Fetch the tree's pages, then flatten from what was fetched.
    let mut pages = std::collections::HashMap::new();
    if let ContentDescriptor::Tree { root, .. } = desc {
        let mut pending = vec![*root];
        while let Some(h) = pending.pop() {
            let bytes = src.page(&h).await?;
            if let Ok(ManifestPage::Node { children }) = serde_json::from_slice(&bytes) {
                pending.extend(children.iter().map(|c| c.page));
            }
            pages.insert(h, bytes);
        }
    }
    Ok(manifest::flatten(desc, &mut |h| pages.get(h).cloned())?)
}
