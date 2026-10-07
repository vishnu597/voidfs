// SPDX-License-Identifier: Apache-2.0
//! Local edits from mount adapters and RPCs share targeted invalidations even offline.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures::future::BoxFuture;
use voidfs_client::{Invalidation, Link, mount::LocalChange};
use voidfs_daemon::api::{Build, NewMount, Scope, fs};
use voidfs_daemon::{Adapter, Core, Daemon, DaemonClient, DaemonConfig, InvalidationWatch, MountSpec, Mounted};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

#[derive(Default)]
struct Capture { cores: Mutex<Vec<Core>>, observers: Mutex<Vec<Arc<Mutex<Vec<LocalChange>>>>> }
struct CaptureMount { seen: Arc<Mutex<Vec<LocalChange>>> }

impl Adapter for Capture {
    fn name(&self) -> &str { "capture" }
    fn mount<'a>(&'a self, _: &'a MountSpec, core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        Box::pin(async move {
            let seen = Arc::new(Mutex::new(Vec::new()));
            self.cores.lock().unwrap().push(core.clone());
            self.observers.lock().unwrap().push(seen.clone());
            Ok(Arc::new(CaptureMount { seen }) as Arc<dyn Mounted>)
        })
    }
}

impl Mounted for CaptureMount {
    fn local_change(&self, change: &LocalChange) { self.seen.lock().unwrap().push(change.clone()); }
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> { Box::pin(async { Ok(()) }) }
}

fn sdk(endpoint: &str) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: endpoint.replace("127.0.0.1", "localhost"), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..Default::default() }
}

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(u64::MAX) }

fn config(endpoint: &str, dir: &Path, adapter: Arc<Capture>) -> DaemonConfig {
    let mut cfg = DaemonConfig::new(dir, sdk(endpoint), Build::default());
    cfg.adapters = vec![adapter];
    cfg.cache = voidfs_client::CacheConfig { min_free_bytes: 0, free_space: Some(plenty), read_ahead_bytes: 0, ..Default::default() };
    cfg.connectivity = voidfs_client::ConnectivityConfig { offline_after: 1, probe_every: Duration::from_secs(3600), probe_max: Duration::from_secs(3600), ..Default::default() };
    cfg
}

async fn matching(watch: &mut InvalidationWatch, ino: u64) -> fs::InvalidationEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = watch.next().await.expect("watch stays open").unwrap();
            if !event.resync && event.inodes.contains(&ino) { return event; }
        }
    }).await.expect("local change is announced without a remote feed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_mount_edits_reach_both_adapters_and_rpc_observers() {
    let server = TestServer::start().await.unwrap();
    let endpoint = server.endpoint.clone();
    let remote = voidfs_sdk::Client::new(sdk(&endpoint)).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    remote.put_object("drive", "data", Bytes::from_static(b"base"), Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vlocal").tempdir_in(std::env::temp_dir()).unwrap();
    let adapter = Arc::new(Capture::default());
    let daemon = Daemon::start(config(&endpoint, dir.path(), adapter.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    client.pause(&Scope::All).await.unwrap();
    for name in ["one", "two"] {
        client.mount(&NewMount { drive: "drive".into(), mountpoint: Some(dir.path().join(name).display().to_string()), ..Default::default() }).await.unwrap();
    }
    let core = adapter.cores.lock().unwrap()[0].clone();
    let reader = client.session("drive", true).await.unwrap();
    let ino = reader.lookup(reader.info().root, "data").await.unwrap().ino;
    let opened = reader.open(ino, false).await.unwrap().fh;
    let writable = core.session.open(ino, true).await.unwrap();
    drop(server);
    core.connectivity.unanswered();
    assert_eq!(core.connectivity.link(), Link::Offline);
    let mut watch = reader.watch().await.unwrap();
    assert!(watch.next().await.unwrap().unwrap().resync);
    core.session.write(writable, 4, Bytes::from_static(b"edit")).await.unwrap();
    let event = matching(&mut watch, ino).await;
    assert!(!event.resync);
    assert_eq!(event.invalidations, [fs::Invalidation::Object("data".into())]);
    assert_eq!(reader.getattr(ino).await.unwrap().size, 8);
    assert_eq!(reader.handle_attr(opened).await.unwrap().size, 8);
    for seen in adapter.observers.lock().unwrap().iter() {
        assert!(seen.lock().unwrap().iter().any(|change| change.inodes.contains(&ino) && change.invalidations == [Invalidation::Object("data".into())] && !change.namespace));
    }
    core.session.truncate(writable, 2).await.unwrap();
    let resized = matching(&mut watch, ino).await;
    assert!(!resized.resync && resized.generation > event.generation);
    assert_eq!(reader.handle_attr(opened).await.unwrap().size, 2);
    core.session.unlink(core.session.root(), "data").await.unwrap();
    matching(&mut watch, ino).await;
    core.session.write(writable, 2, Bytes::from_static(b"orphan")).await.unwrap();
    let orphan = matching(&mut watch, ino).await;
    assert!(!orphan.resync && orphan.invalidations.is_empty(), "an unlinked inode remains a targeted metadata edit");
    assert_eq!(reader.handle_attr(opened).await.unwrap().size, 8);
    for seen in adapter.observers.lock().unwrap().iter() {
        assert!(seen.lock().unwrap().iter().any(|change| change.inodes == [ino] && change.invalidations.is_empty() && !change.namespace));
    }
    core.session.close(writable).await.unwrap();
    reader.close(opened).await.unwrap();
    reader.release().await.unwrap();
    drop(watch);
    drop(core);
    adapter.cores.lock().unwrap().clear();
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writes_during_directory_pages_keep_the_namespace_generation_stable() {
    let server = TestServer::start().await.unwrap();
    let endpoint = server.endpoint.clone();
    let remote = voidfs_sdk::Client::new(sdk(&endpoint)).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    for key in ["data", "unrelated"] { remote.put_object("drive", key, Bytes::from_static(b"base"), Default::default()).await.unwrap(); }
    let dir = tempfile::Builder::new().prefix("vpages").tempdir_in(std::env::temp_dir()).unwrap();
    let adapter = Arc::new(Capture::default());
    let daemon = Daemon::start(config(&endpoint, dir.path(), adapter.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    client.pause(&Scope::All).await.unwrap();
    client.mount(&NewMount { drive: "drive".into(), mountpoint: Some(dir.path().join("mount").display().to_string()), ..Default::default() }).await.unwrap();
    let core = adapter.cores.lock().unwrap()[0].clone();
    let writer = client.session("drive", false).await.unwrap();
    let ino = writer.lookup(writer.info().root, "data").await.unwrap().ino;
    let fh = writer.open(ino, true).await.unwrap().fh;
    drop(server);
    core.connectivity.unanswered();
    let before = writer.readdir(writer.info().root, None, 1).await.unwrap().generation;
    tokio::join!(async {
        for offset in 0..32 { writer.write(fh, offset, Bytes::from_static(b"x")).await.unwrap(); }
    }, async {
        for _ in 0..32 {
            let page = writer.readdir(writer.info().root, None, 1).await.expect("attribute edits do not race the namespace epoch");
            assert_eq!(page.generation, before);
            assert_eq!(page.entries[0].0, "data");
        }
    });
    assert_eq!(writer.readdir(writer.info().root, Some("data"), 1).await.unwrap().generation, before);
    core.session.create(core.session.root(), "new", 0o644).await.unwrap();
    let after = writer.readdir(writer.info().root, None, 10).await.unwrap();
    assert!(after.generation > before, "a namespace edit advances enumeration");
    assert_eq!(after.entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["data", "new", "unrelated"]);
    writer.close(fh).await.unwrap();
    writer.release().await.unwrap();
    drop(core);
    adapter.cores.lock().unwrap().clear();
    daemon.stop().await;
}
