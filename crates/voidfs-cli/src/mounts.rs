// SPDX-License-Identifier: Apache-2.0
//! `mount`, `unmount` and `mounts`: the daemon's mount table, and the mounts it remembers.
//! Mounting is an adapter's (step 5); a daemon without one answers `NoAdapter`.

use std::path::{Path, PathBuf};

use serde_json::json;
use voidfs_daemon::ClientError;
use voidfs_daemon::api::{self, NewMount};

use crate::config::Connection;
use crate::daemon::{self, DaemonOptions, Place, failure};
use crate::output::{Failure, Out, Result, count, table};

#[derive(clap::Args, Debug)]
pub struct MountArgs {
    /// The drive, by alias or id
    pub drive: String,
    /// Where to mount it [default: ~/voidfs/<drive>]
    pub mountpoint: Option<PathBuf>,
    /// Mount it read-only
    #[arg(long)]
    pub read_only: bool,
    /// The adapter that mounts it (with step 5: fskit, smb or fuse) [default: the daemon's first]
    #[arg(long, value_name = "NAME")]
    pub adapter: Option<String>,
}

fn absolute(p: &Path) -> Result<String> {
    std::path::absolute(p).map(|a| a.display().to_string()).map_err(|e| Failure::local(p, e))
}

/// `mount`: starts the daemon if it isn't running, as Space's does.
pub async fn mount(conn: &Connection, out: Out, args: &MountArgs) -> Result<()> {
    let place = Place::new()?;
    daemon::ensure_started(conn, &DaemonOptions::default(), &place).await?;
    let mountpoint = args.mountpoint.as_deref().map(absolute).transpose()?;
    let m = place.client.mount(&NewMount { drive: args.drive.clone(), mountpoint, read_only: args.read_only, adapter: args.adapter.clone() }).await.map_err(failure)?;
    let ro = if m.read_only { ", read-only" } else { "" };
    out.emit(&m, || format!("mounted {} at {} ({}{ro}); it comes back when the daemon starts", m.drive, m.mountpoint, m.adapter))
}

/// `unmount <mountpoint | drive>`.
pub async fn unmount(out: Out, target: &str) -> Result<()> {
    let place = Place::new()?;
    // A path, as typed or relative; otherwise a drive's alias or id.
    let target = if target.contains('/') || target.starts_with('.') || Path::new(target).exists() { absolute(Path::new(target))? } else { target.to_owned() };
    let u = place.client.unmount(&target).await.map_err(failure)?;
    out.emit(&u, || u.unmounted.iter().map(|m| format!("unmounted {m}, and forgot it")).collect::<Vec<_>>().join("\n"))
}

/// One line for a mount, for `mounts` and `status`.
pub fn line(m: &api::Mount) -> String {
    let mut l = format!("{} at {} · {}", m.drive, m.mountpoint, m.adapter);
    if m.read_only {
        l.push_str(" · read-only");
    }
    match (&m.feed, &m.error) {
        (_, Some(e)) => l.push_str(&format!(" · {}: {e}", m.state)),
        (Some(f), _) => l.push_str(&format!(" · {} · feed at {}, {}", m.state, f.seq, count(f.events as usize, "change"))),
        _ => l.push_str(&format!(" · {}", m.state)),
    }
    l
}

/// The remembered mounts while no daemon runs, if its state can be read.
fn remembered_without_daemon(dir: &Path) -> Vec<voidfs_client::Remembered> {
    if !dir.join("state.sqlite").exists() {
        return Vec::new();
    }
    voidfs_client::Store::open(dir).ok().and_then(|s| voidfs_client::mounts::remembered(&s).ok()).unwrap_or_default()
}

pub async fn mounts(out: Out) -> Result<()> {
    let place = Place::new()?;
    let (running, view) = match place.client.mounts().await {
        Ok(v) => (true, v),
        Err(ClientError::NotRunning { .. }) => (false, api::Mounts { mounts: Vec::new(), remembered: remembered_without_daemon(&place.dir) }),
        Err(e) => return Err(failure(e)),
    };
    let doc = json!({ "daemon": { "running": running, "socket": place.socket() }, "mounts": view.mounts, "remembered": view.remembered });
    out.emit(&doc, || {
        let mut parts = Vec::new();
        if !running {
            parts.push(format!("daemon not running (socket {}): nothing is mounted", place.socket().display()));
        } else if view.mounts.is_empty() {
            parts.push("no drives mounted (mount one with `void mount <drive>`)".to_owned());
        } else {
            let rows: Vec<Vec<String>> = view
                .mounts
                .iter()
                .map(|m| {
                    let state = m.error.as_ref().map_or_else(|| m.state.clone(), |e| format!("{}: {e}", m.state));
                    let feed = m.feed.as_ref().map_or_else(String::new, |f| format!("at {}", f.seq));
                    vec![m.drive.clone(), m.mountpoint.clone(), m.adapter.clone(), state, m.since.clone(), feed]
                })
                .collect();
            parts.push(table(&["DRIVE", "MOUNTPOINT", "ADAPTER", "STATE", "SINCE", "FEED"], &rows));
        }
        let waiting: Vec<&voidfs_client::Remembered> = view.remembered.iter().filter(|r| !view.mounts.iter().any(|m| m.mountpoint == r.mountpoint)).collect();
        if !waiting.is_empty() {
            let rows: Vec<Vec<String>> = waiting.iter().map(|r| vec![r.drive.clone(), r.mountpoint.clone(), r.adapter.clone()]).collect();
            parts.push(format!("remembered, mounted again when the daemon starts:\n{}", table(&["DRIVE", "MOUNTPOINT", "ADAPTER"], &rows)));
        }
        parts.join("\n\n")
    })
}
