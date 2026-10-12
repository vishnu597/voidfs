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
/// call and removes the line, unless `busy` exists.
struct Commands {
    dir: PathBuf,
}

impl Commands {
    fn new(dir: &Path) -> Commands {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir.join("commands");
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.display();
        let mount = format!("#!/bin/sh\nif [ $# -eq 0 ]; then cat '{d}/table' 2>/dev/null; exit 0; fi\necho \"mount $*\" >> '{d}/calls'\nif [ -e '{d}/refuse' ]; then cat '{d}/refuse' >&2; exit 1; fi\n\
            eval \"res=\\${{$(($# - 1))}}\"; eval \"mp=\\${{$#}}\"\necho \"$res on $mp (voidfs, nodev, nosuid, read-only, noowners, noatime, fskit, mounted by test)\" >> '{d}/table'\n");
        let umount = format!("#!/bin/sh\necho \"umount $*\" >> '{d}/calls'\nif [ -e '{d}/busy' ]; then echo \"umount: unmount($1): Resource busy\" >&2; exit 1; fi\n\
            grep -v -F \" on $1 (\" '{d}/table' > '{d}/table.new'; mv '{d}/table.new' '{d}/table'\n");
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
        std::fs::write(self.dir.join("table"), text).unwrap();
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
    assert_eq!(c.unmount(&mp.display().to_string()).await.unwrap().unmounted, [mp.display().to_string()]);
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
