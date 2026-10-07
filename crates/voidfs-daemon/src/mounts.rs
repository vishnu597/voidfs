// SPDX-License-Identifier: Apache-2.0
//! The mount table: what is mounted where, by which adapter, with its drive's change feed; and the
//! remembered mounts, which the daemon mounts again when it starts. Mounting is an [`Adapter`]'s
//! (step 5: FSKit, SMB or FUSE); the `void` binary has none yet.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{SecondsFormat, Utc};
use futures::future::BoxFuture;
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
    /// Mounts `spec`, serving it from `core`. The mountpoint exists.
    fn mount<'a>(&'a self, spec: &'a MountSpec, core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>>;
}

/// A drive an adapter has mounted.
pub trait Mounted: Send + Sync + 'static {
    /// What the drive's change feed made stale, for the adapter to tell the kernel.
    fn invalidate(&self, _changes: &[Invalidation]) {}
    /// Unmounts it.
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>>;
}

enum Is {
    Mounting,
    Mounted(Arc<dyn Mounted>),
    Failed(String),
}

struct Live {
    spec: MountSpec,
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
    /// One mount or unmount at a time.
    ops: tokio::sync::Mutex<()>,
}

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
        Mounts { adapters, root, live: Mutex::new(Vec::new()), ops: tokio::sync::Mutex::new(()) }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, Vec<Live>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn table(&self) -> Vec<api::Mount> {
        self.live().iter().map(Live::api).collect()
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
        let effective = MountSpec { read_only: spec.read_only || !drive.pinned, ..spec.clone() };
        let handle = adapter.mount(&effective, &s.core(drive.core.clone())).await.map_err(|m| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "MountFailed", m))?;
        drive.observe(handle.clone());
        let unwanted = {
            let mut live = s.mounts.live();
            match live.iter_mut().find(|l| l.spec.mountpoint == spec.mountpoint) {
                Some(l) => {
                    l.spec.read_only = effective.read_only;
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
            s.mounts.live().push(Live { spec: spec.clone(), adapter: r.adapter.clone(), since: now(), is, drive: None });
            let Ok(adapter) = adapter else { continue };
            let s = s.clone();
            tokio::spawn(async move {
                let _op = s.mounts.ops.lock().await;
                if let Err(f) = Mounts::attach(&s, &spec, &adapter, None).await
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
    let drive = session.drive.clone();
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
        live.push(Live { spec: spec.clone(), adapter: adapter.name().into(), since: now(), is: Is::Mounting, drive: None });
    }
    if let Err(f) = Mounts::attach(&s, &spec, &adapter, Some(session)).await {
        s.mounts.live().retain(|l| l.spec.mountpoint != mountpoint);
        return Err(f);
    }
    let r = Remembered { mountpoint: mountpoint.display().to_string(), drive, adapter: adapter.name().into(), read_only: m.read_only };
    let store = s.store.clone();
    blocking(move || voidfs_client::mounts::remember(&store, &r)).await?;
    let view = s.mounts.live().iter().find(|l| l.spec.mountpoint == mountpoint).map(Live::api);
    view.map(Json).ok_or_else(|| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "InternalError", "the mount went away"))
}

/// Unmounts and forgets what `target` names: a mountpoint, or every mount of a drive.
pub(crate) async fn unmount(State(s): State<Arc<Shared>>, Json(u): Json<api::Unmount>) -> Answer<api::Unmounted> {
    let _op = s.mounts.ops.lock().await;
    let target = if u.target.starts_with('/') { key(&u.target) } else { u.target.clone() };
    // A drive named by id finds its mounts by alias.
    let alias = if target.starts_with('/') { None } else { s.drive(&target).await.ok() };
    let names = |drive: &str, mountpoint: &str| mountpoint == target || drive == target || alias.as_deref() == Some(drive);
    let store = s.store.clone();
    let remembered = blocking(move || voidfs_client::mounts::remembered(&store)).await?;
    let gone: Vec<Live> = {
        let mut live = s.mounts.live();
        let (gone, kept) = std::mem::take(&mut *live).into_iter().partition(|l| names(&l.spec.drive, &l.spec.mountpoint.display().to_string()) || l.drive.as_ref().is_some_and(|d| d.id == target));
        *live = kept;
        gone
    };
    let mut unmounted: Vec<String> = Vec::new();
    for l in gone {
        if let Is::Mounted(h) = l.is
            && let Err(e) = h.unmount().await
        {
            return Err(Failure::new(StatusCode::INTERNAL_SERVER_ERROR, "UnmountFailed", format!("{}: {e}", l.spec.mountpoint.display())));
        }
        unmounted.push(l.spec.mountpoint.display().to_string());
    }
    let forgotten = remembered.into_iter().filter(|r| names(&r.drive, &r.mountpoint) || unmounted.contains(&r.mountpoint)).collect::<Vec<_>>();
    for r in forgotten {
        let store = s.store.clone();
        let mp = r.mountpoint.clone();
        blocking(move || voidfs_client::mounts::forget(&store, &mp)).await?;
        if !unmounted.contains(&r.mountpoint) {
            unmounted.push(r.mountpoint);
        }
    }
    if unmounted.is_empty() {
        return Err(Failure::new(StatusCode::NOT_FOUND, "NoSuchMount", format!("nothing is mounted or remembered at or for {}", u.target)));
    }
    Ok(Json(api::Unmounted { unmounted }))
}

/// The remembered mounts in `store`, for the daemon to restore.
pub(crate) fn stored(store: &Store) -> voidfs_client::Result<Vec<Remembered>> {
    voidfs_client::mounts::remembered(store)
}
