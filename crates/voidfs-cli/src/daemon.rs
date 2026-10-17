// SPDX-License-Identifier: Apache-2.0
//! `daemon run|start|stop|restart|status|info` and `status`: the per-user agent, which owns the
//! state directory and answers on a Unix socket in it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use serde_json::json;
use voidfs_daemon::api::{self, Build};
use voidfs_daemon::{ClientError, Daemon, DaemonClient, DaemonConfig, Settings};

use crate::config::Connection;
use crate::launchd::Login;
use crate::output::{Failure, Out, Result, count, size};

/// How long `start` waits for a new daemon to answer.
const START_WAIT: Duration = Duration::from_secs(20);
/// How long `stop` waits for the daemon to let go of the state directory.
const STOP_WAIT: Duration = Duration::from_secs(60);
/// How long the app's daemon waits for one it took over from to let go of the state directory.
const TAKE_OVER_WAIT: Duration = Duration::from_secs(10);
/// How often `daemon run` looks for new conflicts to log.
const CONFLICT_LOOK: Duration = Duration::from_secs(5);
/// A log larger than this is moved to `daemon.log.1` when the daemon starts.
const LOG_MAX: u64 = 8 << 20;
/// How the app's daemon, or the app's job before it, says in the log that it has no key.
const NO_KEY: &str = "no server key in voidfs.app for ";
/// The end of that line, which `daemon start` looks for.
const NOT_STARTING: &str = "sign in from its menu; not starting";
/// The connection variables a daemon `start` launches must not see: it reads `daemon.json`.
const CONNECTION_VARS: [&str; 5] = ["VOIDFS_ENDPOINT", "VOIDFS_ACCESS_KEY_ID", "VOIDFS_SECRET_ACCESS_KEY", "VOIDFS_KEY_FILE", "VOIDFS_REGION"];

/// How the daemon's queue cuts large files, for tests (hidden flags of `daemon run|start|restart`).
#[derive(clap::Args, Debug, Clone, Default)]
pub struct DaemonOptions {
    /// Files of this many MiB and more go up in parts
    #[arg(long, hide = true, value_name = "MIB")]
    pub multipart_threshold: Option<u64>,
    /// The size of each part, in MiB (at least 5)
    #[arg(long, hide = true, value_name = "MIB")]
    pub part_size: Option<u64>,
}

/// `daemon run`'s options.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct RunOptions {
    #[command(flatten)]
    pub daemon: DaemonOptions,
    /// voidfs.app's daemon, which launchd starts from the app's job (step 5, item 7): in the
    /// app's state directory, logging to its log, and taking over from a daemon started otherwise
    #[arg(long, hide = true)]
    pub app: bool,
    /// The app's daemon once its output goes to the log, in the state directory
    /// `VOIDFS_STATE_DIR` names. The app's job starts it so, with the server's key from the app's
    /// Keychain on stdin (`--key-file /dev/stdin`)
    #[arg(long, hide = true, requires = "app")]
    pub logged: bool,
}

impl DaemonOptions {
    fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(t) = self.multipart_threshold {
            a.extend(["--multipart-threshold".into(), t.to_string()]);
        }
        if let Some(p) = self.part_size {
            a.extend(["--part-size".into(), p.to_string()]);
        }
        a
    }
}

pub fn build() -> Build {
    Build { version: env!("CARGO_PKG_VERSION").into(), commit: env!("VOID_COMMIT").into(), commit_date: env!("VOID_COMMIT_DATE").into() }
}

/// The state directory and the daemon's socket in it.
pub struct Place {
    pub dir: PathBuf,
    pub client: DaemonClient,
}

impl Place {
    pub fn new() -> Result<Place> {
        let dir = voidfs_client::default_dir().ok_or_else(|| Failure::new("NoStateDirectory", "no state directory: set HOME, or VOIDFS_STATE_DIR"))?;
        let client = DaemonClient::new(&voidfs_daemon::socket_path(&dir));
        Ok(Place { dir, client })
    }

    pub fn socket(&self) -> &Path {
        self.client.socket()
    }

    /// The daemon's status, or `None` if nothing answers.
    pub async fn status(&self) -> Result<Option<api::Status>> {
        match self.client.status().await {
            Ok(s) => Ok(Some(s)),
            Err(ClientError::NotRunning { .. }) => Ok(None),
            Err(e) => Err(failure(e)),
        }
    }
}

pub fn failure(e: ClientError) -> Failure {
    match e {
        ClientError::NotRunning { socket, .. } => {
            Failure::new("DaemonNotRunning", format!("the daemon isn't running (socket {}): start it with `void daemon start`", socket.display())).with("socket", socket)
        }
        ClientError::Api { status, code, message } => Failure { status: Some(status), ..Failure::new(&code, message) },
        ClientError::Failed(m) => Failure::new("DaemonFailed", m),
    }
}

/// A line for the daemon's log: its stderr. A terminal that went away doesn't stop the daemon.
fn log(msg: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "{} {msg}", Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
}

/// Logs each file that went into conflict since the last look (a mount's save the drive refused
/// because it changed since), once, by drive and key, and again if it goes into conflict anew.
async fn log_conflicts(queue: &voidfs_client::Queue, logged: &mut std::collections::BTreeSet<(String, String)>) {
    let Ok(items) = queue.conflicts().await else { return };
    let now: std::collections::BTreeSet<(String, String)> = items.iter().map(|i| (i.drive.clone(), i.key.clone())).collect();
    logged.retain(|k| now.contains(k));
    for i in &items {
        if logged.insert((i.drive.clone(), i.key.clone())) {
            log(&format!("{} ({}:{}, entry {})", crate::uploads::conflict(i), i.drive, i.key, i.id));
        }
    }
}

/// The connection the daemon should use: the one given, else what `daemon.json` holds.
pub fn connection(conn: &Connection, dir: &Path) -> Result<voidfs_sdk::Config> {
    if let Some(c) = conn.config(|n| std::env::var(n).ok())? {
        return Ok(c);
    }
    match Settings::load(dir) {
        Ok(Some(s)) => Ok(s.config()),
        Ok(None) => Err(Failure::new(
            "NoCredentials",
            "no access key for the daemon: set VOIDFS_ACCESS_KEY_ID and VOIDFS_SECRET_ACCESS_KEY, or pass --key-file, the first time it starts",
        )),
        Err(e) => Err(Failure::local(&Settings::path(dir), e)),
    }
}

/// `daemon run --app`, as launchd starts it: its output goes to the log of the app's state
/// directory, which the job's plist, fixed in the app, can't name. It runs itself again on it, in
/// that directory, as the agent's plist gives its daemon both.
fn app_logged(opts: &RunOptions) -> Result<()> {
    use std::os::unix::process::CommandExt;
    if !cfg!(target_os = "macos") {
        return Err(Failure::new("NotSupported", "`daemon run --app` is voidfs.app's daemon, on macOS"));
    }
    let dir = crate::launchd::app_dir().ok_or_else(|| Failure::new("NoStateDirectory", "no state directory for the app's daemon: no HOME"))?;
    let log = open_log(&dir)?;
    let exe = std::env::current_exe().map_err(|e| Failure::new("InternalError", format!("finding the void binary: {e}")))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["daemon", "run", "--app", "--logged"]).args(opts.daemon.args()).env("VOIDFS_STATE_DIR", &dir);
    cmd.stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|e| Failure::local(&dir, e))?).stderr(log);
    Err(Failure::new("InternalError", format!("running the app's daemon: {}", cmd.exec())))
}

/// The app's daemon takes over (step 5, item 7): the terminal's agent for its state directory goes,
/// and a daemon started otherwise stops, so that one daemon, started one way, owns the directory.
async fn take_over(place: &Place) {
    if crate::launchd::installed_for(&place.dir) {
        match crate::launchd::remove_agent() {
            Ok(loaded) => log(&format!("voidfs.app starts the daemon at login now: removed {} ({})", crate::launchd::LABEL, if loaded { "it was loaded, and its daemon stopped" } else { "it wasn't loaded" })),
            Err(e) => log(&format!("removing {}: {}", crate::launchd::LABEL, e.message)),
        }
    }
    match stop_daemon(place).await {
        Ok(Some(pid)) => log(&format!("stopped the daemon that was running (pid {pid}), to take over")),
        Ok(None) => {}
        Err(e) => log(&format!("stopping the daemon that was running: {}", e.message)),
    }
    // A daemon launchd stopped may let go of the directory just after it answers no more.
    let deadline = Instant::now() + TAKE_OVER_WAIT;
    while !free(&place.dir) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `daemon run`: the daemon in the foreground, until a client or a signal stops it.
pub async fn run(conn: &Connection, opts: &RunOptions) -> Result<()> {
    if opts.app && !opts.logged {
        return app_logged(opts);
    }
    let place = Place::new()?;
    if opts.app {
        take_over(&place).await;
    }
    if let Some(s) = place.status().await? {
        return Err(Failure::new("DaemonRunning", format!("a daemon is already running on {} (pid {})", place.dir.display(), s.daemon.pid)));
    }
    let config = match connection(conn, &place.dir) {
        // Without a key, the app's daemon has nothing to run, and launchd shouldn't run it again
        // until it is asked: a sign-in in the app's menu keeps the key first.
        Err(f) if opts.app && f.code == "NoCredentials" => {
            log(&format!("{NO_KEY}{}: {NOT_STARTING}", place.dir.display()));
            return Ok(());
        }
        r => r?,
    };
    let opts = &opts.daemon;
    let endpoint = config.endpoint.clone();
    let mut cfg = DaemonConfig::new(&place.dir, config, build());
    // Mounting: FSKit, through voidfs.app's module (step 5, item 3).
    if cfg!(target_os = "macos") {
        cfg.adapters.push(std::sync::Arc::new(voidfs_daemon::FsKit::default()));
    }
    if let Some(t) = opts.multipart_threshold {
        cfg.queue.multipart_from = t << 20;
    }
    if let Some(p) = opts.part_size {
        cfg.queue.part_size = p.max(5) << 20;
    }
    let daemon = Daemon::start(cfg).await.map_err(|e| match e {
        voidfs_daemon::Error::Locked(_) => Failure::new("DaemonRunning", format!("{e}: `void daemon status` says which")),
        voidfs_daemon::Error::Socket(m) => Failure::new("SocketError", m),
        voidfs_daemon::Error::Client(e) => Failure::new("StateError", e.to_string()),
    })?;
    log(&format!("void {} daemon started: pid {}, socket {}, server {endpoint}", build(), std::process::id(), daemon.socket().display()));
    use tokio::signal::unix::{SignalKind, signal};
    let sig = |k: SignalKind| signal(k).map_err(|e| Failure::new("InternalError", format!("signals: {e}")));
    let (mut term, mut int, mut hup) = (sig(SignalKind::terminate())?, sig(SignalKind::interrupt())?, sig(SignalKind::hangup())?);
    let (mut look, mut logged) = (tokio::time::interval(CONFLICT_LOOK), std::collections::BTreeSet::new());
    let why = loop {
        tokio::select! {
            _ = daemon.stop_asked() => break "asked to stop",
            _ = term.recv() => break "SIGTERM",
            _ = int.recv() => break "SIGINT",
            // The terminal that started it went away: the daemon outlives it.
            _ = hup.recv() => log("SIGHUP: ignored"),
            _ = look.tick() => log_conflicts(daemon.queue(), &mut logged).await,
        }
    };
    log(&format!("stopping ({why})"));
    daemon.stop().await;
    log("stopped");
    Ok(())
}

/// Opens the log for a daemon about to start, moving one that has grown large aside.
fn open_log(dir: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = dir.join(voidfs_daemon::LOG);
    std::fs::create_dir_all(dir).map_err(|e| Failure::local(dir, e))?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::rename(&path, dir.join(format!("{}.1", voidfs_daemon::LOG)));
    }
    std::fs::File::options().create(true).append(true).mode(0o600).open(&path).map_err(|e| Failure::local(&path, e))
}

/// The log's length now, from where to look for what a daemon about to start says.
fn log_len(dir: &Path) -> u64 {
    std::fs::metadata(dir.join(voidfs_daemon::LOG)).map_or(0, |m| m.len())
}

/// The line the app's job, or its daemon, wrote to the log after `from` to say it has no key.
fn no_key(dir: &Path, from: u64) -> Option<String> {
    let text = std::fs::read(dir.join(voidfs_daemon::LOG)).ok()?;
    // A log moved aside as it started is new.
    let from = if (text.len() as u64) < from { 0 } else { from as usize };
    let text = String::from_utf8_lossy(&text[from..]);
    text.lines().find(|l| l.contains(NO_KEY) && l.ends_with(NOT_STARTING)).map(str::to_owned)
}

/// The last lines of the log, to say why a daemon didn't start.
fn log_tail(dir: &Path) -> String {
    let text = std::fs::read_to_string(dir.join(voidfs_daemon::LOG)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// Starts a daemon in the background unless one answers; returns its status, and whether it was
/// started now.
pub async fn ensure_started(conn: &Connection, opts: &DaemonOptions, place: &Place) -> Result<(api::Status, bool)> {
    if let Some(s) = place.status().await? {
        return Ok((s, false));
    }
    let login = crate::launchd::login(&place.dir);
    // voidfs.app keeps the key for its state directory in its Keychain, and hands it to the daemon
    // its job starts (step 5, item 7): nothing here writes one to disk for it.
    if login != Some(Login::App) {
        let config = connection(conn, &place.dir)?;
        Settings::of(&config).save(&place.dir).map_err(|e| Failure::local(&Settings::path(&place.dir), e))?;
    }
    // The job that starts it at login, the app's or the agent, so that launchd keeps the daemon
    // it starts.
    let from = log_len(&place.dir);
    if let Some(login) = login
        && crate::launchd::kickstart(login)
    {
        let deadline = Instant::now() + START_WAIT;
        loop {
            if let Some(s) = place.status().await? {
                return Ok((s, true));
            }
            if login == Login::App
                && let Some(line) = no_key(&place.dir, from)
            {
                return Err(Failure::new("NoCredentials", format!("voidfs.app has no server key for the daemon: sign in from its menu. Its log says:\n{line}")));
            }
            if Instant::now() > deadline {
                return Err(Failure::new("DaemonFailed", format!("launchd started {}, but its daemon didn't answer within {} s; its log:\n{}", login.label(), START_WAIT.as_secs(), log_tail(&place.dir))));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let log = open_log(&place.dir)?;
    let exe = std::env::current_exe().map_err(|e| Failure::new("InternalError", format!("finding the void binary: {e}")))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["daemon", "run"]).args(opts.args()).stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|e| Failure::local(&place.dir, e))?).stderr(log);
    // Its own process group, so that the terminal's Ctrl-C doesn't reach it.
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    for v in CONNECTION_VARS {
        cmd.env_remove(v);
    }
    let mut child = cmd.spawn().map_err(|e| Failure::new("InternalError", format!("starting the daemon: {e}")))?;
    let deadline = Instant::now() + START_WAIT;
    loop {
        if let Some(s) = place.status().await? {
            return Ok((s, true));
        }
        if let Ok(Some(exit)) = child.try_wait() {
            return Err(Failure::new("DaemonFailed", format!("the daemon exited ({exit}) as it started:\n{}", log_tail(&place.dir))));
        }
        if Instant::now() > deadline {
            return Err(Failure::new("DaemonFailed", format!("the daemon didn't answer within {} s; its log:\n{}", START_WAIT.as_secs(), log_tail(&place.dir))));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn start(conn: &Connection, opts: &DaemonOptions, out: Out) -> Result<()> {
    let place = Place::new()?;
    let (s, started) = ensure_started(conn, opts, &place).await?;
    let d = &s.daemon;
    let word = if started { "started" } else { "already running" };
    out.emit(&json!({ "running": true, "started": started, "pid": d.pid, "socket": d.socket }), || format!("daemon {word} (pid {}, socket {})", d.pid, d.socket))
}

/// No daemon holds state directory `dir`.
fn free(dir: &Path) -> bool {
    match std::fs::File::open(dir.join("lock")) {
        Ok(f) => f.try_lock().is_ok(),
        Err(_) => true,
    }
}

/// Asks the daemon to stop and waits until it has let go of the state directory. `None` if none
/// was running.
pub async fn stop_daemon(place: &Place) -> Result<Option<u32>> {
    let pid = match place.client.stop().await {
        Ok(s) => s.pid,
        Err(ClientError::NotRunning { .. }) => return Ok(None),
        Err(e) => return Err(failure(e)),
    };
    // The daemon removes its socket last, just before it lets go of the state directory; one that
    // died on the way lets go of the directory and leaves its socket.
    let deadline = Instant::now() + STOP_WAIT;
    while place.socket().exists() && !free(&place.dir) {
        if let Ok(s) = place.client.status().await
            && s.daemon.pid != pid
        {
            break;
        }
        if Instant::now() > deadline {
            return Err(Failure::new("DaemonFailed", format!("the daemon (pid {pid}) is still stopping after {} s", STOP_WAIT.as_secs())));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(Some(pid))
}

pub async fn stop(out: Out) -> Result<()> {
    let place = Place::new()?;
    match stop_daemon(&place).await? {
        Some(pid) => out.emit(&json!({ "running": false, "stopped": true, "pid": pid }), || format!("daemon stopped (pid {pid})")),
        None => out.emit(&json!({ "running": false, "stopped": false, "socket": place.socket() }), || format!("daemon not running (socket {})", place.socket().display())),
    }
}

pub async fn restart(conn: &Connection, opts: &DaemonOptions, out: Out) -> Result<()> {
    let place = Place::new()?;
    let old = stop_daemon(&place).await?;
    let (s, _) = ensure_started(conn, opts, &place).await?;
    let d = &s.daemon;
    out.emit(&json!({ "running": true, "stoppedPid": old, "pid": d.pid, "socket": d.socket }), || format!("daemon restarted (pid {}, socket {})", d.pid, d.socket))
}

/// `3d 4h`, `2h 5m`, `3m 2s`, `5s`.
pub fn uptime(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m {s}s"),
        (0, _, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

/// Who starts the daemon at login, for `daemon status`: a line on macOS.
fn login_line(login: Option<Login>) -> String {
    match login {
        _ if !cfg!(target_os = "macos") => String::new(),
        Some(l) => format!("\nstarts at login: {}", l.describe()),
        None => "\ndoesn't start at login: `void daemon install`, or Start at login in voidfs.app's menu".into(),
    }
}

pub async fn daemon_status(out: Out) -> Result<()> {
    let place = Place::new()?;
    let login = crate::launchd::login(&place.dir);
    let label = login.map(Login::label);
    match place.status().await? {
        Some(s) => {
            let d = &s.daemon;
            let mut v = serde_json::to_value(d).map_err(|e| Failure::new("InternalError", e.to_string()))?;
            v["loginJob"] = json!(label);
            out.emit(&v, || format!("daemon running · pid {} · up {} · {} · socket {}{}", d.pid, uptime(d.uptime_secs), count(s.mounts.len(), "mount"), d.socket, login_line(login)))
        }
        None => out.emit(&json!({ "running": false, "socket": place.socket(), "loginJob": label }), || {
            format!("daemon not running (socket {}){}", place.socket().display(), login_line(login))
        }),
    }
}

pub async fn info(out: Out) -> Result<()> {
    let place = Place::new()?;
    let i = place.client.info().await.map_err(failure)?;
    out.emit(&i, || {
        let j = &i.journal;
        let rows = [
            ("version", i.build.to_string()),
            ("pid", i.pid.to_string()),
            ("uptime", uptime(i.uptime_secs)),
            ("socket", i.socket.clone()),
            ("state", i.state_dir.clone()),
            ("mounts", i.mounts.to_string()),
            ("restart safe", if i.restart_safe { "yes".into() } else { format!("NO: {} only in memory", size(i.memory_only_bytes)) }),
            ("journal", format!("{} unpublished, {} to send, all of it on disk", count(j.unpublished as usize, "change"), size(j.unpublished_bytes))),
            ("uploading", format!("{} now: a restart sends a file again, or a large one from its last part", j.uploading)),
            ("imports", format!("{} waiting, read from where they are: leave them there until they are up", j.imports)),
        ];
        rows.iter().map(|(k, v)| format!("{k:<14}{v}")).collect::<Vec<_>>().join("\n")
    })
}

/// What the journal holds unpublished while no daemon runs, if its state can be read: its
/// changes to send and their bytes, and the files with a change kept on this Mac.
fn waiting(dir: &Path) -> Option<(u64, u64, u64)> {
    if !dir.join("state.sqlite").exists() {
        return None;
    }
    let store = voidfs_client::Store::open(dir).ok()?;
    let (n, bytes) = voidfs_client::queue::unpublished(&store).ok()?;
    Some((n, bytes, voidfs_client::queue::kept(&store).ok()?))
}

/// Files with a mount change whose upload was cancelled (step 5, item 8).
pub fn kept(files: u64) -> String {
    format!("{} kept on this Mac, not uploaded", count(files as usize, "file"))
}

/// The uploads line of `status`.
fn uploads_line(u: &api::Uploads) -> String {
    if u.unpublished == 0 {
        let idle = if u.paused { "idle · paused" } else { "idle" };
        return if u.kept > 0 { format!("{idle} · {}", kept(u.kept)) } else { idle.into() };
    }
    let mut parts = Vec::new();
    for (n, what) in [(u.uploading, "uploading"), (u.queued, "queued"), (u.failed, "failed"), (u.conflicts, "in conflict")] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    let mut line = format!("{} · {} to send", parts.join(", "), size(u.unpublished_bytes));
    if u.rate > 0 {
        line.push_str(&format!(" · {}/s", size(u.rate)));
    }
    if u.paused {
        line.push_str(" · paused");
    } else if u.paused_items > 0 {
        line.push_str(&format!(" · {} paused", u.paused_items));
    }
    if !u.paused_drives.is_empty() {
        line.push_str(&format!(" · paused on {}", u.paused_drives.join(", ")));
    }
    if let Some(b) = u.bandwidth {
        line.push_str(&format!(" · limit {}/s", size(b)));
    }
    if u.kept > 0 {
        line.push_str(&format!(" · {}", kept(u.kept)));
    }
    line
}

pub async fn status(out: Out) -> Result<()> {
    let place = Place::new()?;
    let Some(s) = place.status().await? else {
        let socket = place.socket().display();
        let waiting = waiting(&place.dir);
        let mut v = json!({ "daemon": { "running": false, "socket": place.socket() } });
        if let Some((n, bytes, kept)) = waiting {
            v["uploads"] = json!({ "unpublished": n, "unpublishedBytes": bytes, "kept": kept });
        }
        return out.emit(&v, || {
            let mut text = format!("daemon    not running (socket {socket})\n          start it with `void daemon start`");
            match waiting {
                Some((n, bytes, k)) if n > 0 => {
                    text.push_str(&format!("\nuploads   {} ({}) wait in the journal, for the daemon to start", count(n as usize, "change"), size(bytes)));
                    if k > 0 { text.push_str(&format!(" · {}", kept(k))); }
                }
                Some((_, _, k)) if k > 0 => text.push_str(&format!("\nuploads   {}", kept(k))),
                _ => {}
            }
            text
        });
    };
    out.emit(&s, || {
        let d = &s.daemon;
        let c = &s.cache;
        let link = match s.connection.link {
            api::Link::Online => "online",
            api::Link::Degraded => "degraded: errors or slow answers",
            api::Link::Offline => "offline: uploads wait, and reads of what isn't cached fail",
        };
        let rows = [
            ("daemon", format!("running · pid {} · up {} · socket {}", d.pid, uptime(d.uptime_secs), d.socket)),
            ("server", format!("{} · {link}", s.connection.endpoint)),
            ("uploads", uploads_line(&s.uploads)),
            ("cache", format!("{} of {} on disk · {} of {} in memory", size(c.disk_bytes), size(c.max_bytes), size(c.memory_bytes), size(c.memory_max_bytes))),
        ];
        let mut lines: Vec<String> = rows.iter().map(|(k, v)| format!("{k:<10}{v}")).collect();
        if s.mounts.is_empty() {
            lines.push(format!("{:<10}none", "mounts"));
        }
        for (i, m) in s.mounts.iter().enumerate() {
            lines.push(format!("{:<10}{}", if i == 0 { "mounts" } else { "" }, crate::mounts::line(m)));
        }
        lines.join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptimes_read_as_people_say_them() {
        assert_eq!(uptime(5), "5s");
        assert_eq!(uptime(62), "1m 2s");
        assert_eq!(uptime(2 * 3600 + 5 * 60 + 9), "2h 5m");
        assert_eq!(uptime(3 * 86400 + 4 * 3600 + 1), "3d 4h");
    }

    #[test]
    fn the_uploads_line_says_what_is_left() {
        let mut u = api::Uploads::default();
        assert_eq!(uptime(0), "0s");
        assert_eq!(uploads_line(&u), "idle");
        u.paused = true;
        assert_eq!(uploads_line(&u), "idle · paused");
        u = api::Uploads { uploading: 2, queued: 3, unpublished: 5, unpublished_bytes: 3 << 20, bandwidth: Some(1 << 20), paused_items: 1, ..Default::default() };
        assert_eq!(uploads_line(&u), "2 uploading, 3 queued · 3.0 MiB to send · 1 paused · limit 1.0 MiB/s");
        u = api::Uploads { failed: 1, conflicts: 2, unpublished: 5, unpublished_bytes: 30, ..Default::default() };
        assert_eq!(uploads_line(&u), "1 failed, 2 in conflict · 30 B to send");
        u.kept = 1;
        assert_eq!(uploads_line(&u), "1 failed, 2 in conflict · 30 B to send · 1 file kept on this Mac, not uploaded");
        u = api::Uploads { kept: 2, ..Default::default() };
        assert_eq!(uploads_line(&u), "idle · 2 files kept on this Mac, not uploaded");
    }
}
