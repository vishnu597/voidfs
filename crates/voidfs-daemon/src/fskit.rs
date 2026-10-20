// SPDX-License-Identifier: Apache-2.0
//! The `fskit` adapter (step 5, item 3): a drive mounted by the FSKit module in voidfs.app, as the
//! user, with `mount -F -t voidfs voidfs://<drive> <mountpoint>`. The resource names only the
//! drive: this daemon holds the server and key. The module reaches the daemon through the app's
//! launch agent, with a session and an invalidation watch of its own, so the feed has nothing to
//! tell the adapter here. Mounts are writable (item 4) unless asked read-only (`mount -r`), so a
//! writable mount remembered while the module was read-only comes back writable.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use crate::mounts::{Adapter, Core, MountSpec, Mounted};

/// How long `mount` or `umount` may take: FSKit loads the module, which opens its session.
const WAIT: Duration = Duration::from_secs(60);
/// A monitor probe must not hold up mount controls while a helper is unresponsive.
const PRESENCE_WAIT: Duration = Duration::from_millis(500);
const TABLE_TTL: Duration = Duration::from_millis(250);

struct MountSnapshot { read: Instant, table: Result<String, String> }

/// Concurrent presence checks share one short-lived, owned snapshot. A newly attached volume
/// clears it under the same mutex, so a pre-mount query cannot refill it with stale absence.
struct MountTable {
    command: PathBuf,
    cached: tokio::sync::Mutex<Option<MountSnapshot>>,
}

impl MountTable {
    async fn read(&self) -> Result<String, String> {
        let mut cached = self.cached.lock().await;
        if let Some(snapshot) = &*cached
            && snapshot.read.elapsed() < TABLE_TTL
        { return snapshot.table.clone(); }
        let table = run_with_timeout(&self.command, Vec::new(), PRESENCE_WAIT).await;
        *cached = Some(MountSnapshot { read: Instant::now(), table: table.clone() });
        table
    }

    async fn invalidate(&self) { *self.cached.lock().await = None; }
}

/// Mounts drives with FSKit.
pub struct FsKit {
    mount: PathBuf,
    umount: PathBuf,
    table: Arc<MountTable>,
}

impl Default for FsKit {
    fn default() -> FsKit {
        FsKit::new("/sbin/mount", "/sbin/umount")
    }
}

impl FsKit {
    /// With these `mount` and `umount` commands: the system's, or a test's.
    pub fn new(mount: impl Into<PathBuf>, umount: impl Into<PathBuf>) -> FsKit {
        let mount = mount.into();
        let table = Arc::new(MountTable { command: mount.clone(), cached: tokio::sync::Mutex::new(None) });
        FsKit { mount, umount: umount.into(), table }
    }

    /// The `voidfs` mount at `at`, if there is one: its resource.
    async fn mounted(&self, at: &Path) -> Result<Option<String>, String> {
        let table = run(&self.mount, Vec::new()).await?;
        Ok(find(&table, at))
    }

    /// Called only after mounting or adopting a volume succeeded.
    async fn attached(&self, at: PathBuf, url: String) -> Arc<dyn Mounted> {
        self.table.invalidate().await;
        Arc::new(FsKitMount { umount: self.umount.clone(), mount: self.mount.clone(), table: self.table.clone(), at, url })
    }
}

/// The resource FSKit mounts a drive from: `voidfs://<drive>`.
pub fn resource(drive: &str) -> String {
    format!("voidfs://{drive}")
}

/// The resource mounted at `at` with type `voidfs`, from `mount`'s table: lines such as
/// `voidfs://footage on /Users/me/voidfs/footage (voidfs, nodev, nosuid, read-only, fskit, …)`.
/// Something of another type at `at` answers `Some("")`.
fn find(table: &str, at: &Path) -> Option<String> {
    let at = at.to_str()?;
    table.lines().find_map(|line| {
        let (from, rest) = line.split_once(" on ")?;
        let (on, options) = rest.rsplit_once(" (")?;
        (on == at).then(|| if options.starts_with("voidfs,") || options.starts_with("voidfs)") { from.to_owned() } else { String::new() })
    })
}

/// Absence is authoritative only when the complete successful snapshot is well formed.
fn present(table: &str, at: &Path, url: &str) -> Result<bool, String> {
    let at = at.to_str().ok_or("the mount table cannot identify a non-UTF-8 mountpoint")?;
    let malformed = || "mount returned a malformed mount table".to_owned();
    let mut matches = false;
    for line in table.lines().filter(|line| !line.trim().is_empty()) {
        let (from, rest) = line.split_once(" on ").ok_or_else(malformed)?;
        let (on, options) = rest.rsplit_once(" (").ok_or_else(malformed)?;
        if !options.ends_with(')') { return Err(malformed()); }
        if on == at && from == url && (options.starts_with("voidfs,") || options == "voidfs)") { matches = true; }
    }
    Ok(matches)
}

/// Whether the mount at `at` lists its top folder within a few seconds.
async fn answers(at: &Path) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(5), tokio::fs::read_dir(at)).await, Ok(Ok(_)))
}

/// Runs `program` with `args` and returns what it printed, or, when it fails, what it said.
async fn run(program: &Path, args: Vec<OsString>) -> Result<String, String> {
    run_with_timeout(program, args, WAIT).await
}

async fn run_with_timeout(program: &Path, args: Vec<OsString>, timeout: Duration) -> Result<String, String> {
    let name = program.display().to_string();
    let child = tokio::process::Command::new(program).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .kill_on_drop(true).spawn().map_err(|e| format!("{name}: {e}"))?;
    let out = tokio::time::timeout(timeout, child.wait_with_output()).await.map_err(|_| format!("{name} took over {} s", timeout.as_secs_f64()))?.map_err(|e| format!("{name}: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&out.stderr).trim().replace('\n', "; ");
    Err(if said.is_empty() { format!("{name} failed ({})", out.status) } else { said })
}

impl Adapter for FsKit {
    fn name(&self) -> &str {
        "fskit"
    }

    fn mount<'a>(&'a self, spec: &'a MountSpec, _core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        Box::pin(async move {
            // `mount` prints a mountpoint's real path (`/private/tmp`, not `/tmp`).
            let at = std::fs::canonicalize(&spec.mountpoint).map_err(|e| format!("{}: {e}", spec.mountpoint.display()))?;
            let url = resource(&spec.drive);
            match self.mounted(&at).await? {
                // Still mounted from before this daemon started (it was killed): the module opens
                // a new session on its next call. If the module died instead, FSKit keeps the mount
                // and answers every call with EIO, without starting it again (step 5, item 4): such
                // a mount is unmounted and mounted anew.
                Some(from) if from == url => {
                    if answers(&at).await {
                        return Ok(self.attached(at, url).await);
                    }
                    run(&self.umount, vec!["-f".into(), at.clone().into_os_string()]).await?;
                }
                Some(from) if from.is_empty() => return Err(format!("something else is mounted at {}", at.display())),
                Some(from) => return Err(format!("{from} is mounted at {}", at.display())),
                None => {}
            }
            let mut args: Vec<OsString> = vec!["-F".into(), "-t".into(), "voidfs".into()];
            if spec.read_only {
                args.push("-r".into());
            }
            args.extend([OsString::from(&url), at.clone().into_os_string()]);
            run(&self.mount, args).await.map_err(|e| {
                if e.contains("disabled") { format!("{e}: switch voidfs on in System Settings → General → Login Items & Extensions → File System Extensions") } else { e }
            })?;
            Ok(self.attached(at, url).await)
        })
    }
}

struct FsKitMount {
    mount: PathBuf,
    umount: PathBuf,
    table: Arc<MountTable>,
    at: PathBuf,
    url: String,
}

impl Mounted for FsKitMount {
    fn is_present(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move {
            let table = self.table.read().await?;
            present(&table, &self.at, &self.url)
        })
    }

    /// Unmounts it, unless it is gone already (ejected in Finder, or `umount`ed). Something else
    /// mounted there since is left alone.
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move { self.eject(false).await })
    }

    fn force_unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move { self.eject(true).await })
    }
}

impl FsKitMount {
    async fn eject(&self, force: bool) -> Result<(), String> {
        let table = run(&self.mount, Vec::new()).await?;
        if find(&table, &self.at).as_deref() != Some(self.url.as_str()) {
            return Ok(());
        }
        let mut args = Vec::new();
        if force { args.push("-f".into()); }
        args.push(self.at.clone().into_os_string());
        run(&self.umount, args).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_what_is_mounted_where() {
        let table = "/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)\n\
            voidfs://footage on /Users/me/voidfs/footage (voidfs, nodev, nosuid, read-only, noowners, noatime, fskit, mounted by me)\n\
            voidfs://second on /Users/me/a on b (x) (voidfs, nodev, fskit)\n\
            //me@nas/share on /Volumes/share (smbfs, nodev, nosuid, mounted by me)\n";
        assert_eq!(find(table, Path::new("/Users/me/voidfs/footage")).as_deref(), Some("voidfs://footage"));
        assert_eq!(find(table, Path::new("/Users/me/a on b (x)")).as_deref(), Some("voidfs://second"), "a mountpoint with ' on ' and parentheses");
        assert_eq!(find(table, Path::new("/Volumes/share")).as_deref(), Some(""), "another type");
        assert_eq!(find(table, Path::new("/Users/me/voidfs")), None);
        assert_eq!(resource("footage"), "voidfs://footage");
    }

    #[test]
    fn presence_requires_a_complete_well_formed_snapshot() {
        let at = Path::new("/Users/me/mounted");
        let url = "voidfs://footage";
        assert!(!present("", at, url).unwrap());
        assert!(present("voidfs://footage on /Users/me/mounted (voidfs, fskit)\n", at, url).unwrap());
        for malformed in ["helper warning\n", "voidfs://footage on /Users/me/mounted (voidfs\n",
            "voidfs://footage on /Users/me/mounted (voidfs, fskit)\ntruncated\n"] {
            assert!(present(malformed, at, url).is_err(), "a malformed snapshot must not forget a mount");
        }
        assert!(!present("voidfs://other on /Users/me/mounted (voidfs, fskit)\n", at, url).unwrap());
        assert!(!present("voidfs://footage on /Users/me/mounted (apfs, local)\n", at, url).unwrap());
    }

    #[tokio::test]
    async fn an_attached_handle_cannot_observe_a_cached_pre_mount_absence() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let command = dir.path().join("mount");
        let table = dir.path().join("table");
        std::fs::write(&command, "#!/bin/sh\nexec /bin/cat \"${0%/*}/table\"\n").unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&table, "").unwrap();
        let adapter = FsKit::new(command, "/unused/umount");
        let at = PathBuf::from("/Users/me/mounted");
        assert_eq!(find(&adapter.table.read().await.unwrap(), &at), None);

        std::fs::write(&table, "voidfs://footage on /Users/me/mounted (voidfs, fskit)\n").unwrap();
        // Both a successful mount and an adoption return through this factory. Inspecting the
        // cache first makes this regression independent of how long CI took to update the table.
        let handle = adapter.attached(at, resource("footage")).await;
        assert!(adapter.table.cached.lock().await.is_none(), "new handles must clear snapshots made before attachment");
        assert!(handle.is_present().await.unwrap());
    }
}
