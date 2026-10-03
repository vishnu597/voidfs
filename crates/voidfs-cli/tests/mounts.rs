// SPDX-License-Identifier: Apache-2.0
//! `void mount|unmount|mounts` against a daemon started in this process on a socket of its own,
//! with a fake adapter (the binary has none until step 5); and `void daemon install|uninstall`,
//! with the plist in a temporary folder and a stand-in for launchctl.

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
fn void_has_no_adapter_until_step_5() {
    let f = Fixture::new("noadapter");
    f.drive("footage");
    let s = State::new(&f, "a");
    // `mount` starts the daemon, as Space's does, which has nothing to mount with.
    let e = s.void(&["mount", "footage", path(&s.dir.join("m")), "--json"]).fails(1).error();
    assert_eq!((e["code"].as_str(), e["status"].as_u64()), (Some("NoAdapter"), Some(501)), "{e}");
    assert!(e["message"].as_str().unwrap().contains("step 5"));
    assert_eq!(s.void(&["daemon", "status", "--json"]).ok().json()["running"], true);
    assert!(s.void(&["mounts", "--json"]).ok().json()["remembered"].as_array().unwrap().is_empty());
    assert!(lines(&s.void(&["status"]).ok().out()).contains(&"mounts none".to_owned()));
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
