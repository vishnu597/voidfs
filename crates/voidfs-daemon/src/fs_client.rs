// SPDX-License-Identifier: Apache-2.0
//! Typed, bounded filesystem calls over the daemon's existing Unix socket.

use std::time::Duration;

use bytes::Bytes;
use serde::{Serialize, de::DeserializeOwned};
use voidfs_client::mount::{Attr, Capabilities, Conflict, ConflictSide, RenameMode, XattrMode};

use crate::{ClientError, DaemonClient, api::fs};

const TIMEOUT: Duration = Duration::from_secs(30);
type Result<T> = std::result::Result<T, FsClientError>;

#[derive(Debug)]
pub enum FsClientError {
    Transport(ClientError),
    Api { status: u16, code: String, message: String, errno: i32 },
    Failed(String),
}

impl std::fmt::Display for FsClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => std::fmt::Display::fmt(error, f),
            Self::Api { message, .. } | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for FsClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self { Self::Transport(error) => Some(error), _ => None }
    }
}

impl From<ClientError> for FsClientError {
    fn from(error: ClientError) -> Self { Self::Transport(error) }
}

fn json<B: Serialize>(body: &B) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(body).map_err(|e| FsClientError::Failed(e.to_string()))?;
    if bytes.len() > fs::MAX_JSON { return Err(FsClientError::Failed("filesystem request exceeds the JSON limit".into())); }
    Ok(bytes)
}

async fn bounded(client: &DaemonClient, mut response: reqwest::Response, limit: usize) -> Result<Bytes> {
    if response.content_length().is_some_and(|length| length > limit as u64) {
        return Err(FsClientError::Failed("filesystem response exceeds its size limit".into()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| client.sent(e))? {
        if body.len().checked_add(chunk.len()).is_none_or(|length| length > limit) {
            return Err(FsClientError::Failed("filesystem response exceeds its size limit".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn refused(status: reqwest::StatusCode, body: &[u8]) -> FsClientError {
    if let Ok(body) = serde_json::from_slice::<fs::FsErrorBody>(body) {
        return FsClientError::Api { status: status.as_u16(), code: body.error.code, message: body.error.message, errno: body.error.errno };
    }
    if let Ok(body) = serde_json::from_slice::<crate::api::ErrorBody>(body) {
        return FsClientError::Api { status: status.as_u16(), code: body.error.code, message: body.error.message, errno: 0 };
    }
    FsClientError::Api { status: status.as_u16(), code: "UnexpectedResponse".into(), message: format!("the daemon answered {status}"), errno: 0 }
}

async fn answer<T: DeserializeOwned>(client: &DaemonClient, response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let body = bounded(client, response, fs::MAX_RESPONSE).await?;
    if !status.is_success() { return Err(refused(status, &body)); }
    serde_json::from_slice(&body).map_err(|e| FsClientError::Failed(format!("the daemon's filesystem answer: {e}")))
}

/// Percent-encodes a query value: everything but RFC 3986's unreserved characters.
fn encode(value: &str) -> String {
    value.bytes().map(|byte| if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) { (byte as char).to_string() } else { format!("%{byte:02X}") }).collect()
}

fn timestamp(at: std::time::SystemTime) -> Result<String> {
    let (seconds, nanos) = match at.duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => (i64::try_from(after.as_secs()).ok(), after.subsec_nanos()),
        Err(before) => {
            let before = before.duration();
            let carry = u64::from(before.subsec_nanos() > 0);
            (i64::try_from(before.as_secs().saturating_add(carry)).ok().map(|s| -s), if carry == 1 { 1_000_000_000 - before.subsec_nanos() } else { 0 })
        }
    };
    seconds.and_then(|seconds| chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos))
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true))
        .ok_or_else(|| FsClientError::Failed("modification time out of range".into()))
}

/// A logical session owned by the process that created it. Clones and new socket connections
/// work within that process; another process must create its own session.
#[derive(Clone, Debug)]
pub struct FsClient { client: DaemonClient, info: fs::SessionInfo }

impl DaemonClient {
    pub async fn session(&self, drive: &str, read_only: bool) -> Result<FsClient> {
        let request = fs::NewSession { version: fs::VERSION, drive: drive.to_owned(), read_only };
        let response = self.http.post(Self::url("/v1/fs/sessions")).timeout(TIMEOUT)
            .header("content-type", "application/json").body(json(&request)?).send().await.map_err(|e| self.sent(e))?;
        let info: fs::SessionInfo = answer(self, response).await?;
        if info.version != fs::VERSION || info.generation == 0 || info.id.is_empty() || info.id.len() > 256
            || !info.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            || info.max_io == 0 || info.max_io > fs::MAX_IO || info.max_entries == 0 || info.max_entries > fs::MAX_ENTRIES {
            return Err(FsClientError::Failed("the daemon returned invalid filesystem session limits or identity".into()));
        }
        Ok(FsClient { client: self.clone(), info })
    }
}

impl FsClient {
    pub fn info(&self) -> &fs::SessionInfo { &self.info }

    pub fn capabilities(&self) -> &Capabilities { &self.info.capabilities }

    fn path(&self, operation: &str) -> String { format!("/v1/fs/{}/{operation}", self.info.id) }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client.http.request(method, DaemonClient::url(path)).header("x-voidfs-generation", self.info.generation.to_string())
    }

    async fn post<B: Serialize, T: DeserializeOwned>(&self, operation: &str, body: &B) -> Result<T> {
        let response = self.request(reqwest::Method::POST, &self.path(operation)).timeout(TIMEOUT)
            .header("content-type", "application/json").body(json(body)?).send().await.map_err(|e| self.client.sent(e))?;
        answer(&self.client, response).await
    }

    pub async fn lookup(&self, parent: u64, name: &str) -> Result<Attr> {
        self.post("lookup", &fs::Lookup { parent, name: name.to_owned() }).await
    }

    pub async fn getattr(&self, ino: u64) -> Result<Attr> { self.post("getattr", &fs::Inode { ino }).await }

    pub async fn readdir(&self, ino: u64, after: Option<&str>, limit: usize) -> Result<fs::ReadDirReply> {
        if limit > self.info.max_entries { return Err(FsClientError::Failed("directory request exceeds the entry limit".into())); }
        let reply: fs::ReadDirReply = self.post("readdir", &fs::ReadDir { ino, after: after.map(str::to_owned), limit }).await?;
        if reply.entries.len() > limit { return Err(FsClientError::Failed("directory response exceeds the requested entry limit".into())); }
        Ok(reply)
    }

    pub async fn open(&self, ino: u64, write: bool) -> Result<fs::Opened> { self.post("open", &fs::Open { ino, write }).await }

    pub async fn handle_attr(&self, fh: u64) -> Result<Attr> { self.post("handle_attr", &fs::Handle { fh }).await }

    pub async fn read(&self, fh: u64, offset: u64, length: u64) -> Result<Bytes> {
        if length > self.info.max_io || offset.checked_add(length).is_none() {
            return Err(FsClientError::Failed("file read exceeds the range or I/O limit".into()));
        }
        let path = format!("{}?fh={fh}&offset={offset}&length={length}", self.path("read"));
        let response = self.request(reqwest::Method::GET, &path).timeout(TIMEOUT).send().await.map_err(|e| self.client.sent(e))?;
        let status = response.status();
        if !status.is_success() { return Err(refused(status, &bounded(&self.client, response, fs::MAX_RESPONSE).await?)); }
        bounded(&self.client, response, length as usize).await
    }

    pub async fn write(&self, fh: u64, offset: u64, bytes: Bytes) -> Result<usize> {
        let length = bytes.len();
        if length as u64 > self.info.max_io || offset.checked_add(length as u64).is_none() {
            return Err(FsClientError::Failed("file write exceeds the range or I/O limit".into()));
        }
        let path = format!("{}?fh={fh}&offset={offset}", self.path("write"));
        let response = self.request(reqwest::Method::PUT, &path).timeout(TIMEOUT)
            .header("content-type", "application/octet-stream").body(bytes).send().await.map_err(|e| self.client.sent(e))?;
        let result: fs::Written = answer(&self.client, response).await?;
        if result.written > length { return Err(FsClientError::Failed("file write answer exceeds the bytes sent".into())); }
        Ok(result.written)
    }

    pub async fn truncate(&self, fh: u64, size: u64) -> Result<()> {
        let _: fs::Empty = self.post("truncate", &fs::Truncate { fh, size }).await?;
        Ok(())
    }

    pub async fn fsync(&self, fh: u64) -> Result<()> {
        let _: fs::Empty = self.post("fsync", &fs::Handle { fh }).await?;
        Ok(())
    }

    pub async fn close(&self, fh: u64) -> Result<()> {
        let _: fs::Empty = self.post("close", &fs::Handle { fh }).await?;
        Ok(())
    }

    pub async fn readlink(&self, ino: u64) -> Result<String> {
        let link: fs::Link = self.post("readlink", &fs::Inode { ino }).await?;
        Ok(link.target)
    }

    /// Creates an empty file exclusively.
    pub async fn create(&self, parent: u64, name: &str, mode: u32) -> Result<Attr> {
        self.post("create", &fs::Create { parent, name: name.to_owned(), mode }).await
    }

    pub async fn mkdir(&self, parent: u64, name: &str, mode: u32) -> Result<Attr> {
        self.post("mkdir", &fs::Create { parent, name: name.to_owned(), mode }).await
    }

    pub async fn unlink(&self, parent: u64, name: &str) -> Result<()> {
        let _: fs::Empty = self.post("unlink", &fs::Lookup { parent, name: name.to_owned() }).await?;
        Ok(())
    }

    pub async fn rmdir(&self, parent: u64, name: &str) -> Result<()> {
        let _: fs::Empty = self.post("rmdir", &fs::Lookup { parent, name: name.to_owned() }).await?;
        Ok(())
    }

    pub async fn rename(&self, from_parent: u64, from_name: &str, to_parent: u64, to_name: &str, how: RenameMode) -> Result<()> {
        let request = fs::Rename { from_parent, from_name: from_name.to_owned(), to_parent, to_name: to_name.to_owned(), how: how.into() };
        let _: fs::Empty = self.post("rename", &request).await?;
        Ok(())
    }

    /// Refused: the core has no hard links (`Capabilities::hard_links`).
    pub async fn link(&self, ino: u64, parent: u64, name: &str) -> Result<Attr> {
        self.post("link", &fs::LinkTo { ino, parent, name: name.to_owned() }).await
    }

    /// Refused: the core doesn't clone (`Capabilities::clone`).
    pub async fn clone_file(&self, ino: u64, parent: u64, name: &str) -> Result<Attr> {
        self.post("clone_file", &fs::LinkTo { ino, parent, name: name.to_owned() }).await
    }

    /// `None` leaves the mode or the modification time as it is.
    pub async fn setattr(&self, ino: u64, mode: Option<u32>, mtime: Option<std::time::SystemTime>) -> Result<Attr> {
        self.post("setattr", &fs::SetAttr { ino, mode, mtime: mtime.map(timestamp).transpose()? }).await
    }

    /// The raw value, empty included.
    pub async fn getxattr(&self, ino: u64, name: &str) -> Result<Bytes> {
        let path = format!("{}?ino={ino}&name={}", self.path("getxattr"), encode(name));
        let response = self.request(reqwest::Method::GET, &path).timeout(TIMEOUT).send().await.map_err(|e| self.client.sent(e))?;
        let status = response.status();
        if !status.is_success() { return Err(refused(status, &bounded(&self.client, response, fs::MAX_RESPONSE).await?)); }
        bounded(&self.client, response, fs::MAX_XATTR).await
    }

    pub async fn listxattr(&self, ino: u64) -> Result<Vec<String>> {
        let reply: fs::XattrNames = self.post("listxattr", &fs::Inode { ino }).await?;
        Ok(reply.names)
    }

    pub async fn setxattr(&self, ino: u64, name: &str, value: &[u8], how: XattrMode) -> Result<()> {
        if value.len() > fs::MAX_XATTR { return Err(FsClientError::Failed("extended attribute exceeds the size limit".into())); }
        let how = match fs::XattrHow::from(how) { fs::XattrHow::Set => "set", fs::XattrHow::Create => "create", fs::XattrHow::Replace => "replace" };
        let path = format!("{}?ino={ino}&name={}&how={how}", self.path("setxattr"), encode(name));
        let response = self.request(reqwest::Method::PUT, &path).timeout(TIMEOUT)
            .header("content-type", "application/octet-stream").body(value.to_vec()).send().await.map_err(|e| self.client.sent(e))?;
        let _: fs::Empty = answer(&self.client, response).await?;
        Ok(())
    }

    pub async fn removexattr(&self, ino: u64, name: &str) -> Result<()> {
        let _: fs::Empty = self.post("removexattr", &fs::Xattr { ino, name: name.to_owned() }).await?;
        Ok(())
    }

    /// The retained versions of a conflicted save the inode belongs to, if any.
    pub async fn conflict(&self, ino: u64) -> Result<Option<Conflict>> {
        let reply: fs::ConflictReply = self.post("conflict", &fs::Inode { ino }).await?;
        Ok(reply.conflict)
    }

    pub async fn read_conflict(&self, ino: u64, side: ConflictSide, offset: u64, length: u64) -> Result<Bytes> {
        if length > self.info.max_io || offset.checked_add(length).is_none() {
            return Err(FsClientError::Failed("conflict read exceeds the range or I/O limit".into()));
        }
        let side = match fs::Side::from(side) { fs::Side::Local => "local", fs::Side::Remote => "remote" };
        let path = format!("{}?ino={ino}&side={side}&offset={offset}&length={length}", self.path("read_conflict"));
        let response = self.request(reqwest::Method::GET, &path).timeout(TIMEOUT).send().await.map_err(|e| self.client.sent(e))?;
        let status = response.status();
        if !status.is_success() { return Err(refused(status, &bounded(&self.client, response, fs::MAX_RESPONSE).await?)); }
        bounded(&self.client, response, length as usize).await
    }

    pub async fn release(&self) -> Result<()> {
        let _: fs::Empty = self.post("release", &fs::Empty {}).await?;
        Ok(())
    }

    pub async fn watch(&self) -> Result<InvalidationWatch> {
        let request = self.request(reqwest::Method::GET, &self.path("watch")).send();
        let response = tokio::time::timeout(TIMEOUT, request).await.map_err(|_| FsClientError::Failed("filesystem watch connection timed out".into()))?
            .map_err(|e| self.client.sent(e))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = tokio::time::timeout(TIMEOUT, bounded(&self.client, response, fs::MAX_RESPONSE)).await
                .map_err(|_| FsClientError::Failed("filesystem watch error response timed out".into()))??;
            return Err(refused(status, &body));
        }
        Ok(InvalidationWatch { client: self.client.clone(), response, buf: Vec::new(), generation: None,
            initial_generation: self.info.metadata_generation, ended: false })
    }
}

#[derive(Debug)]
pub struct InvalidationWatch {
    client: DaemonClient,
    response: reqwest::Response,
    buf: Vec<u8>,
    generation: Option<u64>,
    initial_generation: u64,
    ended: bool,
}

impl InvalidationWatch {
    fn fail(&mut self, error: FsClientError) -> Option<Result<fs::InvalidationEvent>> {
        self.ended = true;
        self.buf.clear();
        Some(Err(error))
    }

    pub async fn next(&mut self) -> Option<Result<fs::InvalidationEvent>> {
        if self.ended { return None; }
        loop {
            if let Some(end) = self.buf.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=end).collect();
                let event = match serde_json::from_slice::<fs::InvalidationEvent>(&line) {
                    Ok(event) => event,
                    Err(e) => return self.fail(FsClientError::Failed(format!("the daemon's invalidation: {e}"))),
                };
                let valid = match self.generation {
                    Some(previous) => event.generation > previous,
                    None => event.generation >= self.initial_generation && event.resync && event.invalidations.contains(&fs::Invalidation::All),
                };
                if !valid { return self.fail(FsClientError::Failed("filesystem invalidation generation regressed or initial resync is missing".into())); }
                self.generation = Some(event.generation);
                return Some(Ok(event));
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => {
                    if self.buf.len().checked_add(chunk.len()).is_none_or(|length| length > fs::MAX_RESPONSE) {
                        return self.fail(FsClientError::Failed("filesystem invalidation exceeds the response limit".into()));
                    }
                    self.buf.extend_from_slice(&chunk);
                }
                Ok(None) => {
                    if !self.buf.is_empty() { return self.fail(FsClientError::Failed("filesystem invalidation ended before its newline".into())); }
                    self.ended = true;
                    return None;
                }
                Err(e) => return self.fail(self.client.sent(e).into()),
            }
        }
    }
}
