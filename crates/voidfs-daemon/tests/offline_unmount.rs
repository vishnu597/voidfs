// SPDX-License-Identifier: Apache-2.0
//! A stable drive ID still forgets its mounted aliases when the service cannot answer.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use voidfs_daemon::api::{Build, NewMount};
use voidfs_daemon::{Adapter, Core, Daemon, DaemonClient, DaemonConfig, MountSpec, Mounted};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

struct Fake;
struct FakeMount;

impl Adapter for Fake {
    fn name(&self) -> &str { "fake" }
    fn mount<'a>(&'a self, _spec: &'a MountSpec, _core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
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
async fn offline_unmount_by_stable_id_forgets_the_removed_mountpoint() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    let id = remote.describe_drive("drive").await.unwrap().drive_id;
    let dir = tempfile::Builder::new().prefix("vdsofflineid").tempdir_in(std::env::temp_dir()).unwrap();
    let mountpoint = dir.path().join("mount");
    let mut cfg = DaemonConfig::new(dir.path(), sdk(&server.endpoint), Build::default());
    cfg.adapters = vec![Arc::new(Fake)];
    let daemon = Daemon::start(cfg.clone()).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    client.mount(&NewMount { drive: "drive".into(), mountpoint: Some(mountpoint.display().to_string()), ..Default::default() }).await.unwrap();
    daemon.stop().await;

    // A new daemon has no live service connection: it restores the durable identity memo.
    let spare = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    cfg.client.endpoint = format!("http://{}", spare.local_addr().unwrap());
    drop(spare);
    let daemon = Daemon::start(cfg).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    tokio::time::timeout(Duration::from_secs(5), async {
        while !client.mounts().await.unwrap().mounts.first().is_some_and(|mount| mount.state == "mounted") {
            tokio::task::yield_now().await;
        }
    }).await.expect("the remembered mount restores from its known stable ID offline");
    assert_eq!(client.unmount(&id).await.unwrap().unmounted, [mountpoint.display().to_string()]);
    let view = client.mounts().await.unwrap();
    assert!(view.mounts.is_empty(), "the mount is removed");
    assert!(view.remembered.is_empty(), "the removed mountpoint must not be restored again: {view:?}");
    daemon.stop().await;
}
