// SPDX-License-Identifier: Apache-2.0
//! The change-feed client and connectivity, against a server in this process.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Fault, Proxy, client_for, server};
use voidfs_client::{ApiFetcher, Attrs, Base, Cache, CacheConfig, Connectivity, ConnectivityConfig, Content, Error, FeedEvent, FeedWatch, Invalidation, Link, Queue, QueueConfig, Store};
use voidfs_sdk::{Client, Config, ReadOptions};

/// Events until `done` says so, or ten seconds.
async fn events_until(w: &mut FeedWatch, mut done: impl FnMut(&[FeedEvent]) -> bool) -> Vec<FeedEvent> {
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&got) {
        match tokio::time::timeout_at(deadline.into(), w.next()).await {
            Ok(Some(e)) => got.push(e),
            _ => panic!("waited for events; got {got:?}"),
        }
    }
    got
}

fn all(events: &[FeedEvent]) -> BTreeSet<Invalidation> {
    events.iter().flat_map(|e| e.invalidations.iter().cloned()).collect()
}

async fn position(c: &Client) -> u64 {
    c.list_folder_page("drv", "", None).await.unwrap().seq
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changes_arrive_as_what_they_make_stale_from_a_listings_position() {
    use Invalidation::*;
    let s = server().await;
    let c = client_for(&s.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    c.put_object("drv", "a/x", "x", Default::default()).await.unwrap();
    c.put_object("drv", "d/f", "1", Default::default()).await.unwrap();
    let at = c.list_versions("drv", "d/f", false).await.unwrap()[0].last_modified.clone();
    let since = position(&c).await;
    let mut w = FeedWatch::start(c.clone(), "drv", since, None);
    c.put_object("drv", "a/y", "y", Default::default()).await.unwrap();
    c.rename("drv", "a/x", "b/x", Default::default()).await.unwrap();
    c.rename("drv", "a/", "c/", Default::default()).await.unwrap();
    c.delete_object("drv", "b/x", Default::default()).await.unwrap();
    c.put_object("drv", "d/f", "2", Default::default()).await.unwrap();
    c.restore_as_of("drv", "d/", &at, Default::default()).await.unwrap();
    let want: BTreeSet<Invalidation> = [
        Object("a/y".into()),
        Object("a/x".into()),
        Object("b/x".into()),
        Object("b/".into()),
        Object("a/".into()),
        Object("c/".into()),
        Subtree("a/".into()),
        Subtree("c/".into()),
        Object("d/f".into()),
        Object("d/".into()),
        Subtree("d/".into()),
    ]
    .into();
    let got = events_until(&mut w, |e| all(e).is_superset(&want)).await;
    assert!(got[0].seq > since && got.windows(2).all(|p| p[0].seq < p[1].seq), "{got:?}");
    assert!(got.iter().all(|e| e.drive == "drv"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_stream_reconnects_and_misses_nothing() {
    let s = server().await;
    let p = Proxy::start(&s.endpoint).await;
    let direct = client_for(&s.endpoint, Config::default());
    let c = client_for(&p.endpoint, Config::default());
    direct.create_drive("drv", Default::default()).await.unwrap();
    let since = position(&direct).await;
    p.fault(Fault::CutAfterFirstEvent);
    let mut w = FeedWatch::start(c, "drv", since, None);
    for i in 0..5 {
        direct.put_object("drv", &format!("k{i}"), "x", Default::default()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let got = events_until(&mut w, |e| (0..5).all(|i| all(e).contains(&Invalidation::Object(format!("k{i}"))))).await;
    let seqs: Vec<u64> = got.iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|p| p[0] < p[1]), "each position once, in order: {seqs:?}");
    let streams: Vec<String> = p.seen().into_iter().filter(|r| r.contains("x-voidfs-changes")).collect();
    assert!(streams.len() >= 2, "it reconnected: {streams:?}");
    assert!(streams[1].contains(&format!("since={}", got[0].seq)), "from the last position delivered: {streams:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resync_watch_invalidates_an_idle_drive_when_its_stream_reopens() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(|| async { ([("content-type", "text/event-stream")], ": keep-alive\n\n") });
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let client = client_for(&endpoint, Config { max_attempts: 1, timeout: Duration::from_millis(300), ..Default::default() });
    let mut watch = FeedWatch::start_with_resync(client, "drv", 17, None);
    let event = tokio::time::timeout(Duration::from_secs(5), watch.next()).await.expect("idle reconnect invalidation").unwrap();
    assert_eq!(event, FeedEvent { drive: "drv".into(), seq: 17, invalidations: vec![Invalidation::All] });
    drop(watch);
    server.abort();
    let _ = server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_changes_relist_the_drive_and_watch_on() {
    let s = server().await;
    let p = Proxy::start(&s.endpoint).await;
    let direct = client_for(&s.endpoint, Config::default());
    let c = client_for(&p.endpoint, Config::default());
    direct.create_drive("drv", Default::default()).await.unwrap();
    direct.put_object("drv", "old", "x", Default::default()).await.unwrap();
    p.fault(Fault::Status(410));
    let mut w = FeedWatch::start(c, "drv", 0, None);
    let first = events_until(&mut w, |e| !e.is_empty()).await;
    let now = position(&direct).await;
    assert_eq!((first[0].seq, first[0].invalidations.as_slice()), (now, &[Invalidation::All][..]), "relisted at the current position");
    direct.put_object("drv", "new", "x", Default::default()).await.unwrap();
    let next = events_until(&mut w, |e| !e.is_empty()).await;
    assert_eq!(next[0].invalidations, [Invalidation::Object("new".into())], "and watched on from there");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_reads_fail_at_once_and_uploads_wait_for_the_link() {
    let s = server().await;
    let p = Proxy::start(&s.endpoint).await;
    let direct = client_for(&s.endpoint, Config::default());
    direct.create_drive("drv", Default::default()).await.unwrap();
    direct.put_object("drv", "f", "remote bytes", Default::default()).await.unwrap();
    let f = Content::of("drv", "f", &direct.head_object("drv", "f", ReadOptions::default()).await.unwrap());
    let conn = Connectivity::new(ConnectivityConfig { offline_after: 3, probe_every: Duration::from_millis(800), ..Default::default() });
    let c = client_for(&p.endpoint, Config { timeout: Duration::from_millis(200), observer: Some(Arc::new(conn.clone())), ..Default::default() });
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(c.clone()).with_connectivity(conn.clone())), CacheConfig { min_free_bytes: 0, ..Default::default() }).await.unwrap();
    let q = Queue::open(store, c.clone(), QueueConfig { connectivity: Some(conn.clone()), ..Default::default() }).await.unwrap();

    // Three requests in a row without an answer.
    for _ in 0..3 {
        p.fault(Fault::Hang(Duration::from_secs(1)));
    }
    assert!(c.head_object("drv", "f", ReadOptions::default()).await.is_err());
    assert_eq!(conn.link(), Link::Offline);
    p.clear();
    let t = Instant::now();
    assert!(matches!(cache.read(&f, 0, 6).await, Err(Error::Offline)));
    assert!(t.elapsed() < Duration::from_millis(50), "at once: {:?}", t.elapsed());
    q.put("drv", "q", "queued".into(), Base::Absent, Attrs::default()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(p.seen().is_empty(), "nothing sent while offline: {:?}", p.seen());
    assert_eq!(q.status().await.unwrap().unpublished, 1);

    // A probe finds the server again; the queue goes on.
    let _probe = conn.probe(c.clone());
    let mut rx = conn.watch();
    tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|l| *l == Link::Online)).await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(10), q.settle()).await.expect("the queue went on once online");
    assert_eq!(direct.get_object("drv", "q", ReadOptions::default()).await.unwrap().body, "queued");
    assert_eq!(cache.read(&f, 0, 6).await.unwrap(), "remote");
}
