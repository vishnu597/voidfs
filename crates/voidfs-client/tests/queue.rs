// SPDX-License-Identifier: Apache-2.0
//! The write journal and the upload queue, against a server in this process.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{Fault, Proxy, client_for, server};
use voidfs_client::{Attrs, Base, Import, Op, Queue, QueueConfig, Scope, State, Store};
use voidfs_sdk::{Client, Config, ReadOptions};

const MIB: u64 = 1 << 20;

fn bytes_of(seed: u64, n: u64) -> Bytes {
    (0..n).map(|i| (i.wrapping_add(seed << 40).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8).collect::<Vec<_>>().into()
}

fn config() -> QueueConfig {
    QueueConfig { part_size: 5 * MIB, multipart_from: 12 * MIB, retry_max: Duration::from_millis(200), ..Default::default() }
}

async fn open(dir: &Path, c: &Client, cfg: QueueConfig) -> Queue {
    Queue::open(Arc::new(Store::open(dir).unwrap()), c.clone(), cfg).await.unwrap()
}

async fn text(c: &Client, key: &str) -> Option<String> {
    match c.get_object("drv", key, ReadOptions::default()).await {
        Ok(o) => Some(String::from_utf8_lossy(&o.body).into_owned()),
        Err(e) if e.status() == Some(404) => None,
        Err(e) => panic!("{e}"),
    }
}

/// Requests of `method` to `key` (a path ending, with its query) the proxy saw.
fn count(p: &Proxy, method: &str, needle: &str) -> usize {
    p.seen().iter().filter(|r| r.starts_with(&format!("{method} ")) && r.contains(needle)).count()
}

async fn setup() -> (voidfs_server::test_server::TestServer, Proxy, Client, Client) {
    let s = server().await;
    let proxy = Proxy::start(&s.endpoint).await;
    let direct = client_for(&s.endpoint, Config::default());
    let c = client_for(&proxy.endpoint, Config::default());
    direct.create_drive("drv", Default::default()).await.unwrap();
    (s, proxy, direct, c)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changes_are_durable_and_publish_in_order_after_a_restart() {
    let (_s, _p, direct, c) = setup().await;
    direct.put_object("drv", "c.txt", "gone soon", Default::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    {
        let q = open(dir.path(), &c, config()).await;
        q.pause(Scope::All).await.unwrap();
        q.put("drv", "a.txt", "one".into(), Base::Absent, Attrs::default()).await.unwrap();
        q.write("drv", "a.txt", 1, "X".into(), Base::Any).await.unwrap();
        q.rename("drv", "a.txt", "b.txt", false, Base::Any).await.unwrap();
        q.write("drv", "b.txt", 0, "Z".into(), Base::Any).await.unwrap();
        q.delete("drv", "c.txt", Base::Any).await.unwrap();
        q.folder("drv", "d", Attrs::default()).await.unwrap();
        q.close().await;
    }
    assert_eq!(text(&direct, "c.txt").await.as_deref(), Some("gone soon"), "nothing went while paused");
    let q = open(dir.path(), &c, config()).await;
    let st = q.status().await.unwrap();
    assert_eq!((st.paused, st.unpublished, st.items.len()), (true, 6, 6), "the journal outlived the queue");
    q.resume(Scope::All).await.unwrap();
    q.settle().await;
    assert_eq!(text(&direct, "b.txt").await.as_deref(), Some("ZXe"));
    assert_eq!((text(&direct, "a.txt").await, text(&direct, "c.txt").await), (None, None));
    assert_eq!(direct.head_object("drv", "d/", ReadOptions::default()).await.unwrap().kind, voidfs_sdk::Kind::Folder);
    let st = q.status().await.unwrap();
    assert!(st.items.iter().all(|i| i.state == State::Done), "{:?}", st.items);
    assert_eq!(st.unpublished, 0);
    assert_eq!(std::fs::read_dir(dir.path().join("journal")).unwrap().count(), 0, "published bytes are deleted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_put_takes_the_writes_after_it_and_writes_become_one_patch() {
    let (_s, p, direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let q = open(dir.path(), &c, config()).await;
    q.pause(Scope::All).await.unwrap();
    q.put("drv", "f", "hello world".into(), Base::Absent, Attrs::default()).await.unwrap();
    q.write("drv", "f", 0, "J".into(), Base::Any).await.unwrap();
    q.write("drv", "f", 11, "!!".into(), Base::Any).await.unwrap();
    q.truncate("drv", "f", 12, Base::Any).await.unwrap();
    p.clear();
    q.resume(Scope::All).await.unwrap();
    q.settle().await;
    assert_eq!(text(&direct, "f").await.as_deref(), Some("Jello world!"));
    assert_eq!((count(&p, "PUT", "/drv/f"), count(&p, "POST", "/drv/f")), (1, 0), "one put: {:?}", p.seen());
    let versions: Vec<_> = q.status().await.unwrap().items.into_iter().map(|i| i.version).collect();
    assert!(versions.iter().all(|v| v.is_some() && *v == versions[0]), "one version for all four: {versions:?}");

    q.pause(Scope::All).await.unwrap();
    q.write("drv", "f", 1, "E".into(), Base::Any).await.unwrap();
    q.write("drv", "f", 6, "W".into(), Base::Any).await.unwrap();
    q.truncate("drv", "f", 11, Base::Any).await.unwrap();
    q.write("drv", "f", 0, "j".into(), Base::Any).await.unwrap();
    p.clear();
    q.resume(Scope::All).await.unwrap();
    q.settle().await;
    assert_eq!(text(&direct, "f").await.as_deref(), Some("jEllo World"));
    assert_eq!(count(&p, "POST", "x-voidfs-patch"), 2, "the writes and the truncate as one patch, the write after the truncate as another: {:?}", p.seen());
    assert_eq!(direct.list_versions("drv", "f", false).await.unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflict_publishes_the_local_version_and_says_so() {
    let (_s, _p, direct, c) = setup().await;
    let v1 = direct.put_object("drv", "doc", "aaaa", Default::default()).await.unwrap().version_id;
    let v2 = direct.put_object("drv", "doc", "bbbb", Default::default()).await.unwrap().version_id;
    let dir = tempfile::tempdir().unwrap();
    let q = open(dir.path(), &c, config()).await;
    // A put based on v1, after someone else wrote v2.
    let id = q.put("drv", "doc", "mine".into(), Base::Version(v1.clone()), Attrs::default()).await.unwrap();
    q.settle().await;
    let item = q.status().await.unwrap().items.into_iter().find(|i| i.id == id).unwrap();
    assert_eq!((item.state, item.conflict.as_deref()), (State::Done, Some(v2.as_str())));
    assert_eq!(text(&direct, "doc").await.as_deref(), Some("mine"));
    let history: Vec<String> = direct.list_versions("drv", "doc", false).await.unwrap().into_iter().map(|v| v.version_id).collect();
    assert_eq!(&history[..2], [v1.clone(), v2.clone()], "nothing lost");

    // A write based on v1: the local version is v1 with the write.
    let id = q.write("drv", "doc", 0, "L".into(), Base::Version(v1)).await.unwrap();
    q.settle().await;
    let item = q.status().await.unwrap().items.into_iter().find(|i| i.id == id).unwrap();
    assert_eq!((item.state, item.conflict.as_deref()), (State::Done, Some(history.last().unwrap().as_str())));
    assert_eq!(text(&direct, "doc").await.as_deref(), Some("Laaa"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_put_whose_answer_was_lost_is_known_as_its_own() {
    let s = server().await;
    let p = Proxy::start(&s.endpoint).await;
    let direct = client_for(&s.endpoint, Config::default());
    direct.create_drive("drv", Default::default()).await.unwrap();
    let c = client_for(&p.endpoint, Config { timeout: Duration::from_millis(400), ..Default::default() });
    let dir = tempfile::tempdir().unwrap();
    let q = open(dir.path(), &c, config()).await;
    p.fault(Fault::Hang(Duration::from_millis(1500)));
    let id = q.put("drv", "new", "made once".into(), Base::Absent, Attrs::default()).await.unwrap();
    q.settle().await;
    let item = q.status().await.unwrap().items.into_iter().find(|i| i.id == id).unwrap();
    assert_eq!((item.state, item.conflict.as_deref()), (State::Done, None), "{item:?}");
    assert_eq!(direct.list_versions("drv", "new", false).await.unwrap().len(), 1, "landed once, not published over itself");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pause_stops_an_upload_and_a_resume_goes_on_from_its_parts() {
    let (_s, p, direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.bin");
    let body = bytes_of(1, 30 * MIB);
    std::fs::write(&src, &body).unwrap();
    let q = open(dir.path(), &c, QueueConfig { parts_at_once: 2, ..config() }).await;
    q.set_bandwidth(Some(10 * MIB)).await.unwrap();
    let batch = q.import("big", vec![Import { path: src.clone(), drive: "drv".into(), key: "big.bin".into() }]).await.unwrap();
    // Paused once a part is up, however fast the machine is.
    let deadline = Instant::now() + Duration::from_secs(20);
    while q.status().await.unwrap().batches[0].sent < 5 * MIB {
        assert!(Instant::now() < deadline, "no part went up");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    q.pause(Scope::Batch(batch)).await.unwrap();
    q.settle().await;
    let st = q.status().await.unwrap();
    let b = &st.batches[0];
    assert!(b.paused && b.done == 0 && b.sent >= 5 * MIB && b.sent < 30 * MIB, "{b:?}");
    // Progress counts finished parts, of 5 MiB each.
    let finished = b.sent / (5 * MIB);
    let before = count(&p, "PUT", "partNumber=");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count(&p, "PUT", "partNumber="), before, "nothing goes while paused");
    q.set_bandwidth(None).await.unwrap();
    q.resume(Scope::Batch(batch)).await.unwrap();
    q.settle().await;
    let parts = count(&p, "PUT", "partNumber=") - before;
    assert!(parts <= 6 - finished as usize, "the {finished} parts it had were not sent again: {parts} part requests after the resume, for 6 parts");
    assert!(direct.get_object("drv", "big.bin", ReadOptions::default()).await.unwrap().body == body);
    assert_eq!(q.status().await.unwrap().batches[0].done, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_goes_on_with_a_multipart_upload() {
    let (_s, p, direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.bin");
    let body = bytes_of(2, 30 * MIB);
    std::fs::write(&src, &body).unwrap();
    {
        let q = open(dir.path(), &c, QueueConfig { parts_at_once: 1, ..config() }).await;
        q.set_bandwidth(Some(10 * MIB)).await.unwrap();
        q.import("big", vec![Import { path: src.clone(), drive: "drv".into(), key: "big.bin".into() }]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1700)).await;
        q.close().await;
    }
    let done_before = count(&p, "PUT", "partNumber=");
    assert!(done_before >= 2, "{done_before}");
    let q = open(dir.path(), &c, config()).await;
    assert_eq!(q.status().await.unwrap().bandwidth, Some(10 * MIB), "the limit is kept");
    q.set_bandwidth(None).await.unwrap();
    q.settle().await;
    let parts = count(&p, "PUT", "partNumber=");
    assert!(parts <= 6 + 1, "{parts} part requests for 6 parts across the restart");
    assert!(direct.get_object("drv", "big.bin", ReadOptions::default()).await.unwrap().body == body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_never_publishes_and_takes_what_was_built_on_it() {
    let (_s, p, direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let q = open(dir.path(), &c, config()).await;
    q.pause(Scope::All).await.unwrap();
    let put = q.put("drv", "x", "draft".into(), Base::Absent, Attrs::default()).await.unwrap();
    q.write("drv", "x", 0, "D".into(), Base::Any).await.unwrap();
    let other = q.put("drv", "y", "kept".into(), Base::Absent, Attrs::default()).await.unwrap();
    q.cancel(Scope::Entry(put)).await.unwrap();
    q.resume(Scope::All).await.unwrap();
    q.settle().await;
    assert_eq!((text(&direct, "x").await, text(&direct, "y").await.as_deref()), (None, Some("kept")));
    let st = q.status().await.unwrap();
    let states: Vec<_> = st.items.iter().map(|i| (i.op, i.state)).collect();
    assert_eq!(states, [(Op::Put, State::Cancelled), (Op::Write, State::Cancelled), (Op::Put, State::Done)]);
    assert!(st.items.iter().any(|i| i.id == other));

    // Cancelled part way, a large upload is abandoned and leaves no version.
    let src = dir.path().join("big.bin");
    std::fs::write(&src, bytes_of(3, 30 * MIB)).unwrap();
    q.set_bandwidth(Some(10 * MIB)).await.unwrap();
    let batch = q.import("big", vec![Import { path: src, drive: "drv".into(), key: "big.bin".into() }]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    q.cancel(Scope::Batch(batch)).await.unwrap();
    q.settle().await;
    assert_eq!(q.status().await.unwrap().batches.iter().find(|b| b.id == batch).unwrap().cancelled, 1);
    assert_eq!(text(&direct, "big.bin").await, None);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(count(&p, "DELETE", "uploadId=") >= 1, "the upload was aborted: {:?}", p.seen());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bandwidth_limit_holds() {
    let (_s, _p, _direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<Import> = (0..3)
        .map(|i| {
            let path = dir.path().join(format!("f{i}"));
            std::fs::write(&path, bytes_of(i, 4 * MIB)).unwrap();
            Import { path, drive: "drv".into(), key: format!("f{i}") }
        })
        .collect();
    let q = open(dir.path(), &c, config()).await;
    q.set_bandwidth(Some(4 * MIB)).await.unwrap();
    let t = Instant::now();
    q.import("three", files).await.unwrap();
    q.settle().await;
    let secs = t.elapsed().as_secs_f64();
    // Never faster than the limit, less its burst of a quarter second's worth; and not much
    // slower, with room for a slow machine's requests and journal writes.
    assert!((2.7..4.0).contains(&secs), "12 MiB at 4 MiB/s took {secs:.2} s");
    assert_eq!(q.status().await.unwrap().batches[0].done, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn imports_keep_their_modification_time_mode_and_extended_attributes() {
    use std::os::unix::fs::PermissionsExt;
    let (_s, _p, direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tagged.txt");
    std::fs::write(&path, "tagged").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let when = std::time::UNIX_EPOCH + Duration::from_micros(1_790_000_000_123_456);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
    xattr::set(&path, "user.tag", b"red").unwrap();
    let _ = xattr::set(&path, "com.apple.quarantine", b"0081;0;x;");
    let q = open(dir.path(), &c, config()).await;
    q.import("one", vec![Import { path, drive: "drv".into(), key: "tagged.txt".into() }]).await.unwrap();
    q.settle().await;
    let h = direct.head_object("drv", "tagged.txt", ReadOptions::default()).await.unwrap();
    assert_eq!((h.mtime.as_deref(), h.mode.as_deref()), (Some("2026-09-21T14:13:20.123456Z"), Some("0640")));
    let a = direct.attributes("drv", "tagged.txt", ReadOptions::default()).await.unwrap();
    assert_eq!(a.xattrs.get("user.tag").map(String::as_str), Some("cmVk"));
    assert!(!a.xattrs.contains_key("com.apple.quarantine"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixteen_go_at_once_and_one_keys_changes_wait_for_each_other() {
    let (_s, p, direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<Import> = (0..40)
        .map(|i| {
            let path = dir.path().join(format!("s{i}"));
            std::fs::write(&path, format!("small {i}")).unwrap();
            Import { path, drive: "drv".into(), key: format!("many/s{i}") }
        })
        .collect();
    let q = open(dir.path(), &c, config()).await;
    p.slow(Duration::from_millis(100));
    q.import("many", files).await.unwrap();
    q.settle().await;
    let most = p.most_at_once();
    assert!((12..=16).contains(&most), "{most} at once");
    assert_eq!(q.status().await.unwrap().batches[0].done, 40);
    assert_eq!(text(&direct, "many/s39").await.as_deref(), Some("small 39"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_publish_is_tried_again_and_holds_back_its_key() {
    let (_s, p, direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let q = open(dir.path(), &c, config()).await;
    q.pause(Scope::All).await.unwrap();
    q.put("drv", "k", "first".into(), Base::Absent, Attrs::default()).await.unwrap();
    q.rename("drv", "k", "k2", false, Base::Any).await.unwrap();
    // More than the SDK's three attempts: the publish fails, and is tried again later.
    for _ in 0..3 {
        p.fault(Fault::Status(503));
    }
    p.clear();
    q.resume(Scope::All).await.unwrap();
    q.settle().await;
    assert_eq!((text(&direct, "k").await, text(&direct, "k2").await.as_deref()), (None, Some("first")));
    let seen = p.seen();
    let puts = seen.iter().filter(|r| r.as_str() == "PUT /drv/k").count();
    let rename = seen.iter().position(|r| r.contains("x-voidfs-rename")).unwrap();
    assert_eq!(puts, 4, "three failed attempts, then one more: {seen:?}");
    assert!(seen[..rename].iter().filter(|r| r.as_str() == "PUT /drv/k").count() == 4, "the rename waited for the put: {seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_put_that_landed_as_the_client_stopped_is_not_sent_again() {
    let (_s, p, direct, c) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    {
        let q = open(dir.path(), &c, config()).await;
        // Forwarded, so it lands, but its answer is held past the client's stop.
        p.fault(Fault::Hang(Duration::from_secs(3)));
        q.put("drv", "k", "landed".into(), Base::Absent, Attrs::default()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        q.close().await;
    }
    assert_eq!(text(&direct, "k").await.as_deref(), Some("landed"));
    p.clear();
    let q = open(dir.path(), &c, config()).await;
    q.settle().await;
    let item = q.status().await.unwrap().items.remove(0);
    assert_eq!((item.state, item.conflict.as_deref()), (State::Done, None), "{item:?}");
    assert_eq!(count(&p, "PUT", "/drv/k"), 0, "known by its marker, not sent again: {:?}", p.seen());
    assert_eq!(direct.list_versions("drv", "k", false).await.unwrap().len(), 1);
}
