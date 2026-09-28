// SPDX-License-Identifier: Apache-2.0
//! The collector and the writers' checks on a simulated bucket with a manual clock: a case for
//! each rule of format §12, and a seeded simulation of writers and a collector running at once
//! while requests are reordered and fail.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures::FutureExt;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use voidfs_core::ids::{ShardHash, Timestamp};
use voidfs_core::model::{Actor, Attrs, CommitGuard, Extent, Op};
use voidfs_core::names::Key;
use voidfs_core::ops::{self, Precondition};

use super::*;
use crate::clock::Clock;
use crate::pool::{CreateError, Drive, Pool};
use crate::store::{Fault, MemOp, MemStore, Store};

const SECOND: Duration = Duration::from_secs(1);
const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(86_400);

fn start() -> Timestamp {
    "2026-09-27T00:00:00Z".parse().unwrap()
}

struct Sim {
    mem: Arc<MemStore>,
    store: Store,
    clock: Clock,
}

impl Sim {
    fn new() -> Sim {
        let clock = Clock::manual(start());
        let mem = Arc::new(MemStore::new(clock.clone()));
        Sim { store: Store::Mem(mem.clone()), mem, clock }
    }

    async fn pool(&self) -> Arc<Pool> {
        Pool::open_with(self.store.clone(), 64 << 20, self.clock.clone()).await.unwrap()
    }

    /// The hashes of every shard and page in the bucket.
    async fn objects(&self) -> HashSet<ShardHash> {
        list_objects(&self.store).await.unwrap().into_iter().map(|o| o.hash).collect()
    }

    async fn has(&self, data: &[u8]) -> bool {
        self.store.exists(&format!("shards/{}", ShardHash::of(data).object_path())).await.unwrap()
    }

    /// Reads `key` of `drive` from the bucket, through a server that has just started.
    async fn read(&self, drive: &str, key: &str) -> Option<Vec<u8>> {
        let pool = self.pool().await;
        let d = pool.drive(drive)?;
        let s = d.snapshot();
        let r = s.record(&s.lookup(&Key::parse(key).ok()?)?)?.clone();
        let mut out = Vec::new();
        for e in pool.extents(r.content.as_ref()?).await.ok()? {
            match e {
                Extent::Shard { s, .. } => out.extend_from_slice(&self.store.get(&format!("shards/{}", s.object_path())).await.ok()??),
                Extent::Zero { z } => out.resize(out.len() + z as usize, 0),
            }
        }
        Some(out)
    }
}

#[tokio::test]
async fn an_external_pool_is_collected_without_conditional_writes() {
    let sim = Sim::new();
    // The bucket rejects conditional writes; with the external guard, none is sent (format §7.3).
    sim.mem.set_hook(Some(Arc::new(|op, _| futures::future::ready(if op == MemOp::PutNew { Fault::Fail } else { Fault::None }).boxed())));
    let pool = Pool::open_as(sim.store.clone(), 64 << 20, sim.clock.clone(), CommitGuard::External).await.unwrap();
    garbage(&pool, b"external").await;
    sim.clock.advance(SECOND);
    assert_eq!(step(&pool, &offline()).await.unwrap().outcome, Outcome::Deleted { objects: 1, bytes: 8 });
    assert!(!sim.has(b"external").await);
    assert!(sim.store.get(PENDING).await.unwrap().is_none());
}

fn offline() -> Options {
    Options { grace: Duration::ZERO, offline: true, ..Options::default() }
}

async fn put(pool: &Arc<Pool>, d: &Arc<Drive>, key: &str, data: &[u8]) -> anyhow::Result<()> {
    let e = voidfs_core::content::from_bytes(&Bytes::copy_from_slice(data), pool.params);
    pool.write_shards(&e.new_shards).await?;
    let desc = pool.describe(e.extents).await?;
    let key = key.to_owned();
    pool.commit(d, move |s| Ok(ops::put(s, &key, desc.clone(), Attrs::default(), Op::Put, &Precondition::default(), &Actor::system())?)).await.map_err(|e| anyhow!("{e:?}"))?;
    Ok(())
}

/// A drive with a file whose only reference goes away when the drive is hard-deleted.
async fn garbage(pool: &Arc<Pool>, data: &[u8]) {
    let d = pool.create_drive("doomed", None).await.unwrap();
    put(pool, &d, "file", data).await.unwrap();
    assert!(pool.hard_delete("doomed").await.unwrap());
}

#[tokio::test]
async fn offline_collection_reclaims_a_hard_deleted_drive() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let a = pool.create_drive("a", None).await.unwrap();
    let b = pool.create_drive("b", None).await.unwrap();
    put(&pool, &a, "only-a", b"alpha").await.unwrap();
    put(&pool, &a, "shared", b"shared").await.unwrap();
    put(&pool, &b, "shared", b"shared").await.unwrap();
    put(&pool, &b, "only-b", b"beta").await.unwrap();
    sim.clock.advance(SECOND);
    assert_eq!(step(&pool, &offline()).await.unwrap().outcome, Outcome::NothingToCollect);
    assert!(pool.hard_delete("a").await.unwrap());
    sim.clock.advance(SECOND);
    let r = step(&pool, &offline()).await.unwrap();
    assert_eq!(r.outcome, Outcome::Deleted { objects: 1, bytes: 5 });
    assert!(!sim.has(b"alpha").await);
    assert!(sim.has(b"shared").await);
    assert_eq!(sim.read("b", "shared").await.unwrap(), b"shared");
    assert_eq!(sim.read("b", "only-b").await.unwrap(), b"beta");
    assert!(sim.store.get(PENDING).await.unwrap().is_none());
}

#[tokio::test]
async fn a_run_waits_out_the_grace_period() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    garbage(&pool, b"old").await;
    let opts = Options::default();
    // Too new to be a candidate.
    assert_eq!(step(&pool, &opts).await.unwrap().outcome, Outcome::NothingToCollect);
    sim.clock.advance(2 * DAY);
    let r = step(&pool, &opts).await.unwrap();
    let Outcome::Proposed { candidates: 1, due, .. } = r.outcome else { panic!("{r}") };
    assert!(matches!(step(&pool, &opts).await.unwrap().outcome, Outcome::Waiting { due: d } if d == due));
    sim.clock.advance(23 * HOUR);
    assert!(matches!(step(&pool, &opts).await.unwrap().outcome, Outcome::Waiting { .. }));
    assert!(sim.has(b"old").await);
    sim.clock.advance(2 * HOUR);
    assert_eq!(step(&pool, &opts).await.unwrap().outcome, Outcome::Deleted { objects: 1, bytes: 3 });
    assert!(!sim.has(b"old").await);
}

#[tokio::test]
async fn the_grace_period_is_at_least_a_day_unless_offline() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    assert!(step(&pool, &Options { grace: HOUR, ..Options::default() }).await.is_err());
    assert!(step(&pool, &Options { grace: HOUR, offline: true, ..Options::default() }).await.is_ok());
}

#[tokio::test]
async fn a_dry_run_changes_nothing() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    garbage(&pool, b"old").await;
    sim.clock.advance(2 * DAY);
    let r = step(&pool, &Options { dry_run: true, ..Options::default() }).await.unwrap();
    assert_eq!(r.outcome, Outcome::WouldDelete { candidates: 1, bytes: 3 });
    assert!(sim.store.get(PENDING).await.unwrap().is_none());
    assert!(sim.has(b"old").await);
}

/// §12.4 option 3: a writer that uploads a candidate's bytes while the run waits rescues it.
#[tokio::test]
async fn an_upload_during_the_wait_rescues_a_candidate() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let b = pool.create_drive("b", None).await.unwrap();
    garbage(&pool, b"again").await;
    sim.clock.advance(2 * DAY);
    assert!(matches!(step(&pool, &Options::default()).await.unwrap().outcome, Outcome::Proposed { candidates: 1, .. }));
    put(&pool, &b, "copy", b"again").await.unwrap();
    sim.clock.advance(25 * HOUR);
    assert_eq!(step(&pool, &Options::default()).await.unwrap().outcome, Outcome::Deleted { objects: 0, bytes: 0 });
    assert_eq!(sim.read("b", "copy").await.unwrap(), b"again");
}

/// The race RFC 0002 closes: a writer uploads a candidate's bytes while the collector deletes.
/// The writer must see the run deleting, wait for it to end, and upload again.
#[tokio::test]
async fn an_upload_during_deletion_waits_for_the_run() {
    let sim = Sim::new();
    let writer = sim.pool().await;
    let b = writer.create_drive("b", None).await.unwrap();
    garbage(&writer, b"again").await;
    sim.clock.advance(2 * DAY);
    let collector = sim.pool().await;
    assert!(matches!(step(&collector, &Options::default()).await.unwrap().outcome, Outcome::Proposed { candidates: 1, .. }));
    sim.clock.advance(25 * HOUR);

    // Hold the collector at its delete, and count the writer's reads of the run record.
    let target = format!("shards/{}", ShardHash::of(b"again").object_path());
    let (paused, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
    let (held, reads) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    {
        let (paused, release, held, reads) = (paused.clone(), release.clone(), held.clone(), reads.clone());
        sim.mem.set_hook(Some(Arc::new(move |op, path| {
            if op == MemOp::Delete && path == target && !held.swap(true, Ordering::SeqCst) {
                paused.notify_one();
                let release = release.clone();
                return async move {
                    release.notified().await;
                    Fault::None
                }
                .boxed();
            }
            if op == MemOp::Get && path == PENDING && held.load(Ordering::SeqCst) {
                reads.fetch_add(1, Ordering::SeqCst);
            }
            futures::future::ready(Fault::None).boxed()
        })));
    }
    let c = tokio::spawn(async move { step(&collector, &Options::default()).await });
    paused.notified().await;
    let w = tokio::spawn(async move { put(&writer, &b, "copy", b"again").await });
    while reads.load(Ordering::SeqCst) < 3 {
        tokio::task::yield_now().await;
    }
    assert!(!w.is_finished(), "the writer must wait while the run deletes what it needs");
    release.notify_one();
    assert_eq!(c.await.unwrap().unwrap().outcome, Outcome::Deleted { objects: 1, bytes: 5 });
    w.await.unwrap().unwrap();
    sim.mem.set_hook(None);
    assert!(sim.has(b"again").await);
    assert_eq!(sim.read("b", "copy").await.unwrap(), b"again");
}

/// The hazard of the server before garbage collection: a shard it holds in its cache may have
/// been collected by another process since.
#[tokio::test]
async fn a_cached_shard_that_was_collected_is_uploaded_again() {
    let sim = Sim::new();
    let writer = sim.pool().await;
    let a = writer.create_drive("a", None).await.unwrap();
    let b = writer.create_drive("b", None).await.unwrap();
    put(&writer, &a, "file", b"cached").await.unwrap();
    writer.shard(&ShardHash::of(b"cached")).await.unwrap();
    assert!(writer.hard_delete("a").await.unwrap());
    sim.clock.advance(2 * DAY);
    let collector = sim.pool().await;
    step(&collector, &Options::default()).await.unwrap();
    sim.clock.advance(25 * HOUR);
    assert!(matches!(step(&collector, &Options::default()).await.unwrap().outcome, Outcome::Deleted { objects: 1, .. }));
    assert!(!sim.has(b"cached").await);
    put(&writer, &b, "file", b"cached").await.unwrap();
    assert!(sim.has(b"cached").await);
    assert_eq!(sim.read("b", "file").await.unwrap(), b"cached");
}

#[tokio::test]
async fn a_fork_keeps_what_it_shares_with_a_deleted_parent() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let a = pool.create_drive("a", None).await.unwrap();
    put(&pool, &a, "file", b"shared").await.unwrap();
    pool.create_drive("b", Some(&a)).await.unwrap();
    assert!(pool.hard_delete("a").await.unwrap());
    sim.clock.advance(SECOND);
    assert_eq!(step(&pool, &offline()).await.unwrap().outcome, Outcome::NothingToCollect);
    assert_eq!(sim.read("b", "file").await.unwrap(), b"shared");
}

/// A hard delete that fails part way must not leave the server serving the drive: content
/// referenced only through it is no longer protected, so a fork of it could reference objects
/// that have been collected.
#[tokio::test]
async fn a_drive_whose_hard_delete_failed_is_not_served() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let a = pool.create_drive("a", None).await.unwrap();
    put(&pool, &a, "file", b"only here").await.unwrap();
    let prefix = format!("drives/{}/", a.id);
    sim.mem.set_hook(Some(Arc::new(move |op, path| {
        let fault = if op == MemOp::Delete && path == prefix { Fault::FailAfter } else { Fault::None };
        futures::future::ready(fault).boxed()
    })));
    assert!(pool.hard_delete("a").await.is_err());
    sim.mem.set_hook(None);
    assert!(pool.drive("a").is_none());
    assert!(matches!(pool.create_drive("b", Some(&a)).await, Err(CreateError::NotFound)));
    sim.clock.advance(SECOND);
    assert_eq!(step(&pool, &offline()).await.unwrap().outcome, Outcome::Deleted { objects: 1, bytes: 9 });
}

#[tokio::test]
async fn soft_deleted_drives_are_roots_until_they_expire() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let a = pool.create_drive("a", None).await.unwrap();
    put(&pool, &a, "file", b"kept").await.unwrap();
    pool.soft_delete(&a).await.unwrap();
    sim.clock.advance(29 * DAY);
    assert_eq!(step(&pool, &offline()).await.unwrap().outcome, Outcome::NothingToCollect);
    assert!(sim.has(b"kept").await);
    sim.clock.advance(2 * DAY);
    let r = step(&pool, &offline()).await.unwrap();
    assert_eq!(r.expired_drives, vec![a.id.clone()]);
    assert_eq!(r.outcome, Outcome::Deleted { objects: 1, bytes: 4 });
    assert!(matches!(pool.undelete("a").await, Err(CreateError::NotFound)));
}

#[tokio::test]
async fn open_uploads_are_roots_until_they_go_stale() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let a = pool.create_drive("a", None).await.unwrap();
    let e = voidfs_core::content::from_bytes(&Bytes::from_static(b"part one"), pool.params);
    pool.write_shards(&e.new_shards).await.unwrap();
    let dir = format!("drives/{}/uploads/{}/", a.id, uuid::Uuid::new_v4());
    let upload = serde_json::json!({ "key": "big", "created": sim.clock.now(), "actor": { "kind": "system", "id": "test" }, "attrs": {} });
    let part = serde_json::json!({ "part": 1, "size": 8, "etag": "\"x\"", "extents": e.extents });
    sim.store.put(&format!("{dir}upload.json"), Bytes::from(upload.to_string())).await.unwrap();
    sim.store.put(&format!("{dir}00001.json"), Bytes::from(part.to_string())).await.unwrap();
    sim.clock.advance(6 * DAY);
    assert_eq!(step(&pool, &offline()).await.unwrap().outcome, Outcome::NothingToCollect);
    sim.clock.advance(2 * DAY);
    let r = step(&pool, &offline()).await.unwrap();
    assert_eq!(r.aborted_uploads, 1);
    assert_eq!(r.outcome, Outcome::Deleted { objects: 1, bytes: 8 });
}

#[tokio::test]
async fn manifest_trees_are_followed() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let a = pool.create_drive("a", None).await.unwrap();
    let mut extents = Vec::new();
    for i in 0..1100 {
        let e = voidfs_core::content::from_bytes(&Bytes::from(format!("shard {i}")), pool.params);
        pool.write_shards(&e.new_shards).await.unwrap();
        extents.extend(e.extents);
    }
    let desc = pool.describe(extents).await.unwrap();
    assert!(matches!(desc, voidfs_core::model::ContentDescriptor::Tree { .. }));
    pool.commit(&a, move |s| Ok(ops::put(s, "big", desc.clone(), Attrs::default(), Op::Put, &Precondition::default(), &Actor::system())?)).await.unwrap();
    sim.clock.advance(SECOND);
    assert_eq!(step(&pool, &offline()).await.unwrap().outcome, Outcome::NothingToCollect);
    assert!(pool.hard_delete("a").await.unwrap());
    sim.clock.advance(SECOND);
    let r = step(&pool, &offline()).await.unwrap();
    assert!(matches!(r.outcome, Outcome::Deleted { objects, .. } if objects > 1100), "{r}");
    assert!(sim.objects().await.is_empty());
}

#[tokio::test]
async fn checkpoints_and_history_are_roots() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let a = pool.create_drive("a", None).await.unwrap();
    for i in 0..1001 {
        put(&pool, &a, "counter", format!("tick {i}").as_bytes()).await.unwrap();
    }
    assert!(sim.store.exists(&format!("drives/{}/checkpoints/{:020}.json", a.id, 1000)).await.unwrap());
    sim.clock.advance(SECOND);
    assert_eq!(step(&pool, &offline()).await.unwrap().outcome, Outcome::NothingToCollect);
    assert!(sim.has(b"tick 0").await, "old versions stay");
    assert_eq!(sim.read("a", "counter").await.unwrap(), b"tick 1000");
}

#[tokio::test]
async fn a_run_left_marking_is_replaced_once_stale() {
    let sim = Sim::new();
    let pool = sim.pool().await;
    let stale = serde_json::json!({ "format": 1, "run": "crashed", "phase": "marking", "grace": 86400 });
    sim.store.put(PENDING, Bytes::from(stale.to_string())).await.unwrap();
    assert_eq!(step(&pool, &Options::default()).await.unwrap().outcome, Outcome::Busy);
    sim.clock.advance(25 * HOUR);
    let r = step(&pool, &Options::default()).await.unwrap();
    assert_eq!(r.outcome, Outcome::NothingToCollect);
    assert_ne!(r.run.as_deref(), Some("crashed"));
    assert!(sim.store.get(PENDING).await.unwrap().is_none());
}

/// The collector's clock runs two hours ahead of the bucket's; the bucket's decides.
#[tokio::test]
async fn the_buckets_clock_decides_when_phase_2_is_due() {
    let bucket = Clock::manual(start());
    let server = Clock::manual(start());
    server.advance(2 * HOUR);
    let mem = Arc::new(MemStore::new(bucket.clone()));
    let store = Store::Mem(mem);
    let pool = Pool::open_with(store.clone(), 1 << 20, server.clone()).await.unwrap();
    garbage(&pool, b"old").await;
    for c in [&bucket, &server] {
        c.advance(2 * DAY);
    }
    assert!(matches!(step(&pool, &Options::default()).await.unwrap().outcome, Outcome::Proposed { .. }));
    for c in [&bucket, &server] {
        c.advance(23 * HOUR);
    }
    assert!(matches!(step(&pool, &Options::default()).await.unwrap().outcome, Outcome::Waiting { .. }));
    let rec: PendingRecord = serde_json::from_slice(&store.get(PENDING).await.unwrap().unwrap()).unwrap();
    assert_eq!(rec.phase, Phase::Waiting);
    for c in [&bucket, &server] {
        c.advance(2 * HOUR);
    }
    assert!(matches!(step(&pool, &Options::default()).await.unwrap().outcome, Outcome::Deleted { objects: 1, .. }));
}

// ---------------------------------------------------------------------------------------------
// Simulation

/// How many different content blocks the simulation writes: few enough that the same shards keep
/// coming back, enough that some are often referenced only by deleted drives.
const BLOCKS: usize = 24;

fn block_bytes(i: usize) -> Vec<u8> {
    format!("block {i}").into_bytes()
}
const KEYS: [&str; 3] = ["a", "b", "dir/c"];

struct World {
    rng: Mutex<StdRng>,
    /// For peeking at the run record, as an adversary would.
    mem: Arc<MemStore>,
    /// Set from the collector's listing of the objects until its step ends: the window in which
    /// it may delete what it listed.
    window: AtomicBool,
    /// Candidates written in the window.
    aimed: AtomicUsize,
    ops_ok: AtomicUsize,
    ops_failed: AtomicUsize,
    deleted: AtomicUsize,
    /// Soft-deleted drives and when.
    soft_deleted: Mutex<HashMap<String, Timestamp>>,
    drives_made: AtomicUsize,
}

impl World {
    fn pick(&self, n: usize) -> usize {
        self.rng.lock().unwrap().random_range(0..n)
    }

    /// A block to write. Re-uploading a candidate's bytes after the collector has listed the
    /// objects is where the races are (RFC 0002), so in that window, mostly a candidate.
    /// Outside it, any block: re-uploading candidates earlier only saves them.
    fn block(&self) -> Vec<u8> {
        if !self.window.load(Ordering::SeqCst) {
            return block_bytes(self.pick(BLOCKS));
        }
        let listed: Vec<Vec<u8>> = self
            .mem
            .peek(PENDING)
            .and_then(|b| serde_json::from_slice::<PendingRecord>(&b).ok())
            .map(|r| (0..BLOCKS).map(block_bytes).filter(|b| r.candidates.contains(&ShardHash::of(b))).collect())
            .unwrap_or_default();
        if !listed.is_empty() && self.pick(10) < 9 {
            self.aimed.fetch_add(1, Ordering::SeqCst);
            return listed[self.pick(listed.len())].clone();
        }
        block_bytes(self.pick(BLOCKS))
    }
}

/// Upload-staging records the simulation writes, as `object.rs` does (format §11).
async fn start_upload(w: &Arc<Pool>, d: &Arc<Drive>, data: &[u8], now: Timestamp) -> anyhow::Result<()> {
    let e = voidfs_core::content::from_bytes(&Bytes::copy_from_slice(data), w.params);
    w.write_shards(&e.new_shards).await?;
    let dir = format!("drives/{}/uploads/{}/", d.id, uuid::Uuid::new_v4());
    let upload = serde_json::json!({ "key": "upload", "created": now, "actor": { "kind": "system", "id": "sim" }, "attrs": {} });
    w.store.put(&format!("{dir}upload.json"), Bytes::from(upload.to_string())).await?;
    let part = serde_json::json!({ "part": 1, "size": data.len(), "etag": "\"x\"", "extents": e.extents });
    w.store.put(&format!("{dir}00001.json"), Bytes::from(part.to_string())).await?;
    Ok(())
}

async fn finish_upload(w: &Arc<Pool>, d: &Arc<Drive>, complete: bool) -> anyhow::Result<()> {
    let uploads = format!("drives/{}/uploads/", d.id);
    let Some(up) = w.store.list_dirs(&uploads).await?.into_iter().next() else { return Ok(()) };
    let dir = format!("{uploads}{up}/");
    if complete {
        let Some(b) = w.store.get(&format!("{dir}00001.json")).await? else { return Ok(()) };
        let part: Part = serde_json::from_slice(&b)?;
        let desc = w.describe(part.extents).await?;
        w.commit(d, move |s| Ok(ops::put(s, "upload", desc.clone(), Attrs::default(), Op::Put, &Precondition::default(), &Actor::system())?)).await.map_err(|e| anyhow!("{e:?}"))?;
    }
    w.store.delete_prefix(&dir).await
}

/// One random operation by a writer. Failures are expected: requests fail at random.
async fn operate(w: &Arc<Pool>, world: &World, clock: &Clock) -> anyhow::Result<()> {
    let live = w.list_drives();
    if live.is_empty() {
        let name = format!("d{}", world.drives_made.fetch_add(1, Ordering::SeqCst));
        w.create_drive(&name, None).await.map_err(|e| anyhow!("{e}"))?;
        return Ok(());
    }
    let d = live[world.pick(live.len())].clone();
    let block = world.block();
    let key = KEYS[world.pick(KEYS.len())];
    match world.pick(100) {
        0..40 => put(w, &d, key, &block).await?,
        40..48 => {
            w.commit(&d, move |s| match ops::delete(s, key, &Precondition::default(), &Actor::system())? {
                Some(t) => Ok(t),
                None => Err(crate::pool::CommitError::Op(voidfs_core::ops::OpError::NoSuchKey)),
            })
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        }
        48..58 => {
            // A copy from another drive references content without uploading it (§12.4, 1).
            let src = live[world.pick(live.len())].snapshot();
            let Some(r) = Key::parse(key).ok().and_then(|k| src.lookup(&k)).and_then(|o| src.record(&o).cloned()) else { return Ok(()) };
            let Some(content) = r.content else { return Ok(()) };
            let dst = KEYS[world.pick(KEYS.len())];
            w.commit(&d, move |s| Ok(ops::put(s, dst, content.clone(), Attrs::default(), Op::Copy, &Precondition::default(), &Actor::system())?)).await.map_err(|e| anyhow!("{e:?}"))?;
        }
        58..63 if world.drives_made.load(Ordering::SeqCst) < 8 => {
            let name = format!("d{}", world.drives_made.fetch_add(1, Ordering::SeqCst));
            w.create_drive(&name, Some(&d)).await.map_err(|e| anyhow!("{e}"))?;
        }
        63..68 => {
            w.soft_delete(&d).await?;
            world.soft_deleted.lock().unwrap().insert(d.alias.clone(), clock.now());
        }
        68..72 => {
            // Undelete only well inside the window: racing expiry is not supported (RFC 0002).
            let recent: Vec<String> = world.soft_deleted.lock().unwrap().iter().filter(|(_, t)| clock.now().datetime() - t.datetime() < chrono::Duration::days(20)).map(|(n, _)| n.clone()).collect();
            if let Some(name) = recent.first() {
                world.soft_deleted.lock().unwrap().remove(name);
                let _ = w.undelete(name).await;
            }
        }
        72..80 => {
            w.hard_delete(&d.alias).await?;
        }
        80..88 => start_upload(w, &d, &block, clock.now()).await?,
        88..92 => finish_upload(w, &d, true).await?,
        92..95 => finish_upload(w, &d, false).await?,
        _ => {
            let s = d.snapshot();
            if let Some(r) = Key::parse(key).ok().and_then(|k| s.lookup(&k)).and_then(|o| s.record(&o).cloned())
                && let Some(c) = r.content
            {
                for e in w.extents(&c).await? {
                    if let Extent::Shard { s, .. } = e {
                        w.shard(&s).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Everything referenced must exist.
async fn check_safety(sim: &Sim, pool: &Pool) -> Result<HashSet<ShardHash>, String> {
    let refs = referenced(pool).await.map_err(|e| format!("computing the references: {e:#}"))?;
    let objects = sim.objects().await;
    let missing: Vec<_> = refs.difference(&objects).collect();
    if !missing.is_empty() {
        return Err(format!("referenced but missing: {missing:?}"));
    }
    Ok(refs)
}

async fn simulate(seed: u64, rounds: usize) -> Result<(), String> {
    let sim = Sim::new();
    let writer = sim.pool().await;
    let collector = sim.pool().await;
    let world = Arc::new(World {
        rng: Mutex::new(StdRng::seed_from_u64(seed)),
        mem: sim.mem.clone(),
        window: AtomicBool::new(false),
        aimed: AtomicUsize::new(0),
        ops_ok: AtomicUsize::new(0),
        ops_failed: AtomicUsize::new(0),
        deleted: AtomicUsize::new(0),
        soft_deleted: Mutex::new(HashMap::new()),
        drives_made: AtomicUsize::new(0),
    });
    let faults = Arc::new(AtomicBool::new(true));
    {
        let (world, faults) = (world.clone(), faults.clone());
        sim.mem.set_hook(Some(Arc::new(move |op, path| {
            // Only the collector deletes shards and pages; hold its deletes longer, so writers
            // get between its listing and its deletes.
            let collecting = op == MemOp::Delete && (path.starts_with("shards/") || path.starts_with("pages/"));
            if op == MemOp::List && path == "shards/" {
                world.window.store(true, Ordering::SeqCst);
            }
            let (yields, fault) = {
                let mut r = world.rng.lock().unwrap();
                let fault = match r.random_range(0..100) {
                    _ if !faults.load(Ordering::SeqCst) => Fault::None,
                    0..2 => Fault::Fail,
                    2..4 => Fault::FailAfter,
                    _ => Fault::None,
                };
                (if collecting { r.random_range(20..200) } else { r.random_range(0..4) }, fault)
            };
            async move {
                for _ in 0..yields {
                    tokio::task::yield_now().await;
                }
                fault
            }
            .boxed()
        })));
    }
    const JUMPS: [Duration; 8] = [Duration::ZERO, Duration::from_secs(60), Duration::from_secs(600), Duration::from_secs(2 * 3600), Duration::from_secs(13 * 3600), Duration::from_secs(25 * 3600), Duration::from_secs(36 * 3600), Duration::from_secs(31 * 86_400)];
    for round in 0..rounds {
        let mut tasks = Vec::new();
        // Writers keep going while the collector runs, so that they are active in every part of
        // its run, not only before it reads the pool.
        let collecting = Arc::new(AtomicBool::new(true));
        for _ in 0..3 {
            let (w, world, clock, collecting) = (writer.clone(), world.clone(), sim.clock.clone(), collecting.clone());
            tasks.push(tokio::spawn(async move {
                for i in 0..40 {
                    if i >= 4 && !collecting.load(Ordering::SeqCst) {
                        break;
                    }
                    let counter = if operate(&w, &world, &clock).await.is_ok() { &world.ops_ok } else { &world.ops_failed };
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        let (c, clock, world2, done) = (collector.clone(), sim.clock.clone(), world.clone(), collecting.clone());
        tasks.push(tokio::spawn(async move {
            // Small steps of the clock while requests are in flight.
            if world2.pick(3) == 0 {
                tokio::task::yield_now().await;
                clock.advance(Duration::from_secs(world2.pick(600) as u64));
            }
            if let Ok(Report { outcome: Outcome::Deleted { objects, .. }, .. }) = step(&c, &Options::default()).await {
                world2.deleted.fetch_add(objects, Ordering::SeqCst);
            }
            done.store(false, Ordering::SeqCst);
            world2.window.store(false, Ordering::SeqCst);
        }));
        for t in tasks {
            t.await.map_err(|e| format!("a task panicked: {e}"))?;
        }
        let jump = if world.pick(20) == 0 { JUMPS[7] } else { JUMPS[world.pick(7)] };
        sim.clock.advance(jump);
        faults.store(false, Ordering::SeqCst);
        check_safety(&sim, &writer).await.map_err(|e| format!("after round {round}: {e}"))?;
        faults.store(true, Ordering::SeqCst);
    }
    // With the writers stopped, collection must leave nothing unreferenced. Each step a day
    // apart ages more uploads past their 7 days, so it takes a few runs to settle.
    faults.store(false, Ordering::SeqCst);
    let mut steps = Vec::new();
    let mut settled = false;
    for _ in 0..20 {
        sim.clock.advance(25 * HOUR);
        let r = step(&collector, &Options::default()).await.map_err(|e| format!("final collection: {e:#}"))?;
        steps.push(format!("{}: {r}", sim.clock.now()));
        if let Outcome::Deleted { objects, .. } = r.outcome {
            world.deleted.fetch_add(objects, Ordering::SeqCst);
        }
        if r.outcome == Outcome::NothingToCollect {
            settled = true;
            break;
        }
    }
    if !settled {
        return Err(format!("collection did not settle:\n  {}", steps.join("\n  ")));
    }
    let refs = check_safety(&sim, &writer).await.map_err(|e| format!("at the end: {e}"))?;
    let leftover: Vec<String> = list_objects(&sim.store)
        .await
        .unwrap()
        .into_iter()
        .filter(|o| !refs.contains(&o.hash))
        .map(|o| format!("{} (modified {:?}, block {:?})", o.path, o.modified, (0..BLOCKS).find(|&i| ShardHash::of(&block_bytes(i)) == o.hash)))
        .collect();
    if !leftover.is_empty() {
        let pending = sim.store.get(PENDING).await.unwrap().map(|b| String::from_utf8_lossy(&b).into_owned());
        return Err(format!("unreferenced objects survived collection: {leftover:?}\nfinal steps:\n  {}\npending: {pending:?}", steps.join("\n  ")));
    }
    // Every drive in the bucket still loads.
    let fresh = sim.pool().await;
    for dir in sim.store.list_dirs("drives/").await.unwrap() {
        let id: voidfs_core::ids::DriveId = dir.parse().unwrap();
        if sim.store.exists(&format!("drives/{id}/drive.json")).await.unwrap() && fresh.drive_by_id(&id).is_none() {
            return Err(format!("drive {id} no longer loads"));
        }
    }
    eprintln!(
        "seed {seed}: {} operations succeeded, {} failed, {} objects collected, {} candidates re-uploaded while collecting, {} drives made",
        world.ops_ok.load(Ordering::SeqCst),
        world.ops_failed.load(Ordering::SeqCst),
        world.deleted.load(Ordering::SeqCst),
        world.aimed.load(Ordering::SeqCst),
        world.drives_made.load(Ordering::SeqCst)
    );
    Ok(())
}

/// `VOIDFS_GC_SIM_SEEDS` sets how many seeds to run, and `VOIDFS_GC_SIM_SEED` runs one seed. The
/// default of 256 catches each rule of RFC 0002 when it is removed. Ids are random, so which seed
/// does varies from run to run: with group commit, the unchecked cache within the first three
/// seeds, the recheck after a rescue at seed 32 or 186 in four runs of four, and the `deleting`
/// mark at seed 186.
#[tokio::test]
async fn simulation() {
    let one: Option<u64> = std::env::var("VOIDFS_GC_SIM_SEED").ok().and_then(|s| s.parse().ok());
    let seeds: u64 = std::env::var("VOIDFS_GC_SIM_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(256);
    let list: Vec<u64> = match one {
        Some(s) => vec![s],
        None => (0..seeds).collect(),
    };
    for seed in list {
        if let Err(e) = simulate(seed, 16).await {
            panic!("seed {seed}: {e}\nrerun with VOIDFS_GC_SIM_SEED={seed}");
        }
    }
}
