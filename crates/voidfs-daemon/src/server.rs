// SPDX-License-Identifier: Apache-2.0
//! The daemon: the client core, and the socket it answers on.

use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use tokio::sync::watch;
use voidfs_client::{ApiFetcher, Cache, CacheConfig, Connectivity, ConnectivityConfig, Queue, QueueConfig, Store};

use crate::mounts::{Adapter, Core, Mounts};

use crate::api::{self, ApiError, Build, ErrorBody};

/// Why a daemon didn't start.
#[derive(Debug)]
pub enum Error {
    /// Another daemon has the state directory.
    Locked(PathBuf),
    /// The socket couldn't be made.
    Socket(String),
    Client(voidfs_client::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Locked(d) => write!(f, "another daemon is running on {}", d.display()),
            Error::Socket(m) => f.write_str(m),
            Error::Client(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<voidfs_client::Error> for Error {
    fn from(e: voidfs_client::Error) -> Error {
        Error::Client(e)
    }
}

#[derive(Clone)]
pub struct DaemonConfig {
    pub state_dir: PathBuf,
    /// `None` is `<state>/daemon.sock`.
    pub socket: Option<PathBuf>,
    /// The server and key it publishes with.
    pub client: voidfs_sdk::Config,
    pub build: Build,
    pub queue: QueueConfig,
    pub cache: CacheConfig,
    pub connectivity: ConnectivityConfig,
    /// What mounts drives; the first is the default. None yet in `void` (step 5).
    pub adapters: Vec<Arc<dyn Adapter>>,
    /// Where a drive mounts when no mountpoint is named, as `<root>/<drive>`: `~/voidfs`.
    pub mount_root: Option<PathBuf>,
}

impl DaemonConfig {
    pub fn new(state_dir: &Path, client: voidfs_sdk::Config, build: Build) -> DaemonConfig {
        DaemonConfig {
            state_dir: state_dir.to_owned(),
            socket: None,
            client,
            build,
            queue: QueueConfig::default(),
            cache: CacheConfig::default(),
            connectivity: ConnectivityConfig::default(),
            adapters: Vec::new(),
            mount_root: std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join("voidfs")),
        }
    }
}

/// What the handlers share.
pub(crate) struct Shared {
    build: Build,
    pid: u32,
    started: Instant,
    started_at: String,
    socket: PathBuf,
    state_dir: PathBuf,
    endpoint: String,
    access_key_id: String,
    pub(crate) client: voidfs_sdk::Client,
    pub(crate) queue: Queue,
    cache: Cache,
    conn: Connectivity,
    pub(crate) store: Arc<Store>,
    pub(crate) mounts: Mounts,
    /// Set when a client asks the daemon to stop.
    stop_asked: watch::Sender<bool>,
    /// Set when the daemon starts stopping: streams end.
    pub(crate) closing: watch::Sender<bool>,
    /// The queue's bytes sent, sampled each second, for a rate.
    samples: Mutex<VecDeque<(Instant, u64)>>,
}

/// A running daemon. Stop it with [`Daemon::stop`]: dropped, its tasks go on.
pub struct Daemon {
    shared: Arc<Shared>,
    store: Arc<Store>,
    server: tokio::task::JoinHandle<()>,
    probe: tokio::task::JoinHandle<()>,
    sampler: tokio::task::JoinHandle<()>,
    /// Tells the server to stop answering.
    shutdown: watch::Sender<bool>,
}

/// How long `start` waits for a daemon that is stopping to let go of the state directory.
const LOCK_WAIT: Duration = Duration::from_secs(2);
/// How long `stop` waits for requests in progress to finish.
const DRAIN_WAIT: Duration = Duration::from_secs(5);
/// The rate is over this many of the last samples, a second apart.
const RATE_SAMPLES: usize = 5;

async fn open_store(dir: &Path) -> Result<Arc<Store>, Error> {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        let d = dir.to_owned();
        match tokio::task::spawn_blocking(move || Store::open(&d)).await.map_err(voidfs_client::Error::from)? {
            Ok(s) => return Ok(Arc::new(s)),
            Err(voidfs_client::Error::Locked(_)) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(25)).await,
            Err(voidfs_client::Error::Locked(_)) => return Err(Error::Locked(dir.to_owned())),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Makes the socket at `path`, readable and writable by this user only, in place of one a daemon
/// that didn't stop left behind: the caller holds the state directory, so no daemon answers on it.
fn bind(path: &Path) -> Result<tokio::net::UnixListener, Error> {
    let len = path.as_os_str().len();
    if len > crate::MAX_SOCKET_PATH {
        return Err(Error::Socket(format!(
            "the socket path {} is {len} bytes, more than the {} a Unix socket may have: set VOIDFS_STATE_DIR to a shorter directory",
            path.display(),
            crate::MAX_SOCKET_PATH
        )));
    }
    let failed = |e: std::io::Error| Error::Socket(format!("{}: {e}", path.display()));
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(failed(e)),
    }
    let listener = tokio::net::UnixListener::bind(path).map_err(failed)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(failed)?;
    Ok(listener)
}

impl Daemon {
    /// Takes the state directory, opens the client core in it, and answers on the socket. What the
    /// journal holds starts publishing at once.
    pub async fn start(cfg: DaemonConfig) -> Result<Daemon, Error> {
        let dir = cfg.state_dir.clone();
        let io = |e: std::io::Error| Error::Client(e.into());
        std::fs::create_dir_all(&dir).map_err(io)?;
        // The journal holds copies of the user's bytes, and daemon.json a secret.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
        let store = open_store(&dir).await?;
        let socket = cfg.socket.clone().unwrap_or_else(|| crate::socket_path(&dir));
        let listener = bind(&socket)?;
        let opened = Daemon::open_core(&cfg, &store).await;
        let (queue, cache, conn, client) = match opened {
            Ok(o) => o,
            Err(e) => {
                let _ = std::fs::remove_file(&socket);
                return Err(e);
            }
        };
        let probe = conn.probe(client.clone());
        let shared = Arc::new(Shared {
            build: cfg.build.clone(),
            pid: std::process::id(),
            started: Instant::now(),
            started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            socket: socket.clone(),
            state_dir: dir,
            endpoint: cfg.client.endpoint.clone(),
            access_key_id: cfg.client.access_key_id.clone(),
            client,
            queue,
            cache,
            conn,
            store: store.clone(),
            mounts: Mounts::new(cfg.adapters.clone(), cfg.mount_root.clone()),
            stop_asked: watch::Sender::new(false),
            closing: watch::Sender::new(false),
            samples: Mutex::new(VecDeque::new()),
        });
        let sampler = tokio::spawn({
            let shared = shared.clone();
            async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tick.tick().await;
                    let mut s = shared.samples.lock().unwrap_or_else(|p| p.into_inner());
                    s.push_back((Instant::now(), shared.queue.bytes_sent()));
                    while s.len() > RATE_SAMPLES {
                        s.pop_front();
                    }
                }
            }
        });
        let app = Router::new()
            .route("/v1/status", get(status))
            .route("/v1/info", get(info))
            .route("/v1/stop", post(stop))
            .route("/v1/uploads", get(crate::uploads::list).post(crate::uploads::enqueue))
            .route("/v1/uploads/watch", get(crate::uploads::watch))
            .route("/v1/uploads/pause", post(crate::uploads::pause))
            .route("/v1/uploads/resume", post(crate::uploads::resume))
            .route("/v1/uploads/cancel", post(crate::uploads::cancel))
            .route("/v1/uploads/limit", put(crate::uploads::limit))
            .route("/v1/uploads/clear", post(crate::uploads::clear))
            .route("/v1/mounts", get(crate::mounts::list).post(crate::mounts::mount))
            .route("/v1/mounts/unmount", post(crate::mounts::unmount))
            .fallback(not_found)
            .with_state(shared.clone());
        let (shutdown, mut rx) = watch::channel(false);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.wait_for(|s| *s).await;
                })
                .await;
        });
        let s2 = store.clone();
        let remembered = tokio::task::spawn_blocking(move || crate::mounts::stored(&s2)).await.map_err(voidfs_client::Error::from)?;
        Mounts::restore(&shared, remembered?);
        Ok(Daemon { shared, store, server, probe, sampler, shutdown })
    }

    async fn open_core(cfg: &DaemonConfig, store: &Arc<Store>) -> Result<(Queue, Cache, Connectivity, voidfs_sdk::Client), Error> {
        let conn = Connectivity::new(cfg.connectivity.clone());
        let client = voidfs_sdk::Client::new(voidfs_sdk::Config { observer: Some(Arc::new(conn.clone())), upload_bandwidth: None, ..cfg.client.clone() }).map_err(voidfs_client::Error::from)?;
        let fetcher = Arc::new(ApiFetcher::new(client.clone()).with_connectivity(conn.clone()));
        let cache = Cache::open(store.clone(), fetcher, cfg.cache.clone()).await?;
        let queue = Queue::open(store.clone(), client.clone(), QueueConfig { connectivity: Some(conn.clone()), ..cfg.queue.clone() }).await?;
        Ok((queue, cache, conn, client))
    }

    pub fn socket(&self) -> &Path {
        &self.shared.socket
    }

    pub fn queue(&self) -> &Queue {
        &self.shared.queue
    }

    /// Returns once a client has asked the daemon to stop (`POST /v1/stop`).
    pub async fn stop_asked(&self) {
        let mut rx = self.shared.stop_asked.subscribe();
        let _ = rx.wait_for(|s| *s).await;
    }

    /// Stops: the socket stops answering, the queue stops publishing (what is uploading stops at
    /// its next request), the socket goes, and then the state directory is let go, so that
    /// another daemon may take it.
    pub async fn stop(self) {
        let Daemon { shared, store, mut server, probe, sampler, shutdown } = self;
        shared.closing.send_replace(true);
        sampler.abort();
        let _ = sampler.await;
        let _ = shutdown.send(true);
        if tokio::time::timeout(DRAIN_WAIT, &mut server).await.is_err() {
            server.abort();
            let _ = server.await;
        }
        shared.mounts.detach_all().await;
        shared.queue.close().await;
        shared.cache.settle().await;
        probe.abort();
        let _ = probe.await;
        let socket = shared.socket.clone();
        drop(shared);
        // The socket first: once the state directory is free, a new daemon may have made its own.
        let _ = std::fs::remove_file(&socket);
        drop(store);
    }
}

pub(crate) struct Failure(pub StatusCode, pub String, pub String);

impl Failure {
    pub(crate) fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Failure {
        Failure(status, code.into(), message.into())
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorBody { error: ApiError { code: self.1, message: self.2 } })).into_response()
    }
}

impl From<voidfs_client::Error> for Failure {
    fn from(e: voidfs_client::Error) -> Failure {
        Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "InternalError", e.to_string())
    }
}

/// What the server said, passed on; `502` when it couldn't be reached.
impl From<voidfs_sdk::Error> for Failure {
    fn from(e: voidfs_sdk::Error) -> Failure {
        match (e.status(), e.code()) {
            (Some(status), Some(code)) => Failure::new(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), code, e.to_string()),
            _ => Failure::new(StatusCode::BAD_GATEWAY, "RequestFailed", e.to_string()),
        }
    }
}

pub(crate) type Answer<T> = Result<Json<T>, Failure>;

async fn not_found(method: Method, uri: Uri) -> Failure {
    Failure::new(StatusCode::NOT_FOUND, "NotFound", format!("no such request: {method} {}", uri.path()))
}

impl Shared {
    /// The client core, for an adapter.
    pub(crate) fn core(&self) -> Core {
        Core { client: self.client.clone(), cache: self.cache.clone(), queue: self.queue.clone(), connectivity: self.conn.clone() }
    }

    pub(crate) fn connectivity(&self) -> &Connectivity {
        &self.conn
    }

    /// Bytes a second the queue sent, from the oldest sample to now, over a second at least.
    pub(crate) fn rate(&self) -> u64 {
        let now = (Instant::now(), self.queue.bytes_sent());
        let s = self.samples.lock().unwrap_or_else(|p| p.into_inner());
        let Some(first) = s.front() else { return 0 };
        let secs = now.0.duration_since(first.0).as_secs_f64().max(1.0);
        (now.1.saturating_sub(first.1) as f64 / secs) as u64
    }

    fn daemon(&self) -> api::Daemon {
        api::Daemon {
            running: true,
            pid: self.pid,
            build: self.build.clone(),
            socket: self.socket.display().to_string(),
            state_dir: self.state_dir.display().to_string(),
            started: self.started_at.clone(),
            uptime_secs: self.started.elapsed().as_secs(),
            endpoint: self.endpoint.clone(),
            access_key_id: self.access_key_id.clone(),
        }
    }
}

async fn status(State(s): State<Arc<Shared>>) -> Answer<api::Status> {
    let q = s.queue.status().await?;
    let usage = s.cache.usage();
    let cfg = s.cache.config();
    Ok(Json(api::Status {
        daemon: s.daemon(),
        connection: api::Connection { endpoint: s.endpoint.clone(), link: s.conn.link().into() },
        uploads: api::Uploads { rate: s.rate(), ..api::Uploads::of(&q) },
        cache: api::Cache { disk_bytes: usage.disk_bytes, max_bytes: cfg.max_bytes, pinned_bytes: usage.pinned_bytes, memory_bytes: usage.memory_bytes, memory_max_bytes: cfg.memory_bytes },
        mounts: s.mounts.table(),
    }))
}

async fn info(State(s): State<Arc<Shared>>) -> Answer<api::Info> {
    let q = s.queue.status().await?;
    // Every change the journal took is on disk; nothing here holds one only in memory yet.
    let memory_only_bytes = 0;
    Ok(Json(api::Info {
        build: s.build.clone(),
        pid: s.pid,
        uptime_secs: s.started.elapsed().as_secs(),
        socket: s.socket.display().to_string(),
        state_dir: s.state_dir.display().to_string(),
        restart_safe: memory_only_bytes == 0,
        mounts: s.mounts.table().len() as u64,
        memory_only_bytes,
        journal: api::Journal::of(&q),
    }))
}

async fn stop(State(s): State<Arc<Shared>>) -> (StatusCode, Json<api::Stopping>) {
    s.stop_asked.send_replace(true);
    (StatusCode::ACCEPTED, Json(api::Stopping { stopping: true, pid: s.pid }))
}
