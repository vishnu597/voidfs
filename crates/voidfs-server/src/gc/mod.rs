// SPDX-License-Identifier: Apache-2.0
//! Garbage collection (format §12, RFC 0002): deletes shards and pages that nothing in the pool
//! references any more.
//!
//! A run has two phases, at least the grace period apart. [`step`] does whichever is due:
//! - **Phase 1** creates `gc/pending.json`, computes what is referenced, and publishes the
//!   candidates: unreferenced objects last written before `t1 - grace`.
//! - **Phase 2**, once `t1 + grace` has passed, marks the run `deleting`, computes the
//!   references again, and deletes the candidates that are still unreferenced and unchanged.
//!
//! Before phase 1, a step also expires what the format lets it: drives soft-deleted longer
//! than their window (§10) and multipart uploads left open too long (§11).
//!
//! Writers' side of the protocol is in [`guard`]. The protocol itself is checked by [`model`].

pub mod guard;
#[cfg(test)]
mod model;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use voidfs_core::ids::{DriveId, ShardHash, Timestamp};
use voidfs_core::model::{Change, Commit, ContentDescriptor, Extent, ManifestPage};

use crate::pool::Pool;
use crate::store::Store;

/// The run record (format §12.1).
pub const PENDING: &str = "gc/pending.json";
/// The shortest grace period allowed while writers may be active (format §12.1).
pub const MIN_GRACE: Duration = Duration::from_secs(24 * 3600);
/// Objects deleted at once.
const DELETE_PARALLELISM: usize = 32;
/// Drives, checkpoints and log entries read at once while computing references.
const READ_PARALLELISM: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Marking,
    Waiting,
    Deleting,
}

/// `gc/pending.json` (format §12.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingRecord {
    pub format: u32,
    pub run: String,
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t1: Option<Timestamp>,
    pub grace: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ShardHash>,
}

impl PendingRecord {
    fn bytes(&self) -> anyhow::Result<Bytes> {
        Ok(Bytes::from(serde_json::to_vec(self)?))
    }

    fn grace(&self) -> Duration {
        Duration::from_secs(self.grace)
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub grace: Duration,
    /// No writer is active in the pool, so a grace shorter than [`MIN_GRACE`] is allowed.
    pub offline: bool,
    /// Report what a run would do, and change nothing.
    pub dry_run: bool,
    /// Hard-delete drives soft-deleted longer than this (format §10).
    pub expire_deleted_drives: Option<Duration>,
    /// Abort multipart uploads open longer than this (format §11).
    pub abort_uploads: Option<Duration>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            grace: MIN_GRACE,
            offline: false,
            dry_run: false,
            expire_deleted_drives: Some(Duration::from_secs(30 * 86_400)),
            abort_uploads: Some(Duration::from_secs(7 * 86_400)),
        }
    }
}

/// What a step did.
#[derive(Debug, Default)]
pub struct Report {
    pub run: Option<String>,
    pub outcome: Outcome,
    pub expired_drives: Vec<DriveId>,
    pub aborted_uploads: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    #[default]
    Nothing,
    /// Phase 1 found nothing to collect.
    NothingToCollect,
    /// Phase 1 proposed these candidates; phase 2 is due at `due`.
    Proposed { candidates: usize, bytes: u64, due: Timestamp },
    /// A run is waiting for its grace period to pass.
    Waiting { due: Timestamp },
    /// Phase 2 deleted these.
    Deleted { objects: usize, bytes: u64 },
    /// Phase 1 took longer than half the grace period, so the run was dropped (§12.2 step 5).
    Abandoned,
    /// Another collector is marking.
    Busy,
    /// A dry run found these candidates.
    WouldDelete { candidates: usize, bytes: u64 },
}

impl Outcome {
    /// Its name in metrics ([`crate::metrics::GC_OUTCOMES`]).
    pub fn name(&self) -> &'static str {
        match self {
            Outcome::Nothing => "nothing",
            Outcome::NothingToCollect => "nothing_to_collect",
            Outcome::Proposed { .. } => "proposed",
            Outcome::Waiting { .. } => "waiting",
            Outcome::Deleted { .. } => "deleted",
            Outcome::Abandoned => "abandoned",
            Outcome::Busy => "busy",
            Outcome::WouldDelete { .. } => "dry_run",
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.outcome {
            Outcome::Nothing => write!(f, "nothing to do")?,
            Outcome::NothingToCollect => write!(f, "nothing to collect")?,
            Outcome::Proposed { candidates, bytes, due } => write!(f, "proposed {candidates} objects ({}) for deletion; phase 2 is due at {due}", human(*bytes))?,
            Outcome::Waiting { due } => write!(f, "a run is waiting; phase 2 is due at {due}")?,
            Outcome::Deleted { objects, bytes } => write!(f, "deleted {objects} objects ({})", human(*bytes))?,
            Outcome::Abandoned => write!(f, "phase 1 took longer than half the grace period; the run was abandoned")?,
            Outcome::Busy => write!(f, "another collector is marking")?,
            Outcome::WouldDelete { candidates, bytes } => write!(f, "dry run: {candidates} objects ({}) are unreferenced and older than the grace period", human(*bytes))?,
        }
        if let Some(run) = &self.run {
            write!(f, " (run {run})")?;
        }
        if !self.expired_drives.is_empty() {
            let ids: Vec<String> = self.expired_drives.iter().map(|d| d.to_string()).collect();
            write!(f, "; {} soft-deleted drive(s) {}: {}", ids.len(), if matches!(self.outcome, Outcome::WouldDelete { .. }) { "would be hard-deleted" } else { "hard-deleted" }, ids.join(", "))?;
        }
        if self.aborted_uploads > 0 {
            write!(f, "; {} stale multipart upload(s) {}", self.aborted_uploads, if matches!(self.outcome, Outcome::WouldDelete { .. }) { "would be aborted" } else { "aborted" })?;
        }
        Ok(())
    }
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

fn secs(d: Duration) -> chrono::Duration {
    chrono::Duration::from_std(d).unwrap_or(chrono::Duration::MAX)
}

fn plus(t: Timestamp, d: Duration) -> Timestamp {
    Timestamp::from_datetime(t.datetime() + secs(d))
}

/// `t` truncated to whole seconds: modification times from some stores have no finer
/// resolution, and rounding `t1` down only ever keeps more.
fn whole_seconds(t: Timestamp) -> Timestamp {
    let dt = t.datetime();
    Timestamp::from_datetime(chrono::DateTime::from_timestamp(dt.timestamp(), 0).unwrap_or(dt))
}

async fn read_pending(store: &Store) -> anyhow::Result<Option<(PendingRecord, Timestamp)>> {
    let Some(b) = store.get(PENDING).await? else { return Ok(None) };
    let rec: PendingRecord = serde_json::from_slice(&b).with_context(|| format!("reading {PENDING}"))?;
    if rec.format != 1 {
        bail!("{PENDING} has format {}, which this server does not implement", rec.format);
    }
    let modified = store.modified(PENDING).await?.ok_or_else(|| anyhow!("{PENDING} vanished"))?;
    Ok(Some((rec, modified)))
}

/// Does whichever part of a run is due: expiry and phase 1 if no run is in progress, phase 2 if
/// a run's grace period has passed. With a grace period of zero (offline only), a single step
/// does a whole run.
pub async fn step(pool: &Pool, opts: &Options) -> anyhow::Result<Report> {
    if opts.grace < MIN_GRACE && !opts.offline {
        bail!("a grace period under 24 hours is only safe when no server is writing to the pool (offline collection)");
    }
    let mut report = Report::default();
    if opts.dry_run {
        dry_run(pool, opts, &mut report).await?;
        return Ok(report);
    }
    let now = pool.clock.now();
    match read_pending(&pool.store).await? {
        None => phase1(pool, opts, &mut report).await?,
        Some((rec, modified)) => {
            report.run = Some(rec.run.clone());
            match rec.phase {
                // A collector that stopped while marking leaves this behind (§12.1).
                Phase::Marking if now.datetime() - modified.datetime() > secs(rec.grace().max(opts.grace)) => {
                    tracing::warn!("removing run {} left marking since {modified}", rec.run);
                    pool.store.delete(PENDING).await?;
                    report.run = None;
                    phase1(pool, opts, &mut report).await?;
                }
                Phase::Marking => report.outcome = Outcome::Busy,
                Phase::Waiting => {
                    let t1 = rec.t1.ok_or_else(|| anyhow!("{PENDING} is waiting but has no t1"))?;
                    let due = plus(t1, rec.grace());
                    if now >= due {
                        phase2(pool, rec, &mut report).await?;
                    } else {
                        report.outcome = Outcome::Waiting { due };
                    }
                }
                // A collector stopped while deleting: finish its run.
                Phase::Deleting => phase2(pool, rec, &mut report).await?,
            }
        }
    }
    Ok(report)
}

/// Runs [`step`] every `every`, forever.
pub async fn run_periodically(pool: Arc<Pool>, opts: Options, every: Duration) {
    loop {
        tokio::time::sleep(every).await;
        let stepped = step(&pool, &opts).await;
        record(&pool, &stepped);
        match stepped {
            Ok(r) => tracing::info!("garbage collection: {r}"),
            Err(e) => tracing::warn!("garbage collection failed: {e:#}"),
        }
    }
}

/// Records a step's outcome in the pool's metrics.
fn record(pool: &Pool, stepped: &anyhow::Result<Report>) {
    match stepped {
        Ok(r) => {
            let (objects, bytes) = match r.outcome {
                Outcome::Deleted { objects, bytes } => (objects, bytes),
                _ => (0, 0),
            };
            pool.metrics.gc_step(r.outcome.name(), objects, bytes);
        }
        Err(_) => pool.metrics.gc_step("failed", 0, 0),
    }
}

async fn phase1(pool: &Pool, opts: &Options, report: &mut Report) -> anyhow::Result<()> {
    let store = &pool.store;
    expire(pool, opts, report, false).await?;
    let run = uuid::Uuid::new_v4().to_string();
    let started = pool.clock.mono();
    let mut rec = PendingRecord { format: 1, run: run.clone(), phase: Phase::Marking, t1: None, grace: opts.grace.as_secs(), candidates: Vec::new() };
    if !pool.create(PENDING, rec.bytes()?).await? {
        report.outcome = Outcome::Busy;
        return Ok(());
    }
    report.run = Some(run.clone());
    // t1 is on the bucket's clock: the record's own modification time (§12.2 step 1).
    let t1 = whole_seconds(store.modified(PENDING).await?.ok_or_else(|| anyhow!("{PENDING} vanished"))?);
    let refs = referenced(pool).await?;
    let cutoff = Timestamp::from_datetime(t1.datetime() - secs(opts.grace));
    let mut candidates = HashSet::new();
    let mut bytes = 0;
    for o in list_objects(store).await? {
        if !refs.contains(&o.hash) && o.modified.is_some_and(|m| m < cutoff) && candidates.insert(o.hash) {
            bytes += o.size;
        }
    }
    if candidates.is_empty() {
        store.delete(PENDING).await?;
        report.outcome = Outcome::NothingToCollect;
        return Ok(());
    }
    if !opts.offline && pool.clock.mono().saturating_sub(started) > opts.grace / 2 {
        store.delete(PENDING).await?;
        report.outcome = Outcome::Abandoned;
        return Ok(());
    }
    // One collector at a time is the deployment's job (§12.1); this only catches mistakes.
    match read_pending(store).await? {
        Some((r, _)) if r.run == run => {}
        _ => bail!("{PENDING} was replaced while run {run} was marking; is another collector running?"),
    }
    rec.phase = Phase::Waiting;
    rec.t1 = Some(t1);
    rec.candidates = candidates.into_iter().collect();
    rec.candidates.sort();
    let count = rec.candidates.len();
    store.put(PENDING, rec.bytes()?).await?;
    let _ = pool.guard.refresh(store, &pool.clock).await;
    let due = plus(t1, opts.grace);
    report.outcome = Outcome::Proposed { candidates: count, bytes, due };
    if pool.clock.now() >= due {
        phase2(pool, rec, report).await?;
    }
    Ok(())
}

async fn phase2(pool: &Pool, mut rec: PendingRecord, report: &mut Report) -> anyhow::Result<()> {
    let store = &pool.store;
    let t1 = rec.t1.ok_or_else(|| anyhow!("{PENDING} has no t1"))?;
    // Mark the run deleting before anything is recomputed (§12.3 step 1), and check on the
    // bucket's clock that the grace period has passed.
    rec.phase = Phase::Deleting;
    store.put(PENDING, rec.bytes()?).await?;
    let q = store.modified(PENDING).await?.ok_or_else(|| anyhow!("{PENDING} vanished"))?;
    let due = plus(t1, rec.grace());
    if q < due {
        rec.phase = Phase::Waiting;
        store.put(PENDING, rec.bytes()?).await?;
        report.outcome = Outcome::Waiting { due };
        return Ok(());
    }
    let _ = pool.guard.refresh(store, &pool.clock).await;
    let refs = referenced(pool).await?;
    let candidates: HashSet<ShardHash> = rec.candidates.iter().copied().collect();
    // Observed after the mark: anything rewritten since t1 stays (§12.3 step 3).
    let doomed: Vec<Object> = list_objects(store)
        .await?
        .into_iter()
        .filter(|o| candidates.contains(&o.hash) && !refs.contains(&o.hash) && o.modified.is_some_and(|m| m < t1))
        .collect();
    let bytes = doomed.iter().map(|o| o.size).sum();
    let objects = doomed.len();
    let mut deletes = futures::stream::iter(doomed)
        .map(|o| async move {
            store.delete(&o.path).await?;
            pool.forget(&o.hash);
            anyhow::Ok(())
        })
        .buffer_unordered(DELETE_PARALLELISM);
    while let Some(r) = deletes.next().await {
        r?;
    }
    store.delete(PENDING).await?;
    let _ = pool.guard.refresh(store, &pool.clock).await;
    report.outcome = Outcome::Deleted { objects, bytes };
    Ok(())
}

async fn dry_run(pool: &Pool, opts: &Options, report: &mut Report) -> anyhow::Result<()> {
    expire(pool, opts, report, true).await?;
    let refs = referenced(pool).await?;
    let cutoff = Timestamp::from_datetime(pool.clock.now().datetime() - secs(opts.grace));
    let mut seen = HashSet::new();
    let mut bytes = 0;
    for o in list_objects(&pool.store).await? {
        if !refs.contains(&o.hash) && o.modified.is_some_and(|m| m < cutoff) && seen.insert(o.hash) {
            bytes += o.size;
        }
    }
    report.outcome = Outcome::WouldDelete { candidates: seen.len(), bytes };
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Expiry

#[derive(Deserialize)]
struct Marker {
    time: Timestamp,
}

#[derive(Deserialize)]
struct UploadRecord {
    created: Timestamp,
}

async fn expire(pool: &Pool, opts: &Options, report: &mut Report, dry_run: bool) -> anyhow::Result<()> {
    let store = &pool.store;
    let now = pool.clock.now();
    let older = |t: Timestamp, d: Duration| now.datetime() - t.datetime() > secs(d);
    for dir in store.list_dirs("drives/").await? {
        let Ok(id) = dir.parse::<DriveId>() else { continue };
        if let Some(window) = opts.expire_deleted_drives
            && let Some(b) = store.get(&format!("drives/{id}/deleted.json")).await?
        {
            let marker: Marker = serde_json::from_slice(&b).with_context(|| format!("reading drives/{id}/deleted.json"))?;
            if older(marker.time, window) {
                if !dry_run {
                    pool.hard_delete_id(&id).await?;
                }
                report.expired_drives.push(id);
                continue;
            }
        }
        if let Some(limit) = opts.abort_uploads {
            let uploads = format!("drives/{id}/uploads/");
            for up in store.list_dirs(&uploads).await? {
                let Some(b) = store.get(&format!("{uploads}{up}/upload.json")).await? else { continue };
                let Ok(r) = serde_json::from_slice::<UploadRecord>(&b) else { continue };
                if older(r.created, limit) {
                    if !dry_run {
                        store.delete_prefix(&format!("{uploads}{up}/")).await?;
                    }
                    report.aborted_uploads += 1;
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// References

/// A shard or page in the bucket.
struct Object {
    path: String,
    hash: ShardHash,
    size: u64,
    modified: Option<Timestamp>,
}

/// Every shard and page in the bucket. Objects whose names are not hashes are not voidfs's and
/// are never touched.
async fn list_objects(store: &Store) -> anyhow::Result<Vec<Object>> {
    let mut out = Vec::new();
    for prefix in ["shards/", "pages/"] {
        for l in store.list_recursive(prefix).await? {
            let Some(hash) = l.name.rsplit('/').next().and_then(|n| n.parse::<ShardHash>().ok()) else { continue };
            if l.name != hash.object_path() {
                continue;
            }
            out.push(Object { path: format!("{prefix}{}", l.name), hash, size: l.size, modified: l.modified });
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct Index {
    tables: IndexTables,
}

#[derive(Deserialize)]
struct IndexTables {
    #[serde(default)]
    entries: Vec<SegmentRef>,
    #[serde(default)]
    objects: Vec<SegmentRef>,
    #[serde(default)]
    history: Vec<SegmentRef>,
    #[serde(default)]
    removed: Vec<SegmentRef>,
}

#[derive(Deserialize)]
struct SegmentRef {
    page: ShardHash,
}

/// Any checkpoint row; only `objects` and `history` rows have content.
#[derive(Deserialize)]
struct Row {
    #[serde(default)]
    content: Option<ContentDescriptor>,
}

#[derive(Deserialize)]
struct Segment {
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct Part {
    extents: Vec<Extent>,
}

/// Everything the pool references (format §12): each drive's checkpoints and the log after the
/// newest, including soft-deleted drives, and every open multipart upload. Read from the bucket,
/// not from memory, so drives other servers write are counted too. A referenced page that
/// cannot be read is an error: its references are unknown.
///
/// Commits before a drive's newest checkpoint are not read. Every content descriptor they set
/// is in that checkpoint's `objects` or `history` rows, which are.
pub async fn referenced(pool: &Pool) -> anyhow::Result<HashSet<ShardHash>> {
    let store = &pool.store;
    let mut refs = Refs::default();
    let ids: Vec<DriveId> = store.list_dirs("drives/").await?.iter().filter_map(|d| d.parse().ok()).collect();
    for id in ids {
        let dir = format!("drives/{id}/");
        if !store.exists(&format!("{dir}drive.json")).await? {
            // Not a drive yet (a fork being created) or any more (being hard-deleted).
            continue;
        }
        let names: Vec<String> = store.list_files(&format!("{dir}checkpoints/"), None).await?.into_iter().filter(|n| n.ends_with(".json")).collect();
        let newest = names.iter().filter_map(|n| n.trim_end_matches(".json").parse::<u64>().ok()).max().unwrap_or(0);
        let mut indexes = futures::stream::iter(names)
            .map(|n| {
                let path = format!("{dir}checkpoints/{n}");
                async move {
                    let b = store.get(&path).await?.ok_or_else(|| anyhow!("{path} vanished"))?;
                    serde_json::from_slice::<Index>(&b).with_context(|| format!("reading {path}"))
                }
            })
            .buffered(READ_PARALLELISM);
        while let Some(idx) = indexes.next().await {
            let t = idx?.tables;
            for seg in t.entries.iter().chain(&t.objects).chain(&t.history).chain(&t.removed) {
                refs.segment(store, seg.page).await?;
            }
        }
        let log = format!("{dir}log/");
        let tail = store.list_files(&log, Some(&format!("{newest:020}.json"))).await?;
        let mut commits = futures::stream::iter(tail)
            .map(|n| {
                let path = format!("{log}{n}");
                async move {
                    let b = store.get(&path).await?.ok_or_else(|| anyhow!("{path} vanished"))?;
                    serde_json::from_slice::<Commit>(&b).with_context(|| format!("reading {path}"))
                }
            })
            .buffered(READ_PARALLELISM);
        while let Some(c) = commits.next().await {
            for txn in c?.txns {
                for change in txn.changes {
                    if let Change::Set(s) = change
                        && let Some(content) = s.content
                    {
                        refs.content(store, &content).await?;
                    }
                }
            }
        }
        let uploads = format!("{dir}uploads/");
        for up in store.list_dirs(&uploads).await? {
            for name in store.list_files(&format!("{uploads}{up}/"), None).await? {
                if name == "upload.json" {
                    continue;
                }
                let path = format!("{uploads}{up}/{name}");
                // Completing or aborting the upload may delete it while this runs.
                let Some(b) = store.get(&path).await? else { continue };
                let part: Part = serde_json::from_slice(&b).with_context(|| format!("reading {path}"))?;
                refs.extents(&part.extents);
            }
        }
    }
    Ok(refs.hashes)
}

#[derive(Default)]
struct Refs {
    hashes: HashSet<ShardHash>,
    /// Pages already read.
    pages: HashSet<ShardHash>,
}

impl Refs {
    async fn page(&mut self, store: &Store, h: ShardHash) -> anyhow::Result<Option<Bytes>> {
        self.hashes.insert(h);
        if !self.pages.insert(h) {
            return Ok(None);
        }
        let path = format!("pages/{}", h.object_path());
        Ok(Some(store.get(&path).await?.ok_or_else(|| anyhow!("{path} is referenced but missing"))?))
    }

    async fn segment(&mut self, store: &Store, h: ShardHash) -> anyhow::Result<()> {
        let Some(b) = self.page(store, h).await? else { return Ok(()) };
        let seg: Segment = serde_json::from_slice(&b).with_context(|| format!("reading segment {h}"))?;
        for row in seg.rows {
            if let Some(c) = row.content {
                self.content(store, &c).await?;
            }
        }
        Ok(())
    }

    async fn content(&mut self, store: &Store, c: &ContentDescriptor) -> anyhow::Result<()> {
        match c {
            ContentDescriptor::Inline { extents } => self.extents(extents),
            ContentDescriptor::Tree { root, .. } => {
                let mut pending = vec![*root];
                while let Some(h) = pending.pop() {
                    let Some(b) = self.page(store, h).await? else { continue };
                    match serde_json::from_slice(&b).with_context(|| format!("reading manifest page {h}"))? {
                        ManifestPage::Leaf { extents } => self.extents(&extents),
                        ManifestPage::Node { children } => pending.extend(children.iter().map(|c| c.page)),
                    }
                }
            }
        }
        Ok(())
    }

    fn extents(&mut self, extents: &[Extent]) {
        self.hashes.extend(extents.iter().filter_map(|e| e.shard()));
    }
}
