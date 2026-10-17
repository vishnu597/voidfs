// SPDX-License-Identifier: Apache-2.0
//! The link's state from what a client's requests get, through the SDK's observer, against a
//! server in this process: an upload bandwidth limit isn't a slow server, and a server that holds
//! requests without answering is found out while one waits.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Fault, Proxy, client_for, server};
use sha2::{Digest, Sha256};
use voidfs_client::{Connectivity, ConnectivityConfig, Link};
use voidfs_sdk::{Bandwidth, Client, Config};

fn observed(endpoint: &str, conn: &Connectivity, bandwidth: Option<Arc<Bandwidth>>) -> Client {
    client_for(endpoint, Config { observer: Some(Arc::new(conn.clone())), upload_bandwidth: bandwidth, ..Default::default() })
}

/// The probes the proxy saw: `ListBuckets`, which nothing else here sends.
fn probes(p: &Proxy) -> usize {
    p.seen().iter().filter(|l| l.starts_with("GET /?") || *l == "GET /").count()
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_at_the_limits_pace_are_not_slow_answers() {
    let s = server().await;
    let p = Proxy::start(&s.endpoint).await;
    let conn = Connectivity::new(ConnectivityConfig { slow: Duration::from_millis(150), degraded_after: 3, ..Default::default() });
    // Three 64 KiB pieces a quarter of a second apart: well past `slow` each.
    let c = observed(&p.endpoint, &conn, Some(Arc::new(Bandwidth::new(Some(256 << 10)))));
    c.create_drive("drv", Default::default()).await.unwrap();
    let body = vec![7u8; 192 << 10];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("body");
    std::fs::write(&path, &body).unwrap();
    let sha = hex::encode(Sha256::digest(&body));
    for i in 0..3 {
        let t = Instant::now();
        c.put_object("drv", &format!("put{i}"), body.clone(), Default::default()).await.unwrap();
        assert!(t.elapsed() >= Duration::from_millis(400), "paced: {:?}", t.elapsed());
        let t = Instant::now();
        c.put_object_file("drv", &format!("file{i}"), &path, &sha, Default::default(), None).await.unwrap();
        assert!(t.elapsed() >= Duration::from_millis(400), "paced: {:?}", t.elapsed());
    }
    assert_eq!(conn.link(), Link::Online, "six uploads at the limit's pace are not six slow answers");

    // A slow server still is one.
    p.slow(Duration::from_millis(300));
    for i in 0..3 {
        c.put_object("drv", &format!("slow{i}"), "x", Default::default()).await.unwrap();
    }
    assert_eq!(conn.link(), Link::Degraded, "three answers slower than `slow`");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_answers_nothing_takes_the_link_offline_while_a_request_waits() {
    let s = server().await;
    let p = Proxy::start(&s.endpoint).await;
    let patience = Duration::from_millis(300);
    let conn = Connectivity::new(ConnectivityConfig { patience, probe_every: Duration::from_millis(100), ..Default::default() });
    let c = observed(&p.endpoint, &conn, None);
    let _probe = conn.probe(c.clone());
    c.create_drive("drv", Default::default()).await.unwrap();

    // Connections open, nothing answered, and the one request out allows two minutes.
    p.slow(Duration::from_secs(60));
    let t = Instant::now();
    let upload = tokio::spawn({
        let c = c.clone();
        async move { c.put_object("drv", "hung", "x", Default::default()).await }
    });
    let mut rx = conn.watch();
    tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|l| *l == Link::Offline)).await.expect("offline while the upload waits").unwrap();
    assert!(t.elapsed() >= patience * 2, "the upload's patience, then the probe's: {:?}", t.elapsed());
    assert!(!upload.is_finished(), "found out before the upload gave up");
    assert!(probes(&p) >= 1, "{:?}", p.seen());

    // The server answers again: the probe finds it.
    p.slow(Duration::ZERO);
    tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|l| *l == Link::Online)).await.expect("online once it answers").unwrap();
    upload.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_answers_gets_no_probes() {
    let s = server().await;
    let p = Proxy::start(&s.endpoint).await;
    let patience = Duration::from_millis(200);
    let conn = Connectivity::new(ConnectivityConfig { patience, ..Default::default() });
    // Each upload takes about 0.75 s at the limit's pace: several patiences.
    let c = observed(&p.endpoint, &conn, Some(Arc::new(Bandwidth::new(Some(256 << 10)))));
    let _probe = conn.probe(c.clone());
    c.create_drive("drv", Default::default()).await.unwrap();
    let rx = conn.watch();
    for i in 0..3 {
        c.put_object("drv", &format!("paced{i}"), vec![1u8; 192 << 10], Default::default()).await.unwrap();
        c.head_object("drv", &format!("paced{i}"), Default::default()).await.unwrap();
    }
    tokio::time::sleep(patience * 3).await;
    assert_eq!(probes(&p), 0, "{:?}", p.seen());
    assert!(!rx.has_changed().unwrap(), "online throughout: {:?}", conn.link());

    // An answer that takes long, from a server that answers others: probes, ever further apart,
    // and the link stays online.
    p.fault(Fault::Hang(Duration::from_secs(3)));
    c.put_object("drv", "held", "x", Default::default()).await.unwrap();
    let n = probes(&p);
    assert!((1..=5).contains(&n), "probes: {n}, {:?}", p.seen());
    assert!(!rx.has_changed().unwrap(), "online throughout: {:?}", conn.link());
}
