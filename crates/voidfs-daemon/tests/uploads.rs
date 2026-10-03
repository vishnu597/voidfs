// SPDX-License-Identifier: Apache-2.0
//! Uploads handed to the daemon in this process, through `DaemonClient`.

use std::path::Path;
use std::time::{Duration, Instant};

use voidfs_daemon::api::{Build, NewBatch, NewFile, Scope};
use voidfs_daemon::{ClientError, Daemon, DaemonClient, DaemonConfig};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

fn state_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(&format!("vdu{tag}")).tempdir_in(std::env::temp_dir()).unwrap()
}

fn sdk(server: &TestServer) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: server.endpoint.replace("127.0.0.1", "localhost"), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..voidfs_sdk::Config::default() }
}

async fn start(server: &TestServer, dir: &Path) -> (Daemon, DaemonClient) {
    let d = Daemon::start(DaemonConfig::new(dir, sdk(server), Build::default())).await.unwrap();
    let c = DaemonClient::new(d.socket());
    (d, c)
}

fn batch(drive: &str, files: &[(&Path, &str)]) -> NewBatch {
    NewBatch { label: "t".into(), drive: drive.into(), files: files.iter().map(|(p, k)| NewFile { path: p.display().to_string(), key: (*k).into() }).collect() }
}

#[track_caller]
fn refused<T: std::fmt::Debug>(r: Result<T, ClientError>, status: u16, code: &str) -> String {
    match r {
        Err(ClientError::Api { status: s, code: c, message }) if s == status && c == code => message,
        other => panic!("expected {status} {code}, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batch_is_checked_before_it_is_queued() {
    let server = TestServer::start().await.unwrap();
    let client = voidfs_sdk::Client::new(sdk(&server)).unwrap();
    client.create_drive("footage", Default::default()).await.unwrap();
    let id = client.describe_drive("footage").await.unwrap().drive_id;
    let dir = state_dir("a");
    let (d, c) = start(&server, dir.path()).await;
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("a.txt");
    std::fs::write(&f, "abc").unwrap();

    refused(c.enqueue(&batch("footage", &[])).await, 400, "InvalidArgument");
    let m = refused(c.enqueue(&batch("footage", &[(Path::new("a.txt"), "a.txt")])).await, 400, "InvalidArgument");
    assert!(m.contains("not an absolute path"), "{m}");
    refused(c.enqueue(&batch("footage", &[(&src.path().join("missing"), "m")])).await, 400, "LocalFileError");
    refused(c.enqueue(&batch("footage", &[(&f, "/a.txt")])).await, 400, "InvalidArgument");
    refused(c.enqueue(&batch("nosuchdrive", &[(&f, "a.txt")])).await, 404, "NoSuchBucket");
    assert!(c.uploads(true, None).await.unwrap().queue.items.is_empty(), "nothing was queued");

    // By id, it is queued under the drive's alias, which a pause by either name finds.
    c.pause(&Scope::All).await.unwrap();
    let q = c.enqueue(&batch(&id, &[(&f, "a.txt")])).await.unwrap();
    assert_eq!((q.drive.as_str(), q.items, q.bytes), ("footage", 1, 3));
    assert_eq!(c.pause(&Scope::Drive(id.clone())).await.unwrap().items, 1);
    assert_eq!(c.uploads(false, None).await.unwrap().queue.paused_drives, ["footage"]);
    refused(c.pause(&Scope::Batch(q.batch + 1)).await, 404, "NoSuchUpload");
    refused(c.uploads(false, Some(q.batch + 1)).await, 404, "NoSuchUpload");
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_are_watched_until_the_daemon_stops_and_counted_for_a_rate() {
    let server = TestServer::start().await.unwrap();
    let client = voidfs_sdk::Client::new(sdk(&server)).unwrap();
    client.create_drive("footage", Default::default()).await.unwrap();
    let dir = state_dir("w");
    let (d, c) = start(&server, dir.path()).await;
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("big.bin");
    std::fs::write(&f, vec![7u8; 3 << 20]).unwrap();
    let q = c.enqueue(&batch("footage", &[(&f, "big.bin")])).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let l = c.uploads(true, Some(q.batch)).await.unwrap();
        if l.queue.batches[0].done == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "the upload didn't finish: {l:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(c.status().await.unwrap().uploads.rate > 0, "3 MiB went out in the last seconds");

    let mut w = c.watch_uploads(true, None).await.unwrap();
    let first = w.next().await.unwrap().unwrap();
    assert_eq!(first.queue.batches.len(), 1);
    let t = Instant::now();
    let second = w.next().await.unwrap().unwrap();
    assert!(t.elapsed() >= Duration::from_millis(500), "a second apart");
    assert_eq!(second.queue.items.len(), 1);
    let stopping = tokio::spawn(d.stop());
    let end = tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(l) = w.next().await {
            l.unwrap();
        }
    });
    end.await.expect("the stream ends when the daemon stops, without waiting out the drain");
    stopping.await.unwrap();
}
