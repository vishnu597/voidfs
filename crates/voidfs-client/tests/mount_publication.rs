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
    Invalidation, Queue, QueueConfig, Scope, State, Store, invalidations};
use voidfs_sdk::{AttributesUpdate, Change, Client, Config, Kind, PutOptions, ReadOptions};
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
        // As a restarted process waits for the old one to exit: a disk flush may still hold it.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let store = loop {
            match Store::open(state.path()) {
                Err(voidfs_client::Error::Locked(_)) if std::time::Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(5)).await,
                store => break Arc::new(store.unwrap()),
            }
        };
        let cache = cache(store.clone(), client.clone(), connectivity.clone(), backend).await;
        let queue = Queue::open(store.clone(), client.clone(), queue_config(&connectivity)).await.unwrap();
        let f = Self { _server, proxy, remote, client, state, store, cache, queue, connectivity, backend };
        let ns = f.session().await;
        (f, ns)
    }
}

async fn existing(f: &Fixture, ns: &Session) -> (u64, u64) {
    f.put("file", b"0123456789ab").await;
    ns.invalidate(&[Invalidation::All]).await.unwrap();
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

fn change(op: &str, key: &str, from_key: Option<&str>, object_id: &str, version_id: &str) -> Change {
    Change { op: op.into(), key: key.into(), from_key: from_key.map(Into::into), object_id: object_id.into(), version_id: version_id.into(),
        kind: Kind::File, seq: None, time: None }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_directory_refresh_before_the_feed_does_not_hide_remote_changes_from_observers() {
    let f = Fixture::new(Backend::Api).await;
    f.put("folder/", b"").await;
    let ns = f.session().await;
    for prefix in ["", "folder/"] {
        let parent = if prefix.is_empty() { ns.root() } else { ns.lookup(ns.root(), "folder").await.unwrap().ino };
        assert_eq!(ns.lookup(parent, "remote").await.unwrap_err(), FsError::NotFound);
        let mut since = f.remote.changes("drv", 0, Duration::ZERO).await.unwrap().seq;
        for operation in ["create", "overwrite", "rename", "delete"] {
            let key = format!("{prefix}remote");
            let renamed = format!("{prefix}renamed");
            match operation {
                "create" => f.put(&key, b"created").await,
                "overwrite" => f.put(&key, b"updated").await,
                "rename" => { f.remote.rename("drv", &key, &renamed, Default::default()).await.unwrap(); },
                "delete" => { f.remote.delete_object("drv", &renamed, Default::default()).await.unwrap(); },
                _ => unreachable!(),
            }
            // The feed is deliberately not applied until a separate metadata refresh has
            // learned the change. An adapter can still hold older positive or negative names.
            ns.invalidate(&[Invalidation::All]).await.unwrap();
            let listing = ns.readdir(parent, None, 10).await.unwrap();
            let expected = match operation { "rename" => "renamed", _ => "remote" };
            if operation == "delete" {
                assert!(!listing.iter().any(|(name, _)| name == "renamed"));
            } else {
                assert!(listing.iter().any(|(name, _)| name == expected));
            }
            let batch = f.remote.changes("drv", since, Duration::ZERO).await.unwrap();
            assert_eq!(batch.changes.len(), 1, "{prefix}{operation}");
            let (reported, inodes) = ns.invalidate_feed(&batch.changes).await.unwrap();
            assert_eq!(reported, invalidations(&batch.changes), "observers still need {prefix}{operation}");
            assert!(inodes.contains(&parent), "the containing directory must be invalidated: {inodes:?}");
            since = batch.seq;
        }
    }
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_feed_reports_other_computers_changes_and_not_this_macs_own() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    f.put("saved", b"saved").await;
    f.put("from", b"from").await;
    f.put("dir/inside", b"inside").await;
    let since = f.remote.changes("drv", 0, Duration::ZERO).await.unwrap().seq;
    let saved = ns.lookup(ns.root(), "saved").await.unwrap().ino;
    ns.lookup(ns.root(), "from").await.unwrap();
    let dir = ns.lookup(ns.root(), "dir").await.unwrap();
    let inside = ns.lookup(dir.ino, "inside").await.unwrap();
    // This Mac's own publications coming back: a new file, an edit and a rename.
    let created = ns.create(ns.root(), "own", 0o600).await.unwrap();
    let fh = ns.open(created.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"own")).await.unwrap();
    ns.close(fh).await.unwrap();
    let fh = ns.open(saved, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"edit")).await.unwrap();
    ns.close(fh).await.unwrap();
    ns.rename(ns.root(), "from", ns.root(), "to", RenameMode::Exclusive).await.unwrap();
    f.publish().await;
    // A second edit: the first's version is no longer the one the inode records.
    let fh = ns.open(saved, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"again")).await.unwrap();
    ns.close(fh).await.unwrap();
    f.publish().await;
    let own = f.remote.changes("drv", since, Duration::ZERO).await.unwrap();
    assert!(own.changes.iter().filter(|c| c.key == "saved").count() >= 2, "{:?}", own.changes);
    let ops = own.changes.iter().map(|c| (c.op.as_str(), c.key.as_str())).collect::<Vec<_>>();
    assert!(["own", "saved", "to"].iter().all(|key| ops.iter().any(|(_, k)| k == key)) && ops.iter().any(|(op, _)| *op == "rename"), "{ops:?}");
    let generation = |ino| f.db().query_row("SELECT generation FROM mount_inodes WHERE ino=?1", [ino], |r| r.get::<_, i64>(0)).unwrap();
    let before = generation(saved);
    assert_eq!(ns.invalidate_feed(&own.changes).await.unwrap(), (Vec::new(), Vec::new()), "this Mac's own publications are not news");
    assert!(generation(saved) > before, "the session still invalidates what its own publications changed");

    // Other computers' changes: an edit, a new name, a rename and a delete.
    f.put("to", b"elsewhere").await;
    f.put("other", b"other").await;
    f.remote.rename("drv", "own", "renamed", Default::default()).await.unwrap();
    f.remote.delete_object("drv", "saved", Default::default()).await.unwrap();
    let foreign = f.remote.changes("drv", own.seq, Duration::ZERO).await.unwrap();
    assert_eq!(foreign.changes.len(), 4);
    let (reported, inodes) = ns.invalidate_feed(&foreign.changes).await.unwrap();
    assert_eq!(reported, invalidations(&foreign.changes));
    assert!(inodes.contains(&ns.root()) && inodes.contains(&saved) && inodes.contains(&created.ino), "{inodes:?}");

    // Unpublished edits and renames keep the local view until publication settles them.
    // Changes to saved or absent names still need to reach adapters, which may cache an older
    // view even after a listing refreshes the core.
    f.queue.pause(Scope::All).await.unwrap();
    let to = ns.lookup(ns.root(), "to").await.unwrap();
    let other = ns.lookup(ns.root(), "other").await.unwrap();
    let fh = ns.open(to.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"pending")).await.unwrap();
    ns.rename(ns.root(), "renamed", ns.root(), "moved", RenameMode::Exclusive).await.unwrap();
    ns.rename(ns.root(), "dir", ns.root(), "dir2", RenameMode::Exclusive).await.unwrap();
    let shown = [
        change("put", "to", None, to.object_id.as_deref().unwrap(), "remote"),
        change("rename", "moved", Some("renamed"), "created", "remote"),
    ];
    for one in &shown {
        assert_eq!(ns.invalidate_feed(std::slice::from_ref(one)).await.unwrap(), (Vec::new(), Vec::new()), "{one:?}");
    }
    let news = [
        change("put", "other", None, other.object_id.as_deref().unwrap(), "remote"),
        change("rename", "to", Some("other"), other.object_id.as_deref().unwrap(), "remote"),
        change("put", "never", None, "missing", "remote"),
        change("delete", "never", None, "missing", "remote"),
        // Inside a folder this Mac moved and hasn't published: the local view shows it elsewhere.
        change("put", "dir/inside", None, inside.object_id.as_deref().unwrap(), "remote"),
    ];
    for one in &news {
        let (reported, inodes) = ns.invalidate_feed(std::slice::from_ref(one)).await.unwrap();
        assert_eq!(reported, invalidations(std::slice::from_ref(one)), "{one:?}");
        assert!(!inodes.is_empty(), "{one:?}");
    }
    ns.close(fh).await.unwrap();
    f.queue.close().await;
}

async fn history(f: &Fixture, key: &str) -> Vec<String> {
    f.remote.list_versions("drv", key, true).await.unwrap().into_iter().map(|v| v.operation).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_files_bytes_mode_times_and_xattrs_go_up_with_its_creating_put() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    f.queue.resume(Scope::All).await.unwrap();
    // As Finder copies a file: create, provenance, bytes, mode and dates, close.
    let mtime = std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    let created = ns.create(ns.root(), "copied", 0o600).await.unwrap();
    ns.setxattr(created.ino, "com.apple.provenance", &[1, 2, 3], XattrMode::Set).await.unwrap();
    let fh = ns.open(created.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"copied bytes")).await.unwrap();
    ns.setxattr(created.ino, "user.empty", &[], XattrMode::Set).await.unwrap();
    ns.setattr(created.ino, Some(0o640), Some(mtime)).await.unwrap();
    let (normal, full) = ns.staging_commits();
    ns.close(fh).await.unwrap();
    assert_eq!(ns.staging_commits(), (normal, full + 1), "the close is one FULL staging commit");
    let entries: i64 = f.db().query_row("SELECT count(*) FROM entries WHERE key='copied'", [], |r| r.get(0)).unwrap();
    assert_eq!(entries, 1, "the create, xattrs, mode, dates and bytes are one journal entry");
    tokio::time::timeout(Duration::from_secs(10), f.queue.settle()).await.expect("publication settles");
    // A put can't carry xattrs: they are the version after it (protocol §4.8).
    assert_eq!(history(&f, "copied").await, ["put", "attrs"]);
    assert_eq!(f.body("copied").await, Some(Bytes::from_static(b"copied bytes")));
    let saved = ns.lookup(ns.root(), "copied").await.unwrap();
    assert_eq!((saved.sync, saved.mode, saved.mtime, saved.size), (Sync::Saved, 0o640, mtime, 12));
    assert_eq!(ns.getxattr(saved.ino, "com.apple.provenance").await.unwrap(), [1, 2, 3]);
    assert_eq!(ns.getxattr(saved.ino, "user.empty").await.unwrap(), Vec::<u8>::new());
    let attrs = f.remote.attributes("drv", "copied", ReadOptions::default()).await.unwrap();
    assert_eq!(attrs.xattrs.keys().collect::<Vec<_>>(), ["com.apple.provenance", "user.empty"]);
    // Without xattrs, one version.
    let plain = ns.create(ns.root(), "plain", 0o644).await.unwrap();
    let fh = ns.open(plain.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"plain")).await.unwrap();
    ns.setattr(plain.ino, None, Some(mtime)).await.unwrap();
    ns.close(fh).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), f.queue.settle()).await.expect("publication settles");
    assert_eq!(history(&f, "plain").await, ["put"]);
    assert_eq!(ns.lookup(ns.root(), "plain").await.unwrap().mtime, mtime);
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_file_written_past_the_hold_publishes_its_create_first() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    f.queue.resume(Scope::All).await.unwrap();
    let created = ns.create(ns.root(), "slow", 0o644).await.unwrap();
    let fh = ns.open(created.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
    let hold = QueueConfig::default().create_hold;
    assert_eq!(hold, Duration::from_secs(1));
    tokio::time::timeout(hold * 5, async { while f.body("slow").await.is_none() { tokio::time::sleep(Duration::from_millis(20)).await; } })
        .await.expect("the create goes once the hold ends");
    ns.write(fh, 5, Bytes::from_static(b" second")).await.unwrap();
    ns.close(fh).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), f.queue.settle()).await.expect("publication settles");
    assert_eq!(history(&f, "slow").await, ["put", "put"]);
    assert_eq!(f.body("slow").await, Some(Bytes::from_static(b"first second")));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_sets_its_mode_and_times_in_its_own_request() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = existing(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"AB")).await.unwrap();
    let mtime = ns.handle_attr(fh).unwrap().mtime;
    ns.close(fh).await.unwrap();
    f.publish().await;
    assert_eq!(history(&f, "file").await, ["put", "write"], "no attributes version after the edit");
    let saved = ns.getattr(ino).await.unwrap();
    assert_eq!((saved.sync, saved.mtime, saved.mode), (Sync::Saved, mtime, 0o640));
    let attrs = f.remote.attributes("drv", "file", ReadOptions::default()).await.unwrap();
    assert_eq!(attrs.mode.as_deref(), Some("0640"));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rewrite_in_place_is_one_version() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = existing(&f, &ns).await;
    // As O_TRUNC and a save in place: truncate to nothing, then the new bytes.
    ns.truncate(fh, 0).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"new")).await.unwrap();
    ns.close(fh).await.unwrap();
    f.publish().await;
    assert_eq!(history(&f, "file").await, ["put", "write"], "no empty version before the new bytes");
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"new")));
    // The truncate stays when the writes leave bytes it cleared.
    let fh = ns.open(ino, true).await.unwrap();
    ns.truncate(fh, 0).await.unwrap();
    ns.write(fh, 4, Bytes::from_static(b"gap")).await.unwrap();
    ns.close(fh).await.unwrap();
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"\0\0\0\0gap")));
    assert_eq!(ns.read(ns.open(ino, false).await.unwrap(), 0, 64).await.unwrap(), Bytes::from_static(b"\0\0\0\0gap"));
    f.queue.close().await;
}

async fn names(ns: &Session, dir: u64) -> Vec<String> {
    ns.readdir(dir, None, 100).await.unwrap().into_iter().map(|(name, _)| name).collect()
}

async fn write_new(ns: &Session, parent: u64, name: &str, body: &'static [u8]) -> u64 {
    let created = ns.create(parent, name, 0o644).await.unwrap();
    let fh = ns.open(created.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(body)).await.unwrap();
    ns.close(fh).await.unwrap();
    created.ino
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ds_store_stays_on_this_mac_across_a_restart_and_nearby_names_publish() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    // As Finder writes one: create, FinderInfo, truncate, bytes, close.
    let created = ns.create(ns.root(), ".DS_Store", 0o644).await.unwrap();
    assert_eq!(created.sync, Sync::Local);
    ns.setxattr(created.ino, "com.apple.FinderInfo", &[0; 32], XattrMode::Set).await.unwrap();
    let fh = ns.open(created.ino, true).await.unwrap();
    ns.truncate(fh, 0).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"Bud1 view")).await.unwrap();
    ns.setattr(created.ino, Some(0o600), None).await.unwrap();
    ns.close(fh).await.unwrap();
    let dir = ns.mkdir(ns.root(), "dir", 0o755).await.unwrap().ino;
    write_new(&ns, dir, ".DS_Store", b"Bud1 dir").await;
    // Names near it are the user's own and publish.
    let nearby = ["DS_Store", ".DS_Store.txt", "._notes", ".ds_store", ".localized"];
    for name in nearby { write_new(&ns, ns.root(), name, b"mine").await; }
    f.queue.resume(Scope::All).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), f.queue.settle()).await.expect("publication settles");
    let journaled: i64 = f.db().query_row("SELECT count(*) FROM entries WHERE key GLOB '*.DS_Store'", [], |r| r.get(0)).unwrap();
    assert_eq!(journaled, 0, "nothing about a .DS_Store is journaled");
    assert_eq!((f.body(".DS_Store").await, f.body("dir/.DS_Store").await), (None, None));
    for name in nearby { assert_eq!(f.body(name).await, Some(Bytes::from_static(b"mine")), "{name}"); }
    let (f, ns) = f.restart(ns).await;
    tokio::time::timeout(Duration::from_secs(10), f.queue.settle()).await.expect("recovery settles");
    assert_eq!(f.body(".DS_Store").await, None, "recovery doesn't publish it either");
    let saved = ns.lookup(ns.root(), ".DS_Store").await.unwrap();
    assert_eq!((saved.ino, saved.sync, saved.mode, saved.size), (created.ino, Sync::Local, 0o600, 9));
    assert_eq!(ns.read(ns.open(saved.ino, false).await.unwrap(), 0, 64).await.unwrap(), Bytes::from_static(b"Bud1 view"));
    assert_eq!(ns.getxattr(saved.ino, "com.apple.FinderInfo").await.unwrap(), [0; 32]);
    assert!(names(&ns, ns.root()).await.contains(&".DS_Store".to_owned()));
    let inner = ns.lookup(dir, ".DS_Store").await.unwrap();
    assert_eq!(ns.read(ns.open(inner.ino, false).await.unwrap(), 0, 64).await.unwrap(), Bytes::from_static(b"Bud1 dir"));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_local_ds_store_renames_only_as_one_and_shadows_the_drives_own() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    write_new(&ns, ns.root(), ".DS_Store", b"local").await;
    write_new(&ns, ns.root(), "plain", b"plain").await;
    let empty = ns.mkdir(ns.root(), "empty", 0o755).await.unwrap().ino;
    // These local-only renames do not test racing the parent's initial publication.
    f.publish().await;
    // Crossing between a local-only file and a published name is EXDEV: `mv` copies instead.
    assert_eq!(ns.rename(ns.root(), ".DS_Store", ns.root(), "named", RenameMode::Replace).await.unwrap_err(), FsError::CrossDevice);
    assert_eq!(ns.rename(ns.root(), "plain", empty, ".DS_Store", RenameMode::Replace).await.unwrap_err(), FsError::CrossDevice);
    assert_eq!(FsError::CrossDevice.errno(), libc::EXDEV);
    // Into another folder, it stays local.
    ns.rename(ns.root(), ".DS_Store", empty, ".DS_Store", RenameMode::Replace).await.unwrap();
    ns.rename(empty, ".DS_Store", ns.root(), ".DS_Store", RenameMode::Replace).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), f.queue.settle()).await.expect("publication settles");
    assert_eq!(f.body(".DS_Store").await, None);
    assert_eq!(names(&ns, empty).await, Vec::<String>::new());
    // One another computer puts in the drive stays behind this Mac's until it goes.
    f.put(".DS_Store", b"theirs").await;
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    let local = ns.lookup(ns.root(), ".DS_Store").await.unwrap();
    assert_eq!(local.sync, Sync::Local);
    assert_eq!(ns.read(ns.open(local.ino, false).await.unwrap(), 0, 64).await.unwrap(), Bytes::from_static(b"local"));
    ns.unlink(ns.root(), ".DS_Store").await.unwrap();
    let theirs = ns.lookup(ns.root(), ".DS_Store").await.unwrap();
    assert_eq!(theirs.sync, Sync::Saved);
    assert_eq!(ns.read(ns.open(theirs.ino, false).await.unwrap(), 0, 64).await.unwrap(), Bytes::from_static(b"theirs"));
    // The drive's own is an ordinary file: a create can't take its name, and an edit publishes.
    assert_eq!(ns.create(ns.root(), ".DS_Store", 0o644).await.unwrap_err(), FsError::Exists);
    let fh = ns.open(theirs.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"THEIRS")).await.unwrap();
    ns.close(fh).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), f.queue.settle()).await.expect("publication settles");
    assert_eq!(f.body(".DS_Store").await, Some(Bytes::from_static(b"THEIRS")));
    let journaled: i64 = f.db().query_row("SELECT count(*) FROM entries WHERE key='.DS_Store' AND op='delete'", [], |r| r.get(0)).unwrap();
    assert_eq!(journaled, 0, "removing the local one deleted nothing in the drive");
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folder_removed_elsewhere_goes_with_its_local_ds_store() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    f.put("dir/file", b"file").await;
    let dir = ns.lookup(ns.root(), "dir").await.unwrap().ino;
    write_new(&ns, dir, ".DS_Store", b"view").await;
    f.remote.delete_object("drv", "dir/file", Default::default()).await.unwrap();
    f.remote.delete_object("drv", "dir/", Default::default()).await.unwrap();
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    assert_eq!(names(&ns, ns.root()).await, Vec::<String>::new(), "only local-only names were left in it");
    f.queue.close().await;
}


fn stage_paths(f: &Fixture) -> Vec<String> {
    f.db().prepare("SELECT path FROM mount_staged").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

fn sources(f: &Fixture) -> Vec<String> {
    f.db().prepare("SELECT source FROM entries WHERE source IS NOT NULL").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

async fn read_all(ns: &Session, ino: u64) -> Bytes {
    let fh = ns.open(ino, false).await.unwrap();
    let bytes = ns.read(fh, 0, 1024).await.unwrap();
    ns.close(fh).await.unwrap();
    bytes
}

async fn kept(f: &Fixture) -> Vec<voidfs_client::Item> {
    f.queue.status().await.unwrap().items.into_iter().filter(|i| i.state == State::Kept).collect()
}

/// Cancelling a mounted file's unpublished change keeps it on this Mac, not uploaded, and the
/// next save of the file uploads the file as it is then, guarded on the version the change was
/// made on (step 5, item 8).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_mount_change_stays_here_not_uploaded_until_the_next_save() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = existing(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    let queued = f.queue.status().await.unwrap().items;
    let (frozen, stage) = (sources(&f), stage_paths(&f));
    f.queue.cancel(Scope::Entry(queued[0].id)).await.unwrap();
    let status = f.queue.status().await.unwrap();
    assert!(status.items.iter().all(|i| i.state == State::Kept && !i.paused), "the change's truncate is kept with it: {:?}", status.items);
    assert_eq!((status.unpublished, status.unpublished_bytes), (0, 0), "a kept change isn't waiting to be sent");
    assert!(Path::new(&frozen[0]).exists(), "the queue keeps its copy of the change");
    assert_eq!(stage_paths(&f), stage);
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Kept);
    assert_eq!(ns.read(fh, 0, 1024).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"));
    ns.close(fh).await.unwrap();
    assert_eq!(read_all(&ns, ino).await, Bytes::from_static(b"LOCAL56789ab"), "the copy here keeps the change");
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"0123456789ab")), "a resume doesn't upload it");
    assert_eq!(kept(&f).await.len(), 2);

    // It stays so through a restart.
    let (f, ns) = f.restart(ns).await;
    f.publish().await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Kept);
    assert_eq!(read_all(&ns, ino).await, Bytes::from_static(b"LOCAL56789ab"));
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"0123456789ab")));
    assert_eq!(kept(&f).await.len(), 2);

    // The next save uploads the file as it is here: the kept change and the new one.
    let fh = ns.open(ino, true).await.unwrap();
    ns.write(fh, 12, Bytes::from_static(b"!")).await.unwrap();
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Pending);
    ns.fsync(fh).await.unwrap();
    ns.close(fh).await.unwrap();
    assert!(kept(&f).await.is_empty(), "the save queued the kept change again");
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"LOCAL56789ab!")));
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Saved);
    assert!(f.queue.status().await.unwrap().items.iter().all(|i| i.state == State::Done));
    // The kept change's truncate ends a run, so the new save is a version of its own after it.
    assert_eq!(f.remote.list_versions("drv", "file", true).await.unwrap().len(), 3, "both made on the version the kept change was made on");
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_change_and_another_computers_save_become_a_conflict_at_the_next_save() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = existing(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    let done = f.queue.cancel(Scope::All).await.unwrap();
    assert_eq!((done.cancelled, done.kept), (0, f.queue.status().await.unwrap().items.len() as u64), "a cancel says what it kept");
    f.put("file", b"from another mac").await;
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    assert_eq!(ns.read(fh, 0, 1024).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"), "this Mac keeps its copy");
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Kept);
    ns.write(fh, 0, Bytes::from_static(b"AGAIN")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.publish().await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"from another mac")));
    assert_eq!(ns.read_conflict(ino, ConflictSide::Local, 0, 1024).await.unwrap(), Bytes::from_static(b"AGAIN56789ab"));
    assert_eq!(ns.read_conflict(ino, ConflictSide::Remote, 0, 1024).await.unwrap(), Bytes::from_static(b"from another mac"));
    // A cancel leaves the conflict as it is: it is no upload, and both versions stay here.
    let before = f.queue.status().await.unwrap().items;
    assert!(before.iter().any(|i| i.state == State::Failed && i.conflict.is_some()), "{before:?}");
    let done = f.queue.cancel(Scope::All).await.unwrap();
    assert_eq!((done.cancelled, done.kept), (0, 0));
    assert_eq!(f.queue.status().await.unwrap().items, before);
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    ns.close(fh).await.unwrap();
    let (f, ns) = f.restart(ns).await;
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Conflict);
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"from another mac")));
    f.queue.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_new_file_uploads_whole_at_its_next_save_and_a_kept_edit_goes_before_its_deletion() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let new = ns.create(ns.root(), "new", 0o600).await.unwrap();
    let fh = ns.open(new.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"fresh")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    f.queue.cancel(Scope::All).await.unwrap();
    f.publish().await;
    assert_eq!(f.body("new").await, None, "a kept creation leaves the drive as it was");
    assert_eq!(ns.getattr(new.ino).await.unwrap().sync, Sync::Kept);
    assert_eq!(ns.lookup(ns.root(), "new").await.unwrap().sync, Sync::Kept);
    assert_eq!(ns.readdir(ns.root(), None, 10).await.unwrap().into_iter().map(|(n, a)| (n, a.sync)).collect::<Vec<_>>(), [("new".to_owned(), Sync::Kept)]);
    ns.write(fh, 5, Bytes::from_static(b"er")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    ns.close(fh).await.unwrap();
    f.publish().await;
    assert_eq!(f.body("new").await, Some(Bytes::from_static(b"fresher")));
    assert_eq!(f.remote.list_versions("drv", "new", true).await.unwrap().len(), 1, "the new save folded into the kept creation: one put");
    assert_eq!(ns.getattr(new.ino).await.unwrap().sync, Sync::Saved);

    // A kept edit, then the file's deletion here: the edit goes first, so the drive's history has it.
    f.queue.pause(Scope::All).await.unwrap();
    f.put("file", b"0123456789ab").await;
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    let ino = ns.lookup(ns.root(), "file").await.unwrap().ino;
    let fh = ns.open(ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    ns.close(fh).await.unwrap();
    f.queue.cancel(Scope::All).await.unwrap();
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Kept);
    ns.unlink(ns.root(), "file").await.unwrap();
    assert!(kept(&f).await.is_empty());
    f.publish().await;
    assert_eq!(f.body("file").await, None);
    let items = f.queue.status().await.unwrap().items.into_iter().filter(|i| i.key == "file").collect::<Vec<_>>();
    assert_eq!(items.iter().map(|i| (i.op, i.state)).collect::<Vec<_>>(),
        [(voidfs_client::Op::Write, State::Done), (voidfs_client::Op::Truncate, State::Done), (voidfs_client::Op::Delete, State::Done)]);
    assert!(items[0].version.is_some(), "the kept edit became a version before the deletion: {items:?}");
    f.queue.close().await;
}
