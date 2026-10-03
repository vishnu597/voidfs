// SPDX-License-Identifier: Apache-2.0
//! `void daemon …` and `void status`: the binary starting, asking and stopping a daemon on a state
//! directory of the test's own, against a server in this process.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use voidfs_server::test_server::ADMIN_KEY_ID;

mod common;
use common::*;

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_daemon_starts_answers_and_stops() {
    let f = Fixture::new("daemon");
    let s = State::new(&f, "a");
    let socket = s.socket().display().to_string();

    let v = s.void(&["status", "--json"]).ok().json();
    assert_eq!(v, serde_json::json!({ "daemon": { "running": false, "socket": socket } }), "as SpaceFS's says it");
    assert_eq!(lines(&s.void(&["status"]).ok().out())[0], format!("daemon not running (socket {socket})"));
    assert_eq!(s.void(&["daemon", "status", "--json"]).ok().json()["running"], false);
    let e = s.void(&["daemon", "info", "--json"]).fails(1).error();
    assert_eq!(e["code"], "DaemonNotRunning", "{e}");

    let v = s.void(&["daemon", "start", "--json"]).ok().json();
    assert_eq!((v["running"].as_bool(), v["started"].as_bool(), v["socket"].as_str()), (Some(true), Some(true), Some(socket.as_str())));
    let pid = v["pid"].as_u64().unwrap();
    let v = s.void(&["daemon", "start", "--json"]).ok().json();
    assert_eq!((v["started"].as_bool(), v["pid"].as_u64()), (Some(false), Some(pid)), "a second start finds it running");
    assert_eq!(s.void(&["daemon", "start"]).ok().out().trim(), format!("daemon already running (pid {pid}, socket {socket})"));

    let d = s.void(&["daemon", "status", "--json"]).ok().json();
    assert_eq!((d["running"].as_bool(), d["pid"].as_u64(), d["version"].as_str()), (Some(true), Some(pid), Some(env!("CARGO_PKG_VERSION"))));
    assert_eq!((d["endpoint"].as_str(), d["accessKeyId"].as_str()), (Some(f.endpoint.as_str()), Some(ADMIN_KEY_ID)));
    assert!(s.void(&["daemon", "status"]).ok().out().starts_with(&format!("daemon running · pid {pid} · up ")));

    let i = s.void(&["daemon", "info", "--json"]).ok().json();
    assert_eq!((i["pid"].as_u64(), i["restartSafe"].as_bool(), i["memoryOnlyBytes"].as_u64()), (Some(pid), Some(true), Some(0)));
    assert_eq!(i["journal"], serde_json::json!({ "unpublished": 0, "unpublishedBytes": 0, "uploading": 0, "imports": 0 }));
    let text = s.void(&["daemon", "info"]).ok().out();
    assert!(lines(&text).contains(&"restart safe yes".to_owned()), "{text}");

    let st = s.void(&["status", "--json"]).ok().json();
    assert_eq!(st["daemon"]["pid"].as_u64(), Some(pid));
    assert_eq!((st["connection"]["endpoint"].as_str(), st["connection"]["link"].as_str()), (Some(f.endpoint.as_str()), Some("online")));
    assert_eq!((st["uploads"]["unpublished"].as_u64(), st["uploads"]["bandwidth"].clone()), (Some(0), Value::Null));
    assert_eq!(st["cache"]["maxBytes"].as_u64(), Some(20 << 30));
    let page = lines(&s.void(&["status"]).ok().out());
    assert_eq!(page.iter().map(|l| l.split(' ').next().unwrap()).collect::<Vec<_>>(), ["daemon", "server", "uploads", "cache"], "{page:?}");
    assert_eq!(page[1], format!("server {} · online", f.endpoint));
    assert_eq!(page[2], "uploads idle");

    // What only its user may read: the socket, the directory, and the key it keeps.
    assert_eq!((mode(&s.socket()), mode(&s.dir), mode(&s.dir.join("daemon.json"))), (0o600, 0o700, 0o600));
    let kept: Value = serde_json::from_slice(&std::fs::read(s.dir.join("daemon.json")).unwrap()).unwrap();
    assert_eq!((kept["endpoint"].as_str(), kept["accessKeyId"].as_str()), (Some(f.endpoint.as_str()), Some(ADMIN_KEY_ID)));

    let v = s.void(&["daemon", "stop", "--json"]).ok().json();
    assert_eq!((v["stopped"].as_bool(), v["pid"].as_u64()), (Some(true), Some(pid)));
    assert!(!s.socket().exists(), "the socket goes with it");
    assert_eq!(s.void(&["daemon", "status", "--json"]).ok().json()["running"], false);
    assert_eq!(s.void(&["daemon", "stop"]).ok().out().trim(), format!("daemon not running (socket {socket})"));
    let log = std::fs::read_to_string(s.dir.join("daemon.log")).unwrap();
    assert!(log.contains(&format!("daemon started: pid {pid}")) && log.contains("stopping (asked to stop)") && log.ends_with("stopped\n"), "{log}");
}

#[test]
fn a_restart_keeps_the_connection_it_was_given() {
    let f = Fixture::new("restart");
    let s = State::new(&f, "r");
    let e = run(&["daemon", "start", "--json"], &s.bare(), None).fails(1).error();
    assert_eq!(e["code"], "NoCredentials", "nothing to start with: {e}");

    // The connection given on the command line, not the environment's, is the one it keeps.
    let mut env = s.env.clone();
    env.retain(|(k, _)| *k != "VOIDFS_ENDPOINT");
    env.push(("VOIDFS_ENDPOINT", "http://127.0.0.1:1".into()));
    run(&["daemon", "start", "--endpoint", &f.endpoint], &env, None).ok();
    assert_eq!(s.void(&["status", "--json"]).ok().json()["daemon"]["endpoint"].as_str(), Some(f.endpoint.as_str()));
    let pid = s.pid();
    // No connection now: the daemon uses the one it was started with.
    let v = run(&["daemon", "restart", "--json"], &s.bare(), None).ok().json();
    assert_eq!(v["stoppedPid"].as_u64(), Some(pid));
    let new = v["pid"].as_u64().unwrap();
    assert_ne!(new, pid, "a new process");
    let st = run(&["status", "--json"], &s.bare(), None).ok().json();
    assert_eq!((st["daemon"]["pid"].as_u64(), st["daemon"]["endpoint"].as_str(), st["connection"]["link"].as_str()), (Some(new), Some(f.endpoint.as_str()), Some("online")));

    run(&["daemon", "stop"], &s.bare(), None).ok();
    let v = run(&["daemon", "restart", "--json"], &s.bare(), None).ok().json();
    assert_eq!(v["stoppedPid"], Value::Null, "restarting a stopped daemon starts it");
    assert!(v["pid"].as_u64().is_some());
}

#[test]
fn one_daemon_per_state_directory_and_a_dead_ones_socket_is_replaced() {
    let f = Fixture::new("one");
    let s = State::new(&f, "o");
    s.void(&["daemon", "start"]).ok();
    let pid = s.pid();
    let e = s.void(&["daemon", "run", "--json"]).fails(1).error();
    assert_eq!(e["code"], "DaemonRunning", "{e}");
    assert!(e["message"].as_str().unwrap().contains(&format!("(pid {pid})")), "it asks the socket first, and says which: {e}");
    assert_eq!(s.pid(), pid, "the first one is untouched");
    s.void(&["daemon", "stop"]).ok();

    // A daemon that died leaves its socket behind, which nothing answers on.
    drop(std::os::unix::net::UnixListener::bind(s.socket()).unwrap());
    assert!(s.socket().exists());
    assert_eq!(s.void(&["daemon", "status", "--json"]).ok().json()["running"], false);
    let v = s.void(&["daemon", "start", "--json"]).ok().json();
    assert_eq!(v["started"], true);
    assert_eq!(s.void(&["status", "--json"]).ok().json()["daemon"]["running"], true);
}

#[test]
fn daemon_run_stops_on_a_signal() {
    let f = Fixture::new("signal");
    let s = State::new(&f, "s");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_void"));
    cmd.args(["daemon", "run"]).env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    for (k, v) in &s.env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while s.void(&["daemon", "status", "--json"]).ok().json()["running"] != true {
        assert!(Instant::now() < deadline, "the daemon didn't answer");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(s.pid(), u64::from(child.id()));
    for sig in ["-HUP", "-TERM"] {
        let ok = Command::new("kill").args([sig, &child.id().to_string()]).status().unwrap();
        assert!(ok.success());
        std::thread::sleep(Duration::from_millis(200));
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(Instant::now() < deadline, "the daemon didn't stop");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
    assert!(err.contains("SIGHUP: ignored") && err.contains("stopping (SIGTERM)"), "{err}");
    assert!(!s.socket().exists());
}

#[test]
fn a_daemon_whose_terminal_went_away_still_stops() {
    let f = Fixture::new("hup");
    let s = State::new(&f, "h");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_void"));
    cmd.args(["daemon", "run"]).env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    for (k, v) in &s.env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    drop(child.stderr.take());
    let deadline = Instant::now() + Duration::from_secs(20);
    while s.void(&["daemon", "status", "--json"]).ok().json()["running"] != true {
        assert!(Instant::now() < deadline, "the daemon didn't answer");
        assert!(child.try_wait().unwrap().is_none(), "the daemon died");
        std::thread::sleep(Duration::from_millis(20));
    }
    let t = Instant::now();
    s.void(&["daemon", "stop"]).ok();
    assert!(t.elapsed() < Duration::from_secs(20));
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(Instant::now() < deadline, "the daemon didn't stop");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "it stopped, rather than died writing to a closed stderr: {status}");
    assert!(!s.socket().exists());
}

#[test]
fn stop_returns_when_the_daemon_died_on_the_way() {
    let f = Fixture::new("died");
    let s = State::new(&f, "d");
    std::fs::create_dir_all(&s.dir).unwrap();
    // A daemon that answers the stop request, then dies without removing its socket.
    let listener = std::os::unix::net::UnixListener::bind(s.socket()).unwrap();
    let fake = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut conn, _) = listener.accept().unwrap();
        let mut req = [0u8; 4096];
        let _ = conn.read(&mut req).unwrap();
        let body = r#"{"stopping":true,"pid":4242}"#;
        write!(conn, "HTTP/1.1 202 Accepted\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let t = Instant::now();
    let v = s.void(&["daemon", "stop", "--json"]).ok().json();
    fake.join().unwrap();
    assert_eq!((v["stopped"].as_bool(), v["pid"].as_u64()), (Some(true), Some(4242)));
    assert!(t.elapsed() < Duration::from_secs(20), "it saw the state directory free, socket or not");
    assert!(s.socket().exists());
}

#[test]
fn a_daemon_that_cannot_start_says_why() {
    let f = Fixture::new("cannot");
    let mut s = State::new(&f, "c");
    // Past the length a Unix socket's path may have.
    s.dir = s.dir.join("x".repeat(120));
    s.env.retain(|(k, _)| *k != "VOIDFS_STATE_DIR");
    s.env.push(("VOIDFS_STATE_DIR", s.dir.display().to_string()));
    let e = s.void(&["daemon", "start", "--json"]).fails(1).error();
    assert_eq!(e["code"], "DaemonFailed", "{e}");
    let m = e["message"].as_str().unwrap();
    assert!(m.contains("exited") && m.contains("bytes, more than the") && m.contains("VOIDFS_STATE_DIR"), "{m}");
    let _ = std::fs::remove_dir_all(s.dir.parent().unwrap());
}
