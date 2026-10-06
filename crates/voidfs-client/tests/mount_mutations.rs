// SPDX-License-Identifier: Apache-2.0
//! Durable mount namespace mutations, exercised before and after queue publication.

mod common;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use common::{Proxy, client_for};
use rusqlite::{Connection, params};
use voidfs_client::mount::{FsError, RenameMode, Session, Sync};
use voidfs_client::{ApiFetcher, BucketConfig, BucketFetcher, Cache, CacheConfig, Connectivity, Fetch,
    Invalidation, Link, Op, Queue, QueueConfig, Scope, State, Store};
use voidfs_sdk::{Client, Config, Kind, ReadOptions};
use voidfs_server::test_server::{Rules, TestServer};

#[derive(Clone, Copy)]
enum Backend { Api, Bucket }

struct Fixture {
    _server: TestServer,
    proxy: Proxy,
    remote: Client,
    client: Client,
    state: tempfile::TempDir,
    store: Arc<Store>,
    cache: Cache,
    queue: Queue,
    connectivity: Connectivity,
}

fn queue_config() -> QueueConfig {
    QueueConfig { retry_max: Duration::from_millis(50), ..Default::default() }
}

impl Fixture {
    async fn new() -> Self { Self::with_backend(Backend::Api).await }

    async fn with_backend(backend: Backend) -> Self {
        let server = match backend {
            Backend::Api => TestServer::start().await.unwrap(),
            Backend::Bucket => TestServer::with_storage_credentials(Vec::new(), Rules::default()).await.unwrap(),
        };
        let proxy = Proxy::start(&server.endpoint).await;
        let remote = client_for(&server.endpoint, Config::default());
        let client = client_for(&proxy.endpoint, Config::default());
        remote.create_drive("drv", Default::default()).await.unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(state.path()).unwrap());
        let connectivity = Connectivity::default();
        let fetcher: Arc<dyn Fetch> = match backend {
            Backend::Api => Arc::new(ApiFetcher::new(client.clone()).with_connectivity(connectivity.clone())),
            Backend::Bucket => Arc::new(BucketFetcher::new(client.clone(), BucketConfig::default()).with_connectivity(connectivity.clone())),
        };
        let cache = Cache::open(store.clone(), fetcher, CacheConfig {
            min_free_bytes: 0, block_size: 4, read_ahead_bytes: 0, ..Default::default()
        }).await.unwrap();
        let queue = Queue::open(store.clone(), client.clone(), queue_config()).await.unwrap();
        queue.pause(Scope::All).await.unwrap();
        Self { _server: server, proxy, remote, client, state, store, cache, queue, connectivity }
    }

    async fn session(&self) -> Session { self.session_for("drv").await }

    async fn session_for(&self, drive: &str) -> Session {
        Session::new_writable(self.store.clone(), self.client.clone(), self.cache.clone(), self.queue.clone(),
            drive, self.connectivity.clone()).await.unwrap()
    }

    async fn put(&self, key: &str, body: &'static str) {
        self.remote.put_object("drv", key, body, Default::default()).await.unwrap();
    }

    async fn publish(&self) {
        self.queue.resume(Scope::All).await.unwrap();
        self.queue.settle().await;
    }

    fn offline(&self) {
        for _ in 0..3 { self.connectivity.unanswered(); }
        assert_eq!(self.connectivity.link(), Link::Offline);
    }

    fn db(&self) -> Connection { Connection::open(self.state.path().join("state.sqlite")).unwrap() }

    fn directory_generation(&self, ino: u64) -> u64 {
        self.db().query_row("SELECT generation FROM mount_dirs WHERE ino=?1", [ino], |r| r.get(0)).unwrap()
    }
}

async fn remote_body(client: &Client, key: &str) -> Option<Bytes> {
    match client.get_object("drv", key, ReadOptions::default()).await {
        Ok(object) => Some(object.body),
        Err(error) if error.status() == Some(404) => None,
        Err(error) => panic!("{error}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_is_exclusive_normalizes_names_and_opens_empty_files_locally() {
    let f = Fixture::new().await;
    f.put("remote", "kept").await;
    let ns = f.session().await;
    ns.readdir(ns.root(), None, 100).await.unwrap();
    let before = f.directory_generation(ns.root());
    let file = ns.create(ns.root(), "cafe\u{301}", 0o640).await.unwrap();
    assert_eq!((file.kind, file.size, file.mode, file.sync), (Kind::File, 0, 0o640, Sync::Pending));
    assert!(file.object_id.is_none() && file.version_id.is_none() && file.etag.is_none());
    assert_eq!(ns.lookup(ns.root(), "caf\u{e9}").await.unwrap(), file);
    let names = ns.readdir(ns.root(), None, 100).await.unwrap();
    assert_eq!(names.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["caf\u{e9}", "remote"]);
    assert!(f.directory_generation(ns.root()) > before);
    let queued = f.queue.status().await.unwrap();
    assert_eq!(queued.items.len(), 1);
    assert_eq!((queued.items[0].op, queued.items[0].key.as_str(), queued.items[0].size), (Op::Put, "caf\u{e9}", 0));
    let unchanged = f.directory_generation(ns.root());
    assert_eq!(ns.create(ns.root(), "cafe\u{301}", 0o600).await.unwrap_err(), FsError::Exists);
    assert_eq!(ns.create(ns.root(), "remote", 0o600).await.unwrap_err(), FsError::Exists);
    assert_eq!(f.queue.status().await.unwrap(), queued);
    assert_eq!(f.directory_generation(ns.root()), unchanged);
    f.proxy.clear();
    let fh = ns.open(file.ino, false).await.unwrap();
    assert_eq!(ns.handle_attr(fh).unwrap(), file);
    assert!(ns.read(fh, 0, u64::MAX).await.unwrap().is_empty());
    assert!(ns.read(fh, u64::MAX, 1).await.unwrap().is_empty());
    assert_eq!(ns.open(file.ino, true).await.unwrap_err(), FsError::ReadOnly);
    ns.close(fh).await.unwrap();
    assert_eq!(ns.read(fh, 0, 1).await.unwrap_err(), FsError::BadHandle);
    assert!(f.proxy.seen().is_empty(), "empty local reads need no remote version");
    f.publish().await;
    assert_eq!(remote_body(&f.remote, "caf\u{e9}").await, Some(Bytes::new()));
    let head = f.remote.head_object("drv", "caf\u{e9}", Default::default()).await.unwrap();
    assert_eq!(head.mode.as_deref(), Some("0640"));
    assert!(f.queue.status().await.unwrap().items.iter().all(|item| item.state == State::Done));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_directories_are_complete_offline_and_publish_before_their_children() {
    let f = Fixture::new().await;
    let ns = f.session().await;
    ns.readdir(ns.root(), None, 100).await.unwrap();
    f.offline();
    f.proxy.clear();
    let dir = ns.mkdir(ns.root(), "parent", 0o750).await.unwrap();
    assert_eq!((dir.kind, dir.mode, dir.sync), (Kind::Folder, 0o750, Sync::Pending));
    assert!(ns.readdir(dir.ino, None, 100).await.unwrap().is_empty());
    let sub = ns.mkdir(dir.ino, "sub", 0o700).await.unwrap();
    let file = ns.create(sub.ino, "leaf", 0o600).await.unwrap();
    assert_eq!(ns.lookup(sub.ino, "leaf").await.unwrap(), file);
    assert_eq!(ns.lookup(dir.ino, "missing").await.unwrap_err(), FsError::NotFound);
    assert!(f.proxy.seen().is_empty(), "complete offline parents accept namespace changes immediately");
    let items = f.queue.status().await.unwrap().items;
    assert_eq!(items.iter().map(|item| (item.op, item.key.as_str())).collect::<Vec<_>>(),
        [(Op::Folder, "parent/"), (Op::Folder, "parent/sub/"), (Op::Put, "parent/sub/leaf")]);
    f.connectivity.answered(false);
    f.publish().await;
    for (key, mode) in [("parent/", "0750"), ("parent/sub/", "0700"), ("parent/sub/leaf", "0600")] {
        assert_eq!(f.remote.head_object("drv", key, Default::default()).await.unwrap().mode.as_deref(), Some(mode));
    }
    assert!(f.queue.status().await.unwrap().items.iter().all(|item| item.state == State::Done));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_uncached_parents_reject_mutations_without_a_partial_journal() {
    let f = Fixture::new().await;
    f.put("uncached/child", "remote").await;
    f.put("cached", "cached remote bytes").await;
    let ns = f.session().await;
    let parent = ns.lookup(ns.root(), "uncached").await.unwrap().ino;
    let cached = ns.lookup(ns.root(), "cached").await.unwrap();
    let fh = ns.open(cached.ino, false).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"cached remote bytes"));
    ns.close(fh).await.unwrap();
    f.cache.settle().await;
    // Rows upgraded from v5 have no remembered remote key.
    f.db().execute("UPDATE mount_inodes SET remote_key=NULL WHERE ino=?1", [cached.ino]).unwrap();
    f.offline();
    f.proxy.clear();
    assert_eq!(ns.create(parent, "new", 0o644).await.unwrap_err(), FsError::Offline);
    assert_eq!(ns.mkdir(parent, "newdir", 0o755).await.unwrap_err(), FsError::Offline);
    assert_eq!(ns.unlink(parent, "child").await.unwrap_err(), FsError::Offline);
    assert_eq!(ns.rmdir(parent, "child").await.unwrap_err(), FsError::Offline);
    assert_eq!(ns.rename(parent, "child", ns.root(), "moved", RenameMode::Exclusive).await.unwrap_err(), FsError::Offline);
    assert!(f.queue.status().await.unwrap().items.is_empty());
    assert!(f.proxy.seen().is_empty());
    f.queue.close().await;
    let Fixture { _server, proxy, remote: _, client, state, store, cache, queue, connectivity } = f;
    drop(ns);
    drop(cache);
    drop(queue);
    drop(store);
    let store = Arc::new(Store::open(state.path()).unwrap());
    let queue = Queue::open(store.clone(), client.clone(), queue_config()).await.unwrap();
    let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(client.clone()).with_connectivity(connectivity.clone())),
        CacheConfig { min_free_bytes: 0, block_size: 4, read_ahead_bytes: 0, ..Default::default() }).await.unwrap();
    let ns = Session::new_writable(store, client, cache, queue.clone(), "drv", connectivity).await.unwrap();
    proxy.clear();
    let fh = ns.open(cached.ino, false).await.unwrap();
    assert_eq!(ns.handle_attr(fh).unwrap(), cached);
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"cached remote bytes"));
    ns.close(fh).await.unwrap();
    assert!(proxy.seen().is_empty(), "offline migrated snapshots remain readable after reopen marks their listing stale");
    queue.close().await;
    drop(proxy);
    drop(_server);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unlink_and_rmdir_enforce_kind_and_effective_emptiness_and_publish_children_first() {
    let f = Fixture::new().await;
    f.put("dir/child", "remote").await;
    f.put("file", "remote").await;
    f.put("switch-file", "old file").await;
    f.put("switch-dir/", "").await;
    let ns = f.session().await;
    let dir = ns.lookup(ns.root(), "dir").await.unwrap();
    let child = ns.lookup(dir.ino, "child").await.unwrap();
    assert_eq!(ns.unlink(ns.root(), "dir").await.unwrap_err(), FsError::IsDir);
    assert_eq!(ns.rmdir(ns.root(), "file").await.unwrap_err(), FsError::NotDir);
    assert_eq!(ns.rmdir(ns.root(), "dir").await.unwrap_err(), FsError::NotEmpty);
    assert_eq!(ns.unlink(dir.ino, "missing").await.unwrap_err(), FsError::NotFound);
    assert!(f.queue.status().await.unwrap().items.is_empty());
    let before = f.directory_generation(dir.ino);
    ns.unlink(dir.ino, "child").await.unwrap();
    assert!(f.directory_generation(dir.ino) > before);
    assert_eq!(ns.lookup(dir.ino, "child").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.getattr(child.ino).await.unwrap_err(), FsError::Stale);
    assert!(ns.readdir(dir.ino, None, 100).await.unwrap().is_empty());
    ns.rmdir(ns.root(), "dir").await.unwrap();
    assert_eq!(ns.lookup(ns.root(), "dir").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.getattr(dir.ino).await.unwrap_err(), FsError::Stale);
    let local = ns.mkdir(ns.root(), "local", 0o755).await.unwrap();
    let nested = ns.mkdir(local.ino, "nested", 0o755).await.unwrap();
    ns.create(nested.ino, "leaf", 0o644).await.unwrap();
    assert_eq!(ns.rmdir(local.ino, "nested").await.unwrap_err(), FsError::NotEmpty);
    ns.unlink(nested.ino, "leaf").await.unwrap();
    ns.rmdir(local.ino, "nested").await.unwrap();
    ns.rmdir(ns.root(), "local").await.unwrap();
    ns.unlink(ns.root(), "switch-file").await.unwrap();
    ns.mkdir(ns.root(), "switch-file", 0o700).await.unwrap();
    ns.rmdir(ns.root(), "switch-dir").await.unwrap();
    ns.create(ns.root(), "switch-dir", 0o600).await.unwrap();
    f.publish().await;
    for key in ["dir/", "dir/child", "local/", "local/nested/", "local/nested/leaf"] {
        assert!(remote_body(&f.remote, key).await.is_none(), "{key} remained remote");
    }
    assert_eq!(f.remote.head_object("drv", "switch-file/", Default::default()).await.unwrap().kind, Kind::Folder);
    assert_eq!(f.remote.head_object("drv", "switch-dir", Default::default()).await.unwrap().kind, Kind::File);
    assert!(f.queue.status().await.unwrap().items.iter().all(|item| item.state == State::Done));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_keeps_local_identity_rebases_subtrees_and_publishes_following_changes() {
    let f = Fixture::new().await;
    f.put("before/sub/file", "bytes").await;
    f.put("right/", "").await;
    let ns = f.session().await;
    let dir = ns.lookup(ns.root(), "before").await.unwrap();
    let sub = ns.lookup(dir.ino, "sub").await.unwrap();
    let file = ns.lookup(sub.ino, "file").await.unwrap();
    let right = ns.lookup(ns.root(), "right").await.unwrap();
    ns.readdir(right.ino, None, 100).await.unwrap();
    let source_generation = f.directory_generation(ns.root());
    let destination_generation = f.directory_generation(right.ino);
    ns.rename(ns.root(), "before", right.ino, "after", RenameMode::Exclusive).await.unwrap();
    let moved = ns.lookup(right.ino, "after").await.unwrap();
    assert_eq!((moved.ino, &moved.object_id, moved.sync), (dir.ino, &dir.object_id, Sync::Pending));
    assert!(moved.generation > dir.generation);
    assert_eq!(ns.lookup(moved.ino, "sub").await.unwrap().ino, sub.ino);
    assert_eq!(ns.lookup(sub.ino, "file").await.unwrap().ino, file.ino);
    assert_eq!(ns.lookup(ns.root(), "before").await.unwrap_err(), FsError::NotFound);
    assert!(f.directory_generation(ns.root()) > source_generation);
    assert!(f.directory_generation(right.ino) > destination_generation);
    let created = ns.create(sub.ino, "new", 0o644).await.unwrap();
    ns.rename(sub.ino, "file", sub.ino, "renamed", RenameMode::Exclusive).await.unwrap();
    assert_eq!(ns.lookup(sub.ino, "new").await.unwrap().ino, created.ino);
    let items = f.queue.status().await.unwrap().items;
    assert_eq!(items[0].key, "before/");
    assert_eq!(items[0].to_key.as_deref(), Some("right/after/"));
    assert_eq!(items[1].key, "right/after/sub/new");
    assert_eq!(items[2].key, "right/after/sub/file");
    f.publish().await;
    assert_eq!(remote_body(&f.remote, "right/after/sub/renamed").await, Some(Bytes::from_static(b"bytes")));
    assert_eq!(remote_body(&f.remote, "right/after/sub/new").await, Some(Bytes::new()));
    assert!(remote_body(&f.remote, "before/sub/file").await.is_none());
    assert!(f.queue.status().await.unwrap().items.iter().all(|item| item.state == State::Done));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_exclusive_swap_cycles_and_same_names_leave_the_queue_unchanged() {
    let f = Fixture::new().await;
    f.put("a/sub/", "").await;
    f.put("file", "one").await;
    f.put("other", "two").await;
    let ns = f.session().await;
    let a = ns.lookup(ns.root(), "a").await.unwrap();
    let sub = ns.lookup(a.ino, "sub").await.unwrap();
    let a = ns.getattr(a.ino).await.unwrap();
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    let other = ns.lookup(ns.root(), "other").await.unwrap();
    let generation = f.directory_generation(ns.root());
    assert_eq!(ns.rename(ns.root(), "file", ns.root(), "other", RenameMode::Exclusive).await.unwrap_err(), FsError::Exists);
    assert_eq!(ns.rename(ns.root(), "file", ns.root(), "other", RenameMode::Swap).await.unwrap_err(), FsError::Unsupported);
    assert_eq!(ns.rename(ns.root(), "a", sub.ino, "loop", RenameMode::Exclusive).await.unwrap_err(), FsError::InvalidArgument);
    ns.rename(ns.root(), "file", ns.root(), "file", RenameMode::Exclusive).await.unwrap();
    ns.rename(ns.root(), "file", ns.root(), "file", RenameMode::Replace).await.unwrap();
    assert!(f.queue.status().await.unwrap().items.is_empty());
    assert_eq!(f.directory_generation(ns.root()), generation);
    assert_eq!(ns.lookup(ns.root(), "file").await.unwrap(), file);
    assert_eq!(ns.lookup(ns.root(), "other").await.unwrap(), other);
    assert_eq!(ns.lookup(ns.root(), "a").await.unwrap(), a);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn equivalent_unicode_rename_to_the_same_inode_is_a_noop() {
    let f = Fixture::new().await;
    let ns = f.session().await;
    let file = ns.create(ns.root(), "cafe\u{301}", 0o644).await.unwrap();
    let queued = f.queue.status().await.unwrap();
    let generation = f.directory_generation(ns.root());
    ns.rename(ns.root(), "cafe\u{301}", ns.root(), "caf\u{e9}", RenameMode::Exclusive).await.unwrap();
    assert_eq!(f.queue.status().await.unwrap(), queued);
    assert_eq!(f.directory_generation(ns.root()), generation);
    assert_eq!(ns.lookup(ns.root(), "caf\u{e9}").await.unwrap(), file);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_decomposed_names_keep_their_exact_source_key_when_edited() {
    let f = Fixture::new().await;
    f.put("cafe\u{301}", "moved").await;
    f.put("delete\u{301}", "deleted").await;
    let ns = f.session().await;
    let source = ns.lookup(ns.root(), "caf\u{e9}").await.unwrap();
    ns.rename(ns.root(), "caf\u{e9}", ns.root(), "re\u{301}sume\u{301}", RenameMode::Exclusive).await.unwrap();
    ns.unlink(ns.root(), "delet\u{e9}").await.unwrap();
    let moved = ns.lookup(ns.root(), "r\u{e9}sum\u{e9}").await.unwrap();
    assert_eq!(moved.ino, source.ino);
    let items = f.queue.status().await.unwrap().items;
    assert_eq!((items[0].key.as_str(), items[0].to_key.as_deref()), ("cafe\u{301}", Some("r\u{e9}sum\u{e9}")));
    assert_eq!(items[1].key, "delete\u{301}");
    f.publish().await;
    assert_eq!(remote_body(&f.remote, "r\u{e9}sum\u{e9}").await, Some(Bytes::from_static(b"moved")));
    assert!(remote_body(&f.remote, "cafe\u{301}").await.is_none());
    assert!(remote_body(&f.remote, "delete\u{301}").await.is_none());
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ambiguous_remote_unicode_names_cannot_be_mutated_arbitrarily() {
    let f = Fixture::new().await;
    f.put("caf\u{e9}", "one").await;
    f.put("cafe\u{301}", "two").await;
    f.put("source", "kept").await;
    let ns = f.session().await;
    for name in ["caf\u{e9}", "cafe\u{301}"] {
        assert_eq!(ns.unlink(ns.root(), name).await.unwrap_err(), FsError::Ambiguous);
        assert_eq!(ns.rename(ns.root(), name, ns.root(), "moved", RenameMode::Exclusive).await.unwrap_err(), FsError::Ambiguous);
        assert_eq!(ns.rename(ns.root(), "source", ns.root(), name, RenameMode::Replace).await.unwrap_err(), FsError::Ambiguous);
    }
    assert!(f.queue.status().await.unwrap().items.is_empty());
    assert_eq!(ns.readdir(ns.root(), None, 100).await.unwrap().len(), 3);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacing_a_file_retains_the_original_open_snapshot_for_both_fetchers() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::with_backend(backend).await;
        f.put("target", "old untouched bytes").await;
        f.put("temporary", "replacement bytes").await;
        let ns = f.session().await;
        let original = ns.lookup(ns.root(), "target").await.unwrap();
        let replacement = ns.lookup(ns.root(), "temporary").await.unwrap();
        let original_fh = ns.open(original.ino, false).await.unwrap();
        ns.rename(ns.root(), "temporary", ns.root(), "target", RenameMode::Replace).await.unwrap();
        assert_eq!(ns.lookup(ns.root(), "target").await.unwrap().ino, replacement.ino);
        assert_eq!(ns.lookup(ns.root(), "temporary").await.unwrap_err(), FsError::NotFound);
        assert_eq!(ns.getattr(original.ino).await.unwrap_err(), FsError::Stale);
        assert_eq!(ns.handle_attr(original_fh).unwrap(), original);
        assert_eq!(ns.read(original_fh, 0, 3).await.unwrap(), Bytes::from_static(b"old"));
        let replacement_fh = ns.open(replacement.ino, false).await.unwrap();
        assert_eq!(ns.read(replacement_fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"replacement bytes"));
        f.publish().await;
        assert_eq!(remote_body(&f.remote, "target").await, Some(Bytes::from_static(b"replacement bytes")));
        assert_eq!(ns.handle_attr(original_fh).unwrap(), original);
        // This part of the original version was never fetched before remote replacement.
        assert_eq!(ns.read(original_fh, 4, 9).await.unwrap(), Bytes::from_static(b"untouched"));
        ns.close(original_fh).await.unwrap();
        ns.close(replacement_fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_replace_enforces_folder_kinds_and_empty_destination() {
    let f = Fixture::new().await;
    f.put("source/", "").await;
    f.put("empty/", "").await;
    f.put("full/child", "bytes").await;
    f.put("file", "bytes").await;
    let ns = f.session().await;
    let source = ns.lookup(ns.root(), "source").await.unwrap();
    let empty = ns.lookup(ns.root(), "empty").await.unwrap();
    assert_eq!(ns.rename(ns.root(), "source", ns.root(), "full", RenameMode::Replace).await.unwrap_err(), FsError::NotEmpty);
    assert_eq!(ns.rename(ns.root(), "source", ns.root(), "file", RenameMode::Replace).await.unwrap_err(), FsError::NotDir);
    assert_eq!(ns.rename(ns.root(), "file", ns.root(), "source", RenameMode::Replace).await.unwrap_err(), FsError::IsDir);
    assert!(f.queue.status().await.unwrap().items.is_empty());
    ns.rename(ns.root(), "source", ns.root(), "empty", RenameMode::Replace).await.unwrap();
    assert_eq!(ns.lookup(ns.root(), "empty").await.unwrap().ino, source.ino);
    assert_eq!(ns.getattr(empty.ino).await.unwrap_err(), FsError::Stale);
    f.publish().await;
    assert_eq!(f.remote.head_object("drv", "empty/", Default::default()).await.unwrap().object_id,
        source.object_id);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutations_validate_names_modes_parent_types_and_foreign_inodes() {
    let f = Fixture::new().await;
    f.put("file", "bytes").await;
    f.remote.create_drive("other", Default::default()).await.unwrap();
    let ns = f.session().await;
    let other = f.session_for("other").await;
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    for name in ["", ".", "..", "two/names", "nul\0name", &"x".repeat(256)] {
        assert_eq!(ns.create(ns.root(), name, 0o644).await.unwrap_err(), FsError::InvalidName, "{name:?}");
        assert_eq!(ns.mkdir(ns.root(), name, 0o755).await.unwrap_err(), FsError::InvalidName, "{name:?}");
        assert_eq!(ns.unlink(ns.root(), name).await.unwrap_err(), FsError::InvalidName, "{name:?}");
        assert_eq!(ns.rmdir(ns.root(), name).await.unwrap_err(), FsError::InvalidName, "{name:?}");
        assert_eq!(ns.rename(ns.root(), "file", ns.root(), name, RenameMode::Exclusive).await.unwrap_err(), FsError::InvalidName);
        assert_eq!(ns.rename(ns.root(), name, ns.root(), "new", RenameMode::Exclusive).await.unwrap_err(), FsError::InvalidName);
    }
    for mode in [0o10000, u32::MAX] {
        assert_eq!(ns.create(ns.root(), "new", mode).await.unwrap_err(), FsError::InvalidArgument);
        assert_eq!(ns.mkdir(ns.root(), "new", mode).await.unwrap_err(), FsError::InvalidArgument);
    }
    assert_eq!(ns.create(file.ino, "child", 0o644).await.unwrap_err(), FsError::NotDir);
    assert_eq!(ns.mkdir(file.ino, "child", 0o755).await.unwrap_err(), FsError::NotDir);
    for ino in [0, 999_999, u64::MAX, other.root()] {
        assert_eq!(ns.create(ino, "child", 0o644).await.unwrap_err(), FsError::Stale);
        assert_eq!(ns.rename(ns.root(), "file", ino, "moved", RenameMode::Exclusive).await.unwrap_err(), FsError::Stale);
    }
    assert!(f.queue.status().await.unwrap().items.is_empty());
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_keys_enforce_the_protocol_byte_limit_before_journaling() {
    let f = Fixture::new().await;
    let ns = f.session().await;
    let component = "x".repeat(255);
    let mut parent = ns.root();
    for _ in 0..4 { parent = ns.mkdir(parent, &component, 0o755).await.unwrap().ino; }
    // Four directories include 1,024 bytes including their final slash.
    let before = f.queue.status().await.unwrap();
    assert_eq!(ns.create(parent, "x", 0o644).await.unwrap_err(), FsError::InvalidName);
    assert_eq!(ns.mkdir(parent, "x", 0o755).await.unwrap_err(), FsError::InvalidName);
    let source = ns.create(ns.root(), "short", 0o644).await.unwrap();
    let created = f.queue.status().await.unwrap();
    assert_eq!(ns.rename(ns.root(), "short", parent, "x", RenameMode::Exclusive).await.unwrap_err(), FsError::InvalidName);
    assert_eq!(f.queue.status().await.unwrap(), created);
    assert_eq!(created.items.len(), before.items.len() + 1);
    assert_eq!(ns.lookup(ns.root(), "short").await.unwrap(), source);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_overlay_and_queue_survive_a_restart_and_remote_refresh() {
    let f = Fixture::new().await;
    f.put("old", "bytes").await;
    f.put("gone", "bytes").await;
    let ns = f.session().await;
    let original = ns.lookup(ns.root(), "old").await.unwrap();
    let local = ns.mkdir(ns.root(), "local", 0o750).await.unwrap();
    let leaf = ns.create(local.ino, "leaf", 0o640).await.unwrap();
    ns.rename(ns.root(), "old", local.ino, "moved", RenameMode::Exclusive).await.unwrap();
    ns.unlink(ns.root(), "gone").await.unwrap();
    let root = ns.root();
    let local = ns.getattr(local.ino).await.unwrap();
    let queued = f.queue.status().await.unwrap();
    f.queue.close().await;
    f.cache.settle().await;
    let Fixture { _server, proxy, remote, client, state, store, cache, queue, connectivity } = f;
    drop(ns);
    drop(cache);
    drop(queue);
    drop(store);
    let reopened = Arc::new(Store::open(state.path()).unwrap());
    let queue = Queue::open(reopened.clone(), client.clone(), queue_config()).await.unwrap();
    assert_eq!(queue.status().await.unwrap(), queued);
    let cache = Cache::open(reopened.clone(), Arc::new(ApiFetcher::new(client.clone()).with_connectivity(connectivity.clone())),
        CacheConfig { min_free_bytes: 0, ..Default::default() }).await.unwrap();
    let ns = Session::new_writable(reopened, client, cache, queue.clone(), "drv", connectivity).await.unwrap();
    assert_eq!(ns.root(), root);
    assert_eq!(ns.lookup(root, "local").await.unwrap(), local);
    assert_eq!(ns.lookup(local.ino, "leaf").await.unwrap(), leaf);
    assert_eq!(ns.lookup(local.ino, "moved").await.unwrap().ino, original.ino);
    assert_eq!(ns.lookup(root, "old").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.lookup(root, "gone").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.readdir(root, None, 100).await.unwrap().iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["local"]);
    queue.resume(Scope::All).await.unwrap();
    queue.settle().await;
    assert_eq!(remote_body(&remote, "local/leaf").await, Some(Bytes::new()));
    assert_eq!(remote_body(&remote, "local/moved").await, Some(Bytes::from_static(b"bytes")));
    assert!(remote_body(&remote, "gone").await.is_none());
    assert!(queue.status().await.unwrap().items.iter().all(|item| item.state == State::Done));
    queue.close().await;
    drop(proxy);
    drop(_server);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_folder_renames_read_cold_descendants_at_the_remote_path_after_feed_and_restart() {
    let f = Fixture::new().await;
    f.put("before/first/leaf", "first untouched").await;
    f.put("before/second/leaf", "second untouched").await;
    let ns = f.session().await;
    let before = ns.lookup(ns.root(), "before").await.unwrap();
    ns.rename(ns.root(), "before", ns.root(), "after", RenameMode::Exclusive).await.unwrap();
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    f.proxy.clear();
    let after = ns.lookup(ns.root(), "after").await.unwrap();
    assert_eq!(after.ino, before.ino);
    let first = ns.lookup(after.ino, "first").await.unwrap();
    let leaf = ns.lookup(first.ino, "leaf").await.unwrap();
    let fh = ns.open(leaf.ino, false).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"first untouched"));
    ns.close(fh).await.unwrap();
    f.put("before/first/leaf", "first updated remotely").await;
    let affected = ns.invalidate(&[Invalidation::Object("before/first/leaf".into())]).await.unwrap();
    for ino in [after.ino, first.ino, leaf.ino] {
        assert!(affected.contains(&ino), "old remote-path Object invalidation missed inode {ino}: {affected:?}");
    }
    let refreshed = ns.getattr(leaf.ino).await.unwrap();
    assert_eq!(refreshed.ino, leaf.ino);
    assert_ne!(refreshed.version_id, leaf.version_id);
    let fh = ns.open(refreshed.ino, false).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"first updated remotely"));
    ns.close(fh).await.unwrap();
    assert_eq!(ns.lookup(first.ino, "added").await.unwrap_err(), FsError::NotFound);
    f.put("before/first/added", "new remote child").await;
    let affected = ns.invalidate(&[Invalidation::Subtree("before/first/".into())]).await.unwrap();
    for ino in [after.ino, first.ino, leaf.ino] {
        assert!(affected.contains(&ino), "old remote-path Subtree invalidation missed inode {ino}: {affected:?}");
    }
    assert_eq!(ns.lookup(first.ino, "added").await.unwrap().size, 16);
    assert!(f.proxy.seen().iter().any(|request| request.contains("prefix=before%2Ffirst%2F")), "{:?}", f.proxy.seen());
    f.queue.close().await;
    f.cache.settle().await;
    let Fixture { _server, proxy, remote, client, state, store, cache, queue, connectivity } = f;
    drop(ns);
    drop(cache);
    drop(queue);
    drop(store);
    let reopened = Arc::new(Store::open(state.path()).unwrap());
    let queue = Queue::open(reopened.clone(), client.clone(), queue_config()).await.unwrap();
    let cache = Cache::open(reopened.clone(), Arc::new(ApiFetcher::new(client.clone()).with_connectivity(connectivity.clone())),
        CacheConfig { min_free_bytes: 0, read_ahead_bytes: 0, ..Default::default() }).await.unwrap();
    let ns = Session::new_writable(reopened, client, cache, queue.clone(), "drv", connectivity).await.unwrap();
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    proxy.clear();
    let after = ns.lookup(ns.root(), "after").await.unwrap();
    let second = ns.lookup(after.ino, "second").await.unwrap();
    let cold_leaf = ns.lookup(second.ino, "leaf").await.unwrap();
    let fh = ns.open(cold_leaf.ino, false).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"second untouched"));
    ns.close(fh).await.unwrap();
    assert!(proxy.seen().iter().any(|request| request.contains("prefix=before%2Fsecond%2F")), "{:?}", proxy.seen());
    assert!(remote_body(&remote, "before/second/leaf").await.is_some(), "the rename is still queued");
    queue.resume(Scope::All).await.unwrap();
    queue.settle().await;
    assert_eq!(remote_body(&remote, "after/second/leaf").await, Some(Bytes::from_static(b"second untouched")));
    assert!(queue.status().await.unwrap().items.iter().all(|item| item.state == State::Done));
    queue.close().await;
    drop(proxy);
    drop(_server);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_have_one_winner_and_one_durable_entry() {
    let f = Fixture::new().await;
    let ns = Arc::new(f.session().await);
    ns.readdir(ns.root(), None, 100).await.unwrap();
    let mut tasks = Vec::new();
    let barrier = Arc::new(tokio::sync::Barrier::new(16));
    for _ in 0..16 {
        let ns = ns.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            ns.create(ns.root(), "raced", 0o644).await
        }));
    }
    let mut winners = Vec::new();
    for task in tasks {
        match task.await.unwrap() {
            Ok(attr) => winners.push(attr),
            Err(error) => assert_eq!(error, FsError::Exists),
        }
    }
    assert_eq!(winners.len(), 1);
    assert_eq!(ns.lookup(ns.root(), "raced").await.unwrap(), winners[0]);
    assert_eq!(f.queue.status().await.unwrap().items.len(), 1);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_database_failure_rolls_back_both_the_namespace_and_journal() {
    let f = Fixture::new().await;
    let ns = f.session().await;
    ns.readdir(ns.root(), None, 100).await.unwrap();
    let before = f.directory_generation(ns.root());
    let db = f.db();
    db.execute_batch("CREATE TRIGGER reject_mount_overlay BEFORE INSERT ON mount_overlay BEGIN SELECT RAISE(ABORT, 'injected overlay failure'); END;").unwrap();
    assert!(matches!(ns.create(ns.root(), "rollback", 0o644).await, Err(FsError::Io(_))));
    assert!(f.queue.status().await.unwrap().items.is_empty());
    assert_eq!(ns.lookup(ns.root(), "rollback").await.unwrap_err(), FsError::NotFound);
    assert_eq!(f.directory_generation(ns.root()), before);
    assert_eq!(db.query_row("SELECT count(*) FROM mount_inodes WHERE drive='drv'", [], |r| r.get::<_, u64>(0)).unwrap(), 1);
    db.execute_batch("DROP TRIGGER reject_mount_overlay;").unwrap();
    ns.create(ns.root(), "rollback", 0o644).await.unwrap();
    assert_eq!(f.queue.status().await.unwrap().items.len(), 1);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_sessions_reject_each_mutation_and_write_open() {
    let f = Fixture::new().await;
    f.put("file", "bytes").await;
    let ns = Session::new(f.store.clone(), f.client.clone(), f.cache.clone(), "drv", f.connectivity.clone()).await.unwrap();
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    assert_eq!(ns.create(ns.root(), "new", 0o644).await.unwrap_err(), FsError::ReadOnly);
    assert_eq!(ns.mkdir(ns.root(), "new", 0o755).await.unwrap_err(), FsError::ReadOnly);
    assert_eq!(ns.unlink(ns.root(), "file").await.unwrap_err(), FsError::ReadOnly);
    assert_eq!(ns.rmdir(ns.root(), "new").await.unwrap_err(), FsError::ReadOnly);
    assert_eq!(ns.rename(ns.root(), "file", ns.root(), "new", RenameMode::Replace).await.unwrap_err(), FsError::ReadOnly);
    assert_eq!(ns.open(file.ino, true).await.unwrap_err(), FsError::ReadOnly);
    assert!(f.queue.status().await.unwrap().items.is_empty());
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writable_session_requires_the_queue_to_share_its_transaction_store() {
    let f = Fixture::new().await;
    let other_dir = tempfile::tempdir().unwrap();
    let other = Arc::new(Store::open(other_dir.path()).unwrap());
    let queue = Queue::open(other, f.client.clone(), queue_config()).await.unwrap();
    queue.pause(Scope::All).await.unwrap();
    let result = Session::new_writable(f.store.clone(), f.client.clone(), f.cache.clone(), queue.clone(),
        "drv", f.connectivity.clone()).await;
    assert!(matches!(result, Err(FsError::InvalidArgument)));
    assert!(f.queue.status().await.unwrap().items.is_empty());
    assert!(queue.status().await.unwrap().items.is_empty());
    queue.close().await;
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mount_creation_and_folder_races_preserve_competing_remote_objects() {
    for folder in [false, true] {
        let f = Fixture::new().await;
        let ns = f.session().await;
        let key = if folder { "raced/" } else { "raced" };
        if folder { ns.mkdir(ns.root(), "raced", 0o700).await.unwrap(); }
        else { ns.create(ns.root(), "raced", 0o600).await.unwrap(); }
        let competitor = f.remote.put_object("drv", key, if folder { "" } else { "competitor" }, Default::default()).await.unwrap();
        f.publish().await;
        let status = f.queue.status().await.unwrap();
        assert_eq!(status.items[0].state, State::Failed, "a mount edit must preserve the competing object: {status:?}");
        assert_eq!(f.remote.head_object("drv", key, Default::default()).await.unwrap().version_id, competitor.version_id);
        if !folder { assert_eq!(remote_body(&f.remote, key).await, Some(Bytes::from_static(b"competitor"))); }
        assert_eq!(f.remote.list_versions("drv", key, false).await.unwrap().len(), 1);
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mount_delete_and_rename_races_preserve_new_remote_versions() {
    for rename in [false, true] {
        let f = Fixture::new().await;
        f.put("source", "original").await;
        let ns = f.session().await;
        ns.lookup(ns.root(), "source").await.unwrap();
        if rename { ns.rename(ns.root(), "source", ns.root(), "target", RenameMode::Exclusive).await.unwrap(); }
        else { ns.unlink(ns.root(), "source").await.unwrap(); }
        let updated = f.remote.put_object("drv", "source", "remote update", Default::default()).await.unwrap();
        f.publish().await;
        assert_eq!(f.queue.status().await.unwrap().items[0].state, State::Failed);
        assert_eq!(f.remote.head_object("drv", "source", Default::default()).await.unwrap().version_id, updated.version_id);
        assert_eq!(remote_body(&f.remote, "source").await, Some(Bytes::from_static(b"remote update")));
        assert!(remote_body(&f.remote, "target").await.is_none());
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exclusive_rename_does_not_replace_a_destination_created_after_the_local_check() {
    let f = Fixture::new().await;
    f.put("source", "original").await;
    let ns = f.session().await;
    ns.rename(ns.root(), "source", ns.root(), "target", RenameMode::Exclusive).await.unwrap();
    f.put("target", "competing destination").await;
    f.publish().await;
    assert_eq!(f.queue.status().await.unwrap().items[0].state, State::Failed);
    assert_eq!(remote_body(&f.remote, "source").await, Some(Bytes::from_static(b"original")));
    assert_eq!(remote_body(&f.remote, "target").await, Some(Bytes::from_static(b"competing destination")));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_keeps_a_newer_remote_destination_and_blocks_the_source_move() {
    for folder in [false, true] {
        for (remote_name, local_name) in [("target", "target"), ("cafe\u{301}", "caf\u{e9}")] {
            let f = Fixture::new().await;
            let source = if folder { "source/" } else { "source" };
            let target = format!("{remote_name}{}", if folder { "/" } else { "" });
            f.put(source, if folder { "" } else { "original source" }).await;
            f.put(&target, if folder { "" } else { "old destination" }).await;
            let ns = f.session().await;
            ns.rename(ns.root(), "source", ns.root(), local_name, RenameMode::Replace).await.unwrap();
            let newer = f.remote.put_object("drv", &target, if folder { "" } else { "newer destination" },
                voidfs_sdk::PutOptions { mode: Some(0o700), ..Default::default() }).await.unwrap();
            f.publish().await;
            let status = f.queue.status().await.unwrap();
            assert_eq!(status.items.iter().map(|item| item.state).collect::<Vec<_>>(), [State::Failed, State::Queued],
                "the failed deletion blocks its dependent move: {status:?}");
            assert_eq!(f.remote.head_object("drv", &target, Default::default()).await.unwrap().version_id, newer.version_id);
            if !folder {
                assert_eq!(remote_body(&f.remote, &target).await, Some(Bytes::from_static(b"newer destination")));
                assert_eq!(remote_body(&f.remote, source).await, Some(Bytes::from_static(b"original source")));
            } else {
                assert_eq!(f.remote.head_object("drv", source, Default::default()).await.unwrap().kind, Kind::Folder);
            }
            f.queue.close().await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn successful_namespace_mutations_do_not_lose_overlay_names_after_invalidation() {
    let f = Fixture::new().await;
    f.put("old", "original").await;
    let ns = f.session().await;
    let old = ns.lookup(ns.root(), "old").await.unwrap();
    let file = ns.create(ns.root(), "new", 0o644).await.unwrap();
    ns.rename(ns.root(), "old", ns.root(), "moved", RenameMode::Exclusive).await.unwrap();
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    let refreshed = ns.lookup(ns.root(), "new").await.unwrap();
    assert_eq!((refreshed.ino, refreshed.mode, refreshed.size, refreshed.sync), (file.ino, file.mode, file.size, Sync::Pending));
    assert!(refreshed.generation > file.generation);
    assert_eq!(ns.lookup(ns.root(), "moved").await.unwrap().ino, old.ino);
    assert_eq!(ns.lookup(ns.root(), "old").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.readdir(ns.root(), None, 100).await.unwrap().iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["moved", "new"]);
    // Linked local inodes retain durable queue lineage, and the old name stays tombstoned.
    let db = f.db();
    assert_eq!(db.query_row("SELECT count(*) FROM mount_overlay o JOIN mount_inodes i ON o.ino=i.ino WHERE o.parent=?1 AND i.entry_id IS NOT NULL",
        params![ns.root()], |r| r.get::<_, u64>(0)).unwrap(), 2);
    assert_eq!(db.query_row("SELECT count(*) FROM mount_overlay WHERE parent=?1 AND name='old' AND ino IS NULL",
        params![ns.root()], |r| r.get::<_, u64>(0)).unwrap(), 1);
    f.queue.close().await;
}
