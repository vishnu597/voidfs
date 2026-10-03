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
        let body = serde_json::to_vec(body).expect("JSON");
        let r = self.http.post(Self::url(path)).timeout(TIMEOUT).header("content-type", "application/json").body(body).send().await.map_err(|e| self.sent(e))?;
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
}
