// SPDX-License-Identifier: Apache-2.0
//! Process kill points. A child process runs a scenario until a named point sends it `SIGKILL`;
//! the test then reopens the state directory and checks what survived: acknowledged bytes and
//! names, one publication of each change, and no bytes that nothing records.

use super::*;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};

use crate::{ApiFetcher, CacheConfig, Queue, QueueConfig, Scope};

const CHILD: &str = "mount::crash_tests::child";
const DRIVE: &str = "drive";

fn plenty(_: &Path) -> std::io::Result<u64> { Ok(1 << 50) }

fn client(endpoint: &str) -> Client {
    Client::new(voidfs_sdk::Config { endpoint: endpoint.to_owned(), access_key_id: voidfs_server::test_server::ADMIN_KEY_ID.into(),
        secret_access_key: voidfs_server::test_server::ADMIN_SECRET.into(), ..Default::default() }).unwrap()
}

struct Env { client: Client, cache: Cache, queue: Queue, session: Session }

impl Env {
    async fn open(dir: &Path, endpoint: &str) -> Env {
        let client = client(endpoint);
        let store = Arc::new(Store::open(dir).unwrap());
        let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(client.clone())), CacheConfig { min_free_bytes: 0, free_space: Some(plenty), ..Default::default() }).await.unwrap();
        let queue = Queue::open(store.clone(), client.clone(), QueueConfig { retry_max: Duration::from_millis(50), ..Default::default() }).await.unwrap();
        let cfg = StagingConfig { min_free_bytes: 0, free_space: Some(plenty), quiet_period: None, compact_garbage: 0 };
        let session = Session::new_writable_with_config(store, client.clone(), cache.clone(), queue.clone(), DRIVE, Connectivity::default(), cfg).await.unwrap();
        session.recovered().await;
        Env { client, cache, queue, session }
    }

    async fn publish(&self) {
        self.queue.resume(Scope::All).await.unwrap();
        tokio::time::timeout(Duration::from_secs(20), self.queue.settle()).await.unwrap();
        self.queue.pause(Scope::All).await.unwrap();
    }

    async fn file(&self) -> Ino { self.session.lookup(self.session.root(), "file").await.unwrap().ino }

    async fn read(&self, ino: Ino) -> Bytes {
        let fh = self.session.open(ino, false).await.unwrap();
        let bytes = self.session.read(fh, 0, 1 << 20).await.unwrap();
        self.session.close(fh).await.unwrap();
        bytes
    }

    async fn remote(&self) -> (Bytes, usize) {
        let body = self.client.get_object(DRIVE, "file", Default::default()).await.unwrap().body;
        (body, self.client.list_versions(DRIVE, "file", true).await.unwrap().len())
    }

    async fn unfinished(&self) -> Vec<crate::Item> {
        self.queue.status().await.unwrap().items.into_iter().filter(|i| i.state != crate::State::Done).collect()
    }

    async fn close(self) {
        drop(self.session);
        self.queue.close().await;
        self.cache.settle().await;
    }
}

fn files(dir: &Path, sub: &str, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir.join(sub)) else { return Vec::new() };
    let mut files = entries.map(|e| e.unwrap().path()).filter(|p| p.is_file() && p.file_name().unwrap().to_str().unwrap().starts_with(prefix)).collect::<Vec<_>>();
    files.sort();
    files
}

async fn edit_elsewhere(client: &Client, bytes: &'static [u8]) {
    let current = client.head_object(DRIVE, "file", Default::default()).await.unwrap().version_id;
    client.put_object(DRIVE, "file", Bytes::from_static(bytes), voidfs_sdk::PutOptions { if_version: Some(current), ..Default::default() }).await.unwrap();
}

/// What the child does until its kill point fires. Uploads stay paused unless it publishes.
async fn scenario(name: &str, dir: &Path, endpoint: &str) {
    let env = Env::open(dir, endpoint).await;
    env.queue.pause(Scope::All).await.unwrap();
    let ns = &env.session;
    let ino = ns.create(ns.root(), "file", 0o644).await.unwrap().ino;
    let fh = ns.open(ino, true).await.unwrap();
    match name {
        "write" => {
            ns.write(fh, 0, Bytes::from_static(b"first")).await.unwrap();
            ns.write(fh, 5, Bytes::from_static(b"second")).await.unwrap();
        }
        "fsync" => {
            ns.write(fh, 0, Bytes::from_static(b"data")).await.unwrap();
            ns.fsync(fh).await.unwrap();
        }
        "publish" => {
            ns.write(fh, 0, Bytes::from_static(b"data")).await.unwrap();
            // A published entry still held open when the process dies.
            let _reader = ns.open(ino, false).await.unwrap();
            ns.close(fh).await.unwrap();
            env.publish().await;
        }
        "compact" => {
            for n in 0..4u8 { ns.write(fh, 0, Bytes::from(vec![n; 4096])).await.unwrap(); }
            ns.fsync(fh).await.unwrap();
        }
        "conflict" => {
            ns.write(fh, 0, Bytes::from_static(b"one")).await.unwrap();
            ns.close(fh).await.unwrap();
            env.publish().await;
            let fh = ns.open(ino, true).await.unwrap();
            edit_elsewhere(&env.client, b"foreign").await;
            ns.write(fh, 0, Bytes::from_static(b"two")).await.unwrap();
            ns.close(fh).await.unwrap();
            env.publish().await;
        }
        other => panic!("no scenario {other}"),
    }
}

#[test]
fn child() {
    let Ok(name) = std::env::var("VOIDFS_CRASH_SCENARIO") else { return };
    let dir = PathBuf::from(std::env::var("VOIDFS_CRASH_DIR").unwrap());
    let endpoint = std::env::var("VOIDFS_CRASH_ENDPOINT").unwrap();
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap().block_on(scenario(&name, &dir, &endpoint));
    eprintln!("the scenario {name} finished without reaching its kill point");
    std::process::exit(3);
}

/// Runs `scenario` in a child process that is killed at the `after`th hit of `point`.
async fn crashed(scenario: &str, point: &str, after: u32) -> (tempfile::TempDir, voidfs_server::test_server::TestServer) {
    let server = voidfs_server::test_server::TestServer::start().await.unwrap();
    client(&server.endpoint).create_drive(DRIVE, Default::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (exe, path, endpoint) = (std::env::current_exe().unwrap(), dir.path().to_owned(), server.endpoint.clone());
    let (scenario, armed) = (scenario.to_owned(), point.to_owned());
    let status = tokio::task::spawn_blocking(move || std::process::Command::new(exe).args([CHILD, "--exact", "--nocapture", "--test-threads=1"])
        .env("VOIDFS_CRASH_SCENARIO", &scenario).env("VOIDFS_KILL_POINT", &armed).env("VOIDFS_KILL_AFTER", after.to_string())
        .env("VOIDFS_CRASH_DIR", &path).env("VOIDFS_CRASH_ENDPOINT", &endpoint).stdout(std::process::Stdio::null()).status()).await.unwrap().unwrap();
    assert_eq!(status.signal(), Some(9), "the child must die at {point}: {status:?}");
    (dir, server)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_between_appending_and_recording_a_write_keeps_the_acknowledged_bytes() {
    let (dir, server) = crashed("write", "stage.appended", 2).await;
    let env = Env::open(dir.path(), &server.endpoint).await;
    let ino = env.file().await;
    assert_eq!(env.read(ino).await, Bytes::from_static(b"first"), "the unacknowledged write is absent, the earlier one whole");
    env.publish().await;
    assert_eq!(env.remote().await.0, Bytes::from_static(b"first"));
    assert_eq!(env.session.getattr(ino).await.unwrap().sync, Sync::Saved);
    assert!(files(dir.path(), "mount-stage", "").is_empty(), "a saved file keeps no staging bytes");
    env.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_after_recording_a_write_keeps_it_whole() {
    let (dir, server) = crashed("write", "stage.recorded", 2).await;
    let env = Env::open(dir.path(), &server.endpoint).await;
    let ino = env.file().await;
    assert_eq!(env.read(ino).await, Bytes::from_static(b"firstsecond"));
    assert_eq!(env.session.getattr(ino).await.unwrap().size, 11);
    env.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_after_freezing_a_flush_drops_its_copies_and_flushes_again() {
    let (dir, server) = crashed("fsync", "flush.frozen", 1).await;
    let left = files(dir.path(), "journal", "");
    assert_eq!(left.len(), 1, "the crash left the frozen copy");
    let env = Env::open(dir.path(), &server.endpoint).await;
    assert!(!left[0].exists(), "no entry names it");
    assert_eq!(files(dir.path(), "journal", "").len(), 1, "the recovered flush's own copy");
    let ino = env.file().await;
    assert_eq!(env.session.getattr(ino).await.unwrap().sync, Sync::Saving, "opening the writer queued what the crash left unflushed");
    assert_eq!(env.read(ino).await, Bytes::from_static(b"data"));
    env.publish().await;
    assert_eq!(env.remote().await, (Bytes::from_static(b"data"), 2));
    assert_eq!(env.session.getattr(ino).await.unwrap().sync, Sync::Saved);
    env.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_after_a_flush_commits_publishes_its_entries_once() {
    let (dir, server) = crashed("fsync", "flush.committed", 1).await;
    let env = Env::open(dir.path(), &server.endpoint).await;
    let ino = env.file().await;
    assert_eq!(env.unfinished().await.len(), 2, "the create and the flushed bytes are queued");
    env.publish().await;
    assert_eq!(env.remote().await, (Bytes::from_static(b"data"), 2));
    assert_eq!(env.session.getattr(ino).await.unwrap().sync, Sync::Saved);
    env.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_after_a_put_lands_the_restart_knows_it_as_its_own() {
    // The create's put is guarded by absence, the content's by the create's version.
    for after in [1, 2] {
        let (dir, server) = crashed("publish", "publish.sent", after).await;
        let env = Env::open(dir.path(), &server.endpoint).await;
        env.publish().await;
        assert!(env.unfinished().await.is_empty(), "put {after}: {:?}", env.unfinished().await);
        let ino = env.file().await;
        assert_eq!(env.session.getattr(ino).await.unwrap().sync, Sync::Saved, "put {after}");
        assert_eq!(env.remote().await, (Bytes::from_static(b"data"), 2), "put {after} landed once and was not sent again");
        env.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_after_acknowledging_a_put_reconciles_without_sending_it_again() {
    let (dir, server) = crashed("publish", "publish.acknowledged", 3).await;
    let env = Env::open(dir.path(), &server.endpoint).await;
    env.publish().await;
    let ino = env.file().await;
    assert_eq!(env.session.getattr(ino).await.unwrap().sync, Sync::Saved);
    assert_eq!(env.remote().await, (Bytes::from_static(b"data"), 2));
    env.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_after_reconciling_a_publication_held_open_leaves_no_orphans() {
    let (dir, server) = crashed("publish", "reconcile.committed", 2).await;
    assert_eq!(files(dir.path(), "mount-stage", "").len(), 1, "the retired stage an open handle still read");
    assert_eq!(files(dir.path(), "journal", "").len(), 1, "the published entry's source");
    let env = Env::open(dir.path(), &server.endpoint).await;
    assert!(files(dir.path(), "mount-stage", "").is_empty());
    assert!(files(dir.path(), "journal", "").is_empty());
    let ino = env.file().await;
    assert_eq!(env.session.getattr(ino).await.unwrap().sync, Sync::Saved);
    assert_eq!(env.read(ino).await, Bytes::from_static(b"data"), "a new open reads the published version");
    assert_eq!(env.remote().await, (Bytes::from_static(b"data"), 2));
    env.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_while_compacting_keeps_exactly_the_recorded_copy() {
    for point in ["compact.written", "compact.committed"] {
        let (dir, server) = crashed("compact", point, 1).await;
        assert_eq!(files(dir.path(), "mount-stage", "").len(), 2, "{point}: the old and the new file");
        let env = Env::open(dir.path(), &server.endpoint).await;
        let ino = env.file().await;
        let staged = files(dir.path(), "mount-stage", &format!("{ino}-"));
        assert_eq!(staged.len(), 1, "{point}: the copy the record names, compacted by the flush that recovery runs");
        assert_eq!(std::fs::metadata(&staged[0]).unwrap().len(), 4096, "{point}");
        assert_eq!(env.read(ino).await, Bytes::from(vec![3u8; 4096]), "{point}");
        env.publish().await;
        assert_eq!(env.remote().await.0, Bytes::from(vec![3u8; 4096]), "{point}");
        env.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_after_capturing_a_conflict_records_it_once_on_restart() {
    let (dir, server) = crashed("conflict", "conflict.captured", 1).await;
    let left = files(dir.path(), "mount-conflicts", "");
    assert_eq!(left.len(), 2, "captured, never recorded");
    // The kill interrupted a publication, so the reopened queue publishes, and captures, at once.
    let env = Env::open(dir.path(), &server.endpoint).await;
    assert!(left.iter().all(|path| !path.exists()), "no conflict records them");
    env.publish().await;
    let ino = env.file().await;
    assert_eq!(env.session.getattr(ino).await.unwrap().sync, Sync::Conflict);
    assert_eq!(files(dir.path(), "mount-conflicts", "").len(), 2);
    assert_eq!(env.session.read_conflict(ino, ConflictSide::Local, 0, 64).await.unwrap(), Bytes::from_static(b"two"));
    assert_eq!(env.session.read_conflict(ino, ConflictSide::Remote, 0, 64).await.unwrap(), Bytes::from_static(b"foreign"));
    assert_eq!(env.remote().await.0, Bytes::from_static(b"foreign"), "the guard kept the other Mac's version");
    env.close().await;
}
