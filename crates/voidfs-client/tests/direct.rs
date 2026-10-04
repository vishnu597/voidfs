// SPDX-License-Identifier: Apache-2.0
//! The upload queue's direct uploads (protocol §4.11), against a server in this process whose
//! stand-in bucket binds what its URLs carry.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use common::{Fault, Proxy, client_for};
use voidfs_client::{Attrs, Base, Import, Queue, QueueConfig, State, Store};
use voidfs_sdk::{Client, Config, ReadOptions};
use voidfs_server::test_server::{FakeBucket, TestServer};

const MIB: u64 = 1 << 20;

fn bytes_of(seed: u64, n: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn config() -> QueueConfig {
    QueueConfig { part_size: 5 * MIB, multipart_from: 12 * MIB, retry_max: Duration::from_millis(200), ..Default::default() }
}

async fn open(dir: &Path, c: &Client) -> Queue {
    Queue::open(Arc::new(Store::open(dir).unwrap()), c.clone(), config()).await.unwrap()
}

/// A server with direct uploads, its bucket, a proxy in front of the server, an admin client
/// straight to the server and one through the proxy, with drive `drv`.
async fn setup(direct_uploads: bool) -> (TestServer, Option<FakeBucket>, Proxy, Client, Client) {
    let s = if direct_uploads { TestServer::with_direct_uploads(Vec::new()).await } else { TestServer::start().await }.unwrap();
    let proxy = Proxy::start(&s.endpoint).await;
    let straight = client_for(&s.endpoint, Config::default());
    let c = client_for(&proxy.endpoint, Config::default());
    straight.create_drive("drv", Default::default()).await.unwrap();
    let bucket = s.bucket.clone();
    (s, bucket, proxy, straight, c)
}

fn seen(p: &Proxy, needle: &str) -> usize {
    p.seen().iter().filter(|r| r.contains(needle)).count()
}

fn exactly(p: &Proxy, request: &str) -> usize {
    p.seen().iter().filter(|r| *r == request).count()
}

async fn read(c: &Client, key: &str) -> Bytes {
    c.get_object("drv", key, ReadOptions::default()).await.unwrap().body
}

/// An import that replaces a file the drive holds most of goes direct: only the shards around the
/// change reach the bucket, and the server gets a plan and a commit, not the file. The first
/// upload of that file, new bytes, went the ordinary way, as one put (a file uploaded in parts is
/// cut at their boundaries, which a direct upload's cut doesn't share).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_the_drive_mostly_holds_goes_direct() {
    let (_s, bucket, proxy, straight, c) = setup(true).await;
    let bucket = bucket.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cut.mov");
    let data = bytes_of(1, 11 * MIB);
    std::fs::write(&path, &data).unwrap();
    let q = open(&dir.path().join("state"), &c).await;
    let (_, before) = bucket.accepted();
    q.import("one", vec![Import { path: path.clone(), drive: "drv".into(), key: "cut.mov".into() }]).await.unwrap();
    q.settle().await;
    assert_eq!(read(&straight, "cut.mov").await, data);
    assert_eq!(bucket.accepted().1, before, "new bytes went to the server");
    assert_eq!((seen(&proxy, "x-voidfs-upload-plan"), seen(&proxy, "x-voidfs-upload-commit"), exactly(&proxy, "PUT /drv/cut.mov")), (1, 0, 1), "a plan, then a put");

    let mut edited = data.clone();
    edited[(5 * MIB) as usize..(5 * MIB) as usize + 64].copy_from_slice(&[9; 64]);
    std::fs::write(&path, &edited).unwrap();
    proxy.clear();
    // The commit's answer is held, so that the upload's progress shows while it waits.
    proxy.fault(Fault::Pass);
    proxy.fault(Fault::Hang(Duration::from_secs(2)));
    let sent = q.bytes_sent();
    q.import("two", vec![Import { path: path.clone(), drive: "drv".into(), key: "cut.mov".into() }]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while seen(&proxy, "x-voidfs-upload-commit") == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a commit");
    let item = q.status().await.unwrap().items.into_iter().find(|i| i.state != State::Done).expect("the upload, committing");
    assert_eq!(item.sent, item.size, "held shards count as sent, and the rest have been");
    q.settle().await;
    assert_eq!(read(&straight, "cut.mov").await, edited);
    let to_bucket = bucket.accepted().1 - before;
    assert!(to_bucket > 0 && to_bucket < 6 * MIB, "{to_bucket} bytes to the bucket for a change of 64");
    let all = q.bytes_sent() - sent;
    assert!(all < to_bucket + MIB, "{all} bytes sent in all");
    assert_eq!((seen(&proxy, "x-voidfs-upload-plan"), seen(&proxy, "x-voidfs-upload-commit"), seen(&proxy, "uploadId"), exactly(&proxy, "PUT /drv/cut.mov")), (1, 1, 0, 0));
    let st = q.status().await.unwrap();
    assert!(st.items.iter().all(|i| i.state == State::Done && i.sent == i.size), "{:?}", st.items);
    let head = straight.head_object("drv", "cut.mov", ReadOptions::default()).await.unwrap();
    assert!(head.metadata.contains_key("voidfs-entry"), "the commit carries the entry's marker");
    assert_eq!(straight.list_versions("drv", "cut.mov", false).await.unwrap().len(), 2);
}

/// A new file is new bytes: it is never planned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_file_goes_the_ordinary_way() {
    let (_s, _bucket, proxy, straight, c) = setup(true).await;
    let dir = tempfile::tempdir().unwrap();
    let q = open(dir.path(), &c).await;
    let data = bytes_of(2, 9 * MIB);
    q.put("drv", "new.bin", Bytes::from(data.clone()), Base::Absent, Attrs::default()).await.unwrap();
    q.settle().await;
    assert_eq!(read(&straight, "new.bin").await, data);
    assert_eq!(seen(&proxy, "x-voidfs-upload"), 0);
}

/// Where the server doesn't offer direct uploads, the file goes the ordinary way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_direct_uploads_a_file_is_put() {
    let (_s, _bucket, proxy, straight, c) = setup(false).await;
    let data = bytes_of(3, 9 * MIB);
    let v1 = straight.put_object("drv", "f", Bytes::from(data.clone()), Default::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let q = open(dir.path(), &c).await;
    let mut edited = data.clone();
    edited[100] ^= 1;
    q.put("drv", "f", Bytes::from(edited.clone()), Base::Version(v1.version_id), Attrs::default()).await.unwrap();
    q.settle().await;
    assert_eq!(read(&straight, "f").await, edited);
    assert_eq!((seen(&proxy, "x-voidfs-upload-plan"), exactly(&proxy, "PUT /drv/f")), (1, 1));
}

/// A direct commit over a version that changed meanwhile follows the `412` rule: the local
/// version is published anyway, and the version it replaced is the conflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_direct_commit_follows_the_412_rule() {
    let (_s, _bucket, proxy, straight, c) = setup(true).await;
    let data = bytes_of(4, 9 * MIB);
    let v1 = straight.put_object("drv", "f", Bytes::from(data.clone()), Default::default()).await.unwrap();
    let mut theirs = data.clone();
    theirs[0] ^= 1;
    let v2 = straight.put_object("drv", "f", Bytes::from(theirs), Default::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let q = open(dir.path(), &c).await;
    let mut ours = data.clone();
    ours[5 * MIB as usize] ^= 1;
    q.put("drv", "f", Bytes::from(ours.clone()), Base::Version(v1.version_id), Attrs::default()).await.unwrap();
    q.settle().await;
    assert_eq!(read(&straight, "f").await, ours);
    let st = q.status().await.unwrap();
    assert_eq!(st.items[0].conflict.as_deref(), Some(v2.version_id.as_str()));
    assert_eq!((seen(&proxy, "x-voidfs-upload-commit"), exactly(&proxy, "PUT /drv/f")), (2, 0), "a guarded commit, then an unguarded one");
}
