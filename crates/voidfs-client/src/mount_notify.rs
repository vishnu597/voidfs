// SPDX-License-Identifier: Apache-2.0
//! Accepted local changes notify every adapter sharing a session, including offline edits.

use super::*;
use crate::journal::Entry;

/// A committed local edit. Attribute-only edits do not invalidate directory enumeration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalChange {
    pub invalidations: Vec<Invalidation>,
    pub inodes: Vec<Ino>,
    pub namespace: bool,
}

type Callback = Arc<dyn Fn(LocalChange) + Send + std::marker::Sync>;

#[derive(Default)]
pub(super) struct Observer { callback: Mutex<Option<Callback>> }

impl Observer {
    pub(super) fn send(&self, change: LocalChange) {
        if change.invalidations.is_empty() && change.inodes.is_empty() { return; }
        let callback = self.callback.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(callback) = callback { callback(change); }
    }
}

pub(super) fn attribute(c: &Connection, drive: &str, root: Ino, ino: Ino) -> rusqlite::Result<LocalChange> {
    let path = chain(c, drive, root, ino)?;
    let invalidations = path.map(|path| {
        let mut key = path.into_iter().skip(1).map(|(_, name)| name).collect::<Vec<_>>().join("/");
        if !key.is_empty() && node(c, drive, ino)?.is_some_and(|n| n.entry.kind == Kind::Folder) { key.push('/'); }
        Ok::<_, rusqlite::Error>(vec![Invalidation::Object(key)])
    }).transpose()?.unwrap_or_default();
    Ok(LocalChange { invalidations, inodes: vec![ino], namespace: false })
}

impl Session {
    /// Installs the daemon's shared observer. Callbacks run after operation locks are released;
    /// use a weak session reference if the observer refers back to this session.
    pub fn set_local_observer(&self, observer: Callback) {
        *self.local.callback.lock().unwrap_or_else(|p| p.into_inner()) = Some(observer);
    }

    pub(super) async fn local_transaction<T: Send + 'static>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<((T, LocalChange), Vec<Entry>)> + Send + 'static,
    ) -> Result<T> {
        let queue = self.queue.clone().ok_or(FsError::ReadOnly)?;
        let local = self.local.clone();
        let data = self.data.clone();
        tokio::spawn(async move {
            let (value, change) = queue.mount_transaction(f).await?;
            local.send(change);
            drop(data);
            Ok(value)
        }).await.map_err(Error::from)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    async fn fixture() -> (tempfile::TempDir, Arc<Store>, Arc<Session>, crate::Queue) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let client = Client::new(voidfs_sdk::Config { endpoint: "http://localhost:9".into(), access_key_id: "test".into(), secret_access_key: "test".into(), ..Default::default() }).unwrap();
        let connectivity = Connectivity::default();
        for _ in 0..3 { connectivity.unanswered(); }
        let cache = Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone())), crate::CacheConfig::default()).await.unwrap();
        let queue = crate::Queue::open(store.clone(), client.clone(), crate::QueueConfig { connectivity: Some(connectivity.clone()), ..Default::default() }).await.unwrap();
        queue.pause(crate::Scope::All).await.unwrap();
        let core = Arc::new(Session::new_writable_with_config(store.clone(), client, cache, queue.clone(), "drive", connectivity,
            StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() }).await.unwrap());
        store.with(|c| c.execute("UPDATE mount_dirs SET listed=1, seq=1 WHERE ino=?1", [core.root()])).unwrap();
        (dir, store, core, queue)
    }

    fn recorder(store: Arc<Store>, core: &Arc<Session>) -> Arc<Mutex<Vec<LocalChange>>> {
        let changes = Arc::new(Mutex::new(Vec::new()));
        let observed = changes.clone();
        let weak = Arc::downgrade(core);
        core.set_local_observer(Arc::new(move |change| {
            let core = weak.upgrade().unwrap();
            store.with(|c| c.query_row("SELECT COUNT(*) FROM mount_inodes", [], |r| r.get::<_, u64>(0))).unwrap();
            for ino in &change.inodes {
                if let Some(file) = core.data.file(*ino) { assert!(file.state.try_lock().is_ok(), "callback held staging state"); }
            }
            observed.lock().unwrap().push(change);
        }));
        changes
    }

    fn take(changes: &Mutex<Vec<LocalChange>>) -> LocalChange {
        let mut changes = changes.lock().unwrap();
        assert_eq!(changes.len(), 1, "each accepted mutation announces once");
        changes.pop().unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn offline_local_edits_notify_targeted_objects_after_locks_and_preserve_enumeration() {
        let (_dir, store, core, queue) = fixture().await;
        let changes = recorder(store.clone(), &core);
        let root = core.root();
        let file = core.create(root, "file", 0o644).await.unwrap();
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Object("file".into())], inodes: vec![root, file.ino], namespace: true });
        let fh = core.open(file.ino, true).await.unwrap();
        let generation = store.with(|c| c.query_row("SELECT generation FROM mount_dirs WHERE ino=?1", [root], |r| r.get::<_, u64>(0))).unwrap();
        core.write(fh, 0, Bytes::from_static(b"changed")).await.unwrap();
        let expected = LocalChange { invalidations: vec![Invalidation::Object("file".into())], inodes: vec![file.ino], namespace: false };
        assert_eq!(take(&changes), expected);
        assert_eq!(core.readdir(root, None, 8).await.unwrap()[0].1.size, 7);
        core.truncate(fh, 3).await.unwrap();
        assert_eq!(take(&changes), expected);
        assert_eq!(core.getattr(file.ino).await.unwrap().size, 3);
        core.setxattr(file.ino, "user.label", b"value", XattrMode::Set).await.unwrap();
        assert_eq!(take(&changes), expected);
        core.removexattr(file.ino, "user.label").await.unwrap();
        assert_eq!(take(&changes), expected);
        core.fsync(fh).await.unwrap();
        assert_eq!(take(&changes), expected);
        core.close(fh).await.unwrap();
        assert_eq!(store.with(|c| c.query_row("SELECT generation FROM mount_dirs WHERE ino=?1", [root], |r| r.get::<_, u64>(0))).unwrap(), generation);
        let fh = core.open(file.ino, true).await.unwrap();
        assert_eq!(core.write(fh, 0, Bytes::new()).await.unwrap(), 0);
        core.truncate(fh, 3).await.unwrap();
        core.rename(root, "file", root, "file", RenameMode::Replace).await.unwrap();
        assert_eq!(core.create(root, "file", 0o644).await, Err(FsError::Exists));
        assert_eq!(core.removexattr(file.ino, "missing").await, Err(FsError::NoAttr));
        assert!(changes.lock().unwrap().is_empty());
        core.close(fh).await.unwrap();
        core.rename(root, "file", root, "renamed", RenameMode::Exclusive).await.unwrap();
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Object("file".into()), Invalidation::Object("renamed".into())], inodes: vec![root, file.ino], namespace: true });
        core.unlink(root, "renamed").await.unwrap();
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Object("renamed".into())], inodes: vec![root, file.ino], namespace: true });
        let folder = core.mkdir(root, "folder", 0o755).await.unwrap();
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Subtree("folder/".into())], inodes: vec![root, folder.ino], namespace: true });
        let child = core.create(folder.ino, "child", 0o644).await.unwrap();
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Object("folder/child".into())], inodes: vec![folder.ino, child.ino], namespace: true });
        core.rename(root, "folder", root, "moved", RenameMode::Exclusive).await.unwrap();
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Subtree("folder/".into()), Invalidation::Subtree("moved/".into())], inodes: vec![root, folder.ino], namespace: true });
        core.unlink(folder.ino, "child").await.unwrap();
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Object("moved/child".into())], inodes: vec![folder.ino, child.ino], namespace: true });
        core.rmdir(root, "moved").await.unwrap();
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Subtree("moved/".into())], inodes: vec![root, folder.ino], namespace: true });
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelled_write_still_announces_its_accepted_local_commit() {
        let (_dir, store, core, queue) = fixture().await;
        let ino = core.create(core.root(), "file", 0o644).await.unwrap().ino;
        let fh = core.open(ino, true).await.unwrap();
        core.write(fh, 0, Bytes::from_static(b"before")).await.unwrap();
        let changes = recorder(store.clone(), &core);
        let file = core.data.file(ino).unwrap();
        let (entered, held) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let blocked = tokio::task::spawn_blocking(move || store.with(|_| {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        }).unwrap());
        tokio::task::spawn_blocking(move || held.recv_timeout(Duration::from_secs(5)).unwrap()).await.unwrap();
        let writer = {
            let core = core.clone();
            tokio::spawn(async move { core.write(fh, 0, Bytes::from_static(b"after cancellation")).await })
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while file.state.try_lock().is_ok() { tokio::task::yield_now().await; }
        }).await.unwrap();
        writer.abort();
        assert!(writer.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        blocked.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while changes.lock().unwrap().is_empty() { tokio::task::yield_now().await; }
        }).await.expect("accepted write lost its local notification when the caller was cancelled");
        assert_eq!(take(&changes), LocalChange { invalidations: vec![Invalidation::Object("file".into())], inodes: vec![ino], namespace: false });
        assert_eq!(core.read(fh, 0, 32).await.unwrap(), Bytes::from_static(b"after cancellation"));
        core.close(fh).await.unwrap();
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelled_local_transaction_keeps_the_writer_lease_through_commit_and_notification() {
        let (_dir, store, core, queue) = fixture().await;
        let root = core.root();
        let (client, cache, connectivity) = (core.client.clone(), core.cache.clone(), core.connectivity.clone());
        let (transaction_entered, transaction_wait) = mpsc::sync_channel(1);
        let (commit, commit_wait) = mpsc::sync_channel(1);
        let (callback_entered, callback_wait) = mpsc::sync_channel(1);
        let (finish, finish_wait) = mpsc::sync_channel(1);
        let finish_wait = Mutex::new(finish_wait);
        let observed_store = store.clone();
        core.set_local_observer(Arc::new(move |_| {
            observed_store.with(|c| c.query_row("SELECT generation FROM mount_inodes WHERE ino=?1", [root], |r| r.get::<_, u64>(0))).unwrap();
            callback_entered.send(()).unwrap();
            finish_wait.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
        }));
        let caller = {
            let core = core.clone();
            tokio::spawn(async move {
                core.local_transaction(move |tx| {
                    transaction_entered.send(()).unwrap();
                    commit_wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    tx.execute("UPDATE mount_inodes SET generation=generation+1 WHERE ino=?1", [root])?;
                    Ok((((), LocalChange { invalidations: vec![Invalidation::Object(String::new())], inodes: vec![root], namespace: true }), Vec::new()))
                }).await
            })
        };
        tokio::task::spawn_blocking(move || transaction_wait.recv_timeout(Duration::from_secs(5)).unwrap()).await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        drop(core);
        let blocked_lease = store.mount_session("drive", true).is_none();
        commit.send(()).unwrap();
        tokio::task::spawn_blocking(move || callback_wait.recv_timeout(Duration::from_secs(5)).unwrap()).await.unwrap();
        let result = Session::new_writable(store.clone(), client.clone(), cache.clone(), queue.clone(), "drive", connectivity.clone()).await;
        let blocked_session = matches!(result, Err(FsError::Again));
        drop(result);
        finish.send(()).unwrap();
        assert!(blocked_lease, "cancelled caller released the writer lease before its accepted transaction committed");
        assert!(blocked_session, "a second core opened while the accepted transaction's notification was still running");
        let replacement = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match Session::new_writable(store.clone(), client.clone(), cache.clone(), queue.clone(), "drive", connectivity.clone()).await {
                    Err(FsError::Again) => tokio::task::yield_now().await,
                    result => break result.unwrap(),
                }
            }
        }).await.expect("completed notification retained the writer lease");
        drop(replacement);
        queue.close().await;
    }
}
