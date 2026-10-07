// SPDX-License-Identifier: Apache-2.0
//! One filesystem core and change watcher per drive, shared by mounts and RPC sessions.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use axum::http::StatusCode;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};
use voidfs_client::{FeedWatch, Invalidation, Remembered, mount::Session};

use crate::api;
use crate::mounts::Mounted;
use crate::server::{Failure, Shared};

const MAX_DRIVES: usize = 32;
const MAX_ALIASES: usize = 4096;
const MAX_MEMO: usize = 1024 * 1024;
const EVENT_BYTES: usize = 64 * 1024;
const EVENT_INVALIDATIONS: usize = 1024;
const EVENT_INODES: usize = 4096;

#[derive(Clone, Deserialize, Serialize)]
struct Identity { id: String, alias: String, #[serde(default)] used: u64 }

#[derive(Default)]
struct Registry {
    drives: HashMap<String, Weak<DriveSession>>,
    aliases: HashMap<String, Identity>,
    loaded: bool,
    closed: bool,
}

pub(crate) struct Sessions {
    state: Mutex<Registry>,
    creating: Arc<tokio::sync::Mutex<()>>,
}

impl Sessions {
    pub(crate) fn new() -> Self { Self { state: Mutex::new(Registry::default()), creating: Arc::default() } }

    pub(crate) async fn drive(&self, s: &Shared, name: &str) -> Result<Arc<DriveSession>, Failure> {
        self.open(s, name, true, None).await
    }

    pub(crate) async fn drive_for_rpc(&self, s: &Shared, name: &str) -> Result<Arc<DriveSession>, Failure> {
        self.open(s, name, false, None).await
    }

    pub(crate) async fn remembered(&self, s: &Shared, mount: &Remembered) -> Result<Arc<DriveSession>, Failure> {
        let id = mount.drive_id.as_deref().ok_or_else(|| Failure::new(StatusCode::CONFLICT, "UnknownRememberedDrive", "the remembered drive has no stable identity; mount it again explicitly"))?;
        let known = Identity { id: id.into(), alias: mount.drive.clone(), used: 0 };
        self.open(s, id, false, Some(known)).await
    }

    async fn open(&self, s: &Shared, name: &str, provisional: bool, known: Option<Identity>) -> Result<Arc<DriveSession>, Failure> {
        let mut closing = s.closing.subscribe();
        let mut creation = bootstrap(&mut closing, self.creating.clone().lock_owned()).await?;
        if self.state.lock().unwrap_or_else(|p| p.into_inner()).closed {
            return Err(stopping());
        }
        if name.is_empty() || name.len() > 1024 {
            return Err(Failure::new(StatusCode::BAD_REQUEST, "InvalidArgument", "invalid drive name"));
        }
        let load = !self.state.lock().unwrap_or_else(|p| p.into_inner()).loaded;
        if load {
            let dir = s.store.dir().to_owned();
            let aliases = tokio::task::spawn_blocking(move || load_aliases(&dir)).await.map_err(voidfs_client::Error::from)??;
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.aliases = aliases;
            state.loaded = true;
        }
        let memo = self.state.lock().unwrap_or_else(|p| p.into_inner()).aliases.get(name).cloned();
        let query = known.as_ref().map(|identity| identity.id.as_str()).unwrap_or(name);
        // Resolve online every time: an alias can be deleted and then reused for another drive.
        let (identity, pinned) = match bootstrap(&mut closing, s.client.describe_drive(query)).await? {
            Ok(info) => {
                let identity = Identity { id: info.drive_id, alias: info.alias, used: 0 };
                if identity.id.is_empty() || identity.id.len() > 1024 || identity.alias.is_empty() || identity.alias.len() > 1024 {
                    return Err(Failure::new(StatusCode::BAD_GATEWAY, "InvalidResponse", "invalid drive identity"));
                }
                if known.as_ref().is_some_and(|known| known.id != identity.id) {
                    return Err(Failure::new(StatusCode::BAD_GATEWAY, "InvalidResponse", "the service returned a different stable drive identity"));
                }
                let aliases = {
                    let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    remember_identity(&mut state.aliases, identity.clone());
                    state.aliases.clone()
                };
                let dir = s.store.dir().to_owned();
                // Keep creation serialized even if the request is cancelled during fsync.
                let (guard, saved) = tokio::task::spawn_blocking(move || (creation, save_aliases(&dir, &aliases))).await.map_err(voidfs_client::Error::from)?;
                creation = guard;
                saved?;
                (identity, true)
            }
            Err(e) if e.status().is_none() => match known.or(memo) {
                Some(identity) => (identity, true),
                None if provisional => (Identity { id: name.into(), alias: name.into(), used: 0 }, false),
                None => return Err(voidfs_client::mount::FsError::Offline.into()),
            },
            Err(e) => {
                if e.status() == Some(404) {
                    let aliases = {
                        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                        forget_identity(&mut state.aliases, query);
                        state.aliases.clone()
                    };
                    let dir = s.store.dir().to_owned();
                    let (_creation, saved) = tokio::task::spawn_blocking(move || (creation, save_aliases(&dir, &aliases))).await.map_err(voidfs_client::Error::from)?;
                    saved?;
                }
                return Err(e.into());
            }
        };
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.drives.retain(|_, d| d.strong_count() > 0);
            if let Some(drive) = state.drives.get(&identity.id).and_then(Weak::upgrade)
                && (drive.pinned || !pinned)
            {
                *drive.drive.lock().unwrap_or_else(|p| p.into_inner()) = identity.alias;
                return Ok(drive);
            }
            if state.drives.len() >= MAX_DRIVES {
                return Err(Failure::new(StatusCode::SERVICE_UNAVAILABLE, "SessionLimit", "too many active filesystem drives"));
            }
        }
        // A root listing supplies the feed's starting position. Offline bootstrap starts at zero.
        let seq = match bootstrap(&mut closing, s.client.list_folder_page(&identity.id, "", None)).await? {
            Ok(page) => page.seq,
            Err(e) if e.status().is_none() => 0,
            Err(e) => return Err(e.into()),
        };
        let core = Arc::new(if pinned {
            Session::new_writable(s.store.clone(), s.client.clone(), s.cache.clone(), s.queue.clone(), &identity.id, s.connectivity().clone()).await?
        } else {
            Session::new(s.store.clone(), s.client.clone(), s.cache.clone(), &identity.id, s.connectivity().clone()).await?
        });
        if *closing.borrow() { return Err(stopping()); }
        let (events, _) = broadcast::channel(64);
        let drive = Arc::new(DriveSession {
            core, drive: Mutex::new(identity.alias), id: identity.id.clone(), pinned,
            state: Mutex::new(FeedState { seq, events: 0, last: None, generation: 1, namespace_generation: 1 }),
            publish: Mutex::new(()), observers: Mutex::new(Vec::new()), events, task: Mutex::new(None),
        });
        let local = Arc::downgrade(&drive);
        drive.core.set_local_observer(Arc::new(move |change| {
            if let Some(drive) = local.upgrade() { drive.local(change); }
        }));
        let mut watch = FeedWatch::start_with_resync(s.client.clone(), &identity.id, seq, Some(s.connectivity().clone()));
        let weak = Arc::downgrade(&drive);
        let task = tokio::spawn(async move {
            while let Some(event) = watch.next().await {
                let mut invalidations = event.invalidations;
                loop {
                    let Some(drive) = weak.upgrade() else { return };
                    match drive.core.invalidate(&invalidations).await {
                        Ok(inodes) => {
                            drive.remote(event.seq, invalidations, inodes);
                            break;
                        }
                        Err(_) => {
                            // No observer may see an event before its metadata invalidation commits.
                            invalidations = vec![Invalidation::All];
                            drop(drive);
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                    }
                }
            }
        });
        *drive.task.lock().unwrap_or_else(|p| p.into_inner()) = Some(task);
        self.state.lock().unwrap_or_else(|p| p.into_inner()).drives.insert(identity.id, Arc::downgrade(&drive));
        drop(creation);
        Ok(drive)
    }

    pub(crate) async fn shutdown(&self) {
        let _creation = self.creating.lock().await;
        let drives = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.closed = true;
            state.drives.drain().filter_map(|(_, d)| d.upgrade()).collect::<Vec<_>>()
        };
        for drive in drives {
            let task = drive.task.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(task) = task { task.abort(); let _ = task.await; }
        }
    }
}

fn stopping() -> Failure { Failure::new(StatusCode::SERVICE_UNAVAILABLE, "Stopping", "the daemon is stopping") }

/// Only creation admission and read-only network requests use this cancellation boundary.
async fn bootstrap<T>(closing: &mut watch::Receiver<bool>, pending: impl Future<Output = T>) -> Result<T, Failure> {
    if *closing.borrow() { return Err(stopping()); }
    tokio::select! {
        biased;
        _ = closing.wait_for(|closed| *closed) => Err(stopping()),
        result = pending => Ok(result),
    }
}

struct FeedState { seq: u64, events: u64, last: Option<String>, generation: u64, namespace_generation: u64 }

pub(crate) struct DriveSession {
    pub(crate) core: Arc<Session>,
    drive: Mutex<String>,
    pub(crate) id: String,
    /// The drive was resolved to an ID online or from the durable alias memo.
    pub(crate) pinned: bool,
    state: Mutex<FeedState>,
    publish: Mutex<()>,
    observers: Mutex<Vec<Weak<dyn Mounted>>>,
    events: broadcast::Sender<api::fs::InvalidationEvent>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for DriveSession {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().unwrap_or_else(|p| p.into_inner()).take() { task.abort(); }
    }
}

impl DriveSession {
    pub(crate) fn drive(&self) -> String { self.drive.lock().unwrap_or_else(|p| p.into_inner()).clone() }

    pub(crate) fn feed(&self) -> api::Feed {
        let f = self.state.lock().unwrap_or_else(|p| p.into_inner());
        api::Feed { seq: f.seq, events: f.events, last_event: f.last.clone() }
    }

    pub(crate) fn metadata_generation(&self) -> u64 { self.state.lock().unwrap_or_else(|p| p.into_inner()).generation }

    pub(crate) fn namespace_generation(&self) -> u64 { self.state.lock().unwrap_or_else(|p| p.into_inner()).namespace_generation }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<api::fs::InvalidationEvent> { self.events.subscribe() }

    pub(crate) fn event(&self, resync: bool) -> api::fs::InvalidationEvent {
        let f = self.state.lock().unwrap_or_else(|p| p.into_inner());
        api::fs::InvalidationEvent { generation: f.generation, seq: f.seq, resync, invalidations: vec![api::fs::Invalidation::All], inodes: Vec::new() }
    }

    pub(crate) fn observe(&self, handle: Arc<dyn Mounted>) {
        let _publish = self.publish.lock().unwrap_or_else(|p| p.into_inner());
        {
            let mut observers = self.observers.lock().unwrap_or_else(|p| p.into_inner());
            observers.retain(|h| h.strong_count() > 0);
            observers.push(Arc::downgrade(&handle));
        }
        // Covers changes that arrived while the adapter was mounting.
        handle.invalidate(&[Invalidation::All]);
    }

    fn local(&self, change: voidfs_client::mount::LocalChange) { self.notify(None, change.invalidations, change.inodes, change.namespace); }

    fn remote(&self, seq: u64, invalidations: Vec<Invalidation>, inodes: Vec<u64>) { self.notify(Some(seq), invalidations, inodes, true); }

    fn notify(&self, seq: Option<u64>, invalidations: Vec<Invalidation>, inodes: Vec<u64>, namespace: bool) {
        let _publish = self.publish.lock().unwrap_or_else(|p| p.into_inner());
        let local = seq.is_none().then(|| voidfs_client::mount::LocalChange { invalidations: invalidations.clone(), inodes: inodes.clone(), namespace });
        let mut event = {
            let mut f = self.state.lock().unwrap_or_else(|p| p.into_inner());
            f.generation = f.generation.saturating_add(1);
            if namespace { f.namespace_generation = f.namespace_generation.saturating_add(1); }
            if let Some(seq) = seq {
                f.seq = f.seq.max(seq);
                f.events = f.events.saturating_add(1);
                f.last = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
            }
            api::fs::InvalidationEvent { generation: f.generation, seq: f.seq, resync: invalidations.contains(&Invalidation::All),
                invalidations: invalidations.iter().cloned().map(Into::into).collect(), inodes }
        };
        if event.invalidations.len() > EVENT_INVALIDATIONS || event.inodes.len() > EVENT_INODES
            || serde_json::to_vec(&event).is_ok_and(|bytes| bytes.len() > EVENT_BYTES)
        {
            event.resync = true;
            event.invalidations = vec![api::fs::Invalidation::All];
            event.inodes.clear();
        }
        let observers = {
            let mut observers = self.observers.lock().unwrap_or_else(|p| p.into_inner());
            let live = observers.iter().filter_map(Weak::upgrade).collect::<Vec<_>>();
            observers.retain(|h| h.strong_count() > 0);
            live
        };
        for handle in observers {
            if let Some(change) = &local { handle.local_change(change); }
            else { handle.invalidate(&invalidations); }
        }
        let _ = self.events.send(event);
    }
}

fn remember_identity(aliases: &mut HashMap<String, Identity>, mut identity: Identity) {
    identity.used = aliases.values().map(|entry| entry.used).max().unwrap_or(0).saturating_add(1);
    aliases.retain(|name, entry| entry.id != identity.id && name != &identity.alias && name != &identity.id);
    aliases.insert(identity.id.clone(), identity.clone());
    aliases.insert(identity.alias.clone(), identity.clone());
    while aliases.len() > MAX_ALIASES || serde_json::to_vec(aliases).expect("JSON").len() > MAX_MEMO {
        let oldest = aliases.values().filter(|entry| entry.id != identity.id).min_by(|left, right| (left.used, &left.id).cmp(&(right.used, &right.id))).map(|entry| entry.id.clone());
        let Some(oldest) = oldest else { break };
        aliases.retain(|_, entry| entry.id != oldest);
    }
}

fn forget_identity(aliases: &mut HashMap<String, Identity>, name: &str) {
    let Some(identity) = aliases.get(name).cloned() else { return };
    if name == identity.id { aliases.retain(|_, entry| entry.id != identity.id); }
    else { aliases.remove(name); }
}

fn load_aliases(dir: &Path) -> voidfs_client::Result<HashMap<String, Identity>> {
    let mut file = match std::fs::File::open(dir.join("drive-aliases.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(MAX_MEMO as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_MEMO { return Err(std::io::Error::other("drive alias memo is too large").into()); }
    let mut aliases: HashMap<String, Identity> = serde_json::from_slice(&bytes).map_err(|e| std::io::Error::other(format!("invalid drive alias memo: {e}")))?;
    if aliases.len() > MAX_ALIASES || aliases.iter().any(|(name, identity)| name.is_empty() || name.len() > 1024 || identity.id.is_empty()
        || identity.id.len() > 1024 || identity.alias.is_empty() || identity.alias.len() > 1024)
    { return Err(std::io::Error::other("invalid drive alias memo identity").into()); }
    let identities = aliases.iter().filter(|(name, identity)| *name == &identity.id).map(|(_, identity)| (identity.id.clone(), identity.clone())).collect::<HashMap<_, _>>();
    aliases.retain(|name, identity| identities.get(&identity.id).is_some_and(|current| name == &identity.id || name == &current.alias));
    for identity in aliases.values_mut() {
        if let Some(current) = identities.get(&identity.id) { *identity = current.clone(); }
    }
    Ok(aliases)
}

fn save_aliases(dir: &Path, aliases: &HashMap<String, Identity>) -> voidfs_client::Result<()> {
    let bytes = serde_json::to_vec(aliases).expect("JSON");
    if bytes.len() > MAX_MEMO { return Err(std::io::Error::other("drive alias memo is too large").into()); }
    let temporary = dir.join("drive-aliases.json.tmp");
    let saved = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, dir.join("drive-aliases.json"))?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if saved.is_err() { let _ = std::fs::remove_file(temporary); }
    Ok(saved?)
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn identity(id: &str, alias: &str) -> Identity { Identity { id: id.into(), alias: alias.into(), used: 0 } }

    #[test]
    fn refreshed_identity_removes_old_aliases_and_load_discards_legacy_stale_names() {
        let mut aliases = HashMap::new();
        remember_identity(&mut aliases, identity("id-a", "old"));
        remember_identity(&mut aliases, identity("id-a", "renamed"));
        assert!(!aliases.contains_key("old"));
        assert_eq!(aliases["renamed"].id, "id-a");
        // An older daemon left this alias beside its current stable-ID mapping.
        aliases.insert("old".into(), identity("id-a", "old"));
        let dir = tempfile::tempdir().unwrap();
        save_aliases(dir.path(), &aliases).unwrap();
        let loaded = load_aliases(dir.path()).unwrap();
        assert!(!loaded.contains_key("old"), "legacy aliases contradicting the canonical stable-ID record are discarded");
        assert_eq!(loaded["id-a"].alias, "renamed");
    }

    #[test]
    fn memo_capacity_evicts_one_old_identity_and_keeps_the_other_recent_identities() {
        let mut aliases = HashMap::new();
        for index in 0..MAX_ALIASES / 2 {
            let entry = Identity { id: format!("id-{index:04}"), alias: format!("alias-{index:04}"), used: index as u64 };
            aliases.insert(entry.id.clone(), entry.clone());
            aliases.insert(entry.alias.clone(), entry);
        }
        remember_identity(&mut aliases, identity("id-new", "alias-new"));
        assert_eq!(aliases.len(), MAX_ALIASES);
        assert!(!aliases.contains_key("id-0000"));
        assert!(!aliases.contains_key("alias-0000"));
        assert_eq!(aliases["alias-0001"].id, "id-0001");
        assert_eq!(aliases["alias-new"].id, "id-new");
        let long = "x".repeat(1000);
        let mut aliases = HashMap::new();
        for index in 0..250 {
            let entry = Identity { id: format!("{index}{long}"), alias: format!("name-{index}{long}"), used: index };
            aliases.insert(entry.id.clone(), entry.clone());
            aliases.insert(entry.alias.clone(), entry);
        }
        remember_identity(&mut aliases, identity("id-new", "alias-new"));
        assert!(serde_json::to_vec(&aliases).unwrap().len() <= MAX_MEMO);
        assert!(aliases.len() > 2, "byte pressure evicts individual old identities instead of clearing the memo");
        assert!(aliases.contains_key("id-new"));
    }

    #[test]
    fn a_known_missing_name_and_deleted_identity_stay_removed_after_reopen() {
        let mut aliases = HashMap::new();
        remember_identity(&mut aliases, identity("id-a", "old"));
        forget_identity(&mut aliases, "old");
        assert!(aliases.contains_key("id-a"));
        let dir = tempfile::tempdir().unwrap();
        save_aliases(dir.path(), &aliases).unwrap();
        let mut aliases = load_aliases(dir.path()).unwrap();
        assert!(!aliases.contains_key("old"));
        remember_identity(&mut aliases, identity("id-a", "renamed"));
        forget_identity(&mut aliases, "id-a");
        save_aliases(dir.path(), &aliases).unwrap();
        assert!(load_aliases(dir.path()).unwrap().is_empty());
    }
}
