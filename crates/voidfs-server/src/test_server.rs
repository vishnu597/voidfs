// SPDX-License-Identifier: Apache-2.0
//! A server over a memory pool, in this process, on a free port of 127.0.0.1: what the tests of the
//! SDK, the CLI and the client run against. It serves the S3 port as the binary does, with the
//! keys it is given, and with direct uploads through a [`FakeBucket`] if asked.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use futures::future::BoxFuture;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use crate::direct::{Presign, Presigned};
use crate::pool::Pool;
use crate::s3::{App, Domains};
use crate::sigv4::{KeyInfo, Keys, Scope};
use crate::store::Store;

/// The admin key every test server accepts.
pub const ADMIN_KEY_ID: &str = "VFTESTADMINKEY234567";
pub const ADMIN_SECRET: &str = "testsecrettestsecrettestsecrettestsecret";

pub struct TestServer {
    /// `http://127.0.0.1:<port>`.
    pub endpoint: String,
    pub addr: SocketAddr,
    pub pool: Arc<Pool>,
    /// Where direct uploads go, for a server started with them.
    pub bucket: Option<FakeBucket>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// A server that accepts [`ADMIN_KEY_ID`].
    pub async fn start() -> anyhow::Result<TestServer> {
        TestServer::with_keys(Vec::new()).await
    }

    /// A server that accepts [`ADMIN_KEY_ID`] and `keys`.
    pub async fn with_keys(keys: Vec<KeyInfo>) -> anyhow::Result<TestServer> {
        TestServer::serve(keys, false).await
    }

    /// A server that accepts [`ADMIN_KEY_ID`] and `keys`, and offers direct uploads (protocol
    /// §4.11) to a [`FakeBucket`] over its pool's store, which binds what its URLs carry. The
    /// server checks the bucket as it would a real one when it starts.
    pub async fn with_direct_uploads(keys: Vec<KeyInfo>) -> anyhow::Result<TestServer> {
        TestServer::serve(keys, true).await
    }

    async fn serve(keys: Vec<KeyInfo>, direct_uploads: bool) -> anyhow::Result<TestServer> {
        let mut all = Keys::default();
        all.insert(KeyInfo { id: ADMIN_KEY_ID.into(), secret: ADMIN_SECRET.into(), scope: Scope::Admin, drives: None });
        for k in keys {
            all.insert(k);
        }
        let store = Store::memory()?;
        let pool = Pool::open(store.clone(), 64 << 20).await?;
        let (bucket, direct) = match direct_uploads {
            false => (None, None),
            true => {
                let bucket = FakeBucket::start(store).await?;
                (Some(bucket.clone()), crate::probe::offer(Box::new(bucket), &opendal::raw::HttpClient::new()?).await.1)
            }
        };
        let app = Arc::new(App { pool: pool.clone(), keys: all, domains: Domains::new(Vec::new()), metrics: crate::metrics::S3Metrics::new(), uploads: Default::default(), read_ahead: Default::default(), direct });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, crate::s3::router(app)).await;
        });
        Ok(TestServer { endpoint: format!("http://{addr}"), addr, pool, bucket, task })
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Which of what its URLs bind a [`FakeBucket`] enforces. All of it by default; turning one off
/// makes it a bucket that ignores it.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// Refuse bytes that don't match `x-amz-checksum-sha256` (400 BadDigest).
    pub checksum: bool,
    /// Refuse a PUT without, or with other values of, the headers its URL was signed with (403).
    pub signed_headers: bool,
    /// For `If-None-Match: *`: `Some(true)` refuses an object that exists (412), `Some(false)`
    /// ignores the header, and `None` answers 501, as Backblaze B2 does.
    pub if_none_match: Option<bool>,
    /// Answer every PUT with this status, as a bucket the client can't use.
    pub refuse: Option<u16>,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { checksum: true, signed_headers: true, if_none_match: Some(true), refuse: None }
    }
}

/// A stand-in for a bucket that presigns PUTs, over a pool's [`Store`]: where the tests of direct
/// uploads send shards. It signs a URL's path, expiry and headers with a key of its own.
#[derive(Clone)]
pub struct FakeBucket {
    /// `http://127.0.0.1:<port>`.
    pub endpoint: String,
    state: Arc<FakeState>,
    _task: Arc<AbortOnDrop>,
}

struct FakeState {
    rules: Mutex<Rules>,
    /// PUTs it accepted, and their bytes.
    puts: AtomicU64,
    bytes: AtomicU64,
    key: [u8; 32],
    store: Store,
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl FakeBucket {
    pub async fn start(store: Store) -> anyhow::Result<FakeBucket> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let state = Arc::new(FakeState { rules: Mutex::default(), puts: AtomicU64::new(0), bytes: AtomicU64::new(0), key: rand::random(), store });
        let served = state.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, axum::Router::new().fallback(fake_put).with_state(served)).await;
        });
        Ok(FakeBucket { endpoint, state, _task: Arc::new(AbortOnDrop(task)) })
    }

    pub fn set_rules(&self, rules: Rules) {
        *self.state.rules.lock().unwrap() = rules;
    }

    /// The PUTs it accepted so far, and their bytes.
    pub fn accepted(&self) -> (u64, u64) {
        (self.state.puts.load(Ordering::Relaxed), self.state.bytes.load(Ordering::Relaxed))
    }
}

impl FakeState {
    fn sign(&self, path: &str, expires: i64, headers: &[(String, String)]) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(&self.key).expect("any key length");
        m.update(path.as_bytes());
        m.update(&expires.to_be_bytes());
        for (k, v) in headers {
            m.update(format!("\n{k}:{v}").as_bytes());
        }
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(m.finalize().into_bytes())
    }
}

impl Presign for FakeBucket {
    fn put<'a>(&'a self, path: &'a str, headers: &'a [(String, String)], expires: Duration) -> BoxFuture<'a, anyhow::Result<Presigned>> {
        Box::pin(async move {
            let s = &self.state;
            let until = chrono::Utc::now().timestamp() + expires.as_secs() as i64;
            let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
            let url = format!("{}/{path}?expires={until}&headers={}&sig={}&path-sig={}", self.endpoint, names.join(","), s.sign(path, until, headers), s.sign(path, until, &[]));
            Ok(Presigned { url, headers: headers.to_vec() })
        })
    }
}

fn fake_error(status: u16, code: &str) -> Response {
    (http::StatusCode::from_u16(status).unwrap(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code><Message>{code}</Message></Error>")).into_response()
}

async fn fake_put(State(b): State<Arc<FakeState>>, req: Request) -> Response {
    let rules = *b.rules.lock().unwrap();
    let (parts, body) = req.into_parts();
    if parts.method != http::Method::PUT {
        return fake_error(405, "MethodNotAllowed");
    }
    if let Some(status) = rules.refuse {
        return fake_error(status, "AccessDenied");
    }
    let path = parts.uri.path().trim_start_matches('/').to_owned();
    let q: std::collections::HashMap<String, String> = parts.uri.query().unwrap_or("").split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
    let expires: i64 = q.get("expires").and_then(|e| e.parse().ok()).unwrap_or(0);
    if q.get("path-sig") != Some(&b.sign(&path, expires, &[])) || chrono::Utc::now().timestamp() > expires {
        return fake_error(403, "AccessDenied");
    }
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    if rules.signed_headers {
        let sent: Vec<(String, String)> = q.get("headers").map(|h| h.split(',').filter(|n| !n.is_empty()).map(|n| (n.to_owned(), header(n).unwrap_or_default())).collect()).unwrap_or_default();
        if q.get("sig") != Some(&b.sign(&path, expires, &sent)) {
            return fake_error(403, "SignatureDoesNotMatch");
        }
    }
    let Ok(data) = axum::body::to_bytes(body, 64 << 20).await else { return fake_error(400, "IncompleteBody") };
    if rules.checksum
        && let Some(sum) = header("x-amz-checksum-sha256")
        && sum != base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&data))
    {
        return fake_error(400, "BadDigest");
    }
    let written = match (header("if-none-match").is_some(), rules.if_none_match) {
        (true, None) => return fake_error(501, "NotImplemented"),
        (true, Some(true)) => b.store.put_new(&path, Bytes::clone(&data)).await,
        _ => b.store.put(&path, Bytes::clone(&data)).await.map(|()| true),
    };
    match written {
        Ok(true) => {
            b.puts.fetch_add(1, Ordering::Relaxed);
            b.bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
            http::StatusCode::OK.into_response()
        }
        Ok(false) => fake_error(412, "PreconditionFailed"),
        Err(_) => fake_error(500, "InternalError"),
    }
}
