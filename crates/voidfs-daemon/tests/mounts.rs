// SPDX-License-Identifier: Apache-2.0
//! The mount table, with a fake adapter: the daemon in this process, asked through `DaemonClient`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use voidfs_client::Invalidation;
use voidfs_daemon::api::{Build, NewMount};
use voidfs_daemon::{Adapter, ClientError, Core, Daemon, DaemonClient, DaemonConfig, MountSpec, Mounted};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

/// Mounts by leaving a marker in the mountpoint, and keeps what the feed made stale.
#[derive(Default)]
struct Fake {
    refuse: AtomicBool,
    seen: Arc<Mutex<Vec<Invalidation>>>,
}

struct FakeMount {
    marker: PathBuf,
    seen: Arc<Mutex<Vec<Invalidation>>>,
}

impl Adapter for Fake {
    fn name(&self) -> &str {
        "fake"
    }

    fn mount<'a>(&'a self, spec: &'a MountSpec, _core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        Box::pin(async move {
            if self.refuse.load(Ordering::SeqCst) {
                return Err("the fake refuses".into());
            }
            let marker = spec.mountpoint.join(".mounted");
            std::fs::write(&marker, &spec.drive).map_err(|e| e.to_string())?;
            Ok(Arc::new(FakeMount { marker, seen: self.seen.clone() }) as Arc<dyn Mounted>)
        })
    }
}

impl Mounted for FakeMount {
    fn invalidate(&self, changes: &[Invalidation]) {
        self.seen.lock().unwrap().extend_from_slice(changes);
    }

    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move { std::fs::remove_file(&self.marker).map_err(|e| e.to_string()) })
    }
}

fn state_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(&format!("vdm{tag}")).tempdir_in(std::env::temp_dir()).unwrap()
}

fn sdk(server: &TestServer) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: server.endpoint.replace("127.0.0.1", "localhost"), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..voidfs_sdk::Config::default() }
}

async fn start(server: &TestServer, dir: &Path, adapters: Vec<Arc<dyn Adapter>>) -> (Daemon, DaemonClient) {
    let cfg = DaemonConfig { adapters, ..DaemonConfig::new(dir, sdk(server), Build::default()) };
    let d = Daemon::start(cfg).await.unwrap();
    let c = DaemonClient::new(d.socket());
    (d, c)
}

#[track_caller]
fn refused<T: std::fmt::Debug>(r: Result<T, ClientError>, status: u16, code: &str) -> String {
    match r {
        Err(ClientError::Api { status: s, code: c, message }) if s == status && c == code => message,
        other => panic!("expected {status} {code}, got {other:?}"),
    }
}

async fn until(what: &str, mut ok: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ok().await {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn new_mount(drive: &str, mountpoint: &Path) -> NewMount {
    NewMount { drive: drive.into(), mountpoint: Some(mountpoint.display().to_string()), ..Default::default() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mounts_follow_their_feed_and_come_back_when_the_daemon_starts() {
    let server = TestServer::start().await.unwrap();
    let client = voidfs_sdk::Client::new(sdk(&server)).unwrap();
    client.create_drive("footage", Default::default()).await.unwrap();
    let id = client.describe_drive("footage").await.unwrap().drive_id;
    let dir = state_dir("f");
    let mp = dir.path().join("mnt/footage");
    let fake = Arc::new(Fake::default());
    let (d, c) = start(&server, dir.path(), vec![fake.clone()]).await;

    let m = c.mount(&new_mount(&id, &mp)).await.unwrap();
    assert_eq!((m.drive.as_str(), m.adapter.as_str(), m.state.as_str(), m.mountpoint.as_str()), ("footage", "fake", "mounted", mp.to_str().unwrap()), "by its alias, the mountpoint made");
    assert!(mp.join(".mounted").exists());
    let seq = m.feed.as_ref().unwrap().seq;
    refused(c.mount(&new_mount("footage", &mp)).await, 409, "AlreadyMounted");

    client.put_object("footage", "a.txt", "alpha", Default::default()).await.unwrap();
    until("the feed reaches the mount", async || c.status().await.unwrap().mounts[0].feed.as_ref().is_some_and(|f| f.events >= 1 && f.seq > seq)).await;
    assert!(fake.seen.lock().unwrap().contains(&Invalidation::Object("a.txt".into())), "the adapter is told");
    assert_eq!(c.info().await.unwrap().mounts, 1);
    let view = c.mounts().await.unwrap();
    assert_eq!(view.remembered.len(), 1);
    assert_eq!((view.remembered[0].drive.as_str(), view.remembered[0].mountpoint.as_str()), ("footage", mp.to_str().unwrap()));

    // Stopping detaches it; starting again mounts it again.
    d.stop().await;
    assert!(!mp.join(".mounted").exists(), "detached as the daemon stopped");
    let (d, c) = start(&server, dir.path(), vec![fake.clone()]).await;
    until("the remembered mount comes back", async || c.mounts().await.unwrap().mounts.first().is_some_and(|m| m.state == "mounted")).await;
    assert!(mp.join(".mounted").exists());

    // Unmounted by the drive's id: gone, and forgotten.
    assert_eq!(c.unmount(&id).await.unwrap().unmounted, [mp.display().to_string()]);
    assert!(!mp.join(".mounted").exists());
    let view = c.mounts().await.unwrap();
    assert!(view.mounts.is_empty() && view.remembered.is_empty(), "{view:?}");
    refused(c.unmount(&mp.display().to_string()).await, 404, "NoSuchMount");
    d.stop().await;
    let (d, c) = start(&server, dir.path(), vec![fake]).await;
    assert!(c.mounts().await.unwrap().mounts.is_empty(), "nothing to bring back");
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mount_without_an_adapter_or_that_fails_is_not_remembered() {
    let server = TestServer::start().await.unwrap();
    let client = voidfs_sdk::Client::new(sdk(&server)).unwrap();
    client.create_drive("footage", Default::default()).await.unwrap();
    let dir = state_dir("n");
    let mp = dir.path().join("mnt");

    let (d, c) = start(&server, dir.path(), Vec::new()).await;
    let m = refused(c.mount(&new_mount("footage", &mp)).await, 501, "NoAdapter");
    assert!(m.contains("step 5"), "{m}");
    refused(c.mount(&NewMount { mountpoint: Some("rel/x".into()), ..new_mount("footage", &mp) }).await, 501, "NoAdapter");
    d.stop().await;

    let fake = Arc::new(Fake::default());
    let (d, c) = start(&server, dir.path(), vec![fake.clone()]).await;
    refused(c.mount(&NewMount { mountpoint: Some("rel/x".into()), ..new_mount("footage", &mp) }).await, 400, "InvalidArgument");
    refused(c.mount(&new_mount("nosuchdrive", &mp)).await, 404, "NoSuchBucket");
    refused(c.mount(&NewMount { adapter: Some("smb".into()), ..new_mount("footage", &mp) }).await, 501, "NoAdapter");
    fake.refuse.store(true, Ordering::SeqCst);
    refused(c.mount(&new_mount("footage", &mp)).await, 500, "MountFailed");
    let view = c.mounts().await.unwrap();
    assert!(view.mounts.is_empty() && view.remembered.is_empty(), "a failed mount leaves nothing: {view:?}");
    fake.refuse.store(false, Ordering::SeqCst);
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    d.stop().await;

    // No mountpoint named: `<root>/<drive>`.
    let root = dir.path().join("root");
    let cfg = DaemonConfig { adapters: vec![fake.clone()], mount_root: Some(root.clone()), ..DaemonConfig::new(&dir.path().join("other"), sdk(&server), Build::default()) };
    let other = Daemon::start(cfg).await.unwrap();
    let m = DaemonClient::new(other.socket()).mount(&NewMount { drive: "footage".into(), ..Default::default() }).await.unwrap();
    assert_eq!(m.mountpoint, root.join("footage").display().to_string());
    assert!(root.join("footage/.mounted").exists());
    other.stop().await;

    // Remembered, then started where no adapter can mount it: kept, and said why.
    let (d, c) = start(&server, dir.path(), Vec::new()).await;
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1));
    assert_eq!(view.mounts[0].state, "failed");
    assert!(view.mounts[0].error.as_deref().unwrap().contains("no adapter"), "{view:?}");
    d.stop().await;

    // Started with the server out of reach: mounted all the same, its feed from the beginning.
    let gone = voidfs_sdk::Config { endpoint: "http://127.0.0.1:1".into(), max_attempts: 1, ..sdk(&server) };
    let cfg = DaemonConfig { adapters: vec![fake.clone()], ..DaemonConfig::new(dir.path(), gone, Build::default()) };
    let d = Daemon::start(cfg).await.unwrap();
    let c2 = DaemonClient::new(d.socket());
    until("the remembered mount comes back offline", async || c2.mounts().await.unwrap().mounts.first().is_some_and(|m| m.state == "mounted")).await;
    assert_eq!(c2.status().await.unwrap().mounts[0].feed.as_ref().map(|f| f.seq), Some(0));
    assert!(mp.join(".mounted").exists());
    d.stop().await;
    let (d, c) = start(&server, dir.path(), Vec::new()).await;
    assert_eq!(c.unmount(&mp.display().to_string()).await.unwrap().unmounted, [mp.display().to_string()]);
    assert!(c.mounts().await.unwrap().remembered.is_empty(), "forgotten");
    d.stop().await;
}
