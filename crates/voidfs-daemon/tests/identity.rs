// SPDX-License-Identifier: Apache-2.0
//! Drive identity remains pinned when aliases move or the bounded offline memo evicts entries.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use futures::future::BoxFuture;
use voidfs_client::mount::Session;
use voidfs_daemon::api::{Build, NewMount};
use voidfs_daemon::{Adapter, Core, Daemon, DaemonClient, DaemonConfig, FsClientError, MountSpec, Mounted};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

#[derive(Default)]
struct Fake { cores: Mutex<Vec<Arc<Session>>> }
struct FakeMount;
impl Adapter for Fake {
    fn name(&self) -> &str { "fake" }
    fn mount<'a>(&'a self, _spec: &'a MountSpec, core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        self.cores.lock().unwrap().push(core.session.clone());
        Box::pin(async { Ok(Arc::new(FakeMount) as Arc<dyn Mounted>) })
    }
}
impl Mounted for FakeMount {
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> { Box::pin(async { Ok(()) }) }
}
fn sdk(endpoint: &str) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: endpoint.into(), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), max_attempts: 1,
        timeout: Duration::from_millis(500), ..Default::default() }
}
fn config(endpoint: &str, dir: &Path, fake: Arc<Fake>) -> DaemonConfig {
    let mut cfg = DaemonConfig::new(dir, sdk(endpoint), Build::default());
    cfg.adapters = vec![fake];
    cfg.connectivity.offline_after = 1;
    cfg
}
fn mount(drive: &str, path: &Path) -> NewMount { NewMount { drive: drive.into(), mountpoint: Some(path.display().to_string()), ..Default::default() } }
async fn restored(client: &DaemonClient, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mounts = client.mounts().await.unwrap().mounts;
            if mounts.len() == count && mounts.iter().all(|mount| mount.state == "mounted") { break; }
            assert!(!mounts.iter().any(|mount| mount.state == "failed"), "restore failed: {mounts:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("remembered mounts restore within their deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_renamed_shared_drive_is_remembered_by_id_and_never_restores_the_reused_alias() {
    let upstream = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&upstream.endpoint)).unwrap();
    let original = remote.create_drive("old", Default::default()).await.unwrap().drive_id;
    let replacement = remote.create_drive("other", Default::default()).await.unwrap().drive_id;
    remote.put_object(&original, "original", "a", Default::default()).await.unwrap();
    remote.put_object(&replacement, "replacement", "b", Default::default()).await.unwrap();
    let moved = Arc::new(AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let http = reqwest::Client::new();
    let original_info = remote.describe_drive(&original).await.unwrap();
    let replacement_info = remote.describe_drive(&replacement).await.unwrap();
    let moved_for_relay = moved.clone();
    let upstream_endpoint = upstream.endpoint.clone();
    let original_for_relay = original.clone();
    // The protocol has no rename endpoint yet; its identity response is the rename boundary.
    let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let (http, endpoint, moved, original, original_info, replacement_info) =
            (http.clone(), upstream_endpoint.clone(), moved_for_relay.clone(), original_for_relay.clone(), original_info.clone(), replacement_info.clone());
        async move {
            if request.uri().query().is_some_and(|query| query.split('&').any(|field| field.split('=').next() == Some("x-voidfs-drive"))) && moved.load(Ordering::SeqCst) {
                if request.uri().path() == format!("/{original}") {
                    let mut info = original_info;
                    info.alias = "renamed".into();
                    return axum::Json(info).into_response();
                }
                if request.uri().path() == "/old" {
                    let mut info = replacement_info;
                    info.alias = "old".into();
                    return axum::Json(info).into_response();
                }
            }
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
                match response.chunk().await { Ok(Some(bytes)) => Some((Ok::<_, std::io::Error>(bytes), response)), _ => None }
            }))).unwrap()
        }
    });
    use axum::response::IntoResponse;
    let relay = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let dir = tempfile::Builder::new().prefix("vdsidentity").tempdir_in(std::env::temp_dir()).unwrap();
    let fake = Arc::new(Fake::default());
    let daemon = Daemon::start(config(&endpoint, dir.path(), fake.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    let session = client.session("old", true).await.unwrap();
    moved.store(true, Ordering::SeqCst);
    let mountpoint = dir.path().join("renamed");
    let mounted = client.mount(&mount(&original, &mountpoint)).await.unwrap();
    assert_eq!(mounted.drive, "renamed", "a reused core must use the freshly resolved display alias");
    let remembered = client.mounts().await.unwrap().remembered;
    assert_eq!(remembered[0].drive_id.as_deref(), Some(original.as_str()));
    assert_eq!(remembered[0].drive, "renamed");
    session.release().await.unwrap();
    fake.cores.lock().unwrap().clear();
    daemon.stop().await;
    // Even an obsolete display alias must not become the authority for restoration.
    let store = voidfs_client::Store::open(dir.path()).unwrap();
    let mut remembered = voidfs_client::mounts::remembered(&store).unwrap().remove(0);
    remembered.drive = "old".into();
    voidfs_client::mounts::remember(&store, &remembered).unwrap();
    drop(store);
    let daemon = Daemon::start(config(&endpoint, dir.path(), fake.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    restored(&client, 1).await;
    let core = fake.cores.lock().unwrap()[0].clone();
    assert!(core.lookup(core.root(), "original").await.is_ok());
    assert_eq!(core.lookup(core.root(), "replacement").await.unwrap_err(), voidfs_client::mount::FsError::NotFound);
    assert_eq!(client.mounts().await.unwrap().mounts[0].drive, "renamed");
    drop(core);
    fake.cores.lock().unwrap().clear();
    daemon.stop().await;
    relay.abort();
    let _ = relay.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remembered_ids_restore_writable_offline_without_any_alias_memo() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
    let id = remote.create_drive("drive", Default::default()).await.unwrap().drive_id;
    let dir = tempfile::Builder::new().prefix("vdsmemoevict").tempdir_in(std::env::temp_dir()).unwrap();
    let fake = Arc::new(Fake::default());
    let daemon = Daemon::start(config(&server.endpoint, dir.path(), fake.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    let mountpoint = dir.path().join("mount");
    client.mount(&mount("drive", &mountpoint)).await.unwrap();
    fake.cores.lock().unwrap().clear();
    daemon.stop().await;
    std::fs::write(dir.path().join("drive-aliases.json"), "{}").unwrap();
    let spare = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", spare.local_addr().unwrap());
    drop(spare);
    let daemon = Daemon::start(config(&endpoint, dir.path(), fake.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    restored(&client, 1).await;
    let view = client.mounts().await.unwrap();
    assert!(!view.mounts[0].read_only, "memo eviction cannot revoke a remembered stable identity");
    assert_eq!(view.remembered[0].drive_id.as_deref(), Some(id.as_str()));
    fake.cores.lock().unwrap().clear();
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_unknown_offline_rpc_sessions_leave_the_state_database_unchanged() {
    let spare = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", spare.local_addr().unwrap());
    drop(spare);
    let dir = tempfile::Builder::new().prefix("vdsrefusedstate").tempdir_in(std::env::temp_dir()).unwrap();
    let daemon = Daemon::start(config(&endpoint, dir.path(), Arc::new(Fake::default()))).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    let database = || ["state.sqlite", "state.sqlite-wal"].map(|name| std::fs::read(dir.path().join(name)).unwrap_or_default());
    let before = database();
    for index in 0..20 {
        match client.session(&format!("unknown-{index}"), false).await {
            Err(FsClientError::Api { status: 503, code, .. }) => assert_eq!(code, "Offline"),
            result => panic!("unknown offline session should be refused: {result:?}"),
        }
    }
    assert_eq!(database(), before, "refusal must precede namespace roots, inodes and epoch allocation");
    assert!(!dir.path().join("drive-aliases.json").exists());
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_remembered_aliases_without_identity_fail_instead_of_selecting_a_reused_name() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
    remote.create_drive("old", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdslegacyidentity").tempdir_in(std::env::temp_dir()).unwrap();
    let store = voidfs_client::Store::open(dir.path()).unwrap();
    voidfs_client::mounts::remember(&store, &voidfs_client::Remembered { mountpoint: dir.path().join("mount").display().to_string(), drive: "old".into(),
        drive_id: None, adapter: "fake".into(), read_only: false }).unwrap();
    drop(store);
    let fake = Arc::new(Fake::default());
    let daemon = Daemon::start(config(&server.endpoint, dir.path(), fake.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mounts = client.mounts().await.unwrap().mounts;
            if mounts[0].state != "mounting" {
                assert_eq!(mounts[0].state, "failed", "an unpinned remembered alias cannot be trusted after reuse");
                assert!(mounts[0].error.as_deref().unwrap().contains("stable identity"));
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("unresolved legacy remembered identity fails promptly");
    assert!(fake.cores.lock().unwrap().is_empty());
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_mount_cannot_adopt_a_reused_alias_even_when_the_memo_validly_resolves_it() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
    let original = remote.create_drive("original", Default::default()).await.unwrap().drive_id;
    let replacement = remote.create_drive("old", Default::default()).await.unwrap().drive_id;
    remote.put_object(&original, "original-file", "a", Default::default()).await.unwrap();
    remote.put_object(&replacement, "replacement-file", "b", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdslegacyreuse").tempdir_in(std::env::temp_dir()).unwrap();
    let fake = Arc::new(Fake::default());
    let daemon = Daemon::start(config(&server.endpoint, dir.path(), fake.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    let mountpoint = dir.path().join("mount");
    client.mount(&mount(&original, &mountpoint)).await.unwrap();
    let other = client.session("old", false).await.unwrap();
    assert!(other.lookup(other.info().root, "replacement-file").await.is_ok());
    other.release().await.unwrap();
    fake.cores.lock().unwrap().clear();
    daemon.stop().await;
    let store = voidfs_client::Store::open(dir.path()).unwrap();
    let mut remembered = voidfs_client::mounts::remembered(&store).unwrap().remove(0);
    // PR44 could store an old display alias for A while the current alias memo safely names B.
    remembered.drive = "old".into();
    remembered.drive_id = None;
    voidfs_client::mounts::remember(&store, &remembered).unwrap();
    drop(store);
    let aliases: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.path().join("drive-aliases.json")).unwrap()).unwrap();
    assert_eq!(aliases["old"]["id"], replacement);
    let daemon = Daemon::start(config(&server.endpoint, dir.path(), fake.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mounts = client.mounts().await.unwrap().mounts;
            if mounts[0].state != "mounting" {
                assert_eq!(mounts[0].state, "failed", "the alias memo proves B's current name, not A's remembered identity");
                assert!(mounts[0].error.as_deref().unwrap().contains("stable identity"));
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("ambiguous legacy restoration fails promptly");
    assert!(fake.cores.lock().unwrap().is_empty(), "neither the replacement drive nor a provisional namespace may be mounted");
    daemon.stop().await;
}
