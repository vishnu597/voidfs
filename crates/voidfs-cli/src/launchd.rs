// SPDX-License-Identifier: Apache-2.0
//! `daemon install|uninstall`: a launchd agent on macOS, so that the daemon, and the mounts it
//! remembers, come back at login. A systemd user unit comes with step 9.
//!
//! voidfs.app starts the daemon at login too, with its own copy of `void` (step 5, item 7): its
//! Start at login registers the job [`APP_LABEL`] with SMAppService. The app takes over: its
//! daemon removes this agent when it starts, for the same state directory, and while the app's job
//! is loaded `daemon install` refuses, so that one daemon, started one way, owns the directory.
//!
//! `VOIDFS_LAUNCH_AGENTS_DIR` puts the plist elsewhere than `~/Library/LaunchAgents`,
//! `VOIDFS_LAUNCHCTL` runs another `launchctl` and `VOIDFS_DEFAULTS` another `defaults`: the tests
//! use all three.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;
use voidfs_daemon::Settings;

use crate::config::Connection;
use crate::daemon::{self, Place};
use crate::output::{Failure, Out, Result};

pub const LABEL: &str = "dev.voidfs.daemon";
/// The app's job: `Contents/MacOS/void daemon run --app`, from the app's
/// `Contents/Library/LaunchAgents/dev.voidfs.app.daemon.plist`.
pub const APP_LABEL: &str = "dev.voidfs.app.daemon";
/// The app's defaults, whose `VoidfsStateDirectory` points the app, and its daemon, at another
/// state directory, as `VOIDFS_STATE_DIR` points the CLI.
const APP_DOMAIN: &str = "dev.voidfs.app";

/// Who starts the daemon at login.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Login {
    /// voidfs.app's job, [`APP_LABEL`].
    App,
    /// The agent `daemon install` writes, [`LABEL`].
    Agent,
}

impl Login {
    pub fn label(self) -> &'static str {
        match self {
            Login::App => APP_LABEL,
            Login::Agent => LABEL,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Login::App => format!("voidfs.app ({APP_LABEL}, Start at login in its menu)"),
            Login::Agent => format!("the launchd agent `void daemon install` wrote ({LABEL})"),
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
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

/// The state directory on macOS when nothing points elsewhere, whatever `VOIDFS_STATE_DIR` says
/// in this process: the one a launchd job without it uses.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join("Library/Application Support/voidfs"))
}

/// The state directory an agent's plist gives its daemon: its `VOIDFS_STATE_DIR`, or the default.
fn agent_dir(plist: &str) -> Option<PathBuf> {
    let Some(at) = plist.find("<key>VOIDFS_STATE_DIR</key>") else { return home_dir() };
    let rest = &plist[at..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = start + rest[start..].find("</string>")?;
    Some(PathBuf::from(unescape(&rest[start..end])))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let trim = |p: &Path| p.components().collect::<PathBuf>();
    trim(a) == trim(b) || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// Whether the agent is installed for state directory `dir`.
pub fn installed_for(dir: &Path) -> bool {
    installed() && plist_path().ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| agent_dir(&t)).is_some_and(|d| same_dir(&d, dir))
}

/// The state directory of the app's daemon: the app's `VoidfsStateDirectory`, if it is a path,
/// or the default.
pub fn app_dir() -> Option<PathBuf> {
    let bin = std::env::var("VOIDFS_DEFAULTS").ok().filter(|b| !b.is_empty()).unwrap_or_else(|| "/usr/bin/defaults".into());
    let set = Command::new(bin).args(["read", APP_DOMAIN, "VoidfsStateDirectory"]).stderr(std::process::Stdio::null()).output().ok().filter(|o| o.status.success());
    let set = set.map(|o| String::from_utf8_lossy(&o.stdout).trim_end_matches('\n').to_owned()).filter(|d| d.starts_with('/'));
    set.map(PathBuf::from).or_else(home_dir)
}

/// Whether voidfs.app's job is loaded: registered, and allowed in Login Items.
fn app_loaded() -> bool {
    domain().is_ok_and(|d| launchctl(&["print", &format!("{d}/{APP_LABEL}")]).is_ok())
}

/// Whether voidfs.app starts the daemon of state directory `dir` at login.
pub fn app_owns(dir: &Path) -> bool {
    cfg!(target_os = "macos") && app_dir().is_some_and(|d| same_dir(&d, dir)) && app_loaded()
}

/// Who starts the daemon of state directory `dir` at login: the app before the agent, since the
/// app's daemon removes the agent when it starts.
pub fn login(dir: &Path) -> Option<Login> {
    if app_owns(dir) {
        Some(Login::App)
    } else if installed_for(dir) {
        Some(Login::Agent)
    } else {
        None
    }
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

/// Asks launchd to start the daemon of the job that starts it at login; whether it would.
pub fn kickstart(login: Login) -> bool {
    domain().is_ok_and(|d| launchctl(&["kickstart", &format!("{d}/{}", login.label())]).is_ok())
}

/// Unloads the agent, which stops its daemon, and removes its plist: what the app's daemon does
/// when it takes over. Whether it was loaded.
pub fn remove_agent() -> Result<bool> {
    let path = plist_path()?;
    let loaded = launchctl(&["bootout", &format!("{}/{LABEL}", domain()?)]).is_ok();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(loaded),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(loaded),
        Err(e) => Err(Failure::local(&path, e)),
    }
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
    // One daemon, started one way: the app's, while it has it.
    if app_owns(&place.dir) {
        return Err(Failure::new(
            "ManagedByApp",
            format!("voidfs.app starts the daemon at login ({APP_LABEL}): turn off Start at login in its menu to install an agent from the terminal, or leave it to the app"),
        ));
    }
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
    let app = Place::new().is_ok_and(|p| app_owns(&p.dir));
    out.emit(&json!({ "installed": false, "label": LABEL, "plist": path, "removed": removed, "unloaded": loaded, "app": app }), || {
        let text = if removed || loaded { format!("uninstalled {LABEL}: the daemon no longer starts at login") } else { format!("{LABEL} wasn't installed") };
        if app { format!("{text}; voidfs.app starts it ({APP_LABEL}): turn off Start at login in its menu") } else { text }
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

    #[test]
    fn an_agents_state_directory_is_read_back_from_its_plist() {
        let p = plist(Path::new("/v"), Some(Path::new("/tmp/s<1>&2")), Path::new("/l"));
        assert_eq!(agent_dir(&p), Some(PathBuf::from("/tmp/s<1>&2")));
        let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/voidfs"));
        assert_eq!(agent_dir(&plist(Path::new("/v"), None, Path::new("/l"))), home, "none is the default");
        assert!(same_dir(Path::new("/tmp/a/"), Path::new("/tmp/a")) && !same_dir(Path::new("/tmp/a"), Path::new("/tmp/b")));
    }
}
