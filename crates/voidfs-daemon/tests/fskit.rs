// SPDX-License-Identifier: Apache-2.0
//! The `fskit` adapter, with stand-ins for `mount` and `umount` that keep a mount table in a file,
//! as `mount` prints it: the daemon in this process, asked through `DaemonClient`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use voidfs_daemon::api::{Build, NewMount};
use voidfs_daemon::{ClientError, Daemon, DaemonClient, DaemonConfig, FsKit};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

/// The stand-ins: `mount` without arguments prints `table`; with them, it records them in
/// `calls` and adds the mount, unless `refuse` holds what to say instead. `umount` records its
/// call and removes the line. `busy` refuses a normal unmount (all paths when empty, otherwise
/// only the path it contains); `force-refuse` refuses an explicit forced unmount. Mount-table
/// observations count in `probes`; `probe-error` fails them and `probe-delay` sleeps beyond the
/// presence budget (`delayed-probes` counts these attempts). `probe-gate` holds the first
/// observation after capturing its table until removed (`probe-entered` marks that boundary).
struct Commands {
    dir: PathBuf,
}

impl Commands {
    fn new(dir: &Path) -> Commands {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir.join("commands");
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.display();
        let mount = format!("#!/bin/sh\nif [ $# -eq 0 ]; then\n echo probe >> '{d}/probes'\n if [ -e '{d}/probe-error' ]; then cat '{d}/probe-error' >&2; exit 1; fi\n\
            if [ -e '{d}/probe-delay' ]; then echo delay >> '{d}/delayed-probes'; exec sleep 2; fi\n\
            if [ -e '{d}/probe-gate' ] && mkdir '{d}/probe-claimed' 2>/dev/null; then\n cat '{d}/table' > '{d}/probe-snapshot' 2>/dev/null\n touch '{d}/probe-entered'\n while [ -e '{d}/probe-gate' ]; do sleep 0.01; done\n cat '{d}/probe-snapshot'; exit 0\n fi\n\
            cat '{d}/table' 2>/dev/null; exit 0; fi\necho \"mount $*\" >> '{d}/calls'\nif [ -e '{d}/refuse' ]; then cat '{d}/refuse' >&2; exit 1; fi\n\
            eval \"res=\\${{$(($# - 1))}}\"; eval \"mp=\\${{$#}}\"\necho \"$res on $mp (voidfs, nodev, nosuid, read-only, noowners, noatime, fskit, mounted by test)\" >> '{d}/table'\n");
        let umount = format!("#!/bin/sh\necho \"umount $*\" >> '{d}/calls'\neval \"mp=\\${{$#}}\"\nif [ \"$1\" = '-f' ] && [ -e '{d}/force-refuse' ]; then cat '{d}/force-refuse' >&2; exit 1; fi\n\
            if [ \"$1\" != '-f' ] && [ -e '{d}/busy' ] && {{ [ ! -s '{d}/busy' ] || [ \"$(cat '{d}/busy')\" = \"$mp\" ]; }}; then echo \"umount: unmount($mp): Resource busy\" >&2; exit 1; fi\n\
            grep -v -F \" on $mp (\" '{d}/table' > '{d}/table.new'; mv '{d}/table.new' '{d}/table'\n");
        for (name, body) in [("mount", mount), ("umount", umount)] {
            std::fs::write(dir.join(name), body).unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Commands { dir }
    }

    fn adapter(&self) -> Arc<FsKit> {
        Arc::new(FsKit::new(self.dir.join("mount"), self.dir.join("umount")))
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn table(&self) -> String {
        std::fs::read_to_string(self.dir.join("table")).unwrap_or_default()
    }

    fn set_table(&self, text: &str) {
        let replacement = self.dir.join("external-table");
        std::fs::write(&replacement, text).unwrap();
        std::fs::rename(replacement, self.dir.join("table")).unwrap();
    }

    fn probes(&self) -> usize {
        std::fs::read_to_string(self.dir.join("probes")).unwrap_or_default().lines().count()
    }

    fn set(&self, name: &str, text: Option<&str>) {
        match text {
            Some(t) => std::fs::write(self.dir.join(name), t).unwrap(),
            None => { let _ = std::fs::remove_file(self.dir.join(name)); }
        }
    }
}

fn sdk(server: &TestServer) -> voidfs_sdk::Config {
    voidfs_sdk::Config { endpoint: server.endpoint.replace("127.0.0.1", "localhost"), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..voidfs_sdk::Config::default() }
}

async fn start(server: &TestServer, dir: &Path, commands: &Commands) -> (Daemon, DaemonClient) {
    let cfg = DaemonConfig { adapters: vec![commands.adapter()], ..DaemonConfig::new(dir, sdk(server), Build::default()) };
    let d = Daemon::start(cfg).await.unwrap();
    let c = DaemonClient::new(d.socket());
    (d, c)
}

#[track_caller]
fn refused<T: std::fmt::Debug>(r: Result<T, ClientError>, status: u16, code: &str) -> String {
    match r {
        Err(ClientError::Api { status: s, code: c, message }) if s == status && c == code => message,
        other => panic!("expected {status} {code}, got {other:?}"),
    }
}

async fn until(what: &str, mut ok: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ok().await {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn new_mount(drive: &str, mountpoint: &Path) -> NewMount {
    NewMount { drive: drive.into(), mountpoint: Some(mountpoint.display().to_string()), ..Default::default() }
}

/// The monitor may have already reconciled an out-of-band eject before this explicit cleanup.
fn already_gone(result: Result<voidfs_daemon::api::Unmounted, ClientError>, mountpoint: &Path) {
    match result {
        Ok(value) => assert_eq!(value.unmounted, [mountpoint.display().to_string()]),
        Err(ClientError::Api { status: 404, code, .. }) if code == "NoSuchMount" => {},
        other => panic!("expected an already-gone mount or a successful cleanup, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fskit_mounts_with_the_system_mount_and_comes_back_or_is_adopted_when_the_daemon_starts() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdk").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt/footage");
    let (d, c) = start(&server, dir.path(), &commands).await;

    let m = c.mount(&new_mount("footage", &mp)).await.unwrap();
    assert_eq!((m.adapter.as_str(), m.state.as_str(), m.read_only), ("fskit", "mounted", false), "writable: the module writes");
    let real = std::fs::canonicalize(&mp).unwrap();
    assert_eq!(commands.calls(), [format!("mount -F -t voidfs voidfs://footage {}", real.display())], "the resource names only the drive, at the real path");
    assert!(commands.table().contains(&format!("voidfs://footage on {} (voidfs,", real.display())));
    let view = c.mounts().await.unwrap();
    assert_eq!((view.remembered.len(), view.remembered[0].adapter.as_str(), view.remembered[0].read_only), (1, "fskit", false), "remembered as asked");

    // Stopping unmounts it; starting again mounts it again.
    d.stop().await;
    assert_eq!(commands.calls().last().unwrap(), &format!("umount {}", real.display()));
    assert!(commands.table().is_empty());
    let (d, c) = start(&server, dir.path(), &commands).await;
    until("the remembered mount comes back", async || c.mounts().await.unwrap().mounts.first().is_some_and(|m| m.state == "mounted")).await;
    assert_eq!(commands.calls().iter().filter(|c| c.starts_with("mount ")).count(), 2);

    // A daemon that was killed leaves the drive mounted: the next one adopts it.
    d.stop().await;
    commands.set_table(&format!("voidfs://footage on {} (voidfs, nodev, nosuid, read-only, noowners, noatime, fskit, mounted by test)\n", real.display()));
    let before = commands.calls().len();
    let (d, c) = start(&server, dir.path(), &commands).await;
    until("the mount still there is adopted", async || c.mounts().await.unwrap().mounts.first().is_some_and(|m| m.state == "mounted")).await;
    assert_eq!(commands.calls().len(), before, "no second mount over it");

    // One whose module died answers nothing (here: its top folder can't be listed): the next
    // daemon unmounts it and mounts it anew.
    d.stop().await;
    commands.set_table(&format!("voidfs://footage on {} (voidfs, nodev, nosuid, noowners, noatime, fskit, mounted by test)\n", real.display()));
    let before = commands.calls().len();
    let mode = |m| std::fs::set_permissions(&real, std::os::unix::fs::PermissionsExt::from_mode(m)).unwrap();
    mode(0o000);
    let (d, c) = start(&server, dir.path(), &commands).await;
    until("the dead mount is mounted anew", async || c.mounts().await.unwrap().mounts.first().is_some_and(|m| m.state == "mounted")).await;
    mode(0o755);
    assert_eq!(commands.calls()[before..], [format!("umount -f {}", real.display()), format!("mount -F -t voidfs voidfs://footage {}", real.display())]);
    assert_eq!(commands.table().lines().count(), 1, "{}", commands.table());

    // Busy: it stays mounted, listed and remembered.
    commands.set("busy", Some(""));
    let message = refused(c.unmount("footage").await, 500, "UnmountFailed");
    assert!(message.contains("Resource busy"), "{message}");
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.mounts[0].state.as_str(), view.remembered.len()), (1, "mounted", 1), "{view:?}");
    commands.set("busy", None);
    assert_eq!(c.unmount("footage").await.unwrap().unmounted, [mp.display().to_string()]);
    assert!(commands.table().is_empty());
    let view = c.mounts().await.unwrap();
    assert!(view.mounts.is_empty() && view.remembered.is_empty(), "{view:?}");

    // Ejected in Finder: unmounting it is no error, and runs no umount.
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    commands.set_table("");
    let before = commands.calls().len();
    already_gone(c.unmount(&mp.display().to_string()).await, &mp);
    assert_eq!(commands.calls().len(), before);

    // Asked read-only, `mount -r`.
    let m = c.mount(&NewMount { read_only: true, ..new_mount("footage", &mp) }).await.unwrap();
    assert!(m.read_only);
    assert_eq!(commands.calls().last().unwrap(), &format!("mount -F -t voidfs -r voidfs://footage {}", real.display()));
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_fskit_mount_that_fails_or_finds_something_else_mounted_is_not_remembered() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdk").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;

    commands.set("refuse", Some("mount: Loading resource: Module dev.voidfs.app.fskit is disabled!\n"));
    let message = refused(c.mount(&new_mount("footage", &mp)).await, 500, "MountFailed");
    assert!(message.contains("is disabled!") && message.contains("File System Extensions"), "says what to switch on: {message}");
    commands.set("refuse", Some("mount: Loading resource: Couldn't communicate with a helper application.\nmount: Unable to invoke task\n"));
    let message = refused(c.mount(&new_mount("footage", &mp)).await, 500, "MountFailed");
    assert_eq!(message, "mount: Loading resource: Couldn't communicate with a helper application.; mount: Unable to invoke task");
    commands.set("refuse", None);

    let real = std::fs::canonicalize(&mp).unwrap();
    commands.set_table(&format!("/dev/disk9s1 on {} (apfs, local, journaled)\n", real.display()));
    let message = refused(c.mount(&new_mount("footage", &mp)).await, 500, "MountFailed");
    assert!(message.contains("something else is mounted"), "{message}");
    commands.set_table(&format!("voidfs://other on {} (voidfs, nodev, fskit)\n", real.display()));
    let message = refused(c.mount(&new_mount("footage", &mp)).await, 500, "MountFailed");
    assert!(message.contains("voidfs://other is mounted"), "{message}");
    let view = c.mounts().await.unwrap();
    assert!(view.mounts.is_empty() && view.remembered.is_empty(), "{view:?}");
    assert_eq!(commands.calls().iter().filter(|c| c.starts_with("mount ")).count(), 2, "nothing mounted over another mount");
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn normal_eject_refuses_a_busy_mount_and_explicit_force_forgets_it_only_on_success() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkforce").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    let real = std::fs::canonicalize(&mp).unwrap();
    let before = commands.calls().len();

    commands.set("busy", Some(""));
    let error = refused(c.unmount("footage").await, 500, "UnmountFailed");
    assert!(error.contains("Resource busy"), "{error}");
    assert_eq!(commands.calls()[before..], [format!("umount {}", real.display())], "a normal eject must never retry with force");
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.mounts[0].state.as_str(), view.remembered.len()), (1, "mounted", 1), "{view:?}");
    assert!(commands.table().contains("voidfs://footage"));

    commands.set("force-refuse", Some("umount: injected force failure\n"));
    let error = refused(c.unmount_force("footage").await, 500, "UnmountFailed");
    assert!(error.contains("injected force failure"), "{error}");
    assert_eq!(commands.calls().last(), Some(&format!("umount -f {}", real.display())));
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.mounts[0].state.as_str(), view.remembered.len()), (1, "mounted", 1), "a refused force eject must retain the mount: {view:?}");
    assert!(commands.table().contains("voidfs://footage"));

    commands.set("force-refuse", None);
    assert_eq!(c.unmount_force(&mp.display().to_string()).await.unwrap().unmounted, [mp.display().to_string()]);
    assert_eq!(commands.calls()[before..], [format!("umount {}", real.display()), format!("umount -f {}", real.display()), format!("umount -f {}", real.display())]);
    assert!(commands.table().is_empty(), "the actual volume must be gone before it is forgotten");
    let view = c.mounts().await.unwrap();
    assert!(view.mounts.is_empty() && view.remembered.is_empty(), "{view:?}");
    d.stop().await;

    let before = commands.calls().len();
    let (d, c) = start(&server, dir.path(), &commands).await;
    assert!(c.mounts().await.unwrap().mounts.is_empty(), "a force-ejected mount must not restore");
    assert_eq!(commands.calls().len(), before);
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_eject_never_unmounts_a_different_volume_at_the_same_path() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkforeign").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    for (resource, options) in [("/dev/disk9s1", "apfs, local, journaled"), ("voidfs://other", "voidfs, nodev, fskit")] {
        c.mount(&new_mount("footage", &mp)).await.unwrap();
        let real = std::fs::canonicalize(&mp).unwrap();
        let foreign = format!("{resource} on {} ({options})\n", real.display());
        commands.set_table(&foreign);
        let before = commands.calls().len();
        already_gone(c.unmount_force("footage").await, &mp);
        assert_eq!(commands.calls().len(), before, "no umount, with or without force, may touch {resource}");
        assert_eq!(commands.table(), foreign, "the replacement volume is left alone");
        let view = c.mounts().await.unwrap();
        assert!(view.mounts.is_empty() && view.remembered.is_empty(), "the original voidfs mount is already gone: {view:?}");
        commands.set_table("");
    }
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_busy_mount_does_not_restore_an_earlier_successfully_ejected_mount() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkpartial").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let first = dir.path().join("mnt/first");
    let second = dir.path().join("mnt/second");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &first)).await.unwrap();
    c.mount(&new_mount("footage", &second)).await.unwrap();
    let first_real = std::fs::canonicalize(&first).unwrap();
    let second_real = std::fs::canonicalize(&second).unwrap();
    commands.set("busy", Some(second_real.to_str().unwrap()));
    let before = commands.calls().len();
    let error = refused(c.unmount("footage").await, 500, "UnmountFailed");
    assert!(error.contains("Resource busy"), "{error}");
    assert_eq!(commands.calls()[before..], [format!("umount {}", first_real.display()), format!("umount {}", second_real.display())]);
    assert!(!commands.table().contains(first_real.to_str().unwrap()));
    assert!(commands.table().contains(second_real.to_str().unwrap()));
    let view = c.mounts().await.unwrap();
    assert_eq!(view.mounts.iter().map(|m| m.mountpoint.as_str()).collect::<Vec<_>>(), [second.to_str().unwrap()], "the successfully ejected mount is no longer live");
    assert_eq!(view.remembered.iter().map(|m| m.mountpoint.as_str()).collect::<Vec<_>>(), [second.to_str().unwrap()], "a later failure must not leave the earlier success remembered");
    d.stop().await;

    let before = commands.calls().len();
    let (d, c) = start(&server, dir.path(), &commands).await;
    until("only the refused mount restores", async || c.mounts().await.unwrap().mounts.first().is_some_and(|mount| mount.state == "mounted")).await;
    let view = c.mounts().await.unwrap();
    assert_eq!(view.mounts.iter().map(|m| m.mountpoint.as_str()).collect::<Vec<_>>(), [second.to_str().unwrap()]);
    assert_eq!(view.remembered.iter().map(|m| m.mountpoint.as_str()).collect::<Vec<_>>(), [second.to_str().unwrap()]);
    assert_eq!(commands.calls().len(), before, "the busy volume is adopted; the successful eject must not be mounted again");
    commands.set("busy", None);
    c.unmount("footage").await.unwrap();
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adapter_without_force_support_does_not_fall_back_to_normal_eject() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use futures::future::BoxFuture;
    use voidfs_daemon::{Adapter, Core, MountSpec, Mounted};

    struct NormalOnly(Arc<AtomicUsize>);
    impl Adapter for NormalOnly {
        fn name(&self) -> &str { "normal-only" }
        fn mount<'a>(&'a self, _: &'a MountSpec, _: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
            Box::pin(async move { Ok(Arc::new(NormalOnly(self.0.clone())) as Arc<dyn Mounted>) })
        }
    }
    impl Mounted for NormalOnly {
        fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> {
            Box::pin(async move { self.0.fetch_add(1, Ordering::SeqCst); Ok(()) })
        }
    }

    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdknoforce").tempdir_in(std::env::temp_dir()).unwrap();
    let normal_calls = Arc::new(AtomicUsize::new(0));
    let cfg = DaemonConfig { adapters: vec![Arc::new(NormalOnly(normal_calls.clone()))], ..DaemonConfig::new(dir.path(), sdk(&server), Build::default()) };
    let d = Daemon::start(cfg).await.unwrap();
    let c = DaemonClient::new(d.socket());
    c.mount(&new_mount("footage", &dir.path().join("mnt"))).await.unwrap();
    refused(c.unmount_force("footage").await, 500, "UnmountFailed");
    assert_eq!(normal_calls.load(Ordering::SeqCst), 0, "a default force implementation must refuse, rather than invoke ordinary eject");
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1), "an unsupported force must retain the mounted and remembered volume: {view:?}");
    c.unmount("footage").await.unwrap();
    assert_eq!(normal_calls.load(Ordering::SeqCst), 1);
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ejected_volume_is_not_reported_mounted_when_forgetting_it_fails() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkforget").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    let real = std::fs::canonicalize(&mp).unwrap();
    let database = rusqlite::Connection::open(dir.path().join("state.sqlite")).unwrap();
    database.busy_timeout(Duration::from_secs(5)).unwrap();
    database.execute_batch("CREATE TRIGGER refuse_forgetting BEFORE DELETE ON mounts
        BEGIN SELECT RAISE(FAIL, 'injected failure forgetting an ejected mount'); END;").unwrap();

    commands.set("busy", Some(""));
    let before = commands.calls().len();
    let error = c.unmount_force("footage").await.unwrap_err();
    assert!(matches!(&error, ClientError::Api { status: 500, .. }), "{error:?}");
    assert_eq!(commands.calls()[before..], [format!("umount -f {}", real.display())]);
    assert!(commands.table().is_empty(), "the OS eject succeeded before the SQLite failure");
    let view = c.mounts().await.unwrap();
    assert!(view.mounts.is_empty(), "an actually ejected volume must not remain reported as mounted: {view:?}");
    assert_eq!(view.remembered.iter().map(|m| m.mountpoint.as_str()).collect::<Vec<_>>(), [mp.to_str().unwrap()], "failed forgetting leaves its durable row available for retry");

    let after_eject = commands.calls().len();
    assert!(c.unmount("footage").await.is_err(), "forgetting still fails until the storage fault is removed");
    assert_eq!(commands.calls().len(), after_eject, "a forgetting-only retry must not call the adapter again");
    let view = c.mounts().await.unwrap();
    assert!(view.mounts.is_empty() && view.remembered.len() == 1, "{view:?}");

    database.execute_batch("DROP TRIGGER refuse_forgetting;").unwrap();
    already_gone(c.unmount_force(&mp.display().to_string()).await, &mp);
    assert_eq!(commands.calls().len(), after_eject, "a successful forgetting-only retry needs no second OS eject");
    let view = c.mounts().await.unwrap();
    assert!(view.mounts.is_empty() && view.remembered.is_empty(), "{view:?}");
    drop(database);
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_eject_forgets_only_that_mountpoint_without_unmounting_its_replacement() {
    for replacement in ["", "/dev/disk9s1", "voidfs://another-drive"] {
        let server = TestServer::start().await.unwrap();
        voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
        let dir = tempfile::Builder::new().prefix("vdkexternal").tempdir_in(std::env::temp_dir()).unwrap();
        let commands = Commands::new(dir.path());
        let first = dir.path().join("mnt/first");
        let second = dir.path().join("mnt/second");
        let (d, c) = start(&server, dir.path(), &commands).await;
        c.mount(&new_mount("footage", &first)).await.unwrap();
        c.mount(&new_mount("footage", &second)).await.unwrap();
        let first_real = std::fs::canonicalize(&first).unwrap();
        let second_real = std::fs::canonicalize(&second).unwrap();
        let foreign = if replacement.is_empty() { String::new() } else {
            let kind = if replacement.starts_with("voidfs:") { "voidfs, nodev, fskit" } else { "apfs, local" };
            format!("{replacement} on {} ({kind})\n", first_real.display())
        };
        let remaining = format!("voidfs://footage on {} (voidfs, nodev, fskit)\n", second_real.display());
        let before = commands.calls().len();
        commands.set_table(&format!("{foreign}{remaining}"));
        until("only the externally removed mountpoint is forgotten", async || {
            let view = c.mounts().await.unwrap();
            view.mounts.len() == 1 && view.mounts[0].mountpoint == second.to_str().unwrap()
                && view.remembered.len() == 1 && view.remembered[0].mountpoint == second.to_str().unwrap()
        }).await;
        assert_eq!(commands.calls().len(), before, "observing an external eject must never call umount: {replacement}");
        assert_eq!(commands.table(), format!("{foreign}{remaining}"), "the replacement volume and other mount of this drive are untouched");
        d.stop().await;

        let (d, c) = start(&server, dir.path(), &commands).await;
        until("only the still-remembered mount restores", async || c.mounts().await.unwrap().mounts.first().is_some_and(|m| m.state == "mounted")).await;
        let view = c.mounts().await.unwrap();
        assert_eq!(view.mounts.iter().map(|m| m.mountpoint.as_str()).collect::<Vec<_>>(), [second.to_str().unwrap()]);
        assert_eq!(view.remembered.iter().map(|m| m.mountpoint.as_str()).collect::<Vec<_>>(), [second.to_str().unwrap()]);
        assert_eq!(commands.table().lines().filter(|line| line.contains(first_real.to_str().unwrap())).collect::<Vec<_>>(), foreign.lines().collect::<Vec<_>>());
        c.unmount("footage").await.unwrap();
        d.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_mount_table_observations_do_not_forget_an_absent_volume() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkprobeerror").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    commands.set("probe-error", Some("injected mount-table failure\n"));
    commands.set_table("");
    let probes = commands.probes();
    let before = commands.calls().len();
    until("several background observations fail", async || commands.probes() >= probes + 3).await;
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1), "an observation failure is not evidence of eject: {view:?}");
    assert_eq!(view.mounts[0].state, "mounted");
    commands.set("probe-error", None);
    until("a successful observation can confirm the external eject", async || {
        let view = c.mounts().await.unwrap();
        view.mounts.is_empty() && view.remembered.is_empty()
    }).await;
    assert_eq!(commands.calls().len(), before, "neither an observation error nor a confirmed absence may invoke umount");
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_mount_table_observations_do_not_forget_an_absent_volume() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkprobetimeout").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    let delayed_probes = || std::fs::read_to_string(commands.dir.join("delayed-probes")).unwrap_or_default().lines().count();
    commands.set("probe-delay", Some(""));
    until("a delayed query has replaced any previously cached table", async || delayed_probes() >= 1).await;
    commands.set_table("");
    let before = commands.calls().len();
    // Every command now sleeps for two seconds; each 500 ms helper deadline must leave its
    // observation unknown. Starting the fourth attempt proves three prior attempts finished.
    until("several genuinely absent observations time out", async || delayed_probes() >= 4).await;
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1), "a timed-out observation is not evidence of eject: {view:?}");
    assert_eq!(view.mounts[0].state, "mounted");
    commands.set("probe-delay", None);
    until("a recovered helper can confirm the external eject", async || {
        let view = c.mounts().await.unwrap();
        view.mounts.is_empty() && view.remembered.is_empty()
    }).await;
    assert_eq!(commands.calls().len(), before, "neither an observation timeout nor a confirmed absence may invoke umount");
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_mount_is_retained_while_its_exact_resource_remains_present() {
    use std::os::unix::fs::PermissionsExt;

    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkdead").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    std::fs::set_permissions(&mp, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(std::fs::read_dir(&mp).is_err(), "the test mount's data path must be unreadable");
    let probes = commands.probes();
    let before = commands.calls().len();
    until("several observations retain a dead but mounted resource", async || commands.probes() >= probes + 3).await;
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1), "an unreadable module is not an explicit eject: {view:?}");
    assert_eq!(view.mounts[0].state, "mounted");
    assert_eq!(commands.calls().len(), before, "a presence check must not attempt dead-module recovery");
    std::fs::set_permissions(&mp, std::fs::Permissions::from_mode(0o755)).unwrap();
    c.unmount("footage").await.unwrap();
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_eject_retries_failed_forgetting_without_another_os_unmount() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkexternaldb").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    let database = rusqlite::Connection::open(dir.path().join("state.sqlite")).unwrap();
    database.busy_timeout(Duration::from_secs(5)).unwrap();
    database.execute_batch("CREATE TRIGGER refuse_external_forgetting BEFORE DELETE ON mounts
        BEGIN SELECT RAISE(FAIL, 'injected external-eject forgetting failure'); END;").unwrap();
    let before = commands.calls().len();
    commands.set_table("");
    until("confirmed absence is retained for a storage retry", async || {
        let view = c.mounts().await.unwrap();
        view.mounts.iter().all(|m| m.state != "mounted") && view.remembered.len() == 1
    }).await;
    assert_eq!(commands.calls().len(), before, "reconciling a gone resource may never invoke umount");
    database.execute_batch("DROP TRIGGER refuse_external_forgetting;").unwrap();
    until("background reconciliation retries the remembered-row deletion", async || {
        let view = c.mounts().await.unwrap();
        view.mounts.is_empty() && view.remembered.is_empty()
    }).await;
    assert_eq!(commands.calls().len(), before, "retrying a SQLite failure needs no adapter unmount");
    drop(database);
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_successful_remount_supersedes_an_old_pending_forget_after_sqlite_failure() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkpendingnew").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    let database = rusqlite::Connection::open(dir.path().join("state.sqlite")).unwrap();
    database.busy_timeout(Duration::from_secs(5)).unwrap();
    database.execute_batch("CREATE TRIGGER refuse_old_forgetting BEFORE DELETE ON mounts
        BEGIN SELECT RAISE(FAIL, 'injected old pending-forget failure'); END;").unwrap();
    commands.set_table("");
    until("the old absent mount has a pending persistence retry", async || {
        let view = c.mounts().await.unwrap();
        view.mounts.is_empty() && view.remembered.len() == 1
    }).await;
    c.mount(&NewMount { read_only: true, ..new_mount("footage", &mp) }).await.unwrap();
    database.execute_batch("DROP TRIGGER refuse_old_forgetting;").unwrap();
    drop(database);
    let probes = commands.probes();
    until("reconciliation has checked the replacement after storage recovery", async || commands.probes() >= probes + 3).await;
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1), "retrying an old forget must not delete the new mount's intent: {view:?}");
    assert!(view.mounts[0].read_only && view.remembered[0].read_only, "the new read-only remembered row must supersede the old writable one: {view:?}");
    d.stop().await;

    let (d, c) = start(&server, dir.path(), &commands).await;
    until("the replacement's remembered row survives restart", async || c.mounts().await.unwrap().mounts.first().is_some_and(|mount| mount.state == "mounted")).await;
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1));
    assert!(view.mounts[0].read_only && view.remembered[0].read_only);
    c.unmount("footage").await.unwrap();
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orderly_shutdown_cancels_a_held_probe_and_preserves_remembered_mounts() {
    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkstopprobe").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    commands.set("probe-gate", Some(""));
    until("the background mount-table observation is held", async || commands.dir.join("probe-entered").exists()).await;
    tokio::time::timeout(Duration::from_secs(10), d.stop()).await.expect("shutdown must cancel its held monitor before detaching mounts");
    assert!(commands.table().is_empty(), "orderly shutdown still unmounts the OS volume");
    commands.set("probe-gate", None);
    let (d, c) = start(&server, dir.path(), &commands).await;
    until("orderly shutdown preserves remembered mounts for restoration", async || c.mounts().await.unwrap().mounts.first().is_some_and(|m| m.state == "mounted")).await;
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1), "shutdown's own detach must not be mistaken for an external eject: {view:?}");
    assert_eq!(commands.calls().iter().filter(|call| call.starts_with("mount ")).count(), 2);
    c.unmount("footage").await.unwrap();
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_presence_result_cannot_forget_a_new_mount_at_the_same_path() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use futures::future::BoxFuture;
    use tokio::sync::Notify;
    use voidfs_daemon::{Adapter, Core, MountSpec, Mounted};

    #[derive(Default)]
    struct HeldPresence {
        mounts: AtomicUsize,
        entered: Notify,
        release: Notify,
        returned_old_absence: AtomicBool,
        new_probes: AtomicUsize,
    }
    struct HeldAdapter(Arc<HeldPresence>);
    struct HeldMount { first: bool, state: Arc<HeldPresence> }
    impl Adapter for HeldAdapter {
        fn name(&self) -> &str { "held-presence" }
        fn mount<'a>(&'a self, _: &'a MountSpec, _: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
            Box::pin(async move {
                Ok(Arc::new(HeldMount { first: self.0.mounts.fetch_add(1, Ordering::SeqCst) == 0, state: self.0.clone() }) as Arc<dyn Mounted>)
            })
        }
    }
    impl Mounted for HeldMount {
        fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> { Box::pin(async { Ok(()) }) }
        fn is_present(&self) -> BoxFuture<'_, Result<bool, String>> {
            Box::pin(async move {
                if self.first {
                    let release = self.state.release.notified();
                    self.state.entered.notify_one();
                    release.await;
                    self.state.returned_old_absence.store(true, Ordering::SeqCst);
                    Ok(false)
                } else {
                    self.state.new_probes.fetch_add(1, Ordering::SeqCst);
                    Ok(true)
                }
            })
        }
    }

    let server = TestServer::start().await.unwrap();
    voidfs_sdk::Client::new(sdk(&server)).unwrap().create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkrerace").tempdir_in(std::env::temp_dir()).unwrap();
    let state = Arc::new(HeldPresence::default());
    let cfg = DaemonConfig { adapters: vec![Arc::new(HeldAdapter(state.clone()))], ..DaemonConfig::new(dir.path(), sdk(&server), Build::default()) };
    let mp = dir.path().join("mnt");
    let d = Daemon::start(cfg).await.unwrap();
    let c = DaemonClient::new(d.socket());
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), state.entered.notified()).await.expect("the old mount's observation is held");
    c.unmount(&mp.display().to_string()).await.unwrap();
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    state.release.notify_one();
    until("the stale absence is returned and the replacement is subsequently checked", async || {
        state.returned_old_absence.load(Ordering::SeqCst) && state.new_probes.load(Ordering::SeqCst) > 0
    }).await;
    let view = c.mounts().await.unwrap();
    assert_eq!((view.mounts.len(), view.remembered.len()), (1, 1), "an old disappearance must not erase a new mount at the same path: {view:?}");
    assert_eq!(state.mounts.load(Ordering::SeqCst), 2);
    c.unmount("footage").await.unwrap();
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_eject_preserves_a_queued_mount_save_across_daemon_restart() {
    use bytes::Bytes;
    use voidfs_daemon::api::Scope;

    let server = TestServer::start().await.unwrap();
    let remote = voidfs_sdk::Client::new(sdk(&server)).unwrap();
    remote.create_drive("footage", Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdkexternaldata").tempdir_in(std::env::temp_dir()).unwrap();
    let commands = Commands::new(dir.path());
    let mp = dir.path().join("mnt");
    let (d, c) = start(&server, dir.path(), &commands).await;
    c.mount(&new_mount("footage", &mp)).await.unwrap();
    c.pause(&Scope::All).await.unwrap();
    let session = c.session("footage", false).await.unwrap();
    let file = session.create(session.info().root, "save", 0o600).await.unwrap();
    let opened = session.open(file.ino, true).await.unwrap();
    let bytes = Bytes::from_static(b"acknowledged mount save before external eject\0\xff");
    assert_eq!(session.write(opened.fh, 0, bytes.clone()).await.unwrap(), bytes.len());
    session.close(opened.fh).await.unwrap();
    session.release().await.unwrap();
    drop(session);
    let queued = c.uploads(true, None).await.unwrap().queue.items.into_iter().find(|entry| entry.key == "save").unwrap();
    assert_eq!(queued.state, voidfs_client::State::Queued);
    let before = commands.calls().len();
    commands.set_table("");
    until("external eject forgets only mount intent", async || {
        let view = c.mounts().await.unwrap();
        view.mounts.is_empty() && view.remembered.is_empty()
    }).await;
    let retained = c.uploads(true, None).await.unwrap().queue.items.into_iter().find(|entry| entry.id == queued.id).unwrap();
    assert_eq!(retained.state, voidfs_client::State::Queued, "eject must not cancel a queued save");
    assert_eq!(commands.calls().len(), before);
    d.stop().await;

    let (d, c) = start(&server, dir.path(), &commands).await;
    assert!(c.mounts().await.unwrap().mounts.is_empty(), "the externally ejected volume must not restore");
    assert_eq!(commands.calls().len(), before, "recovery of queued bytes needs no OS mount");
    let session = c.session("footage", true).await.unwrap();
    let saved = session.lookup(session.info().root, "save").await.unwrap();
    let opened = session.open(saved.ino, false).await.unwrap();
    assert_eq!(session.read(opened.fh, 0, bytes.len() as u64).await.unwrap(), bytes);
    session.close(opened.fh).await.unwrap();
    session.release().await.unwrap();
    c.resume(&Scope::All).await.unwrap();
    until("the retained save publishes after resume", async || c.uploads(true, None).await.unwrap().queue.items.iter().any(|entry| entry.id == queued.id && entry.state == voidfs_client::State::Done)).await;
    assert_eq!(remote.get_object("footage", "save", Default::default()).await.unwrap().body, bytes);
    d.stop().await;
}
