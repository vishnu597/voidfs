// SPDX-License-Identifier: Apache-2.0
//! Shared local bytes and immutable journal snapshots.

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::journal::{Attrs, Entry, Op, StoredBase};

pub(super) struct File {
    pub state: Arc<tokio::sync::Mutex<stage::Stage>>,
    pub attr: Mutex<Attr>,
    timer: AtomicBool,
    touched: Mutex<tokio::time::Instant>,
}

pub(super) struct Staged {
    store: Arc<Store>, drive: String, root: Ino, queue: Option<crate::Queue>, cfg: StagingConfig,
    files: Mutex<HashMap<Ino, Arc<File>>>, creation: tokio::sync::Mutex<()>,
    normal_commits: AtomicU64, full_commits: AtomicU64,
    _lease: Arc<crate::store::MountLease>,
    local: Arc<notify::Observer>,
}

impl Staged {
    pub async fn load(store: Arc<Store>, drive: String, root: Ino, queue: Option<crate::Queue>, cfg: StagingConfig, lease: Arc<crate::store::MountLease>, local: Arc<notify::Observer>) -> Result<Arc<Self>> {
        let (s, d) = (store.clone(), drive.clone());
        let files = tokio::task::spawn_blocking(move || {
            let stages = stage::load_all(&s, &d)?;
            stages.into_iter().map(|(ino, state)| {
                let attr = s.with(|c| Ok(node(c, &d, ino)))??.ok_or(FsError::Stale)?.attr()?;
                Ok((ino, Arc::new(File { state: Arc::new(tokio::sync::Mutex::new(state)), attr: Mutex::new(attr), timer: AtomicBool::new(false), touched: Mutex::new(tokio::time::Instant::now()) })))
            }).collect::<Result<HashMap<_, _>>>()
        }).await.map_err(Error::from)??;
        Ok(Arc::new(Self { store, drive, root, queue, cfg, files: Mutex::new(files), creation: tokio::sync::Mutex::new(()), normal_commits: AtomicU64::new(0), full_commits: AtomicU64::new(0), _lease: lease, local }))
    }

    pub fn file(&self, ino: Ino) -> Option<Arc<File>> { self.files.lock().unwrap_or_else(|p| p.into_inner()).get(&ino).cloned() }

    async fn ensure(&self, handle: &Handle) -> Result<Arc<File>> {
        let ino = handle.attr.ino;
        if let Some(file) = self.file(ino) { return Ok(file); }
        let _creation = self.creation.lock().await;
        if let Some(file) = self.file(ino) { return Ok(file); }
        let key = handle.reader.lock().await.content().key.clone();
        let (store, attr) = (self.store.clone(), handle.attr.clone());
        let state = tokio::task::spawn_blocking(move || stage::Stage::new(&store, attr, key)).await.map_err(Error::from)??;
        let file = Arc::new(File { attr: Mutex::new(handle.attr.clone()), state: Arc::new(tokio::sync::Mutex::new(state)), timer: AtomicBool::new(false), touched: Mutex::new(tokio::time::Instant::now()) });
        self.files.lock().unwrap_or_else(|p| p.into_inner()).insert(ino, file.clone());
        Ok(file)
    }

    pub fn restart_timers(self: &Arc<Self>) {
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
                let state = file.state.clone().lock_owned().await;
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

    pub async fn flush(self: &Arc<Self>, ino: Ino, file: Arc<File>) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let state = file.state.clone().lock_owned().await;
            this.flush_inner(ino, file, state).await
        }).await.map_err(Error::from)?
    }

    async fn flush_owned(self: &Arc<Self>, ino: Ino, file: Arc<File>, state: tokio::sync::OwnedMutexGuard<stage::Stage>) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move { this.flush_inner(ino, file, state).await }).await.map_err(Error::from)?
    }

    async fn flush_inner(&self, ino: Ino, file: Arc<File>, state: tokio::sync::OwnedMutexGuard<stage::Stage>) -> Result<()> {
        let queue = self.queue.as_ref().ok_or(FsError::ReadOnly)?;
        let state = tokio::task::spawn_blocking(move || { state.sync()?; Ok::<_, FsError>(state) }).await.map_err(Error::from)??;
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
                n.sync = Sync::Saving;
            } else { entries.clear(); }
            tx.execute("UPDATE mount_staged SET record=?2 WHERE ino=?1", params![ino, json])?;
            save_node(tx, &n)?;
            let change = if n.sync != previous { Some(notify::attribute(tx, &drive, root, ino)?) } else { None };
            Ok(((n.attr()?, change), entries))
        }).await;
        let (attr, change) = result?;
        if attr.sync != Sync::Saving { clean(&sources).await; }
        let mut state = state;
        state.record = record;
        *file.attr.lock().unwrap_or_else(|p| p.into_inner()) = attr;
        self.full_commits.fetch_add(1, Ordering::Relaxed);
        drop(state);
        if let Some(change) = change { self.local.send(change); }
        Ok(())
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
    let path = dir.join(format!("snapshot-{}", hex::encode(raw)));
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
        let file = self.data.ensure(&handle).await?;
        let mut state = file.state.clone().lock_owned().await;
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
        let file = self.data.ensure(&handle).await?;
        let mut state = file.state.clone().lock_owned().await;
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
            let file = session.data.ensure(&handle).await.unwrap();
            file.timer.store(true, Ordering::Release);
        }
        session.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
        (dir, session, queue, fh)
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
