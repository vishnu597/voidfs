// SPDX-License-Identifier: Apache-2.0
//! Local staged bytes, immutable queue snapshots and guarded publication.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{Proxy, client_for};
use rusqlite::Connection;
use voidfs_client::mount::{FsError, RenameMode, Session, StagingConfig, Sync, XattrMode};
use voidfs_client::{ApiFetcher, BucketConfig, BucketFetcher, Cache, CacheConfig, Connectivity, Fetch,
    Content, Invalidation, Link, Op, Queue, QueueConfig, Scope, State, Store};
use voidfs_sdk::{AttributesUpdate, Client, Config, PutOptions, ReadOptions};
use voidfs_server::test_server::{Rules, TestServer};

#[derive(Clone, Copy)]
enum Backend { Api, Bucket }

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(1 << 50) }
fn no_space(_: &Path) -> std::io::Result<u64> { Ok(0) }

fn staging_config() -> StagingConfig {
    StagingConfig { min_free_bytes: 0, free_space: Some(plenty), quiet_period: None, ..Default::default() }
}

fn queue_config(connectivity: &Connectivity) -> QueueConfig {
    QueueConfig { retry_max: Duration::from_millis(50), connectivity: Some(connectivity.clone()), ..Default::default() }
}

async fn cache(store: Arc<Store>, client: Client, connectivity: Connectivity, backend: Backend) -> Cache {
    let fetcher: Arc<dyn Fetch> = match backend {
        Backend::Api => Arc::new(ApiFetcher::new(client).with_connectivity(connectivity)),
        Backend::Bucket => Arc::new(BucketFetcher::new(client, BucketConfig::default()).with_connectivity(connectivity)),
    };
    Cache::open(store, fetcher, CacheConfig { min_free_bytes: 0, free_space: Some(plenty), block_size: 4,
        read_ahead_bytes: 0, ..Default::default() }).await.unwrap()
}

struct Fixture {
    _server: TestServer, proxy: Proxy, remote: Client, client: Client, state: tempfile::TempDir,
    store: Arc<Store>, cache: Cache, queue: Queue, connectivity: Connectivity, backend: Backend,
}

impl Fixture {
    async fn new(backend: Backend) -> Self {
        let server = match backend {
            Backend::Api => TestServer::start().await.unwrap(),
            Backend::Bucket => TestServer::with_storage_credentials(Vec::new(), Rules::default()).await.unwrap(),
        };
        let proxy = Proxy::start(&server.endpoint).await;
        let remote = client_for(&server.endpoint, Config::default());
        let connectivity = Connectivity::default();
        let client = client_for(&proxy.endpoint, Config { observer: Some(Arc::new(connectivity.clone())), ..Default::default() });
        remote.create_drive("drv", Default::default()).await.unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(state.path()).unwrap());
        let cache = cache(store.clone(), client.clone(), connectivity.clone(), backend).await;
        let queue = Queue::open(store.clone(), client.clone(), queue_config(&connectivity)).await.unwrap();
        queue.pause(Scope::All).await.unwrap();
        Self { _server: server, proxy, remote, client, state, store, cache, queue, connectivity, backend }
    }

    async fn session(&self) -> Session { self.session_with(staging_config()).await }

    async fn session_with(&self, cfg: StagingConfig) -> Session {
        Session::new_writable_with_config(self.store.clone(), self.client.clone(), self.cache.clone(), self.queue.clone(),
            "drv", self.connectivity.clone(), cfg).await.unwrap()
    }

    async fn put(&self, key: &str, bytes: &'static [u8]) {
        self.remote.put_object("drv", key, Bytes::from_static(bytes), PutOptions {
            mtime: Some("2026-01-02T03:04:05Z".into()), mode: Some(0o640), ..Default::default()
        }).await.unwrap();
    }

    async fn body(&self, key: &str) -> Option<Bytes> {
        match self.remote.get_object("drv", key, ReadOptions::default()).await {
            Ok(object) => Some(object.body),
            Err(error) if error.status() == Some(404) => None,
            Err(error) => panic!("{error}"),
        }
    }

    async fn publish(&self) {
        self.queue.resume(Scope::All).await.unwrap();
        self.queue.settle().await;
    }

    fn offline(&self) {
        for _ in 0..3 { self.connectivity.unanswered(); }
        assert_eq!(self.connectivity.link(), Link::Offline);
    }

    async fn reopen(self, session: Session) -> (Self, Session) {
        self.queue.close().await;
        self.cache.settle().await;
        let Self { _server, proxy, remote, client, state, store, cache: old_cache, queue: old_queue, connectivity, backend } = self;
        drop(session);
        drop(old_cache);
        drop(old_queue);
        drop(store);
        let store = Arc::new(Store::open(state.path()).unwrap());
        let cache = cache(store.clone(), client.clone(), connectivity.clone(), backend).await;
        let queue = Queue::open(store.clone(), client.clone(), queue_config(&connectivity)).await.unwrap();
        let reopened = Self { _server, proxy, remote, client, state, store, cache, queue, connectivity, backend };
        let session = reopened.session().await;
        (reopened, session)
    }
}

async fn file(f: &Fixture, ns: &Session) -> (u64, u64) {
    f.put("file", b"0123456789ab").await;
    let ino = ns.lookup(ns.root(), "file").await.unwrap().ino;
    (ino, ns.open(ino, true).await.unwrap())
}

struct GapMoveState { current_key: String, offsets: HashSet<u64> }
struct GapMoveFetcher { remote: Client, body: Bytes, state: tokio::sync::Mutex<GapMoveState> }

fn missing_version() -> voidfs_client::Error {
    voidfs_sdk::Error::Service(voidfs_sdk::ServiceError { status: 404, code: "NoSuchVersion".into(), message: String::new(),
        request_id: None, current_version_id: None, retry_after: None }).into()
}

impl Fetch for GapMoveFetcher {
    fn fetch<'a>(&'a self, content: &'a Content, offset: u64, len: u64) -> futures::future::BoxFuture<'a, voidfs_client::Result<Bytes>> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if content.key != state.current_key { return Err(missing_version()); }
            if state.offsets.insert(offset) {
                let next = format!("z/moved{}", state.offsets.len());
                self.remote.rename("drv", &state.current_key, &next, Default::default()).await?;
                state.current_key = next;
                return Err(missing_version());
            }
            Ok(self.body.slice(offset as usize..(offset + len) as usize))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fragmented_merged_reads_share_one_relocation_request_budget() {
    let f = Fixture::new(Backend::Api).await;
    f.put("a/", b"").await;
    let body = Bytes::from_static(b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-");
    f.remote.put_object("drv", "z/file", body.clone(), Default::default()).await.unwrap();
    let fetcher = Arc::new(GapMoveFetcher { remote: f.remote.clone(), body, state: tokio::sync::Mutex::new(GapMoveState {
        current_key: "z/file".into(), offsets: HashSet::new()
    }) });
    let cache = Cache::open(f.store.clone(), fetcher.clone(), CacheConfig { min_free_bytes: 0, free_space: Some(plenty),
        block_size: 4, read_ahead_bytes: 0, ..Default::default() }).await.unwrap();
    let ns = Session::new_writable_with_config(f.store.clone(), f.client.clone(), cache, f.queue.clone(),
        "drv", f.connectivity.clone(), staging_config()).await.unwrap();
    let folder = ns.lookup(ns.root(), "z").await.unwrap();
    let file = ns.lookup(folder.ino, "file").await.unwrap();
    let fh = ns.open(file.ino, true).await.unwrap();
    for offset in [4, 12, 20, 28, 36] { ns.write(fh, offset, Bytes::from_static(b"edit")).await.unwrap(); }
    f.proxy.clear();
    assert_eq!(ns.read(fh, 0, 44).await.unwrap_err(), FsError::Again);
    let requests = f.proxy.seen();
    let listings = requests.iter().filter(|r| r.contains("x-voidfs-list") || r.contains("x-voidfs-deleted")).count();
    assert!(listings <= 16, "all six gaps share sixteen logical listing calls: {requests:?}");
    let moves = fetcher.state.lock().await.offsets.len();
    assert!((3..=4).contains(&moves), "several gaps completed before the shared budget stopped the callback: {moves}");
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn earlier_and_later_handles_see_local_bytes_and_attributes_for_both_fetchers() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        f.put("file", b"0123456789ab").await;
        let ns = f.session().await;
        let before = ns.lookup(ns.root(), "file").await.unwrap();
        let earlier = ns.open(before.ino, false).await.unwrap();
        let writable = ns.open(before.ino, true).await.unwrap();
        assert_eq!(ns.write(writable, 4, Bytes::from_static(b"LOCAL")).await.unwrap(), 5);
        let current = ns.getattr(before.ino).await.unwrap();
        assert_eq!((current.size, current.mode, current.sync), (12, before.mode, Sync::Pending));
        assert!(current.mtime > before.mtime);
        assert_eq!(ns.handle_attr(earlier).unwrap(), current);
        let later = ns.open(before.ino, false).await.unwrap();
        for fh in [earlier, writable, later] {
            assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"0123LOCAL9ab"));
        }
        assert_eq!(f.body("file").await, Some(Bytes::from_static(b"0123456789ab")));
        assert!(f.queue.status().await.unwrap().items.is_empty(), "write stages without flushing");
        ns.close(earlier).await.unwrap();
        ns.close(later).await.unwrap();
        ns.close(writable).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_extents_publish_the_last_local_bytes_without_fetching_untouched_data() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (_, fh) = file(&f, &ns).await;
        f.proxy.clear();
        ns.write(fh, 2, Bytes::from_static(b"abc")).await.unwrap();
        ns.write(fh, 3, Bytes::from_static(b"ZZ")).await.unwrap();
        ns.fsync(fh).await.unwrap();
        assert!(!f.proxy.seen().iter().any(|r| r.contains("versionId=")), "sparse flushing must not download untouched gaps");
        assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"01aZZ56789ab"));
        f.publish().await;
        assert_eq!(f.body("file").await, Some(Bytes::from_static(b"01aZZ56789ab")));
        assert!(f.queue.status().await.unwrap().items.iter().all(|i| i.state == State::Done));
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_sparse_files_read_zero_gaps_and_truncation_discards_the_old_tail() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let attr = ns.create(ns.root(), "new", 0o600).await.unwrap();
    let fh = ns.open(attr.ino, true).await.unwrap();
    ns.write(fh, 8, Bytes::from_static(b"XYZ")).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from([vec![0; 8], b"XYZ".to_vec()].concat()));
    ns.truncate(fh, 6).await.unwrap();
    ns.truncate(fh, 11).await.unwrap();
    ns.write(fh, 7, Bytes::from_static(b"Q")).await.unwrap();
    let expected = Bytes::from([vec![0; 7], b"Q".to_vec(), vec![0; 3]].concat());
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), expected);
    assert_eq!(ns.handle_attr(fh).unwrap().size, 11);
    assert!(ns.read(fh, 11, 20).await.unwrap().is_empty());
    assert!(ns.read(fh, u64::MAX, 20).await.unwrap().is_empty());
    ns.close(fh).await.unwrap();
    f.publish().await;
    assert_eq!(f.body("new").await, Some(expected));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shrinking_then_growing_a_remote_base_does_not_restore_remote_tail_bytes() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (ino, fh) = file(&f, &ns).await;
        let earlier = ns.open(ino, false).await.unwrap();
        ns.truncate(fh, 4).await.unwrap();
        ns.truncate(fh, 10).await.unwrap();
        ns.write(fh, 8, Bytes::from_static(b"X")).await.unwrap();
        let expected = Bytes::from([b"0123".to_vec(), vec![0; 4], b"X\0".to_vec()].concat());
        assert_eq!(ns.read(earlier, 0, u64::MAX).await.unwrap(), expected);
        ns.fsync(fh).await.unwrap();
        f.publish().await;
        assert_eq!(f.body("file").await, Some(expected));
        ns.close(earlier).await.unwrap();
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn complete_local_bytes_and_zero_gaps_remain_readable_offline() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    ns.readdir(ns.root(), None, 100).await.unwrap();
    f.offline();
    f.proxy.clear();
    let attr = ns.create(ns.root(), "offline", 0o640).await.unwrap();
    let fh = ns.open(attr.ino, true).await.unwrap();
    ns.write(fh, 4, Bytes::from_static(b"bytes")).await.unwrap();
    ns.truncate(fh, 12).await.unwrap();
    ns.fsync(fh).await.unwrap();
    let expected = Bytes::from([vec![0; 4], b"bytes".to_vec(), vec![0; 3]].concat());
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), expected);
    assert_eq!(ns.lookup(ns.root(), "offline").await.unwrap().size, 12);
    assert!(f.proxy.seen().is_empty(), "fully local reads and saves issue no request offline");
    ns.close(fh).await.unwrap();
    f.connectivity.answered(false);
    f.publish().await;
    assert_eq!(f.body("offline").await, Some(expected));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_merged_reads_use_cached_base_gaps_and_reject_uncached_gaps() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (_, fh) = file(&f, &ns).await;
        assert_eq!(ns.read(fh, 0, 4).await.unwrap(), Bytes::from_static(b"0123"));
        ns.write(fh, 4, Bytes::from_static(b"DATA")).await.unwrap();
        f.offline();
        f.proxy.clear();
        assert_eq!(ns.read(fh, 0, 8).await.unwrap(), Bytes::from_static(b"0123DATA"));
        assert_eq!(ns.read(fh, 8, 4).await.unwrap_err(), FsError::Offline);
        assert!(f.proxy.seen().is_empty(), "uncached gaps fail before contacting either fetcher");
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fsync_freezes_queue_bytes_while_further_writes_stay_local() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (_, fh) = file(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"ONE")).await.unwrap();
    assert_eq!(ns.handle_attr(fh).unwrap().sync, Sync::Pending);
    ns.fsync(fh).await.unwrap();
    assert_eq!(ns.handle_attr(fh).unwrap().sync, Sync::Saving);
    ns.write(fh, 0, Bytes::from_static(b"TWO")).await.unwrap();
    assert_eq!(ns.handle_attr(fh).unwrap().sync, Sync::Pending);
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"ONE3456789ab")));
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"TWO3456789ab"));
    ns.fsync(fh).await.unwrap();
    f.queue.settle().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"TWO3456789ab")));
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_fsync_and_clean_close_do_not_enqueue_duplicate_data() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (_, fh) = file(&f, &ns).await;
    ns.write(fh, 1, Bytes::from_static(b"edit")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    let once = f.queue.status().await.unwrap();
    assert!(!once.items.is_empty());
    ns.fsync(fh).await.unwrap();
    assert_eq!(f.queue.status().await.unwrap(), once);
    ns.close(fh).await.unwrap();
    assert_eq!(f.queue.status().await.unwrap(), once);
    assert_eq!(ns.close(fh).await.unwrap_err(), FsError::BadHandle);
    assert_eq!(ns.fsync(fh).await.unwrap_err(), FsError::BadHandle);
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"0edit56789ab")));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_flushes_dirty_bytes_and_does_not_invalidate_other_handles() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = file(&f, &ns).await;
    let other = ns.open(ino, false).await.unwrap();
    ns.write(fh, 12, Bytes::from_static(b"tail")).await.unwrap();
    ns.close(fh).await.unwrap();
    assert_eq!(ns.read(fh, 0, 1).await.unwrap_err(), FsError::BadHandle);
    assert_eq!(ns.read(other, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"0123456789abtail"));
    assert!(!f.queue.status().await.unwrap().items.is_empty());
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"0123456789abtail")));
    ns.close(other).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_unlinked_file_keeps_its_bytes_after_its_removal_publishes() {
    for (backend, existing) in [(Backend::Api, true), (Backend::Api, false), (Backend::Bucket, true)] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (expected, fh) = if existing {
            let (_, fh) = file(&f, &ns).await;
            (&b"first56789aX"[..], fh)
        } else {
            let ino = ns.create(ns.root(), "file", 0o644).await.unwrap().ino;
            (&b"firstX"[..], ns.open(ino, true).await.unwrap())
        };
        ns.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
        ns.unlink(ns.root(), "file").await.unwrap();
        ns.fsync(fh).await.unwrap();
        f.publish().await;
        assert!(f.body("file").await.is_none());
        f.queue.pause(Scope::All).await.unwrap();
        ns.write(fh, expected.len() as u64 - 1, Bytes::from_static(b"X")).await.unwrap();
        assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(expected), "existing {existing}: the handle keeps the bytes it wrote");
        ns.close(fh).await.unwrap();
        f.publish().await;
        assert!(f.body("file").await.is_none(), "nothing publishes after close");
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_unlink_keeps_local_reads_and_writes_without_publishing_after_close() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (ino, fh) = file(&f, &ns).await;
        ns.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
        ns.unlink(ns.root(), "file").await.unwrap();
        assert_eq!(ns.lookup(ns.root(), "file").await.unwrap_err(), FsError::NotFound);
        assert_eq!(ns.open(ino, false).await.unwrap_err(), FsError::Stale);
        ns.write(fh, 5, Bytes::from_static(b"last")).await.unwrap();
        assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"firstlast9ab"));
        ns.fsync(fh).await.unwrap();
        ns.close(fh).await.unwrap();
        let queued = f.queue.status().await.unwrap();
        assert_eq!(queued.items.iter().map(|i| i.op).collect::<Vec<_>>(), [Op::Delete]);
        f.publish().await;
        assert!(f.body("file").await.is_none());
        assert!(f.queue.status().await.unwrap().items.iter().all(|i| i.state == State::Done));
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn atomic_save_replaces_remote_bytes_and_preserves_the_original_open_inode() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        f.put("target", b"original saved bytes").await;
        let ns = f.session().await;
        let original = ns.lookup(ns.root(), "target").await.unwrap();
        let old = ns.open(original.ino, false).await.unwrap();
        let temporary = ns.create(ns.root(), "temporary", 0o600).await.unwrap();
        let fh = ns.open(temporary.ino, true).await.unwrap();
        ns.write(fh, 0, Bytes::from_static(b"replacement saved bytes")).await.unwrap();
        ns.fsync(fh).await.unwrap();
        ns.rename(ns.root(), "temporary", ns.root(), "target", RenameMode::Replace).await.unwrap();
        assert_eq!(ns.lookup(ns.root(), "target").await.unwrap().ino, temporary.ino);
        assert_eq!(ns.lookup(ns.root(), "temporary").await.unwrap_err(), FsError::NotFound);
        assert_eq!(ns.read(old, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"original saved bytes"));
        assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"replacement saved bytes"));
        ns.close(fh).await.unwrap();
        f.publish().await;
        assert_eq!(f.body("target").await, Some(Bytes::from_static(b"replacement saved bytes")));
        assert!(f.body("temporary").await.is_none());
        assert_eq!(ns.read(old, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"original saved bytes"));
        ns.close(old).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dirty_data_after_rename_uses_the_destination_and_guarded_entry_lineage() {
    for flush_before in [false, true] {
        let f = Fixture::new(Backend::Api).await;
        let ns = f.session().await;
        let (ino, fh) = file(&f, &ns).await;
        ns.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
        if flush_before { ns.fsync(fh).await.unwrap(); }
        ns.rename(ns.root(), "file", ns.root(), "renamed", RenameMode::Exclusive).await.unwrap();
        ns.write(fh, 5, Bytes::from_static(b"last")).await.unwrap();
        ns.fsync(fh).await.unwrap();
        assert_eq!(ns.lookup(ns.root(), "renamed").await.unwrap().ino, ino);
        assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"firstlast9ab"));
        f.publish().await;
        assert!(f.body("file").await.is_none());
        assert_eq!(f.body("renamed").await, Some(Bytes::from_static(b"firstlast9ab")));
        assert!(f.queue.status().await.unwrap().items.iter().all(|i| i.state == State::Done));
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_remote_edits_fail_the_guard_and_retain_staged_local_bytes() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (_, fh) = file(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"local")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.put("file", b"competing remote bytes").await;
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"competing remote bytes")));
    assert!(f.queue.status().await.unwrap().items.iter().any(|i| i.state == State::Failed));
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"local56789ab"));
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acknowledged_bytes_and_names_survive_dropping_the_session_without_close() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        f.put("base", b"0123456789ab").await;
        let ns = f.session().await;
        let remote = ns.lookup(ns.root(), "base").await.unwrap();
        let old_fh = ns.open(remote.ino, true).await.unwrap();
        ns.write(old_fh, 2, Bytes::from_static(b"local")).await.unwrap();
        ns.truncate(old_fh, 15).await.unwrap();
        let dir = ns.mkdir(ns.root(), "local", 0o750).await.unwrap();
        let local = ns.create(dir.ino, "cafe\u{301}", 0o600).await.unwrap();
        let local_fh = ns.open(local.ino, true).await.unwrap();
        ns.write(local_fh, 3, Bytes::from_static(b"saved locally")).await.unwrap();
        let (f, ns) = f.reopen(ns).await;
        assert_eq!(ns.read(old_fh, 0, 1).await.unwrap_err(), FsError::Stale);
        assert_eq!(ns.lookup(ns.root(), "base").await.unwrap().size, 15);
        assert_eq!(ns.lookup(dir.ino, "caf\u{e9}").await.unwrap().ino, local.ino);
        let fh = ns.open(remote.ino, true).await.unwrap();
        assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from([b"01local789ab".to_vec(), vec![0; 3]].concat()));
        let local_fh = ns.open(local.ino, true).await.unwrap();
        let expected = Bytes::from([vec![0; 3], b"saved locally".to_vec()].concat());
        assert_eq!(ns.read(local_fh, 0, u64::MAX).await.unwrap(), expected);
        ns.close(fh).await.unwrap();
        ns.close(local_fh).await.unwrap();
        f.publish().await;
        assert_eq!(f.body("local/caf\u{e9}").await, Some(expected));
        assert_eq!(f.body("base").await, Some(Bytes::from([b"01local789ab".to_vec(), vec![0; 3]].concat())));
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserve_admission_returns_enospc_before_bytes_or_metadata_change() {
    let f = Fixture::new(Backend::Api).await;
    f.put("file", b"unchanged").await;
    let ns = f.session_with(StagingConfig { min_free_bytes: 1024, free_space: Some(no_space), quiet_period: None, ..Default::default() }).await;
    let before = ns.lookup(ns.root(), "file").await.unwrap();
    let fh = ns.open(before.ino, true).await.unwrap();
    let error = ns.write(fh, 2, Bytes::from_static(b"rejected")).await.unwrap_err();
    assert_eq!((error.clone(), error.errno()), (FsError::NoSpace, libc::ENOSPC));
    assert_eq!(ns.getattr(before.ino).await.unwrap(), before);
    assert_eq!(ns.handle_attr(fh).unwrap(), before);
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"unchanged"));
    assert!(f.queue.status().await.unwrap().items.is_empty());
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publishing_and_clearing_finished_sources_keeps_open_staged_readers_valid() {
    let f = Fixture::new(Backend::Bucket).await;
    let ns = f.session().await;
    let attr = ns.create(ns.root(), "new", 0o600).await.unwrap();
    let fh = ns.open(attr.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"locally retained data")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.publish().await;
    f.queue.clear_finished().await.unwrap();
    f.offline();
    f.proxy.clear();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"locally retained data"));
    assert!(f.proxy.seen().is_empty());
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_handles_serialize_disjoint_writes_without_losing_any_extent() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let attr = ns.create(ns.root(), "new", 0o600).await.unwrap();
    let mut handles = Vec::new();
    for _ in 0..8 { handles.push(ns.open(attr.ino, true).await.unwrap()); }
    let writes = handles.iter().enumerate().map(|(i, fh)| ns.write(*fh, i as u64 * 4, Bytes::from(vec![i as u8 + 1; 4])));
    for result in futures::future::join_all(writes).await { assert_eq!(result.unwrap(), 4); }
    let expected = Bytes::from((1..=8).flat_map(|i| [i; 4]).collect::<Vec<u8>>());
    for fh in &handles { assert_eq!(ns.read(*fh, 0, u64::MAX).await.unwrap(), expected); }
    for fh in handles { ns.close(fh).await.unwrap(); }
    f.publish().await;
    assert_eq!(f.body("new").await, Some(expected));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_feed_changes_do_not_rebind_a_staged_files_base_or_local_size() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (ino, fh) = file(&f, &ns).await;
        ns.write(fh, 4, Bytes::from_static(b"LOCAL")).await.unwrap();
        ns.truncate(fh, 14).await.unwrap();
        let before = ns.handle_attr(fh).unwrap();
        f.put("file", b"a newer remote object version that is longer").await;
        ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
        let attr = ns.getattr(ino).await.unwrap();
        assert_eq!((attr.size, attr.version_id, attr.etag), (before.size, before.version_id, before.etag));
        let later = ns.open(ino, false).await.unwrap();
        let expected = Bytes::from([b"0123LOCAL9ab".to_vec(), vec![0; 2]].concat());
        for handle in [fh, later] { assert_eq!(ns.read(handle, 0, u64::MAX).await.unwrap(), expected); }
        ns.close(later).await.unwrap();
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_extents_are_shared_while_each_handles_external_base_stays_bound() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        f.put("file", b"0123456789ab").await;
        let ns = f.session().await;
        let original = ns.lookup(ns.root(), "file").await.unwrap();
        let earlier = ns.open(original.ino, false).await.unwrap();
        f.put("file", b"ABCDEFGHIJKL").await;
        ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
        let current = ns.getattr(original.ino).await.unwrap();
        assert_ne!(current.version_id, original.version_id);
        let writable = ns.open(original.ino, true).await.unwrap();
        ns.write(writable, 4, Bytes::from_static(b"edit")).await.unwrap();
        assert_eq!(ns.read(earlier, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"0123edit89ab"));
        assert_eq!(ns.read(writable, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"ABCDeditIJKL"));
        let later = ns.open(original.ino, false).await.unwrap();
        assert_eq!(ns.read(later, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"ABCDeditIJKL"));
        ns.fsync(writable).await.unwrap();
        f.publish().await;
        assert_eq!(f.body("file").await, Some(Bytes::from_static(b"ABCDeditIJKL")));
        assert_eq!(ns.read(earlier, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"0123edit89ab"));
        ns.close(earlier).await.unwrap();
        ns.close(later).await.unwrap();
        ns.close(writable).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_write_after_remote_refresh_keeps_the_write_handles_original_guard() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = file(&f, &ns).await;
    let before = ns.handle_attr(fh).unwrap();
    f.put("file", b"a competing newer remote version").await;
    ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
    let newer = ns.getattr(ino).await.unwrap();
    assert_ne!(newer.version_id, before.version_id);
    ns.write(fh, 0, Bytes::from_static(b"local")).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"local56789ab"));
    ns.fsync(fh).await.unwrap();
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"a competing newer remote version")));
    assert!(f.queue.status().await.unwrap().items.iter().any(|i| i.state == State::Failed));
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_writes_preserve_remote_and_local_extended_attributes() {
    let f = Fixture::new(Backend::Api).await;
    f.remote.put_object("drv", "remote", "before", PutOptions { content_type: Some("application/x-voidfs-test".into()),
        metadata: BTreeMap::from([("kept".into(), "remote metadata".into())]), ..Default::default() }).await.unwrap();
    f.remote.set_attributes("drv", "remote", AttributesUpdate {
        set_xattrs: BTreeMap::from([("user.kept".into(), Bytes::from_static(&[0, 128, 255]))]),
        flags: Some(vec!["hidden".into()]), ..Default::default()
    }, Default::default()).await.unwrap();
    let before = f.remote.attributes("drv", "remote", Default::default()).await.unwrap();
    let ns = f.session().await;
    let remote = ns.lookup(ns.root(), "remote").await.unwrap();
    assert_eq!(ns.getxattr(remote.ino, "user.kept").await.unwrap(), [0, 128, 255]);
    let fh = ns.open(remote.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"fully rewritten remote bytes")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    let local = ns.create(ns.root(), "local", 0o600).await.unwrap();
    ns.setxattr(local.ino, "user.kept", &[0, 128, 255], XattrMode::Create).await.unwrap();
    let local_fh = ns.open(local.ino, true).await.unwrap();
    ns.write(local_fh, 0, Bytes::from_static(b"new local bytes")).await.unwrap();
    ns.fsync(local_fh).await.unwrap();
    f.publish().await;
    let mut after = f.remote.attributes("drv", "remote", Default::default()).await.unwrap();
    assert!(after.meta.remove("voidfs-entry").is_some(), "the edit carries its entry's marker, as a put does");
    assert_eq!((after.xattrs.clone(), after.flags, after.content_type, after.meta),
        (before.xattrs.clone(), before.flags, before.content_type, before.meta), "and keeps everything else");
    let published_local = f.remote.attributes("drv", "local", Default::default()).await.unwrap();
    assert_eq!(published_local.xattrs, before.xattrs);
    assert_eq!(f.body("remote").await, Some(Bytes::from_static(b"fully rewritten remote bytes")));
    assert_eq!(f.body("local").await, Some(Bytes::from_static(b"new local bytes")));
    ns.close(fh).await.unwrap();
    ns.close(local_fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sparse_edits_larger_than_one_patch_publish_in_guarded_bounded_runs() {
    const MIB: usize = 1 << 20;
    let f = Fixture::new(Backend::Api).await;
    f.remote.put_object("drv", "large", Bytes::from(vec![0x11; 80 * MIB]), Default::default()).await.unwrap();
    let ns = f.session().await;
    let attr = ns.lookup(ns.root(), "large").await.unwrap();
    let fh = ns.open(attr.ino, true).await.unwrap();
    f.proxy.clear();
    assert_eq!(ns.write(fh, (4 * MIB) as u64, Bytes::from(vec![0x7c; 72 * MIB])).await.unwrap(), 72 * MIB);
    ns.fsync(fh).await.unwrap();
    let queued = f.queue.status().await.unwrap();
    assert!(queued.items.iter().filter(|i| i.op == Op::Write).all(|i| i.size <= (8 * MIB) as u64));
    assert!(!f.proxy.seen().iter().any(|r| r.contains("versionId=")), "flushing large edits does not fetch the base");
    f.publish().await;
    assert!(f.queue.status().await.unwrap().items.iter().all(|i| i.state == State::Done));
    assert!(f.proxy.seen().iter().filter(|r| r.contains("x-voidfs-patch")).count() >= 2, "edits exceed the protocol's single-patch bound");
    let body = f.body("large").await.unwrap();
    assert_eq!(body.len(), 80 * MIB);
    assert!(body[..4 * MIB].iter().all(|b| *b == 0x11));
    assert!(body[4 * MIB..76 * MIB].iter().all(|b| *b == 0x7c));
    assert!(body[76 * MIB..].iter().all(|b| *b == 0x11));
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_access_handles_and_offset_limits_reject_invalid_mutations() {
    let f = Fixture::new(Backend::Api).await;
    f.put("file", b"unchanged").await;
    let ns = f.session().await;
    let before = ns.lookup(ns.root(), "file").await.unwrap();
    let read_only = ns.open(before.ino, false).await.unwrap();
    assert_eq!(ns.write(read_only, 0, Bytes::from_static(b"no")).await.unwrap_err(), FsError::BadHandle);
    assert_eq!(ns.truncate(read_only, 0).await.unwrap_err(), FsError::BadHandle);
    let fh = ns.open(before.ino, true).await.unwrap();
    assert_eq!(ns.write(fh, u64::MAX, Bytes::from_static(b"overflow")).await.unwrap_err(), FsError::InvalidArgument);
    assert_eq!(ns.truncate(fh, u64::MAX).await.unwrap_err(), FsError::InvalidArgument);
    assert_eq!(ns.write(fh, 3, Bytes::new()).await.unwrap(), 0);
    assert_eq!(ns.getattr(before.ino).await.unwrap(), before);
    assert!(f.queue.status().await.unwrap().items.is_empty());
    ns.close(fh).await.unwrap();
    assert_eq!(ns.write(fh, 0, Bytes::from_static(b"no")).await.unwrap_err(), FsError::BadHandle);
    assert_eq!(ns.truncate(fh, 0).await.unwrap_err(), FsError::BadHandle);
    ns.close(read_only).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quiet_period_publishes_open_files_after_the_last_write() {
    assert_eq!(StagingConfig::default().quiet_period, Some(Duration::from_secs(2)));
    let f = Fixture::new(Backend::Api).await;
    let quiet = Duration::from_millis(200);
    let ns = f.session_with(StagingConfig { quiet_period: Some(quiet), ..staging_config() }).await;
    let (_, fh) = file(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;
    let previous = f.queue.status().await.unwrap().items.len();
    ns.write(fh, 0, Bytes::from_static(b"final")).await.unwrap();
    let last_write = tokio::time::Instant::now();
    tokio::time::sleep(Duration::from_millis(120)).await;
    if last_write.elapsed() < quiet {
        assert_eq!(f.queue.status().await.unwrap().items.len(), previous, "the first write's timer must not flush the newer edit early");
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while f.queue.status().await.unwrap().items.len() == previous {
        assert!(tokio::time::Instant::now() < deadline, "quiet-period staging was never queued");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The flush commits its entries, then updates the file's attributes in memory.
    while ns.handle_attr(fh).unwrap().sync != Sync::Saving {
        assert!(tokio::time::Instant::now() < deadline, "a queued quiet-period flush never reported saving");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"final56789ab")));
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"final56789ab"));
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "writes 1 GiB and stages 10,000 files; run explicitly for the staged-write measurements"]
async fn staged_write_throughput_one_gib_and_ten_thousand_small_files() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let attr = ns.create(ns.root(), "large", 0o600).await.unwrap();
    let fh = ns.open(attr.ino, true).await.unwrap();
    let one_mib = Bytes::from(vec![0x5a; 1 << 20]);
    let large_commits = ns.staging_commits();
    let begin = Instant::now();
    let mut latencies = Vec::new();
    for i in 0..1024 {
        let write_begin = Instant::now();
        assert_eq!(ns.write(fh, i * (1 << 20), one_mib.clone()).await.unwrap(), 1 << 20);
        latencies.push(write_begin.elapsed());
    }
    let large_time = begin.elapsed();
    latencies.sort();
    let commits = ns.staging_commits();
    eprintln!("1 GiB / 1 MiB writes: elapsed={large_time:?}, writes=1024, mean={:?}, max={:?}, p50={:?}, p99={:?}, NORMAL_commits={}, FULL_commits={}",
        latencies.iter().sum::<Duration>() / 1024, latencies[1023], latencies[512], latencies[1013], commits.0 - large_commits.0, commits.1 - large_commits.1);
    assert_eq!(ns.handle_attr(fh).unwrap().size, 1 << 30);
    let small = Bytes::from(vec![0xa5; 4096]);
    let small_commits = ns.staging_commits();
    let begin = Instant::now();
    let mut latencies = Vec::new();
    for i in 0..10_000 {
        let attr = ns.create(ns.root(), &format!("small-{i:05}"), 0o600).await.unwrap();
        let fh = ns.open(attr.ino, true).await.unwrap();
        let write_begin = Instant::now();
        assert_eq!(ns.write(fh, 0, small.clone()).await.unwrap(), 4096);
        latencies.push(write_begin.elapsed());
        ns.close(fh).await.unwrap();
    }
    latencies.sort();
    let db = Connection::open(f.state.path().join("state.sqlite")).unwrap();
    let entries: u64 = db.query_row("SELECT count(*) FROM entries", [], |r| r.get(0)).unwrap();
    let commits = ns.staging_commits();
    eprintln!("10,000 / 4 KiB files: elapsed={:?}, writes=10000, mean={:?}, max={:?}, p50={:?}, p99={:?}, NORMAL_commits={}, FULL_staging_commits={}, FULL_namespace_creates=10000, journal_entries={entries}",
        begin.elapsed(), latencies.iter().sum::<Duration>() / 10000, latencies[9999], latencies[5000], latencies[9900],
        commits.0 - small_commits.0, commits.1 - small_commits.1);
    f.queue.close().await;
}
