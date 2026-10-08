// SPDX-License-Identifier: Apache-2.0
//! A seeded model test: random sequences of namespace changes, writes, truncation, flushes,
//! restarts without closing and publications run against a writable mount session. After every
//! step its view must equal an in-memory filesystem's, and after each publication so must the
//! drive's. `VOIDFS_MODEL_SEED` and `VOIDFS_MODEL_CASES` explore beyond the fixed seed.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use common::client_for;
use proptest::prelude::*;
use proptest::test_runner::{Config as RunnerConfig, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use voidfs_client::mount::{Fh, FsError, Ino, RenameMode, Session, StagingConfig, Sync};
use voidfs_client::{ApiFetcher, Cache, CacheConfig, Connectivity, Queue, QueueConfig, Scope, State, Store};
use voidfs_sdk::{Client, Config, Kind};
use voidfs_server::test_server::TestServer;

const NAMES: [&str; 4] = ["a", "b", "c", "d"];
/// Folders are made at most this deep, so that names collide often.
const DEPTH: usize = 2;

#[derive(Clone, Debug)]
enum Step {
    Create { dir: u8, name: usize },
    Mkdir { dir: u8, name: usize },
    Write { file: u8, offset: u32, len: u32, byte: u8 },
    Truncate { file: u8, size: u32 },
    Rename { from: u8, dir: u8, name: usize },
    Unlink { file: u8 },
    Rmdir { dir: u8 },
    Fsync { file: u8 },
    Close,
    Restart,
    /// Publishes what is queued while handles stay open.
    Upload,
    /// Closes every handle, publishes, and checks the drive.
    Publish,
}

fn step() -> impl Strategy<Value = Step> {
    let name = 0..NAMES.len();
    prop_oneof![
        3 => (any::<u8>(), name.clone()).prop_map(|(dir, name)| Step::Create { dir, name }),
        2 => (any::<u8>(), name.clone()).prop_map(|(dir, name)| Step::Mkdir { dir, name }),
        6 => (any::<u8>(), 0u32..20_000, 1u32..3_000, any::<u8>()).prop_map(|(file, offset, len, byte)| Step::Write { file, offset, len, byte }),
        2 => (any::<u8>(), 0u32..20_000).prop_map(|(file, size)| Step::Truncate { file, size }),
        3 => (any::<u8>(), any::<u8>(), name).prop_map(|(from, dir, name)| Step::Rename { from, dir, name }),
        2 => any::<u8>().prop_map(|file| Step::Unlink { file }),
        1 => any::<u8>().prop_map(|dir| Step::Rmdir { dir }),
        2 => any::<u8>().prop_map(|file| Step::Fsync { file }),
        1 => Just(Step::Close),
        1 => Just(Step::Restart),
        1 => Just(Step::Upload),
        1 => Just(Step::Publish),
    ]
}

/// The in-memory filesystem: names to entries, and each file's bytes by an identity that follows
/// renames as an inode does.
#[derive(Clone, Debug, Default)]
struct Dir(BTreeMap<String, Node>);

#[derive(Clone, Debug)]
enum Node { File(u32), Dir(Dir) }

#[derive(Default)]
struct Model { root: Dir, bytes: HashMap<u32, Vec<u8>>, next: u32 }

type Names = Vec<String>;

impl Model {
    fn walk(&self) -> Vec<(Names, &Node)> {
        fn go<'a>(dir: &'a Dir, at: &Names, out: &mut Vec<(Names, &'a Node)>) {
            for (name, node) in &dir.0 {
                let path = at.iter().cloned().chain([name.clone()]).collect::<Vec<_>>();
                out.push((path.clone(), node));
                if let Node::Dir(d) = node { go(d, &path, out); }
            }
        }
        let mut out = Vec::new();
        go(&self.root, &Vec::new(), &mut out);
        out
    }

    fn dirs(&self) -> Vec<Names> {
        std::iter::once(Vec::new()).chain(self.walk().into_iter().filter(|(_, n)| matches!(n, Node::Dir(_))).map(|(p, _)| p)).collect()
    }

    fn files(&self) -> Vec<(Names, u32)> {
        self.walk().into_iter().filter_map(|(p, n)| match n { Node::File(id) => Some((p, *id)), Node::Dir(_) => None }).collect()
    }

    fn dir(&self, path: &[String]) -> &Dir {
        path.iter().fold(&self.root, |dir, name| match dir.0.get(name) { Some(Node::Dir(d)) => d, _ => panic!("no folder at {path:?}") })
    }

    fn dir_mut(&mut self, path: &[String]) -> &mut Dir {
        path.iter().fold(&mut self.root, |dir, name| match dir.0.get_mut(name) { Some(Node::Dir(d)) => d, _ => panic!("no folder at {path:?}") })
    }

    fn get(&self, path: &[String]) -> Option<&Node> {
        let (last, parent) = path.split_last()?;
        parent.iter().try_fold(&self.root, |dir, name| match dir.0.get(name) { Some(Node::Dir(d)) => Some(d), _ => None })?.0.get(last)
    }
}

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(1 << 50) }

fn check<T: std::fmt::Debug>(got: Result<T, FsError>, expected: Option<FsError>, what: &str) -> Result<Option<T>, TestCaseError> {
    match (got, expected) {
        (Ok(value), None) => Ok(Some(value)),
        (Err(error), Some(expected)) if error == expected => Ok(None),
        (got, expected) => Err(TestCaseError::fail(format!("{what}: got {got:?}, the model expects {expected:?}"))),
    }
}

struct Mount { state: tempfile::TempDir, client: Client, drive: String, open: Option<(Cache, Queue, Session)>, handles: HashMap<u32, Fh>, inodes: HashMap<u32, Ino> }

impl Mount {
    async fn new(client: Client, drive: String) -> Mount {
        let mut mount = Mount { state: tempfile::tempdir().unwrap(), client, drive, open: None, handles: HashMap::new(), inodes: HashMap::new() };
        mount.reopen().await;
        mount
    }

    /// Stops as a crash would leave it, handles open and nothing flushed, then opens again.
    async fn reopen(&mut self) {
        self.handles.clear();
        if let Some((cache, queue, session)) = self.open.take() {
            drop(session);
            queue.close().await;
            cache.settle().await;
        }
        // As a restarted process waits for the old one to exit: an owned flush may still hold it.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let store = loop {
            match Store::open(self.state.path()) {
                Err(voidfs_client::Error::Locked(_)) if std::time::Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(5)).await,
                store => break Arc::new(store.unwrap()),
            }
        };
        let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(self.client.clone())), CacheConfig { min_free_bytes: 0, free_space: Some(plenty), ..Default::default() }).await.unwrap();
        let queue = Queue::open(store.clone(), self.client.clone(), QueueConfig { retry_max: Duration::from_millis(50), ..Default::default() }).await.unwrap();
        queue.pause(Scope::All).await.unwrap();
        // A small compaction threshold, so that overwrites compact during the run.
        let cfg = StagingConfig { min_free_bytes: 0, free_space: Some(plenty), quiet_period: None, compact_garbage: 4096 };
        let session = Session::new_writable_with_config(store, self.client.clone(), cache.clone(), queue.clone(), &self.drive, Connectivity::default(), cfg).await.unwrap();
        self.open = Some((cache, queue, session));
    }

    fn ns(&self) -> &Session { &self.open.as_ref().unwrap().2 }

    fn queue(&self) -> &Queue { &self.open.as_ref().unwrap().1 }

    async fn ino(&self, path: &[String]) -> Ino {
        let mut ino = self.ns().root();
        for name in path { ino = self.ns().lookup(ino, name).await.unwrap().ino; }
        ino
    }

    /// Files to write: those the model reaches, and those unlinked or replaced while a handle
    /// still has them open, whose writes must never reach the drive.
    fn targets(&self, model: &Model) -> Vec<u32> {
        let mut ids = model.files().into_iter().map(|(_, id)| id).collect::<Vec<_>>();
        let mut open = self.handles.keys().filter(|id| !ids.contains(id)).copied().collect::<Vec<_>>();
        open.sort_unstable();
        ids.extend(open);
        ids
    }

    async fn handle(&mut self, id: u32) -> Fh {
        if let Some(fh) = self.handles.get(&id) { return *fh; }
        let fh = self.ns().open(self.inodes[&id], true).await.unwrap();
        self.handles.insert(id, fh);
        fh
    }

    async fn close_all(&mut self) {
        for (_, fh) in std::mem::take(&mut self.handles) { self.ns().close(fh).await.unwrap(); }
    }

    async fn close(mut self) {
        self.close_all().await;
        if let Some((cache, queue, session)) = self.open.take() {
            drop(session);
            queue.close().await;
            cache.settle().await;
        }
    }

    async fn run(&mut self, model: &mut Model, step: &Step) -> Result<(), TestCaseError> {
        match *step {
            Step::Create { dir, name } | Step::Mkdir { dir, name } => {
                let dirs = model.dirs();
                let parent = dirs[dir as usize % dirs.len()].clone();
                let folder = matches!(step, Step::Mkdir { .. });
                if folder && parent.len() >= DEPTH { return Ok(()); }
                let name = NAMES[name];
                let expected = model.dir(&parent).0.contains_key(name).then_some(FsError::Exists);
                let at = self.ino(&parent).await;
                let made = if folder { self.ns().mkdir(at, name, 0o755).await } else { self.ns().create(at, name, 0o644).await };
                if let Some(attr) = check(made, expected, &format!("make {parent:?}/{name}"))? {
                    let node = if folder { Node::Dir(Dir::default()) } else {
                        model.next += 1;
                        model.bytes.insert(model.next, Vec::new());
                        self.inodes.insert(model.next, attr.ino);
                        Node::File(model.next)
                    };
                    model.dir_mut(&parent).0.insert(name.into(), node);
                }
            }
            Step::Write { file, offset, len, byte } => {
                let targets = self.targets(model);
                if targets.is_empty() { return Ok(()); }
                let id = targets[file as usize % targets.len()];
                let fh = self.handle(id).await;
                prop_assert_eq!(self.ns().write(fh, offset.into(), Bytes::from(vec![byte; len as usize])).await, Ok(len as usize));
                let bytes = model.bytes.get_mut(&id).unwrap();
                let end = (offset + len) as usize;
                if bytes.len() < end { bytes.resize(end, 0); }
                bytes[offset as usize..end].fill(byte);
            }
            Step::Truncate { file, size } => {
                let targets = self.targets(model);
                if targets.is_empty() { return Ok(()); }
                let id = targets[file as usize % targets.len()];
                let fh = self.handle(id).await;
                prop_assert_eq!(self.ns().truncate(fh, size.into()).await, Ok(()));
                model.bytes.get_mut(&id).unwrap().resize(size as usize, 0);
            }
            Step::Fsync { file } => {
                let targets = self.targets(model);
                if targets.is_empty() { return Ok(()); }
                if let Some(fh) = self.handles.get(&targets[file as usize % targets.len()]) { prop_assert_eq!(self.ns().fsync(*fh).await, Ok(())); }
            }
            Step::Rename { from, dir, name } => {
                let entries = model.walk().into_iter().map(|(path, _)| path).collect::<Vec<_>>();
                if entries.is_empty() { return Ok(()); }
                let source = entries[from as usize % entries.len()].clone();
                let dirs = model.dirs();
                let parent = dirs[dir as usize % dirs.len()].clone();
                let target = parent.iter().cloned().chain([NAMES[name].to_owned()]).collect::<Vec<_>>();
                let moving = model.get(&source).cloned().unwrap();
                let folder = matches!(moving, Node::Dir(_));
                let expected = match model.get(&target) {
                    _ if source == target => None,
                    Some(Node::File(_)) if folder => Some(FsError::NotDir),
                    Some(Node::Dir(_)) if !folder => Some(FsError::IsDir),
                    _ if folder && parent.starts_with(&source) => Some(FsError::InvalidArgument),
                    Some(Node::Dir(d)) if !d.0.is_empty() => Some(FsError::NotEmpty),
                    _ => None,
                };
                let (from_name, from_parent) = source.split_last().unwrap();
                let (a, b) = (self.ino(from_parent).await, self.ino(&parent).await);
                let renamed = self.ns().rename(a, from_name, b, NAMES[name], RenameMode::Replace).await;
                if check(renamed, expected, &format!("rename {source:?} to {target:?}"))?.is_some() && source != target {
                    model.dir_mut(from_parent).0.remove(from_name);
                    model.dir_mut(&parent).0.insert(NAMES[name].into(), moving);
                }
            }
            Step::Unlink { file } => {
                let files = model.files();
                if files.is_empty() { return Ok(()); }
                let (path, _) = files[file as usize % files.len()].clone();
                let (name, parent) = path.split_last().unwrap();
                let at = self.ino(parent).await;
                prop_assert_eq!(self.ns().unlink(at, name).await, Ok(()));
                model.dir_mut(parent).0.remove(name);
            }
            Step::Rmdir { dir } => {
                let dirs = model.dirs();
                if dirs.len() < 2 { return Ok(()); }
                let path = dirs[1 + dir as usize % (dirs.len() - 1)].clone();
                let (name, parent) = path.split_last().unwrap();
                let expected = (!model.dir(&path).0.is_empty()).then_some(FsError::NotEmpty);
                let at = self.ino(parent).await;
                if check(self.ns().rmdir(at, name).await, expected, &format!("rmdir {path:?}"))?.is_some() {
                    model.dir_mut(parent).0.remove(name);
                }
            }
            Step::Close => self.close_all().await,
            Step::Restart => self.reopen().await,
            Step::Upload => {
                self.ns().recovered().await;
                self.queue().resume(Scope::All).await.unwrap();
                tokio::time::timeout(Duration::from_secs(30), self.queue().settle()).await.map_err(|_| TestCaseError::fail("publication never settled"))?;
                self.queue().pause(Scope::All).await.unwrap();
            }
            Step::Publish => {
                self.close_all().await;
                self.ns().recovered().await;
                self.queue().resume(Scope::All).await.unwrap();
                tokio::time::timeout(Duration::from_secs(30), self.queue().settle()).await.map_err(|_| TestCaseError::fail("publication never settled"))?;
                self.queue().pause(Scope::All).await.unwrap();
                let unfinished = self.queue().status().await.unwrap().items.into_iter().filter(|i| i.state != State::Done).collect::<Vec<_>>();
                prop_assert!(unfinished.is_empty(), "with no other writer every change publishes: {:?}", unfinished);
                self.verify_remote(model).await?;
            }
        }
        Ok(())
    }

    /// The session's view equals the model's.
    async fn verify(&self, model: &Model) -> Result<(), TestCaseError> {
        for dir in model.dirs() {
            let listed = self.ns().readdir(self.ino(&dir).await, None, 1000).await.unwrap();
            let listed = listed.into_iter().map(|(name, attr)| (name, attr.kind)).collect::<Vec<_>>();
            let expected = model.dir(&dir).0.iter().map(|(name, node)| (name.clone(), if matches!(node, Node::Dir(_)) { Kind::Folder } else { Kind::File })).collect::<Vec<_>>();
            prop_assert_eq!(listed, expected, "listing of {:?}", dir);
        }
        for (path, id) in model.files() {
            let (name, parent) = path.split_last().unwrap();
            let attr = self.ns().lookup(self.ino(parent).await, name).await.unwrap();
            prop_assert_eq!(attr.ino, self.inodes[&id], "{:?} keeps its inode", path);
            let bytes = &model.bytes[&id];
            prop_assert_eq!(attr.size, bytes.len() as u64, "size of {:?}", path);
            let fh = self.ns().open(attr.ino, false).await.unwrap();
            let read = self.ns().read(fh, 0, 1 << 20).await.unwrap();
            self.ns().close(fh).await.unwrap();
            prop_assert!(read[..] == bytes[..], "bytes of {:?}", path);
        }
        for (id, fh) in &self.handles {
            let read = self.ns().read(*fh, 0, 1 << 20).await.unwrap();
            prop_assert!(read[..] == model.bytes[id][..], "an open handle of file {} reads its own bytes", id);
        }
        Ok(())
    }

    /// After a publication the drive holds the model, and every name is saved.
    async fn verify_remote(&self, model: &Model) -> Result<(), TestCaseError> {
        let mut remote = BTreeMap::new();
        let mut prefixes = vec![String::new()];
        while let Some(prefix) = prefixes.pop() {
            for entry in self.client.list_folder(&self.drive, &prefix).await.unwrap().entries {
                let key = format!("{prefix}{}", entry.name);
                let body = if entry.kind == Kind::Folder { prefixes.push(key.clone()); None }
                    else { Some(self.client.get_object(&self.drive, &key, Default::default()).await.unwrap().body) };
                remote.insert(key, body);
            }
        }
        let expected = model.walk().into_iter().map(|(path, node)| match node {
            Node::Dir(_) => (format!("{}/", path.join("/")), None),
            Node::File(id) => (path.join("/"), Some(Bytes::from(model.bytes[id].clone()))),
        }).collect::<BTreeMap<_, _>>();
        prop_assert_eq!(remote, expected);
        for (path, _) in model.walk() {
            let (name, parent) = path.split_last().unwrap();
            prop_assert_eq!(self.ns().lookup(self.ino(parent).await, name).await.unwrap().sync, Sync::Saved, "{:?}", path);
        }
        // Everything is published and no handle is open: no local bytes are still needed.
        for dir in ["mount-stage", "journal", "mount-conflicts"] {
            let left = std::fs::read_dir(self.state.path().join(dir)).map(|entries| entries.count()).unwrap_or(0);
            prop_assert_eq!(left, 0, "files left in {}", dir);
        }
        Ok(())
    }
}

#[test]
fn a_mount_session_agrees_with_an_in_memory_filesystem() {
    let seed = std::env::var("VOIDFS_MODEL_SEED").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0x766f_6964);
    let cases = std::env::var("VOIDFS_MODEL_CASES").ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(16);
    let mut key = [0u8; 32];
    key[..8].copy_from_slice(&seed.to_le_bytes());
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    let server = rt.block_on(TestServer::start()).unwrap();
    let client = client_for(&server.endpoint, Config::default());
    let drives = AtomicU32::new(0);
    let config = RunnerConfig { cases, failure_persistence: None, max_shrink_iters: 512, ..RunnerConfig::default() };
    let mut runner = TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &key));
    let result = runner.run(&prop::collection::vec(step(), 1..40), |steps| rt.block_on(async {
        let drive = format!("model{}", drives.fetch_add(1, Ordering::Relaxed));
        client.create_drive(&drive, Default::default()).await.unwrap();
        let mut mount = Mount::new(client.clone(), drive).await;
        let mut model = Model::default();
        for (i, step) in steps.iter().enumerate() {
            let checked = match mount.run(&mut model, step).await { Ok(()) => mount.verify(&model).await, error => error };
            checked.map_err(|e| TestCaseError::fail(format!("step {i}, {step:?}: {e}")))?;
        }
        mount.close().await;
        Ok(())
    }));
    if let Err(error) = result { panic!("seed {seed}: {error}"); }
}
