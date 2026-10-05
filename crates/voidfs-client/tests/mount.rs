// SPDX-License-Identifier: Apache-2.0
//! The persistent mount namespace, against an in-process server and request-counting proxy.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use common::{Proxy, client_for, server};
use voidfs_client::mount::{FsError, Session, Sync};
use voidfs_client::{Connectivity, Invalidation, Link, Store};
use voidfs_sdk::{AttributesUpdate, Client, Config, Kind, PutOptions, RenameOptions};
use voidfs_server::test_server::TestServer;

struct Fixture {
    _server: TestServer,
    proxy: Proxy,
    remote: Client,
    client: Client,
    state: tempfile::TempDir,
    store: Arc<Store>,
    connectivity: Connectivity,
}

impl Fixture {
    async fn new() -> Self {
        let server = server().await;
        let proxy = Proxy::start(&server.endpoint).await;
        let remote = client_for(&server.endpoint, Config::default());
        let client = client_for(&proxy.endpoint, Config::default());
        remote.create_drive("drv", Default::default()).await.unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(state.path()).unwrap());
        Self { _server: server, proxy, remote, client, state, store, connectivity: Connectivity::default() }
    }

    async fn session(&self, drive: &str) -> Session {
        Session::new(self.store.clone(), self.client.clone(), drive, self.connectivity.clone()).await.unwrap()
    }

    async fn put(&self, key: &str, body: &'static str) {
        self.remote.put_object("drv", key, body, Default::default()).await.unwrap();
    }
}

fn objects(keys: &[&str]) -> Vec<Invalidation> {
    keys.iter().map(|key| Invalidation::Object((*key).into())).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_listing_supplies_sorted_names_and_all_child_attributes_without_heads() {
    let f = Fixture::new().await;
    f.put("zed", "last").await;
    f.put("folder/", "").await;
    f.remote.put_object("drv", "alpha", "first", PutOptions {
        mtime: Some("2026-01-02T03:04:05.000006Z".into()), mode: Some(0o640), ..Default::default()
    }).await.unwrap();
    let version = f.remote.set_attributes("drv", "alpha", AttributesUpdate {
        set_xattrs: BTreeMap::from([("user.tag".into(), "red".into())]), ..Default::default()
    }, Default::default()).await.unwrap();
    let ns = f.session("drv").await;
    let root = ns.root();
    let root_attr = ns.getattr(root).await.unwrap();
    assert_eq!((root_attr.kind, root_attr.mode, root_attr.object_id), (Kind::Folder, 0o755, None));

    let first = ns.readdir(root, None, 2).await.unwrap();
    assert_eq!(first.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["alpha", "folder"]);
    assert_eq!(first[1].1.kind, Kind::Folder);
    let alpha = ns.lookup(root, "alpha").await.unwrap();
    assert_eq!(alpha, first[0].1);
    assert_eq!((alpha.kind, alpha.size, alpha.mode, alpha.sync), (Kind::File, 5, 0o640, Sync::Saved));
    assert_eq!(alpha.mtime, SystemTime::from(chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05.000006Z").unwrap()));
    assert_eq!(alpha.version_id.as_deref(), Some(version.version_id.as_str()));
    assert!(alpha.object_id.is_some() && alpha.etag.is_some() && alpha.has_xattrs);
    assert_eq!(ns.getattr(alpha.ino).await.unwrap(), alpha);
    let remaining = ns.readdir(root, Some("folder"), 10).await.unwrap();
    assert_eq!(remaining.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["zed"]);
    assert!(ns.readdir(root, None, 0).await.unwrap().is_empty());
    let requests = f.proxy.seen();
    assert_eq!(requests.len(), 1, "all metadata came from one listing: {requests:?}");
    assert!(requests[0].starts_with("GET /drv?") && requests[0].contains("x-voidfs-list"), "{requests:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overwrite_refreshes_attributes_without_changing_the_inode() {
    let f = Fixture::new().await;
    f.put("file", "old").await;
    let ns = f.session("drv").await;
    let before = ns.lookup(ns.root(), "file").await.unwrap();
    f.put("file", "new and longer").await;
    let affected = ns.invalidate(&objects(&["file"])).await.unwrap();
    assert!(affected.contains(&ns.root()) && affected.contains(&before.ino));
    let after = ns.getattr(before.ino).await.unwrap();
    assert_eq!((after.ino, &after.object_id), (before.ino, &before.object_id));
    assert_eq!(after.size, 14);
    assert_ne!(after.version_id, before.version_id);
    assert_ne!(after.etag, before.etag);
    assert!(after.generation > before.generation);
    assert_eq!(ns.lookup(ns.root(), "file").await.unwrap(), after);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renames_within_and_across_directories_keep_the_object_inode() {
    let f = Fixture::new().await;
    f.put("left/file", "bytes").await;
    f.put("right/", "").await;
    let ns = f.session("drv").await;
    let left = ns.lookup(ns.root(), "left").await.unwrap().ino;
    let right = ns.lookup(ns.root(), "right").await.unwrap().ino;
    let file = ns.lookup(left, "file").await.unwrap();
    assert!(ns.readdir(right, None, 10).await.unwrap().is_empty());

    f.remote.rename("drv", "left/file", "left/renamed", Default::default()).await.unwrap();
    ns.invalidate(&objects(&["left/file", "left/renamed"])).await.unwrap();
    let renamed = ns.lookup(left, "renamed").await.unwrap();
    assert_eq!((renamed.ino, &renamed.object_id), (file.ino, &file.object_id));
    assert_eq!(ns.lookup(left, "file").await.unwrap_err(), FsError::NotFound);

    f.remote.rename("drv", "left/renamed", "right/moved", Default::default()).await.unwrap();
    ns.invalidate(&objects(&["left/renamed", "right/moved"])).await.unwrap();
    let moved = ns.lookup(right, "moved").await.unwrap();
    assert_eq!((moved.ino, &moved.object_id), (file.ino, &file.object_id));
    assert_eq!(ns.getattr(file.ino).await.unwrap(), moved);
    assert_eq!(ns.lookup(left, "renamed").await.unwrap_err(), FsError::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folder_rename_rebases_cached_descendants_without_changing_their_inodes() {
    let f = Fixture::new().await;
    f.put("before/sub/file", "bytes").await;
    let ns = f.session("drv").await;
    let before = ns.lookup(ns.root(), "before").await.unwrap();
    let sub = ns.lookup(before.ino, "sub").await.unwrap();
    let file = ns.lookup(sub.ino, "file").await.unwrap();
    f.remote.rename("drv", "before/", "after/", Default::default()).await.unwrap();
    let mut changes = objects(&["before/", "after/"]);
    changes.extend([Invalidation::Subtree("before/".into()), Invalidation::Subtree("after/".into())]);
    let affected = ns.invalidate(&changes).await.unwrap();
    assert!(affected.contains(&sub.ino) && affected.contains(&file.ino));

    let after = ns.lookup(ns.root(), "after").await.unwrap();
    assert_eq!((after.ino, after.object_id), (before.ino, before.object_id));
    assert_eq!(ns.lookup(after.ino, "sub").await.unwrap().ino, sub.ino);
    assert_eq!(ns.lookup(sub.ino, "file").await.unwrap().ino, file.ino);
    assert_eq!(ns.getattr(file.ino).await.unwrap().object_id, file.object_id);
    assert_eq!(ns.lookup(ns.root(), "before").await.unwrap_err(), FsError::NotFound);
    assert!(f.proxy.seen().iter().any(|r| r.contains("prefix=after%2Fsub%2F")), "descendants use their new path: {:?}", f.proxy.seen());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_makes_an_inode_stale_and_restore_revives_the_same_identity() {
    let f = Fixture::new().await;
    let version = f.remote.put_object("drv", "file", "bytes", Default::default()).await.unwrap();
    let ns = f.session("drv").await;
    let before = ns.lookup(ns.root(), "file").await.unwrap();
    f.remote.delete_object("drv", "file", Default::default()).await.unwrap();
    ns.invalidate(&objects(&["file"])).await.unwrap();
    assert_eq!(ns.lookup(ns.root(), "file").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.getattr(before.ino).await.unwrap_err(), FsError::Stale);

    f.remote.restore_version("drv", "file", &version.version_id, Default::default()).await.unwrap();
    ns.invalidate(&objects(&["file"])).await.unwrap();
    let restored = ns.lookup(ns.root(), "file").await.unwrap();
    assert_eq!((restored.ino, restored.object_id, restored.size), (before.ino, before.object_id, 5));
    assert_ne!(restored.version_id, before.version_id);
    assert_eq!(ns.getattr(before.ino).await.unwrap().sync, Sync::Saved);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacing_a_name_uses_the_replacement_objects_inode() {
    let f = Fixture::new().await;
    f.put("target", "old").await;
    f.put("temporary", "replacement").await;
    let ns = f.session("drv").await;
    let old = ns.lookup(ns.root(), "target").await.unwrap();
    let temporary = ns.lookup(ns.root(), "temporary").await.unwrap();
    f.remote.rename("drv", "temporary", "target", RenameOptions { replace: true, ..Default::default() }).await.unwrap();
    ns.invalidate(&objects(&["temporary", "target"])).await.unwrap();
    let replacement = ns.lookup(ns.root(), "target").await.unwrap();
    assert_eq!((replacement.ino, replacement.object_id), (temporary.ino, temporary.object_id));
    assert_ne!(replacement.ino, old.ino);
    assert_eq!(replacement.size, 11);
    assert_eq!(ns.getattr(old.ino).await.unwrap_err(), FsError::Stale);
    assert_eq!(ns.lookup(ns.root(), "temporary").await.unwrap_err(), FsError::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reopening_keeps_inodes_and_distrusts_old_metadata_online() {
    let f = Fixture::new().await;
    f.put("file", "old").await;
    let ns = f.session("drv").await;
    let root = ns.root();
    let before = ns.lookup(root, "file").await.unwrap();
    assert_eq!(ns.lookup(root, "new").await.unwrap_err(), FsError::NotFound);
    drop(ns);
    f.put("file", "updated").await;
    f.put("new", "new").await;
    drop(f.store);
    let reopened = Arc::new(Store::open(f.state.path()).unwrap());
    f.proxy.clear();
    let ns = Session::new(reopened, f.client, "drv", f.connectivity).await.unwrap();
    assert_eq!(ns.root(), root);
    let after = ns.getattr(before.ino).await.unwrap();
    assert_eq!((after.ino, after.object_id, after.size), (before.ino, before.object_id, 7));
    assert_ne!(after.version_id, before.version_id);
    assert!(ns.lookup(root, "new").await.is_ok());
    assert_eq!(f.proxy.seen().len(), 1, "the persisted listing was refreshed once");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drives_have_separate_roots_inodes_and_invalidations() {
    let f = Fixture::new().await;
    f.put("file", "one").await;
    f.remote.create_drive("other", Default::default()).await.unwrap();
    f.remote.put_object("other", "file", "different", Default::default()).await.unwrap();
    let one = f.session("drv").await;
    let other = f.session("other").await;
    let a = one.lookup(one.root(), "file").await.unwrap();
    let b = other.lookup(other.root(), "file").await.unwrap();
    assert_ne!(one.root(), other.root());
    assert_ne!(a.ino, b.ino);
    assert_eq!((a.size, b.size), (3, 9));
    assert_eq!(one.getattr(b.ino).await.unwrap_err(), FsError::Stale);
    assert_eq!(other.getattr(a.ino).await.unwrap_err(), FsError::Stale);
    f.proxy.clear();
    let affected = one.invalidate(&[Invalidation::All]).await.unwrap();
    assert!(affected.contains(&a.ino) && !affected.contains(&b.ino));
    assert_eq!(other.lookup(other.root(), "file").await.unwrap(), b);
    assert!(f.proxy.seen().is_empty(), "one drive's invalidation leaves the other listing fresh");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_creation_invalidation_discards_the_negative_lookup_cache() {
    let f = Fixture::new().await;
    let ns = f.session("drv").await;
    assert_eq!(ns.lookup(ns.root(), "new").await.unwrap_err(), FsError::NotFound);
    f.put("new", "bytes").await;
    f.proxy.clear();
    assert_eq!(ns.lookup(ns.root(), "new").await.unwrap_err(), FsError::NotFound);
    assert!(f.proxy.seen().is_empty());
    assert!(ns.invalidate(&objects(&["new"])).await.unwrap().contains(&ns.root()));
    let created = ns.lookup(ns.root(), "new").await.unwrap();
    assert_eq!(created.size, 5);
    assert_eq!(ns.lookup(ns.root(), "new").await.unwrap(), created);
    assert_eq!(f.proxy.seen().len(), 1, "creation needs one new listing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn equivalent_unicode_lookups_preserve_spellings_and_report_collisions() {
    let f = Fixture::new().await;
    let nfc = "caf\u{e9}";
    let nfd = "cafe\u{301}";
    f.put(nfc, "composed").await;
    f.put("Readme", "mixed case").await;
    f.put("README", "upper case").await;
    let ns = f.session("drv").await;
    let first = ns.lookup(ns.root(), nfc).await.unwrap();
    assert_eq!(ns.lookup(ns.root(), nfd).await.unwrap(), first);
    assert_ne!(ns.lookup(ns.root(), "Readme").await.unwrap().ino, ns.lookup(ns.root(), "README").await.unwrap().ino);
    assert_eq!(ns.lookup(ns.root(), "readme").await.unwrap_err(), FsError::NotFound);

    f.put(nfd, "decomposed").await;
    ns.invalidate(&objects(&[nfd])).await.unwrap();
    for spelling in [nfc, nfd] {
        assert_eq!(ns.lookup(ns.root(), spelling).await.unwrap_err(), FsError::Ambiguous);
    }
    let entries = ns.readdir(ns.root(), None, 10).await.unwrap();
    let spellings: Vec<_> = entries.iter().map(|(name, _)| name.as_str()).collect();
    assert!(spellings.contains(&nfc) && spellings.contains(&nfd), "remote spellings survive: {spellings:?}");
    let decomposed = entries.iter().find(|(name, _)| name == nfd).unwrap();
    assert_ne!(decomposed.1.ino, first.ino);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_names_non_directories_and_stale_inodes_have_distinct_errors() {
    let f = Fixture::new().await;
    f.put("file", "bytes").await;
    let ns = f.session("drv").await;
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    assert_eq!(ns.lookup(file.ino, "child").await.unwrap_err(), FsError::NotDir);
    assert_eq!(ns.readdir(file.ino, None, 1).await.unwrap_err(), FsError::NotDir);
    for name in ["", ".", "..", "two/names", "nul\0name"] {
        assert_eq!(ns.lookup(ns.root(), name).await.unwrap_err(), FsError::InvalidName, "{name:?}");
    }
    for stale in [0, 999_999, u64::MAX] {
        assert_eq!(ns.getattr(stale).await.unwrap_err(), FsError::Stale);
        assert_eq!(ns.lookup(stale, "child").await.unwrap_err(), FsError::Stale);
        assert_eq!(ns.readdir(stale, None, 1).await.unwrap_err(), FsError::Stale);
    }
}

/// The SDK has no symlink-creation call yet, so serve a protocol listing directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn symlink_attributes_keep_the_target_and_do_not_behave_as_a_directory() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().route("/drv", axum::routing::get(|| async {
        axum::Json(serde_json::json!({
            "prefix": "", "seq": 1, "entries": [{
                "name": "link", "kind": "symlink", "objectId": "o-link", "versionId": "v1",
                "size": 3, "etag": "etag", "mtime": "2026-01-02T03:04:05Z",
                "mode": "0777", "hasXattrs": true, "target": "foo"
            }]
        }))
    }));
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let dir = tempfile::tempdir().unwrap();
    let ns = Session::new(Arc::new(Store::open(dir.path()).unwrap()), client_for(&endpoint, Config::default()), "drv", Connectivity::default()).await.unwrap();
    let link = ns.lookup(ns.root(), "link").await.unwrap();
    assert_eq!((link.kind, link.size, link.mode, link.target.as_deref()), (Kind::Symlink, 3, 0o777, Some("foo")));
    assert!(link.has_xattrs);
    assert_eq!(ns.getattr(link.ino).await.unwrap(), link);
    assert_eq!(ns.lookup(link.ino, "child").await.unwrap_err(), FsError::NotDir);
    assert_eq!(ns.readdir(link.ino, None, 1).await.unwrap_err(), FsError::NotDir);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_reopening_serves_complete_snapshots_and_fails_uncached_lookups_without_requests() {
    let f = Fixture::new().await;
    f.put("cached/file", "bytes").await;
    f.put("uncached/file", "remote").await;
    let ns = f.session("drv").await;
    let root = ns.root();
    let cached = ns.lookup(root, "cached").await.unwrap().ino;
    let uncached = ns.lookup(root, "uncached").await.unwrap().ino;
    let file = ns.lookup(cached, "file").await.unwrap();
    for _ in 0..3 { f.connectivity.unanswered(); }
    assert_eq!(f.connectivity.link(), Link::Offline);
    drop(ns);
    drop(f.store);
    let reopened = Arc::new(Store::open(f.state.path()).unwrap());
    let ns = Session::new(reopened, f.client, "drv", f.connectivity.clone()).await.unwrap();
    f.proxy.clear();
    assert_eq!(ns.root(), root);
    assert_eq!(ns.lookup(cached, "file").await.unwrap(), file);
    assert_eq!(ns.getattr(file.ino).await.unwrap(), file);
    assert_eq!(ns.readdir(cached, None, 10).await.unwrap()[0], ("file".into(), file));
    assert_eq!(ns.lookup(cached, "missing").await.unwrap_err(), FsError::Offline);
    assert_eq!(ns.lookup(uncached, "file").await.unwrap_err(), FsError::Offline);
    assert_eq!(ns.readdir(uncached, None, 10).await.unwrap_err(), FsError::Offline);
    assert!(f.proxy.seen().is_empty(), "offline metadata never waits for the network");

    f.connectivity.answered(false);
    assert_eq!(ns.lookup(uncached, "file").await.unwrap().size, 6);
    assert_eq!(f.proxy.seen().len(), 2, "reconnecting refreshes the root and the uncached directory");
}
