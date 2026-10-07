// SPDX-License-Identifier: Apache-2.0
//! Filesystem sessions through the real daemon Unix socket, without a platform mount.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use bytes::Bytes;
use voidfs_client::mount::{FsError, Sync};
use voidfs_daemon::api::{Build, Scope, fs};
use voidfs_daemon::{Daemon, DaemonClient, DaemonConfig, FsClient, FsClientError, InvalidationWatch};
use voidfs_sdk::Kind;
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

fn state_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(&format!("vds{tag}")).tempdir_in(std::env::temp_dir()).unwrap()
}

fn sdk(endpoint: &str) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: endpoint.replace("127.0.0.1", "localhost"), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..Default::default() }
}

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(u64::MAX) }

fn config(endpoint: &str, dir: &Path) -> DaemonConfig {
    let mut cfg = DaemonConfig::new(dir, sdk(endpoint), Build::default());
    cfg.cache = voidfs_client::CacheConfig { min_free_bytes: 0, free_space: Some(plenty), block_size: 4096, read_ahead_bytes: 0, ..Default::default() };
    cfg
}

struct Fixture {
    server: TestServer,
    remote: voidfs_sdk::Client,
    dir: tempfile::TempDir,
    daemon: Daemon,
    client: DaemonClient,
}

impl Fixture {
    async fn new(tag: &str, objects: &[(&str, Bytes)]) -> Self {
        let server = TestServer::start().await.unwrap();
        let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
        remote.create_drive("drive", Default::default()).await.unwrap();
        for (key, bytes) in objects { remote.put_object("drive", key, bytes.clone(), Default::default()).await.unwrap(); }
        let dir = state_dir(tag);
        let daemon = Daemon::start(config(&server.endpoint, dir.path())).await.unwrap();
        let client = DaemonClient::new(daemon.socket());
        Self { server, remote, dir, daemon, client }
    }

    fn raw(&self) -> reqwest::Client { reqwest::Client::builder().unix_socket(self.daemon.socket()).build().unwrap() }
}

#[track_caller]
fn refused<T: std::fmt::Debug>(result: Result<T, FsClientError>, status: u16, code: &str, errno: FsError) {
    match result {
        Err(FsClientError::Api { status: got_status, code: got_code, errno: got_errno, .. }) => {
            assert_eq!((got_status, got_code.as_str(), got_errno), (status, code, errno.errno()));
        }
        other => panic!("expected {status} {code}, got {other:?}"),
    }
}

async fn raw_refused(response: reqwest::Response, status: u16, code: &str, errno: FsError) {
    assert_eq!(response.status().as_u16(), status);
    let body = response.bytes().await.unwrap();
    let error: fs::FsErrorBody = serde_json::from_slice(&body).unwrap();
    assert_eq!((error.error.code.as_str(), error.error.errno), (code, errno.errno()));
}

fn route(session: &FsClient, operation: &str) -> String { format!("http://localhost/v1/fs/{}/{operation}", session.info().id) }

fn post(http: &reqwest::Client, session: &FsClient, operation: &str, value: serde_json::Value) -> reqwest::RequestBuilder {
    http.post(route(session, operation)).header("x-voidfs-generation", session.info().generation)
        .header("content-type", "application/json").body(serde_json::to_vec(&value).unwrap())
}

async fn next(watch: &mut InvalidationWatch) -> fs::InvalidationEvent {
    tokio::time::timeout(Duration::from_secs(15), watch.next()).await.expect("bounded invalidation wait").expect("watch remains open").unwrap()
}

async fn matching(watch: &mut InvalidationWatch, ok: impl Fn(&fs::InvalidationEvent) -> bool) -> fs::InvalidationEvent {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop { let event = watch.next().await.expect("watch remains open").unwrap(); if ok(&event) { return event; } }
    }).await.expect("the requested invalidation reaches the socket")
}

async fn until(what: &str, mut ok: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !ok().await { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.expect(what);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binary_reads_and_handle_metadata_cross_the_unix_socket() {
    let bytes = Bytes::from_static(b"\0\xffA\0\x80B\n");
    let f = Fixture::new("binary", &[("data", bytes.clone())]).await;
    let session = f.client.session("drive", true).await.unwrap();
    let info = session.info();
    assert_eq!((info.version, info.read_only, info.max_io, info.max_entries), (fs::VERSION, true, fs::MAX_IO, fs::MAX_ENTRIES));
    let attr = session.lookup(info.root, "data").await.unwrap();
    assert_eq!((attr.kind, attr.size, attr.mode, attr.sync), (Kind::File, bytes.len() as u64, 0o644, Sync::Saved));
    assert_eq!(session.getattr(attr.ino).await.unwrap(), attr);
    let opened = session.open(attr.ino, false).await.unwrap();
    assert_eq!(opened.attr, attr);
    assert_eq!(session.handle_attr(opened.fh).await.unwrap(), attr);
    assert_eq!(session.read(opened.fh, 0, 64).await.unwrap(), bytes);
    assert_eq!(session.read(opened.fh, 1, 4).await.unwrap(), Bytes::from_static(b"\xffA\0\x80"));
    assert!(session.read(opened.fh, 7, 9).await.unwrap().is_empty());
    assert!(session.read(opened.fh, 0, 0).await.unwrap().is_empty());
    session.close(opened.fh).await.unwrap();
    refused(session.read(opened.fh, 0, 1).await, 400, "BadHandle", FsError::BadHandle);
    session.release().await.unwrap();
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn directory_pages_preserve_exact_names_and_a_stable_generation() {
    let f = Fixture::new("pages", &[("z", Bytes::new()), ("A", Bytes::new()), ("cafe\u{301}", Bytes::new()), ("folder/x", Bytes::new()), ("é", Bytes::new())]).await;
    let session = f.client.session("drive", true).await.unwrap();
    let root = session.info().root;
    assert_eq!(session.getattr(root).await.unwrap().kind, Kind::Folder);
    let first = session.readdir(root, None, 2).await.unwrap();
    assert_eq!(first.entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["A", "cafe\u{301}"]);
    let second = session.readdir(root, Some(&first.entries.last().unwrap().0), 2).await.unwrap();
    assert_eq!(second.entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["folder", "z"]);
    let last = session.readdir(root, Some("z"), 2).await.unwrap();
    assert_eq!(last.entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["é"]);
    assert_eq!((first.generation, second.generation), (last.generation, last.generation));
    assert!(session.readdir(root, Some("é"), 2).await.unwrap().entries.is_empty());
    let equivalent = session.lookup(root, "café").await.unwrap();
    assert_eq!(equivalent.ino, first.entries[1].1.ino);
    let folder = second.entries[0].1.ino;
    assert_eq!(session.readdir(folder, None, 10).await.unwrap().entries[0].0, "x");
    session.release().await.unwrap();
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_namespace_errors_keep_their_errno_on_the_wire() {
    let f = Fixture::new("errno", &[("data", Bytes::from_static(b"x"))]).await;
    let session = f.client.session("drive", true).await.unwrap();
    let root = session.info().root;
    let attr = session.lookup(root, "data").await.unwrap();
    refused(session.lookup(root, "absent").await, 404, "NotFound", FsError::NotFound);
    refused(session.lookup(root, "a/b").await, 400, "InvalidName", FsError::InvalidName);
    refused(session.open(root, false).await, 400, "IsDir", FsError::IsDir);
    refused(session.readdir(attr.ino, None, 1).await, 400, "NotDir", FsError::NotDir);
    refused(session.getattr(0).await, 409, "Stale", FsError::Stale);
    refused(session.readlink(attr.ino).await, 400, "InvalidName", FsError::InvalidName);
    session.release().await.unwrap();
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logical_read_only_sessions_share_staged_bytes_but_reject_mutation() {
    let f = Fixture::new("shared", &[("data", Bytes::from_static(b"abcdefgh"))]).await;
    f.client.pause(&Scope::All).await.unwrap();
    let reader = f.client.session("drive", true).await.unwrap();
    let writer = f.client.session("drive", false).await.unwrap();
    assert_eq!((reader.info().root, reader.info().generation), (writer.info().root, writer.info().generation));
    assert_ne!(reader.info().id, writer.info().id);
    let attr = reader.lookup(reader.info().root, "data").await.unwrap();
    let old = reader.open(attr.ino, false).await.unwrap();
    let rw = writer.open(attr.ino, true).await.unwrap();
    assert_eq!(writer.write(rw.fh, 2, Bytes::from_static(b"\0\xff")).await.unwrap(), 2);
    assert_eq!(reader.read(old.fh, 0, 8).await.unwrap(), Bytes::from_static(b"ab\0\xffefgh"));
    assert_eq!(reader.handle_attr(old.fh).await.unwrap().sync, Sync::Pending);
    refused(reader.open(attr.ino, true).await, 403, "ReadOnly", FsError::ReadOnly);
    refused(reader.write(old.fh, 0, Bytes::from_static(b"x")).await, 403, "ReadOnly", FsError::ReadOnly);
    refused(reader.truncate(old.fh, 0).await, 403, "ReadOnly", FsError::ReadOnly);
    writer.truncate(rw.fh, 10).await.unwrap();
    assert_eq!(reader.handle_attr(old.fh).await.unwrap().size, 10);
    assert_eq!(reader.read(old.fh, 0, 16).await.unwrap(), Bytes::from_static(b"ab\0\xffefgh\0\0"));
    writer.fsync(rw.fh).await.unwrap();
    assert_eq!(f.remote.get_object("drive", "data", Default::default()).await.unwrap().body, Bytes::from_static(b"abcdefgh"));
    assert!(f.client.status().await.unwrap().uploads.paused);
    writer.close(rw.fh).await.unwrap();
    reader.close(old.fh).await.unwrap();
    reader.release().await.unwrap();
    writer.release().await.unwrap();
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_ownership_and_release_are_per_logical_session() {
    let f = Fixture::new("handles", &[("data", Bytes::from_static(b"hello"))]).await;
    let a = f.client.session("drive", true).await.unwrap();
    let b = f.client.session("drive", true).await.unwrap();
    let ino = a.lookup(a.info().root, "data").await.unwrap().ino;
    let ah = a.open(ino, false).await.unwrap().fh;
    let bh = b.open(ino, false).await.unwrap().fh;
    assert_ne!(ah, bh);
    refused(b.read(ah, 0, 5).await, 400, "BadHandle", FsError::BadHandle);
    refused(b.handle_attr(ah).await, 400, "BadHandle", FsError::BadHandle);
    refused(b.close(ah).await, 400, "BadHandle", FsError::BadHandle);
    a.release().await.unwrap();
    refused(a.read(ah, 0, 5).await, 409, "StaleSession", FsError::Stale);
    refused(b.read(ah, 0, 5).await, 400, "BadHandle", FsError::BadHandle);
    assert_eq!(b.read(bh, 0, 5).await.unwrap(), Bytes::from_static(b"hello"));
    b.close(bh).await.unwrap();
    refused(b.close(bh).await, 400, "BadHandle", FsError::BadHandle);
    b.release().await.unwrap();
    f.daemon.stop().await;
    drop(voidfs_client::Store::open(f.dir.path()).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drive_alias_and_stable_id_reuse_the_same_core_epoch() {
    let f = Fixture::new("alias", &[("data", Bytes::from_static(b"a"))]).await;
    let id = f.remote.describe_drive("drive").await.unwrap().drive_id;
    let alias = f.client.session("drive", true).await.unwrap();
    let stable = f.client.session(&id, false).await.unwrap();
    assert_eq!((alias.info().drive.as_str(), stable.info().drive.as_str()), ("drive", "drive"));
    assert_eq!((alias.info().root, alias.info().generation), (stable.info().root, stable.info().generation));
    assert_eq!(alias.lookup(alias.info().root, "data").await.unwrap(), stable.lookup(stable.info().root, "data").await.unwrap());
    alias.release().await.unwrap();
    let still = f.client.session(&id, true).await.unwrap();
    assert_eq!(still.info().generation, stable.info().generation);
    still.release().await.unwrap();
    stable.release().await.unwrap();
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_generation_and_json_schema_are_checked_before_dispatch() {
    let f = Fixture::new("protocol", &[]).await;
    let http = f.raw();
    let create = |body: serde_json::Value| http.post("http://localhost/v1/fs/sessions").header("content-type", "application/json").body(serde_json::to_vec(&body).unwrap());
    raw_refused(create(serde_json::json!({"version": fs::VERSION + 1, "drive":"drive", "readOnly":true})).send().await.unwrap(), 426, "UnsupportedVersion", FsError::Unsupported).await;
    raw_refused(create(serde_json::json!({"version": fs::VERSION, "drive":"drive", "readOnly":true, "extra":1})).send().await.unwrap(), 400, "InvalidArgument", FsError::InvalidArgument).await;
    let session = f.client.session("drive", true).await.unwrap();
    raw_refused(http.post(route(&session, "getattr")).header("content-type", "application/json").body("{} ").send().await.unwrap(), 400, "InvalidArgument", FsError::InvalidArgument).await;
    raw_refused(http.post(route(&session, "getattr")).header("x-voidfs-generation", session.info().generation + 1).header("content-type", "application/json").body(format!("{{\"ino\":{}}}", session.info().root)).send().await.unwrap(), 409, "StaleGeneration", FsError::Stale).await;
    raw_refused(post(&http, &session, "getattr", serde_json::json!({"ino":session.info().root,"extra":true})).send().await.unwrap(), 400, "InvalidArgument", FsError::InvalidArgument).await;
    raw_refused(http.post(route(&session, "getattr")).header("x-voidfs-generation", session.info().generation).header("content-type", "application/json").body("{".repeat(fs::MAX_JSON + 1)).send().await.unwrap(), 413, "TooLarge", FsError::InvalidArgument).await;
    assert!(session.getattr(session.info().root).await.is_ok());
    session.release().await.unwrap();
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_io_and_page_limits_refuse_work_without_changing_bytes() {
    let f = Fixture::new("limits", &[("data", Bytes::from_static(b"original"))]).await;
    let session = f.client.session("drive", false).await.unwrap();
    let fh = session.open(session.lookup(session.info().root, "data").await.unwrap().ino, true).await.unwrap().fh;
    let http = f.raw();
    raw_refused(http.get(format!("{}?fh={fh}&offset=0&length={}", route(&session, "read"), fs::MAX_IO + 1)).header("x-voidfs-generation", session.info().generation).send().await.unwrap(), 413, "TooLarge", FsError::InvalidArgument).await;
    raw_refused(http.put(format!("{}?fh={fh}&offset=0", route(&session, "write"))).header("x-voidfs-generation", session.info().generation).header("content-type", "application/octet-stream").body(vec![42; fs::MAX_IO as usize + 1]).send().await.unwrap(), 413, "TooLarge", FsError::InvalidArgument).await;
    raw_refused(post(&http, &session, "readdir", serde_json::json!({"ino":session.info().root,"after":null,"limit":fs::MAX_ENTRIES+1})).send().await.unwrap(), 400, "InvalidArgument", FsError::InvalidArgument).await;
    raw_refused(post(&http, &session, "readdir", serde_json::json!({"ino":session.info().root,"after":null,"limit":0})).send().await.unwrap(), 400, "InvalidArgument", FsError::InvalidArgument).await;
    raw_refused(http.get(format!("{}?fh={fh}&offset={}&length=1", route(&session, "read"), u64::MAX)).header("x-voidfs-generation", session.info().generation).send().await.unwrap(), 400, "InvalidArgument", FsError::InvalidArgument).await;
    assert_eq!(session.read(fh, 0, 16).await.unwrap(), Bytes::from_static(b"original"));
    assert_eq!(session.handle_attr(fh).await.unwrap().sync, Sync::Saved);
    session.close(fh).await.unwrap();
    session.release().await.unwrap();
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incomplete_request_bodies_consume_call_slots_and_controls_stay_responsive() {
    let f = Fixture::new("admission", &[]).await;
    let session = f.client.session("drive", true).await.unwrap();
    let mut held = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while held.len() < fs::MAX_CALLS {
        assert!(Instant::now() < deadline, "32 incomplete bodies are admitted");
        let mut stream = std::os::unix::net::UnixStream::connect(f.daemon.socket()).unwrap();
        stream.set_read_timeout(Some(deadline.saturating_duration_since(Instant::now()))).unwrap();
        write!(stream, "POST /v1/fs/{}/getattr HTTP/1.1\r\nHost: localhost\r\nx-voidfs-generation: {}\r\nContent-Type: application/json\r\nContent-Length: 32\r\nExpect: 100-continue\r\n\r\n", session.info().id, session.info().generation).unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            assert!(header.len() < 4096, "bounded admission response header");
            let mut byte = [0];
            stream.read_exact(&mut byte).expect("bounded admission response");
            header.push(byte[0]);
        }
        if header.starts_with(b"HTTP/1.1 100 ") {
            held.push(stream);
        } else {
            assert!(header.starts_with(b"HTTP/1.1 503 "), "unexpected admission response: {}", String::from_utf8_lossy(&header));
            drop(stream);
            tokio::task::yield_now().await;
        }
    }
    let http = f.raw();
    let busy = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = post(&http, &session, "getattr", serde_json::json!({"ino":session.info().root})).send().await.unwrap();
            if response.status().as_u16() == 503 { return response; }
            assert!(response.status().is_success());
            response.bytes().await.unwrap();
            tokio::task::yield_now().await;
        }
    }).await.expect("all incomplete request bodies hold their admission slots");
    raw_refused(busy, 503, "Busy", FsError::Again).await;
    tokio::time::timeout(Duration::from_secs(3), f.client.status()).await.expect("status remains responsive when filesystem calls are full").unwrap();
    drop(held);
    until("dropped request bodies return the call permits", async || session.getattr(session.info().root).await.is_ok()).await;
    session.release().await.unwrap();
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_watch_invalidates_metadata_while_open_readers_keep_their_snapshot() {
    let f = Fixture::new("feed", &[("data", Bytes::from_static(b"old bytes"))]).await;
    let session = f.client.session("drive", true).await.unwrap();
    let original = session.lookup(session.info().root, "data").await.unwrap();
    let old = session.open(original.ino, false).await.unwrap();
    let mut watch = session.watch().await.unwrap();
    let initial = next(&mut watch).await;
    assert!(initial.resync && initial.invalidations.contains(&fs::Invalidation::All));
    let result = f.remote.put_object("drive", "data", "new bytes", Default::default()).await.unwrap();
    let event = matching(&mut watch, |event| event.generation > initial.generation && event.invalidations.contains(&fs::Invalidation::Object("data".into()))).await;
    assert!(event.seq > initial.seq);
    assert!(event.inodes.contains(&original.ino));
    until("feed invalidation makes the new version discoverable", async || session.lookup(session.info().root, "data").await.unwrap().version_id.as_ref() == Some(&result.version_id)).await;
    let current = session.lookup(session.info().root, "data").await.unwrap();
    assert_eq!(current.ino, original.ino);
    let new = session.open(current.ino, false).await.unwrap();
    assert_eq!(session.read(old.fh, 0, 64).await.unwrap(), Bytes::from_static(b"old bytes"));
    assert_eq!(session.handle_attr(old.fh).await.unwrap(), original);
    assert_eq!(session.read(new.fh, 0, 64).await.unwrap(), Bytes::from_static(b"new bytes"));
    let mut resumed = session.watch().await.unwrap();
    let resync = next(&mut resumed).await;
    assert!(resync.resync && resync.invalidations.contains(&fs::Invalidation::All));
    assert!(resync.generation >= event.generation);
    session.close(old.fh).await.unwrap();
    session.close(new.fh).await.unwrap();
    session.release().await.unwrap();
    drop((watch, resumed));
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_mutations_notify_other_sessions_after_the_new_bytes_are_visible() {
    let f = Fixture::new("localwatch", &[("data", Bytes::from_static(b"abcd"))]).await;
    f.client.pause(&Scope::All).await.unwrap();
    let writer = f.client.session("drive", false).await.unwrap();
    let reader = f.client.session("drive", true).await.unwrap();
    let ino = reader.lookup(reader.info().root, "data").await.unwrap().ino;
    let old = reader.open(ino, false).await.unwrap().fh;
    let rw = writer.open(ino, true).await.unwrap().fh;
    let mut watch = reader.watch().await.unwrap();
    let initial = next(&mut watch).await;
    writer.write(rw, 1, Bytes::from_static(b"X")).await.unwrap();
    let event = matching(&mut watch, |event| event.generation > initial.generation && event.inodes.contains(&ino) && event.invalidations.contains(&fs::Invalidation::Object("data".into()))).await;
    assert!(!event.resync && event.invalidations == [fs::Invalidation::Object("data".into())]);
    assert_eq!(reader.read(old, 0, 4).await.unwrap(), Bytes::from_static(b"aXcd"));
    writer.truncate(rw, 2).await.unwrap();
    let resized = matching(&mut watch, |next| next.generation > event.generation && next.inodes.contains(&ino)).await;
    assert!(resized.generation > event.generation);
    assert_eq!(reader.handle_attr(old).await.unwrap().size, 2);
    assert_eq!(reader.read(old, 0, 8).await.unwrap(), Bytes::from_static(b"aX"));
    writer.close(rw).await.unwrap();
    reader.close(old).await.unwrap();
    writer.release().await.unwrap();
    reader.release().await.unwrap();
    drop(watch);
    f.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_ends_watches_and_stop_releases_every_store_lease() {
    let f = Fixture::new("release", &[("data", Bytes::from_static(b"x"))]).await;
    let session = f.client.session("drive", true).await.unwrap();
    let ino = session.lookup(session.info().root, "data").await.unwrap().ino;
    let fh = session.open(ino, false).await.unwrap().fh;
    let mut watch = session.watch().await.unwrap();
    next(&mut watch).await;
    session.release().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), watch.next()).await.expect("release ends the stream").is_none());
    refused(session.handle_attr(fh).await, 409, "StaleSession", FsError::Stale);
    let alive = f.client.session("drive", true).await.unwrap();
    let mut stopped = alive.watch().await.unwrap();
    next(&mut stopped).await;
    f.daemon.stop().await;
    let ended = tokio::time::timeout(Duration::from_secs(5), stopped.next()).await.expect("stop ends active watch connections");
    assert!(ended.is_none() || ended.is_some_and(|event| event.is_err()));
    drop(voidfs_client::Store::open(f.dir.path()).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_rejects_old_sessions_and_handles_and_recovers_staged_bytes() {
    let f = Fixture::new("restart", &[("data", Bytes::from_static(b"abcd"))]).await;
    f.client.pause(&Scope::All).await.unwrap();
    let old = f.client.session("drive", false).await.unwrap();
    let ino = old.lookup(old.info().root, "data").await.unwrap().ino;
    let fh = old.open(ino, true).await.unwrap().fh;
    old.write(fh, 1, Bytes::from_static(b"Z")).await.unwrap();
    old.fsync(fh).await.unwrap();
    let generation = old.info().generation;
    f.daemon.stop().await;
    let daemon = Daemon::start(config(&f.server.endpoint, f.dir.path())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    refused(old.read(fh, 0, 4).await, 409, "StaleSession", FsError::Stale);
    let fresh = client.session("drive", true).await.unwrap();
    assert!(fresh.info().generation > generation);
    let reopened = fresh.open(fresh.lookup(fresh.info().root, "data").await.unwrap().ino, false).await.unwrap().fh;
    assert_eq!(fresh.read(reopened, 0, 4).await.unwrap(), Bytes::from_static(b"aZcd"));
    refused(fresh.read(fh, 0, 4).await, 409, "Stale", FsError::Stale);
    fresh.close(reopened).await.unwrap();
    fresh.release().await.unwrap();
    daemon.stop().await;
    drop(voidfs_client::Store::open(f.dir.path()).unwrap());
}

struct LinkProxy { endpoint: String, task: tokio::task::JoinHandle<()> }
impl Drop for LinkProxy { fn drop(&mut self) { self.task.abort(); } }

impl LinkProxy {
    async fn start(upstream: &str, target: &str) -> Self {
        let state = Arc::new((upstream.to_owned(), target.to_owned(), reqwest::Client::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback(link_proxy).with_state(state);
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        Self { endpoint, task }
    }
}

async fn link_proxy(axum::extract::State(state): axum::extract::State<Arc<(String, String, reqwest::Client)>>, request: axum::extract::Request) -> axum::response::Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path_and_query().map_or("/", |value| value.as_str());
    let listing = parts.uri.query().is_some_and(|query| query.split('&').any(|field| field.split('=').next() == Some("x-voidfs-list")));
    let body = axum::body::to_bytes(body, fs::MAX_IO as usize).await.unwrap();
    let mut request = state.2.request(parts.method, format!("{}{path}", state.0));
    for (name, value) in &parts.headers { request = request.header(name, value); }
    let response = request.body(body).send().await.unwrap();
    let mut output = axum::response::Response::builder().status(response.status().as_u16());
    for (name, value) in response.headers() {
        if !matches!(name.as_str(), "content-length" | "transfer-encoding" | "connection") { output = output.header(name, value); }
    }
    let body = if listing && response.status().is_success() {
        let mut value: serde_json::Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        value["entries"].as_array_mut().unwrap().push(serde_json::json!({"name":"link","kind":"symlink","objectId":"test-link","versionId":"1.0","size":state.1.len(),"etag":"link-etag","target":state.1}));
        Body::from(serde_json::to_vec(&value).unwrap())
    } else {
        Body::from_stream(futures::stream::unfold(response, |mut response| async move {
            match response.chunk().await { Ok(Some(bytes)) => Some((Ok::<_, reqwest::Error>(bytes), response)), Ok(None) => None, Err(error) => Some((Err(error), response)) }
        }))
    };
    output.body(body).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symlink_targets_are_exact_metadata_and_are_never_opened_as_files() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    let target = "../cafe\u{301}/../target";
    let proxy = LinkProxy::start(&server.endpoint, target).await;
    let dir = state_dir("link");
    let daemon = Daemon::start(config(&proxy.endpoint, dir.path())).await.unwrap();
    let session = DaemonClient::new(daemon.socket()).session("drive", true).await.unwrap();
    let link = session.lookup(session.info().root, "link").await.unwrap();
    assert_eq!((link.kind, link.target.as_deref()), (Kind::Symlink, Some(target)));
    assert_eq!(session.readlink(link.ino).await.unwrap(), target);
    refused(session.open(link.ino, false).await, 501, "Unsupported", FsError::Unsupported);
    session.release().await.unwrap();
    daemon.stop().await;
}

fn distribution(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let total: Duration = samples.iter().copied().sum();
    println!("{label}: n={} mean={:?} p50={:?} p99={:?} max={:?}", samples.len(), total / samples.len() as u32, samples[samples.len()/2], samples[samples.len()*99/100], samples.last().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measurement: cargo test -p voidfs-daemon --test sessions measure_warm_metadata_and_cached_read_socket_hops -- --ignored --nocapture"]
async fn measure_warm_metadata_and_cached_read_socket_hops() {
    let bytes = Bytes::from(vec![0x5a; 4096]);
    let f = Fixture::new("bench", &[("data", bytes.clone())]).await;
    let session = f.client.session("drive", true).await.unwrap();
    let attr = session.lookup(session.info().root, "data").await.unwrap();
    let fh = session.open(attr.ino, false).await.unwrap().fh;
    assert_eq!(session.read(fh, 0, 4096).await.unwrap(), bytes);
    let mut metadata = Vec::new();
    let mut reads = Vec::new();
    for _ in 0..1000 {
        let started = Instant::now();
        assert_eq!(session.getattr(attr.ino).await.unwrap().ino, attr.ino);
        metadata.push(started.elapsed());
        let started = Instant::now();
        assert_eq!(session.read(fh, 0, 4096).await.unwrap(), bytes);
        reads.push(started.elapsed());
    }
    distribution("warm getattr UDS", metadata);
    distribution("cached 4KiB read UDS", reads);
    session.close(fh).await.unwrap();
    session.release().await.unwrap();
    f.daemon.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_release_keeps_its_handles_owned_for_a_later_retry() {
    let f = Fixture::new("retry", &[("data", Bytes::from_static(b"base"))]).await;
    f.client.pause(&Scope::All).await.unwrap();
    let session = f.client.session("drive", false).await.unwrap();
    let attr = session.lookup(session.info().root, "data").await.unwrap();
    let fh = session.open(attr.ino, true).await.unwrap().fh;
    session.write(fh, 0, Bytes::from_static(b"edit")).await.unwrap();
    let staged = f.dir.path().join("mount-stage");
    let held = f.dir.path().join("mount-stage-held");
    std::fs::rename(&staged, &held).unwrap();
    refused(session.release().await, 500, "Io", FsError::Io(String::new()));
    assert_eq!(session.handle_attr(fh).await.unwrap().ino, attr.ino, "a failed close must retain its owned handle");
    std::fs::rename(&held, &staged).unwrap();
    assert_eq!(session.read(fh, 0, 4).await.unwrap(), Bytes::from_static(b"edit"));
    let mut watch = session.watch().await.unwrap();
    assert!(next(&mut watch).await.resync, "the retained session can subscribe again");
    session.release().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), watch.next()).await.unwrap().is_none());
    refused(session.handle_attr(fh).await, 409, "StaleSession", FsError::Stale);
    f.daemon.stop().await;
}
