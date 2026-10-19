// SPDX-License-Identifier: Apache-2.0
//! A save published from a mount seeds the read cache with its own bytes, under the version it
//! published, so that the file reads offline after a restart as a file read here does (fault 2 of
//! step 5's 18 October check). A version the stage doesn't hold, or one too big to keep, isn't
//! seeded.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use common::client_for;
use voidfs_client::mount::{FsError, Session, StagingConfig, Sync};
use voidfs_client::{ApiFetcher, Cache, CacheConfig, Connectivity, Content, Link, Queue, QueueConfig, Scope, Store};
use voidfs_sdk::{Client, Config, PutOptions, ReadOptions};
use voidfs_server::test_server::TestServer;

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(1 << 50) }

fn staging() -> StagingConfig {
    StagingConfig { min_free_bytes: 0, free_space: Some(plenty), quiet_period: None, disk_flush: None, ..Default::default() }
}

/// Blocks of 4 bytes, so that a few bytes span several, and a cap of `max_bytes`.
fn cache_config(max_bytes: u64) -> CacheConfig {
    CacheConfig { min_free_bytes: 0, free_space: Some(plenty), block_size: 4, read_ahead_bytes: 0, max_bytes, ..Default::default() }
}

struct Fixture {
    _server: TestServer, remote: Client, client: Client, state: tempfile::TempDir, max_bytes: u64,
    store: Arc<Store>, cache: Cache, queue: Queue, connectivity: Connectivity,
}

impl Fixture {
    async fn new(max_bytes: u64) -> Self {
        let server = TestServer::start().await.unwrap();
        let remote = client_for(&server.endpoint, Config::default());
        let connectivity = Connectivity::default();
        let client = client_for(&server.endpoint, Config { observer: Some(Arc::new(connectivity.clone())), ..Default::default() });
        remote.create_drive("drv", Default::default()).await.unwrap();
        let state = tempfile::tempdir().unwrap();
        let (store, cache, queue) = Self::open(state.path(), &client, &connectivity, max_bytes).await;
        Self { _server: server, remote, client, state, max_bytes, store, cache, queue, connectivity }
    }

    async fn open(dir: &Path, client: &Client, connectivity: &Connectivity, max_bytes: u64) -> (Arc<Store>, Cache, Queue) {
        let store = Arc::new(Store::open(dir).unwrap());
        let fetcher = Arc::new(ApiFetcher::new(client.clone()).with_connectivity(connectivity.clone()));
        let cache = Cache::open(store.clone(), fetcher, cache_config(max_bytes)).await.unwrap();
        let queue = Queue::open(store.clone(), client.clone(), QueueConfig { retry_max: Duration::from_millis(50), connectivity: Some(connectivity.clone()), ..Default::default() }).await.unwrap();
        queue.pause(Scope::All).await.unwrap();
        (store, cache, queue)
    }

    async fn session(&self) -> Session {
        Session::new_writable_with_config(self.store.clone(), self.client.clone(), self.cache.clone(), self.queue.clone(), "drv", self.connectivity.clone(), staging()).await.unwrap()
    }

    /// Publishes what is queued, then pauses again so later edits stay local.
    async fn publish(&self) {
        self.queue.resume(Scope::All).await.unwrap();
        self.queue.settle().await;
        self.queue.pause(Scope::All).await.unwrap();
    }

    /// Stops cleanly, as the daemon does, and starts again with the server out of reach: its
    /// address refuses connections, and the link is offline.
    async fn restart_offline(self, session: Session) -> (Self, Session) {
        drop(session);
        self.queue.close().await;
        self.cache.settle().await;
        let Self { _server, remote, client: _, state, max_bytes, store, cache, queue, connectivity } = self;
        drop((store, cache, queue));
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        let client = client_for(&format!("http://{closed}"), Config { observer: Some(Arc::new(connectivity.clone())), ..Default::default() });
        for _ in 0..3 { connectivity.unanswered(); }
        assert_eq!(connectivity.link(), Link::Offline);
        let (store, cache, queue) = Self::open(state.path(), &client, &connectivity, max_bytes).await;
        let f = Self { _server, remote, client, state, max_bytes, store, cache, queue, connectivity };
        let session = f.session().await;
        (f, session)
    }

    /// The version at `key` as the server has it now.
    async fn current(&self, key: &str) -> Content {
        Content::of("drv", key, &self.remote.head_object("drv", key, ReadOptions::default()).await.unwrap())
    }
}

async fn write_new(ns: &Session, name: &str, body: &[u8]) -> u64 {
    let ino = ns.create(ns.root(), name, 0o644).await.unwrap().ino;
    let fh = ns.open(ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::copy_from_slice(body)).await.unwrap();
    ns.close(fh).await.unwrap();
    ino
}

async fn read_all(ns: &Session, name: &str) -> Result<Bytes, FsError> {
    let ino = ns.lookup(ns.root(), name).await?.ino;
    let fh = ns.open(ino, false).await?;
    let read = ns.read(fh, 0, u64::MAX).await;
    ns.close(fh).await?;
    read
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_saved_file_reads_offline_after_a_restart() {
    let f = Fixture::new(1 << 30).await;
    f.remote.put_object("drv", "edited", Bytes::from_static(b"0123456789ab"), PutOptions::default()).await.unwrap();
    f.remote.put_object("drv", "half-read", Bytes::from_static(b"abcdefghijkl"), PutOptions::default()).await.unwrap();
    let ns = f.session().await;
    ns.readdir(ns.root(), None, 100).await.unwrap();
    // A new file, the whole of it written here.
    write_new(&ns, "new", b"saved on this Mac").await;
    // An edit of a file read here: the published version is its base's bytes around the edit.
    let edited = ns.lookup(ns.root(), "edited").await.unwrap().ino;
    let fh = ns.open(edited, true).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"0123456789ab"));
    ns.write(fh, 5, Bytes::from_static(b"EDIT")).await.unwrap();
    ns.close(fh).await.unwrap();
    // An edit of a file read only in part: the block of its base this Mac never read isn't here
    // to seed from.
    let half = ns.lookup(ns.root(), "half-read").await.unwrap().ino;
    let fh = ns.open(half, true).await.unwrap();
    assert_eq!(ns.read(fh, 0, 8).await.unwrap(), Bytes::from_static(b"abcdefgh"));
    ns.write(fh, 5, Bytes::from_static(b"EDIT")).await.unwrap();
    ns.close(fh).await.unwrap();
    f.publish().await;
    for name in ["new", "edited", "half-read"] {
        assert_eq!(ns.lookup(ns.root(), name).await.unwrap().sync, Sync::Saved, "{name}");
    }
    assert_eq!(f.remote.get_object("drv", "edited", ReadOptions::default()).await.unwrap().body, Bytes::from_static(b"01234EDIT9ab"));
    f.cache.settle().await;
    let seeded = f.cache.usage().seeded;

    let (f, ns) = f.restart_offline(ns).await;
    assert_eq!(read_all(&ns, "new").await.unwrap(), Bytes::from_static(b"saved on this Mac"));
    assert_eq!(read_all(&ns, "edited").await.unwrap(), Bytes::from_static(b"01234EDIT9ab"));
    let half = ns.lookup(ns.root(), "half-read").await.unwrap().ino;
    let fh = ns.open(half, false).await.unwrap();
    assert_eq!(ns.read(fh, 0, 8).await.unwrap(), Bytes::from_static(b"abcdeEDI"));
    assert_eq!(ns.read(fh, 8, 4).await.unwrap_err(), FsError::Offline, "its unread base block was never here");
    ns.close(fh).await.unwrap();
    assert_eq!(f.cache.usage().fetches, 0);
    // "new" in 5 blocks, "edited" in 3, and "half-read"'s first 2.
    assert_eq!(seeded, 10);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_save_changed_again_before_its_upload_lands_seeds_only_the_later_version() {
    let f = Fixture::new(1 << 30).await;
    let ns = f.session().await;
    ns.readdir(ns.root(), None, 100).await.unwrap();
    let ino = ns.create(ns.root(), "doc", 0o644).await.unwrap().ino;
    let fh = ns.open(ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"version1")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    // A later change, the same size, not flushed when the first version lands.
    ns.write(fh, 0, Bytes::from_static(b"VERSION2")).await.unwrap();
    f.publish().await;
    let first = f.current("doc").await;
    assert_eq!(f.remote.get_object("drv", "doc", ReadOptions::default()).await.unwrap().body, Bytes::from_static(b"version1"));
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Pending);
    f.cache.settle().await;
    assert_eq!(f.cache.read_cached(&first, 0, 8).await.unwrap(), None, "the stage's bytes aren't the first version's");
    assert_eq!(f.cache.usage().seeded, 0);

    ns.close(fh).await.unwrap();
    f.publish().await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Saved);
    f.cache.settle().await;
    let (f, ns) = f.restart_offline(ns).await;
    assert_eq!(read_all(&ns, "doc").await.unwrap(), Bytes::from_static(b"VERSION2"));
    assert_eq!(f.cache.read_cached(&first, 0, 8).await.unwrap(), None);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_save_larger_than_a_quarter_of_the_cache_is_not_seeded() {
    // A cap of 64 bytes seeds saves of up to 16.
    let f = Fixture::new(64).await;
    let ns = f.session().await;
    ns.readdir(ns.root(), None, 100).await.unwrap();
    write_new(&ns, "fits", b"sixteen bytes ok").await;
    write_new(&ns, "too-big", b"seventeen bytes!!").await;
    f.publish().await;
    f.cache.settle().await;
    let seeded = f.cache.usage().seeded;

    let (f, ns) = f.restart_offline(ns).await;
    assert_eq!(read_all(&ns, "fits").await.unwrap(), Bytes::from_static(b"sixteen bytes ok"));
    assert_eq!(read_all(&ns, "too-big").await.unwrap_err(), FsError::Offline);
    assert_eq!(seeded, 4, "\"fits\" alone");
    f.queue.close().await;
}
