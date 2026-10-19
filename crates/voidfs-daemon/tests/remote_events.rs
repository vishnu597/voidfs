// SPDX-License-Identifier: Apache-2.0
//! Adapter invalidations survive a directory refresh that overtakes the remote feed.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use bytes::Bytes;
use futures::future::BoxFuture;
use tokio::sync::watch;
use voidfs_client::{Invalidation, Link, mount::FsError};
use voidfs_daemon::api::{Build, NewMount};
use voidfs_daemon::{Adapter, Core, Daemon, DaemonClient, DaemonConfig, MountSpec, Mounted};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

#[derive(Default)]
struct Capture { mounts: Mutex<Vec<Captured>> }
type Captured = (Core, Arc<Mutex<Vec<Invalidation>>>);
struct CaptureMount { seen: Arc<Mutex<Vec<Invalidation>>> }

impl Adapter for Capture {
    fn name(&self) -> &str { "capture" }
    fn mount<'a>(&'a self, _: &'a MountSpec, core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        Box::pin(async move {
            let seen = Arc::new(Mutex::new(Vec::new()));
            self.mounts.lock().unwrap().push((core.clone(), seen.clone()));
            Ok(Arc::new(CaptureMount { seen }) as Arc<dyn Mounted>)
        })
    }
}

impl Mounted for CaptureMount {
    fn invalidate(&self, changes: &[Invalidation]) { self.seen.lock().unwrap().extend_from_slice(changes); }
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> { Box::pin(async { Ok(()) }) }
}

/// A stable endpoint that can refuse connections during bootstrap, then hold only feed bodies.
/// The TCP switch drops offline connections instead of returning an HTTP error: the daemon must
/// restore its remembered drive through the same transport-failure path as an absent service.
struct Proxy {
    endpoint: String,
    online: Arc<AtomicBool>,
    feed_open: watch::Sender<bool>,
    feeds: Arc<AtomicUsize>,
    bridge: tokio::task::JoinHandle<()>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) { self.bridge.abort(); self.server.abort(); }
}

struct Upstream {
    url: String,
    http: reqwest::Client,
    feed_open: watch::Receiver<bool>,
    feeds: Arc<AtomicUsize>,
}

impl Proxy {
    async fn start(upstream: &str) -> Self {
        let online = Arc::new(AtomicBool::new(true));
        let feeds = Arc::new(AtomicUsize::new(0));
        let (feed_open, feed_rx) = watch::channel(false);
        let state = Arc::new(Upstream { url: upstream.into(), http: reqwest::Client::new(), feed_open: feed_rx, feeds: feeds.clone() });
        let inner = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let inner_addr = inner.local_addr().unwrap();
        let app = axum::Router::new().fallback(forward).with_state(state);
        let server = tokio::spawn(async move { let _ = axum::serve(inner, app).await; });
        let outer = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://localhost:{}", outer.local_addr().unwrap().port());
        let reachable = online.clone();
        let bridge = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    incoming = outer.accept() => {
                        let Ok((mut client, _)) = incoming else { return };
                        if !reachable.load(Ordering::SeqCst) { continue; }
                        connections.spawn(async move {
                            if let Ok(mut server) = tokio::net::TcpStream::connect(inner_addr).await {
                                let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                            }
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Self { endpoint, online, feed_open, feeds, bridge, server }
    }
}

async fn forward(State(up): State<Arc<Upstream>>, request: axum::extract::Request) -> Response {
    let (parts, body) = request.into_parts();
    let target = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let feed = target.contains("x-voidfs-changes");
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let mut forwarded = up.http.request(parts.method.clone(), format!("{}{target}", up.url));
    // Preserve Host too, because the signature covers the proxy's endpoint.
    for (name, value) in &parts.headers { forwarded = forwarded.header(name, value); }
    let response = match forwarded.body(body).send().await {
        Ok(response) => response,
        Err(_) => return Response::builder().status(502).body(Body::empty()).unwrap(),
    };
    let mut out = Response::builder().status(response.status().as_u16());
    for (name, value) in response.headers() {
        if !matches!(name.as_str(), "transfer-encoding" | "connection") && (name.as_str() != "content-length" || parts.method == reqwest::Method::HEAD) {
            out = out.header(name, value);
        }
    }
    let gate = feed.then(|| up.feed_open.clone());
    if feed { up.feeds.fetch_add(1, Ordering::SeqCst); }
    out.body(Body::from_stream(futures::stream::unfold((response, gate), |(mut response, mut gate)| async move {
        if let Some(gate) = &mut gate {
            while !*gate.borrow_and_update() {
                if gate.changed().await.is_err() { return None; }
            }
        }
        match response.chunk().await {
            Ok(Some(chunk)) => Some((Ok::<Bytes, std::io::Error>(chunk), (response, gate))),
            _ => None,
        }
    }))).unwrap()
}

fn sdk(endpoint: &str) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: endpoint.replace("127.0.0.1", "localhost"), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(),
        max_attempts: 1, timeout: Duration::from_secs(5), ..Default::default() }
}

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(u64::MAX) }

fn config(endpoint: &str, dir: &Path, adapter: Arc<Capture>) -> DaemonConfig {
    let mut cfg = DaemonConfig::new(dir, sdk(endpoint), Build::default());
    cfg.adapters = vec![adapter];
    cfg.cache = voidfs_client::CacheConfig { min_free_bytes: 0, free_space: Some(plenty), read_ahead_bytes: 0, ..Default::default() };
    cfg.connectivity = voidfs_client::ConnectivityConfig { offline_after: 1, probe_every: Duration::from_secs(3600), probe_max: Duration::from_secs(3600), ..Default::default() };
    cfg
}

async fn until(what: &str, mut ready: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !ready().await { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.unwrap_or_else(|_| panic!("{what}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restored_mount_receives_remote_create_even_when_lookup_refreshes_before_the_feed() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    remote.put_object("drive", "existing", Bytes::from_static(b"saved"), Default::default()).await.unwrap();
    let proxy = Proxy::start(&server.endpoint).await;
    let dir = tempfile::Builder::new().prefix("vremote").tempdir_in(std::env::temp_dir()).unwrap();
    let adapter = Arc::new(Capture::default());
    let cfg = config(&proxy.endpoint, dir.path(), adapter.clone());
    let daemon = Daemon::start(cfg.clone()).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    client.mount(&NewMount { drive: "drive".into(), mountpoint: Some(dir.path().join("mount").display().to_string()), ..Default::default() }).await.unwrap();
    let core = adapter.mounts.lock().unwrap()[0].0.clone();
    let root = core.session.root();
    assert_eq!(core.session.readdir(root, None, 10).await.unwrap()[0].0, "existing");
    assert_eq!(core.session.lookup(root, "created").await, Err(FsError::NotFound));
    drop(core);
    adapter.mounts.lock().unwrap().clear();
    daemon.stop().await;

    // Restart with a remembered mount and a saved namespace, but no service connection.
    proxy.online.store(false, Ordering::SeqCst);
    let daemon = Daemon::start(cfg).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    until("the remembered mount restores offline", async || {
        client.mounts().await.unwrap().mounts.first().is_some_and(|mount| mount.state == "mounted")
    }).await;
    let (core, seen) = adapter.mounts.lock().unwrap()[0].clone();
    until("the failed bootstrap marks the connection offline", async || core.connectivity.link() == Link::Offline).await;
    assert_eq!(core.session.root(), root);
    assert_eq!(core.session.lookup(root, "existing").await.unwrap().size, 5);
    // This is the adapter's negative cache: it must be invalidated even if a later lookup
    // updates the daemon's saved namespace before the adapter receives the change feed.
    assert_eq!(core.session.lookup(root, "created").await, Err(FsError::NotFound));
    seen.lock().unwrap().clear();

    let feeds_before = proxy.feeds.load(Ordering::SeqCst);
    proxy.online.store(true, Ordering::SeqCst);
    until("the feed reconnects while its events remain gated", async || {
        proxy.feeds.load(Ordering::SeqCst) > feeds_before && core.connectivity.link() == Link::Online
    }).await;
    let reader = client.session("drive", true).await.unwrap();
    let mut watch = reader.watch().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(10), watch.next()).await.unwrap().unwrap().unwrap().resync);
    let created = remote.put_object("drive", "created", Bytes::from_static(b"remote"), Default::default()).await.unwrap();
    let seq = remote.list_folder_page("drive", "", None).await.unwrap().seq;
    assert_eq!(core.session.lookup(root, "created").await.unwrap().version_id.as_ref(), Some(&created.version_id));
    assert!(!seen.lock().unwrap().contains(&Invalidation::Object("created".into())), "no feed event is released yet");

    proxy.feed_open.send_replace(true);
    // The RPC event is broadcast after mounted adapters have been notified, so observing its
    // sequence is also a deterministic barrier for the adapter callback (including an empty one).
    tokio::time::timeout(Duration::from_secs(10), async {
        while watch.next().await.unwrap().unwrap().seq < seq {}
    }).await.expect("the daemon consumes and broadcasts the remote creation");
    assert!(seen.lock().unwrap().contains(&Invalidation::Object("created".into())), "the adapter must discard its earlier ENOENT after a foreign creation, even when the daemon already listed that version");

    reader.release().await.unwrap();
    drop(watch);
    drop(core);
    adapter.mounts.lock().unwrap().clear();
    daemon.stop().await;
}
