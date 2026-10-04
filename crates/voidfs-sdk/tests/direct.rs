// SPDX-License-Identifier: Apache-2.0
//! Direct uploads (protocol §4.11) against a server in this process, whose stand-in bucket binds
//! what its URLs carry.

mod common;

use std::sync::Arc;

use bytes::Bytes;
use common::{Fault, Proxy, client_for};
use rand::{RngExt, SeedableRng};
use voidfs_sdk::{Bandwidth, Client, Config, PutOptions, ReadOptions};
use voidfs_server::test_server::{FakeBucket, Rules, TestServer};

fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    (0..len).map(|_| rng.random()).collect()
}

async fn direct_server() -> (TestServer, FakeBucket) {
    let s = TestServer::with_direct_uploads(Vec::new()).await.expect("test server");
    let b = s.bucket.clone().expect("a bucket");
    (s, b)
}

async fn read(c: &Client, key: &str) -> Bytes {
    c.get_object("drv", key, ReadOptions::default()).await.unwrap().body
}

/// A new body sends every shard to the bucket; the same body with a few bytes changed sends only
/// the shards around them, and reads back changed.
#[tokio::test]
async fn a_direct_upload_sends_only_what_the_drive_lacks() {
    let (s, bucket) = direct_server().await;
    let proxy = Proxy::start(&s.endpoint).await;
    let c = client_for(&proxy.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    let data = random_bytes(1, 16 << 20);
    let (_, before) = bucket.accepted();
    proxy.clear();
    let w = c.put_object_direct("drv", "f", data.clone(), PutOptions::default()).await.unwrap();
    let (_, after) = bucket.accepted();
    assert_eq!(after - before, data.len() as u64, "every byte, once");
    assert_eq!(proxy.seen(), ["POST /drv/f?x-voidfs-upload-plan", "PUT /drv/f?x-voidfs-upload-commit"], "the server gets a plan and a commit, not the body");
    assert_eq!(read(&c, "f").await, data);
    assert_eq!(w.size, Some(data.len() as u64));

    let mut edited = data.clone();
    edited[8 << 20..(8 << 20) + 100].copy_from_slice(&[7; 100]);
    let w2 = c.put_object_direct("drv", "f", edited.clone(), PutOptions { if_version: Some(w.version_id.clone()), ..Default::default() }).await.unwrap();
    let (_, again) = bucket.accepted();
    assert!(again > after && again - after < data.len() as u64 / 3, "{} bytes for a change of 100", again - after);
    assert_eq!(read(&c, "f").await, edited);
    assert_ne!(w2.version_id, w.version_id);
    assert_eq!(c.list_versions("drv", "f", false).await.unwrap().len(), 2);
}

/// With `direct_uploads`, `put_object` sends a body of 8 MiB or more direct, and a smaller one as
/// a put; without it, everything is a put.
#[tokio::test]
async fn put_object_goes_direct_when_configured() {
    let (s, bucket) = direct_server().await;
    let direct = client_for(&s.endpoint, Config { direct_uploads: true, ..Config::default() });
    let plain = client_for(&s.endpoint, Config::default());
    direct.create_drive("drv", Default::default()).await.unwrap();
    let (puts, _) = bucket.accepted();
    direct.put_object("drv", "small", random_bytes(2, (8 << 20) - 1), PutOptions::default()).await.unwrap();
    plain.put_object("drv", "plain", random_bytes(3, 9 << 20), PutOptions::default()).await.unwrap();
    assert_eq!(bucket.accepted().0, puts, "puts send nothing to the bucket");
    let big = random_bytes(4, 8 << 20);
    direct.put_object("drv", "big", big.clone(), PutOptions::default()).await.unwrap();
    assert!(bucket.accepted().0 > puts);
    assert_eq!(read(&plain, "big").await, big);
}

/// Anything but a `409` or `412` puts the ordinary way: a server without direct uploads, a bucket
/// that refuses the shards, a commit the server refuses.
#[tokio::test]
async fn a_direct_upload_falls_back_to_a_put() {
    let data = random_bytes(5, 9 << 20);
    // A server that answers 501.
    let s = TestServer::start().await.unwrap();
    let c = client_for(&s.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    c.put_object_direct("drv", "a", data.clone(), PutOptions::default()).await.unwrap();
    assert_eq!(read(&c, "a").await, data);

    // A bucket that refuses every shard.
    let (s, bucket) = direct_server().await;
    let proxy = Proxy::start(&s.endpoint).await;
    let c = client_for(&proxy.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    bucket.set_rules(Rules { refuse: Some(403), ..Rules::default() });
    proxy.clear();
    c.put_object_direct("drv", "b", data.clone(), PutOptions::default()).await.unwrap();
    assert_eq!(read(&c, "b").await, data);
    assert_eq!(proxy.seen()[..2], ["POST /drv/b?x-voidfs-upload-plan".to_owned(), "PUT /drv/b".to_owned()], "a plan, then a put, and no commit");

    // A commit the server refuses.
    bucket.set_rules(Rules::default());
    proxy.clear();
    proxy.fault(Fault::Pass);
    proxy.fault(Fault::Status(400));
    c.put_object_direct("drv", "c", data.clone(), PutOptions::default()).await.unwrap();
    assert_eq!(read(&c, "c").await, data);
    assert_eq!(proxy.seen()[..3], ["POST /drv/c?x-voidfs-upload-plan".to_owned(), "PUT /drv/c?x-voidfs-upload-commit".to_owned(), "PUT /drv/c".to_owned()]);
}

/// A `412` or a `409` is the answer: no put follows.
#[tokio::test]
async fn a_precondition_or_a_conflict_stands() {
    let (s, _) = direct_server().await;
    let proxy = Proxy::start(&s.endpoint).await;
    let c = client_for(&proxy.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    let first = c.put_object("drv", "k", "before", PutOptions::default()).await.unwrap();
    c.put_object("drv", "dir/x", "x", PutOptions::default()).await.unwrap();
    let data = random_bytes(6, 9 << 20);
    proxy.clear();
    let e = c.put_object_direct("drv", "k", data.clone(), PutOptions { if_version: Some("99.0".into()), ..Default::default() }).await.unwrap_err();
    assert_eq!((e.status(), e.current_version_id()), (Some(412), Some(first.version_id.as_str())));
    let e = c.put_object_direct("drv", "dir", data.clone(), PutOptions::default()).await.unwrap_err();
    assert_eq!(e.status(), Some(409));
    assert!(proxy.seen().iter().all(|r| r.contains("x-voidfs-upload")), "{:?}", proxy.seen());
    assert_eq!(read(&c, "k").await, &b"before"[..]);
}

/// A shard the bucket has already answers `412` to its create-if-absent PUT, which is as good as
/// sending it; shards go out within the client's bandwidth limit, which counts them.
#[tokio::test]
async fn shards_count_once_and_within_the_limit() {
    let (s, _) = direct_server().await;
    let bw = Arc::new(Bandwidth::new(None));
    let c = client_for(&s.endpoint, Config { upload_bandwidth: Some(bw.clone()), ..Config::default() });
    c.create_drive("drv", Default::default()).await.unwrap();
    let data = Bytes::from(random_bytes(7, 9 << 20));
    let shards = voidfs_sdk::direct::shards_of(&data);
    let plan = c.plan_upload("drv", "s", &shards).await.unwrap();
    assert_eq!((plan.held, plan.upload.len()), (0, shards.len()));
    let taken = bw.taken();
    c.upload_planned(&plan, &data).await.unwrap();
    assert!(bw.taken() - taken >= data.len() as u64, "the shards' bytes went through the limit");
    // Again: the bucket has them, and says so with 412.
    c.upload_planned(&plan, &data).await.unwrap();
    let w = c.commit_upload("drv", "s", &plan.token, &shards, &hex_sha256(&data), PutOptions { content_type: Some("video/mp4".into()), ..Default::default() }).await.unwrap();
    assert_eq!(w.size, Some(data.len() as u64));
    let head = c.head_object("drv", "s", ReadOptions::default()).await.unwrap();
    assert_eq!(head.content_type.as_deref(), Some("video/mp4"));
    assert_eq!(c.plan_upload("drv", "s", &shards).await.unwrap().held, shards.len() as u64);
}

fn hex_sha256(data: &[u8]) -> String {
    voidfs_core::ids::ShardHash::of(data).to_hex()
}
