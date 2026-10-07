// SPDX-License-Identifier: Apache-2.0
//! Shutdown cancels read-only bootstrap requests and consumers waiting behind their creation lock.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use voidfs_daemon::api::{Build, fs};
use voidfs_daemon::{Daemon, DaemonClient, DaemonConfig, FsClientError};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

struct Proxy {
    endpoint: String,
    http: reqwest::Client,
    stall: &'static str,
    entered: tokio::sync::watch::Sender<bool>,
}

async fn forward(State(proxy): State<Arc<Proxy>>, request: axum::extract::Request) -> Response {
    if request.uri().query().is_some_and(|query| query.contains(proxy.stall)) {
        proxy.entered.send_replace(true);
        return std::future::pending().await;
    }
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
    let mut send = proxy.http.request(parts.method, format!("{}{}", proxy.endpoint, parts.uri));
    for (name, value) in &parts.headers { send = send.header(name, value); }
    let response = send.body(bytes).send().await.unwrap();
    let mut output = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if !matches!(name.as_str(), "content-length" | "transfer-encoding" | "connection") { output = output.header(name, value); }
    }
    output.body(Body::from_stream(futures::stream::unfold(response, |mut response| async move {
        match response.chunk().await {
            Ok(Some(bytes)) => Some((Ok::<_, std::io::Error>(bytes), response)),
            _ => None,
        }
    }))).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_cancels_stalled_bootstrap_and_creators_waiting_for_the_registry() {
    let upstream = TestServer::start().await.unwrap();
    let credentials = voidfs_sdk::Config { endpoint: upstream.endpoint.clone(), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..Default::default() };
    voidfs_sdk::Client::new(credentials.clone()).unwrap().create_drive("drive", Default::default()).await.unwrap();
    for stall in ["x-voidfs-drive", "x-voidfs-list"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (entered, mut observed) = tokio::sync::watch::channel(false);
        let proxy = Arc::new(Proxy { endpoint: upstream.endpoint.clone(), http: reqwest::Client::new(), stall, entered });
        let app = axum::Router::new().fallback(forward).with_state(proxy);
        let relay = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let dir = tempfile::Builder::new().prefix("vdsbootstrap").tempdir_in(std::env::temp_dir()).unwrap();
        let cfg = DaemonConfig::new(dir.path(), voidfs_sdk::Config { endpoint, max_attempts: 1, timeout: Duration::from_secs(20), ..credentials.clone() }, Build::default());
        let daemon = Daemon::start(cfg).await.unwrap();
        let client = DaemonClient::new(daemon.socket());
        let mut creators = Vec::new();
        for _ in 0..fs::MAX_CALLS {
            let client = client.clone();
            creators.push(tokio::spawn(async move { client.session("drive", true).await }));
        }
        tokio::time::timeout(Duration::from_secs(3), observed.wait_for(|entered| *entered)).await.expect("one create reaches the stalled bootstrap request").unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if matches!(tokio::time::timeout(Duration::from_millis(100), client.session("drive", true)).await,
                    Ok(Err(FsClientError::Api { status: 503, code, .. })) if code == "Busy") { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("all call slots are occupied by the bootstrap and queued creators");
        let mut stop = tokio::spawn(daemon.stop());
        let stopped = tokio::time::timeout(Duration::from_secs(2), &mut stop).await;
        if stopped.is_err() { stop.abort(); let _ = stop.await; }
        for creator in creators { creator.abort(); let _ = creator.await; }
        relay.abort();
        let _ = relay.await;
        assert!(stopped.is_ok(), "stop must cancel {stall} and queued creators before the 20s SDK timeout");
        drop(voidfs_client::Store::open(dir.path()).expect("bootstrap consumers released the state directory"));
    }
}
