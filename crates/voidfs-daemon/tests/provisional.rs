// SPDX-License-Identifier: Apache-2.0
//! An offline mount without a known drive ID cannot later publish against a reused alias.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use futures::future::BoxFuture;
use voidfs_client::mount::{FsError, Session};
use voidfs_daemon::api::{Build, NewMount};
use voidfs_daemon::{Adapter, Core, Daemon, DaemonClient, DaemonConfig, FsClientError, MountSpec, Mounted};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

#[derive(Default)]
struct Fake { core: Mutex<Weak<Session>> }
struct FakeMount;

impl Adapter for Fake {
    fn name(&self) -> &str { "fake" }
    fn mount<'a>(&'a self, _spec: &'a MountSpec, core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        *self.core.lock().unwrap() = Arc::downgrade(&core.session);
        Box::pin(async { Ok(Arc::new(FakeMount) as Arc<dyn Mounted>) })
    }
}

impl Mounted for FakeMount {
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> { Box::pin(async { Ok(()) }) }
}

fn sdk(endpoint: &str) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: endpoint.into(), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(),
        max_attempts: 1, timeout: Duration::from_millis(200), ..Default::default() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_offline_mount_stays_read_only_after_reconnect_until_a_pinned_reopen() {
    let upstream = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&upstream.endpoint)).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    remote.put_object("drive", "remote", "bytes", Default::default()).await.unwrap();
    let spare = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = spare.local_addr().unwrap();
    drop(spare);
    let endpoint = format!("http://{addr}");
    let dir = tempfile::Builder::new().prefix("vdsprovisional").tempdir_in(std::env::temp_dir()).unwrap();
    let fake = Arc::new(Fake::default());
    let mut cfg = DaemonConfig::new(dir.path(), sdk(&endpoint), Build::default());
    cfg.adapters = vec![fake.clone()];
    cfg.connectivity.offline_after = 1;
    cfg.connectivity.probe_every = Duration::from_millis(100);
    let daemon = Daemon::start(cfg).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    let mountpoint = dir.path().join("mount");
    let mounted = client.mount(&NewMount { drive: "drive".into(), mountpoint: Some(mountpoint.display().to_string()), ..Default::default() }).await.unwrap();
    assert!(mounted.read_only, "the adapter and API expose the provisional read-only policy");
    let provisional = fake.core.lock().unwrap().upgrade().expect("the mount retains its provisional core");
    match client.session("drive", false).await {
        Err(FsClientError::Api { status: 503, code, errno, .. }) => assert_eq!((code.as_str(), errno), ("Offline", FsError::Offline.errno())),
        other => panic!("an unpinned RPC session must be refused: {other:?}"),
    }

    // Start forwarding on the previously unreachable address, preserving the signed headers.
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let http = reqwest::Client::new();
    let upstream_endpoint = upstream.endpoint.clone();
    let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let (http, endpoint) = (http.clone(), upstream_endpoint.clone());
        async move {
            let (parts, body) = request.into_parts();
            let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
            let mut send = http.request(parts.method, format!("{endpoint}{}", parts.uri));
            for (name, value) in &parts.headers { send = send.header(name, value); }
            let response = send.body(bytes).send().await.unwrap();
            let mut output = Response::builder().status(response.status());
            for (name, value) in response.headers() {
                if !matches!(name.as_str(), "content-length" | "transfer-encoding" | "connection") { output = output.header(name, value); }
            }
            output.body(Body::from_stream(futures::stream::unfold(response, |mut response| async move {
                match response.chunk().await {
                    Ok(Some(bytes)) => Some((Ok::<_, std::io::Error>(bytes), response)),
                    _ => None,
                }
            }))).unwrap()
        }
    });
    let relay = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if provisional.lookup(provisional.root(), "remote").await.is_ok() { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("the provisional read-only mount can read after reconnect");
    assert_eq!(provisional.create(provisional.root(), "unsafe", 0o600).await.unwrap_err(), FsError::ReadOnly, "it must never enqueue an alias-based mutation");
    let pinned = client.session("drive", false).await.unwrap();
    assert_ne!(pinned.info().root, provisional.root(), "reopening resolves a separate stable-ID namespace");
    let remote_file = pinned.lookup(pinned.info().root, "remote").await.unwrap();
    let opened = pinned.open(remote_file.ino, true).await.unwrap();
    assert_eq!(pinned.write(opened.fh, 0, bytes::Bytes::from_static(b"safe!")).await.unwrap(), 5);
    pinned.close(opened.fh).await.unwrap();
    pinned.release().await.unwrap();
    drop(provisional);
    client.unmount(&mountpoint.display().to_string()).await.unwrap();
    daemon.stop().await;
    relay.abort();
    let _ = relay.await;
}
