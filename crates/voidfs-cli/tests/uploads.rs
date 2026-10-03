// SPDX-License-Identifier: Apache-2.0
//! `void upload` through the daemon, and `void uploads`: the binary against a daemon it started,
//! on a state directory of the test's own, and a server in this process.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

mod common;
use common::*;

/// Waits until `void uploads --json --all` satisfies `done`, and returns it.
#[track_caller]
fn wait_for(s: &State, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let v = s.void(&["uploads", "--json", "--all"]).ok().json();
        if done(&v) {
            return v;
        }
        assert!(Instant::now() < deadline, "{what}: {v:#}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn batch(v: &Value, id: u64) -> Value {
    v["batches"].as_array().unwrap().iter().find(|b| b["id"] == id).cloned().unwrap_or(Value::Null)
}

fn item<'a>(v: &'a Value, key: &str) -> &'a Value {
    v["items"].as_array().unwrap().iter().find(|i| i["key"] == key).unwrap_or_else(|| panic!("{key} in {v:#}"))
}

fn exists(f: &Fixture, drive: &str, key: &str) -> bool {
    match f.block(f.client.head_object(drive, key, Default::default())) {
        Ok(_) => true,
        Err(e) if e.status() == Some(404) => false,
        Err(e) => panic!("{e}"),
    }
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn uploads_go_through_the_daemon_detached_or_followed() {
    let f = Fixture::new("through");
    f.drive("footage");
    let s = State::new(&f, "t");
    let a = f.write("a.txt", b"alpha");
    f.write("dir/sub/b.txt", b"bravo");
    std::fs::create_dir_all(f.path("dir/empty")).unwrap();
    s.void(&["daemon", "start"]).ok();

    let v = s.void(&["upload", "--detach", path(&a), path(&f.path("dir")), "footage:/cuts", "--json"]).ok().json();
    let id = v["batch"].as_u64().unwrap();
    assert_eq!((v["drive"].as_str(), v["folder"].as_str(), v["items"].as_u64(), v["bytes"].as_u64(), v["detached"].as_bool()), (Some("footage"), Some("cuts/"), Some(3), Some(10), Some(true)));
    let v = wait_for(&s, "the batch finishes", |v| batch(v, id)["done"] == 3);
    assert_eq!(batch(&v, id)["label"], "a.txt and 1 more");
    assert_eq!(item(&v, "cuts/dir/empty/")["op"], "folder");
    assert_eq!((f.text("footage", "cuts/a.txt"), f.text("footage", "cuts/dir/sub/b.txt")), ("alpha".into(), "bravo".into()));
    assert!(exists(&f, "footage", "cuts/dir/empty/"), "the empty folder is made");
    let table = s.void(&["uploads"]).ok().out();
    assert!(table.starts_with("no uploads in progress") && lines(&table).iter().any(|l| l == &format!("{id} a.txt and 1 more 3 3 0 10 B done")), "{table}");
    assert!(table.ends_with("nothing to send\n"), "{table}");

    // Followed until it is up, as the foreground upload reports it.
    let c = f.write("c.txt", b"charlie");
    let v = s.void(&["upload", path(&c), "footage:/f", "--json"]).ok().json();
    assert!(v["batch"].as_u64().unwrap() > id);
    let files = v["files"].as_array().unwrap();
    assert_eq!((files.len(), files[0]["key"].as_str(), files[0]["size"].as_u64(), files[0]["path"].as_str()), (1, Some("f/c.txt"), Some(7), Some(path(&std::path::absolute(&c).unwrap()))));
    assert!(files[0]["versionId"].as_str().is_some());
    assert_eq!(f.text("footage", "f/c.txt"), "charlie");
    let text = s.void(&["upload", path(&c), "footage:/g"]).ok().out();
    let l = lines(&text);
    assert!(l[0].ends_with("-> footage:g/c.txt (7 B)") && l[1].starts_with("Uploaded 1 file (7 B) to footage:g/ in "), "{text}");

    // Into a drive that isn't there: refused before anything is queued.
    let e = s.void(&["upload", "--detach", path(&c), "nosuch:/", "--json"]).fails(1).error();
    assert_eq!(e["code"], "NoSuchBucket", "{e}");
}

#[test]
fn without_a_daemon_upload_runs_in_the_foreground_unless_detached() {
    let f = Fixture::new("nodaemon");
    f.drive("footage");
    let s = State::new(&f, "n");
    let a = f.write("a.txt", b"alpha");
    let v = s.void(&["upload", path(&a), "footage:/fg", "--json"]).ok().json();
    assert!(v.get("batch").is_none(), "in the foreground: {v}");
    assert_eq!(f.text("footage", "fg/a.txt"), "alpha");
    assert_eq!(s.void(&["daemon", "status", "--json"]).ok().json()["running"], false, "and no daemon started");

    let v = s.void(&["upload", "--detach", path(&a), "footage:/bg", "--json"]).ok().json();
    assert!(v["batch"].as_u64().is_some(), "{v}");
    assert_eq!(s.void(&["daemon", "status", "--json"]).ok().json()["running"], true, "--detach started one");
    wait_for(&s, "the detached upload finishes", |v| v["batches"][0]["done"] == 1);
    assert_eq!(f.text("footage", "bg/a.txt"), "alpha");

    // Nowhere to keep a daemon's state: --detach can't, the foreground can.
    let e = run(&["upload", "--detach", path(&a), "footage:/x", "--json"], &f.env(), None).fails(1).error();
    assert_eq!(e["code"], "NoStateDirectory", "{e}");
}

#[test]
fn upload_refuses_a_daemon_on_another_server() {
    let f = Fixture::new("mismatch");
    f.drive("footage");
    let s = State::new(&f, "m");
    let a = f.write("a.txt", b"alpha");
    s.void(&["daemon", "start"]).ok();
    let mut env = s.env.clone();
    env.retain(|(k, _)| *k != "VOIDFS_ENDPOINT");
    env.push(("VOIDFS_ENDPOINT", "http://127.0.0.1:1".into()));
    let e = run(&["upload", "--detach", path(&a), "footage:/m", "--json"], &env, None).fails(1).error();
    assert_eq!(e["code"], "DaemonMismatch", "{e}");
    assert!(s.void(&["uploads", "--json", "--all"]).ok().json()["items"].as_array().unwrap().is_empty(), "nothing queued");
    // Given no connection at all, the daemon's is used.
    let v = run(&["upload", "--detach", path(&a), "footage:/m", "--json"], &s.bare(), None).ok().json();
    assert!(v["batch"].as_u64().is_some());
}

#[test]
fn uploads_pause_resume_cancel_limit_and_clear() {
    let f = Fixture::new("control");
    f.drive("footage");
    let s = State::new(&f, "c");
    let a = f.write("a.txt", b"alpha");
    let b = f.write("b.txt", b"bravo");
    s.void(&["daemon", "start"]).ok();
    assert_eq!(s.void(&["uploads", "pause", "--all", "--json"]).ok().json()["items"], 0);
    let id = s.void(&["upload", "--detach", path(&a), path(&b), "footage:/p", "--json"]).ok().json()["batch"].as_u64().unwrap();
    let v = s.void(&["uploads", "--json"]).ok().json();
    assert_eq!(v["paused"], true);
    for k in ["p/a.txt", "p/b.txt"] {
        assert_eq!((item(&v, k)["state"].as_str(), item(&v, k)["paused"].as_bool()), (Some("queued"), Some(true)), "{v:#}");
    }
    let table = lines(&s.void(&["uploads"]).ok().out());
    assert!(table.iter().any(|l| l.ends_with("p/a.txt 5 B 0% paused")), "{table:?}");
    assert!(table.last().unwrap().starts_with("0 uploading, 2 queued · 10 B to send") && table.last().unwrap().ends_with("· paused"), "{table:?}");

    let a_id = item(&v, "p/a.txt")["id"].as_u64().unwrap().to_string();
    assert_eq!(s.void(&["uploads", "cancel", &a_id, "--json"]).ok().json()["items"], 1);
    assert_eq!(s.void(&["uploads", "limit", "1MiB", "--json"]).ok().json()["bytesPerSecond"], 1 << 20);
    assert_eq!(s.void(&["status", "--json"]).ok().json()["uploads"]["bandwidth"], 1 << 20);
    assert!(s.void(&["uploads"]).ok().out().trim_end().ends_with("· limit 1.0 MiB/s"));
    assert_eq!(s.void(&["uploads", "limit", "unlimited", "--json"]).ok().json()["bytesPerSecond"], Value::Null);

    // A pause by drive, named by its id, finds the uploads queued by its alias.
    let drive_id = f.block(f.client.describe_drive("footage")).unwrap().drive_id;
    s.void(&["uploads", "pause", "--drive", &drive_id]).ok();
    assert_eq!(s.void(&["uploads", "--json"]).ok().json()["pausedDrives"], serde_json::json!(["footage"]));
    s.void(&["uploads", "resume", "--all"]).ok();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!exists(&f, "footage", "p/b.txt"), "still paused on its drive");
    assert_eq!(s.void(&["uploads", "resume", "--drive", "footage", "--json"]).ok().json()["items"], 1);
    let v = wait_for(&s, "b goes up", |v| batch(v, id)["done"] == 1);
    assert_eq!((batch(&v, id)["cancelled"].as_u64(), item(&v, "p/a.txt")["state"].as_str()), (Some(1), Some("cancelled")));
    assert_eq!(f.text("footage", "p/b.txt"), "bravo");
    assert!(!exists(&f, "footage", "p/a.txt"), "a cancelled upload is never published");
    assert!(lines(&s.void(&["uploads"]).ok().out()).iter().any(|l| l.ends_with("done, 1 cancelled")));

    assert_eq!(s.void(&["uploads", "clear", "--json"]).ok().json()["cleared"], 2);
    let v = s.void(&["uploads", "--json", "--all"]).ok().json();
    assert!(v["items"].as_array().unwrap().is_empty() && v["batches"].as_array().unwrap().is_empty(), "{v:#}");

    s.void(&["uploads", "pause"]).fails(2);
    assert_eq!(s.void(&["uploads", "pause", "--batch", "999", "--json"]).fails(1).error()["code"], "NoSuchUpload");
    assert_eq!(s.void(&["uploads", "limit", "fast", "--json"]).fails(2).error()["code"], "Usage");
}

#[test]
fn uploads_survive_a_restart_and_a_crash() {
    let f = Fixture::new("survive");
    f.drive("footage");
    let s = State::new(&f, "v");
    let small = f.write("small.txt", b"small");
    let body: Vec<u8> = (0..24u32 << 20).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    let big = f.write("big.bin", &body);
    let opts = ["--multipart-threshold", "8", "--part-size", "5"];
    s.void(&[&["daemon", "start"][..], &opts].concat()).ok();

    // Paused, through a restart: still queued, still paused.
    s.void(&["uploads", "pause", "--all"]).ok();
    s.void(&["upload", "--detach", path(&small), "footage:/r"]).ok();
    s.void(&[&["daemon", "restart"][..], &opts].concat()).ok();
    let v = s.void(&["uploads", "--json"]).ok().json();
    assert_eq!((item(&v, "r/small.txt")["state"].as_str(), v["paused"].as_bool()), (Some("queued"), Some(true)));
    s.void(&["uploads", "resume", "--all"]).ok();
    wait_for(&s, "the small file goes up after the restart", |v| item(v, "r/small.txt")["state"] == "done");

    // Killed mid-upload: the journal has it, and a new daemon finishes it, once.
    s.void(&["uploads", "limit", "4MiB"]).ok();
    s.void(&["upload", "--detach", path(&big), "footage:/r"]).ok();
    wait_for(&s, "the large file starts", |v| item(v, "r/big.bin")["state"] == "uploading");
    std::thread::sleep(Duration::from_millis(1500));
    let pid = s.pid().to_string();
    assert!(Command::new("kill").args(["-9", &pid]).status().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(10);
    let st = loop {
        let st = s.void(&["status", "--json"]).ok().json();
        if st["daemon"]["running"] == false && st.get("uploads").is_some() {
            break st;
        }
        assert!(Instant::now() < deadline, "{st}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(st["uploads"]["unpublished"], 1, "{st}");
    assert!(st["uploads"]["unpublishedBytes"].as_u64().unwrap() > 0);
    assert!(lines(&s.void(&["status"]).ok().out()).iter().any(|l| l.starts_with("uploads 1 change") && l.ends_with("wait in the journal, for the daemon to start")));
    s.void(&[&["daemon", "start"][..], &opts].concat()).ok();
    s.void(&["uploads", "limit", "unlimited"]).ok();
    wait_for(&s, "the large file goes up after the crash", |v| item(v, "r/big.bin")["state"] == "done");
    let got = f.block(f.client.get_object("footage", "r/big.bin", Default::default())).unwrap().body;
    assert!(got == body, "the whole file, as it was");
    let versions = f.block(f.client.list_versions("footage", "r/big.bin", false)).unwrap();
    assert_eq!(versions.len(), 1, "published once: {versions:?}");
}

#[test]
fn uploads_watch_until_the_daemon_stops() {
    let f = Fixture::new("watch");
    f.drive("footage");
    let s = State::new(&f, "w");
    s.void(&["daemon", "start"]).ok();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_void"));
    cmd.args(["uploads", "--watch", "--json"]).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, v) in &s.env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            if tx.send(line.unwrap()).is_err() {
                return;
            }
        }
    });
    let a = f.write("a.txt", b"alpha");
    s.void(&["upload", "--detach", path(&a), "footage:/w"]).ok();
    let mut seen = Vec::new();
    while seen.len() < 3 {
        let line = rx.recv_timeout(Duration::from_secs(20)).expect("a line each second");
        seen.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    assert!(seen.iter().all(|v| v["items"].is_array() && v["rate"].is_u64()), "{seen:?}");
    s.void(&["daemon", "stop"]).ok();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(Instant::now() < deadline, "the watch didn't end with the daemon");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
}
