// SPDX-License-Identifier: Apache-2.0
//! Uploads handed to the daemon in this process, through `DaemonClient`.

use std::path::Path;
use std::time::{Duration, Instant};

use bytes::Bytes;
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
    let mut paused_names = vec!["footage".to_owned(), id];
    paused_names.sort();
    assert_eq!(c.uploads(false, None).await.unwrap().queue.paused_drives, paused_names);
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

async fn mounted_file(client: &DaemonClient, drive: &str, key: &str) {
    let session = client.session(drive, false).await.unwrap();
    let item = session.create(session.info().root, key, 0o640).await.unwrap();
    let opened = session.open(item.ino, true).await.unwrap().fh;
    session.write(opened, 0, Bytes::from_static(b"mounted bytes")).await.unwrap();
    session.close(opened).await.unwrap();
    session.release().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drive_controls_apply_to_cli_alias_and_mounted_id_entries() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server)).unwrap();
    let id = remote.create_drive("footage", Default::default()).await.unwrap().drive_id;
    remote.create_drive("other", Default::default()).await.unwrap();
    let dir = state_dir("mixed");
    let (daemon, client) = start(&server, dir.path()).await;
    client.pause(&Scope::All).await.unwrap();
    let source = dir.path().join("source.txt");
    std::fs::write(&source, "imported bytes").unwrap();
    client.enqueue(&batch("footage", &[(&source, "cli.txt")])).await.unwrap();
    client.enqueue(&batch("other", &[(&source, "unrelated.txt")])).await.unwrap();
    mounted_file(&client, "footage", "mount.txt").await;
    let before = client.uploads(false, None).await.unwrap().queue;
    let selected = before.items.iter().filter(|item| item.drive == "footage" || item.drive == id).count() as u64;
    assert!(before.items.iter().any(|item| item.drive == "footage") && before.items.iter().any(|item| item.drive == id), "fixture contains both queue spellings");
    assert_eq!(client.pause(&Scope::Drive("footage".into())).await.unwrap().items, selected, "alias pause must include mounted stable-ID entries");
    let paused = client.uploads(false, None).await.unwrap().queue.paused_drives;
    assert!(paused.contains(&"footage".to_owned()) && paused.contains(&id), "both queue spellings stay paused for future entries: {paused:?}");
    assert_eq!(client.resume(&Scope::Drive(id.clone())).await.unwrap().items, selected, "ID resume includes CLI aliases too");
    assert!(client.uploads(false, None).await.unwrap().queue.paused_drives.is_empty());
    let mounted = before.items.iter().filter(|item| item.drive == id).count() as u64;
    let cancelled = client.cancel(&Scope::Drive(id.clone())).await.unwrap();
    assert_eq!(cancelled.items, selected, "ID cancel includes both queue spellings");
    assert_eq!(cancelled.kept, mounted, "the mount's changes are kept on this Mac");
    let remaining = client.uploads(false, None).await.unwrap().queue.items;
    let (to_send, kept): (Vec<_>, Vec<_>) = remaining.iter().partition(|item| item.state.to_send());
    assert_eq!(to_send.len(), 1);
    assert_eq!((to_send[0].drive.as_str(), to_send[0].key.as_str()), ("other", "unrelated.txt"), "drive control cannot affect another drive");
    assert!(kept.len() as u64 == mounted && kept.iter().all(|item| item.state == voidfs_client::State::Kept && item.key == "mount.txt"), "{kept:?}");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alias_pause_covers_new_mounted_entries_before_the_first_session() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server)).unwrap();
    let id = remote.create_drive("footage", Default::default()).await.unwrap().drive_id;
    let dir = state_dir("future");
    let (daemon, client) = start(&server, dir.path()).await;
    client.pause(&Scope::All).await.unwrap();
    assert_eq!(client.pause(&Scope::Drive("footage".into())).await.unwrap().items, 0);
    mounted_file(&client, &id, "future.txt").await;
    client.resume(&Scope::All).await.unwrap();
    let status = client.uploads(false, None).await.unwrap().queue;
    assert!(status.paused_drives.contains(&id), "the alias pause predates this session and must bind its stable ID");
    assert!(status.items.iter().any(|item| item.drive == id && item.key == "future.txt" && item.paused), "a later mounted save is paused");
    assert!(remote.get_object(&id, "future.txt", Default::default()).await.is_err(), "paused bytes have not reached the service");
    client.resume(&Scope::Drive(id.clone())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = client.uploads(false, None).await.unwrap().queue;
            if status.items.is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("resumed mounted save publishes");
    assert_eq!(remote.get_object(&id, "future.txt", Default::default()).await.unwrap().body, Bytes::from_static(b"mounted bytes"));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_alias_pause_is_migrated_before_worker_start_and_offline_controls_share_names() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server)).unwrap();
    let id = remote.create_drive("footage", Default::default()).await.unwrap().drive_id;
    let dir = state_dir("legacy");
    let mut cfg = DaemonConfig::new(dir.path(), sdk(&server), Build::default());
    cfg.client.timeout = Duration::from_millis(500);
    cfg.client.max_attempts = 1;
    let daemon = Daemon::start(cfg.clone()).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    client.pause(&Scope::All).await.unwrap();
    let source = dir.path().join("source.txt");
    std::fs::write(&source, "legacy imported bytes").unwrap();
    client.enqueue(&batch("footage", &[(&source, "cli.txt")])).await.unwrap();
    mounted_file(&client, "footage", "mount.txt").await;
    client.pause(&Scope::Drive("footage".into())).await.unwrap();
    // Faithfully reproduce older daemon state: mounted entries use the ID, but the persistent
    // drive pause has only the alias. Keep the global gate while constructing that state.
    daemon.queue().resume(voidfs_client::Scope::Drive(id.clone())).await.unwrap();
    assert_eq!(client.uploads(false, None).await.unwrap().queue.paused_drives, ["footage"]);
    daemon.stop().await;
    let daemon = Daemon::start(cfg).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    let migrated = client.uploads(false, None).await.unwrap().queue.paused_drives;
    assert!(migrated.contains(&id), "legacy pause must be repaired before the worker starts, without a session/control request: {migrated:?}");
    client.resume(&Scope::All).await.unwrap();
    let status = client.uploads(false, None).await.unwrap().queue;
    assert!(status.items.iter().all(|item| item.paused), "both recovered CLI and mounted entries remain paused after the global gate opens");
    drop(server);
    let count = status.items.len() as u64;
    assert_eq!(client.resume(&Scope::Drive(id.clone())).await.unwrap().items, count, "offline ID uses the validated memo to resume alias entries too");
    assert!(client.uploads(false, None).await.unwrap().queue.paused_drives.is_empty());
    assert_eq!(client.pause(&Scope::Drive("footage".into())).await.unwrap().items, count, "offline alias uses the same stable identity");
    let paused = client.uploads(false, None).await.unwrap().queue.paused_drives;
    assert!(paused.contains(&id) && paused.contains(&"footage".to_owned()));
    daemon.stop().await;
}
