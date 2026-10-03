// SPDX-License-Identifier: Apache-2.0
//! A client for the daemon's socket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::api::{self, ErrorBody};

/// How long a request waits for the daemon's answer.
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum ClientError {
    /// Nothing answers on the socket.
    NotRunning { socket: PathBuf, why: String },
    /// The daemon answered with an error.
    Api { status: u16, code: String, message: String },
    /// The request failed once the daemon had it, or its answer wasn't understood.
    Failed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning { socket, .. } => write!(f, "the daemon isn't running (socket {})", socket.display()),
            ClientError::Api { message, .. } => f.write_str(message),
            ClientError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ClientError {}

/// Talks to the daemon on one socket. Cloning shares its connections.
#[derive(Clone, Debug)]
pub struct DaemonClient {
    http: reqwest::Client,
    socket: PathBuf,
}

impl DaemonClient {
    pub fn new(socket: &Path) -> DaemonClient {
        let http = reqwest::Client::builder().unix_socket(socket).build().expect("an HTTP client for a Unix socket");
        DaemonClient { http, socket: socket.to_owned() }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn url(path: &str) -> String {
        format!("http://localhost{path}")
    }

    fn sent(&self, e: reqwest::Error) -> ClientError {
        if e.is_connect() {
            let why = std::error::Error::source(&e).map_or_else(|| e.to_string(), |s| s.to_string());
            ClientError::NotRunning { socket: self.socket.clone(), why }
        } else {
            ClientError::Failed(format!("the daemon at {}: {e}", self.socket.display()))
        }
    }

    async fn answer<T: DeserializeOwned>(&self, r: reqwest::Response) -> Result<T, ClientError> {
        let status = r.status();
        let body = r.bytes().await.map_err(|e| self.sent(e))?;
        if !status.is_success() {
            return Err(match serde_json::from_slice::<ErrorBody>(&body) {
                Ok(b) => ClientError::Api { status: status.as_u16(), code: b.error.code, message: b.error.message },
                Err(_) => ClientError::Api { status: status.as_u16(), code: "UnexpectedResponse".into(), message: format!("the daemon answered {status}: {}", String::from_utf8_lossy(&body)) },
            });
        }
        serde_json::from_slice(&body).map_err(|e| ClientError::Failed(format!("the daemon's answer: {e}")))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let r = self.http.get(Self::url(path)).timeout(TIMEOUT).send().await.map_err(|e| self.sent(e))?;
        self.answer(r).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, ClientError> {
        self.send(reqwest::Method::POST, path, body).await
    }

    async fn send<B: Serialize, T: DeserializeOwned>(&self, method: reqwest::Method, path: &str, body: &B) -> Result<T, ClientError> {
        let body = serde_json::to_vec(body).expect("JSON");
        let r = self.http.request(method, Self::url(path)).timeout(TIMEOUT).header("content-type", "application/json").body(body).send().await.map_err(|e| self.sent(e))?;
        self.answer(r).await
    }

    pub async fn status(&self) -> Result<api::Status, ClientError> {
        self.get("/v1/status").await
    }

    pub async fn info(&self) -> Result<api::Info, ClientError> {
        self.get("/v1/info").await
    }

    /// Asks the daemon to stop; it answers first.
    pub async fn stop(&self) -> Result<api::Stopping, ClientError> {
        self.post("/v1/stop", &serde_json::json!({})).await
    }

    /// Hands a batch to the daemon's upload queue.
    pub async fn enqueue(&self, batch: &api::NewBatch) -> Result<api::Queued, ClientError> {
        self.post("/v1/uploads", batch).await
    }

    fn uploads_query(all: bool, batch: Option<i64>) -> String {
        let mut q = vec![];
        if all {
            q.push("all=true".to_owned());
        }
        if let Some(b) = batch {
            q.push(format!("batch={b}"));
        }
        if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) }
    }

    /// What is not yet published (everything, with `all`), and every batch, or only `batch`.
    pub async fn uploads(&self, all: bool, batch: Option<i64>) -> Result<api::UploadList, ClientError> {
        self.get(&format!("/v1/uploads{}", Self::uploads_query(all, batch))).await
    }

    /// The same, each second.
    pub async fn watch_uploads(&self, all: bool, batch: Option<i64>) -> Result<UploadWatch, ClientError> {
        let r = self.http.get(Self::url(&format!("/v1/uploads/watch{}", Self::uploads_query(all, batch)))).send().await.map_err(|e| self.sent(e))?;
        if !r.status().is_success() {
            return Err(self.answer::<serde_json::Value>(r).await.err().unwrap_or_else(|| ClientError::Failed("the daemon refused to watch".into())));
        }
        Ok(UploadWatch { client: self.clone(), response: r, buf: Vec::new() })
    }

    pub async fn pause(&self, scope: &api::Scope) -> Result<api::Affected, ClientError> {
        self.post("/v1/uploads/pause", scope).await
    }

    pub async fn resume(&self, scope: &api::Scope) -> Result<api::Affected, ClientError> {
        self.post("/v1/uploads/resume", scope).await
    }

    pub async fn cancel(&self, scope: &api::Scope) -> Result<api::Affected, ClientError> {
        self.post("/v1/uploads/cancel", scope).await
    }

    /// The upload bandwidth limit, in bytes a second; `None` lifts it.
    pub async fn limit(&self, bytes_per_second: Option<u64>) -> Result<api::Limit, ClientError> {
        self.send(reqwest::Method::PUT, "/v1/uploads/limit", &api::Limit { bytes_per_second }).await
    }

    /// Forgets finished uploads.
    pub async fn clear(&self) -> Result<api::Cleared, ClientError> {
        self.post("/v1/uploads/clear", &serde_json::json!({})).await
    }

    /// The mount table and the remembered mounts.
    pub async fn mounts(&self) -> Result<api::Mounts, ClientError> {
        self.get("/v1/mounts").await
    }

    /// Mounts a drive and remembers it.
    pub async fn mount(&self, m: &api::NewMount) -> Result<api::Mount, ClientError> {
        self.post("/v1/mounts", m).await
    }

    /// Unmounts and forgets a mountpoint, or every mount of a drive.
    pub async fn unmount(&self, target: &str) -> Result<api::Unmounted, ClientError> {
        self.post("/v1/mounts/unmount", &api::Unmount { target: target.into() }).await
    }
}

/// The upload queue each second, from `DaemonClient::watch_uploads`.
pub struct UploadWatch {
    client: DaemonClient,
    response: reqwest::Response,
    buf: Vec<u8>,
}

impl UploadWatch {
    /// The next snapshot; `None` once the daemon has stopped sending them.
    pub async fn next(&mut self) -> Option<Result<api::UploadList, ClientError>> {
        loop {
            if let Some(n) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=n).collect();
                return Some(serde_json::from_slice(&line).map_err(|e| ClientError::Failed(format!("the daemon's answer: {e}"))));
            }
            match self.response.chunk().await {
                Ok(Some(c)) => self.buf.extend_from_slice(&c),
                Ok(None) => return None,
                Err(e) => return Some(Err(self.client.sent(e))),
            }
        }
    }
}
