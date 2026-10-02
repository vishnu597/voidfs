// SPDX-License-Identifier: Apache-2.0
//! The retry rule, through a proxy that fails requests: idempotent calls are sent again and
//! succeed, and an unguarded insert or removal is applied once and never sent twice.

mod common;

use std::time::Duration;

use common::{Fault, Proxy, client_for, server};
use voidfs_sdk::*;
use voidfs_server::test_server::TestServer;

/// A server with a drive holding `k` = "hello", a proxy in front of it, and a client of the proxy
/// that gives up on a silent server after 300 ms.
async fn setup() -> (TestServer, Proxy, Client) {
    let s = common::server().await;
    let direct = client_for(&s.endpoint, Config::default());
    direct.create_drive("drv", Default::default()).await.unwrap();
    direct.put_object("drv", "k", "hello", Default::default()).await.unwrap();
    let p = Proxy::start(&s.endpoint).await;
    let c = client_for(&p.endpoint, Config { timeout: Duration::from_millis(300), ..Default::default() });
    (s, p, c)
}

async fn text(c: &Client) -> String {
    String::from_utf8(c.get_object("drv", "k", Default::default()).await.unwrap().body.to_vec()).unwrap()
}

/// The requests the proxy saw whose query has `marker`.
fn count(p: &Proxy, marker: &str) -> usize {
    p.seen().iter().filter(|r| r.contains(marker)).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn idempotent_calls_retry_server_errors() {
    let (_s, p, c) = setup().await;
    p.fault(Fault::Status(503));
    c.write_at("drv", "k", 0, "J", Default::default()).await.unwrap();
    assert_eq!(count(&p, "x-voidfs-write"), 2);
    p.fault(Fault::Status(500));
    p.fault(Fault::Status(502));
    assert_eq!(text(&c).await, "Jello");
    assert_eq!(p.seen().len(), 2 + 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn attempts_stop_at_the_configured_number() {
    let (_s, p, c) = setup().await;
    for _ in 0..3 {
        p.fault(Fault::Status(503));
    }
    let e = c.write_at("drv", "k", 0, "J", Default::default()).await.unwrap_err();
    assert_eq!((e.status(), e.code()), (Some(503), Some("SlowDown")));
    assert_eq!(count(&p, "x-voidfs-write"), 3);
    let c1 = client_for(&p.endpoint, Config { max_attempts: 1, ..Default::default() });
    p.fault(Fault::Status(503));
    assert_eq!(c1.write_at("drv", "k", 0, "J", Default::default()).await.unwrap_err().status(), Some(503));
    assert_eq!(count(&p, "x-voidfs-write"), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_precondition_failure_is_not_retried() {
    let (_s, p, c) = setup().await;
    let e = c.write_at("drv", "k", 0, "J", WriteOptions { if_version: Some("999.0".into()), ..Default::default() }).await.unwrap_err();
    assert_eq!(e.status(), Some(412));
    assert_eq!(count(&p, "x-voidfs-write"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unguarded_splice_is_not_sent_again_after_a_server_error() {
    let (_s, p, c) = setup().await;
    p.fault(Fault::Status(503));
    let e = c.insert("drv", "k", 5, "!", Default::default()).await.unwrap_err();
    assert_eq!(e.status(), Some(503));
    assert_eq!(count(&p, "x-voidfs-splice"), 1);
    assert_eq!(text(&c).await, "hello");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unguarded_splice_whose_answer_is_lost_is_applied_once() {
    let (_s, p, c) = setup().await;
    p.fault(Fault::Hang(Duration::from_secs(2)));
    let e = c.insert("drv", "k", 5, "!", Default::default()).await.unwrap_err();
    assert!(matches!(e, Error::Transport { sent: true, .. }), "{e:?}");
    assert_eq!(count(&p, "x-voidfs-splice"), 1);
    assert_eq!(text(&c).await, "hello!", "applied, once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guarded_splice_is_retried_and_fails_its_precondition_instead_of_applying_twice() {
    let (_s, p, c) = setup().await;
    let head = c.head_object("drv", "k", Default::default()).await.unwrap();
    p.fault(Fault::Hang(Duration::from_secs(2)));
    let e = c.insert("drv", "k", 5, "!", Preconditions::if_version(head.version_id.clone())).await.unwrap_err();
    assert_eq!(e.status(), Some(412));
    assert_eq!(count(&p, "x-voidfs-splice"), 2);
    assert_eq!(text(&c).await, "hello!");
    let now = c.head_object("drv", "k", Default::default()).await.unwrap();
    assert_eq!(e.current_version_id(), Some(now.version_id.as_str()), "the version the first attempt made");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_offset_write_whose_answer_is_lost_is_sent_again() {
    let (_s, p, c) = setup().await;
    p.fault(Fault::Hang(Duration::from_secs(2)));
    let w = c.write_at("drv", "k", 0, "J", Default::default()).await.unwrap();
    assert_eq!(count(&p, "x-voidfs-write"), 2);
    assert_eq!(text(&c).await, "Jello");
    assert_eq!(c.head_object("drv", "k", Default::default()).await.unwrap().version_id, w.version_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unguarded_rename_is_not_sent_again() {
    let (_s, p, c) = setup().await;
    p.fault(Fault::Status(503));
    assert_eq!(c.rename("drv", "k", "k2", Default::default()).await.unwrap_err().status(), Some(503));
    assert_eq!(count(&p, "x-voidfs-rename"), 1);
    p.fault(Fault::Hang(Duration::from_secs(2)));
    assert!(matches!(c.rename("drv", "k", "k2", Default::default()).await, Err(Error::Transport { sent: true, .. })));
    assert_eq!(count(&p, "x-voidfs-rename"), 2);
    // Guarded, it is retried.
    let head = c.head_object("drv", "k2", Default::default()).await.unwrap();
    p.fault(Fault::Status(503));
    c.rename("drv", "k2", "k3", RenameOptions { if_version: Some(head.version_id), ..Default::default() }).await.unwrap();
    assert_eq!(count(&p, "x-voidfs-rename"), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_to_connect_is_retried_even_for_an_unguarded_splice() {
    // A port nothing listens on yet: the first attempt can't connect, and so sent nothing.
    let s = common::server().await;
    let direct = client_for(&s.endpoint, Config::default());
    direct.create_drive("drv", Default::default()).await.unwrap();
    direct.put_object("drv", "k", "hello", Default::default()).await.unwrap();
    let spare = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = spare.local_addr().unwrap();
    drop(spare);
    let c = client_for(&format!("http://{addr}"), Config { max_attempts: 10, ..Default::default() });
    let upstream = s.endpoint.clone();
    let relay = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        // Relay bytes to the server, as the proxy would without looking at them.
        loop {
            let (mut inbound, _) = listener.accept().await.unwrap();
            let mut outbound = tokio::net::TcpStream::connect(upstream.trim_start_matches("http://")).await.unwrap();
            tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            });
        }
    });
    c.insert("drv", "k", 5, "!", Default::default()).await.unwrap();
    relay.abort();
    assert_eq!(String::from_utf8(direct.get_object("drv", "k", Default::default()).await.unwrap().body.to_vec()).unwrap(), "hello!");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_resumes_after_its_stream_breaks() {
    let (s, p, c) = setup().await;
    let direct = client_for(&s.endpoint, Config::default());
    let start = direct.describe_drive("drv").await.unwrap().seq;
    p.fault(Fault::CutAfterFirstEvent);
    let mut watch = c.watch_changes("drv", start);
    let a = direct.put_object("drv", "a", "1", Default::default()).await.unwrap();
    let first = watch.next().await.unwrap();
    assert_eq!(first.changes[0].version_id, a.version_id);
    let b = direct.put_object("drv", "b", "2", Default::default()).await.unwrap();
    let second = watch.next().await.unwrap();
    assert_eq!(second.changes[0].version_id, b.version_id, "nothing repeated, nothing missed");
    let streams: Vec<String> = p.seen().into_iter().filter(|r| r.contains("x-voidfs-changes")).collect();
    assert_eq!(streams.len(), 2, "{streams:?}");
    assert!(streams[1].contains(&format!("since={} last-event-id={}", first.seq, first.seq)), "{streams:?}");
}

#[derive(Default)]
struct Seen(std::sync::Mutex<Vec<(Option<u16>, bool)>>);

impl Observe for Seen {
    fn observe(&self, o: &Observation) {
        self.0.lock().unwrap().push((o.status, o.sent));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_attempt_is_observed() {
    let s = server().await;
    let p = Proxy::start(&s.endpoint).await;
    let seen = std::sync::Arc::new(Seen::default());
    let c = client_for(&p.endpoint, Config { observer: Some(seen.clone()), ..Default::default() });
    c.create_drive("drv", Default::default()).await.unwrap();
    p.fault(Fault::Status(503));
    c.put_object("drv", "k", "x", Default::default()).await.unwrap();
    assert!(c.get_object("drv", "missing", Default::default()).await.is_err());
    assert_eq!(seen.0.lock().unwrap()[1..], [(Some(503), true), (Some(200), true), (Some(404), true)], "the drive, the put twice, the read");
    // Nothing listening: no answer, and nothing sent.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = closed.local_addr().unwrap();
    drop(closed);
    let gone = client_for(&format!("http://{addr}"), Config { observer: Some(seen.clone()), max_attempts: 1, ..Default::default() });
    assert!(gone.get_object("drv", "k", Default::default()).await.is_err());
    assert_eq!(seen.0.lock().unwrap().last(), Some(&(None, false)));
}
