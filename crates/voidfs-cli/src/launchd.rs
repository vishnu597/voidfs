// SPDX-License-Identifier: Apache-2.0
//! `daemon install|uninstall`: a launchd agent on macOS, so that the daemon, and the mounts it
//! remembers, come back at login. A systemd user unit comes with step 9.
//!
//! `VOIDFS_LAUNCH_AGENTS_DIR` puts the plist elsewhere than `~/Library/LaunchAgents`, and
//! `VOIDFS_LAUNCHCTL` runs another `launchctl`: the tests use both.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;
use voidfs_daemon::Settings;

use crate::config::Connection;
use crate::daemon::{self, Place};
use crate::output::{Failure, Out, Result};

pub const LABEL: &str = "dev.voidfs.daemon";

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The agent: `void daemon run` at login, and again if it dies, but not after it stopped as
/// asked; its output in the state directory's log.
pub fn plist(exe: &Path, state_dir: Option<&Path>, log: &Path) -> String {
    let env = state_dir.map_or_else(String::new, |d| {
        format!("    <key>EnvironmentVariables</key>\n    <dict>\n        <key>VOIDFS_STATE_DIR</key>\n        <string>{}</string>\n    </dict>\n", escape(&d.display().to_string()))
    });
    let log = escape(&log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>daemon</string>
        <string>run</string>
    </array>
{env}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        escape(&exe.display().to_string())
    )
}

fn agents_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("VOIDFS_LAUNCH_AGENTS_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).ok_or_else(|| Failure::new("NoStateDirectory", "no HOME, for ~/Library/LaunchAgents"))?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents"))
}

pub fn plist_path() -> Result<PathBuf> {
    Ok(agents_dir()?.join(format!("{LABEL}.plist")))
}

/// Whether the agent is installed: its plist is there.
pub fn installed() -> bool {
    cfg!(target_os = "macos") && plist_path().is_ok_and(|p| p.exists())
}

/// `gui/<uid>`, the domain of this user's agents.
fn domain() -> Result<String> {
    let out = Command::new("id").arg("-u").output().map_err(|e| Failure::new("InternalError", format!("id -u: {e}")))?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || uid.is_empty() {
        return Err(Failure::new("InternalError", "id -u didn't say who you are"));
    }
    Ok(format!("gui/{uid}"))
}

/// Runs `launchctl <args>`; its stderr if it fails.
fn launchctl(args: &[&str]) -> std::result::Result<(), String> {
    let bin = std::env::var("VOIDFS_LAUNCHCTL").ok().filter(|b| !b.is_empty()).unwrap_or_else(|| "launchctl".into());
    match Command::new(&bin).args(args).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("{bin} {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("{bin}: {e}")),
    }
}

/// Asks launchd to start the installed agent's daemon; whether it would.
pub fn kickstart() -> bool {
    domain().is_ok_and(|d| launchctl(&["kickstart", &format!("{d}/{LABEL}")]).is_ok())
}

fn macos_only() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(Failure::new("NotSupported", "`daemon install` writes a launchd agent, on macOS; a systemd user unit comes with step 9"))
    }
}

pub async fn install(conn: &Connection, out: Out) -> Result<()> {
    macos_only()?;
    let place = Place::new()?;
    // The agent's daemon reads its connection from daemon.json.
    let config = daemon::connection(conn, &place.dir)?;
    Settings::of(&config).save(&place.dir).map_err(|e| Failure::local(&Settings::path(&place.dir), e))?;
    // A daemon started otherwise makes way for launchd's.
    let stopped = daemon::stop_daemon(&place).await?;
    let exe = std::env::current_exe().map_err(|e| Failure::new("InternalError", format!("finding the void binary: {e}")))?;
    let state = std::env::var_os("VOIDFS_STATE_DIR").filter(|d| !d.is_empty()).map(|_| place.dir.clone());
    let path = plist_path()?;
    let dir = path.parent().expect("a plist's folder");
    std::fs::create_dir_all(dir).map_err(|e| Failure::local(dir, e))?;
    std::fs::write(&path, plist(&exe, state.as_deref(), &place.dir.join(voidfs_daemon::LOG))).map_err(|e| Failure::local(&path, e))?;
    let domain = domain()?;
    // One loaded already, from an earlier install, goes first.
    let _ = launchctl(&["bootout", &format!("{domain}/{LABEL}")]);
    launchctl(&["bootstrap", &domain, &path.display().to_string()]).map_err(|e| Failure::new("LaunchdFailed", e))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let s = loop {
        if let Some(s) = place.status().await? {
            break s;
        }
        if Instant::now() > deadline {
            return Err(Failure::new("DaemonFailed", format!("launchd loaded {LABEL}, but its daemon didn't answer within 20 s: see {}", place.dir.join(voidfs_daemon::LOG).display())));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    out.emit(&json!({ "installed": true, "label": LABEL, "plist": path, "pid": s.daemon.pid, "stoppedPid": stopped }), || {
        format!("installed {LABEL} ({}): the daemon (pid {}) and its remembered mounts come back at login", path.display(), s.daemon.pid)
    })
}

pub async fn uninstall(out: Out) -> Result<()> {
    macos_only()?;
    let path = plist_path()?;
    let domain = domain()?;
    // launchd stops the daemon it started, as SIGTERM does.
    let loaded = launchctl(&["bootout", &format!("{domain}/{LABEL}")]).is_ok();
    let removed = match std::fs::remove_file(&path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(Failure::local(&path, e)),
    };
    out.emit(&json!({ "installed": false, "label": LABEL, "plist": path, "removed": removed, "unloaded": loaded }), || {
        if removed || loaded { format!("uninstalled {LABEL}: the daemon no longer starts at login") } else { format!("{LABEL} wasn't installed") }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_runs_the_daemon_at_login_and_again_if_it_dies() {
        let p = plist(Path::new("/opt/v&d/void"), Some(Path::new("/tmp/s<1>")), Path::new("/tmp/s<1>/daemon.log"));
        assert!(p.contains("<string>dev.voidfs.daemon</string>"));
        assert!(p.contains("<array>\n        <string>/opt/v&amp;d/void</string>\n        <string>daemon</string>\n        <string>run</string>\n    </array>"), "{p}");
        assert!(p.contains("<key>VOIDFS_STATE_DIR</key>\n        <string>/tmp/s&lt;1&gt;</string>"), "{p}");
        assert!(p.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(p.contains("<key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>"), "not after it stopped as asked: {p}");
        assert!(p.contains("<key>StandardErrorPath</key>\n    <string>/tmp/s&lt;1&gt;/daemon.log</string>"));
        assert!(!plist(Path::new("/v"), None, Path::new("/l")).contains("EnvironmentVariables"), "the default state directory needs none");
    }
}
