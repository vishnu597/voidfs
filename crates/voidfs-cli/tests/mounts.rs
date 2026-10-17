// SPDX-License-Identifier: Apache-2.0
//! `void mount|unmount|mounts` against a daemon started in this process on a socket of its own,
//! with a fake adapter (the binary has none until step 5); and `void daemon install|uninstall` and
//! the app's daemon, with the plist in a temporary folder and stand-ins for launchctl and defaults.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use voidfs_daemon::api::Build;
use voidfs_daemon::{Adapter, Core, Daemon, DaemonConfig, MountSpec, Mounted};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET};

mod common;
use common::*;

/// Mounts by leaving a marker in the mountpoint.
struct Fake;

struct FakeMount(PathBuf);

impl Adapter for Fake {
    fn name(&self) -> &str {
        "fake"
    }

    fn mount<'a>(&'a self, spec: &'a MountSpec, _core: &'a Core) -> BoxFuture<'a, Result<Arc<dyn Mounted>, String>> {
        Box::pin(async move {
            let marker = spec.mountpoint.join(".mounted");
            std::fs::write(&marker, &spec.drive).map_err(|e| e.to_string())?;
            Ok(Arc::new(FakeMount(marker)) as Arc<dyn Mounted>)
        })
    }
}

impl Mounted for FakeMount {
    fn unmount(self: Arc<Self>) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move { std::fs::remove_file(&self.0).map_err(|e| e.to_string()) })
    }
}

/// A daemon with the fake adapter, in this process, on `s`'s state directory.
fn daemon(f: &Fixture, s: &State) -> Daemon {
    let sdk = voidfs_sdk::Config { endpoint: f.endpoint.clone(), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..voidfs_sdk::Config::default() };
    let cfg = DaemonConfig { adapters: vec![Arc::new(Fake)], ..DaemonConfig::new(&s.dir, sdk, Build::default()) };
    f.block(Daemon::start(cfg)).unwrap()
}

fn stop(f: &Fixture, d: Daemon) {
    f.block(d.stop());
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn mounts_through_the_daemon_are_remembered_until_unmounted() {
    let f = Fixture::new("mounts");
    f.drive("footage");
    let s = State::new(&f, "m");
    let mp = s.dir.with_extension("mnt");
    let _ = std::fs::remove_dir_all(&mp);
    let d = daemon(&f, &s);

    let v = s.void(&["mounts", "--json"]).ok().json();
    assert_eq!((v["daemon"]["running"].as_bool(), v["mounts"].as_array().map(Vec::len), v["remembered"].as_array().map(Vec::len)), (Some(true), Some(0), Some(0)));
    assert!(s.void(&["mounts"]).ok().out().starts_with("no drives mounted"));

    let m = s.void(&["mount", "footage", path(&mp), "--json"]).ok().json();
    assert_eq!((m["drive"].as_str(), m["mountpoint"].as_str(), m["adapter"].as_str(), m["state"].as_str()), (Some("footage"), Some(path(&mp)), Some("fake"), Some("mounted")));
    assert!(mp.join(".mounted").exists());
    let table = lines(&s.void(&["mounts"]).ok().out());
    assert_eq!(table[0], "DRIVE MOUNTPOINT ADAPTER STATE SINCE FEED");
    assert!(table[1].starts_with(&format!("footage {} fake mounted ", mp.display())), "{table:?}");
    let page = lines(&s.void(&["status"]).ok().out());
    assert!(page.iter().any(|l| l.starts_with(&format!("mounts footage at {} · fake · mounted · feed at ", mp.display()))), "{page:?}");
    assert!(s.void(&["daemon", "status"]).ok().out().contains(" · 1 mount · "));
    assert_eq!(s.void(&["daemon", "info", "--json"]).ok().json()["mounts"], 1);
    assert_eq!(s.void(&["mount", "footage", path(&mp), "--json"]).fails(1).error()["code"], "AlreadyMounted");

    // The daemon stopped: nothing is mounted, and what comes back is said.
    stop(&f, d);
    assert!(!mp.join(".mounted").exists());
    let v = s.void(&["mounts", "--json"]).ok().json();
    assert_eq!((v["daemon"]["running"].as_bool(), v["mounts"].as_array().map(Vec::len)), (Some(false), Some(0)));
    assert_eq!(v["remembered"][0]["mountpoint"].as_str(), Some(path(&mp)));
    let text = s.void(&["mounts"]).ok().out();
    assert!(text.contains("remembered, mounted again when the daemon starts:") && text.contains(&format!("footage  {}  fake", mp.display())), "{text}");

    let d = daemon(&f, &s);
    let deadline = Instant::now() + Duration::from_secs(20);
    while s.void(&["mounts", "--json"]).ok().json()["mounts"][0]["state"] != "mounted" {
        assert!(Instant::now() < deadline, "the remembered mount didn't come back");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(mp.join(".mounted").exists());
    let text = s.void(&["unmount", path(&mp)]).ok().out();
    assert_eq!(text.trim(), format!("unmounted {}, and forgot it", mp.display()));
    let v = s.void(&["mounts", "--json"]).ok().json();
    assert!(v["mounts"].as_array().unwrap().is_empty() && v["remembered"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(s.void(&["unmount", "footage", "--json"]).fails(1).error()["code"], "NoSuchMount");
    stop(&f, d);
    let _ = std::fs::remove_dir_all(&mp);
}

#[test]
fn void_mounts_with_fskit_on_macos_and_has_no_adapter_elsewhere() {
    let f = Fixture::new("noadapter");
    f.drive("footage");
    let s = State::new(&f, "a");
    // `mount` starts the daemon, as Space's does. An adapter it lacks is refused, naming what it
    // has: FSKit on macOS (not run here: it would mount for real), nothing elsewhere.
    let e = s.void(&["mount", "footage", path(&s.dir.join("m")), "--adapter", "smb", "--json"]).fails(1).error();
    assert_eq!((e["code"].as_str(), e["status"].as_u64()), (Some("NoAdapter"), Some(501)), "{e}");
    let message = e["message"].as_str().unwrap();
    if cfg!(target_os = "macos") {
        assert_eq!(message, "the adapters here are fskit");
    } else {
        assert!(message.contains("step 5"), "{message}");
        let e = s.void(&["mount", "footage", path(&s.dir.join("m")), "--json"]).fails(1).error();
        assert_eq!((e["code"].as_str(), e["status"].as_u64()), (Some("NoAdapter"), Some(501)), "{e}");
    }
    assert_eq!(s.void(&["daemon", "status", "--json"]).ok().json()["running"], true);
    assert!(s.void(&["mounts", "--json"]).ok().json()["remembered"].as_array().unwrap().is_empty());
    assert!(lines(&s.void(&["status"]).ok().out()).contains(&"mounts none".to_owned()));
}

/// voidfs.app's daemon (step 5, item 7), run as launchd runs the app's job: it takes over from the
/// terminal's agent and from a daemon started by hand, in the app's state directory, and logs
/// there; while the app's job is loaded `daemon install` refuses and `daemon start` asks launchd
/// for the app's. The stand-ins for launchctl and defaults are in a temporary folder.
#[cfg(target_os = "macos")]
#[test]
fn the_apps_daemon_takes_over_the_state_directory() {
    let f = Fixture::new("appdaemon");
    let s = State::new(&f, "app");
    let agents = s.dir.with_extension("agents");
    let _ = std::fs::remove_dir_all(&agents);
    std::fs::create_dir_all(&agents).unwrap();
    let log = agents.join("launchctl.log");
    let fake = agents.join("launchctl");
    // As launchctl, for both jobs: print succeeds for a loaded job, bootstrap loads the agent and
    // runs it, kickstart runs a loaded job and bootout stops and unloads one.
    let script = r#"#!/bin/sh
echo "$@" >> 'LOG'
job="${2##*/}"
case "$1" in
  print) [ -e "DIR/$job.loaded" ] || exit 113 ;;
  bootstrap) touch 'DIR/dev.voidfs.daemon.loaded'; 'VOID' daemon run </dev/null >/dev/null 2>&1 & ;;
  kickstart) [ -e "DIR/$job.loaded" ] || exit 113
    if [ "$job" = dev.voidfs.app.daemon ]; then 'VOID' daemon run --app </dev/null >/dev/null 2>&1 &
    else 'VOID' daemon run </dev/null >/dev/null 2>&1 & fi ;;
  bootout) [ -e "DIR/$job.loaded" ] || exit 3; rm "DIR/$job.loaded"; 'VOID' daemon stop >/dev/null 2>&1 ;;
esac
exit 0
"#;
    let script = script.replace("LOG", path(&log)).replace("DIR", path(&agents)).replace("VOID", env!("CARGO_BIN_EXE_void"));
    std::fs::write(&fake, script).unwrap();
    // As defaults: the app's VoidfsStateDirectory is this test's.
    let defaults = agents.join("defaults");
    std::fs::write(&defaults, format!("#!/bin/sh\n[ \"$*\" = 'read dev.voidfs.app VoidfsStateDirectory' ] || exit 1\necho '{}'\n", s.dir.display())).unwrap();
    for f in [&fake, &defaults] {
        std::fs::set_permissions(f, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    let mut env = s.env.clone();
    env.push(("VOIDFS_LAUNCH_AGENTS_DIR", path(&agents).into()));
    env.push(("VOIDFS_LAUNCHCTL", path(&fake).into()));
    env.push(("VOIDFS_DEFAULTS", path(&defaults).into()));
    let void = |args: &[&str]| run(args, &env, None);
    let uid = String::from_utf8(std::process::Command::new("id").arg("-u").output().unwrap().stdout).unwrap().trim().to_owned();
    let status = || void(&["daemon", "status", "--json"]).ok().json();
    let answers = |pid: u32| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while status()["pid"].as_u64() != Some(pid.into()) {
            assert!(Instant::now() < deadline, "pid {pid} didn't answer: {}", std::fs::read_to_string(s.dir.join("daemon.log")).unwrap_or_default());
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // launchd's environment has neither the state directory nor the connection: the app's daemon
    // finds the one in the app's defaults, and the other in its daemon.json.
    let launchd: Vec<(&str, String)> = env.iter().filter(|(k, _)| !k.starts_with("VOIDFS_") || matches!(*k, "VOIDFS_LAUNCH_AGENTS_DIR" | "VOIDFS_LAUNCHCTL" | "VOIDFS_DEFAULTS")).cloned().collect();
    let app = || {
        std::fs::write(agents.join("dev.voidfs.app.daemon.loaded"), "").unwrap();
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_void"));
        cmd.args(["daemon", "run", "--app"]).env_clear().envs(launchd.iter().cloned()).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        cmd.spawn().unwrap()
    };

    // The terminal's agent runs the daemon.
    void(&["daemon", "install"]).ok();
    let plist = agents.join("dev.voidfs.daemon.plist");
    assert!(plist.exists());
    let agents_pid = status()["pid"].as_u64().unwrap();
    assert_eq!(status()["loginJob"], "dev.voidfs.daemon");

    // SMAppService registers the app's job, and launchd runs it: the agent goes, its daemon stops,
    // and the app's answers, in the process launchd started, with its output in the log.
    let mut child = app();
    answers(child.id());
    assert!(!plist.exists(), "the agent is removed");
    assert_ne!(Some(child.id().into()), Some(agents_pid));
    assert_eq!(status()["loginJob"], "dev.voidfs.app.daemon");
    let text = std::fs::read_to_string(s.dir.join("daemon.log")).unwrap();
    assert!(text.contains("voidfs.app starts the daemon at login now: removed dev.voidfs.daemon (it was loaded, and its daemon stopped)"), "{text}");
    assert!(text.contains(&format!("daemon started: pid {}, socket {}", child.id(), s.socket().display())), "{text}");

    // While the app has it, `daemon install` refuses, and `daemon status` says who starts it.
    let e = void(&["daemon", "install", "--json"]).fails(1).error();
    assert_eq!(e["code"], "ManagedByApp", "{e}");
    assert!(!plist.exists());
    let text = void(&["daemon", "status"]).ok().out();
    assert!(text.contains("starts at login: voidfs.app (dev.voidfs.app.daemon, Start at login in its menu)"), "{text}");
    assert!(void(&["daemon", "uninstall"]).ok().out().contains("wasn't installed; voidfs.app starts it (dev.voidfs.app.daemon)"));

    // Stopped as asked, it exits 0, which launchd leaves stopped; `daemon start` asks launchd.
    void(&["daemon", "stop"]).ok();
    assert!(child.wait().unwrap().success());
    assert_eq!(void(&["daemon", "start", "--json"]).ok().json()["started"], true);
    assert_eq!(status()["running"], true);
    void(&["daemon", "stop"]).ok();

    // A daemon started by hand makes way for the app's at the next login.
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_void"));
    cmd.args(["daemon", "run"]).env_clear().envs(env.iter().cloned()).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut hand = cmd.spawn().unwrap();
    answers(hand.id());
    let mut child = app();
    answers(child.id());
    assert!(hand.wait().unwrap().success(), "the daemon started by hand stopped as asked");
    let text = std::fs::read_to_string(s.dir.join("daemon.log")).unwrap();
    assert!(text.contains(&format!("stopped the daemon that was running (pid {}), to take over", hand.id())), "{text}");
    void(&["daemon", "stop"]).ok();
    assert!(child.wait().unwrap().success());

    // `print` asks whether the app's job is loaded, as every status does.
    let calls: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().filter(|l| !l.starts_with("print ")).map(str::to_owned).collect();
    let (agent, app_job) = (format!("gui/{uid}/dev.voidfs.daemon"), format!("gui/{uid}/dev.voidfs.app.daemon"));
    let (bootout, bootstrap) = (format!("bootout {agent}"), format!("bootstrap gui/{uid} {}", plist.display()));
    // Install's, the app's daemon's, uninstall's.
    assert_eq!(calls, [bootout.clone(), bootstrap, bootout.clone(), bootout, format!("kickstart {app_job}")]);
    let _ = std::fs::remove_dir_all(&agents);
}

/// The app's job hands its daemon the server's key from the app's Keychain on stdin (step 5,
/// item 7). With the app's job loaded, `daemon start` writes no `daemon.json`, whatever key the
/// terminal has, and says at once when the app has none. A stand-in for launchctl runs the job as
/// the app's launcher does: from launchd's empty environment, with the key on a pipe if the
/// stand-in Keychain, a file here, has one, and with its output in the log.
#[cfg(target_os = "macos")]
#[test]
fn the_apps_daemon_takes_its_key_from_the_app() {
    let f = Fixture::new("appkey");
    let s = State::new(&f, "key");
    let agents = s.dir.with_extension("agents");
    let _ = std::fs::remove_dir_all(&agents);
    std::fs::create_dir_all(&agents).unwrap();
    let (log, fake, defaults, keychain) = (agents.join("launchctl.log"), agents.join("launchctl"), agents.join("defaults"), agents.join("keychain.json"));
    let script = r#"#!/bin/sh
echo "$@" >> '@LOG@'
job="${2##*/}"
launchd="/usr/bin/env -i HOME=$HOME VOIDFS_LAUNCH_AGENTS_DIR=@DIR@ VOIDFS_LAUNCHCTL=@DIR@/launchctl VOIDFS_DEFAULTS=@DIR@/defaults"
case "$1" in
  print) [ -e "@DIR@/$job.loaded" ] || exit 113 ;;
  kickstart) [ -e "@DIR@/$job.loaded" ] || exit 113
    if [ -e '@DIR@/keychain.json' ]; then
      cat '@DIR@/keychain.json' | $launchd VOIDFS_STATE_DIR='@STATE@' VOIDFS_REGION=us-east-1 '@VOID@' daemon run --app --logged --endpoint '@ENDPOINT@' --key-file /dev/stdin >> '@STATE@/daemon.log' 2>&1 &
    else
      $launchd '@VOID@' daemon run --app </dev/null >/dev/null 2>&1 &
    fi ;;
esac
exit 0
"#;
    let script = script.replace("@LOG@", path(&log)).replace("@DIR@", path(&agents)).replace("@STATE@", path(&s.dir)).replace("@ENDPOINT@", &f.endpoint).replace("@VOID@", env!("CARGO_BIN_EXE_void"));
    std::fs::write(&fake, script).unwrap();
    std::fs::write(&defaults, format!("#!/bin/sh\n[ \"$*\" = 'read dev.voidfs.app VoidfsStateDirectory' ] || exit 1\necho '{}'\n", s.dir.display())).unwrap();
    for f in [&fake, &defaults] {
        std::fs::set_permissions(f, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    let tools = [("VOIDFS_LAUNCH_AGENTS_DIR", path(&agents).to_owned()), ("VOIDFS_LAUNCHCTL", path(&fake).to_owned()), ("VOIDFS_DEFAULTS", path(&defaults).to_owned())];
    // The terminal has a key of its own, and the state directory.
    let mut env = s.env.clone();
    env.extend(tools.iter().cloned());
    let mut bare = s.bare();
    bare.extend(tools.iter().cloned());
    std::fs::create_dir_all(&s.dir).unwrap();
    std::fs::write(agents.join("dev.voidfs.app.daemon.loaded"), "").unwrap();
    let settings = s.dir.join("daemon.json");

    // The app has no key: the job's daemon says so in the log and stops, and `daemon start` says
    // so as soon as it has, without keeping the terminal's key.
    let started = Instant::now();
    let e = run(&["daemon", "start", "--json"], &env, None).fails(1).error();
    assert_eq!(e["code"], "NoCredentials", "{e}");
    let message = e["message"].as_str().unwrap();
    assert!(message.starts_with("voidfs.app has no server key for the daemon: sign in from its menu"), "{message}");
    assert!(message.contains(&format!("no server key in voidfs.app for {}: sign in from its menu; not starting", s.dir.display())), "{message}");
    assert!(started.elapsed() < Duration::from_secs(10), "it waited {:?}", started.elapsed());
    assert!(!settings.exists(), "the terminal's key isn't written for the app's directory");

    // The app has one: its daemon starts with it, from no file.
    std::fs::write(&keychain, format!(r#"{{"accessKeyId": "{ADMIN_KEY_ID}", "secretAccessKey": "{ADMIN_SECRET}"}}"#)).unwrap();
    let j = run(&["daemon", "start", "--json"], &bare, None).ok().json();
    assert_eq!(j["started"], true, "{j}");
    let status = run(&["daemon", "status", "--json"], &bare, None).ok().json();
    assert_eq!((status["running"].as_bool(), status["loginJob"].as_str()), (Some(true), Some("dev.voidfs.app.daemon")), "{status}");
    let page = run(&["status", "--json"], &bare, None).ok().json();
    assert_eq!(page["connection"]["endpoint"], f.endpoint.as_str(), "{page}");
    assert!(!settings.exists(), "nothing wrote a daemon.json");
    let text = std::fs::read_to_string(s.dir.join("daemon.log")).unwrap();
    assert!(text.contains(&format!("pid {}, socket {}, server {}", j["pid"], s.socket().display(), f.endpoint)), "{text}");
    assert!(!text.contains(ADMIN_SECRET), "the key isn't logged");
    run(&["daemon", "stop"], &bare, None).ok();

    let uid = String::from_utf8(std::process::Command::new("id").arg("-u").output().unwrap().stdout).unwrap().trim().to_owned();
    let calls: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().filter(|l| !l.starts_with("print ")).map(str::to_owned).collect();
    assert_eq!(calls, [format!("kickstart gui/{uid}/dev.voidfs.app.daemon"), format!("kickstart gui/{uid}/dev.voidfs.app.daemon")]);
    let _ = std::fs::remove_dir_all(&agents);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn daemon_install_is_for_macos_until_step_9() {
    let f = Fixture::new("install");
    let s = State::new(&f, "i");
    let e = s.void(&["daemon", "install", "--json"]).fails(1).error();
    assert_eq!(e["code"], "NotSupported", "{e}");
}

/// The launchd agent, written to a temporary folder, loaded by a stand-in for launchctl that
/// logs what it is asked and runs the daemon as launchd would.
#[cfg(target_os = "macos")]
#[test]
fn daemon_install_writes_a_launchd_agent() {
    let f = Fixture::new("install");
    let s = State::new(&f, "i");
    let agents = s.dir.with_extension("agents");
    let _ = std::fs::remove_dir_all(&agents);
    std::fs::create_dir_all(&agents).unwrap();
    let log = agents.join("launchctl.log");
    let fake = agents.join("launchctl");
    // As launchctl: bootstrap loads the job and runs it (RunAtLoad), kickstart runs a loaded
    // job, and bootout stops and unloads it; each fails for a job that isn't loaded.
    let script = r#"#!/bin/sh
echo "$@" >> 'LOG'
case "$1" in
  bootstrap) touch 'LOADED'; 'VOID' daemon run </dev/null >/dev/null 2>&1 & ;;
  kickstart) [ -e 'LOADED' ] || exit 113; 'VOID' daemon run </dev/null >/dev/null 2>&1 & ;;
  bootout) [ -e 'LOADED' ] || exit 3; rm 'LOADED'; 'VOID' daemon stop >/dev/null 2>&1 ;;
esac
exit 0
"#;
    let script = script.replace("LOG", path(&log)).replace("LOADED", path(&agents.join("loaded"))).replace("VOID", env!("CARGO_BIN_EXE_void"));
    std::fs::write(&fake, script).unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut env = s.env.clone();
    env.push(("VOIDFS_LAUNCH_AGENTS_DIR", agents.display().to_string()));
    env.push(("VOIDFS_LAUNCHCTL", fake.display().to_string()));
    let void = |args: &[&str]| run(args, &env, None);
    let uid = String::from_utf8(std::process::Command::new("id").arg("-u").output().unwrap().stdout).unwrap().trim().to_owned();

    // A daemon started by hand makes way for launchd's.
    void(&["daemon", "start"]).ok();
    let v = void(&["daemon", "install", "--json"]).ok().json();
    let plist = agents.join("dev.voidfs.daemon.plist");
    assert_eq!((v["installed"].as_bool(), v["plist"].as_str(), v["label"].as_str()), (Some(true), Some(path(&plist)), Some("dev.voidfs.daemon")));
    assert!(v["stoppedPid"].as_u64().is_some(), "{v}");
    assert_eq!(void(&["daemon", "status", "--json"]).ok().json()["pid"].as_u64(), v["pid"].as_u64(), "launchd's daemon answers");
    assert!(std::process::Command::new("plutil").args(["-lint", path(&plist)]).stdout(std::process::Stdio::null()).status().unwrap().success(), "a valid plist");
    let text = std::fs::read_to_string(&plist).unwrap();
    assert!(text.contains(&format!("<string>{}</string>\n        <string>daemon</string>\n        <string>run</string>", env!("CARGO_BIN_EXE_void"))), "{text}");
    assert!(text.contains(&format!("<key>VOIDFS_STATE_DIR</key>\n        <string>{}</string>", s.dir.display())), "{text}");
    assert!(text.contains(&format!("<key>StandardErrorPath</key>\n    <string>{}</string>", s.dir.join("daemon.log").display())));
    assert!(!text.contains("SECRET") && !text.contains(ADMIN_SECRET), "no secret in the plist");
    assert!(s.dir.join("daemon.json").exists(), "the key is kept for the agent's daemon");

    // Installed, `daemon start` asks launchd.
    void(&["daemon", "stop"]).ok();
    void(&["daemon", "start"]).ok();
    assert_eq!(void(&["daemon", "status", "--json"]).ok().json()["running"], true);

    let v = void(&["daemon", "uninstall", "--json"]).ok().json();
    assert_eq!((v["installed"].as_bool(), v["removed"].as_bool()), (Some(false), Some(true)));
    assert!(!plist.exists());
    assert_eq!(void(&["daemon", "status", "--json"]).ok().json()["running"], false, "launchd stopped its daemon");
    assert_eq!(void(&["daemon", "uninstall"]).ok().out().trim(), "dev.voidfs.daemon wasn't installed");
    let calls: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(str::to_owned).collect();
    let job = format!("gui/{uid}/dev.voidfs.daemon");
    assert_eq!(calls, [format!("bootout {job}"), format!("bootstrap gui/{uid} {}", plist.display()), format!("kickstart {job}"), format!("bootout {job}"), format!("bootout {job}")]);
    let _ = std::fs::remove_dir_all(&agents);
}
