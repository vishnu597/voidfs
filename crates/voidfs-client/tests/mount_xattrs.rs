// SPDX-License-Identifier: Apache-2.0
//! Byte-preserving xattrs and durable local metadata, against the server and controlled races.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use base64::Engine;
use common::{Proxy, client_for, server};
use voidfs_client::mount::{FsError, RenameMode, Session, Sync, XattrMode};
use voidfs_client::{ApiFetcher, Cache, CacheConfig, Connectivity, Invalidation, Link, Queue, QueueConfig, Scope, Store};
use voidfs_sdk::{AttributesUpdate, Client, Config};
use voidfs_server::test_server::TestServer;

struct Fixture {
    _server: TestServer,
    proxy: Proxy,
    remote: Client,
    client: Client,
    _state: tempfile::TempDir,
    store: Arc<Store>,
    queue: Queue,
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
        let connectivity = Connectivity::default();
        let queue = Queue::open(store.clone(), client.clone(), QueueConfig {
            connectivity: Some(connectivity.clone()), ..Default::default()
        }).await.unwrap();
        queue.pause(Scope::All).await.unwrap();
        Self { _server: server, proxy, remote, client, _state: state, store, queue, connectivity }
    }

    async fn session(&self) -> Session { self.session_with(self.client.clone()).await }

    async fn session_with(&self, client: Client) -> Session {
        let cache = Cache::open(self.store.clone(), Arc::new(ApiFetcher::new(client.clone()).with_connectivity(self.connectivity.clone())),
            CacheConfig { min_free_bytes: 0, ..Default::default() }).await.unwrap();
        Session::new_writable(self.store.clone(), client, cache, self.queue.clone(), "drv", self.connectivity.clone()).await.unwrap()
    }

    async fn put(&self, key: &str, attrs: &[(&str, &[u8])]) {
        self.remote.put_object("drv", key, "bytes", Default::default()).await.unwrap();
        if !attrs.is_empty() {
            self.remote.set_attributes("drv", key, AttributesUpdate {
                set_xattrs: attrs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_vec().into())).collect(), ..Default::default()
            }, Default::default()).await.unwrap();
        }
    }

    fn offline(&self) {
        for _ in 0..3 { self.connectivity.unanswered(); }
        assert_eq!(self.connectivity.link(), Link::Offline);
    }
}

fn attrs_requests(proxy: &Proxy) -> Vec<String> { proxy.seen().into_iter().filter(|r| r.contains("x-voidfs-attrs")).collect() }

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_xattrs_is_complete_without_an_attributes_request() {
    let f = Fixture::new().await;
    f.put("file", &[]).await;
    let ns = f.session().await;
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    f.proxy.clear();
    assert_eq!(ns.getxattr(file.ino, "com.apple.FinderInfo").await.unwrap_err(), FsError::NoAttr);
    assert!(ns.listxattr(file.ino).await.unwrap().is_empty());
    assert!(ns.listxattr(ns.root()).await.unwrap().is_empty());
    assert!(f.proxy.seen().is_empty());
    f.offline();
    assert_eq!(ns.getxattr(file.ino, "com.apple.ResourceFork").await.unwrap_err(), FsError::NoAttr);
    assert!(ns.listxattr(file.ino).await.unwrap().is_empty());
    assert!(f.proxy.seen().is_empty());
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_attributes_are_pinned_cached_and_preserve_empty_binary_values() {
    let f = Fixture::new().await;
    f.put("cached", &[("user.empty", b""), ("user.binary", &[0, 255, 128, 1])]).await;
    f.put("uncached", &[("user.tag", b"other")]).await;
    let ns = f.session().await;
    let file = ns.lookup(ns.root(), "cached").await.unwrap();
    let uncached = ns.lookup(ns.root(), "uncached").await.unwrap();
    f.proxy.clear();
    assert_eq!(ns.getxattr(file.ino, "user.empty").await.unwrap(), b"");
    assert_eq!(ns.getxattr(file.ino, "user.binary").await.unwrap(), [0, 255, 128, 1]);
    assert_eq!(ns.listxattr(file.ino).await.unwrap(), ["user.binary", "user.empty"]);
    let requests = attrs_requests(&f.proxy);
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(requests[0].contains(&format!("versionId={}", file.version_id.unwrap())), "{requests:?}");
    f.proxy.clear();
    f.offline();
    assert_eq!(ns.getxattr(file.ino, "user.binary").await.unwrap(), [0, 255, 128, 1]);
    assert_eq!(ns.listxattr(uncached.ino).await.unwrap_err(), FsError::Offline);
    assert!(f.proxy.seen().is_empty(), "offline xattr miss made a request");
    drop(ns);
    let reopened = f.session().await;
    assert_eq!(reopened.getxattr(file.ino, "user.empty").await.unwrap(), b"");
    assert_eq!(reopened.listxattr(uncached.ino).await.unwrap_err(), FsError::Offline);
    assert!(f.proxy.seen().is_empty(), "cached xattrs were lost on restart");
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_create_replace_remove_and_empty_values_are_durable() {
    let f = Fixture::new().await;
    let ns = f.session().await;
    ns.readdir(ns.root(), None, 1).await.unwrap();
    f.offline();
    f.proxy.clear();
    let file = ns.create(ns.root(), "local", 0o600).await.unwrap();
    ns.setxattr(file.ino, "com.apple.provenance", b"", XattrMode::Create).await.unwrap();
    assert_eq!(ns.getxattr(file.ino, "com.apple.provenance").await.unwrap(), b"");
    assert_eq!(ns.setxattr(file.ino, "com.apple.provenance", b"wrong", XattrMode::Create).await.unwrap_err(), FsError::Exists);
    assert_eq!(ns.setxattr(file.ino, "missing", b"wrong", XattrMode::Replace).await.unwrap_err(), FsError::NoAttr);
    ns.setxattr(file.ino, "com.apple.provenance", &[0, 128, 255], XattrMode::Replace).await.unwrap();
    ns.setxattr(file.ino, "user.tag", b"red", XattrMode::Set).await.unwrap();
    assert_eq!(ns.listxattr(file.ino).await.unwrap(), ["com.apple.provenance", "user.tag"]);
    assert_eq!(ns.getattr(file.ino).await.unwrap().sync, Sync::Pending);
    drop(ns);
    let ns = f.session().await;
    assert_eq!(ns.getxattr(file.ino, "com.apple.provenance").await.unwrap(), [0, 128, 255]);
    ns.removexattr(file.ino, "com.apple.provenance").await.unwrap();
    assert_eq!(ns.removexattr(file.ino, "com.apple.provenance").await.unwrap_err(), FsError::NoAttr);
    ns.removexattr(file.ino, "user.tag").await.unwrap();
    assert!(!ns.getattr(file.ino).await.unwrap().has_xattrs);
    assert!(ns.listxattr(file.ino).await.unwrap().is_empty());
    assert!(f.proxy.seen().is_empty(), "local xattrs required network: {:?}", f.proxy.seen());
    let status = f.queue.status().await.unwrap();
    assert_eq!(status.unpublished, 6, "failed Create/Replace/Remove must not add entries");
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn complete_map_limits_names_and_total_bytes_without_partial_changes() {
    let f = Fixture::new().await;
    f.put("file", &[("old", b"kept")]).await;
    let ns = f.session().await;
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    let before = ns.getattr(file.ino).await.unwrap();
    for name in [String::new(), "n".repeat(256), "bad\0name".into()] {
        assert_eq!(ns.setxattr(file.ino, &name, b"x", XattrMode::Set).await.unwrap_err(), FsError::InvalidName);
        assert_eq!(ns.getxattr(file.ino, &name).await.unwrap_err(), FsError::InvalidName);
        assert_eq!(ns.removexattr(file.ino, &name).await.unwrap_err(), FsError::InvalidName);
    }
    let too_large = vec![0; 64 * 1024 - "new".len()];
    assert_eq!(ns.setxattr(file.ino, "new", &too_large, XattrMode::Set).await.unwrap_err(), FsError::TooLarge);
    assert_eq!(ns.getattr(file.ino).await.unwrap(), before);
    assert_eq!(ns.getxattr(file.ino, "old").await.unwrap(), b"kept");
    assert_eq!(f.queue.status().await.unwrap().unpublished, 0);
    ns.removexattr(file.ino, "old").await.unwrap();
    ns.setxattr(file.ino, "new", &too_large, XattrMode::Create).await.unwrap();
    assert_eq!(ns.getxattr(file.ino, "new").await.unwrap(), too_large, "64 KiB including name is accepted");
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clean_map_refreshes_by_version_and_dirty_map_survives_invalidation() {
    let f = Fixture::new().await;
    f.put("file", &[("user.tag", b"old"), ("user.keep", b"original")]).await;
    let ns = f.session().await;
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"old");
    f.remote.set_attributes("drv", "file", AttributesUpdate {
        set_xattrs: BTreeMap::from([("user.tag".into(), "remote".into())]), ..Default::default()
    }, Default::default()).await.unwrap();
    ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"remote");
    assert_eq!(attrs_requests(&f.proxy).len(), 2);
    ns.setxattr(file.ino, "user.tag", b"local", XattrMode::Replace).await.unwrap();
    f.remote.set_attributes("drv", "file", AttributesUpdate {
        set_xattrs: BTreeMap::from([("user.tag".into(), "competing".into())]), ..Default::default()
    }, Default::default()).await.unwrap();
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"local");
    assert_eq!(ns.getxattr(file.ino, "user.keep").await.unwrap(), b"original");
    assert_eq!(attrs_requests(&f.proxy).len(), 2, "dirty complete map was refetched");
    f.offline();
    drop(ns);
    let ns = f.session().await;
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"local");
    assert_eq!(ns.getxattr(file.ino, "user.keep").await.unwrap(), b"original");
    assert_eq!(ns.getattr(file.ino).await.unwrap().sync, Sync::Pending);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_rename_reads_uncached_remote_xattrs_and_keeps_dirty_attrs_after_restart() {
    let f = Fixture::new().await;
    f.put("before", &[("user.tag", b"old"), ("user.keep", b"kept")]).await;
    let ns = f.session().await;
    let file = ns.lookup(ns.root(), "before").await.unwrap();
    // Migration 6 leaves existing schema-5 cached inodes without a remembered remote path.
    rusqlite::Connection::open(f.store.dir().join("state.sqlite")).unwrap()
        .execute("UPDATE mount_inodes SET remote_key=NULL WHERE ino=?1", [file.ino]).unwrap();
    f.offline();
    f.proxy.clear();
    ns.rename(ns.root(), "before", ns.root(), "after", RenameMode::Exclusive).await.unwrap();
    assert!(f.proxy.seen().is_empty(), "a complete offline namespace must allow the rename");
    f.connectivity.answered(false);
    f.proxy.clear();
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"old");
    let requests = attrs_requests(&f.proxy);
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(requests[0].starts_with("GET /drv/before?"), "remote base must be read at its original path: {requests:?}");
    ns.setxattr(file.ino, "user.tag", b"local", XattrMode::Replace).await.unwrap();
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"local");
    f.offline();
    f.proxy.clear();
    drop(ns);
    let ns = f.session().await;
    assert_eq!(ns.lookup(ns.root(), "after").await.unwrap().ino, file.ino);
    assert_eq!(ns.lookup(ns.root(), "before").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"local");
    assert_eq!(ns.getxattr(file.ino, "user.keep").await.unwrap(), b"kept");
    assert!(f.proxy.seen().is_empty());
    f.connectivity.answered(false);
    f.queue.resume(Scope::All).await.unwrap();
    f.queue.settle().await;
    let remote = f.remote.attributes("drv", "after", Default::default()).await.unwrap();
    assert_eq!(remote.xattrs.get("user.tag").map(String::as_str), Some("bG9jYWw="));
    assert_eq!(remote.xattrs.get("user.keep").map(String::as_str), Some("a2VwdA=="));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_session_serves_xattrs_and_rejects_mutations() {
    let f = Fixture::new().await;
    f.put("file", &[("user.tag", b"read")]).await;
    let cache = Cache::open(f.store.clone(), Arc::new(ApiFetcher::new(f.client.clone())),
        CacheConfig { min_free_bytes: 0, ..Default::default() }).await.unwrap();
    let ns = Session::new(f.store.clone(), f.client.clone(), cache, "drv", f.connectivity.clone()).await.unwrap();
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"read");
    assert_eq!(ns.setxattr(file.ino, "user.tag", b"write", XattrMode::Set).await.unwrap_err(), FsError::ReadOnly);
    assert_eq!(ns.removexattr(file.ino, "user.tag").await.unwrap_err(), FsError::ReadOnly);
    assert_eq!(f.queue.status().await.unwrap().unpublished, 0);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn root_xattr_changes_are_explicitly_unsupported() {
    let f = Fixture::new().await;
    let ns = f.session().await;
    assert_eq!(ns.setxattr(ns.root(), "user.tag", b"root", XattrMode::Set).await.unwrap_err(), FsError::Unsupported);
    assert_eq!(ns.removexattr(ns.root(), "user.tag").await.unwrap_err(), FsError::Unsupported);
    assert_eq!(f.queue.status().await.unwrap().unpublished, 0);
    f.queue.close().await;
}

struct AttrProxyState {
    upstream: String,
    http: reqwest::Client,
    replacement: Mutex<Option<serde_json::Value>>,
    strip_folder_versions: std::sync::atomic::AtomicBool,
    hold_next: std::sync::atomic::AtomicBool,
    started: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    count: std::sync::atomic::AtomicUsize,
}

struct AttrProxy { endpoint: String, state: Arc<AttrProxyState>, task: tokio::task::JoinHandle<()> }

impl Drop for AttrProxy { fn drop(&mut self) { self.task.abort(); } }

impl AttrProxy {
    async fn start(upstream: &str) -> Self {
        let state = Arc::new(AttrProxyState { upstream: upstream.to_owned(), http: reqwest::Client::new(),
            replacement: Mutex::new(None), strip_folder_versions: false.into(), hold_next: false.into(), started: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0), count: 0.into() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback(attr_proxy).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        Self { endpoint, state, task }
    }

    fn count(&self) -> usize { self.state.count.load(std::sync::atomic::Ordering::SeqCst) }

    async fn started(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.state.started.acquire()).await.unwrap().unwrap().forget();
    }
}

async fn attr_proxy(State(st): State<Arc<AttrProxyState>>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let pq = parts.uri.path_and_query().unwrap().as_str();
    let is_attrs = pq.contains("x-voidfs-attrs");
    let is_listing = pq.contains("x-voidfs-list");
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let mut request = st.http.request(parts.method, format!("{}{pq}", st.upstream));
    for (key, value) in &parts.headers { request = request.header(key, value); }
    let remote = request.body(body).send().await.unwrap();
    let mut response = Response::builder().status(remote.status().as_u16());
    for (key, value) in remote.headers() {
        if !matches!(key.as_str(), "content-length" | "transfer-encoding" | "connection") { response = response.header(key, value); }
    }
    let mut bytes = remote.bytes().await.unwrap();
    if is_listing && st.strip_folder_versions.load(std::sync::atomic::Ordering::SeqCst) {
        let mut listing: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for entry in listing["entries"].as_array_mut().unwrap() {
            if entry["kind"] == "folder" { entry.as_object_mut().unwrap().remove("versionId"); }
        }
        bytes = serde_json::to_vec(&listing).unwrap().into();
    }
    if is_attrs {
        st.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if st.hold_next.swap(false, std::sync::atomic::Ordering::SeqCst) {
            st.started.add_permits(1);
            st.release.acquire().await.unwrap().forget();
        }
        if let Some(replacement) = st.replacement.lock().unwrap().take() { bytes = serde_json::to_vec(&replacement).unwrap().into(); }
    }
    response.body(Body::from(bytes)).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_remote_folder_xattrs_bind_a_version_missing_from_the_listing() {
    let f = Fixture::new().await;
    f.put("folder/file", &[]).await;
    f.remote.set_attributes("drv", "folder/", AttributesUpdate {
        set_xattrs: BTreeMap::from([("user.tag".into(), vec![0, 255, 128].into())]), ..Default::default()
    }, Default::default()).await.unwrap();
    let proxy = AttrProxy::start(&f._server.endpoint).await;
    proxy.state.strip_folder_versions.store(true, std::sync::atomic::Ordering::SeqCst);
    let client = client_for(&proxy.endpoint, Config::default());
    let cache = Cache::open(f.store.clone(), Arc::new(ApiFetcher::new(client.clone())),
        CacheConfig { min_free_bytes: 0, ..Default::default() }).await.unwrap();
    let ns = Session::new(f.store.clone(), client, cache, "drv", f.connectivity.clone()).await.unwrap();
    let folder = ns.lookup(ns.root(), "folder").await.unwrap();
    assert!(folder.has_xattrs && folder.version_id.is_none());
    assert_eq!(ns.getxattr(folder.ino, "user.tag").await.unwrap(), [0, 255, 128]);
    assert_eq!(ns.listxattr(folder.ino).await.unwrap(), ["user.tag"]);
    assert_eq!(proxy.count(), 2, "bind the folder version, then fetch its immutable attributes");
    f.offline();
    assert_eq!(ns.getxattr(folder.ino, "user.tag").await.unwrap(), [0, 255, 128]);
    assert_eq!(proxy.count(), 2);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_folder_replacement_between_identity_check_and_listing_is_rejected() {
    let f = Fixture::new().await;
    f.put("before/original", &[]).await;
    f.put("replacement/intruder", &[]).await;
    let proxy = AttrProxy::start(&f._server.endpoint).await;
    let ns = Arc::new(f.session_with(client_for(&proxy.endpoint, Config::default())).await);
    let folder = ns.lookup(ns.root(), "before").await.unwrap();
    ns.lookup(folder.ino, "original").await.unwrap();
    ns.rename(ns.root(), "before", ns.root(), "after", RenameMode::Exclusive).await.unwrap();
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    proxy.state.hold_next.store(true, std::sync::atomic::Ordering::SeqCst);
    let listing = { let ns = ns.clone(); tokio::spawn(async move { ns.readdir(folder.ino, None, 10).await }) };
    proxy.started().await;
    f.remote.rename("drv", "before/", "away/", Default::default()).await.unwrap();
    f.remote.rename("drv", "replacement/", "before/", Default::default()).await.unwrap();
    proxy.state.release.add_permits(1);
    let result = tokio::time::timeout(Duration::from_secs(5), listing).await.unwrap().unwrap();
    assert!(matches!(result, Err(FsError::Again | FsError::NotFound | FsError::Stale)), "a replacement directory populated the pending inode: {result:?}");
    f.offline();
    let cached = ns.readdir(folder.ino, None, 10).await.unwrap();
    assert_eq!(cached.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["original"], "rejected listing must retain the last accepted children");
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_identity_version_names_and_base64_are_not_memoized() {
    let f = Fixture::new().await;
    f.put("file", &[("user.tag", b"good")]).await;
    let proxy = AttrProxy::start(&f._server.endpoint).await;
    let ns = f.session_with(client_for(&proxy.endpoint, Config::default())).await;
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    let good = serde_json::json!({"objectId":file.object_id,"versionId":file.version_id,"kind":"file","xattrs":{"user.tag":"Z29vZA=="}});
    let mut bad_id = good.clone(); bad_id["objectId"] = "different".into();
    let mut bad_version = good.clone(); bad_version["versionId"] = "0.1".into();
    let mut bad_kind = good.clone(); bad_kind["kind"] = "folder".into();
    let mut bad_base64 = good.clone(); bad_base64["xattrs"]["user.tag"] = "not base64!".into();
    let mut bad_name = good.clone(); bad_name["xattrs"] = serde_json::json!({"bad\u{0000}name":""});
    let mut too_large = good.clone(); too_large["xattrs"]["user.tag"] = base64::engine::general_purpose::STANDARD.encode(vec![0; 64 * 1024]).into();
    for bad in [bad_id, bad_version, bad_kind, bad_base64, bad_name, too_large] {
        *proxy.state.replacement.lock().unwrap() = Some(bad);
        assert!(matches!(ns.getxattr(file.ino, "user.tag").await, Err(FsError::Io(_))));
    }
    assert_eq!(proxy.count(), 6, "malformed responses were cached");
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"good");
    assert_eq!(proxy.count(), 7);
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"good");
    assert_eq!(proxy.count(), 7);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalidation_rejects_an_inflight_xattr_snapshot_and_retries_at_the_new_version() {
    let f = Fixture::new().await;
    f.put("file", &[("user.tag", b"old")]).await;
    let proxy = AttrProxy::start(&f._server.endpoint).await;
    let ns = Arc::new(f.session_with(client_for(&proxy.endpoint, Config::default())).await);
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    proxy.state.hold_next.store(true, std::sync::atomic::Ordering::SeqCst);
    let reader = { let ns = ns.clone(); tokio::spawn(async move { ns.getxattr(file.ino, "user.tag").await }) };
    proxy.started().await;
    f.remote.set_attributes("drv", "file", AttributesUpdate {
        set_xattrs: BTreeMap::from([("user.tag".into(), "new".into())]), ..Default::default()
    }, Default::default()).await.unwrap();
    ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
    ns.readdir(ns.root(), None, 10).await.unwrap();
    proxy.state.release.add_permits(1);
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), reader).await.unwrap().unwrap().unwrap(), b"new");
    assert_eq!(proxy.count(), 2, "rejected generation did not refetch the new version");
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"new");
    assert_eq!(proxy.count(), 2);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_local_mutation_wins_an_inflight_remote_snapshot() {
    let f = Fixture::new().await;
    f.put("file", &[("user.tag", b"remote"), ("user.keep", b"kept")]).await;
    let proxy = AttrProxy::start(&f._server.endpoint).await;
    let ns = Arc::new(f.session_with(client_for(&proxy.endpoint, Config::default())).await);
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    proxy.state.hold_next.store(true, std::sync::atomic::Ordering::SeqCst);
    let reader = { let ns = ns.clone(); tokio::spawn(async move { ns.getxattr(file.ino, "user.tag").await }) };
    proxy.started().await;
    ns.setxattr(file.ino, "user.tag", b"local", XattrMode::Replace).await.unwrap();
    proxy.state.release.add_permits(1);
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), reader).await.unwrap().unwrap().unwrap(), b"local");
    assert_eq!(ns.getxattr(file.ino, "user.keep").await.unwrap(), b"kept");
    assert_eq!(proxy.count(), 2, "dirty local map should end the retry without another fetch");
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_xattr_generation_races_have_a_bounded_retry_count() {
    let f = Fixture::new().await;
    f.put("file", &[("user.tag", b"remote")]).await;
    let proxy = AttrProxy::start(&f._server.endpoint).await;
    let ns = Arc::new(f.session_with(client_for(&proxy.endpoint, Config::default())).await);
    let file = ns.lookup(ns.root(), "file").await.unwrap();
    proxy.state.hold_next.store(true, std::sync::atomic::Ordering::SeqCst);
    let reader = { let ns = ns.clone(); tokio::spawn(async move { ns.getxattr(file.ino, "user.tag").await }) };
    for attempt in 0..4 {
        proxy.started().await;
        ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
        ns.readdir(ns.root(), None, 10).await.unwrap();
        proxy.state.hold_next.store(attempt < 3, std::sync::atomic::Ordering::SeqCst);
        proxy.state.release.add_permits(1);
    }
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), reader).await.unwrap().unwrap().unwrap_err(), FsError::Again);
    assert_eq!(proxy.count(), 4);
    assert_eq!(ns.getxattr(file.ino, "user.tag").await.unwrap(), b"remote");
    assert_eq!(proxy.count(), 5, "the rejected maps must never become the cache");
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_and_local_folders_support_guarded_xattr_changes() {
    let f = Fixture::new().await;
    f.put("remote/file", &[]).await;
    let ns = f.session().await;
    let remote = ns.lookup(ns.root(), "remote").await.unwrap();
    let local = ns.mkdir(ns.root(), "local", 0o755).await.unwrap();
    ns.setxattr(remote.ino, "user.tag", b"remote folder", XattrMode::Create).await.unwrap();
    ns.setxattr(local.ino, "user.tag", b"local folder", XattrMode::Create).await.unwrap();
    assert_eq!(ns.getxattr(remote.ino, "user.tag").await.unwrap(), b"remote folder");
    assert_eq!(ns.getxattr(local.ino, "user.tag").await.unwrap(), b"local folder");
    f.queue.resume(Scope::All).await.unwrap();
    f.queue.settle().await;
    assert_eq!(f.remote.attributes("drv", "remote/", Default::default()).await.unwrap().xattrs.get("user.tag").map(String::as_str), Some("cmVtb3RlIGZvbGRlcg=="));
    assert_eq!(f.remote.attributes("drv", "local/", Default::default()).await.unwrap().xattrs.get("user.tag").map(String::as_str), Some("bG9jYWwgZm9sZGVy"));
    f.queue.close().await;
}
