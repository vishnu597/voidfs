// SPDX-License-Identifier: Apache-2.0
//! Shared local bytes and immutable journal snapshots.

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::journal::{Attrs, Entry, Op, StoredBase};

pub(super) struct File {
    pub state: Arc<tokio::sync::Mutex<stage::Stage>>,
    pub attr: Mutex<Attr>,
    pub guard: Mutex<(Attr, String)>,
    timer: AtomicBool,
    touched: Mutex<tokio::time::Instant>,
}

pub(crate) struct Staged {
    store: Arc<Store>, drive: String, root: Ino, queue: Option<crate::Queue>, cfg: StagingConfig,
    files: Mutex<HashMap<Ino, Arc<File>>>, creation: Arc<tokio::sync::Mutex<()>>, handles: Mutex<Vec<Weak<Handle>>>,
    normal_commits: AtomicU64, full_commits: AtomicU64,
    damaged: HashSet<Ino>,
    recovered: tokio::sync::watch::Sender<bool>,
    _lease: Arc<crate::store::MountLease>,
    local: Arc<notify::Observer>,
}

impl Staged {
    pub(super) async fn load(store: Arc<Store>, drive: String, root: Ino, queue: Option<crate::Queue>, cfg: StagingConfig, lease: Arc<crate::store::MountLease>, local: Arc<notify::Observer>) -> Result<Arc<Self>> {
        let (s, d, writer) = (store.clone(), drive.clone(), queue.is_some());
        let (files, damaged) = tokio::task::spawn_blocking(move || {
            if writer {
                // No handle survives a restart: a stage no name reaches can't publish or be read.
                s.with(|c| {
                    let tx = c.transaction()?;
                    let staged = tx.prepare("SELECT s.ino FROM mount_staged s JOIN mount_inodes n ON n.ino=s.ino WHERE n.drive=?1")?
                        .query_map([&d], |r| r.get::<_, Ino>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    for ino in staged {
                        if unreachable(&tx, &d, root, ino)? { tx.execute("DELETE FROM mount_staged WHERE ino=?1", [ino])?; }
                    }
                    tx.commit()
                })?;
                if let Err(e) = stage::collect(&s, &d) { eprintln!("voidfs mount staging cleanup: {e}"); }
            }
            let (stages, damaged) = stage::load(&s, &d)?;
            for (ino, error) in &damaged { eprintln!("voidfs mount staging: inode {ino} is unusable: {error}"); }
            s.with(|c| {
                for (ino, _) in &damaged { c.execute("UPDATE mount_inodes SET sync='error', generation=generation+1 WHERE ino=?1 AND sync<>'error'", [ino])?; }
                Ok(())
            })?;
            let damaged = damaged.into_iter().map(|(ino, _)| ino).collect::<HashSet<_>>();
            let files = stages.into_iter().map(|(ino, state)| {
                let (n, owned) = s.with(|c| {
                    let n = node(c, &d, ino)?;
                    let owned = match &n {
                        Some(n) => c.query_row("SELECT EXISTS(SELECT 1 FROM entries WHERE mount_ino=?1 AND state='done' AND version=?2)", params![ino, n.entry.version_id], |r| r.get::<_, bool>(0))?,
                        None => false,
                    };
                    Ok((n, owned))
                })?;
                let n = n.ok_or(FsError::Stale)?;
                let attr = n.attr()?;
                let guard = if owned { (attr.clone(), n.remote_key.unwrap_or_else(|| state.record.key.clone())) } else { (state.record.base.clone(), state.record.key.clone()) };
                Ok((ino, Arc::new(File { state: Arc::new(tokio::sync::Mutex::new(state)), attr: Mutex::new(attr), guard: Mutex::new(guard), timer: AtomicBool::new(false), touched: Mutex::new(tokio::time::Instant::now()) })))
            }).collect::<Result<HashMap<_, _>>>()?;
            Ok::<_, FsError>((files, damaged))
        }).await.map_err(Error::from)??;
        Ok(Arc::new(Self { store, drive, root, queue, cfg, files: Mutex::new(files), creation: Arc::new(tokio::sync::Mutex::new(())), handles: Mutex::new(Vec::new()), normal_commits: AtomicU64::new(0), full_commits: AtomicU64::new(0), damaged, recovered: tokio::sync::watch::channel(true).0, _lease: lease, local }))
    }

    /// Whether this inode's staged bytes were lost: opening it fails rather than read other bytes.
    pub(super) fn damaged(&self, ino: Ino) -> bool { self.damaged.contains(&ino) }

    /// Drops the stage of an inode no name reaches once no handle has it open: its bytes can't
    /// publish or be read again. Only a local unlink or replacement leaves a staged inode so:
    /// a refresh keeps names with unpublished changes. A conflict keeps its stage.
    pub(super) async fn discard_unlinked(&self, ino: Ino) -> Result<()> {
        let _creation = self.creation.lock().await;
        let open = self.handles.lock().unwrap_or_else(|p| p.into_inner()).iter().filter_map(Weak::upgrade)
            .any(|handle| handle.attr.ino == ino && !handle.closed.load(Ordering::Acquire));
        if open { return Ok(()); }
        let file = self.file(ino);
        let mut state = match &file { Some(file) => Some(file.state.clone().lock_owned().await), None => None };
        let (store, drive, root) = (self.store.clone(), self.drive.clone(), self.root);
        let gone = tokio::task::spawn_blocking(move || store.with(|c| {
            let tx = c.transaction()?;
            if !unreachable(&tx, &drive, root, ino)? { return Ok(None); }
            let path = tx.query_row("DELETE FROM mount_staged WHERE ino=?1 RETURNING path", [ino], |r| r.get::<_, String>(0)).optional()?;
            tx.commit()?;
            Ok(Some(path))
        })).await.map_err(Error::from)??;
        let Some(path) = gone else { return Ok(()); };
        match (&file, &mut state) {
            (Some(file), Some(state)) => {
                state.retired = true;
                let mut files = self.files.lock().unwrap_or_else(|p| p.into_inner());
                if files.get(&ino).is_some_and(|active| Arc::ptr_eq(active, file)) { files.remove(&ino); }
            }
            _ => if let Some(name) = path.as_deref().and_then(|path| std::path::Path::new(path).file_name()) {
                let _ = std::fs::remove_file(self.store.dir().join("mount-stage").join(name));
            },
        }
        Ok(())
    }

    /// A committed mode or mtime change reaches the live stage's view, and the local state
    /// that handles of its inode already show.
    pub(super) fn attributes_changed(&self, ino: Ino, attr: &Attr) {
        if let Some(file) = self.file(ino) { *file.attr.lock().unwrap_or_else(|p| p.into_inner()) = attr.clone(); }
        let handles = self.handles.lock().unwrap_or_else(|p| p.into_inner());
        for handle in handles.iter().filter_map(Weak::upgrade).filter(|handle| handle.attr.ino == ino) {
            if let Some(local) = handle.local_attr.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
                (local.mode, local.mtime, local.generation, local.sync) = (attr.mode, attr.mtime, attr.generation, attr.sync);
            }
        }
    }

    pub(super) fn file(&self, ino: Ino) -> Option<Arc<File>> { self.files.lock().unwrap_or_else(|p| p.into_inner()).get(&ino).cloned() }

    pub(super) async fn opening(&self) -> tokio::sync::OwnedMutexGuard<()> { self.creation.clone().lock_owned().await }

    pub(super) fn register(&self, handle: &Arc<Handle>) {
        let mut handles = self.handles.lock().unwrap_or_else(|p| p.into_inner());
        handles.retain(|handle| handle.strong_count() > 0);
        handles.push(Arc::downgrade(handle));
    }

    pub(super) async fn retire_saved(&self, ino: Ino, attr: &Attr, persisted: bool) {
        if self.queue.is_some() || attr.sync != Sync::Saved || persisted { return; }
        let Some(file) = self.file(ino) else { return; };
        let mut state = file.state.lock().await;
        state.retired = true;
        let mut retained = file.attr.lock().unwrap_or_else(|p| p.into_inner()).clone();
        retained.sync = Sync::Saved;
        retained.generation = attr.generation;
        *file.attr.lock().unwrap_or_else(|p| p.into_inner()) = retained.clone();
        let mut handles = self.handles.lock().unwrap_or_else(|p| p.into_inner());
        handles.retain(|handle| handle.strong_count() > 0);
        for handle in handles.iter().filter_map(Weak::upgrade).filter(|handle| handle.attr.ino == ino) {
            *handle.local_attr.lock().unwrap_or_else(|p| p.into_inner()) = Some(retained.clone());
            handle.frozen.lock().unwrap_or_else(|p| p.into_inner()).push(file.clone());
        }
        self.files.lock().unwrap_or_else(|p| p.into_inner()).remove(&ino);
    }

    async fn ensure(&self, handle: &Handle) -> Result<(Arc<File>, tokio::sync::OwnedMutexGuard<stage::Stage>)> {
        let ino = handle.attr.ino;
        let _creation = self.creation.lock().await;
        let published = handle.published.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let caller = published.as_ref().map(|(attr, _)| attr).unwrap_or(&handle.attr);
        let file = match self.file(ino) {
            Some(file) => file,
            None => {
                let (attr, key) = match &published { Some(published) => published.clone(), None => (handle.attr.clone(), handle.reader.lock().await.content().key.clone()) };
                let guard = (attr.clone(), key.clone());
                let (store, base) = (self.store.clone(), attr.clone());
                let state = tokio::task::spawn_blocking(move || stage::Stage::new(&store, base, key)).await.map_err(Error::from)??;
                let file = Arc::new(File { attr: Mutex::new(attr), state: Arc::new(tokio::sync::Mutex::new(state)), guard: Mutex::new(guard), timer: AtomicBool::new(false), touched: Mutex::new(tokio::time::Instant::now()) });
                self.files.lock().unwrap_or_else(|p| p.into_inner()).insert(ino, file.clone());
                file
            }
        };
        let state = file.state.clone().lock_owned().await;
        let compatible = {
            let guard = file.guard.lock().unwrap_or_else(|p| p.into_inner());
            caller.object_id == guard.0.object_id && caller.version_id == guard.0.version_id
        };
        if !compatible { return Err(FsError::Stale); }
        Ok((file, state))
    }

    pub(crate) async fn prepare_publication(self: &Arc<Self>, run: &[Entry]) -> Prepared {
        let creation = self.creation.clone().lock_owned().await;
        let mut active = self.files.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|(ino, file)| (*ino, file.clone())).collect::<Vec<_>>();
        active.sort_unstable_by_key(|(ino, _)| *ino);
        let mut files = Vec::new();
        for (ino, file) in active {
            let state = file.state.clone().lock_owned().await;
            files.push((ino, file, state));
        }
        Prepared { staged: self.clone(), _creation: creation, files, keys: run.iter().filter_map(|entry| entry.mount_ino.map(|ino| (ino, entry.to_key.clone().unwrap_or_else(|| entry.key.clone())))).collect() }
    }

    /// Queues, in the background, what a previous session staged without flushing: no handle of
    /// it is left to close or fsync. A stage that can't be flushed now waits for its quiet period,
    /// or the next flush. It stops with the session; the next writer starts it again.
    pub(super) fn recover(self: &Arc<Self>) {
        let files = self.files.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|(ino, file)| (*ino, Arc::downgrade(file))).collect::<Vec<_>>();
        self.recovered.send_replace(false);
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            use futures::StreamExt;
            futures::stream::iter(files).for_each_concurrent(8, |(ino, file)| {
                let weak = weak.clone();
                async move {
                    let (Some(this), Some(file)) = (weak.upgrade(), file.upgrade()) else { return };
                    let dirty = { let state = file.state.lock().await; state.record.revision != state.record.flushed };
                    if dirty && let Err(e) = this.flush(ino, file).await { eprintln!("voidfs mount staged recovery: {e}"); }
                }
            }).await;
            if let Some(this) = weak.upgrade() { this.recovered.send_replace(true); }
        });
    }

    pub(super) async fn recovered(&self) {
        let _ = self.recovered.subscribe().wait_for(|done| *done).await;
    }

    pub(super) fn restart_timers(self: &Arc<Self>) {
        let files = self.files.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|(ino, file)| (*ino, file.clone())).collect::<Vec<_>>();
        for (ino, file) in files {
            let dirty = file.state.try_lock().is_ok_and(|s| s.record.revision != s.record.flushed);
            if dirty { self.schedule(ino, &file); }
        }
    }

    fn schedule(self: &Arc<Self>, ino: Ino, file: &Arc<File>) {
        let Some(quiet) = self.cfg.quiet_period else { return; };
        if self.queue.is_none() || file.timer.swap(true, Ordering::AcqRel) { return; }
        let delay = quiet.saturating_sub(file.touched.lock().unwrap_or_else(|p| p.into_inner()).elapsed());
        let (weak, weak_file) = (Arc::downgrade(self), Arc::downgrade(file));
        tokio::spawn(async move {
            let mut delay = delay;
            loop {
                tokio::time::sleep(delay).await;
                let (Some(this), Some(file)) = (weak.upgrade(), weak_file.upgrade()) else { return; };
                let creation = this.creation.lock().await;
                if this.file(ino).is_none_or(|active| !Arc::ptr_eq(&active, &file)) { return; }
                let state = file.state.clone().lock_owned().await;
                drop(creation);
                let elapsed = file.touched.lock().unwrap_or_else(|p| p.into_inner()).elapsed();
                if elapsed < quiet { delay = quiet - elapsed; continue; }
                let attempted = state.record.revision;
                let successful = match this.flush_owned(ino, file.clone(), state).await {
                    Ok(()) => true,
                    Err(e) => { eprintln!("voidfs mount staged flush: {e}"); false },
                };
                this.finish_timer(ino, &file, attempted, successful).await;
                return;
            }
        });
    }

    async fn finish_timer(self: &Arc<Self>, ino: Ino, file: &Arc<File>, attempted: u64, successful: bool) {
        file.timer.store(false, Ordering::Release);
        let state = file.state.lock().await;
        let dirty = state.record.revision != state.record.flushed;
        if dirty && (successful || state.record.revision != attempted) {
            drop(state);
            self.schedule(ino, file);
        }
    }

    pub(super) async fn flush(self: &Arc<Self>, ino: Ino, file: Arc<File>) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let creation = this.creation.lock().await;
            if this.file(ino).is_none_or(|active| !Arc::ptr_eq(&active, &file)) { return Ok(()); }
            let state = file.state.clone().lock_owned().await;
            drop(creation);
            this.flush_inner(ino, file, state).await
        }).await.map_err(Error::from)?
    }

    async fn flush_owned(self: &Arc<Self>, ino: Ino, file: Arc<File>, state: tokio::sync::OwnedMutexGuard<stage::Stage>) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move { this.flush_inner(ino, file, state).await }).await.map_err(Error::from)?
    }

    async fn flush_inner(&self, ino: Ino, file: Arc<File>, state: tokio::sync::OwnedMutexGuard<stage::Stage>) -> Result<()> {
        let queue = self.queue.as_ref().ok_or(FsError::ReadOnly)?;
        let (store, cfg) = (self.store.clone(), self.cfg.clone());
        let state = tokio::task::spawn_blocking(move || {
            let mut state = state;
            state.sync()?;
            // Overwritten bytes stay in the append-only file until a flush rewrites it.
            let (garbage, live) = state.garbage()?;
            if garbage > cfg.compact_garbage.max(live) && let Err(e) = state.compact(&store, ino, &cfg) { eprintln!("voidfs mount staging compaction: {e}"); }
            Ok::<_, FsError>(state)
        }).await.map_err(Error::from)??;
        if state.record.revision == state.record.flushed {
            let store = self.store.clone();
            tokio::task::spawn_blocking(move || store.with(|c| {
                let tx = c.transaction()?;
                tx.execute("UPDATE mount_staged SET record=record WHERE ino=?1", [ino])?;
                tx.commit()
            })).await.map_err(Error::from)??;
            self.full_commits.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let mut entries = Vec::new();
        let mut sources = Vec::new();
        let full = state.record.base.object_id.is_none() && covered(&state.record);
        let frozen = async {
            if full {
                let record = state.record.clone();
                let source = state.path.clone();
                let dir = state.path.parent().ok_or_else(|| FsError::Io("staging path has no parent".into()))?.to_owned();
                let assembled = tokio::task::spawn_blocking(move || assemble(&dir, &source, &record)).await.map_err(Error::from)??;
                let result = queue.mount_copy(assembled.clone(), 0, state.record.size).await;
                let _ = tokio::fs::remove_file(assembled).await;
                let path = result?;
                sources.push(path.clone());
                let mut entry = Entry::new(&self.drive, "", Op::Put, StoredBase::Any);
                entry.source = Some(path);
                entry.staged = true;
                entry.size = state.record.size;
                entries.push(entry);
            } else {
                if let Some(size) = state.record.reset {
                    let mut entry = Entry::new(&self.drive, "", Op::Truncate, StoredBase::Any);
                    entry.length = size;
                    entries.push(entry);
                }
                for extent in &state.record.extents {
                    for &(start, end) in &state.record.dirty {
                        let mut at = extent.start.max(start);
                        let stop = extent.end.min(end);
                        while at < stop {
                            let length = (stop - at).min(8 * 1024 * 1024);
                            let path = queue.mount_copy(state.path.clone(), extent.physical + at - extent.start, length).await?;
                            sources.push(path.clone());
                            let mut entry = Entry::new(&self.drive, "", Op::Write, StoredBase::Any);
                            entry.source = Some(path);
                            entry.staged = true;
                            entry.offset = at;
                            entry.length = length;
                            entry.size = length;
                            entries.push(entry);
                            at += length;
                        }
                    }
                }
                let mut entry = Entry::new(&self.drive, "", Op::Truncate, StoredBase::Any);
                entry.length = state.record.size;
                entries.push(entry);
            }
            Ok::<_, FsError>(())
        }.await;
        if let Err(error) = frozen { clean(&sources).await; return Err(error); }
        crate::kill::point("flush.frozen");
        let mut record = state.record.clone();
        record.dirty.clear();
        record.reset = None;
        record.flushed = record.revision;
        let (drive, root) = (self.drive.clone(), self.root);
        let json = serde_json::to_string(&record).map_err(|e| FsError::Io(e.to_string()))?;
        let version = record.base.version_id.clone();
        let result = queue.mount_transaction(move |tx| {
            let mut n = node(tx, &drive, ino)?.ok_or(FsError::Stale)?;
            let previous = n.sync;
            let linked = chain(tx, &drive, root, ino)?;
            if let Some(path) = linked {
                let key = path.into_iter().skip(1).map(|(_, name)| name).collect::<Vec<_>>().join("/");
                entries[0].base = match n.entry_id {
                    Some(id) => StoredBase::Entry(id),
                    None => version.filter(|v| !v.is_empty()).map(StoredBase::Version).ok_or(FsError::Again)?,
                };
                for entry in &mut entries { entry.key = key.clone(); entry.mount_ino = Some(ino); }
                let mut attrs = Attrs { mtime: n.entry.mtime.clone(), mode: Some(n.attr()?.mode), ..Default::default() };
                if full {
                    let json: String = tx.query_row("SELECT attrs FROM mount_xattrs WHERE ino=?1", [ino], |r| r.get(0))?;
                    attrs.xattrs = serde_json::from_str(&json).map_err(|e| FsError::Io(e.to_string()))?;
                    entries[0].attrs = attrs;
                } else {
                    let mut entry = Entry::new(&drive, &key, Op::Attrs, StoredBase::Any);
                    entry.attrs = attrs;
                    entry.mount_ino = Some(ino);
                    entries.push(entry);
                }
                if !matches!(n.sync, Sync::Conflict | Sync::Error) { n.sync = Sync::Saving; }
            } else { entries.clear(); }
            tx.execute("UPDATE mount_staged SET record=?2 WHERE ino=?1", params![ino, json])?;
            save_node(tx, &n)?;
            let change = if n.sync != previous { Some(notify::attribute(tx, &drive, root, ino)?) } else { None };
            Ok(((n.attr()?, change, !entries.is_empty()), entries))
        }).await;
        let (attr, change, queued) = match result { Ok(result) => result, Err(error) => { clean(&sources).await; return Err(error); } };
        crate::kill::point("flush.committed");
        if !queued { clean(&sources).await; }
        let mut state = state;
        state.record = record;
        *file.attr.lock().unwrap_or_else(|p| p.into_inner()) = attr;
        self.full_commits.fetch_add(1, Ordering::Relaxed);
        drop(state);
        if let Some(change) = change { self.local.send(change); }
        Ok(())
    }
}

/// No name reaches the inode and no conflict needs its local bytes.
fn unreachable(c: &Connection, drive: &str, root: Ino, ino: Ino) -> rusqlite::Result<bool> {
    Ok(chain(c, drive, root, ino)?.is_none() && !c.query_row("SELECT EXISTS(SELECT 1 FROM mount_conflicts WHERE ino=?1
        UNION ALL SELECT 1 FROM mount_conflict_blockers WHERE ino=?1)", [ino], |r| r.get::<_, bool>(0))?)
}

pub(crate) struct Prepared {
    staged: Arc<Staged>,
    _creation: tokio::sync::OwnedMutexGuard<()>,
    files: Vec<(Ino, Arc<File>, tokio::sync::OwnedMutexGuard<stage::Stage>)>,
    keys: HashMap<Ino, String>,
}

impl Prepared {
    pub(crate) fn apply(mut self, reports: Vec<publication::Report>) {
        let mut changes = Vec::new();
        {
            let mut registered = self.staged.handles.lock().unwrap_or_else(|p| p.into_inner());
            let handles = registered.iter().filter_map(Weak::upgrade).collect::<Vec<_>>();
            registered.retain(|handle| handle.strong_count() > 0);
            for report in reports {
                // An unlinked file's open handles keep writing its live stage, whose bytes no
                // publication holds; the last close discards it.
                let open = handles.iter().any(|handle| handle.attr.ino == report.ino && !handle.closed.load(Ordering::Acquire));
                let retire = report.clean && (report.linked || !open);
                let file = self.files.iter().find(|(ino, _, _)| *ino == report.ino).map(|(_, file, _)| file);
                if let Some(file) = file { *file.attr.lock().unwrap_or_else(|p| p.into_inner()) = report.attr.clone(); }
                if let Some(file) = file && let Some(published) = &report.published && let Some(key) = self.keys.get(&report.ino) {
                    *file.guard.lock().unwrap_or_else(|p| p.into_inner()) = (published.clone(), key.clone());
                }
                for handle in handles.iter().filter(|handle| handle.attr.ino == report.ino) {
                    if file.is_some() || !handle.frozen.lock().unwrap_or_else(|p| p.into_inner()).is_empty() {
                        *handle.local_attr.lock().unwrap_or_else(|p| p.into_inner()) = Some(report.attr.clone());
                    }
                    if let Some(published) = &report.published && let Some(key) = self.keys.get(&report.ino) {
                        *handle.published.lock().unwrap_or_else(|p| p.into_inner()) = Some((published.clone(), key.clone()));
                    }
                    if retire && let Some(file) = file {
                        let mut frozen = handle.frozen.lock().unwrap_or_else(|p| p.into_inner());
                        if !frozen.last().is_some_and(|previous| Arc::ptr_eq(previous, file)) { frozen.push(file.clone()); }
                    }
                }
                if retire && let Some(file) = file {
                    let mut files = self.staged.files.lock().unwrap_or_else(|p| p.into_inner());
                    if files.get(&report.ino).is_some_and(|active| Arc::ptr_eq(active, file)) { files.remove(&report.ino); }
                    // Its record is gone: the bytes remain only for handles that still read them.
                    if let Some((_, _, state)) = self.files.iter_mut().find(|(ino, _, _)| *ino == report.ino) { state.retired = true; }
                }
                changes.push(self.staged.store.with(|c| notify::attribute(c, &self.staged.drive, self.staged.root, report.ino)).unwrap_or_else(|_| LocalChange { invalidations: Vec::new(), inodes: vec![report.ino], namespace: false }));
            }
        }
        let observer = self.staged.local.clone();
        drop(self);
        for change in changes { observer.send(change); }
    }
}

pub(super) struct ViewExtent { pub start: u64, pub end: u64, pub physical: u64, pub path: PathBuf }
pub(super) struct View { pub size: u64, pub remote_size: u64, pub extents: Vec<ViewExtent> }

impl View {
    pub fn new(size: u64) -> Self { Self { size, remote_size: size, extents: Vec::new() } }

    pub fn overlay(&mut self, state: &stage::Stage) {
        self.size = state.record.size;
        self.remote_size = self.remote_size.min(state.record.remote_size);
        self.extents.retain(|extent| extent.start < state.record.remote_size);
        for extent in &mut self.extents { extent.end = extent.end.min(state.record.remote_size); }
        for next in &state.record.extents {
            let mut extents = Vec::with_capacity(self.extents.len() + 2);
            for extent in self.extents.drain(..) {
                if extent.end <= next.start || extent.start >= next.end { extents.push(extent); continue; }
                if extent.start < next.start { extents.push(ViewExtent { start: extent.start, end: next.start, physical: extent.physical, path: extent.path.clone() }); }
                if extent.end > next.end { extents.push(ViewExtent { start: next.end, end: extent.end, physical: extent.physical + next.end - extent.start, path: extent.path }); }
            }
            extents.push(ViewExtent { start: next.start, end: next.end, physical: next.physical, path: state.path.clone() });
            extents.sort_unstable_by_key(|extent| extent.start);
            self.extents = extents;
        }
    }
}

fn covered(record: &stage::Record) -> bool {
    let mut at = 0;
    for extent in &record.extents {
        if extent.start > at { return false; }
        at = at.max(extent.end);
    }
    at >= record.size
}

async fn clean(paths: &[PathBuf]) { for path in paths { let _ = tokio::fs::remove_file(path).await; } }

fn assemble(dir: &std::path::Path, source: &std::path::Path, record: &stage::Record) -> Result<PathBuf> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut raw = [0u8; 12];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut raw)?;
    let path = dir.join(format!("{}-assembly-{}", record.base.ino, hex::encode(raw)));
    let result = (|| {
        let mut dest = std::fs::File::options().create_new(true).write(true).open(&path)?;
        dest.set_len(record.size)?;
        let mut src = std::fs::File::open(source)?;
        let mut buffer = vec![0u8; 1024 * 1024];
        for extent in &record.extents {
            dest.seek(SeekFrom::Start(extent.start))?;
            src.seek(SeekFrom::Start(extent.physical))?;
            let mut left = extent.end - extent.start;
            while left > 0 {
                let n = buffer.len().min(left as usize);
                src.read_exact(&mut buffer[..n])?;
                dest.write_all(&buffer[..n])?;
                left -= n as u64;
            }
        }
        dest.sync_all()?;
        Ok::<_, FsError>(())
    })();
    if let Err(e) = result { let _ = std::fs::remove_file(&path); return Err(e); }
    Ok(path)
}

impl Session {
    /// Successful NORMAL staging and FULL flush commits since this session opened.
    pub fn staging_commits(&self) -> (u64, u64) { (self.data.normal_commits.load(Ordering::Relaxed), self.data.full_commits.load(Ordering::Relaxed)) }

    pub async fn write(&self, fh: Fh, offset: u64, bytes: Bytes) -> Result<usize> {
        let handle = self.handle(fh)?;
        if !handle.write { return Err(FsError::BadHandle); }
        if offset.checked_add(bytes.len() as u64).is_none_or(|end| end > i64::MAX as u64) { return Err(FsError::InvalidArgument); }
        if bytes.is_empty() { return Ok(0); }
        let (file, mut state) = self.data.ensure(&handle).await?;
        if handle.closed.load(Ordering::Acquire) { return Err(FsError::BadHandle); }
        let (store, drive, cfg, view, len, ino, data, root) = (self.store.clone(), self.drive.clone(), self.data.cfg.clone(), file.clone(), bytes.len(), handle.attr.ino, self.data.clone(), self.root);
        let attr = tokio::task::spawn_blocking(move || {
            let attr = state.append(&store, &drive, ino, offset, &bytes, &cfg)?;
            *view.attr.lock().unwrap_or_else(|p| p.into_inner()) = attr.clone();
            *view.touched.lock().unwrap_or_else(|p| p.into_inner()) = tokio::time::Instant::now();
            data.normal_commits.fetch_add(1, Ordering::Relaxed);
            data.schedule(ino, &view);
            drop(state);
            let change = store.with(|c| notify::attribute(c, &drive, root, ino)).unwrap_or_else(|_| LocalChange { invalidations: Vec::new(), inodes: vec![ino], namespace: false });
            data.local.send(change);
            Ok::<_, FsError>(attr)
        }).await.map_err(Error::from)??;
        self.data.schedule(attr.ino, &file);
        Ok(len)
    }

    pub async fn truncate(&self, fh: Fh, size: u64) -> Result<()> {
        let handle = self.handle(fh)?;
        if !handle.write { return Err(FsError::BadHandle); }
        if size > i64::MAX as u64 { return Err(FsError::InvalidArgument); }
        let (file, mut state) = self.data.ensure(&handle).await?;
        if handle.closed.load(Ordering::Acquire) { return Err(FsError::BadHandle); }
        if size == state.record.size { return Ok(()); }
        let (store, drive, cfg, view, ino, data, root) = (self.store.clone(), self.drive.clone(), self.data.cfg.clone(), file.clone(), handle.attr.ino, self.data.clone(), self.root);
        let attr = tokio::task::spawn_blocking(move || {
            let attr = state.truncate(&store, &drive, ino, size, &cfg)?;
            *view.attr.lock().unwrap_or_else(|p| p.into_inner()) = attr.clone();
            *view.touched.lock().unwrap_or_else(|p| p.into_inner()) = tokio::time::Instant::now();
            data.normal_commits.fetch_add(1, Ordering::Relaxed);
            data.schedule(ino, &view);
            drop(state);
            let change = store.with(|c| notify::attribute(c, &drive, root, ino)).unwrap_or_else(|_| LocalChange { invalidations: Vec::new(), inodes: vec![ino], namespace: false });
            data.local.send(change);
            Ok::<_, FsError>(attr)
        }).await.map_err(Error::from)??;
        self.data.schedule(attr.ino, &file);
        Ok(())
    }

    /// Flushes local bytes and metadata, then durably queues a frozen guarded snapshot.
    pub async fn fsync(&self, fh: Fh) -> Result<()> {
        let handle = self.handle(fh)?;
        if let Some(file) = self.data.file(handle.attr.ino) { self.data.flush(handle.attr.ino, file).await?; }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (tempfile::TempDir, Arc<Session>, crate::Queue, Fh) {
        fixture_with_quiet(None).await
    }

    async fn fixture_with_quiet(quiet: Option<Duration>) -> (tempfile::TempDir, Arc<Session>, crate::Queue, Fh) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let client = Client::new(voidfs_sdk::Config { endpoint: "http://localhost:9".into(), access_key_id: "test".into(), secret_access_key: "test".into(), ..Default::default() }).unwrap();
        let connectivity = Connectivity::default();
        for _ in 0..3 { connectivity.unanswered(); }
        let cache = Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone())), crate::CacheConfig::default()).await.unwrap();
        let queue = crate::Queue::open(store.clone(), client.clone(), crate::QueueConfig { connectivity: Some(connectivity.clone()), ..Default::default() }).await.unwrap();
        queue.pause(crate::Scope::All).await.unwrap();
        let session = Arc::new(Session::new_writable_with_config(store.clone(), client, cache, queue.clone(), "drive", connectivity,
            StagingConfig { min_free_bytes: 0, quiet_period: quiet, ..Default::default() }).await.unwrap());
        store.with(|c| c.execute("UPDATE mount_dirs SET listed=1, seq=1 WHERE ino=?1", [session.root()])).unwrap();
        let ino = session.create(session.root(), "file", 0o644).await.unwrap().ino;
        let fh = session.open(ino, true).await.unwrap();
        if quiet.is_some() {
            let handle = session.handle(fh).unwrap();
            let (file, _) = session.data.ensure(&handle).await.unwrap();
            file.timer.store(true, Ordering::Release);
        }
        session.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
        (dir, session, queue, fh)
    }

    async fn clean_publication(session: &Session, queue: &crate::Queue, fh: Fh, version: &str) {
        session.fsync(fh).await.unwrap();
        let ino = session.handle_attr(fh).unwrap().ino;
        let run = queue.store().with(|c| crate::journal::all(c)).unwrap().into_iter().filter(|entry| entry.mount_ino == Some(ino)).collect::<Vec<_>>();
        let prepared = session.data.prepare_publication(&run).await;
        finish_publication(session, prepared, fh, version);
    }

    fn finish_publication(session: &Session, prepared: Prepared, fh: Fh, version: &str) {
        let ino = session.handle_attr(fh).unwrap().ino;
        let mut published = session.handle_attr(fh).unwrap();
        published.object_id = Some("published-object".into());
        published.version_id = Some(version.into());
        published.etag = Some(format!("etag-{version}"));
        published.sync = Sync::Saved;
        session.store.with(|c| {
            let tx = c.transaction()?;
            let mut n = node(&tx, &session.drive, ino)?.unwrap();
            n.entry.object_id = published.object_id.clone().unwrap();
            n.entry.version_id = published.version_id.clone();
            n.entry.etag = published.etag.clone();
            n.sync = Sync::Saved;
            n.entry_id = None;
            save_node(&tx, &n).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            tx.execute("DELETE FROM mount_staged WHERE ino=?1", [ino])?;
            tx.commit()
        }).unwrap();
        prepared.apply(vec![publication::Report { ino, attr: published.clone(), published: Some(published), clean: true, linked: true }]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn completed_bytes_remain_on_old_handles_while_new_opens_bind_the_published_version() {
        let (_dir, session, queue, fh) = fixture().await;
        let ino = session.handle_attr(fh).unwrap().ino;
        let earlier = session.open(ino, false).await.unwrap();
        clean_publication(&session, &queue, fh, "one").await;
        assert!(session.data.file(ino).is_none(), "a completed byte view must not mask later remote versions on new opens");
        for handle in [fh, earlier] {
            assert_eq!(session.read(handle, 0, 5).await.unwrap(), b"first"[..]);
            assert_eq!(session.handle_attr(handle).unwrap().sync, Sync::Saved);
        }
        let later = session.open(ino, false).await.unwrap();
        assert_eq!(session.handle_attr(later).unwrap().version_id.as_deref(), Some("one"));
        assert_eq!(session.read(later, 0, 5).await.unwrap_err(), FsError::Offline, "fresh opens use the remote snapshot instead of another handle's retained local bytes");
        for handle in [fh, earlier, later] { session.close(handle).await.unwrap(); }
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn later_writes_layer_over_completed_bytes_and_truncation_clips_every_older_layer() {
        let (_dir, session, queue, fh) = fixture().await;
        let ino = session.handle_attr(fh).unwrap().ino;
        clean_publication(&session, &queue, fh, "one").await;
        session.write(fh, 1, Bytes::from_static(b"A")).await.unwrap();
        assert_eq!(session.data.file(ino).unwrap().state.lock().await.record.base.version_id.as_deref(), Some("one"));
        assert_eq!(session.read(fh, 0, 5).await.unwrap(), b"fArst"[..]);
        session.truncate(fh, 2).await.unwrap();
        session.truncate(fh, 5).await.unwrap();
        assert_eq!(session.read(fh, 0, 5).await.unwrap(), b"fA\0\0\0"[..]);
        clean_publication(&session, &queue, fh, "two").await;
        session.write(fh, 4, Bytes::from_static(b"Z")).await.unwrap();
        assert_eq!(session.data.file(ino).unwrap().state.lock().await.record.base.version_id.as_deref(), Some("two"));
        assert_eq!(session.read(fh, 0, 5).await.unwrap(), b"fA\0\0Z"[..]);
        session.close(fh).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_write_waiting_for_clean_completion_gets_a_new_stage_and_the_published_guard() {
        let (_dir, session, queue, fh) = fixture().await;
        session.fsync(fh).await.unwrap();
        let ino = session.handle_attr(fh).unwrap().ino;
        let before = session.data.file(ino).unwrap();
        let run = queue.store().with(|c| crate::journal::all(c)).unwrap();
        let prepared = session.data.prepare_publication(&run).await;
        let ns = session.clone();
        let mut writing = tokio::spawn(async move { ns.write(fh, 1, Bytes::from_static(b"A")).await });
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut writing).await.is_err());
        finish_publication(&session, prepared, fh, "one");
        tokio::time::timeout(Duration::from_secs(5), writing).await.unwrap().unwrap().unwrap();
        let active = session.data.file(ino).expect("the accepted write belongs to a new active byte view");
        assert!(!Arc::ptr_eq(&before, &active));
        assert_eq!(before.state.lock().await.record.revision, 1, "publication froze the older acknowledged revision");
        assert_eq!(active.state.lock().await.record.base.version_id.as_deref(), Some("one"));
        assert_eq!(session.read(fh, 0, 5).await.unwrap(), b"fArst"[..]);
        session.close(fh).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn prepared_completion_keeps_the_writer_lease_after_a_waiting_write_is_cancelled() {
        let (_dir, session, queue, fh) = fixture().await;
        let (store, client, cache, conn) = (session.store.clone(), session.client.clone(), session.cache.clone(), session.connectivity.clone());
        let run = queue.store().with(|c| crate::journal::all(c)).unwrap();
        let prepared = session.data.prepare_publication(&run).await;
        let ns = session.clone();
        let mut writing = tokio::spawn(async move { ns.write(fh, 0, Bytes::from_static(b"later")).await });
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut writing).await.is_err());
        writing.abort();
        assert!(writing.await.unwrap_err().is_cancelled());
        drop(session);
        let cfg = StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() };
        assert!(matches!(Session::new_writable_with_config(store.clone(), client.clone(), cache.clone(), queue.clone(), "drive", conn.clone(), cfg.clone()).await, Err(FsError::Again)));
        drop(prepared);
        let reopened = Session::new_writable_with_config(store, client, cache, queue.clone(), "drive", conn, cfg).await.unwrap();
        let ino = reopened.lookup(reopened.root(), "file").await.unwrap().ino;
        let fh = reopened.open(ino, false).await.unwrap();
        assert_eq!(reopened.read(fh, 0, 5).await.unwrap(), b"first"[..]);
        reopened.close(fh).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn preparing_a_parent_publication_waits_for_an_admitted_successor_write() {
        let (_dir, session, queue, fh) = fixture().await;
        let other = session.create(session.root(), "successor", 0o644).await.unwrap();
        let other_fh = session.open(other.ino, true).await.unwrap();
        session.write(other_fh, 0, Bytes::from_static(b"before")).await.unwrap();
        let other_file = session.data.file(other.ino).unwrap();
        let run = queue.store().with(|c| crate::journal::all(c)).unwrap().into_iter().filter(|entry| entry.mount_ino == Some(session.handle_attr(fh).unwrap().ino)).collect::<Vec<_>>();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocked_store = session.store.clone();
        let blocker = tokio::task::spawn_blocking(move || blocked_store.with(|_| {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        }));
        started_rx.await.unwrap();
        let ns = session.clone();
        let writing = tokio::spawn(async move { ns.write(other_fh, 0, Bytes::from_static(b"later!")).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while other_file.state.try_lock().is_ok() { tokio::task::yield_now().await; }
        }).await.unwrap();
        let data = session.data.clone();
        let mut preparing = tokio::spawn(async move { data.prepare_publication(&run).await });
        let waiting = tokio::time::timeout(Duration::from_millis(50), &mut preparing).await;
        let blocked = waiting.is_err();
        release_tx.send(()).unwrap();
        blocker.await.unwrap().unwrap();
        writing.await.unwrap().unwrap();
        let prepared = match waiting { Ok(done) => done.unwrap(), Err(_) => tokio::time::timeout(Duration::from_secs(5), preparing).await.unwrap().unwrap() };
        let locked_revision = prepared.files.iter().find(|(ino, _, _)| *ino == other.ino).map(|(_, _, state)| state.record.revision);
        drop(prepared);
        assert!(blocked, "a newly dependent inode can still have admitted work outside the creation lock");
        assert_eq!(locked_revision, Some(2), "the completed successor write stays locked throughout conflict snapshot capture");
        session.close(fh).await.unwrap();
        session.close(other_fh).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reopening_waits_for_publication_cleanup_before_loading_its_staged_map() {
        let (_dir, session, queue, fh) = fixture().await;
        session.fsync(fh).await.unwrap();
        let ino = session.handle_attr(fh).unwrap().ino;
        let (store, client, cache, conn) = (session.store.clone(), session.client.clone(), session.cache.clone(), session.connectivity.clone());
        drop(session);
        let registration = queue.mount_registration("drive").await;
        let (s, c, ca, q, connection) = (store.clone(), client, cache, queue.clone(), conn);
        let mut opening = tokio::spawn(async move {
            Session::new_writable_with_config(s, c, ca, q, "drive", connection, StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() }).await
        });
        let waiting = tokio::time::timeout(Duration::from_millis(50), &mut opening).await;
        let blocked = waiting.is_err();
        store.with(|c| {
            let tx = c.transaction()?;
            let mut n = node(&tx, "drive", ino)?.unwrap();
            n.entry.object_id = "published-object".into();
            n.entry.version_id = Some("one".into());
            n.entry.etag = Some("etag-one".into());
            n.sync = Sync::Saved;
            n.entry_id = None;
            save_node(&tx, &n).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            tx.execute("DELETE FROM mount_staged WHERE ino=?1", [ino])?;
            tx.commit()
        }).unwrap();
        drop(registration);
        let reopened = match waiting { Ok(done) => done.unwrap().unwrap(), Err(_) => tokio::time::timeout(Duration::from_secs(5), opening).await.unwrap().unwrap().unwrap() };
        assert!(blocked, "mount registration and queue completion share one serialization boundary");
        assert!(reopened.data.file(ino).is_none(), "cleanup cannot leave an obsolete byte view in the new session");
        let fh = reopened.open(ino, false).await.unwrap();
        assert_eq!(reopened.handle_attr(fh).unwrap().version_id.as_deref(), Some("one"));
        assert_eq!(reopened.read(fh, 0, 5).await.unwrap_err(), FsError::Offline);
        reopened.close(fh).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn readonly_new_opens_retire_a_stage_cleaned_by_the_background_queue() {
        let (_dir, session, queue, fh) = fixture().await;
        session.fsync(fh).await.unwrap();
        let ino = session.handle_attr(fh).unwrap().ino;
        let (store, client, cache, conn) = (session.store.clone(), session.client.clone(), session.cache.clone(), session.connectivity.clone());
        drop(session);
        let readonly = Session::new(store.clone(), client, cache, "drive", conn).await.unwrap();
        let earlier = readonly.open(ino, false).await.unwrap();
        assert_eq!(readonly.read(earlier, 0, 5).await.unwrap(), b"first"[..]);
        store.with(|c| {
            let tx = c.transaction()?;
            let mut n = node(&tx, "drive", ino)?.unwrap();
            n.entry.object_id = "published-object".into();
            n.entry.version_id = Some("one".into());
            n.entry.etag = Some("etag-one".into());
            n.sync = Sync::Saved;
            n.entry_id = None;
            save_node(&tx, &n).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            tx.execute("DELETE FROM mount_staged WHERE ino=?1", [ino])?;
            tx.commit()
        }).unwrap();
        let later = readonly.open(ino, false).await.unwrap();
        assert!(readonly.data.file(ino).is_none());
        assert_eq!(readonly.handle_attr(later).unwrap().version_id.as_deref(), Some("one"));
        assert_eq!(readonly.read(later, 0, 5).await.unwrap_err(), FsError::Offline);
        assert_eq!(readonly.read(earlier, 0, 5).await.unwrap(), b"first"[..]);
        assert_eq!(readonly.handle_attr(earlier).unwrap().sync, Sync::Saved);
        readonly.close(earlier).await.unwrap();
        readonly.close(later).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reopened_guard_keeps_the_original_base_until_a_matching_own_publication_is_proven() {
        let (_dir, session, queue, fh) = fixture().await;
        let ino = session.handle_attr(fh).unwrap().ino;
        let (store, client, cache, conn) = (session.store.clone(), session.client.clone(), session.cache.clone(), session.connectivity.clone());
        let file = session.data.file(ino).unwrap();
        let record = {
            let mut state = file.state.lock().await;
            state.record.base.object_id = Some("original-object".into());
            state.record.base.version_id = Some("A".into());
            state.record.base.etag = Some("etag-A".into());
            state.record.base.size = 5;
            serde_json::to_string(&state.record).unwrap()
        };
        store.with(|c| {
            let tx = c.transaction()?;
            let mut n = node(&tx, "drive", ino)?.unwrap();
            n.entry.object_id = "original-object".into();
            n.entry.version_id = Some("C".into());
            n.entry.etag = Some("etag-C".into());
            save_node(&tx, &n).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            tx.execute("UPDATE mount_staged SET record=?2 WHERE ino=?1", params![ino, record])?;
            tx.commit()
        }).unwrap();
        drop(session);
        let cfg = StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() };
        let reopened = Session::new_writable_with_config(store.clone(), client.clone(), cache.clone(), queue.clone(), "drive", conn.clone(), cfg.clone()).await.unwrap();
        assert_eq!(reopened.data.file(ino).unwrap().guard.lock().unwrap().0.version_id.as_deref(), Some("A"), "a namespace refresh is not our acknowledgement");
        let fh = reopened.open(ino, true).await.unwrap();
        reopened.write(fh, 0, Bytes::from_static(b"R")).await.unwrap();
        assert_eq!(reopened.read(fh, 0, 5).await.unwrap(), b"Rirst"[..]);
        store.with(|c| {
            let tx = c.transaction()?;
            let mut n = node(&tx, "drive", ino)?.unwrap();
            n.entry.version_id = Some("D".into());
            n.entry.etag = Some("etag-D".into());
            tx.execute("UPDATE entries SET state='done',version='D' WHERE id=?1", [n.entry_id.unwrap()])?;
            save_node(&tx, &n).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            tx.commit()
        }).unwrap();
        drop(reopened);
        let reopened = Session::new_writable_with_config(store, client, cache, queue.clone(), "drive", conn, cfg).await.unwrap();
        let file = reopened.data.file(ino).unwrap();
        assert_eq!(file.state.lock().await.record.base.version_id.as_deref(), Some("A"), "byte gaps retain their original reader base");
        assert_eq!(file.guard.lock().unwrap().0.version_id.as_deref(), Some("D"), "the durable acknowledged version becomes the write guard");
        let fh = reopened.open(ino, true).await.unwrap();
        reopened.write(fh, 1, Bytes::from_static(b"E")).await.unwrap();
        assert_eq!(reopened.read(fh, 0, 5).await.unwrap(), b"RErst"[..]);
        reopened.close(fh).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dirty_bytes_racing_a_registered_timer_are_queued_after_a_delayed_recheck() {
        for (successful, advanced) in [(true, true), (false, true), (false, false)] {
            let quiet = Duration::from_millis(50);
            let (_dir, session, queue, fh) = fixture_with_quiet(Some(quiet)).await;
            let ino = session.handle_attr(fh).unwrap().ino;
            let file = session.data.file(ino).unwrap();
            session.fsync(fh).await.unwrap();
            let before = queue.status().await.unwrap().items.len();
            let previous = file.state.lock().await.record.revision;
            session.write(fh, 0, Bytes::from_static(b"later")).await.unwrap();
            assert!(file.timer.load(Ordering::Acquire), "the old timer suppresses both write-side scheduling calls");
            let held = file.state.clone().lock_owned().await;
            let attempted = if advanced { previous } else { held.record.revision };
            let (data, view) = (session.data.clone(), file.clone());
            let finishing = tokio::spawn(async move { data.finish_timer(ino, &view, attempted, successful).await });
            tokio::time::timeout(Duration::from_secs(2), async {
                while file.timer.load(Ordering::Acquire) { tokio::task::yield_now().await; }
            }).await.unwrap();
            tokio::time::sleep(quiet * 3).await;
            assert_eq!(queue.status().await.unwrap().items.len(), before);
            drop(held);
            finishing.await.unwrap();
            if successful || advanced {
                tokio::time::timeout(Duration::from_secs(3), async {
                    while queue.status().await.unwrap().items.len() == before { tokio::time::sleep(Duration::from_millis(5)).await; }
                }).await.expect("the dirty revision still needs a timer after its quiet period has elapsed");
                let entries = queue.store().with(|c| crate::journal::all(c)).unwrap();
                let latest = entries.last().unwrap();
                assert_eq!(latest.op, Op::Put);
                assert_eq!(std::fs::read(latest.source.as_ref().unwrap()).unwrap(), b"later");
            } else {
                tokio::time::sleep(quiet * 2).await;
                assert_eq!(queue.status().await.unwrap().items.len(), before, "an unchanged failed revision does not retry forever");
                assert!(!file.timer.load(Ordering::Acquire));
            }
            session.close(fh).await.unwrap();
            queue.close().await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelled_close_releases_its_flag_and_allows_a_later_write_and_close() {
        let (_dir, session, queue, fh) = fixture().await;
        let handle = session.handle(fh).unwrap();
        let file = session.data.file(handle.attr.ino).unwrap();
        let held = file.state.clone().lock_owned().await;
        let ns = session.clone();
        let closing = tokio::spawn(async move { ns.close(fh).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !handle.closed.load(Ordering::Acquire) { tokio::task::yield_now().await; }
        }).await.unwrap();
        closing.abort();
        assert!(closing.await.unwrap_err().is_cancelled());
        assert!(!handle.closed.load(Ordering::Acquire));
        drop(held);
        tokio::time::timeout(Duration::from_secs(5), session.write(fh, 0, Bytes::from_static(b"later"))).await.unwrap().unwrap();
        assert_eq!(session.read(fh, 0, 5).await.unwrap(), b"later"[..]);
        session.close(fh).await.unwrap();
        assert_eq!(session.handle_attr(fh), Err(FsError::BadHandle));
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelled_fsync_holds_the_inode_lock_until_its_metadata_commit_finishes() {
        let (_dir, session, queue, fh) = fixture().await;
        let ino = session.handle_attr(fh).unwrap().ino;
        let file = session.data.file(ino).unwrap();
        let (store, client, cache, conn) = (session.store.clone(), session.client.clone(), session.cache.clone(), session.connectivity.clone());
        let cfg = StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() };
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocked_store = store.clone();
        let blocker = tokio::task::spawn_blocking(move || blocked_store.with(|_| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.await.unwrap();
        let ns = session.clone();
        let syncing = tokio::spawn(async move { ns.fsync(fh).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while file.state.try_lock().is_ok() { tokio::task::yield_now().await; }
        }).await.unwrap();
        syncing.abort();
        assert!(syncing.await.unwrap_err().is_cancelled());
        assert!(file.state.try_lock().is_err(), "the detached flush still owns the state until commit");
        drop(session);
        let blocked = tokio::time::timeout(Duration::from_secs(2), Session::new_writable_with_config(store.clone(), client.clone(), cache.clone(), queue.clone(),
            "drive", conn.clone(), cfg.clone())).await.expect("the owned flush rejects a writer before it can wait on SQLite");
        assert!(matches!(blocked, Err(FsError::Again)), "an in-flight flush retains the writer lease after the original Session is dropped");
        release_tx.send(()).unwrap();
        blocker.await.unwrap().unwrap();
        let reopened = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match Session::new_writable_with_config(store.clone(), client.clone(), cache.clone(), queue.clone(), "drive", conn.clone(), cfg.clone()).await {
                    Ok(session) => break session,
                    Err(FsError::Again) => tokio::task::yield_now().await,
                    Err(error) => panic!("{error}"),
                }
            }
        }).await.expect("the writer lease is released after the flush finishes");
        let loaded = stage::load_all(&store, "drive").unwrap();
        assert_eq!(loaded[&ino].record.revision, 1);
        assert_eq!(loaded[&ino].record.flushed, 1);
        assert_eq!(reopened.handle_attr(fh), Err(FsError::Stale));
        let fh = reopened.open(ino, true).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), reopened.write(fh, 0, Bytes::from_static(b"later"))).await.unwrap().unwrap();
        let loaded = stage::load_all(&store, "drive").unwrap();
        assert_eq!(loaded[&ino].record.revision, 2);
        assert_eq!(loaded[&ino].record.flushed, 1);
        assert_eq!(reopened.read(fh, 0, 5).await.unwrap(), b"later"[..]);
        reopened.fsync(fh).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_refused_flush_commit_removes_the_copies_it_froze() {
        let (dir, session, queue, fh) = fixture().await;
        let journal = || std::fs::read_dir(dir.path().join("journal")).unwrap().count();
        assert_eq!(journal(), 0);
        session.store.with(|c| c.execute_batch("CREATE TRIGGER refuse_flush BEFORE INSERT ON entries BEGIN SELECT RAISE(ABORT, 'test failure'); END")).unwrap();
        assert!(matches!(session.fsync(fh).await, Err(FsError::Io(_))));
        assert_eq!(journal(), 0, "a copy frozen for a commit that failed belongs to no entry");
        session.store.with(|c| c.execute_batch("DROP TRIGGER refuse_flush")).unwrap();
        session.fsync(fh).await.unwrap();
        assert_eq!(journal(), 1);
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_writable_session_per_drive_prevents_stale_extent_maps_and_releases_on_drop() {
        let (_dir, session, queue, fh) = fixture().await;
        let (store, client, cache, conn, generation) = (session.store.clone(), session.client.clone(), session.cache.clone(), session.connectivity.clone(), session.generation());
        let cfg = StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() };
        assert!(matches!(Session::new_writable_with_config(store.clone(), client.clone(), cache.clone(), queue.clone(), "drive", conn.clone(), cfg.clone()).await, Err(FsError::Again)));
        assert!(matches!(Session::new(store.clone(), client.clone(), cache.clone(), "drive", conn.clone()).await, Err(FsError::Again)), "a separate read-only alias would miss this writer's local extents");
        assert_eq!(store.meta("mount_session").unwrap().unwrap(), generation.to_string(), "a rejected writer does not invalidate the active namespace");
        let ino = session.handle_attr(fh).unwrap().ino;
        drop(session);
        let reader = Session::new(store.clone(), client.clone(), cache.clone(), "drive", conn.clone()).await.unwrap();
        assert!(matches!(Session::new_writable_with_config(store.clone(), client.clone(), cache.clone(), queue.clone(), "drive", conn.clone(), cfg.clone()).await, Err(FsError::Again)));
        drop(reader);
        let reopened = Session::new_writable_with_config(store, client, cache, queue.clone(), "drive", conn, cfg).await.unwrap();
        assert_eq!(reopened.handle_attr(fh), Err(FsError::Stale));
        let fh = reopened.open(ino, true).await.unwrap();
        assert_eq!(reopened.read(fh, 0, 5).await.unwrap(), b"first"[..]);
        reopened.write(fh, 2, Bytes::from_static(b"new")).await.unwrap();
        assert_eq!(reopened.read(fh, 0, 5).await.unwrap(), b"finew"[..]);
        queue.close().await;
    }
}
