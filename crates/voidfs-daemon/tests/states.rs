// SPDX-License-Identifier: Apache-2.0
//! A mounted drive's folder states for status badges (`POST /v1/mounts/states`): what each state
//! looks like as the queue moves, and that no answer asks the server anything, through a proxy
//! that counts every request the daemon sends.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use bytes::Bytes;
use futures::future::BoxFuture;
use voidfs_client::mount::Sync;
use voidfs_daemon::api::{Build, Folder, FolderStates, NewMount, Scope};
use voidfs_daemon::{Adapter, ClientError, Core, Daemon, DaemonClient, DaemonConfig, MountSpec, Mounted};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

/// Mounts nothing, and keeps each mount's core for the test to edit through, as FSKit would.
#[derive(Default)]
struct Capture { cores: Mutex<Vec<Core>> }
struct CaptureMount;

impl Adapter for Capture {
    fn name(&self) -> &str { "capture" }
    fn mount<'a>(&'a self, _: &'a MountSpec, core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        Box::pin(async move {
            self.cores.lock().unwrap().push(core.clone());
            Ok(Arc::new(CaptureMount) as Arc<dyn Mounted>)
        })
    }
}

impl Mounted for CaptureMount {
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> { Box::pin(async { Ok(()) }) }
}

/// Forwards to the server and notes each request as `METHOD /path?query`.
struct Proxy { endpoint: String, seen: Arc<Mutex<Vec<String>>>, task: tokio::task::JoinHandle<()> }

impl Drop for Proxy {
    fn drop(&mut self) { self.task.abort(); }
}

struct Upstream { url: String, http: reqwest::Client, seen: Arc<Mutex<Vec<String>>> }

impl Proxy {
    async fn start(upstream: &str) -> Proxy {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Upstream { url: upstream.into(), http: reqwest::Client::new(), seen: seen.clone() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://localhost:{}", listener.local_addr().unwrap().port());
        let app = axum::Router::new().fallback(forward).with_state(state);
        let task = tokio::spawn(async move { let _ = axum::serve(listener, app).await; });
        Proxy { endpoint, seen, task }
    }

    /// What reached the server since the last call, but the change feed's long polls: those
    /// are the mount's, whatever asks for badges.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.seen.lock().unwrap()).into_iter().filter(|line| !line.contains("x-voidfs-changes")).collect()
    }
}

async fn forward(State(up): State<Arc<Upstream>>, request: axum::extract::Request) -> Response {
    let (parts, body) = request.into_parts();
    let target = parts.uri.path_and_query().map_or("/".to_owned(), |p| p.as_str().to_owned());
    up.seen.lock().unwrap().push(format!("{} {target}", parts.method));
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let mut forwarded = up.http.request(parts.method.clone(), format!("{}{target}", up.url));
    // `host` too: the signature covers the proxy's.
    for (name, value) in &parts.headers { forwarded = forwarded.header(name, value); }
    let response = match forwarded.body(body).send().await {
        Ok(response) => response,
        Err(_) => return Response::builder().status(502).body(Body::empty()).unwrap(),
    };
    let mut out = Response::builder().status(response.status().as_u16());
    for (name, value) in response.headers() {
        if !matches!(name.as_str(), "transfer-encoding" | "connection") && (name.as_str() != "content-length" || parts.method == reqwest::Method::HEAD) {
            out = out.header(name, value);
        }
    }
    out.body(Body::from_stream(futures::stream::unfold(response, |mut r| async move {
        match r.chunk().await { Ok(Some(c)) => Some((Ok::<Bytes, std::io::Error>(c), r)), _ => None }
    }))).unwrap()
}

fn sdk(endpoint: &str) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: endpoint.replace("127.0.0.1", "localhost"), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..Default::default() }
}

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(u64::MAX) }

fn config(endpoint: &str, dir: &Path, adapter: Arc<Capture>) -> DaemonConfig {
    let mut cfg = DaemonConfig::new(dir, sdk(endpoint), Build::default());
    cfg.adapters = vec![adapter];
    cfg.cache = voidfs_client::CacheConfig { min_free_bytes: 0, free_space: Some(plenty), read_ahead_bytes: 0, ..Default::default() };
    cfg.connectivity = voidfs_client::ConnectivityConfig { probe_every: Duration::from_secs(3600), probe_max: Duration::from_secs(3600), ..Default::default() };
    cfg
}

fn folder(drive: &str, path: &str) -> Folder { Folder { drive: drive.into(), folder: path.into(), ..Default::default() } }

fn entries(states: &FolderStates) -> Vec<(&str, Sync)> { states.entries.iter().map(|(name, sync)| (name.as_str(), *sync)).collect() }

async fn until(what: &str, mut ok: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ok().await {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[track_caller]
fn refused<T: std::fmt::Debug>(r: Result<T, ClientError>, status: u16, code: &str) {
    match r {
        Err(ClientError::Api { status: s, code: c, .. }) if s == status && c == code => {}
        other => panic!("expected {status} {code}, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn badges_follow_the_queue_from_local_state_without_a_remote_request() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    for key in ["a.txt", "c.txt", "e.txt", "dir/b.txt", "never/x.txt"] { remote.put_object("drive", key, Bytes::from_static(b"base"), Default::default()).await.unwrap(); }
    const FILES: usize = 1000;
    for n in 0..FILES { remote.put_object("drive", &format!("big/{n:04}"), Bytes::from_static(b"x"), Default::default()).await.unwrap(); }
    let proxy = Proxy::start(&server.endpoint).await;
    let dir = tempfile::Builder::new().prefix("vstates").tempdir_in(std::env::temp_dir()).unwrap();
    let adapter = Arc::new(Capture::default());
    let daemon = Daemon::start(config(&proxy.endpoint, dir.path(), adapter.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    client.pause(&Scope::All).await.unwrap();
    refused(client.folder_states(&folder("drive", "")).await, 404, "NotOpen");
    let mountpoint = dir.path().join("mnt").display().to_string();
    client.mount(&NewMount { drive: "drive".into(), mountpoint: Some(mountpoint.clone()), ..Default::default() }).await.unwrap();
    let core = adapter.cores.lock().unwrap()[0].session.clone();
    let root = core.root();
    // Finder lists what it shows: the top, `dir` and the 1,000 files of `big`, but not `never`.
    let mut listed = std::collections::HashMap::new();
    for (name, attr) in core.readdir(root, None, 256).await.unwrap() { listed.insert(name, attr.ino); }
    let mut after = None;
    let mut big = 0;
    loop {
        let page = core.readdir(listed["big"], after.as_deref(), 256).await.unwrap();
        let Some((last, _)) = page.last() else { break };
        after = Some(last.clone());
        big += page.len();
    }
    assert_eq!(big, FILES);
    core.readdir(listed["dir"], None, 256).await.unwrap();
    let listings = proxy.take();
    assert!(listings.iter().filter(|line| line.contains("x-voidfs-list")).count() >= 3, "the proxy carries the mount's listings: {listings:?}");

    // Nothing unpublished yet: every listed name is saved, so no entry is named.
    let top = client.folder_states(&folder("drive", "")).await.unwrap();
    assert!(!top.unchanged && top.state == Sync::Saved && top.entries.is_empty(), "{top:?}");
    let all_big = client.folder_states(&folder("drive", "big")).await.unwrap();
    assert!(all_big.entries.is_empty() && all_big.next.is_none());
    let never = client.folder_states(&folder("drive", "never")).await.unwrap();
    assert!(never.entries.is_empty() && never.state == Sync::Saved, "a folder never listed here isn't listed for a badge: {never:?}");
    refused(client.folder_states(&folder("drive", "missing")).await, 404, "NotFound");
    refused(client.folder_states(&folder("drive", "a.txt")).await, 400, "NotDir");
    refused(client.folder_states(&Folder { limit: Some(0), ..folder("drive", "") }).await, 400, "InvalidArgument");

    // Saving: edits saved on this Mac and queued, with the queue paused. Pending: a new name
    // not yet queued as data.
    let edit = |ino: u64, bytes: &'static [u8]| { let core = core.clone(); async move {
        let fh = core.open(ino, true).await.unwrap();
        core.write(fh, 0, Bytes::from_static(bytes)).await.unwrap();
        core.close(fh).await.unwrap();
    }};
    edit(listed["a.txt"], b"local a").await;
    edit(listed["c.txt"], b"local c").await;
    edit(listed["e.txt"], b"local e").await;
    let created = core.create(listed["dir"], "new.txt", 0o644).await.unwrap();
    for n in 0..300 {
        let ino = core.lookup(listed["big"], &format!("{n:04}")).await.unwrap().ino;
        edit(ino, b"y").await;
    }
    assert_eq!(created.sync, Sync::Pending);
    let top = client.folder_states(&folder("drive", "")).await.unwrap();
    assert_eq!(entries(&top), [("a.txt", Sync::Saving), ("big", Sync::Saving), ("c.txt", Sync::Saving), ("dir", Sync::Pending), ("e.txt", Sync::Saving)]);
    assert_eq!(top.state, Sync::Saving, "saving outranks pending");
    let inner = client.folder_states(&folder("drive", "dir")).await.unwrap();
    assert_eq!(entries(&inner), [("new.txt", Sync::Pending)]);
    // 300 pending names of 1,000, in pages.
    let mut pages = 0;
    let mut named = Vec::new();
    let mut after = None;
    loop {
        let page = client.folder_states(&Folder { after: after.clone(), limit: Some(128), ..folder("drive", "big") }).await.unwrap();
        pages += 1;
        assert!(page.entries.iter().all(|(_, sync)| *sync == Sync::Saving));
        named.extend(page.entries.iter().map(|(name, _)| name.clone()));
        match page.next { Some(next) => after = Some(next), None => break }
    }
    assert_eq!((pages, named.len()), (3, 300));
    assert_eq!(named, (0..300).map(|n| format!("{n:04}")).collect::<Vec<_>>());

    // A version holds until something changes.
    let quiet = client.folder_states(&Folder { since: Some(top.version.clone()), ..folder("drive", "") }).await.unwrap();
    assert!(quiet.unchanged && quiet.entries.is_empty() && quiet.version == top.version, "{quiet:?}");
    let requests = proxy.take();
    assert!(requests.is_empty(), "badges asked the server: {requests:?}");

    // Kept: a cancelled change, on this Mac and not uploaded. Conflict: a change the server's
    // copy moved on from.
    let uploads = client.uploads(false, None).await.unwrap();
    let id = uploads.queue.items.iter().find(|item| item.key == "e.txt").expect("e.txt queued").id;
    client.cancel(&Scope::Entry(id)).await.unwrap();
    remote.put_object("drive", "c.txt", Bytes::from_static(b"theirs"), Default::default()).await.unwrap();
    let changed = client.folder_states(&Folder { since: Some(top.version.clone()), ..folder("drive", "") }).await.unwrap();
    assert!(!changed.unchanged && changed.version != top.version);
    client.resume(&Scope::All).await.unwrap();
    until("the edits publish, but c.txt's, which conflicts", async || {
        let top = client.folder_states(&folder("drive", "")).await.unwrap();
        entries(&top) == [("c.txt", Sync::Conflict), ("e.txt", Sync::Kept)]
    }).await;
    let top = client.folder_states(&folder("drive", "")).await.unwrap();
    assert_eq!(top.state, Sync::Conflict, "a conflict outranks a kept change");
    let inner = client.folder_states(&folder("drive", "dir")).await.unwrap();
    assert!(inner.entries.is_empty() && inner.state == Sync::Saved, "{inner:?}");
    assert!(client.folder_states(&folder("drive", "big")).await.unwrap().entries.is_empty());
    proxy.take();
    for path in ["", "dir", "big", "never"] { client.folder_states(&folder("drive", path)).await.unwrap(); }
    let requests = proxy.take();
    assert!(requests.is_empty(), "badges asked the server: {requests:?}");

    // Without the mount, a drive a daemon session opened answers the same, from the same store.
    client.unmount(&mountpoint).await.unwrap();
    drop(core);
    adapter.cores.lock().unwrap().clear();
    refused(client.folder_states(&folder("drive", "")).await, 404, "NotOpen");
    let session = client.session("drive", true).await.unwrap();
    let top = client.folder_states(&folder("drive", "")).await.unwrap();
    assert_eq!(entries(&top), [("c.txt", Sync::Conflict), ("e.txt", Sync::Kept)]);
    session.release().await.unwrap();
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn badges_come_back_from_the_journal_after_the_daemon_restarts() {
    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server.endpoint)).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    remote.put_object("drive", "folder/file", Bytes::from_static(b"base"), Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vstatesr").tempdir_in(std::env::temp_dir()).unwrap();
    let adapter = Arc::new(Capture::default());
    let daemon = Daemon::start(config(&server.endpoint, dir.path(), adapter.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    client.pause(&Scope::All).await.unwrap();
    let mountpoint = dir.path().join("mnt").display().to_string();
    client.mount(&NewMount { drive: "drive".into(), mountpoint: Some(mountpoint), ..Default::default() }).await.unwrap();
    let core = adapter.cores.lock().unwrap()[0].session.clone();
    let folder_ino = core.lookup(core.root(), "folder").await.unwrap().ino;
    let file = core.lookup(folder_ino, "file").await.unwrap().ino;
    let fh = core.open(file, true).await.unwrap();
    core.write(fh, 0, Bytes::from_static(b"edited")).await.unwrap();
    core.close(fh).await.unwrap();
    let before = client.folder_states(&folder("drive", "folder")).await.unwrap();
    assert_eq!(entries(&before), [("file", Sync::Saving)]);
    drop(core);
    adapter.cores.lock().unwrap().clear();
    daemon.stop().await;

    let daemon = Daemon::start(config(&server.endpoint, dir.path(), adapter.clone())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    until("the remembered mount comes back", async || client.mounts().await.unwrap().mounts.first().is_some_and(|m| m.state == "mounted")).await;
    let after = client.folder_states(&folder("drive", "folder")).await.unwrap();
    assert_eq!(entries(&after), [("file", Sync::Saving)], "the unpublished change is still this Mac's");
    assert_ne!(after.version, before.version, "another daemon's versions never match");
    let top = client.folder_states(&folder("drive", "")).await.unwrap();
    assert_eq!((entries(&top), top.state), (vec![("folder", Sync::Saving)], Sync::Saving));
    client.resume(&Scope::All).await.unwrap();
    until("the change publishes", async || client.folder_states(&folder("drive", "")).await.unwrap().entries.is_empty()).await;
    adapter.cores.lock().unwrap().clear();
    daemon.stop().await;
}
