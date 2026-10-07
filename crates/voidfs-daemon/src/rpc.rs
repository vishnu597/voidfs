// SPDX-License-Identifier: Apache-2.0
//! Per-consumer handles on one drive core, with bounded socket calls and owned release.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::body::{Body, HttpBody, to_bytes};
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore, watch};
use voidfs_client::mount::FsError;

use crate::api::fs;
use crate::server::Shared;
use crate::sessions::DriveSession;

pub(crate) struct Failure { pub status: StatusCode, pub error: fs::FsError }

impl Failure {
    fn new(status: StatusCode, code: &str, error: FsError) -> Self {
        Self { status, error: fs::FsError { code: code.into(), message: error.to_string(), errno: error.errno() } }
    }
    fn invalid() -> Self { Self::new(StatusCode::BAD_REQUEST, "InvalidArgument", FsError::InvalidArgument) }
    fn busy() -> Self { Self::new(StatusCode::SERVICE_UNAVAILABLE, "Busy", FsError::Again) }
    fn large() -> Self { Self::new(StatusCode::PAYLOAD_TOO_LARGE, "TooLarge", FsError::InvalidArgument) }
}

impl From<FsError> for Failure {
    fn from(error: FsError) -> Self {
        let (status, code) = match &error {
            FsError::NotFound => (StatusCode::NOT_FOUND, "NotFound"),
            FsError::Exists => (StatusCode::CONFLICT, "Exists"),
            FsError::NotEmpty => (StatusCode::CONFLICT, "NotEmpty"),
            FsError::IsDir => (StatusCode::BAD_REQUEST, "IsDir"),
            FsError::NotDir => (StatusCode::BAD_REQUEST, "NotDir"),
            FsError::NoSpace => (StatusCode::INSUFFICIENT_STORAGE, "NoSpace"),
            FsError::ReadOnly => (StatusCode::FORBIDDEN, "ReadOnly"),
            FsError::Unsupported => (StatusCode::NOT_IMPLEMENTED, "Unsupported"),
            FsError::Stale => (StatusCode::CONFLICT, "Stale"),
            FsError::BadHandle => (StatusCode::BAD_REQUEST, "BadHandle"),
            FsError::Offline => (StatusCode::SERVICE_UNAVAILABLE, "Offline"),
            FsError::Permission => (StatusCode::FORBIDDEN, "Permission"),
            FsError::InvalidName => (StatusCode::BAD_REQUEST, "InvalidName"),
            FsError::InvalidArgument => (StatusCode::BAD_REQUEST, "InvalidArgument"),
            FsError::NoAttr => (StatusCode::NOT_FOUND, "NoAttr"),
            FsError::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "TooLarge"),
            FsError::Ambiguous => (StatusCode::CONFLICT, "Ambiguous"),
            FsError::Again => (StatusCode::SERVICE_UNAVAILABLE, "Again"),
            FsError::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Io"),
        };
        Self::new(status, code, error)
    }
}

impl From<crate::server::Failure> for Failure {
    fn from(error: crate::server::Failure) -> Self {
        let native = match error.0.as_u16() {
            400 => FsError::InvalidArgument, 401 | 403 => FsError::Permission, 404 => FsError::NotFound,
            409 | 503 => FsError::Again, 502 => FsError::Offline, _ => FsError::Io(error.2.clone()),
        };
        Self { status: error.0, error: fs::FsError { code: error.1, message: error.2, errno: error.3.unwrap_or_else(|| native.errno()) } }
    }
}

impl IntoResponse for Failure {
    fn into_response(mut self) -> Response {
        clip(&mut self.error.code, 256);
        clip(&mut self.error.message, 4096);
        let mut response = (self.status, Json(fs::FsErrorBody { error: self.error })).into_response();
        if self.status == StatusCode::PAYLOAD_TOO_LARGE { response.headers_mut().insert(header::CONNECTION, header::HeaderValue::from_static("close")); }
        response
    }
}

fn clip(value: &mut String, maximum: usize) {
    if value.len() <= maximum { return; }
    let mut end = maximum;
    while !value.is_char_boundary(end) { end -= 1; }
    value.truncate(end);
}

struct Access {
    drive: Arc<DriveSession>,
    read_only: bool,
    handles: Mutex<HashMap<u64, OwnedSemaphorePermit>>,
    slots: Arc<Semaphore>,
    gate: Arc<RwLock<()>>,
    closed: watch::Sender<bool>,
    _slot: OwnedSemaphorePermit,
}

impl Access {
    fn handle(&self, fh: u64) -> Result<(), Failure> {
        if fh >> 32 > 0 && fh >> 32 < u64::from(self.drive.core.generation()) { return Err(FsError::Stale.into()); }
        if !self.handles.lock().unwrap_or_else(|p| p.into_inner()).contains_key(&fh) { return Err(FsError::BadHandle.into()); }
        Ok(())
    }
    fn writable(&self) -> Result<(), Failure> { if self.read_only { Err(FsError::ReadOnly.into()) } else { Ok(()) } }

    async fn drain(self: Arc<Self>) -> Result<(), Failure> {
        let _gate = self.gate.write().await;
        let handles = self.handles.lock().unwrap_or_else(|p| p.into_inner()).keys().copied().collect::<Vec<_>>();
        let mut first = None;
        for fh in handles {
            match self.drive.core.close(fh).await {
                Ok(()) => { self.handles.lock().unwrap_or_else(|p| p.into_inner()).remove(&fh); },
                Err(error) if first.is_none() => first = Some(error),
                Err(_) => {},
            }
        }
        first.map_or(Ok(()), |error| Err(error.into()))
    }
}

pub(crate) struct Calls {
    active: Mutex<HashMap<String, Arc<Access>>>,
    next: Mutex<u64>,
    calls: Arc<Semaphore>,
    sessions: Arc<Semaphore>,
    watches: Arc<Semaphore>,
}

impl Calls {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { active: Mutex::new(HashMap::new()), next: Mutex::new(1), calls: Arc::new(Semaphore::new(fs::MAX_CALLS)),
            sessions: Arc::new(Semaphore::new(fs::MAX_SESSIONS)), watches: Arc::new(Semaphore::new(fs::MAX_WATCHES)) })
    }

    fn access(&self, id: &str, request: &Request) -> Result<Arc<Access>, Failure> {
        let generation: u32 = request.headers().get("x-voidfs-generation").and_then(|h| h.to_str().ok()).and_then(|h| h.parse().ok()).ok_or_else(Failure::invalid)?;
        let access = self.active.lock().unwrap_or_else(|p| p.into_inner()).get(id).cloned()
            .ok_or_else(|| Failure::new(StatusCode::CONFLICT, "StaleSession", FsError::Stale))?;
        if generation != access.drive.core.generation() { return Err(Failure::new(StatusCode::CONFLICT, "StaleGeneration", FsError::Stale)); }
        Ok(access)
    }

    async fn release(self: &Arc<Self>, id: &str, access: Arc<Access>, permit: Arc<OwnedSemaphorePermit>) -> Result<(), Failure> {
        if access.closed.send_replace(true) { return Err(Failure::new(StatusCode::CONFLICT, "StaleSession", FsError::Stale)); }
        let (this, id) = (self.clone(), id.to_owned());
        tokio::spawn(async move {
            let _permit = permit;
            let result = access.clone().drain().await;
            if result.is_ok() { this.active.lock().unwrap_or_else(|p| p.into_inner()).remove(&id); }
            else { access.closed.send_replace(false); }
            result
        }).await.map_err(|e| Failure::from(FsError::Io(e.to_string())))?
    }

    pub async fn shutdown(&self) {
        let active = std::mem::take(&mut *self.active.lock().unwrap_or_else(|p| p.into_inner()));
        let tasks = active.into_values().map(|access| {
            access.closed.send_replace(true);
            tokio::spawn(access.drain())
        }).collect::<Vec<_>>();
        for task in tasks { let _ = task.await; }
    }
}

async fn admit(State(s): State<Arc<Shared>>, mut request: Request, next: Next) -> Result<Response, Failure> {
    if *s.closing.borrow() { return Err(Failure::busy()); }
    let permit = Arc::new(s.rpc.calls.clone().try_acquire_owned().map_err(|_| Failure::busy())?);
    request.extensions_mut().insert(permit.clone());
    let watch = request.uri().path().ends_with("/watch");
    let response = next.run(request).await;
    if watch && response.status().is_success() { return Ok(response); }
    let (mut parts, body) = response.into_parts();
    if let Some(length) = body.size_hint().exact() { parts.headers.insert(header::CONTENT_LENGTH, header::HeaderValue::from(length)); }
    let stream = futures::stream::unfold((body.into_data_stream(), permit), |(mut body, permit)| async move {
        body.next().await.map(|chunk| (chunk, (body, permit)))
    });
    Ok(Response::from_parts(parts, Body::from_stream(stream)))
}

pub(crate) fn routes(shared: Arc<Shared>) -> Router<Arc<Shared>> {
    Router::new().route("/sessions", post(create))
        .route("/{id}/{op}", post(call))
        .route("/{id}/read", get(read))
        .route("/{id}/write", put(write))
        .route("/{id}/watch", get(watch_events))
        .fallback(|| async { Failure::new(StatusCode::NOT_FOUND, "NoSuchOperation", FsError::Unsupported) })
        .method_not_allowed_fallback(|| async { Failure::new(StatusCode::METHOD_NOT_ALLOWED, "Unsupported", FsError::Unsupported) })
        .route_layer(middleware::from_fn_with_state(shared, admit))
}

async fn body(request: Request, maximum: usize) -> Result<Bytes, Failure> {
    if request.headers().get(header::CONTENT_LENGTH).and_then(|h| h.to_str().ok()).and_then(|h| h.parse::<u64>().ok()).is_some_and(|n| n > maximum as u64) { return Err(Failure::large()); }
    to_bytes(request.into_body(), maximum).await.map_err(|_| Failure::large())
}

async fn json<T: DeserializeOwned>(request: Request) -> Result<T, Failure> {
    let bytes = body(request, fs::MAX_JSON).await?;
    decode(&bytes)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Failure> { serde_json::from_slice(bytes).map_err(|_| Failure::invalid()) }

fn permit(request: &Request) -> Arc<OwnedSemaphorePermit> { request.extensions().get::<Arc<OwnedSemaphorePermit>>().expect("call admission").clone() }

fn answer<T: Serialize>(value: T) -> Result<Response, Failure> {
    let bytes = serde_json::to_vec(&value).map_err(|e| FsError::Io(e.to_string()))?;
    if bytes.len() > fs::MAX_RESPONSE { return Err(Failure::large()); }
    Ok(Response::builder().header(header::CONTENT_TYPE, "application/json").body(Body::from(bytes)).expect("a response"))
}

async fn create(State(s): State<Arc<Shared>>, request: Request) -> Result<Response, Failure> {
    let permit = permit(&request);
    let new: fs::NewSession = json(request).await?;
    tokio::spawn(async move {
    let _permit = permit;
    if new.version != fs::VERSION { return Err(Failure::new(StatusCode::UPGRADE_REQUIRED, "UnsupportedVersion", FsError::Unsupported)); }
    if new.drive.is_empty() || new.drive.len() > 1024 || new.drive.contains(['/', '\0']) { return Err(Failure::invalid()); }
    let slot = s.rpc.sessions.clone().try_acquire_owned().map_err(|_| Failure::busy())?;
    let drive = s.sessions.drive(&s, &new.drive).await?;
    if !drive.pinned { return Err(FsError::Offline.into()); }
    let serial = {
        let mut next = s.rpc.next.lock().unwrap_or_else(|p| p.into_inner());
        let serial = *next;
        *next = serial.checked_add(1).ok_or_else(Failure::busy)?;
        serial
    };
    let generation = drive.core.generation();
    let id = format!("{generation:08x}-{serial:016x}");
    let info = fs::SessionInfo { version: fs::VERSION, id: id.clone(), drive: drive.drive.clone(), root: drive.core.root(), generation,
        metadata_generation: drive.metadata_generation(), read_only: new.read_only, max_io: fs::MAX_IO, max_entries: fs::MAX_ENTRIES };
    let access = Arc::new(Access { drive, read_only: new.read_only, handles: Mutex::new(HashMap::new()), slots: Arc::new(Semaphore::new(fs::MAX_HANDLES)),
        gate: Arc::new(RwLock::new(())), closed: watch::Sender::new(false), _slot: slot });
    {
        let mut active = s.rpc.active.lock().unwrap_or_else(|p| p.into_inner());
        if *s.closing.borrow() { return Err(Failure::busy()); }
        active.insert(id, access);
    }
    answer(info)
    }).await.map_err(|e| Failure::from(FsError::Io(e.to_string())))?
}

async fn call(State(s): State<Arc<Shared>>, Path((id, op)): Path<(String, String)>, request: Request) -> Result<Response, Failure> {
    let access = s.rpc.access(&id, &request)?;
    let permit = permit(&request);
    let bytes = body(request, fs::MAX_JSON).await?;
    if op == "release" {
        let _: fs::Empty = decode(&bytes)?;
        s.rpc.release(&id, access, permit).await?;
        return answer(fs::Empty::default());
    }
    let gate = access.gate.clone().read_owned().await;
    if *access.closed.borrow() { return Err(Failure::new(StatusCode::CONFLICT, "StaleSession", FsError::Stale)); }
    tokio::spawn(async move {
    let (_gate, _permit) = (gate, permit);
    let core = &access.drive.core;
    match op.as_str() {
        "lookup" => { let q: fs::Lookup = decode(&bytes)?; answer(core.lookup(q.parent, &q.name).await?) },
        "getattr" => { let q: fs::Inode = decode(&bytes)?; answer(core.getattr(q.ino).await?) },
        "readlink" => { let q: fs::Inode = decode(&bytes)?; answer(fs::Link { target: core.readlink(q.ino).await? }) },
        "readdir" => {
            let q: fs::ReadDir = decode(&bytes)?;
            if q.limit == 0 || q.limit > fs::MAX_ENTRIES || q.after.as_ref().is_some_and(|s| s.len() > 1024) { return Err(Failure::invalid()); }
            let generation = access.drive.metadata_generation();
            let entries = core.readdir(q.ino, q.after.as_deref(), q.limit).await?;
            if generation != access.drive.metadata_generation() { return Err(FsError::Again.into()); }
            answer(fs::ReadDirReply { entries, generation })
        },
        "open" => {
            let q: fs::Open = decode(&bytes)?;
            if q.write { access.writable()?; }
            let slot = access.slots.clone().try_acquire_owned().map_err(|_| Failure::busy())?;
            let fh = core.open(q.ino, q.write).await?;
            access.handles.lock().unwrap_or_else(|p| p.into_inner()).insert(fh, slot);
            answer(fs::Opened { fh, attr: core.handle_attr(fh)? })
        },
        "handle_attr" => { let q: fs::Handle = decode(&bytes)?; access.handle(q.fh)?; answer(core.handle_attr(q.fh)?) },
        "close" => {
            let q: fs::Handle = decode(&bytes)?;
            access.handle(q.fh)?;
            core.close(q.fh).await?;
            access.handles.lock().unwrap_or_else(|p| p.into_inner()).remove(&q.fh);
            answer(fs::Empty::default())
        },
        "truncate" => {
            let q: fs::Truncate = decode(&bytes)?;
            access.writable()?;
            access.handle(q.fh)?;
            let ino = core.handle_attr(q.fh)?.ino;
            core.truncate(q.fh, q.size).await?;
            access.drive.notify_local(ino);
            answer(fs::Empty::default())
        },
        "fsync" => { let q: fs::Handle = decode(&bytes)?; access.handle(q.fh)?; core.fsync(q.fh).await?; answer(fs::Empty::default()) },
        _ => Err(Failure::new(StatusCode::NOT_FOUND, "NoSuchOperation", FsError::Unsupported)),
    }
    }).await.map_err(|e| Failure::from(FsError::Io(e.to_string())))?
}

fn query(request: &Request, fields: &[&str]) -> Result<Vec<u64>, Failure> {
    let mut found = HashMap::new();
    for pair in request.uri().query().unwrap_or("").split('&') {
        let (name, value) = pair.split_once('=').ok_or_else(Failure::invalid)?;
        if !fields.contains(&name) || found.insert(name, value.parse::<u64>().map_err(|_| Failure::invalid())?).is_some() { return Err(Failure::invalid()); }
    }
    fields.iter().map(|name| found.get(name).copied().ok_or_else(Failure::invalid)).collect()
}

async fn read(State(s): State<Arc<Shared>>, Path(id): Path<String>, request: Request) -> Result<Response, Failure> {
    let access = s.rpc.access(&id, &request)?;
    let permit = permit(&request);
    let gate = access.gate.clone().read_owned().await;
    if *access.closed.borrow() { return Err(Failure::new(StatusCode::CONFLICT, "StaleSession", FsError::Stale)); }
    let q = query(&request, &["fh", "offset", "length"])?;
    if q[2] > fs::MAX_IO { return Err(Failure::large()); }
    if q[1].checked_add(q[2]).is_none_or(|end| end > i64::MAX as u64) { return Err(Failure::invalid()); }
    access.handle(q[0])?;
    tokio::spawn(async move {
    let (_gate, _permit) = (gate, permit);
    let bytes = access.drive.core.read(q[0], q[1], q[2]).await?;
    Ok(Response::builder().header(header::CONTENT_TYPE, "application/octet-stream").body(Body::from(bytes)).expect("a response"))
    }).await.map_err(|e| Failure::from(FsError::Io(e.to_string())))?
}

async fn write(State(s): State<Arc<Shared>>, Path(id): Path<String>, request: Request) -> Result<Response, Failure> {
    let access = s.rpc.access(&id, &request)?;
    let permit = permit(&request);
    access.writable()?;
    let q = query(&request, &["fh", "offset"])?;
    access.handle(q[0])?;
    if request.headers().get(header::CONTENT_TYPE).is_none_or(|h| h != "application/octet-stream") { return Err(Failure::invalid()); }
    let bytes = body(request, fs::MAX_IO as usize).await?;
    let gate = access.gate.clone().read_owned().await;
    if *access.closed.borrow() { return Err(Failure::new(StatusCode::CONFLICT, "StaleSession", FsError::Stale)); }
    tokio::spawn(async move {
    let (_gate, _permit) = (gate, permit);
    let ino = access.drive.core.handle_attr(q[0])?.ino;
    let written = access.drive.core.write(q[0], q[1], bytes).await?;
    access.drive.notify_local(ino);
    answer(fs::Written { written })
    }).await.map_err(|e| Failure::from(FsError::Io(e.to_string())))?
}

fn line(event: &fs::InvalidationEvent) -> Bytes {
    let mut bytes = serde_json::to_vec(event).expect("JSON");
    bytes.push(b'\n');
    Bytes::from(bytes)
}

async fn watch_events(State(s): State<Arc<Shared>>, Path(id): Path<String>, request: Request) -> Result<Response, Failure> {
    let access = s.rpc.access(&id, &request)?;
    let _gate = access.gate.read().await;
    if *access.closed.borrow() { return Err(Failure::new(StatusCode::CONFLICT, "StaleSession", FsError::Stale)); }
    let permit = s.rpc.watches.clone().try_acquire_owned().map_err(|_| Failure::busy())?;
    let rx = access.drive.subscribe();
    let initial = access.drive.event(true);
    let closed = access.closed.subscribe();
    let closing = s.closing.subscribe();
    let stream = futures::stream::unfold((access.clone(), rx, closed, closing, permit, Some(initial), 0u64),
        |(access, mut rx, mut closed, mut closing, permit, initial, previous)| async move {
            if *closed.borrow() || *closing.borrow() { return None; }
            let event = match initial {
                Some(event) => event,
                None => loop {
                    let event = tokio::select! {
                        _ = closed.wait_for(|c| *c) => return None,
                        _ = closing.wait_for(|c| *c) => return None,
                        event = rx.recv() => event,
                    };
                    let event = match event {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => access.drive.event(true),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    };
                    if event.generation > previous { break event; }
                },
            };
            let generation = event.generation;
            Some((Ok::<_, Infallible>(line(&event)), (access, rx, closed, closing, permit, None, generation)))
        });
    Ok(Response::builder().header(header::CONTENT_TYPE, "application/x-ndjson").body(Body::from_stream(stream)).expect("a response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bootstrap_errors_preserve_native_errno_through_the_control_error_boundary() {
        for error in [FsError::NoSpace, FsError::Stale, FsError::BadHandle, FsError::ReadOnly, FsError::Offline, FsError::Unsupported, FsError::IsDir, FsError::NotDir, FsError::Io(format!("x{}", "é".repeat(fs::MAX_RESPONSE)))] {
            let errno = error.errno();
            let response = Failure::from(crate::server::Failure::from(error)).into_response();
            let bytes = to_bytes(response.into_body(), fs::MAX_RESPONSE).await.unwrap();
            let body: fs::FsErrorBody = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body.error.errno, errno, "the RPC wire error must retain the native filesystem failure");
        }
    }
}
