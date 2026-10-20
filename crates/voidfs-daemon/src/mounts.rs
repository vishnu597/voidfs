// SPDX-License-Identifier: Apache-2.0
//! The mount table: what is mounted where, by which adapter, with its drive's change feed; and the
//! remembered mounts, which the daemon mounts again when it starts. Mounting is an [`Adapter`]'s
//! (step 5: FSKit, SMB or FUSE); the `void` binary has [`crate::FsKit`] on macOS.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{SecondsFormat, Utc};
use futures::future::BoxFuture;
use futures::StreamExt;
use tokio::sync::watch;
use voidfs_client::{Cache, Connectivity, Invalidation, Queue, Remembered, Store};

use crate::api;
use crate::server::{Answer, Failure, Shared};
use crate::sessions::DriveSession;

/// A drive to mount, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    /// Its alias.
    pub drive: String,
    pub mountpoint: PathBuf,
    pub read_only: bool,
}

/// What an adapter serves a drive from: the daemon's client core.
#[derive(Clone)]
pub struct Core {
    pub client: voidfs_sdk::Client,
    pub cache: Cache,
    pub queue: Queue,
    pub connectivity: Connectivity,
    /// Shared writable core. Adapters enforce each mount's read-only policy at their boundary.
    pub session: Arc<voidfs_client::mount::Session>,
}

/// Mounts drives: FSKit, SMB or FUSE (step 5).
pub trait Adapter: Send + Sync + 'static {
    /// Its name, as `void mount --adapter` takes it.
    fn name(&self) -> &str;
    /// Whether it can mount a drive writable; one that can't mounts every drive read-only.
    fn writable(&self) -> bool { true }
    /// Mounts `spec`, serving it from `core`. The mountpoint exists.
    fn mount<'a>(&'a self, spec: &'a MountSpec, core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>>;
}

/// A drive an adapter has mounted.
pub trait Mounted: Send + Sync + 'static {
    /// What the drive's change feed made stale, for the adapter to tell the kernel.
    fn invalidate(&self, _changes: &[Invalidation]) {}
    /// Local edits also name inode attributes, including open files without a linked path.
    fn local_change(&self, change: &voidfs_client::mount::LocalChange) { self.invalidate(&change.invalidations); }
    /// Whether this exact volume remains mounted. Only authoritative absence/replacement is
    /// false; failed I/O, an unavailable bridge, and probe errors do not establish an eject.
    /// Adapters without an OS presence check retain their mount intent.
    fn is_present(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async { Ok(true) })
    }
    /// Unmounts it.
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>>;
    /// Explicit force eject. Adapters must opt in; never fall back to an ordinary unmount.
    fn force_unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Err("this adapter does not support force eject".into()) })
    }
}

enum Is {
    Mounting,
    Mounted(Arc<dyn Mounted>),
    Failed(String),
}

struct Live {
    spec: MountSpec,
    instance: String,
    adapter: String,
    since: String,
    is: Is,
    drive: Option<Arc<DriveSession>>,
}

impl Live {
    fn api(&self) -> api::Mount {
        let (state, error) = match &self.is {
            Is::Mounting => ("mounting", None),
            Is::Mounted(_) => ("mounted", None),
            Is::Failed(e) => ("failed", Some(e.clone())),
        };
        let feed = self.drive.as_ref().filter(|_| matches!(self.is, Is::Mounted(_))).map(|d| d.feed());
        api::Mount {
            drive: self.spec.drive.clone(),
            mountpoint: self.spec.mountpoint.display().to_string(),
            instance: self.instance.clone(),
            adapter: self.adapter.clone(),
            read_only: self.spec.read_only,
            state: state.into(),
            error,
            since: self.since.clone(),
            feed,
        }
    }
}

/// The daemon's mounts.
pub(crate) struct Mounts {
    adapters: Vec<Arc<dyn Adapter>>,
    /// Where a drive mounts when no mountpoint is named: `<root>/<drive>`.
    root: Option<PathBuf>,
    live: Mutex<Vec<Live>>,
    /// Unique attachment suffix within this daemon; paired with `Shared::nonce` at creation.
    next_instance: AtomicU64,
    /// Ejected volumes whose remembered row still needs deleting. Separate from `live` so an
    /// OS eject never remains reported as mounted when SQLite is temporarily unavailable.
    forgetting: Mutex<HashSet<PathBuf>>,
    /// One mount or unmount at a time.
    ops: tokio::sync::Mutex<()>,
}

const RECONCILE_EVERY: Duration = Duration::from_secs(1);
const PRESENCE_WAIT: Duration = Duration::from_secs(1);

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A mountpoint as the table keys it: absolute, without a trailing `/`.
fn key(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() { "/".into() } else { t.into() }
}

impl Mounts {
    pub(crate) fn new(adapters: Vec<Arc<dyn Adapter>>, root: Option<PathBuf>) -> Mounts {
        Mounts { adapters, root, live: Mutex::new(Vec::new()), next_instance: AtomicU64::new(1), forgetting: Mutex::new(HashSet::new()), ops: tokio::sync::Mutex::new(()) }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, Vec<Live>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn table(&self) -> Vec<api::Mount> {
        self.live().iter().map(Live::api).collect()
    }

    fn instance(s: &Shared) -> String {
        format!("{:x}.{}", s.nonce, s.mounts.next_instance.fetch_add(1, Ordering::Relaxed))
    }

    fn forgetting(&self) -> std::sync::MutexGuard<'_, HashSet<PathBuf>> {
        self.forgetting.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Retire a volume the OS has already ejected; never call the adapter or touch its uploads.
    /// The caller holds `ops`. Keep failed persistence available for a later retry, including
    /// after the last live volume has gone. A failed delete remains durable remembered intent
    /// until it can succeed; no in-memory retry can promise otherwise across a storage failure.
    async fn forget(s: &Shared, at: &Path) -> Result<(), Failure> {
        let removed = {
            let mut live = s.mounts.live();
            live.iter().position(|l| l.spec.mountpoint == at).map(|i| live.remove(i))
        };
        drop(removed);
        s.mounts.forgetting().insert(at.to_owned());
        Self::forget_pending(s, at).await
    }

    async fn forget_pending(s: &Shared, at: &Path) -> Result<(), Failure> {
        let store = s.store.clone();
        let point = at.display().to_string();
        blocking(move || voidfs_client::mounts::forget(&store, &point)).await?;
        s.mounts.forgetting().remove(at);
        Ok(())
    }

    /// Observe OS mount presence independently of the status/menu being open. Read-only probes
    /// run outside `ops`, so a slow or failed adapter cannot block mount/eject controls. Before
    /// forgetting, revalidate the handle under `ops`: an old observation cannot erase a remount.
    pub(crate) fn watch(s: Arc<Shared>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut closing = s.closing.subscribe();
            let mut tick = tokio::time::interval(RECONCILE_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = closing.wait_for(|v| *v) => break,
                    _ = tick.tick() => {}
                }
                Self::reconcile(&s, &mut closing).await;
            }
        })
    }

    async fn reconcile(s: &Shared, closing: &mut watch::Receiver<bool>) {
        let pending: Vec<_> = s.mounts.forgetting().iter().cloned().collect();
        for point in pending {
            let _op = tokio::select! {
                biased;
                _ = closing.wait_for(|v| *v) => return,
                op = s.mounts.ops.lock() => op
            };
            if *closing.borrow() { return; }
            // An explicit remount owns this path now. Its successful remember supersedes the
            // old pending deletion; even a failed remember must not delete a live mount's intent.
            if s.mounts.live().iter().any(|l| l.spec.mountpoint == point) { continue; }
            let _ = Self::forget_pending(s, &point).await;
        }
        let mounted: Vec<_> = s.mounts.live().iter().filter_map(|l| match &l.is {
            Is::Mounted(handle) => Some((l.spec.mountpoint.clone(), handle.clone())),
            _ => None,
        }).collect();
        let pending: Vec<_> = mounted.into_iter().map(Self::probe).collect();
        let mut probes = futures::stream::iter(pending).buffer_unordered(8);
        loop {
            let next = tokio::select! {
                biased;
                _ = closing.wait_for(|v| *v) => return,
                next = probes.next() => next
            };
            let Some((point, handle, gone)) = next else { break };
            if !gone { continue; }
            let _op = tokio::select! {
                biased;
                _ = closing.wait_for(|v| *v) => return,
                op = s.mounts.ops.lock() => op
            };
            if *closing.borrow() { return; }
            let current = s.mounts.live().iter().any(|l| l.spec.mountpoint == point
                && matches!(&l.is, Is::Mounted(h) if Arc::ptr_eq(h, &handle)));
            if current {
                // Once persistence begins, drain it even when shutdown is requested. The watcher
                // is joined before orderly detach; cancellation cannot strand a DB job after stop.
                let _ = Self::forget(s, &point).await;
            }
        }
    }

    async fn probe((point, handle): (PathBuf, Arc<dyn Mounted>)) -> (PathBuf, Arc<dyn Mounted>, bool) {
        let gone = matches!(tokio::time::timeout(PRESENCE_WAIT, handle.is_present()).await, Ok(Ok(false)));
        (point, handle, gone)
    }

    fn adapter(&self, name: Option<&str>) -> Result<Arc<dyn Adapter>, Failure> {
        let none = || {
            let have: Vec<&str> = self.adapters.iter().map(|a| a.name()).collect();
            let what = if have.is_empty() { "this build has no adapter: mounting comes with step 5".to_owned() } else { format!("the adapters here are {}", have.join(", ")) };
            Failure::new(StatusCode::NOT_IMPLEMENTED, "NoAdapter", what)
        };
        match name {
            None => self.adapters.first().cloned().ok_or_else(none),
            Some(n) => self.adapters.iter().find(|a| a.name() == n).cloned().ok_or_else(none),
        }
    }

    /// Mounts `spec` with `adapter` and starts watching its drive; the entry for its mountpoint
    /// is updated as it goes.
    async fn attach(s: &Shared, spec: &MountSpec, adapter: &Arc<dyn Adapter>, drive: Option<Arc<DriveSession>>) -> Result<(), Failure> {
        let drive = match drive { Some(drive) => drive, None => s.sessions.drive(s, &spec.drive).await? };
        let effective = MountSpec { drive: drive.drive(), read_only: spec.read_only || !drive.pinned || !adapter.writable(), ..spec.clone() };
        let handle = adapter.mount(&effective, &s.core(drive.core.clone())).await.map_err(|m| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "MountFailed", m))?;
        drive.observe(handle.clone());
        let unwanted = {
            let mut live = s.mounts.live();
            match live.iter_mut().find(|l| l.spec.mountpoint == spec.mountpoint) {
                Some(l) => {
                    l.spec.read_only = effective.read_only;
                    l.spec.drive = effective.drive.clone();
                    l.is = Is::Mounted(handle);
                    l.drive = Some(drive);
                    l.since = now();
                    None
                }
                None => Some(handle),
            }
        };
        // Unmounted meanwhile: let go of it.
        if let Some(handle) = unwanted {
            let _ = handle.unmount().await;
        }
        Ok(())
    }

    /// Mounts what is remembered, in the background, as the daemon starts.
    pub(crate) fn restore(s: &Arc<Shared>, remembered: Vec<Remembered>) {
        for r in remembered {
            let spec = MountSpec { drive: r.drive.clone(), mountpoint: PathBuf::from(&r.mountpoint), read_only: r.read_only };
            let adapter = s.mounts.adapter(Some(&r.adapter));
            let is = match &adapter {
                Ok(_) => Is::Mounting,
                Err(f) => Is::Failed(f.2.clone()),
            };
            s.mounts.live().push(Live { spec: spec.clone(), instance: Self::instance(s), adapter: r.adapter.clone(), since: now(), is, drive: None });
            let Ok(adapter) = adapter else { continue };
            let s = s.clone();
            tokio::spawn(async move {
                let _op = s.mounts.ops.lock().await;
                let result = async {
                    let drive = s.sessions.remembered(&s, &r).await?;
                    let remembered = Remembered { drive: drive.drive(), drive_id: drive.pinned.then(|| drive.id.clone()), ..r };
                    let store = s.store.clone();
                    blocking(move || voidfs_client::mounts::remember(&store, &remembered)).await?;
                    Mounts::attach(&s, &spec, &adapter, Some(drive)).await
                }.await;
                if let Err(f) = result
                    && let Some(l) = s.mounts.live().iter_mut().find(|l| l.spec.mountpoint == spec.mountpoint)
                {
                    l.is = Is::Failed(f.2);
                }
            });
        }
    }

    /// Unmounts everything, as the daemon stops; what is remembered stays so.
    pub(crate) async fn detach_all(&self) {
        let _op = self.ops.lock().await;
        let live: Vec<Live> = std::mem::take(&mut *self.live());
        for l in live {
            if let Is::Mounted(h) = l.is {
                let _ = h.unmount().await;
            }
        }
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> voidfs_client::Result<T> + Send + 'static) -> Result<T, Failure> {
    Ok(tokio::task::spawn_blocking(f).await.map_err(voidfs_client::Error::from)??)
}

impl Shared {
    async fn mounts_view(&self) -> Result<api::Mounts, Failure> {
        let store = self.store.clone();
        let remembered = blocking(move || voidfs_client::mounts::remembered(&store)).await?;
        Ok(api::Mounts { mounts: self.mounts.table(), remembered })
    }
}

pub(crate) async fn list(State(s): State<Arc<Shared>>) -> Answer<api::Mounts> {
    Ok(Json(s.mounts_view().await?))
}

pub(crate) async fn mount(State(s): State<Arc<Shared>>, Json(m): Json<api::NewMount>) -> Answer<api::Mount> {
    let bad = |m: String| Failure::new(StatusCode::BAD_REQUEST, "InvalidArgument", m);
    let _op = s.mounts.ops.lock().await;
    let adapter = s.mounts.adapter(m.adapter.as_deref())?;
    let session = s.sessions.drive(&s, &m.drive).await?;
    let drive = session.drive();
    let drive_id = session.pinned.then(|| session.id.clone());
    let mountpoint = match &m.mountpoint {
        Some(p) => PathBuf::from(key(p)),
        None => s.mounts.root.as_ref().map(|r| r.join(&drive)).ok_or_else(|| bad("no HOME for the default mountpoint: name one".into()))?,
    };
    if !mountpoint.is_absolute() {
        return Err(bad(format!("{}: not an absolute path", mountpoint.display())));
    }
    if s.mounts.live().iter().any(|l| l.spec.mountpoint == mountpoint && !matches!(l.is, Is::Failed(_))) {
        return Err(Failure::new(StatusCode::CONFLICT, "AlreadyMounted", format!("something is mounted at {}", mountpoint.display())));
    }
    std::fs::create_dir_all(&mountpoint).map_err(|e| Failure::new(StatusCode::BAD_REQUEST, "LocalFileError", format!("{}: {e}", mountpoint.display())))?;
    let spec = MountSpec { drive: drive.clone(), mountpoint: mountpoint.clone(), read_only: m.read_only };
    {
        let mut live = s.mounts.live();
        live.retain(|l| l.spec.mountpoint != mountpoint);
        live.push(Live { spec: spec.clone(), instance: Mounts::instance(&s), adapter: adapter.name().into(), since: now(), is: Is::Mounting, drive: None });
    }
    if let Err(f) = Mounts::attach(&s, &spec, &adapter, Some(session)).await {
        s.mounts.live().retain(|l| l.spec.mountpoint != mountpoint);
        return Err(f);
    }
    let r = Remembered { mountpoint: mountpoint.display().to_string(), drive, drive_id, adapter: adapter.name().into(), read_only: m.read_only };
    let store = s.store.clone();
    blocking(move || voidfs_client::mounts::remember(&store, &r)).await?;
    s.mounts.forgetting().remove(&mountpoint);
    let view = s.mounts.live().iter().find(|l| l.spec.mountpoint == mountpoint).map(Live::api);
    view.map(Json).ok_or_else(|| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "InternalError", "the mount went away"))
}

/// Unmounts and forgets what `target` names: a mountpoint, or every mount of a drive.
pub(crate) async fn unmount(State(s): State<Arc<Shared>>, Json(u): Json<api::Unmount>) -> Answer<api::Unmounted> {
    let _op = s.mounts.ops.lock().await;
    let target = if u.target.starts_with('/') { key(&u.target) } else { u.target.clone() };
    // Conditional requests can name only a current attachment's path, alias or pinned id.
    // Do not resolve unknown names (which can update the alias memo) before checking identity.
    // Legacy drive targets also use the known alias for remembered-only cleanup.
    let alias = if target.starts_with('/') || u.expected_instance.is_some() { None } else { s.sessions.mount_alias(&s, &target).await.ok() };
    let names = |drive: &str, mountpoint: &str| mountpoint == target || drive == target || alias.as_deref() == Some(drive);
    let targets: Vec<_> = s.mounts.live().iter()
        .filter(|l| names(&l.spec.drive, &l.spec.mountpoint.display().to_string()) || l.drive.as_ref().is_some_and(|d| d.id == target))
        .map(|l| (l.spec.mountpoint.clone(), l.instance.clone(), match &l.is { Is::Mounted(handle) => Some(handle.clone()), _ => None }, matches!(l.is, Is::Failed(_))))
        .collect();
    // Hold `ops` from the identity check through OS eject and forgetting. A confirmation from
    // an earlier attachment must never eject its replacement, even at the same path and drive.
    // An unchanged failed attachment supports normal bookkeeping cleanup only; it has no OS
    // handle to force-eject. A mounting attachment is not yet safe to act on conditionally.
    if let Some(expected) = &u.expected_instance
        && !matches!(targets.as_slice(), [(_, instance, handle, failed)] if instance == expected && (handle.is_some() || (!u.force && *failed)))
    {
        return Err(Failure::new(StatusCode::CONFLICT, "MountChanged", format!("{} is no longer the mounted volume you selected. Close this dialog and try again.", u.target)));
    }
    let remembered = if u.expected_instance.is_some() {
        // A conditional request concerns only the checked live attachment. Do not fall back to
        // other remembered rows named by a drive alias after ejecting that one attachment.
        Vec::new()
    } else {
        let store = s.store.clone();
        blocking(move || voidfs_client::mounts::remembered(&store)).await?
    };
    let mut unmounted: Vec<String> = Vec::new();
    for (mountpoint, _, handle, _) in targets {
        if let Some(handle) = handle {
            let result = if u.force { handle.force_unmount().await } else { handle.unmount().await };
            if let Err(error) = result {
                // A refusal leaves this mount and every not-yet-tried mount live and remembered.
                return Err(Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "UnmountFailed", format!("{}: {error}", mountpoint.display())));
            }
        }
        // The OS has ejected it even if forgetting it fails: never report a gone volume as
        // mounted. Persist each success before trying the next, so a later busy mount cannot
        // resurrect an earlier successful eject when the daemon starts again.
        let mp = mountpoint.display().to_string();
        Mounts::forget(&s, &mountpoint).await?;
        unmounted.push(mp);
    }
    let forgotten = remembered.into_iter().filter(|r| !unmounted.contains(&r.mountpoint)
        && (names(&r.drive, &r.mountpoint) || r.drive_id.as_deref() == Some(&target))).collect::<Vec<_>>();
    for r in forgotten {
        Mounts::forget(&s, Path::new(&r.mountpoint)).await?;
        if !unmounted.contains(&r.mountpoint) {
            unmounted.push(r.mountpoint);
        }
    }
    if unmounted.is_empty() {
        return Err(Failure::new(StatusCode::NOT_FOUND, "NoSuchMount", format!("nothing is mounted or remembered at or for {}", u.target)));
    }
    Ok(Json(api::Unmounted { unmounted }))
}

/// Names in one folder-states answer at most, with their quoting: half the socket's response bound.
const STATES_BYTES: usize = crate::api::fs::MAX_RESPONSE / 2;

/// A folder's sync states for status badges, from a drive a mount or a session has open. It
/// never opens one: that would ask the server, and a badge may cost nothing remote.
pub(crate) async fn states(State(s): State<Arc<Shared>>, Json(f): Json<api::Folder>) -> Answer<api::FolderStates> {
    let bad = |m: &str| Failure::new(StatusCode::BAD_REQUEST, "InvalidArgument", m);
    if f.drive.is_empty() || f.drive.len() > 1024 { return Err(bad("invalid drive name")); }
    let limit = f.limit.unwrap_or(voidfs_client::mount::MAX_STATES);
    if limit == 0 || limit > voidfs_client::mount::MAX_STATES { return Err(bad("the page limit is out of bounds")); }
    if f.since.as_ref().is_some_and(|v| v.len() > 256) { return Err(bad("invalid version")); }
    let drive = s.sessions.open_drive(&f.drive)
        .ok_or_else(|| Failure::new(StatusCode::NOT_FOUND, "NotOpen", format!("{} isn't mounted or open here", f.drive)))?;
    // Read before the states: a change that lands meanwhile then shows in the next version.
    let version = format!("{:x}.{}.{}", s.nonce, drive.serial, drive.metadata_generation());
    let unless = f.since.as_deref().filter(|_| f.after.is_none())
        .and_then(|since| since.rsplit_once('.')).filter(|(prefix, _)| *prefix == version).and_then(|(_, generation)| generation.parse().ok());
    let states = drive.core.folder_states(&f.folder, f.after.as_deref(), limit, STATES_BYTES, unless).await?;
    Ok(Json(api::FolderStates {
        drive: drive.drive(),
        folder: f.folder,
        version: format!("{version}.{}", states.generation),
        unchanged: states.unchanged,
        state: states.state,
        entries: states.entries,
        next: states.next,
    }))
}

/// The remembered mounts in `store`, for the daemon to restore.
pub(crate) fn stored(store: &Store) -> voidfs_client::Result<Vec<Remembered>> {
    voidfs_client::mounts::remembered(store)
}
