// SPDX-License-Identifier: Apache-2.0
//! What the client core's tests share: a server in this process, a client for it, and a proxy in
//! front of it that injects faults (as the SDK's tests have).

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use bytes::Bytes;
use voidfs_sdk::{Client, Config};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

pub async fn server() -> TestServer {
    TestServer::start().await.expect("test server")
}

/// `http://localhost:<port>` for a server or proxy on `addr`: a name, so that a missing
/// path-style setting would show.
pub fn localhost(endpoint: &str) -> String {
    endpoint.replace("127.0.0.1", "localhost")
}

pub fn client_for(endpoint: &str, config: Config) -> Client {
    Client::new(Config { endpoint: localhost(endpoint), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..config }).expect("client")
}

/// A server and an admin client for it.
pub async fn setup() -> (TestServer, Client) {
    let s = server().await;
    let c = client_for(&s.endpoint, Config::default());
    (s, c)
}

/// What the proxy does to the next request.
#[derive(Clone, Debug)]
pub enum Fault {
    /// Forwards it.
    Pass,
    /// Answers this status itself, with `Retry-After: 0`, and forwards nothing.
    Status(u16),
    /// Forwards it, so that it takes effect, then holds the answer this long: past the client's
    /// timeout, the answer is lost.
    Hang(Duration),
    /// Forwards it, and ends the answer's body after its first Server-Sent Event.
    CutAfterFirstEvent,
}

struct ProxyState {
    upstream: String,
    http: reqwest::Client,
    faults: Mutex<VecDeque<Fault>>,
    seen: Mutex<Vec<String>>,
}

/// A proxy in front of a server, which applies queued [`Fault`]s to the requests it gets, in
/// order, and passes the rest.
pub struct Proxy {
    pub endpoint: String,
    state: Arc<ProxyState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Proxy {
    pub async fn start(upstream: &str) -> Proxy {
        let state = Arc::new(ProxyState { upstream: upstream.into(), http: reqwest::Client::new(), faults: Mutex::new(VecDeque::new()), seen: Mutex::new(Vec::new()) });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback(handle).with_state(state.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Proxy { endpoint, state, task }
    }

    pub fn fault(&self, f: Fault) {
        self.state.faults.lock().unwrap().push_back(f);
    }

    /// Every request so far, as `METHOD /path?query`, and ` last-event-id=<id>` if it had one.
    pub fn seen(&self) -> Vec<String> {
        self.state.seen.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.state.seen.lock().unwrap().clear();
    }
}

async fn handle(State(st): State<Arc<ProxyState>>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let pq = parts.uri.path_and_query().map_or("/".to_owned(), |p| p.as_str().to_owned());
    let resume = parts.headers.get("last-event-id").and_then(|v| v.to_str().ok()).map(|v| format!(" last-event-id={v}")).unwrap_or_default();
    st.seen.lock().unwrap().push(format!("{} {}{resume}", parts.method, pq));
    let fault = st.faults.lock().unwrap().pop_front().unwrap_or(Fault::Pass);
    if let Fault::Status(s) = fault {
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>SlowDown</Code><Message>injected</Message><RequestId>proxy</RequestId></Error>";
        return Response::builder().status(s).header("retry-after", "0").header("content-type", "application/xml").body(Body::from(xml)).unwrap();
    }
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let mut rb = st.http.request(parts.method.clone(), format!("{}{}", st.upstream, pq));
    // `host` too: the signature covers the proxy's.
    for (k, v) in &parts.headers {
        rb = rb.header(k, v);
    }
    let mut resp = rb.body(body).send().await.unwrap();
    if let Fault::Hang(d) = fault {
        tokio::time::sleep(d).await;
    }
    let mut out = Response::builder().status(resp.status().as_u16());
    for (k, v) in resp.headers() {
        if !matches!(k.as_str(), "content-length" | "transfer-encoding" | "connection") {
            out = out.header(k, v);
        }
    }
    let body = if let Fault::CutAfterFirstEvent = fault {
        let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
        let mut got = Vec::new();
        'read: while let Ok(Some(c)) = resp.chunk().await {
            got.extend_from_slice(&c);
            let mut start = 0;
            while let Some(i) = find(&got[start..], b"\n\n") {
                let end = start + i + 2;
                if find(&got[start..end], b"data:").is_some() {
                    got.truncate(end);
                    break 'read;
                }
                start = end;
            }
        }
        Body::from(got)
    } else {
        Body::from_stream(futures::stream::unfold(resp, |mut r| async move {
            match r.chunk().await {
                Ok(Some(c)) => Some((Ok::<Bytes, std::io::Error>(c), r)),
                _ => None,
            }
        }))
    };
    out.body(body).unwrap()
}
