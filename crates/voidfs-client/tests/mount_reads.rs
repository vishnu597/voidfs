// SPDX-License-Identifier: Apache-2.0
//! Snapshot handles, against both the API and the credential-enforcing stand-in bucket.

mod common;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use common::{Proxy, client_for};
use voidfs_client::mount::{FsError, Session};
use voidfs_client::{ApiFetcher, BucketConfig, BucketFetcher, Cache, CacheConfig, Connectivity, Content, Error, Fetch, Link, Store, Invalidation};
use voidfs_sdk::{Client, Config, Kind, PutOptions};
use voidfs_server::test_server::{Rules, TestServer};

const MIB: u64 = 1 << 20;

#[derive(Clone, Copy)]
enum Backend { Api, Bucket }

fn bytes_of(seed: u64, n: u64) -> Bytes {
    use sha2::Digest;
    let mut out = Vec::with_capacity(n as usize + 32);
    let mut i = 0u64;
    while (out.len() as u64) < n {
        out.extend_from_slice(&sha2::Sha256::digest([seed.to_be_bytes(), i.to_be_bytes()].concat()));
        i += 1;
    }
    out.truncate(n as usize);
    out.into()
}

fn plenty(_: &std::path::Path) -> std::io::Result<u64> { Ok(1 << 50) }

fn config(ahead: u64) -> CacheConfig {
    CacheConfig { block_size: MIB, max_bytes: 64 * MIB, min_free_bytes: 0, memory_bytes: 16 * MIB,
        read_ahead_bytes: ahead, free_space: Some(plenty), ..Default::default() }
}

struct Fixture {
    _server: TestServer,
    proxy: Proxy,
    remote: Client,
    client: Client,
    state: tempfile::TempDir,
    store: Arc<Store>,
    cache: Cache,
    connectivity: Connectivity,
    bucket: Option<Arc<BucketFetcher>>,
}

impl Fixture {
    async fn new(backend: Backend, ahead: u64) -> Self {
        let server = match backend {
            Backend::Api => TestServer::start().await.unwrap(),
            Backend::Bucket => TestServer::with_storage_credentials(Vec::new(), Rules::default()).await.unwrap(),
        };
        let proxy = Proxy::start(&server.endpoint).await;
        let connectivity = Connectivity::default();
        let remote = client_for(&server.endpoint, Config::default());
        let client = client_for(&proxy.endpoint, Config { observer: Some(Arc::new(connectivity.clone())), ..Default::default() });
        remote.create_drive("drv", Default::default()).await.unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(state.path()).unwrap());
        let bucket = match backend {
            Backend::Api => None,
            Backend::Bucket => Some(Arc::new(BucketFetcher::new(client.clone(), BucketConfig::default()).with_connectivity(connectivity.clone()))),
        };
        let fetcher: Arc<dyn Fetch> = match &bucket {
            Some(bucket) => bucket.clone(),
            None => Arc::new(ApiFetcher::new(client.clone()).with_connectivity(connectivity.clone())),
        };
        let cache = Cache::open(store.clone(), fetcher, config(ahead)).await.unwrap();
        Self { _server: server, proxy, remote, client, state, store, cache, connectivity, bucket }
    }

    async fn session(&self) -> Session {
        Session::new(self.store.clone(), self.client.clone(), self.cache.clone(), "drv", self.connectivity.clone()).await.unwrap()
    }

    async fn put(&self, key: &str, body: Bytes) {
        self.remote.put_object("drv", key, body, Default::default()).await.unwrap();
    }

    fn offline(&self) {
        for _ in 0..3 { self.connectivity.unanswered(); }
        assert_eq!(self.connectivity.link(), Link::Offline);
    }
}

fn changes(keys: &[&str]) -> Vec<Invalidation> {
    keys.iter().map(|key| Invalidation::Object((*key).into())).collect()
}

fn gets(proxy: &Proxy) -> usize {
    proxy.seen().iter().filter(|r| r.starts_with("GET ") && r.contains("versionId=")).count()
}

async fn snapshot_survives(backend: Backend) {
    let f = Fixture::new(backend, 0).await;
    let old = bytes_of(1, 5 * MIB + 17);
    f.remote.put_object("drv", "file", old.clone(), PutOptions {
        mode: Some(0o640), mtime: Some("2026-01-02T03:04:05Z".into()), ..Default::default()
    }).await.unwrap();
    let ns = f.session().await;
    let before = ns.lookup(ns.root(), "file").await.unwrap();
    let fh = ns.open(before.ino, false).await.unwrap();
    assert_eq!(ns.handle_attr(fh).unwrap(), before);
    f.proxy.clear();

    let new = bytes_of(2, MIB + 31);
    f.put("file", new.clone()).await;
    ns.invalidate(&changes(&["file"])).await.unwrap();
    let after = ns.getattr(before.ino).await.unwrap();
    assert_eq!((after.ino, &after.object_id, after.size), (before.ino, &before.object_id, new.len() as u64));
    assert_ne!(after.version_id, before.version_id);
    assert_ne!(after.etag, before.etag);
    let current = ns.open(before.ino, false).await.unwrap();
    assert_eq!(ns.handle_attr(current).unwrap(), after);
    assert_eq!(ns.read(current, 0, u64::MAX).await.unwrap(), new);
    assert_eq!(ns.read(fh, 2 * MIB, 91).await.unwrap(), old.slice(2 * MIB as usize..2 * MIB as usize + 91));
    assert_eq!(ns.handle_attr(fh).unwrap(), before);
    ns.close(current).await.unwrap();

    f.remote.rename("drv", "file", "renamed", Default::default()).await.unwrap();
    ns.invalidate(&changes(&["file", "renamed"])).await.unwrap();
    let renamed = ns.lookup(ns.root(), "renamed").await.unwrap();
    assert_eq!(renamed.ino, before.ino);
    assert_eq!(ns.lookup(ns.root(), "file").await.unwrap_err(), FsError::NotFound);
    let renamed_fh = ns.open(renamed.ino, false).await.unwrap();
    assert_eq!(ns.read(renamed_fh, MIB, 31).await.unwrap(), new.slice(MIB as usize..));
    assert_eq!(ns.read(fh, 0, 93).await.unwrap(), old.slice(..93));
    assert_eq!(ns.handle_attr(fh).unwrap(), before);
    ns.close(renamed_fh).await.unwrap();

    f.remote.delete_object("drv", "renamed", Default::default()).await.unwrap();
    ns.invalidate(&changes(&["renamed"])).await.unwrap();
    assert_eq!(ns.lookup(ns.root(), "renamed").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.getattr(before.ino).await.unwrap_err(), FsError::Stale);
    assert_eq!(ns.open(before.ino, false).await.unwrap_err(), FsError::Stale);
    // Each seek breaks forward read-ahead, so this block has never been fetched before delete.
    assert_eq!(ns.read(fh, 4 * MIB, MIB + 100).await.unwrap(), old.slice(4 * MIB as usize..));
    assert_eq!(ns.handle_attr(fh).unwrap(), before);
    ns.close(fh).await.unwrap();
    f.cache.settle().await;
    assert_eq!(f.cache.usage().fetches, 6, "only six distinct content blocks were filled");
    if let Some(bucket) = &f.bucket {
        let usage = bucket.usage();
        assert_eq!(usage.api_reads, 0, "{usage:?}");
        assert!(usage.bucket_reads >= 5, "{usage:?}");
        assert_eq!(gets(&f.proxy), 0, "{:?}", f.proxy.seen());
    } else {
        assert!(gets(&f.proxy) >= 6, "new version has two blocks; the original has four untouched blocks, plus relocation retries: {:?}", f.proxy.seen());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_handles_keep_their_version_and_attributes_after_overwrite_rename_and_delete() {
    snapshot_survives(Backend::Api).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bucket_handles_keep_their_version_and_attributes_after_overwrite_rename_and_delete() {
    snapshot_survives(Backend::Bucket).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_handle_reads_follow_file_and_folder_moves_without_feed_delivery_then_delete() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend, 0).await;
        let body = bytes_of(9, 5 * MIB + 17);
        f.put("left/sub/file", body.clone()).await;
        f.put("right/", Bytes::new()).await;
        let ns = f.session().await;
        let left = ns.lookup(ns.root(), "left").await.unwrap().ino;
        let sub = ns.lookup(left, "sub").await.unwrap().ino;
        let before = ns.lookup(sub, "file").await.unwrap();
        let fh = ns.open(before.ino, false).await.unwrap();
        f.remote.rename("drv", "left/sub/file", "right/moved", Default::default()).await.unwrap();
        assert_eq!(ns.read(fh, 2 * MIB, 31).await.unwrap(), body.slice(2 * MIB as usize..2 * MIB as usize + 31));
        f.remote.rename("drv", "right/", "destination/", Default::default()).await.unwrap();
        assert_eq!(ns.read(fh, 0, 31).await.unwrap(), body.slice(..31));
        f.remote.delete_object("drv", "destination/moved", Default::default()).await.unwrap();
        assert_eq!(ns.read(fh, 4 * MIB, MIB + 100).await.unwrap(), body.slice(4 * MIB as usize..));
        assert_eq!(ns.handle_attr(fh).unwrap(), before);
        ns.close(fh).await.unwrap();
        if let Some(bucket) = &f.bucket {
            let usage = bucket.usage();
            assert!(usage.bucket_reads >= 4, "every cold content block was served by the bucket: {usage:?}");
            assert!(usage.api_reads <= 1, "only the original moved key may try the API before relocation: {usage:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn feed_delivered_file_and_folder_moves_use_ancestors_without_scanning() {
    for backend in [Backend::Api, Backend::Bucket] {
        for folder_move in [false, true] {
            let f = Fixture::new(backend, 0).await;
            let body = bytes_of(12, MIB);
            f.put("folder/before", body.clone()).await;
            f.put("unrelated/", Bytes::new()).await;
            let ns = f.session().await;
            let folder = ns.lookup(ns.root(), "folder").await.unwrap().ino;
            let before = ns.lookup(folder, "before").await.unwrap();
            let fh = ns.open(before.ino, false).await.unwrap();
            if folder_move {
                f.remote.rename("drv", "folder/", "moved/", Default::default()).await.unwrap();
                ns.invalidate(&[Invalidation::Subtree("folder/".into()), Invalidation::Subtree("moved/".into())]).await.unwrap();
            } else {
                f.remote.rename("drv", "folder/before", "folder/after", Default::default()).await.unwrap();
                ns.invalidate(&changes(&["folder/before", "folder/after"])).await.unwrap();
            }
            f.proxy.clear();
            assert_eq!(ns.read(fh, 0, 31).await.unwrap(), body.slice(..31));
            let requests = f.proxy.seen();
            assert!(!requests.iter().any(|r| r.contains("x-voidfs-deleted") || r.contains("unrelated")), "{requests:?}");
            assert_eq!(requests.iter().filter(|r| r.contains("x-voidfs-list")).count(), 2, "{requests:?}");
            assert_eq!(ns.handle_attr(fh).unwrap(), before);
            ns.close(fh).await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cross_folder_move_already_in_the_namespace_needs_no_listing_scan() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend, 0).await;
        let body = bytes_of(13, MIB);
        f.put("left/before", body.clone()).await;
        f.put("right/", Bytes::new()).await;
        let ns = f.session().await;
        let left = ns.lookup(ns.root(), "left").await.unwrap().ino;
        let right = ns.lookup(ns.root(), "right").await.unwrap().ino;
        let before = ns.lookup(left, "before").await.unwrap();
        let fh = ns.open(before.ino, false).await.unwrap();
        f.remote.rename("drv", "left/before", "right/after", Default::default()).await.unwrap();
        ns.invalidate(&changes(&["left/before", "right/after"])).await.unwrap();
        assert_eq!(ns.lookup(right, "after").await.unwrap().ino, before.ino);
        f.proxy.clear();
        assert_eq!(ns.read(fh, 0, 31).await.unwrap(), body.slice(..31));
        let requests = f.proxy.seen();
        assert!(!requests.iter().any(|r| r.contains("x-voidfs-list") || r.contains("x-voidfs-deleted")), "{requests:?}");
        assert_eq!(ns.handle_attr(fh).unwrap(), before);
        ns.close(fh).await.unwrap();
    }
}

struct MissingFetcher { calls: std::sync::atomic::AtomicUsize }

impl Fetch for MissingFetcher {
    fn fetch<'a>(&'a self, _c: &'a Content, _offset: u64, _len: u64) -> futures::future::BoxFuture<'a, voidfs_client::Result<Bytes>> {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(voidfs_sdk::Error::Service(voidfs_sdk::ServiceError { status: 404, code: "NoSuchVersion".into(), message: String::new(),
                request_id: None, current_version_id: None, retry_after: None }).into())
        })
    }
}

async fn missing_session(f: &Fixture) -> (Session, Arc<MissingFetcher>, u64) {
    let fetcher = Arc::new(MissingFetcher { calls: Default::default() });
    let cache = Cache::open(f.store.clone(), fetcher.clone(), config(0)).await.unwrap();
    let ns = Session::new(f.store.clone(), f.client.clone(), cache, "drv", f.connectivity.clone()).await.unwrap();
    let fh = ns.open(ns.lookup(ns.root(), "file").await.unwrap().ino, false).await.unwrap();
    (ns, fetcher, fh)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_version_at_its_resolved_key_stops_after_one_scan() {
    let f = Fixture::new(Backend::Api, 0).await;
    f.put("file", Bytes::from_static(b"bytes")).await;
    let (ns, fetcher, fh) = missing_session(&f).await;
    f.proxy.clear();
    let error = ns.read(fh, 0, 1).await.unwrap_err();
    assert_eq!(error, FsError::Stale);
    assert_eq!(error.errno(), libc::ESTALE);
    assert_eq!(fetcher.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(f.proxy.seen().len(), 3, "one stable root/deleted/root scan: {:?}", f.proxy.seen());
    ns.close(fh).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unseen_move_beyond_the_scan_budget_returns_again() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend, 0).await;
        let body = bytes_of(14, 101);
        f.put("file", body.clone()).await;
        for i in 0..20 { f.put(&format!("dir{i:02}/"), Bytes::new()).await; }
        let ns = f.session().await;
        let before = ns.lookup(ns.root(), "file").await.unwrap();
        let fh = ns.open(before.ino, false).await.unwrap();
        f.remote.rename("drv", "file", "dir19/moved", Default::default()).await.unwrap();
        f.proxy.clear();
        let error = ns.read(fh, 0, 31).await.unwrap_err();
        assert_eq!(error, FsError::Again);
        assert_eq!(error.errno(), libc::EAGAIN);
        let requests = f.proxy.seen();
        assert_eq!(requests.iter().filter(|r| r.contains("x-voidfs-list") || r.contains("x-voidfs-deleted")).count(), 16, "{requests:?}");
        ns.invalidate(&changes(&["file", "dir19/moved"])).await.unwrap();
        let dir = ns.lookup(ns.root(), "dir19").await.unwrap().ino;
        assert_eq!(ns.lookup(dir, "moved").await.unwrap().ino, before.ino);
        f.proxy.clear();
        assert_eq!(ns.read(fh, 0, 31).await.unwrap(), body.slice(..31));
        assert!(!f.proxy.seen().iter().any(|r| r.contains("x-voidfs-deleted")));
        ns.close(fh).await.unwrap();
    }
}

struct MovingMissingFetcher { remote: Client, calls: std::sync::atomic::AtomicUsize }

impl Fetch for MovingMissingFetcher {
    fn fetch<'a>(&'a self, c: &'a Content, _offset: u64, _len: u64) -> futures::future::BoxFuture<'a, voidfs_client::Result<Bytes>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let parent = c.key.rsplit_once('/').map(|(parent, _)| format!("{parent}/")).unwrap_or_default();
            self.remote.rename("drv", &c.key, &format!("{parent}moved{n}"), Default::default()).await?;
            Err(voidfs_sdk::Error::Service(voidfs_sdk::ServiceError { status: 404, code: "NoSuchVersion".into(), message: String::new(),
                request_id: None, current_version_id: None, retry_after: None }).into())
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_read_relocations_share_one_scan_budget() {
    let f = Fixture::new(Backend::Api, 0).await;
    f.put("a/", Bytes::new()).await;
    f.put("z/file", Bytes::from_static(b"bytes")).await;
    let fetcher = Arc::new(MovingMissingFetcher { remote: f.remote.clone(), calls: Default::default() });
    let cache = Cache::open(f.store.clone(), fetcher.clone(), config(0)).await.unwrap();
    let ns = Session::new(f.store.clone(), f.client.clone(), cache, "drv", f.connectivity.clone()).await.unwrap();
    let z = ns.lookup(ns.root(), "z").await.unwrap().ino;
    let fh = ns.open(ns.lookup(z, "file").await.unwrap().ino, false).await.unwrap();
    f.proxy.clear();
    assert_eq!(ns.read(fh, 0, 1).await.unwrap_err(), FsError::Again);
    assert_eq!(fetcher.calls.load(std::sync::atomic::Ordering::SeqCst), 4);
    assert_eq!(f.proxy.seen().len(), 16, "each completed search costs five calls, all relocations share sixteen: {:?}", f.proxy.seen());
    ns.close(fh).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_read_relocations_share_the_resolution_time_budget() {
    let f = Fixture::new(Backend::Api, 0).await;
    f.put("file", Bytes::from_static(b"bytes")).await;
    let fetcher = Arc::new(MovingMissingFetcher { remote: f.remote.clone(), calls: Default::default() });
    let cache = Cache::open(f.store.clone(), fetcher.clone(), config(0)).await.unwrap();
    let ns = Session::new(f.store.clone(), f.client.clone(), cache, "drv", f.connectivity.clone()).await.unwrap();
    let fh = ns.open(ns.lookup(ns.root(), "file").await.unwrap().ino, false).await.unwrap();
    f.proxy.slow(Duration::from_millis(400));
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), ns.read(fh, 0, 1)).await.unwrap().unwrap_err(), FsError::Again);
    assert!(fetcher.calls.load(std::sync::atomic::Ordering::SeqCst) < 4, "three delayed listing calls per relocation must consume the shared time budget before all four fetches");
    ns.close(fh).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_namespace_refresh_and_fallback_resolution_return_again() {
    for invalidate in [false, true] {
        let f = Fixture::new(Backend::Api, 0).await;
        f.put("file", Bytes::from_static(b"bytes")).await;
        let (ns, _, fh) = missing_session(&f).await;
        if invalidate { ns.invalidate(&changes(&["file"])).await.unwrap(); }
        f.proxy.clear();
        f.proxy.slow(Duration::from_secs(30));
        let error = tokio::time::timeout(Duration::from_secs(10), ns.read(fh, 0, 1)).await.expect("resolution has its own deadline").unwrap_err();
        assert_eq!(error, FsError::Again);
        assert_eq!(f.proxy.seen().len(), 1, "only the outstanding listing was sent");
        ns.close(fh).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_clamp_at_eof_and_handle_empty_zero_length_and_overflowing_ranges() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend, 0).await;
        let body = bytes_of(3, 101);
        f.put("file", body.clone()).await;
        f.put("empty", Bytes::new()).await;
        let ns = f.session().await;
        let fh = ns.open(ns.lookup(ns.root(), "file").await.unwrap().ino, false).await.unwrap();
        let empty = ns.open(ns.lookup(ns.root(), "empty").await.unwrap().ino, false).await.unwrap();
        f.proxy.clear();
        for (off, len) in [(101, 1), (102, 100), (u64::MAX, u64::MAX), (0, 0), (100, 0)] {
            assert!(ns.read(fh, off, len).await.unwrap().is_empty(), "{off}+{len}");
        }
        assert!(ns.read(empty, 0, u64::MAX).await.unwrap().is_empty());
        assert!(ns.read(empty, u64::MAX, 1).await.unwrap().is_empty());
        f.cache.settle().await;
        assert_eq!(gets(&f.proxy), 0, "empty ranges fetch no content");
        assert_eq!(f.cache.usage().fetches, 0);
        assert_eq!(ns.read(fh, 97, 20).await.unwrap(), body.slice(97..));
        assert_eq!(ns.read(fh, 7, u64::MAX).await.unwrap(), body.slice(7..));
        ns.close(fh).await.unwrap();
        ns.close(empty).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reads_on_one_handle_return_their_own_ranges() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend, 0).await;
        let body = bytes_of(4, 3 * MIB + 19);
        f.put("file", body.clone()).await;
        let ns = f.session().await;
        let fh = ns.open(ns.lookup(ns.root(), "file").await.unwrap().ino, false).await.unwrap();
        let ranges = [(2 * MIB + 17, 97), (0, 133), (MIB - 7, 21), (3 * MIB + 7, MIB), (73, 91)];
        let reads = (0..30).map(|i| {
            let (off, len) = ranges[i % ranges.len()];
            let ns = &ns;
            let body = &body;
            async move {
                let end = (off + len).min(body.len() as u64);
                assert_eq!(ns.read(fh, off, len).await.unwrap(), body.slice(off as usize..end as usize), "{off}+{len}");
            }
        });
        futures::future::join_all(reads).await;
        f.cache.settle().await;
        assert_eq!(f.cache.usage().fetches, 4, "one cache fill for each block");
        ns.close(fh).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sequential_handle_reads_keep_read_ahead_and_fetch_each_block_once() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend, 4 * MIB).await;
        let body = bytes_of(5, 8 * MIB + 17);
        f.put("file", body.clone()).await;
        let ns = f.session().await;
        let fh = ns.open(ns.lookup(ns.root(), "file").await.unwrap().ino, false).await.unwrap();
        f.proxy.clear();
        let step = 128 * 1024;
        for off in (0..body.len() as u64).step_by(step as usize) {
            let end = (off + step).min(body.len() as u64);
            assert_eq!(ns.read(fh, off, step).await.unwrap(), body.slice(off as usize..end as usize));
            if off == step {
                f.cache.settle().await;
                assert!(f.cache.usage().fetches >= 2, "the second read is still in block zero, but block one was fetched ahead");
                if f.bucket.is_none() { assert!(gets(&f.proxy) >= 2, "{:?}", f.proxy.seen()); }
            }
        }
        f.cache.settle().await;
        assert_eq!(f.cache.usage().fetches, 9);
        if let Some(bucket) = &f.bucket {
            assert_eq!((bucket.usage().api_reads, bucket.usage().bucket_reads), (0, 9));
        } else {
            assert_eq!(gets(&f.proxy), 9, "{:?}", f.proxy.seen());
        }
        ns.close(fh).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_handles_read_cached_disk_blocks_and_fail_uncached_blocks_without_requests() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend, 0).await;
        let body = bytes_of(6, 4 * MIB);
        f.put("file", body.clone()).await;
        let ns = f.session().await;
        let ino = ns.lookup(ns.root(), "file").await.unwrap().ino;
        let fh = ns.open(ino, false).await.unwrap();
        assert_eq!(ns.read(fh, 0, 100).await.unwrap(), body.slice(..100));
        f.cache.settle().await;
        f.cache.drop_memory();
        f.offline();
        f.proxy.clear();
        let usage = f.bucket.as_ref().map(|bucket| bucket.usage());
        let offline_fh = ns.open(ino, false).await.unwrap();
        assert_eq!(ns.read(offline_fh, 0, 100).await.unwrap(), body.slice(..100));
        assert!(f.cache.usage().disk_hits > 0);
        let error = tokio::time::timeout(Duration::from_secs(2), ns.read(fh, 3 * MIB, 100)).await.expect("offline reads fail at once").unwrap_err();
        assert_eq!(error, FsError::Offline);
        assert_eq!(error.errno(), libc::ENETDOWN);
        f.cache.settle().await;
        assert!(f.proxy.seen().is_empty(), "{:?}", f.proxy.seen());
        if let Some(bucket) = &f.bucket { assert_eq!(Some(bucket.usage()), usage); }
        ns.close(fh).await.unwrap();
        ns.close(offline_fh).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_offline_state_blocks_fetches_even_when_the_fetcher_has_no_connectivity() {
    let f = Fixture::new(Backend::Api, 0).await;
    f.put("file", bytes_of(7, MIB)).await;
    let cache = Cache::open(f.store.clone(), Arc::new(ApiFetcher::new(f.client.clone())), config(0)).await.unwrap();
    let ns = Session::new(f.store.clone(), f.client.clone(), cache.clone(), "drv", f.connectivity.clone()).await.unwrap();
    let fh = ns.open(ns.lookup(ns.root(), "file").await.unwrap().ino, false).await.unwrap();
    f.offline();
    f.proxy.clear();
    assert_eq!(ns.read(fh, 0, 1).await.unwrap_err(), FsError::Offline);
    cache.settle().await;
    assert!(f.proxy.seen().is_empty(), "{:?}", f.proxy.seen());
    ns.close(fh).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_rejects_old_handles_and_new_handles_read_with_the_persistent_inode() {
    let f = Fixture::new(Backend::Api, 0).await;
    let body = bytes_of(8, 101);
    f.put("file", body.clone()).await;
    let ns = f.session().await;
    let ino = ns.lookup(ns.root(), "file").await.unwrap().ino;
    let old = ns.open(ino, false).await.unwrap();
    let generation = ns.generation();
    assert_eq!(ns.read(old, 0, 101).await.unwrap(), body);
    f.cache.settle().await;
    drop(ns);
    drop(f.cache);
    drop(f.store);
    let store = Arc::new(Store::open(f.state.path()).unwrap());
    let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(f.client.clone()).with_connectivity(f.connectivity.clone())), config(0)).await.unwrap();
    let ns = Session::new(store, f.client, cache, "drv", f.connectivity).await.unwrap();
    assert!(ns.generation() > generation);
    assert_eq!(ns.read(old, 0, 1).await.unwrap_err(), FsError::Stale);
    assert_eq!(ns.close(old).await.unwrap_err(), FsError::Stale);
    assert_eq!(ns.handle_attr(old).unwrap_err(), FsError::Stale);
    assert_eq!(ns.getattr(ino).await.unwrap().ino, ino);
    let new = ns.open(ino, false).await.unwrap();
    assert_ne!(new, old);
    assert_eq!(ns.read(new, 0, 101).await.unwrap(), body);
    ns.close(new).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closed_and_unknown_handles_are_bad_descriptors_and_ids_are_not_reused() {
    let f = Fixture::new(Backend::Api, 0).await;
    f.put("file", Bytes::from_static(b"bytes")).await;
    let ns = f.session().await;
    let ino = ns.lookup(ns.root(), "file").await.unwrap().ino;
    let old = ns.open(ino, false).await.unwrap();
    ns.close(old).await.unwrap();
    let new = ns.open(ino, false).await.unwrap();
    assert!(new > old);
    let unknown = (u64::from(ns.generation()) << 32) | u64::from(u32::MAX);
    for fh in [0, old, unknown] {
        let error = ns.read(fh, 0, 0).await.unwrap_err();
        assert_eq!(error, FsError::BadHandle);
        assert_eq!(error.errno(), libc::EBADF);
        assert_eq!(ns.close(fh).await.unwrap_err(), FsError::BadHandle);
        assert_eq!(ns.handle_attr(fh).unwrap_err(), FsError::BadHandle);
    }
    ns.close(new).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_rejects_directories_writes_and_stale_inodes() {
    let f = Fixture::new(Backend::Api, 0).await;
    f.put("folder/", Bytes::new()).await;
    f.put("file", Bytes::from_static(b"bytes")).await;
    let ns = f.session().await;
    let folder = ns.lookup(ns.root(), "folder").await.unwrap();
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    let error = ns.open(folder.ino, false).await.unwrap_err();
    assert_eq!(error, FsError::IsDir);
    assert_eq!(error.errno(), libc::EISDIR);
    let error = ns.open(file.ino, true).await.unwrap_err();
    assert_eq!(error, FsError::ReadOnly);
    assert_eq!(error.errno(), libc::EROFS);
    assert_eq!(ns.open(0, false).await.unwrap_err(), FsError::Stale);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn readlink_returns_the_exact_target_offline_and_rejects_other_kinds() {
    let target = "../cafe\u{301}/../target";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().route("/drv", axum::routing::get(move || async move {
        axum::Json(serde_json::json!({ "prefix": "", "seq": 1, "entries": [
            { "name": "link", "kind": "symlink", "objectId": "o-link", "versionId": "1.0", "size": target.len(), "etag": "etag-link", "target": target },
            { "name": "file", "kind": "file", "objectId": "o-file", "versionId": "1.0", "size": 1, "etag": "etag-file" },
            { "name": "folder/", "kind": "folder", "objectId": "o-folder" }
        ] }))
    }));
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let proxy = Proxy::start(&endpoint).await;
    let client = client_for(&proxy.endpoint, Config::default());
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let connectivity = Connectivity::default();
    let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(client.clone()).with_connectivity(connectivity.clone())), config(0)).await.unwrap();
    let ns = Session::new(store, client, cache, "drv", connectivity.clone()).await.unwrap();
    let link = ns.lookup(ns.root(), "link").await.unwrap();
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    let folder = ns.lookup(ns.root(), "folder").await.unwrap();
    assert_eq!(link.kind, Kind::Symlink);
    assert_eq!(ns.readlink(link.ino).await.unwrap(), target);
    assert_eq!(ns.open(link.ino, false).await.unwrap_err(), FsError::Unsupported);
    for _ in 0..3 { connectivity.unanswered(); }
    proxy.clear();
    assert_eq!(ns.readlink(link.ino).await.unwrap(), target);
    for ino in [file.ino, folder.ino] {
        let error = ns.readlink(ino).await.unwrap_err();
        assert_eq!(error, FsError::InvalidName);
        assert_eq!(error.errno(), libc::EINVAL);
    }
    assert_eq!(ns.readlink(0).await.unwrap_err(), FsError::Stale);
    assert!(proxy.seen().is_empty());
    task.abort();
}

struct ChangedFetcher;

impl Fetch for ChangedFetcher {
    fn fetch<'a>(&'a self, c: &'a Content, _offset: u64, _len: u64) -> futures::future::BoxFuture<'a, voidfs_client::Result<Bytes>> {
        Box::pin(async move { Err(Error::Changed { key: c.key.clone(), expected: c.etag.clone(), got: "wrong-etag".into() }) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_snapshot_content_is_an_io_error() {
    let f = Fixture::new(Backend::Api, 0).await;
    f.put("file", Bytes::from_static(b"bytes")).await;
    let cache = Cache::open(f.store.clone(), Arc::new(ChangedFetcher), config(0)).await.unwrap();
    let ns = Session::new(f.store.clone(), f.client.clone(), cache, "drv", f.connectivity.clone()).await.unwrap();
    let fh = ns.open(ns.lookup(ns.root(), "file").await.unwrap().ino, false).await.unwrap();
    let error = ns.read(fh, 0, 1).await.unwrap_err();
    assert!(matches!(&error, FsError::Io(message) if message.contains("wrong-etag")), "{error}");
    assert_eq!(error.errno(), libc::EIO);
    ns.close(fh).await.unwrap();
}

struct ChangedAtOriginalKey { api: ApiFetcher }

impl Fetch for ChangedAtOriginalKey {
    fn fetch<'a>(&'a self, c: &'a Content, offset: u64, len: u64) -> futures::future::BoxFuture<'a, voidfs_client::Result<Bytes>> {
        Box::pin(async move {
            if c.key == "before" {
                Err(Error::Changed { key: c.key.clone(), expected: c.etag.clone(), got: "replacement-etag".into() })
            } else {
                self.api.fetch(c, offset, len).await
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_moved_snapshot_relocates_when_a_replacement_at_its_old_key_has_a_different_etag() {
    let f = Fixture::new(Backend::Api, 0).await;
    let body = bytes_of(11, 101);
    f.put("before", body.clone()).await;
    let fetcher = Arc::new(ChangedAtOriginalKey { api: ApiFetcher::new(f.client.clone()).with_connectivity(f.connectivity.clone()) });
    let cache = Cache::open(f.store.clone(), fetcher, config(0)).await.unwrap();
    let ns = Session::new(f.store.clone(), f.client.clone(), cache, "drv", f.connectivity.clone()).await.unwrap();
    let before = ns.lookup(ns.root(), "before").await.unwrap();
    let fh = ns.open(before.ino, false).await.unwrap();
    f.remote.rename("drv", "before", "after", Default::default()).await.unwrap();
    f.put("before", Bytes::from_static(b"replacement")).await;
    // Restoring several objects together can reuse a transaction version ID. A fetcher's ETag
    // check then reports Changed at the replacement's key, rather than NoSuchVersion.
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), body);
    assert_eq!(ns.handle_attr(fh).unwrap(), before);
    ns.close(fh).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_key_uses_the_namespace_before_scanning_for_a_mismatch() {
    let f = Fixture::new(Backend::Api, 0).await;
    let body = bytes_of(15, 101);
    f.put("before", body.clone()).await;
    let fetcher = Arc::new(ChangedAtOriginalKey { api: ApiFetcher::new(f.client.clone()).with_connectivity(f.connectivity.clone()) });
    let cache = Cache::open(f.store.clone(), fetcher, config(0)).await.unwrap();
    let ns = Session::new(f.store.clone(), f.client.clone(), cache, "drv", f.connectivity.clone()).await.unwrap();
    let before = ns.lookup(ns.root(), "before").await.unwrap();
    let fh = ns.open(before.ino, false).await.unwrap();
    f.remote.rename("drv", "before", "after", Default::default()).await.unwrap();
    f.put("before", Bytes::from_static(b"replacement")).await;
    ns.invalidate(&changes(&["before", "after"])).await.unwrap();
    f.proxy.clear();
    assert_eq!(ns.read(fh, 0, 101).await.unwrap(), body);
    assert_eq!(ns.handle_attr(fh).unwrap(), before);
    let requests = f.proxy.seen();
    assert!(!requests.iter().any(|r| r.contains("x-voidfs-deleted")), "{requests:?}");
    assert_eq!(requests.iter().filter(|r| r.contains("x-voidfs-list")).count(), 1, "{requests:?}");
    ns.close(fh).await.unwrap();
}

struct GatedFetcher {
    body: Bytes,
    requested: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl Fetch for GatedFetcher {
    fn fetch<'a>(&'a self, _c: &'a Content, offset: u64, len: u64) -> futures::future::BoxFuture<'a, voidfs_client::Result<Bytes>> {
        Box::pin(async move {
            self.requested.notify_one();
            self.release.notified().await;
            Ok(self.body.slice(offset as usize..(offset + len) as usize))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_a_handle_preserves_an_accepted_read_and_rejects_later_reads() {
    let f = Fixture::new(Backend::Api, 0).await;
    let body = bytes_of(10, 101);
    f.put("file", body.clone()).await;
    let fetcher = Arc::new(GatedFetcher { body: body.clone(), requested: Default::default(), release: Default::default() });
    let cache = Cache::open(f.store.clone(), fetcher.clone(), config(0)).await.unwrap();
    let ns = Arc::new(Session::new(f.store.clone(), f.client.clone(), cache, "drv", f.connectivity.clone()).await.unwrap());
    let fh = ns.open(ns.lookup(ns.root(), "file").await.unwrap().ino, false).await.unwrap();
    let reading = ns.clone();
    let task = tokio::spawn(async move { reading.read(fh, 0, 101).await });
    tokio::time::timeout(Duration::from_secs(5), fetcher.requested.notified()).await.expect("read entered fetcher");
    ns.close(fh).await.unwrap();
    assert_eq!(ns.read(fh, 0, 1).await.unwrap_err(), FsError::BadHandle);
    fetcher.release.notify_one();
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap(), body);
    assert_eq!(ns.handle_attr(fh).unwrap_err(), FsError::BadHandle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_rejects_a_listing_invalidated_in_flight_and_binds_the_fresh_snapshot() {
    struct ListingState {
        calls: std::sync::atomic::AtomicUsize,
        requested: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    let state = Arc::new(ListingState { calls: Default::default(), requested: Default::default(), release: Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().route("/drv", axum::routing::get(|axum::extract::State(state): axum::extract::State<Arc<ListingState>>| async move {
        let call = state.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 1 {
            state.requested.notify_one();
            state.release.notified().await;
        }
        let fresh = call >= 2;
        axum::Json(serde_json::json!({ "prefix": "", "seq": if fresh { 2 } else { 1 }, "entries": [{
            "name": "file", "kind": "file", "objectId": "o-file", "versionId": if fresh { "2.0" } else { "1.0" },
            "size": if fresh { 101 } else { 8 }, "etag": if fresh { "new-etag" } else { "old-etag" }
        }] }))
    })).with_state(state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let client = client_for(&endpoint, Config::default());
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(client.clone())), config(0)).await.unwrap();
    let ns = Arc::new(Session::new(store, client, cache, "drv", Connectivity::default()).await.unwrap());
    let before = ns.lookup(ns.root(), "file").await.unwrap();
    ns.invalidate(&changes(&["file"])).await.unwrap();
    let opening = ns.clone();
    let open = tokio::spawn(async move { opening.open(before.ino, false).await });
    tokio::time::timeout(Duration::from_secs(5), state.requested.notified()).await.expect("open's refresh reached the gate");
    ns.invalidate(&changes(&["file"])).await.unwrap();
    state.release.notify_one();
    let fh = tokio::time::timeout(Duration::from_secs(5), open).await.unwrap().unwrap().unwrap();
    let snapshot = ns.handle_attr(fh).unwrap();
    assert_eq!((snapshot.size, snapshot.version_id.as_deref(), snapshot.etag.as_deref()), (101, Some("2.0"), Some("new-etag")));
    assert_eq!(state.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    ns.close(fh).await.unwrap();
    task.abort();
}
