// SPDX-License-Identifier: Apache-2.0
//! A server over a memory pool, in this process, on a free port of 127.0.0.1: what the tests of the
//! SDK, the CLI and the client run against. It serves the S3 port as the binary does, with the
//! keys it is given, and with direct uploads and storage credentials through a [`FakeBucket`] if
//! asked.

use std::collections::HashMap;
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

use crate::credentials::{Location, Mint, Minted};
use crate::direct::{Presign, Presigned};
use crate::pool::Pool;
use crate::s3::{App, Domains};
use crate::sigv4::{KeyInfo, Keys, Scope};
use crate::store::Store;
use voidfs_core::ids::DriveId;

/// The admin key every test server accepts.
pub const ADMIN_KEY_ID: &str = "VFTESTADMINKEY234567";
pub const ADMIN_SECRET: &str = "testsecrettestsecrettestsecrettestsecret";

pub struct TestServer {
    /// `http://127.0.0.1:<port>`.
    pub endpoint: String,
    pub addr: SocketAddr,
    pub pool: Arc<Pool>,
    /// Where direct uploads go, and storage credentials reach, for a server started with them.
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
        TestServer::serve(keys, false, false).await
    }

    /// A server that accepts [`ADMIN_KEY_ID`] and `keys`, and offers direct uploads (protocol
    /// §4.11) to a [`FakeBucket`] over its pool's store, which binds what its URLs carry. The
    /// server checks the bucket as it would a real one when it starts.
    pub async fn with_direct_uploads(keys: Vec<KeyInfo>) -> anyhow::Result<TestServer> {
        TestServer::serve(keys, true, false).await
    }

    /// A server that accepts [`ADMIN_KEY_ID`] and `keys`, and offers storage credentials (protocol
    /// §5.5) that a [`FakeBucket`] over its pool's store mints and enforces. The server checks
    /// them as it would a real bucket's when it starts, with `rules`.
    pub async fn with_storage_credentials(keys: Vec<KeyInfo>, rules: Rules) -> anyhow::Result<TestServer> {
        TestServer::serve_with(keys, false, Some(rules)).await
    }

    async fn serve(keys: Vec<KeyInfo>, direct_uploads: bool, credentials: bool) -> anyhow::Result<TestServer> {
        TestServer::serve_with(keys, direct_uploads, credentials.then(Rules::default)).await
    }

    async fn serve_with(keys: Vec<KeyInfo>, direct_uploads: bool, credentials: Option<Rules>) -> anyhow::Result<TestServer> {
        let mut all = Keys::default();
        all.insert(KeyInfo { id: ADMIN_KEY_ID.into(), secret: ADMIN_SECRET.into(), scope: Scope::Admin, drives: None });
        for k in keys {
            all.insert(k);
        }
        let store = Store::memory()?;
        let pool = Pool::open(store.clone(), 64 << 20).await?;
        let bucket = match direct_uploads || credentials.is_some() {
            false => None,
            true => Some(FakeBucket::start(store).await?),
        };
        let http = opendal::raw::HttpClient::new()?;
        let direct = match (&bucket, direct_uploads) {
            (Some(b), true) => crate::probe::offer(Box::new(b.clone()), &http).await.1,
            _ => None,
        };
        let credentials = match (&bucket, credentials) {
            (Some(b), Some(rules)) => {
                b.set_rules(rules);
                crate::credentials::offer(Box::new(b.clone()), b.location(), &http).await.1
            }
            _ => None,
        };
        let app = Arc::new(App {
            pool: pool.clone(),
            keys: all,
            domains: Domains::new(Vec::new()),
            metrics: crate::metrics::S3Metrics::new(),
            uploads: Default::default(),
            read_ahead: Default::default(),
            direct,
            credentials,
        });
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
    /// Refuse a PUT without, or with other values of, the headers its URL was signed with, with
    /// this status: AWS, R2 and versitygw answer 403, MinIO 400. 0 accepts it.
    pub signed_headers: u16,
    /// For `If-None-Match: *`: `Some(true)` refuses an object that exists (412), `Some(false)`
    /// ignores the header, and `None` answers 501, as Backblaze B2 does.
    pub if_none_match: Option<bool>,
    /// Answer every PUT with this status, as a bucket the client can't use.
    pub refuse: Option<u16>,
    /// Mint storage credentials (protocol §5.5); without, minting fails, as on a bucket without STS.
    pub sts: bool,
    /// Hold minted credentials to what they were minted for (format §2's paths, read only);
    /// without, they reach everything, writes included, as credentials minted unscoped would.
    pub scope: bool,
    /// How long minted credentials last, if not as long as asked.
    pub lifetime: Option<Duration>,
    /// Objects per page of a listing, as S3's 1,000.
    pub list_page: usize,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { checksum: true, signed_headers: 403, if_none_match: Some(true), refuse: None, sts: true, scope: true, lifetime: None, list_page: 1000 }
    }
}

/// The bucket a [`FakeBucket`] holds the pool in, and the pool's root there, for requests signed
/// with the credentials it mints.
pub const FAKE_BUCKET: &str = "fake-bucket";
pub const FAKE_ROOT: &str = "pool/";

/// A stand-in for a bucket over a pool's [`Store`]: where the tests of direct uploads send shards,
/// and the tests of storage credentials read. It signs a presigned URL's path, expiry and headers
/// with a key of its own. It mints credentials that it holds to what they were minted for, and
/// verifies the SigV4 signatures and session tokens of requests made with them, which address
/// [`FAKE_BUCKET`] path-style, with the pool under [`FAKE_ROOT`].
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
    /// Credentials it minted, by access key id.
    issued: Mutex<HashMap<String, Issued>>,
    /// Requests made with them that it answered, by kind: minted, read (GET or HEAD of an object),
    /// listed, refused.
    minted: AtomicU64,
    reads: AtomicU64,
    lists: AtomicU64,
    refused: AtomicU64,
}

#[derive(Clone)]
struct Issued {
    secret: String,
    token: String,
    expires: chrono::DateTime<chrono::Utc>,
    readable: Vec<String>,
    listable: String,
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
        let state = Arc::new(FakeState {
            rules: Mutex::default(),
            puts: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            key: rand::random(),
            store,
            issued: Mutex::default(),
            minted: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            lists: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        });
        let served = state.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, axum::Router::new().fallback(fake_request).with_state(served)).await;
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

    /// Where the credentials it mints reach.
    pub fn location(&self) -> Location {
        Location { bucket: FAKE_BUCKET.into(), root: FAKE_ROOT.into(), region: "us-east-1".into(), endpoint: self.endpoint.clone() }
    }

    /// Forgets every credential it minted, as if they had been revoked: requests made with them
    /// are refused (403 InvalidAccessKeyId).
    pub fn revoke(&self) {
        self.state.issued.lock().unwrap().clear();
    }

    /// What it did with storage credentials so far.
    pub fn credential_use(&self) -> CredentialUse {
        let s = &self.state;
        CredentialUse { minted: s.minted.load(Ordering::Relaxed), reads: s.reads.load(Ordering::Relaxed), lists: s.lists.load(Ordering::Relaxed), refused: s.refused.load(Ordering::Relaxed) }
    }
}

/// What a [`FakeBucket`] did with storage credentials: how many it minted, and the requests made
/// with them that it answered or refused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CredentialUse {
    pub minted: u64,
    pub reads: u64,
    pub lists: u64,
    pub refused: u64,
}

fn random_text(alphabet: &[u8], n: usize) -> String {
    (0..n).map(|_| alphabet[rand::random_range(0..alphabet.len())] as char).collect()
}

impl Mint for FakeBucket {
    fn mint<'a>(&'a self, drive: &'a DriveId, ttl: Duration) -> BoxFuture<'a, anyhow::Result<Minted>> {
        Box::pin(async move {
            let rules = *self.state.rules.lock().unwrap();
            if !rules.sts {
                anyhow::bail!("STS AssumeRole answered HTTP 501: NotImplemented");
            }
            let id = format!("FAKE{}", random_text(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567", 16));
            let secret = random_text(b"abcdefghijklmnopqrstuvwxyz0123456789", 40);
            let token = random_text(b"abcdefghijklmnopqrstuvwxyz0123456789", 64);
            let expires = chrono::Utc::now() + rules.lifetime.unwrap_or(ttl);
            let issued = Issued { secret: secret.clone(), token: token.clone(), expires, readable: crate::credentials::readable(drive), listable: format!("drives/{drive}/") };
            self.state.issued.lock().unwrap().insert(id.clone(), issued);
            self.state.minted.fetch_add(1, Ordering::Relaxed);
            Ok(Minted { access_key_id: id, secret_access_key: secret, session_token: Some(token), expires_at: expires })
        })
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

async fn fake_request(State(b): State<Arc<FakeState>>, req: Request) -> Response {
    let signed = req.headers().get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256"));
    if signed { fake_s3(&b, req).await } else { fake_put(&b, req).await }
}

/// A request signed with credentials the bucket minted: a GET or HEAD of an object, a listing
/// (ListObjectsV2, by folder), or a write, which they may make only if they are not scoped.
async fn fake_s3(b: &FakeState, req: Request) -> Response {
    let rules = *b.rules.lock().unwrap();
    let (parts, body) = req.into_parts();
    let refused = |status: u16, code: &str| {
        b.refused.fetch_add(1, Ordering::Relaxed);
        fake_error(status, code)
    };
    let auth = parts.headers.get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let id = auth.split("Credential=").nth(1).and_then(|c| c.split('/').next()).unwrap_or_default().to_owned();
    let Some(issued) = b.issued.lock().unwrap().get(&id).cloned() else { return refused(403, "InvalidAccessKeyId") };
    let mut keys = Keys::default();
    keys.insert(KeyInfo { id, secret: issued.secret.clone(), scope: Scope::Read, drives: None });
    if crate::sigv4::verify(parts.method.as_str(), &parts.uri, &parts.headers, &keys, chrono::Utc::now()).is_err() {
        return refused(403, "SignatureDoesNotMatch");
    }
    if parts.headers.get("x-amz-security-token").and_then(|v| v.to_str().ok()) != Some(issued.token.as_str()) {
        return refused(403, "InvalidToken");
    }
    if chrono::Utc::now() >= issued.expires {
        return refused(400, "ExpiredToken");
    }
    let decoded = percent_encoding::percent_decode_str(parts.uri.path()).decode_utf8_lossy().into_owned();
    let path = decoded.trim_start_matches('/');
    let (bucket, key) = path.split_once('/').unwrap_or((path, ""));
    if bucket != FAKE_BUCKET {
        return fake_error(404, "NoSuchBucket");
    }
    let q: HashMap<String, String> = parts
        .uri
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_owned(), percent_encoding::percent_decode_str(v).decode_utf8_lossy().into_owned())
        })
        .collect();
    let reads = |path: &str| issued.readable.iter().any(|r| if r.ends_with('/') { path.starts_with(r.as_str()) } else { path == r });
    match parts.method {
        http::Method::GET if key.is_empty() && q.get("list-type").map(String::as_str) == Some("2") => {
            let prefix = q.get("prefix").cloned().unwrap_or_default();
            let Some(dir) = prefix.strip_prefix(FAKE_ROOT).map(str::to_owned) else { return refused(403, "AccessDenied") };
            if rules.scope && !dir.starts_with(&issued.listable) {
                return refused(403, "AccessDenied");
            }
            if q.get("delimiter").map(String::as_str) != Some("/") || !(dir.is_empty() || dir.ends_with('/')) {
                return fake_error(501, "NotImplemented");
            }
            // A continuation token is the last name of the page before.
            let token = q.get("continuation-token").cloned();
            let after = token.clone().or_else(|| q.get("start-after").and_then(|a| a.strip_prefix(&prefix)).map(str::to_owned));
            let (Ok(mut files), Ok(dirs)) = (b.store.list_files(&dir, after.as_deref()).await, b.store.list_dirs(&dir).await) else { return fake_error(500, "InternalError") };
            let dirs = if token.is_some() { Vec::new() } else { dirs };
            let truncated = files.len() > rules.list_page;
            files.truncate(rules.list_page);
            b.lists.fetch_add(1, Ordering::Relaxed);
            let contents: String = files.iter().map(|f| format!("<Contents><Key>{prefix}{f}</Key></Contents>")).collect();
            let common: String = dirs.iter().map(|d| format!("<CommonPrefixes><Prefix>{prefix}{d}/</Prefix></CommonPrefixes>")).collect();
            let next = match (truncated, files.last()) {
                (true, Some(last)) => format!("<NextContinuationToken>{last}</NextContinuationToken>"),
                _ => String::new(),
            };
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListBucketResult><Name>{FAKE_BUCKET}</Name><Prefix>{prefix}</Prefix><KeyCount>{}</KeyCount><IsTruncated>{truncated}</IsTruncated>{next}{contents}{common}</ListBucketResult>",
                files.len() + dirs.len()
            );
            (http::StatusCode::OK, [(http::header::CONTENT_TYPE, "application/xml")], xml).into_response()
        }
        http::Method::GET | http::Method::HEAD => {
            let Some(path) = key.strip_prefix(FAKE_ROOT) else { return refused(403, "AccessDenied") };
            if rules.scope && !reads(path) {
                return refused(403, "AccessDenied");
            }
            let data = match b.store.get(path).await {
                Ok(Some(d)) => d,
                Ok(None) => return fake_error(404, "NoSuchKey"),
                Err(_) => return fake_error(500, "InternalError"),
            };
            b.reads.fetch_add(1, Ordering::Relaxed);
            let range = parts.headers.get(http::header::RANGE).and_then(|v| v.to_str().ok()).and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-'));
            let (status, body) = match range {
                Some((from, to)) => {
                    let len = data.len() as u64;
                    let from: u64 = from.parse().unwrap_or(0);
                    let to: u64 = to.parse().map(|t: u64| t.min(len.saturating_sub(1))).unwrap_or(len.saturating_sub(1));
                    if from >= len {
                        return fake_error(416, "InvalidRange");
                    }
                    (http::StatusCode::PARTIAL_CONTENT, data.slice(from as usize..to as usize + 1))
                }
                None => (http::StatusCode::OK, data),
            };
            let len = body.len().to_string();
            let body = if parts.method == http::Method::HEAD { Bytes::new() } else { body };
            (status, [(http::header::CONTENT_LENGTH, len)], body).into_response()
        }
        _ if rules.scope => refused(403, "AccessDenied"),
        http::Method::PUT => {
            let Some(path) = key.strip_prefix(FAKE_ROOT) else { return refused(403, "AccessDenied") };
            let Ok(data) = axum::body::to_bytes(body, 64 << 20).await else { return fake_error(400, "IncompleteBody") };
            match b.store.put(path, data).await {
                Ok(()) => http::StatusCode::OK.into_response(),
                Err(_) => fake_error(500, "InternalError"),
            }
        }
        _ => fake_error(501, "NotImplemented"),
    }
}

async fn fake_put(b: &FakeState, req: Request) -> Response {
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
    if rules.signed_headers != 0 {
        let sent: Vec<(String, String)> = q.get("headers").map(|h| h.split(',').filter(|n| !n.is_empty()).map(|n| (n.to_owned(), header(n).unwrap_or_default())).collect()).unwrap_or_default();
        if q.get("sig") != Some(&b.sign(&path, expires, &sent)) {
            return fake_error(rules.signed_headers, if rules.signed_headers == 403 { "SignatureDoesNotMatch" } else { "AccessDenied" });
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
