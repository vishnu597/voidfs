// SPDX-License-Identifier: Apache-2.0
//! Recovering local mount state: staging bytes that no record or handle needs are removed,
//! overwritten staging bytes are compacted, a lost stage leaves the rest of the drive usable,
//! and a save whose reply was lost is known as the client's own.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{Fault, Proxy, client_for};
use rusqlite::Connection;
use voidfs_client::mount::{ConflictSide, FsError, Session, StagingConfig, Sync};
use voidfs_client::{ApiFetcher, Cache, CacheConfig, Connectivity, Queue, QueueConfig, Scope, Store};
use voidfs_sdk::{Client, Config, PutOptions, ReadOptions};
use voidfs_server::test_server::TestServer;

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(1 << 50) }

fn staging(compact_garbage: u64) -> StagingConfig {
    StagingConfig { min_free_bytes: 0, free_space: Some(plenty), quiet_period: None, compact_garbage }
}

async fn open(dir: &Path, client: &Client) -> (Arc<Store>, Cache, Queue) {
    let store = Arc::new(Store::open(dir).unwrap());
    let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(client.clone())), CacheConfig { min_free_bytes: 0, free_space: Some(plenty), ..Default::default() }).await.unwrap();
    let queue = Queue::open(store.clone(), client.clone(), QueueConfig { retry_max: Duration::from_millis(50), ..Default::default() }).await.unwrap();
    queue.pause(Scope::All).await.unwrap();
    (store, cache, queue)
}

struct Fixture { _server: TestServer, proxy: Proxy, remote: Client, client: Client, state: tempfile::TempDir, store: Arc<Store>, cache: Cache, queue: Queue }

impl Fixture {
    async fn new() -> Self { Self::with(Config::default()).await }

    async fn with(config: Config) -> Self {
        let server = TestServer::start().await.unwrap();
        let proxy = Proxy::start(&server.endpoint).await;
        let remote = client_for(&server.endpoint, Config::default());
        let client = client_for(&proxy.endpoint, config);
        remote.create_drive("drv", Default::default()).await.unwrap();
        let state = tempfile::tempdir().unwrap();
        let (store, cache, queue) = open(state.path(), &client).await;
        Self { _server: server, proxy, remote, client, state, store, cache, queue }
    }

    /// A writer that has queued what the previous one left unflushed.
    async fn session(&self, cfg: StagingConfig) -> Session {
        let session = self.writer(cfg).await;
        session.recovered().await;
        session
    }

    async fn writer(&self, cfg: StagingConfig) -> Session {
        Session::new_writable_with_config(self.store.clone(), self.client.clone(), self.cache.clone(), self.queue.clone(), "drv", Connectivity::default(), cfg).await.unwrap()
    }

    /// Publishes what is queued, then pauses again so later edits stay local.
    async fn publish(&self) {
        self.queue.resume(Scope::All).await.unwrap();
        self.queue.settle().await;
        self.queue.pause(Scope::All).await.unwrap();
    }

    /// Stops as a crash would leave the session: handles still open, nothing flushed.
    async fn restart(self, session: Session) -> Self {
        drop(session);
        self.queue.close().await;
        self.cache.settle().await;
        let Self { _server, proxy, remote, client, state, store, cache, queue } = self;
        drop((store, cache, queue));
        let (store, cache, queue) = open(state.path(), &client).await;
        Self { _server, proxy, remote, client, state, store, cache, queue }
    }

    fn files(&self, dir: &str) -> Vec<PathBuf> {
        let mut files = match std::fs::read_dir(self.state.path().join(dir)) {
            Ok(entries) => entries.map(|e| e.unwrap().path()).filter(|p| p.is_file()).collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("{e}"),
        };
        files.sort();
        files
    }

    fn staged(&self, ino: u64) -> Vec<PathBuf> {
        self.files("mount-stage").into_iter().filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with(&format!("{ino}-"))).collect()
    }

    fn db(&self) -> Connection { Connection::open(self.state.path().join("state.sqlite")).unwrap() }
}

async fn new_file(ns: &Session, name: &str, bytes: &'static [u8]) -> u64 {
    let ino = ns.create(ns.root(), name, 0o644).await.unwrap().ino;
    let fh = ns.open(ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(bytes)).await.unwrap();
    ns.close(fh).await.unwrap();
    ino
}

/// Another Mac's edit, after this one bound its version for writing.
async fn edit_elsewhere(f: &Fixture, key: &str, bytes: &'static [u8]) {
    let current = f.remote.head_object("drv", key, ReadOptions::default()).await.unwrap().version_id;
    f.remote.put_object("drv", key, Bytes::from_static(bytes), PutOptions { if_version: Some(current), ..Default::default() }).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clean_publication_removes_its_staging_file_once_no_handle_reads_it() {
    let f = Fixture::new().await;
    let ns = f.session(staging(u64::MAX)).await;
    let a = ns.create(ns.root(), "a", 0o644).await.unwrap().ino;
    let writer = ns.open(a, true).await.unwrap();
    ns.write(writer, 0, Bytes::from_static(b"first")).await.unwrap();
    let reader = ns.open(a, false).await.unwrap();
    ns.close(writer).await.unwrap();
    let b = new_file(&ns, "b", b"other").await;
    assert_eq!((f.staged(a).len(), f.staged(b).len()), (1, 1));
    f.publish().await;
    assert_eq!((ns.getattr(a).await.unwrap().sync, ns.getattr(b).await.unwrap().sync), (Sync::Saved, Sync::Saved));
    assert!(f.staged(b).is_empty(), "no handle reads b's published bytes from its stage");
    assert_eq!(f.staged(a).len(), 1, "an earlier handle still reads a's");
    assert_eq!(ns.read(reader, 0, 16).await.unwrap(), Bytes::from_static(b"first"));
    ns.close(reader).await.unwrap();
    assert!(f.staged(a).is_empty(), "the last handle's close lets the retained bytes go");
    let fresh = ns.open(a, false).await.unwrap();
    assert_eq!(ns.read(fresh, 0, 16).await.unwrap(), Bytes::from_static(b"first"));
    assert_eq!(f.remote.get_object("drv", "a", ReadOptions::default()).await.unwrap().body, Bytes::from_static(b"first"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflicted_stage_keeps_its_staging_file() {
    let f = Fixture::new().await;
    let ns = f.session(staging(u64::MAX)).await;
    let ino = new_file(&ns, "doc", b"one").await;
    f.publish().await;
    let fh = ns.open(ino, true).await.unwrap();
    edit_elsewhere(&f, "doc", b"remote").await;
    ns.write(fh, 0, Bytes::from_static(b"two")).await.unwrap();
    ns.close(fh).await.unwrap();
    f.publish().await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    assert_eq!(f.staged(ino).len(), 1, "the local side of a conflict stays staged");
    let fh = ns.open(ino, false).await.unwrap();
    assert_eq!(ns.read(fh, 0, 16).await.unwrap(), Bytes::from_static(b"two"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_collects_unrecorded_staging_files_of_its_own_drive_only() {
    let f = Fixture::new().await;
    f.remote.create_drive("other", Default::default()).await.unwrap();
    let ns = f.session(staging(u64::MAX)).await;
    let ino = ns.create(ns.root(), "kept", 0o644).await.unwrap().ino;
    let fh = ns.open(ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"kept")).await.unwrap();
    let f = f.restart(ns).await;
    let other = Session::new(f.store.clone(), f.client.clone(), f.cache.clone(), "other", Connectivity::default()).await.unwrap();
    let other_root = other.root();
    drop(other);
    let recorded = f.staged(ino);
    assert_eq!(recorded.len(), 1);
    let dir = f.state.path().join("mount-stage");
    let stray = [format!("{ino}-00"), format!("{ino}-assembly-11"), "snapshot-22".into()];
    let foreign = [format!("{other_root}-33"), "notes".into()];
    for name in stray.iter().chain(&foreign) { std::fs::write(dir.join(name), b"bytes").unwrap(); }
    let ns = f.session(staging(u64::MAX)).await;
    let mut expected = foreign.iter().map(|name| dir.join(name)).chain(recorded).collect::<Vec<_>>();
    expected.sort();
    assert_eq!(f.files("mount-stage"), expected, "only this drive's unrecorded stages and legacy assemblies go");
    let fh = ns.open(ino, false).await.unwrap();
    assert_eq!(ns.read(fh, 0, 16).await.unwrap(), Bytes::from_static(b"kept"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flush_compacts_overwritten_staging_bytes_without_changing_what_reads_see() {
    let f = Fixture::new().await;
    let ns = f.session(staging(0)).await;
    let ino = ns.create(ns.root(), "db", 0o644).await.unwrap().ino;
    let writer = ns.open(ino, true).await.unwrap();
    for n in 0..10u8 { ns.write(writer, 0, Bytes::from(vec![n; 4096])).await.unwrap(); }
    ns.write(writer, 1 << 20, Bytes::from_static(b"tail")).await.unwrap();
    let reader = ns.open(ino, false).await.unwrap();
    let before = f.staged(ino);
    assert_eq!(before.len(), 1);
    assert_eq!(std::fs::metadata(&before[0]).unwrap().len(), 10 * 4096 + 4);
    ns.fsync(writer).await.unwrap();
    let after = f.staged(ino);
    assert_eq!(after.len(), 1);
    assert_ne!(after, before, "a compacted stage is a new file, so a crash never leaves a half-rewritten one");
    assert_eq!(std::fs::metadata(&after[0]).unwrap().len(), 4096 + 4, "only the bytes extents use remain");
    let recorded: String = f.db().query_row("SELECT path FROM mount_staged WHERE ino=?1", [ino], |r| r.get(0)).unwrap();
    assert_eq!(Path::new(&recorded).file_name(), after[0].file_name());
    for fh in [writer, reader] {
        assert_eq!(ns.read(fh, 0, 4096).await.unwrap(), Bytes::from(vec![9u8; 4096]));
        assert_eq!(ns.read(fh, 4096, 4096).await.unwrap(), Bytes::from(vec![0u8; 4096]));
        assert_eq!(ns.read(fh, 1 << 20, 16).await.unwrap(), Bytes::from_static(b"tail"));
    }
    ns.write(writer, 2, Bytes::from_static(b"!!")).await.unwrap();
    let f = f.restart(ns).await;
    let ns = f.session(staging(0)).await;
    let fh = ns.open(ino, true).await.unwrap();
    let mut expected = vec![9u8; 4096];
    expected[2..4].copy_from_slice(b"!!");
    assert_eq!(ns.read(fh, 0, 4096).await.unwrap(), Bytes::from(expected.clone()));
    assert_eq!(ns.read(fh, 1 << 20, 16).await.unwrap(), Bytes::from_static(b"tail"));
    ns.close(fh).await.unwrap();
    f.publish().await;
    let body = f.remote.get_object("drv", "db", ReadOptions::default()).await.unwrap().body;
    assert_eq!((body.len(), &body[..4096], &body[1 << 20..]), ((1 << 20) + 4, &expected[..], &b"tail"[..]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn garbage_under_the_threshold_or_the_bytes_in_use_is_left_in_place() {
    let f = Fixture::new().await;
    let ns = f.session(staging(1 << 20)).await;
    let ino = ns.create(ns.root(), "small", 0o644).await.unwrap().ino;
    let fh = ns.open(ino, true).await.unwrap();
    for n in 0..4u8 { ns.write(fh, 0, Bytes::from(vec![n; 4096])).await.unwrap(); }
    ns.fsync(fh).await.unwrap();
    assert_eq!(std::fs::metadata(&f.staged(ino)[0]).unwrap().len(), 4 * 4096, "12 KiB of garbage is under the 1 MiB threshold");
    let f = f.restart(ns).await;
    let ns = f.session(staging(0)).await;
    let other = ns.create(ns.root(), "mostly-live", 0o644).await.unwrap().ino;
    let fh = ns.open(other, true).await.unwrap();
    ns.write(fh, 0, Bytes::from(vec![1u8; 8192])).await.unwrap();
    ns.write(fh, 0, Bytes::from(vec![2u8; 4096])).await.unwrap();
    ns.fsync(fh).await.unwrap();
    assert_eq!(std::fs::metadata(&f.staged(other)[0]).unwrap().len(), 3 * 4096, "4 KiB of garbage under 8 KiB in use is not worth a rewrite");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_stage_errors_its_own_file_and_leaves_the_drive_usable() {
    let f = Fixture::new().await;
    let ns = f.session(staging(u64::MAX)).await;
    let mut inodes = Vec::new();
    for (name, bytes) in [("missing", &b"lost"[..]), ("short", &b"cut short"[..]), ("kept", &b"kept"[..])] {
        let ino = ns.create(ns.root(), name, 0o644).await.unwrap().ino;
        let fh = ns.open(ino, true).await.unwrap();
        ns.write(fh, 0, Bytes::copy_from_slice(bytes)).await.unwrap();
        inodes.push(ino);
    }
    let f = f.restart(ns).await;
    // As a power failure can leave them: the records committed, the bytes never written.
    std::fs::remove_file(&f.staged(inodes[0])[0]).unwrap();
    std::fs::File::options().write(true).open(&f.staged(inodes[1])[0]).unwrap().set_len(3).unwrap();
    let ns = f.session(staging(u64::MAX)).await;
    for &ino in &inodes[..2] {
        assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Error);
        assert!(matches!(ns.open(ino, false).await, Err(FsError::Io(_))), "its other bytes must not stand in for the lost ones");
        assert!(matches!(ns.open(ino, true).await, Err(FsError::Io(_))));
    }
    let fh = ns.open(inodes[2], true).await.unwrap();
    assert_eq!(ns.read(fh, 0, 16).await.unwrap(), Bytes::from_static(b"kept"));
    ns.write(fh, 4, Bytes::from_static(b"!")).await.unwrap();
    ns.close(fh).await.unwrap();
    ns.unlink(ns.root(), "missing").await.unwrap();
    f.publish().await;
    assert_eq!(f.remote.get_object("drv", "kept", ReadOptions::default()).await.unwrap().body, Bytes::from_static(b"kept!"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_snapshots_no_conflict_records_are_removed() {
    let f = Fixture::new().await;
    let ns = f.session(staging(u64::MAX)).await;
    let ino = new_file(&ns, "doc", b"local one").await;
    f.publish().await;
    f.db().execute_batch("CREATE TRIGGER refuse_conflict BEFORE INSERT ON mount_conflicts BEGIN SELECT RAISE(ABORT, 'test failure'); END").unwrap();
    let fh = ns.open(ino, true).await.unwrap();
    edit_elsewhere(&f, "doc", b"remote edit").await;
    ns.write(fh, 0, Bytes::from_static(b"local two")).await.unwrap();
    ns.close(fh).await.unwrap();
    f.queue.resume(Scope::All).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.queue.status().await.unwrap().items.iter().any(|i| i.error.as_deref().is_some_and(|e| e.contains("recording publication completion"))) {
        assert!(Instant::now() < deadline, "the conflict's completion never failed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(f.files("mount-conflicts").is_empty(), "snapshots of a completion that failed belong to no conflict");
    f.db().execute_batch("DROP TRIGGER refuse_conflict").unwrap();
    f.queue.settle().await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    assert_eq!(f.files("mount-conflicts").len(), 2, "one local and one remote snapshot, however many attempts captured them");
    std::fs::write(f.state.path().join("mount-conflicts").join("stray"), b"captured before a crash").unwrap();
    let f = f.restart(ns).await;
    assert_eq!(f.files("mount-conflicts").len(), 2);
    let ns = f.session(staging(u64::MAX)).await;
    assert_eq!(ns.read_conflict(ino, ConflictSide::Local, 0, 64).await.unwrap(), Bytes::from_static(b"local two"));
    assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 0, 64).await.unwrap(), Bytes::from_static(b"remote edit"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_save_whose_reply_was_lost_is_known_as_its_own() {
    let f = Fixture::with(Config { timeout: Duration::from_millis(500), ..Default::default() }).await;
    let ns = f.session(staging(u64::MAX)).await;
    let ino = new_file(&ns, "new", b"made once").await;
    // The create's put lands, but its answer comes after the client gave up on it.
    f.proxy.fault(Fault::Hang(Duration::from_secs(2)));
    f.publish().await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Saved, "{:?}", f.queue.status().await.unwrap().items);
    assert!(f.proxy.seen().iter().filter(|r| r.starts_with("PUT /drv/new")).count() >= 2, "{:?}", f.proxy.seen());
    assert_eq!(f.remote.list_versions("drv", "new", true).await.unwrap().len(), 2, "the create and the content landed once each");
    assert_eq!(f.remote.get_object("drv", "new", ReadOptions::default()).await.unwrap().body, Bytes::from_static(b"made once"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unlinked_files_stage_goes_with_its_last_handle_or_a_restart() {
    let f = Fixture::new().await;
    let ns = f.session(staging(u64::MAX)).await;
    let ino = ns.create(ns.root(), "gone", 0o644).await.unwrap().ino;
    let writer = ns.open(ino, true).await.unwrap();
    ns.write(writer, 0, Bytes::from_static(b"before")).await.unwrap();
    let reader = ns.open(ino, false).await.unwrap();
    ns.unlink(ns.root(), "gone").await.unwrap();
    // Its create and removal publish while both handles stay open, then it is written again.
    f.publish().await;
    ns.write(writer, 0, Bytes::from_static(b"after!")).await.unwrap();
    ns.close(writer).await.unwrap();
    assert_eq!(f.staged(ino).len(), 1, "an open handle still reads the unlinked file");
    assert_eq!(ns.read(reader, 0, 16).await.unwrap(), Bytes::from_static(b"after!"));
    ns.close(reader).await.unwrap();
    assert!(f.staged(ino).is_empty(), "nothing can publish or read it any more");
    assert!(f.files("mount-stage").is_empty());
    let other = ns.create(ns.root(), "crashed", 0o644).await.unwrap().ino;
    let fh = ns.open(other, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"open at the crash")).await.unwrap();
    ns.unlink(ns.root(), "crashed").await.unwrap();
    assert_eq!(f.staged(other).len(), 1);
    let f = f.restart(ns).await;
    let _ns = f.session(staging(u64::MAX)).await;
    assert!(f.staged(other).is_empty(), "no handle survives a restart");
    assert_eq!(f.db().query_row("SELECT count(*) FROM mount_staged", [], |r| r.get::<_, u64>(0)).unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement: run with --ignored --nocapture"]
async fn recovery_and_compaction_measurements() {
    // Opening the writer after a crash queues every stage left unflushed.
    for (files, size) in [(1_000usize, 4096usize), (100, 1 << 20)] {
        let f = Fixture::new().await;
        let ns = f.session(staging(u64::MAX)).await;
        for n in 0..files {
            let ino = ns.create(ns.root(), &format!("f{n}"), 0o644).await.unwrap().ino;
            let fh = ns.open(ino, true).await.unwrap();
            ns.write(fh, 0, Bytes::from(vec![n as u8; size])).await.unwrap();
        }
        let f = f.restart(ns).await;
        let started = Instant::now();
        let ns = f.writer(staging(u64::MAX)).await;
        let opened = started.elapsed();
        ns.recovered().await;
        let recovered = started.elapsed();
        let queued = f.queue.status().await.unwrap().items.iter().filter(|i| i.op == voidfs_client::Op::Put && i.size > 0).count();
        println!("{files} unflushed files of {size} bytes: writer open {:.3} s, all {queued} queued after {:.3} s", opened.as_secs_f64(), recovered.as_secs_f64());
        drop(ns);
    }
    // A 64 MiB file rewritten in 1 MiB writes, three times, with an fsync after each pass.
    for threshold in [u64::MAX, 64 << 20] {
        let f = Fixture::new().await;
        let ns = f.session(staging(threshold)).await;
        let ino = ns.create(ns.root(), "big", 0o644).await.unwrap().ino;
        let fh = ns.open(ino, true).await.unwrap();
        let mut syncs = Vec::new();
        let mut largest = 0;
        for pass in 0..3u8 {
            for at in 0..64u64 { ns.write(fh, at << 20, Bytes::from(vec![pass; 1 << 20])).await.unwrap(); }
            largest = largest.max(std::fs::metadata(&f.staged(ino)[0]).unwrap().len());
            let started = Instant::now();
            ns.fsync(fh).await.unwrap();
            syncs.push(started.elapsed().as_secs_f64());
        }
        let after = std::fs::metadata(&f.staged(ino)[0]).unwrap().len();
        let threshold = if threshold == u64::MAX { "off".into() } else { format!("{} MiB", threshold >> 20) };
        println!("compaction {threshold}: fsync {:?} s; staging file at most {} MiB before an fsync, {} MiB after the last",
            syncs.iter().map(|s| format!("{s:.3}")).collect::<Vec<_>>(), largest >> 20, after >> 20);
    }
}
