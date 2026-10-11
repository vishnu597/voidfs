// SPDX-License-Identifier: Apache-2.0
//! The `fskit` adapter (step 5, item 3): a drive mounted by the FSKit module in voidfs.app, as the
//! user, with `mount -F -t voidfs voidfs://<drive> <mountpoint>`. The resource names only the
//! drive: this daemon holds the server and key. The module reaches the daemon through the app's
//! launch agent, with a session and an invalidation watch of its own, so the feed has nothing to
//! tell the adapter here. The module is read-only until item 4.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

use crate::mounts::{Adapter, Core, MountSpec, Mounted};

/// How long `mount` or `umount` may take: FSKit loads the module, which opens its session.
const WAIT: Duration = Duration::from_secs(60);

/// Mounts drives with FSKit.
pub struct FsKit {
    mount: PathBuf,
    umount: PathBuf,
}

impl Default for FsKit {
    fn default() -> FsKit {
        FsKit::new("/sbin/mount", "/sbin/umount")
    }
}

impl FsKit {
    /// With these `mount` and `umount` commands: the system's, or a test's.
    pub fn new(mount: impl Into<PathBuf>, umount: impl Into<PathBuf>) -> FsKit {
        FsKit { mount: mount.into(), umount: umount.into() }
    }

    /// The `voidfs` mount at `at`, if there is one: its resource.
    async fn mounted(&self, at: &Path) -> Result<Option<String>, String> {
        let table = run(&self.mount, Vec::new()).await?;
        Ok(find(&table, at))
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

/// Runs `program` with `args` and returns what it printed, or, when it fails, what it said.
async fn run(program: &Path, args: Vec<OsString>) -> Result<String, String> {
    let name = program.display().to_string();
    let child = tokio::process::Command::new(program).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .kill_on_drop(true).spawn().map_err(|e| format!("{name}: {e}"))?;
    let out = tokio::time::timeout(WAIT, child.wait_with_output()).await.map_err(|_| format!("{name} took over {} s", WAIT.as_secs()))?.map_err(|e| format!("{name}: {e}"))?;
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

    fn writable(&self) -> bool {
        false
    }

    fn mount<'a>(&'a self, spec: &'a MountSpec, _core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        Box::pin(async move {
            // `mount` prints a mountpoint's real path (`/private/tmp`, not `/tmp`).
            let at = std::fs::canonicalize(&spec.mountpoint).map_err(|e| format!("{}: {e}", spec.mountpoint.display()))?;
            let url = resource(&spec.drive);
            match self.mounted(&at).await? {
                // Still mounted from before this daemon started (it was killed): the module opens
                // a new session on its next call.
                Some(from) if from == url => return Ok(Arc::new(FsKitMount { umount: self.umount.clone(), mount: self.mount.clone(), at, url }) as Arc<dyn Mounted>),
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
            Ok(Arc::new(FsKitMount { umount: self.umount.clone(), mount: self.mount.clone(), at, url }) as Arc<dyn Mounted>)
        })
    }
}

struct FsKitMount {
    mount: PathBuf,
    umount: PathBuf,
    at: PathBuf,
    url: String,
}

impl Mounted for FsKitMount {
    /// Unmounts it, unless it is gone already (ejected in Finder, or `umount`ed). Something else
    /// mounted there since is left alone.
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            let table = run(&self.mount, Vec::new()).await?;
            if find(&table, &self.at).as_deref() != Some(self.url.as_str()) {
                return Ok(());
            }
            run(&self.umount, vec![self.at.clone().into_os_string()]).await.map(drop)
        })
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
}
