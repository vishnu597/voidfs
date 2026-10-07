// SPDX-License-Identifier: Apache-2.0
//! A feed connection is observable even before a change arrives.

mod common;

use std::time::Duration;

use voidfs_sdk::{ChangeWatchEvent, Config};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_a_watch_reports_its_position_before_delivering_changes() {
    let (_server, client) = common::setup().await;
    client.create_drive("drv", Default::default()).await.unwrap();
    let since = client.describe_drive("drv").await.unwrap().seq;
    let mut watch = client.watch_changes("drv", since);
    let opened = tokio::time::timeout(Duration::from_secs(5), watch.next_event()).await.expect("an idle stream reports its opening").unwrap();
    assert_eq!(opened, ChangeWatchEvent::Connected { since, reconnect: false });
    client.put_object("drv", "file", "bytes", Default::default()).await.unwrap();
    let batch = tokio::time::timeout(Duration::from_secs(5), watch.next()).await.unwrap().unwrap();
    assert!(batch.seq > since);
    assert_eq!(batch.changes[0].key, "file");
    assert_eq!(watch.position(), batch.seq, "the existing batch-only API still advances normally");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_reconnect_reports_the_same_position_without_waiting_for_changes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(|| async { ([("content-type", "text/event-stream")], ": keep-alive\n\n") });
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let client = common::client_for(&endpoint, Config { max_attempts: 1, timeout: Duration::from_millis(300), ..Default::default() });
    let mut watch = client.watch_changes("drv", 17);
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), watch.next_event()).await.unwrap().unwrap(), ChangeWatchEvent::Connected { since: 17, reconnect: false });
    let reopened = tokio::time::timeout(Duration::from_secs(5), watch.next_event()).await.expect("an idle reconnect must be observable").unwrap();
    assert_eq!(reopened, ChangeWatchEvent::Connected { since: 17, reconnect: true });
    assert_eq!(watch.position(), 17);
    server.abort();
    let _ = server.await;
}
