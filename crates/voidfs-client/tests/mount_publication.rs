// SPDX-License-Identifier: Apache-2.0
//! Publication acknowledgement, immutable handles and locally retained conflicts.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State as AppState;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use common::client_for;
use rusqlite::Connection;
use voidfs_client::mount::{ConflictSide, FsError, RenameMode, Session, StagingConfig, Sync, XattrMode};
use voidfs_client::{ApiFetcher, BucketConfig, BucketFetcher, Cache, CacheConfig, Connectivity, Fetch,
    Invalidation, Queue, QueueConfig, Scope, State, Store};
use voidfs_sdk::{AttributesUpdate, Client, Config, Kind, PutOptions, ReadOptions};
use voidfs_server::test_server::{Rules, TestServer};

#[derive(Clone, Copy)]
enum Backend { Api, Bucket }

#[derive(Clone, Copy)]
enum Action { Hold, Status(u16), OmitIdentity, InvalidMode, InvalidXattrs, WrongKind }
struct Rule { method: &'static str, path: String, query: &'static str, exclude: Option<&'static str>, action: Action, remaining: usize }
struct ProxyState {
    upstream: String, http: reqwest::Client, rule: Mutex<Option<Rule>>,
    started: tokio::sync::Semaphore, release: tokio::sync::Semaphore,
}
struct Proxy { endpoint: String, state: Arc<ProxyState>, task: tokio::task::JoinHandle<()> }

impl Drop for Proxy { fn drop(&mut self) { self.task.abort(); } }

impl Proxy {
    async fn start(upstream: &str) -> Self {
        let state = Arc::new(ProxyState { upstream: upstream.into(), http: reqwest::Client::new(), rule: Mutex::new(None),
            started: tokio::sync::Semaphore::new(0), release: tokio::sync::Semaphore::new(0) });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback(proxy).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        Self { endpoint, state, task }
    }

    fn arm(&self, method: &'static str, key: &str, query: &'static str, action: Action) {
        let mut rule = self.state.rule.lock().unwrap();
        assert!(rule.is_none());
        *rule = Some(Rule { method, path: format!("/drv/{key}"), query, exclude: None, remaining: if matches!(action, Action::WrongKind) { 2 } else { 1 }, action });
    }

    fn arm_content(&self, key: &str, status: u16) {
        self.arm("GET", key, "versionId=", Action::Status(status));
        self.state.rule.lock().unwrap().as_mut().unwrap().exclude = Some("x-voidfs-attrs");
    }

    async fn committed(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.state.started.acquire()).await.expect("the selected publication reached the server").unwrap().forget();
    }

    fn release(&self) { self.state.release.add_permits(1); }
}

async fn proxy(AppState(st): AppState<Arc<ProxyState>>, request: axum::extract::Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let query = parts.uri.query().unwrap_or_default();
    let action = {
        let mut rule = st.rule.lock().unwrap();
        if rule.as_ref().is_some_and(|r| (parts.method.as_str() == r.method || r.method == "*") && path == r.path && query.contains(r.query)
            && !r.exclude.is_some_and(|excluded| query.contains(excluded))) {
            let selected = rule.as_mut().unwrap();
            let action = selected.action;
            selected.remaining -= 1;
            if selected.remaining == 0 { rule.take(); }
            Some(action)
        } else { None }
    };
    if let Some(Action::Status(status)) = action {
        return Response::builder().status(status).header("content-type", "application/xml").body(Body::from(
            "<Error><Code>AccessDenied</Code><Message>injected publication failure</Message><RequestId>proxy</RequestId></Error>"
        )).unwrap();
    }
    let pq = parts.uri.path_and_query().unwrap().as_str();
    let head = parts.method == axum::http::Method::HEAD;
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let mut request = st.http.request(parts.method, format!("{}{pq}", st.upstream));
    for (key, value) in &parts.headers { request = request.header(key, value); }
    let remote = request.body(body).send().await.unwrap();
    let mut response = Response::builder().status(remote.status().as_u16());
    for (key, value) in remote.headers() {
        if !matches!(key.as_str(), "transfer-encoding" | "connection") && (head || key.as_str() != "content-length")
            && !(matches!(action, Some(Action::OmitIdentity)) && key.as_str() == "x-voidfs-object-id")
        { response = response.header(key, if matches!(action, Some(Action::WrongKind)) && key.as_str() == "x-voidfs-kind" { axum::http::HeaderValue::from_static("folder") } else { value.clone() }); }
    }
    let mut body = remote.bytes().await.unwrap();
    if matches!(action, Some(Action::InvalidMode | Action::InvalidXattrs | Action::WrongKind)) && !head {
        let mut attrs: serde_json::Value = serde_json::from_slice(&body).unwrap();
        if matches!(action, Some(Action::InvalidMode)) { attrs["mode"] = "invalid".into(); }
        else if matches!(action, Some(Action::InvalidXattrs)) { attrs["xattrs"] = serde_json::json!({ "user.tag": "!invalid-base64!" }); }
        else { attrs["kind"] = "folder".into(); }
        body = serde_json::to_vec(&attrs).unwrap().into();
    }
    if matches!(action, Some(Action::Hold)) {
        st.started.add_permits(1);
        tokio::time::timeout(Duration::from_secs(20), st.release.acquire()).await.expect("the test releases the publication response").unwrap().forget();
    }
    response.body(Body::from(body)).unwrap()
}

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(1 << 50) }
fn staging() -> StagingConfig { StagingConfig { min_free_bytes: 0, free_space: Some(plenty), quiet_period: None, ..Default::default() } }
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
        remote.create_drive("drv", Default::default()).await.unwrap();
        let connectivity = Connectivity::default();
        let client = client_for(&proxy.endpoint, Config { observer: Some(Arc::new(connectivity.clone())), ..Default::default() });
        let state = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(state.path()).unwrap());
        let cache = cache(store.clone(), client.clone(), connectivity.clone(), backend).await;
        let queue = Queue::open(store.clone(), client.clone(), queue_config(&connectivity)).await.unwrap();
        queue.pause(Scope::All).await.unwrap();
        Self { _server: server, proxy, remote, client, state, store, cache, queue, connectivity, backend }
    }

    async fn session(&self) -> Arc<Session> {
        Arc::new(Session::new_writable_with_config(self.store.clone(), self.client.clone(), self.cache.clone(), self.queue.clone(),
            "drv", self.connectivity.clone(), staging()).await.unwrap())
    }

    async fn put(&self, key: &str, body: &'static [u8]) {
        self.remote.put_object("drv", key, Bytes::from_static(body), PutOptions { mode: Some(0o640), ..Default::default() }).await.unwrap();
    }

    async fn body(&self, key: &str) -> Option<Bytes> {
        match self.remote.get_object("drv", key, ReadOptions::default()).await {
            Ok(object) => Some(object.body), Err(error) if error.status() == Some(404) => None, Err(error) => panic!("{error}"),
        }
    }

    async fn publish(&self) {
        self.queue.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), self.queue.settle()).await.expect("publication settles");
    }

    fn db(&self) -> Connection { Connection::open(self.state.path().join("state.sqlite")).unwrap() }
    fn overlays(&self) -> u64 { self.db().query_row("SELECT count(*) FROM mount_overlay", [], |r| r.get(0)).unwrap() }

    async fn restart(self, session: Arc<Session>) -> (Self, Arc<Session>) {
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
        let f = Self { _server, proxy, remote, client, state, store, cache, queue, connectivity, backend };
        let ns = f.session().await;
        (f, ns)
    }
}

async fn existing(f: &Fixture, ns: &Session) -> (u64, u64) {
    f.put("file", b"0123456789ab").await;
    let ino = ns.lookup(ns.root(), "file").await.unwrap().ino;
    (ino, ns.open(ino, true).await.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn creations_bind_saved_identity_and_adopt_names_and_xattrs() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let folder = ns.mkdir(ns.root(), "folder", 0o750).await.unwrap();
        let file = ns.create(folder.ino, "new", 0o600).await.unwrap();
        ns.setxattr(file.ino, "user.binary", &[0, 255, 0], XattrMode::Set).await.unwrap();
        let fh = ns.open(file.ino, true).await.unwrap();
        ns.write(fh, 0, Bytes::from_static(b"published data")).await.unwrap();
        ns.fsync(fh).await.unwrap();
        f.publish().await;
        let saved = ns.lookup(folder.ino, "new").await.unwrap();
        let remote = f.remote.head_object("drv", "folder/new", Default::default()).await.unwrap();
        assert_eq!((saved.ino, saved.sync, saved.size, saved.mode), (file.ino, Sync::Saved, 14, 0o600));
        assert_eq!((saved.object_id, saved.version_id, saved.etag), (remote.object_id, Some(remote.version_id.clone()), Some(remote.etag)));
        assert_eq!(ns.getattr(folder.ino).await.unwrap().sync, Sync::Saved);
        assert_eq!(f.overlays(), 0, "committed names no longer shadow later remote namespace changes");
        let (version, dirty): (Option<String>, bool) = f.db().query_row("SELECT version, dirty FROM mount_xattrs WHERE ino=?1", [file.ino], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((version, dirty), (Some(remote.version_id), false));
        assert_eq!(ns.getxattr(file.ino, "user.binary").await.unwrap(), [0, 255, 0]);
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saved_patches_release_new_opens_without_changing_leased_snapshots() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (ino, writable) = existing(&f, &ns).await;
        let earlier = ns.open(ino, false).await.unwrap();
        ns.write(writable, 4, Bytes::from_static(b"EDIT")).await.unwrap();
        ns.fsync(writable).await.unwrap();
        f.publish().await;
        let saved = ns.getattr(ino).await.unwrap();
        assert_eq!(saved.sync, Sync::Saved);
        assert_eq!(saved.version_id, Some(f.remote.head_object("drv", "file", Default::default()).await.unwrap().version_id));
        f.put("file", b"REMOTEabcdef").await;
        ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
        let fresh = ns.open(ino, false).await.unwrap();
        assert_eq!(ns.read(fresh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"REMOTEabcdef"));
        for fh in [earlier, writable] { assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"0123EDIT89ab")); }
        for fh in [fresh, earlier, writable] { ns.close(fh).await.unwrap(); }
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_writer_cannot_join_a_stage_based_on_a_newer_external_snapshot() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (ino, earlier) = existing(&f, &ns).await;
        let original = ns.handle_attr(earlier).unwrap();
        f.put("file", b"ABCDEFGHIJKL").await;
        ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
        let current = ns.open(ino, true).await.unwrap();
        let newer = ns.handle_attr(current).unwrap();
        assert_ne!(original.version_id, newer.version_id);
        ns.write(current, 4, Bytes::from_static(b"edit")).await.unwrap();
        let (before, path): (String, String) = f.db().query_row("SELECT record,path FROM mount_staged WHERE ino=?1", [ino], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let length = std::fs::metadata(&path).unwrap().len();
        let attr = ns.getattr(ino).await.unwrap();
        assert_eq!(ns.write(earlier, 0, Bytes::from_static(b"wrong")).await.unwrap_err(), FsError::Stale);
        assert_eq!(ns.truncate(earlier, 2).await.unwrap_err(), FsError::Stale);
        assert_eq!(f.db().query_row("SELECT record FROM mount_staged WHERE ino=?1", [ino], |r| r.get::<_, String>(0)).unwrap(), before);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
        assert_eq!(ns.getattr(ino).await.unwrap(), attr);
        assert_eq!(ns.read(current, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"ABCDeditIJKL"));
        assert_eq!(ns.read(earlier, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"0123edit89ab"));
        ns.fsync(current).await.unwrap();
        let guard: String = f.db().query_row("SELECT base FROM entries WHERE mount_ino=?1 ORDER BY id LIMIT 1", [ino], |r| r.get(0)).unwrap();
        assert_eq!(guard, format!("v:{}", newer.version_id.unwrap()));
        f.publish().await;
        assert_eq!(f.body("file").await, Some(Bytes::from_static(b"ABCDeditIJKL")));
        ns.close(earlier).await.unwrap();
        ns.close(current).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listing_before_publication_ack_keeps_the_original_local_inode() {
    for folder in [false, true] {
        let f = Fixture::new(Backend::Api).await;
        let ns = f.session().await;
        let local = if folder { ns.mkdir(ns.root(), "new", 0o750).await.unwrap() } else { ns.create(ns.root(), "new", 0o600).await.unwrap() };
        let key = if folder { "new/" } else { "new" };
        f.proxy.arm("PUT", key, "", Action::Hold);
        f.queue.resume(Scope::All).await.unwrap();
        f.proxy.committed().await;
        let remote = f.remote.head_object("drv", key, Default::default()).await.unwrap();
        ns.invalidate(&[Invalidation::All]).await.unwrap();
        assert_eq!(ns.lookup(ns.root(), "new").await.unwrap().ino, local.ino);
        assert_eq!(f.db().query_row("SELECT count(*) FROM mount_inodes WHERE object_id=?1", [remote.object_id.as_deref().unwrap()], |r| r.get::<_, u64>(0)).unwrap(), 1,
            "the remote identity was observed before its local acknowledgement");
        f.proxy.release();
        f.publish().await;
        let saved = ns.lookup(ns.root(), "new").await.unwrap();
        assert_eq!((saved.ino, saved.sync, saved.object_id), (local.ino, Sync::Saved, remote.object_id));
        assert_eq!(f.overlays(), 0);
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn later_unflushed_writes_and_truncation_survive_an_earlier_ack() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = existing(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.proxy.arm("POST", "file", "x-voidfs-patch", Action::Hold);
    f.queue.resume(Scope::All).await.unwrap();
    f.proxy.committed().await;
    ns.write(fh, 0, Bytes::from_static(b"LATER")).await.unwrap();
    ns.truncate(fh, 7).await.unwrap();
    f.proxy.release();
    f.publish().await;
    let attr = ns.getattr(ino).await.unwrap();
    assert_eq!((attr.sync, attr.size), (Sync::Pending, 7), "publication cannot claim unflushed later bytes are saved");
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"LATER56"));
    ns.write(fh, 6, Bytes::from_static(b"!")).await.unwrap();
    let later = ns.open(ino, true).await.unwrap();
    ns.write(later, 5, Bytes::from_static(b"X")).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"LATERX!"));
    ns.fsync(fh).await.unwrap();
    f.publish().await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Saved);
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"LATERX!")));
    ns.close(later).await.unwrap();
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_delete_and_recreate_during_upload_do_not_resurrect_the_old_inode() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (old, fh) = existing(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"saved")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.proxy.arm("POST", "file", "x-voidfs-patch", Action::Hold);
    f.queue.resume(Scope::All).await.unwrap();
    f.proxy.committed().await;
    ns.rename(ns.root(), "file", ns.root(), "moved", RenameMode::Exclusive).await.unwrap();
    ns.unlink(ns.root(), "moved").await.unwrap();
    let replacement = ns.create(ns.root(), "moved", 0o600).await.unwrap();
    assert_ne!(replacement.ino, old);
    let replacement_handle = ns.open(replacement.ino, true).await.unwrap();
    ns.write(replacement_handle, 0, Bytes::from_static(b"replacement")).await.unwrap();
    ns.fsync(replacement_handle).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"orphan")).await.unwrap();
    f.proxy.release();
    f.publish().await;
    assert_eq!(ns.lookup(ns.root(), "file").await.unwrap_err(), FsError::NotFound);
    assert_eq!(ns.lookup(ns.root(), "moved").await.unwrap().ino, replacement.ino);
    assert_eq!(f.body("file").await, None);
    assert_eq!(f.body("moved").await, Some(Bytes::from_static(b"replacement")));
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"orphan6789ab"));
    assert_eq!(f.overlays(), 0);
    ns.close(fh).await.unwrap();
    ns.close(replacement_handle).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn folder_publication_rebases_cached_and_cold_children_across_two_moves() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        f.put("before/cached", b"cached bytes").await;
        f.put("before/cold/leaf", b"cold bytes").await;
        f.put("before/remove", b"removed").await;
        let ns = f.session().await;
        let folder = ns.lookup(ns.root(), "before").await.unwrap();
        let child = ns.lookup(folder.ino, "cached").await.unwrap();
        let fh = ns.open(child.ino, true).await.unwrap();
        ns.rename(ns.root(), "before", ns.root(), "after", RenameMode::Exclusive).await.unwrap();
        ns.write(fh, 0, Bytes::from_static(b"EDITED")).await.unwrap();
        ns.fsync(fh).await.unwrap();
        let new = ns.create(folder.ino, "new", 0o600).await.unwrap();
        ns.unlink(folder.ino, "remove").await.unwrap();
        f.proxy.arm("PUT", "after/", "x-voidfs-rename", Action::Hold);
        f.queue.resume(Scope::All).await.unwrap();
        f.proxy.committed().await;
        ns.rename(ns.root(), "after", ns.root(), "final", RenameMode::Exclusive).await.unwrap();
        f.proxy.release();
        f.publish().await;
        assert_eq!(f.db().query_row("SELECT remote_key FROM mount_inodes WHERE ino=?1", [child.ino], |r| r.get::<_, String>(0)).unwrap(), "final/cached",
            "a cached descendant follows each completed folder move before another listing repairs its path");
        ns.invalidate(&[Invalidation::All]).await.unwrap();
        assert_eq!((ns.lookup(ns.root(), "final").await.unwrap().ino, ns.lookup(folder.ino, "new").await.unwrap().ino), (folder.ino, new.ino));
        assert_eq!(ns.lookup(folder.ino, "cached").await.unwrap().ino, child.ino);
        let cold = ns.lookup(folder.ino, "cold").await.unwrap();
        let leaf = ns.lookup(cold.ino, "leaf").await.unwrap();
        let cold_handle = ns.open(leaf.ino, false).await.unwrap();
        assert_eq!(ns.read(cold_handle, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"cold bytes"));
        assert_eq!(f.body("final/cached").await, Some(Bytes::from_static(b"EDITED bytes")));
        assert_eq!(f.body("final/new").await, Some(Bytes::new()));
        assert_eq!(f.body("final/remove").await, None);
        assert_eq!(f.body("before/cached").await, None);
        assert_eq!(f.body("after/cached").await, None);
        assert_eq!(f.overlays(), 0);
        for ino in [folder.ino, child.ino, new.ino] { assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Saved); }
        ns.close(cold_handle).await.unwrap();
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn successful_removals_clear_tombstones_and_allow_new_remote_occupants() {
    for folder in [false, true] {
        let f = Fixture::new(Backend::Api).await;
        let key = if folder { "file/" } else { "file" };
        f.put(key, b"").await;
        let ns = f.session().await;
        let old = ns.lookup(ns.root(), "file").await.unwrap();
        if folder { ns.rmdir(ns.root(), "file").await.unwrap(); } else { ns.unlink(ns.root(), "file").await.unwrap(); }
        f.publish().await;
        assert_eq!(f.body(key).await, None);
        assert_eq!(f.overlays(), 0, "a published deletion no longer hides unrelated future objects");
        f.put(key, b"").await;
        ns.invalidate(&[Invalidation::Object(key.into())]).await.unwrap();
        let current = ns.lookup(ns.root(), "file").await.unwrap();
        assert_ne!(current.ino, old.ino);
        assert_eq!(current.sync, Sync::Saved);
        f.queue.close().await;
    }
    let f = Fixture::new(Backend::Api).await;
    f.put("folder/", b"").await;
    let ns = f.session().await;
    let folder = ns.lookup(ns.root(), "folder").await.unwrap();
    ns.rmdir(ns.root(), "folder").await.unwrap();
    f.put("folder/child", b"competing child").await;
    let competitor = f.remote.head_object("drv", "folder/", Default::default()).await.unwrap();
    f.publish().await;
    assert!(f.queue.status().await.unwrap().items.iter().any(|entry| entry.key == "folder/" && entry.state == State::Failed), "a remotely populated folder cannot be acknowledged as removed");
    let conflict = ns.conflict(folder.ino).await.unwrap().unwrap();
    assert_eq!(conflict.remote.unwrap().object_id, competitor.object_id);
    assert_eq!(f.body("folder/child").await, Some(Bytes::from_static(b"competing child")));
    assert_eq!(f.overlays(), 1, "the refused local deletion retains its tombstone for explicit conflict resolution");
    assert_eq!(ns.lookup(ns.root(), "folder").await.unwrap_err(), FsError::NotFound);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn patch_conflicts_keep_readable_local_and_remote_snapshots_pinned() {
    for backend in [Backend::Api, Backend::Bucket] {
        let f = Fixture::new(backend).await;
        let ns = f.session().await;
        let (ino, fh) = existing(&f, &ns).await;
        let base = ns.handle_attr(fh).unwrap().version_id;
        ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
        ns.fsync(fh).await.unwrap();
        f.put("file", b"remote one").await;
        let remote = f.remote.head_object("drv", "file", Default::default()).await.unwrap();
        f.publish().await;
        assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
        let conflict = ns.conflict(ino).await.unwrap().expect("a rejected guard records a conflict");
        assert_eq!((conflict.ino, conflict.base_version, conflict.local_size, conflict.remote_missing), (ino, base, 12, false));
        let local = conflict.local.as_ref().unwrap();
        assert_eq!((local.ino, local.size, local.mode), (ino, 12, 0o640));
        assert_eq!(conflict.remote.as_ref().unwrap().version_id.as_deref(), Some(remote.version_id.as_str()));
        assert!(conflict.local_retained && conflict.remote_retained);
        assert!(conflict.error.is_none(), "both snapshots were captured successfully");
        f.put("file", b"remote two is newer").await;
        assert_eq!(ns.read_conflict(ino, ConflictSide::Local, 0, 1024).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"));
        assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 0, 1024).await.unwrap(), Bytes::from_static(b"remote one"));
        assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 3, 4).await.unwrap(), Bytes::from_static(b"ote "));
        assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 1024, 1).await.unwrap(), Bytes::new());
        assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 0, 8 * 1024 * 1024 + 1).await.unwrap_err(), FsError::InvalidArgument);
        assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, u64::MAX, 1).await.unwrap_err(), FsError::InvalidArgument);
        assert_eq!(f.body("file").await, Some(Bytes::from_static(b"remote two is newer")));
        assert_eq!(f.remote.list_folder("drv", "").await.unwrap().entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["file"],
            "conflicts remain local rather than publishing another visible object");
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = existing(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.put("file", b"competing").await;
    let remote = f.remote.head_object("drv", "file", Default::default()).await.unwrap();
    f.proxy.arm_content("file", 403);
    f.publish().await;
    let conflict = ns.conflict(ino).await.unwrap().unwrap();
    assert!(conflict.local_retained && !conflict.remote_retained && conflict.error.is_some(), "an inaccessible competing body is explicit without discarding available metadata");
    assert_eq!(conflict.remote.as_ref().unwrap().object_id, remote.object_id);
    assert_eq!(conflict.remote_attrs.as_ref().unwrap().version_id.as_deref(), Some(remote.version_id.as_str()));
    assert_eq!(ns.read_conflict(ino, ConflictSide::Local, 0, 1024).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"));
    assert!(matches!(ns.read_conflict(ino, ConflictSide::Remote, 0, 1024).await, Err(FsError::Io(_))));
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"competing")));
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absence_conflicts_preserve_competing_files_and_folders() {
    for (folder, remote_folder) in [(false, false), (false, true), (true, false), (true, true)] {
        let f = Fixture::new(Backend::Api).await;
        let ns = f.session().await;
        let local = if folder { ns.mkdir(ns.root(), "new", 0o700).await.unwrap() } else { ns.create(ns.root(), "new", 0o600).await.unwrap() };
        let key = if remote_folder { "new/" } else { "new" };
        let fh = if folder { None } else { Some(ns.open(local.ino, true).await.unwrap()) };
        if let Some(fh) = fh { ns.write(fh, 0, Bytes::from_static(b"local")).await.unwrap(); ns.fsync(fh).await.unwrap(); }
        let child = if folder {
            let child = ns.create(local.ino, "child", 0o600).await.unwrap();
            let fh = ns.open(child.ino, true).await.unwrap();
            ns.write(fh, 0, Bytes::from_static(b"local child")).await.unwrap();
            ns.fsync(fh).await.unwrap();
            Some((child.ino, fh))
        } else { None };
        f.put(key, if remote_folder { b"" } else { b"competitor" }).await;
        if folder && remote_folder { f.put("new/child", b"remote child").await; }
        let competitor = f.remote.head_object("drv", key, Default::default()).await.unwrap();
        f.publish().await;
        assert_eq!(ns.lookup(ns.root(), "new").await.unwrap().sync, Sync::Conflict);
        let conflict = ns.conflict(local.ino).await.unwrap().unwrap();
        assert!(conflict.base_version.is_none());
        assert_eq!(conflict.remote.unwrap().object_id, competitor.object_id);
        assert_eq!(f.remote.head_object("drv", key, Default::default()).await.unwrap().version_id, competitor.version_id);
        assert!(f.queue.status().await.unwrap().items.iter().any(|item| item.state == State::Failed));
        let (f, ns) = if let Some((ino, fh)) = child {
            assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict, "a blocked child reports the unresolved folder guard");
            let owner = ns.conflict(ino).await.unwrap().unwrap();
            assert_eq!(owner.ino, local.ino, "the child resolves to the folder's direct conflict instead of inventing a competing child identity");
            assert_eq!(owner.remote.unwrap().kind, if remote_folder { Kind::Folder } else { Kind::File });
            assert_eq!(ns.read_conflict(ino, ConflictSide::Local, 0, 1024).await.unwrap_err(), FsError::IsDir);
            if remote_folder { assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 0, 1024).await.unwrap_err(), FsError::IsDir); }
            else { assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 0, 1024).await.unwrap(), Bytes::from_static(b"competitor")); }
            assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"local child"));
            let competing_child = if remote_folder { Some(Bytes::from_static(b"remote child")) } else { None };
            assert_eq!(f.body("new/child").await, competing_child);
            assert!(f.queue.status().await.unwrap().items.iter().filter(|item| item.key == "new/child").all(|item| item.state == State::Queued));
            let late = ns.create(local.ino, "late", 0o600).await.unwrap();
            assert_eq!(late.sync, Sync::Conflict, "new descendants inherit an existing folder conflict before publication can start");
            assert_eq!(ns.getattr(late.ino).await.unwrap().sync, Sync::Conflict);
            assert_eq!(ns.conflict(late.ino).await.unwrap().unwrap().ino, local.ino);
            ns.rename(local.ino, "child", ns.root(), "escaped", RenameMode::Exclusive).await.unwrap();
            ns.close(fh).await.unwrap();
            f.queue.cancel(Scope::Entry(owner.entry_id)).await.unwrap();
            f.queue.clear_finished().await.unwrap();
            f.publish().await;
            assert_eq!(f.body("escaped").await, None, "moving a blocked child cannot evade its historical folder guard");
            assert_eq!(f.body("new/late").await, None);
            let (f, ns) = f.restart(ns).await;
            assert_eq!(ns.lookup(ns.root(), "escaped").await.unwrap().ino, ino);
            assert_eq!(ns.conflict(ino).await.unwrap().unwrap().ino, local.ino);
            assert_eq!(ns.conflict(late.ino).await.unwrap().unwrap().ino, local.ino);
            let fh = ns.open(ino, true).await.unwrap();
            ns.write(fh, 0, Bytes::from_static(b"later")).await.unwrap();
            ns.fsync(fh).await.unwrap();
            f.publish().await;
            assert_eq!(f.body("escaped").await, None);
            assert_eq!(f.body("new/late").await, None);
            assert_eq!(f.body("new/child").await, competing_child);
            assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
            ns.close(fh).await.unwrap();
            (f, ns)
        } else { (f, ns) };
        if let Some(fh) = fh {
            assert_eq!(ns.read_conflict(local.ino, ConflictSide::Local, 0, 1024).await.unwrap(), Bytes::from_static(b"local"));
            if remote_folder { assert_eq!(ns.read_conflict(local.ino, ConflictSide::Remote, 0, 1024).await.unwrap_err(), FsError::IsDir); }
            else { assert_eq!(ns.read_conflict(local.ino, ConflictSide::Remote, 0, 1024).await.unwrap(), Bytes::from_static(b"competitor")); }
            ns.close(fh).await.unwrap();
        }
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metadata_delete_and_rename_guard_failures_are_conflicts() {
    for operation in ["attrs", "delete", "rename-source", "rename-destination", "replace-destination"] {
        let f = Fixture::new(Backend::Api).await;
        f.put("file", b"original").await;
        if operation == "replace-destination" { f.put("target", b"old target").await; }
        let ns = f.session().await;
        let original = ns.lookup(ns.root(), "file").await.unwrap();
        let conflict_ino = if operation == "replace-destination" { ns.lookup(ns.root(), "target").await.unwrap().ino } else { original.ino };
        match operation {
            "attrs" => ns.setxattr(original.ino, "user.tag", b"local", XattrMode::Set).await.unwrap(),
            "delete" => ns.unlink(ns.root(), "file").await.unwrap(),
            "replace-destination" => ns.rename(ns.root(), "file", ns.root(), "target", RenameMode::Replace).await.unwrap(),
            _ => ns.rename(ns.root(), "file", ns.root(), "target", RenameMode::Exclusive).await.unwrap(),
        }
        let competing_key = if operation.ends_with("destination") { "target" } else { "file" };
        f.put(competing_key, b"competing").await;
        if operation == "attrs" {
            f.remote.set_attributes("drv", "file", AttributesUpdate {
                set_xattrs: BTreeMap::from([("user.tag".into(), Bytes::from_static(b"remote first")), ("user.remote".into(), Bytes::from_static(&[0, 255]))]),
                flags: Some(vec!["hidden".into()]), ..Default::default()
            }, Default::default()).await.unwrap();
        }
        let competitor = f.remote.head_object("drv", competing_key, Default::default()).await.unwrap();
        f.publish().await;
        let conflict = ns.conflict(conflict_ino).await.unwrap().expect(operation);
        assert_eq!(conflict.remote.as_ref().unwrap().version_id.as_deref(), Some(competitor.version_id.as_str()), "{operation}");
        assert_eq!(f.remote.head_object("drv", competing_key, Default::default()).await.unwrap().version_id, competitor.version_id, "{operation}");
        assert_eq!(f.body(competing_key).await, Some(Bytes::from_static(b"competing")));
        if operation == "attrs" {
            assert_eq!(ns.getxattr(original.ino, "user.tag").await.unwrap(), b"local");
            assert_eq!(conflict.local_xattrs.as_ref().unwrap().get("user.tag").map(Vec::as_slice), Some(b"local".as_slice()));
            let remote_attrs = conflict.remote_attrs.as_ref().unwrap();
            assert_eq!(remote_attrs.version_id.as_deref(), Some(competitor.version_id.as_str()));
            assert_eq!(STANDARD.decode(remote_attrs.xattrs.get("user.tag").unwrap()).unwrap(), b"remote first");
            assert_eq!(STANDARD.decode(remote_attrs.xattrs.get("user.remote").unwrap()).unwrap(), [0, 255]);
            assert_eq!(remote_attrs.flags, ["hidden"]);
            ns.setxattr(original.ino, "user.tag", b"later local", XattrMode::Replace).await.unwrap();
            f.remote.set_attributes("drv", "file", AttributesUpdate {
                set_xattrs: BTreeMap::from([("user.tag".into(), Bytes::from_static(b"later remote"))]), flags: Some(vec![]), ..Default::default()
            }, Default::default()).await.unwrap();
            let retained = ns.conflict(original.ino).await.unwrap().unwrap();
            assert_eq!(retained.local_xattrs.as_ref().unwrap().get("user.tag").map(Vec::as_slice), Some(b"local".as_slice()));
            assert_eq!(retained.remote_attrs, conflict.remote_attrs, "later edits cannot mutate the retained complete competing metadata");
        }
        if operation.ends_with("destination") { assert_eq!(f.body("file").await, Some(Bytes::from_static(b"original"))); }
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conflict_edits_queue_controls_and_restart_never_remove_the_guard() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = existing(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.put("file", b"competing").await;
    f.publish().await;
    let first = ns.conflict(ino).await.unwrap().unwrap();
    ns.write(fh, 0, Bytes::from_static(b"LATER")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    f.publish().await;
    f.queue.cancel(Scope::Entry(first.entry_id)).await.unwrap();
    f.queue.clear_finished().await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"FINAL")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"competing")));
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    assert_eq!(ns.read_conflict(ino, ConflictSide::Local, 0, 1024).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"));
    ns.close(fh).await.unwrap();
    let (f, ns) = f.restart(ns).await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    assert_eq!(ns.conflict(ino).await.unwrap().unwrap().entry_id, first.entry_id);
    assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 0, 1024).await.unwrap(), Bytes::from_static(b"competing"));
    let fh = ns.open(ino, true).await.unwrap();
    assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"FINAL56789ab"));
    ns.write(fh, 0, Bytes::from_static(b"AGAIN")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"competing")));
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removal_the_feed_reports_first_keeps_unpublished_edits_as_a_conflict() {
    for (flushed, nested) in [(false, false), (true, false), (false, true), (true, true)] {
        let f = Fixture::new(Backend::Api).await;
        let ns = f.session().await;
        let key = if nested { "dir/file" } else { "file" };
        f.put(key, b"0123456789ab").await;
        let parent = if nested { ns.lookup(ns.root(), "dir").await.unwrap().ino } else { ns.root() };
        let ino = ns.lookup(parent, "file").await.unwrap().ino;
        let fh = ns.open(ino, true).await.unwrap();
        ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
        if flushed { ns.fsync(fh).await.unwrap(); }
        // Another Mac removes it, and the feed reports that before this edit publishes.
        f.remote.delete_object("drv", key, Default::default()).await.unwrap();
        if nested { f.remote.delete_object("drv", "dir/", Default::default()).await.unwrap(); }
        ns.invalidate(&[Invalidation::All]).await.unwrap();
        let listed = ns.readdir(ns.root(), None, 10).await.unwrap().into_iter().map(|(name, _)| name).collect::<Vec<_>>();
        assert_eq!(listed, [if nested { "dir" } else { "file" }], "flushed {flushed}, nested {nested}: unpublished edits keep their path");
        if nested { assert_eq!(ns.readdir(parent, None, 10).await.unwrap().into_iter().map(|(name, _)| name).collect::<Vec<_>>(), ["file"]); }
        assert_eq!(ns.read(fh, 0, 64).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"));
        ns.close(fh).await.unwrap();
        f.publish().await;
        assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict, "flushed {flushed}, nested {nested}");
        let conflict = ns.conflict(ino).await.unwrap().unwrap();
        assert!(conflict.remote_missing, "{conflict:?}");
        assert_eq!(ns.read_conflict(ino, ConflictSide::Local, 0, 64).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"));
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_remote_objects_record_absence_without_losing_local_bytes() {
    for (renamed, truncated) in [(false, false), (true, false), (false, true), (true, true)] {
        let f = Fixture::new(Backend::Api).await;
        let ns = f.session().await;
        let (ino, fh) = existing(&f, &ns).await;
        if truncated { ns.truncate(fh, 7).await.unwrap(); }
        else { ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap(); }
        ns.fsync(fh).await.unwrap();
        if renamed { f.remote.rename("drv", "file", "elsewhere", Default::default()).await.unwrap(); }
        else { f.remote.delete_object("drv", "file", Default::default()).await.unwrap(); }
        f.publish().await;
        let conflict = ns.conflict(ino).await.unwrap().unwrap();
        assert!(conflict.remote_missing && conflict.remote.is_none());
        assert!(conflict.local_retained, "a renamed immutable base can still be located by its object identity");
        assert!(conflict.error.is_none());
        assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
        assert_eq!(ns.read_conflict(ino, ConflictSide::Local, 0, 1024).await.unwrap(), Bytes::from_static(if truncated { b"0123456" } else { b"LOCAL56789ab" }));
        assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 0, 1).await.unwrap_err(), FsError::NotFound);
        assert_eq!(f.body("file").await, None);
        if renamed { assert_eq!(f.body("elsewhere").await, Some(Bytes::from_static(b"0123456789ab"))); }
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_and_snapshot_errors_keep_data_and_retry_without_republishing() {
    for failure in 0..5 {
        let snapshot = failure != 0;
        let f = Fixture::new(Backend::Api).await;
        let ns = f.session().await;
        let (ino, fh) = existing(&f, &ns).await;
        ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
        ns.fsync(fh).await.unwrap();
        match failure {
            0 => f.proxy.arm("POST", "file", "x-voidfs-patch", Action::Status(403)),
            1 => f.proxy.arm("HEAD", "file", "versionId=", Action::OmitIdentity),
            2 => f.proxy.arm("GET", "file", "x-voidfs-attrs", Action::InvalidMode),
            3 => f.proxy.arm("GET", "file", "x-voidfs-attrs", Action::InvalidXattrs),
            _ => f.proxy.arm("*", "file", "versionId=", Action::WrongKind),
        }
        f.publish().await;
        assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Error);
        assert!(ns.conflict(ino).await.unwrap().is_none());
        assert_eq!(ns.read(fh, 0, u64::MAX).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"));
        assert_eq!(f.body("file").await, Some(Bytes::from_static(if snapshot { b"LOCAL56789ab" } else { b"0123456789ab" })));
        let versions = f.remote.list_versions("drv", "file", false).await.unwrap().len();
        f.publish().await;
        assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Saved);
        assert_eq!(f.body("file").await, Some(Bytes::from_static(b"LOCAL56789ab")));
        if snapshot { assert_eq!(f.remote.list_versions("drv", "file", false).await.unwrap().len(), versions, "reconciliation retries the acknowledged version without writing another data version"); }
        ns.close(fh).await.unwrap();
        f.queue.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delayed_xattr_ack_preserves_the_later_complete_map_until_saved() {
    let f = Fixture::new(Backend::Api).await;
    f.put("file", b"original").await;
    let ns = f.session().await;
    let ino = ns.lookup(ns.root(), "file").await.unwrap().ino;
    ns.setxattr(ino, "user.tag", b"first", XattrMode::Set).await.unwrap();
    f.proxy.arm("POST", "file", "x-voidfs-attrs", Action::Hold);
    f.queue.resume(Scope::All).await.unwrap();
    f.proxy.committed().await;
    ns.setxattr(ino, "user.tag", b"second", XattrMode::Replace).await.unwrap();
    ns.setxattr(ino, "user.binary", &[0, 255], XattrMode::Set).await.unwrap();
    f.proxy.release();
    f.publish().await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Saved);
    assert_eq!(ns.getxattr(ino, "user.tag").await.unwrap(), b"second");
    assert_eq!(ns.getxattr(ino, "user.binary").await.unwrap(), [0, 255]);
    f.remote.set_attributes("drv", "file", AttributesUpdate { set_xattrs: BTreeMap::from([("user.tag".into(), Bytes::from_static(b"external"))]), ..Default::default() }, Default::default()).await.unwrap();
    ns.invalidate(&[Invalidation::Object("file".into())]).await.unwrap();
    assert_eq!(ns.getxattr(ino, "user.tag").await.unwrap(), b"external", "the complete map becomes clean after its last guarded acknowledgement");
    assert!(!f.db().query_row("SELECT dirty FROM mount_xattrs WHERE ino=?1", [ino], |r| r.get::<_, bool>(0)).unwrap());
    f.queue.close().await;
}
