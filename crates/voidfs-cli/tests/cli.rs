// SPDX-License-Identifier: Apache-2.0
//! The `void` binary, run against a server in this process: every command in text and JSON, and
//! its errors.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use voidfs_sdk::{Client, Config, Kind, PutOptions, ReadOptions, RenameOptions};
use voidfs_server::sigv4::{KeyInfo, Scope};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

/// A server on its own runtime, an admin client for it, and a scratch folder.
struct Fixture {
    rt: tokio::runtime::Runtime,
    server: Option<TestServer>,
    /// `http://localhost:<port>`: a name, not an address, so that the AWS client underneath
    /// would address drives virtual-host style if it weren't told to use paths.
    endpoint: String,
    client: Client,
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        Fixture::with_keys(name, Vec::new())
    }

    fn with_keys(name: &str, keys: Vec<KeyInfo>) -> Fixture {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let server = rt.block_on(TestServer::with_keys(keys)).unwrap();
        let endpoint = format!("http://localhost:{}", server.addr.port());
        let client = Client::new(Config { endpoint: endpoint.clone(), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..Config::default() }).unwrap();
        let dir = std::env::temp_dir().join(format!("void-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture { rt, server: Some(server), endpoint, client, dir }
    }

    fn block<F: Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![("VOIDFS_ENDPOINT", self.endpoint.clone()), ("VOIDFS_ACCESS_KEY_ID", ADMIN_KEY_ID.into()), ("VOIDFS_SECRET_ACCESS_KEY", ADMIN_SECRET.into())]
    }

    /// `void <args>` with the admin key.
    fn void(&self, args: &[&str]) -> Run {
        run(args, &self.env(), None)
    }

    fn put(&self, drive: &str, key: &str, body: &str) -> String {
        self.block(self.client.put_object(drive, key, body.to_owned(), Default::default())).unwrap().version_id
    }

    fn text(&self, drive: &str, key: &str) -> String {
        String::from_utf8(self.block(self.client.get_object(drive, key, Default::default())).unwrap().body.to_vec()).unwrap()
    }

    fn drive(&self, name: &str) {
        self.block(self.client.create_drive(name, Default::default())).unwrap();
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    fn write(&self, rel: &str, body: &[u8]) -> PathBuf {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        // The server's task belongs to the runtime: drop it inside.
        let _guard = self.rt.enter();
        self.server.take();
    }
}

#[derive(Debug)]
struct Run {
    status: i32,
    stdout: Vec<u8>,
    stderr: String,
}

impl Run {
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    #[track_caller]
    fn ok(self) -> Run {
        assert_eq!(self.status, 0, "stdout: {}\nstderr: {}", self.out(), self.stderr);
        self
    }

    #[track_caller]
    fn fails(self, status: i32) -> Run {
        assert_eq!(self.status, status, "stdout: {}\nstderr: {}", self.out(), self.stderr);
        self
    }

    /// The JSON document on stdout.
    #[track_caller]
    fn json(&self) -> Value {
        serde_json::from_slice(&self.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}\nstderr: {}", self.out(), self.stderr))
    }

    /// The `error` member of the JSON on stderr.
    #[track_caller]
    fn error(&self) -> Value {
        let v: Value = serde_json::from_str(&self.stderr).unwrap_or_else(|e| panic!("stderr is not JSON ({e}): {}", self.stderr));
        v["error"].clone()
    }
}

/// Runs the binary with only `env` for an environment, `stdin` fed and closed, and a deadline.
fn run(args: &[&str], env: &[(&str, String)], stdin: Option<&str>) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_void"));
    cmd.args(args).env_clear().stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("void runs");
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    }
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err = std::thread::spawn(move || {
        let mut b = String::new();
        let _ = err.read_to_string(&mut b);
        b
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("void {args:?} took longer than 30 s");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Run { status: status.code().unwrap_or(-1), stdout: out.join().unwrap(), stderr: err.join().unwrap() }
}

fn lines(s: &str) -> Vec<String> {
    s.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

#[test]
fn version_prints_the_build() {
    let r = run(&["version"], &[], None).ok();
    let text = r.out();
    let words: Vec<&str> = text.trim().splitn(3, ' ').collect();
    assert_eq!(words[..2], ["void", env!("CARGO_PKG_VERSION")], "{text}");
    let (commit, date) = words[2].trim_matches(['(', ')']).split_once(", ").unwrap();
    assert!(commit == "unknown" || (commit.len() == 7 && commit.bytes().all(|b| b.is_ascii_hexdigit())), "{commit}");
    assert!(date == "unknown" || chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok(), "{date}");
    assert_eq!(run(&["-V"], &[], None).ok().out(), text, "-V says the same");
    let v = run(&["version", "--json"], &[], None).ok().json();
    assert_eq!(v, json!({ "name": "void", "version": env!("CARGO_PKG_VERSION"), "commit": commit, "commitDate": date, "protocol": 1 }));
}

/// The key `keys generate --format json` printed.
fn generated(args: &[&str]) -> (String, String, Value) {
    let v = run(args, &[], None).ok().json();
    (v["accessKeyId"].as_str().unwrap().to_owned(), v["secretAccessKey"].as_str().unwrap().to_owned(), v)
}

#[test]
fn keys_generate_makes_keys_the_server_accepts() {
    let (id, secret, v) = generated(&["keys", "generate", "--format", "json"]);
    assert!(id.len() == 20 && id.starts_with("VF") && id[2..].bytes().all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b)), "{id}");
    assert_eq!(secret.len(), 40);
    assert_eq!((v["scope"].as_str(), v["serverKey"].as_str()), (Some("write"), Some(format!("{id}:{secret}:write").as_str())), "write by default, as SpaceFS's");
    let (read_id, read_secret, v) = generated(&["keys", "generate", "--access", "read", "--json"]);
    assert_eq!(v["scope"], "read");
    assert_ne!(read_id, id);
    let text = run(&["keys", "generate", "--scope", "admin"], &[], None).ok().out();
    let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)).map(|v| v.trim().to_owned()).unwrap_or_else(|| panic!("{name} in {text}"));
    let (admin_id, admin_secret) = (field("access key id:"), field("secret access key:"));
    assert_eq!(field("scope:"), "admin");
    assert!(text.contains(&format!("--key {admin_id}:{admin_secret}:admin")), "{text}");
    let env = run(&["keys", "generate", "--format", "env"], &[], None).ok().out();
    let env_lines: Vec<&str> = env.lines().collect();
    assert_eq!(env_lines.len(), 2, "{env}");
    let value = |line: &str, name: &str| line.strip_prefix(&format!("export {name}='")).and_then(|v| v.strip_suffix('\'')).map(str::to_owned).unwrap_or_else(|| panic!("{line}"));
    let (env_id, env_secret) = (value(env_lines[0], "VOIDFS_ACCESS_KEY_ID"), value(env_lines[1], "VOIDFS_SECRET_ACCESS_KEY"));
    assert!(env_id.starts_with("VF") && env_secret.len() == 40);

    // Each works once the server has it: from a key file in either form, or the environment.
    let key = |id: &str, secret: &str, scope| KeyInfo { id: id.into(), secret: secret.into(), scope, drives: None };
    let f = Fixture::with_keys("keys", vec![key(&id, &secret, Scope::Write), key(&read_id, &read_secret, Scope::Read), key(&admin_id, &admin_secret, Scope::Admin), key(&env_id, &env_secret, Scope::Write)]);
    let endpoint = [("VOIDFS_ENDPOINT", f.endpoint.clone())];
    let with_key = |id: &str, secret: &str| vec![("VOIDFS_ENDPOINT", f.endpoint.clone()), ("VOIDFS_ACCESS_KEY_ID", id.to_owned()), ("VOIDFS_SECRET_ACCESS_KEY", secret.to_owned())];
    run(&["drive", "create", "footage"], &with_key(&admin_id, &admin_secret), None).ok();
    let r = run(&["drive", "create", "other", "--json"], &with_key(&read_id, &read_secret), None).fails(1);
    assert_eq!((r.error()["code"].as_str(), r.error()["status"].as_u64()), (Some("AccessDenied"), Some(403)));
    let r = run(&["drive", "create", "other", "--json"], &with_key(&env_id, &env_secret), None).fails(1);
    assert_eq!(r.error()["code"], "AccessDenied", "a write key can't create drives");
    let json_file = f.write("key.json", &run(&["keys", "generate", "--json"], &[], None).ok().stdout);
    let r = run(&["drives", "--json", "--key-file", json_file.to_str().unwrap()], &endpoint, None).fails(1);
    assert_eq!(r.error()["code"], "InvalidAccessKeyId", "a key the server wasn't given");
    let json_file = f.write("key.json", serde_json::to_string(&json!({ "accessKeyId": id, "secretAccessKey": secret })).unwrap().as_bytes());
    let r = run(&["drives", "--json", "--key-file", json_file.to_str().unwrap()], &endpoint, None).ok();
    assert_eq!(r.json()["drives"][0]["alias"], "footage");
    let env_file = f.write("key.env", env.as_bytes());
    run(&["drives", "--key-file", env_file.to_str().unwrap()], &endpoint, None).ok();
}

#[test]
fn drives_lists_every_drive_with_its_id_size_and_fork() {
    let f = Fixture::new("drives");
    assert_eq!(f.void(&["drives"]).ok().out().trim(), "No drives. Create one with `void drive create <name>`.");
    assert_eq!(f.void(&["drives", "--json"]).ok().json(), json!({ "drives": [] }));
    f.drive("footage");
    f.put("footage", "a.txt", "hello");
    let fork = f.block(f.client.fork_drive("footage", "footage-try")).unwrap();
    f.drive("zebra");
    let v = f.void(&["drives", "--json"]).ok().json();
    let drives = v["drives"].as_array().unwrap();
    assert_eq!(drives.iter().map(|d| d["alias"].as_str().unwrap()).collect::<Vec<_>>(), ["footage", "footage-try", "zebra"]);
    let footage = f.block(f.client.describe_drive("footage")).unwrap();
    assert_eq!(drives[0], serde_json::to_value(&footage).unwrap(), "each drive as `drive show --json` gives it");
    assert_eq!((drives[0]["usageBytes"].as_u64(), drives[1]["forkOf"]["driveId"].as_str()), (Some(5), Some(footage.drive_id.as_str())));
    let text = f.void(&["ls"]).ok().out();
    let rows = lines(&text);
    assert_eq!(rows[0], "NAME ID SIZE CREATED FORK OF");
    let created = chrono::DateTime::parse_from_rfc3339(footage.created_at.as_deref().unwrap()).unwrap().with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    assert_eq!(rows[1], format!("footage {} 5 B {created} -", footage.drive_id));
    assert!(rows[2].starts_with(&format!("footage-try {} 5 B ", fork.drive_id)) && rows[2].ends_with(" footage"), "a fork names its source: {}", rows[2]);
    assert!(rows[3].starts_with("zebra ") && rows[3].ends_with(" -"), "{}", rows[3]);
    assert_eq!(rows.len(), 4);
}

#[test]
fn drive_create_and_show() {
    let f = Fixture::new("create");
    let text = f.void(&["drive", "create", "footage"]).ok().out();
    let id = f.block(f.client.describe_drive("footage")).unwrap().drive_id;
    assert_eq!(text.trim(), format!("Created drive footage ({id})."));
    let v = f.void(&["drive", "create", "renders", "--display-name", "Renders", "--json"]).ok().json();
    let renders = f.block(f.client.describe_drive("renders")).unwrap();
    assert_eq!(v, json!({ "driveId": renders.drive_id, "alias": "renders" }));

    let r = f.void(&["drive", "create", "footage"]).fails(1);
    assert!(r.stderr.starts_with("error: drive footage: 409 BucketAlreadyOwnedByYou"), "{}", r.stderr);
    assert!(r.stdout.is_empty());
    let r = f.void(&["--json", "drive", "create", "footage"]).fails(1);
    assert_eq!((r.error()["code"].as_str(), r.error()["status"].as_u64()), (Some("BucketAlreadyOwnedByYou"), Some(409)));
    assert!(r.error()["requestId"].is_string() && r.stdout.is_empty());
    let r = f.void(&["drive", "create", "d", "--json"]).fails(1);
    assert_eq!(r.error()["code"], "InvalidBucketName");

    f.put("footage", "a/b.txt", "hello");
    f.block(f.client.fork_drive("footage", "footage-try")).unwrap();
    let footage = f.block(f.client.describe_drive("footage")).unwrap();
    assert_eq!(f.void(&["drive", "show", "footage", "--json"]).ok().json(), serde_json::to_value(&footage).unwrap());
    assert_eq!(f.void(&["drive", "show", &id, "--json"]).ok().json()["alias"], "footage", "by id too");
    let text = f.void(&["drive", "show", "footage"]).ok().out();
    let rows = lines(&text);
    assert_eq!(rows[..2], ["footage".to_string(), format!("id {id}")]);
    assert!(rows.contains(&"size 5 B (5 bytes)".to_string()), "{text}");
    assert!(rows.contains(&format!("forks {}", footage.forks[0])), "{text}");
    assert!(rows.contains(&format!("position {}", footage.seq)), "{text}");
    let text = f.void(&["drive", "show", "footage-try"]).ok().out();
    assert!(lines(&text).iter().any(|l| l.starts_with(&format!("fork of {} at ", footage.drive_id))), "{text}");
    let r = f.void(&["drive", "show", "nowhere", "--json"]).fails(1);
    assert_eq!(r.error()["code"], "NoSuchBucket");
}

#[test]
fn drive_delete_asks_for_the_name_and_undelete_recovers() {
    let f = Fixture::new("delete");
    f.drive("footage");
    f.put("footage", "a.txt", "kept");
    let live = |name: &str| f.block(f.client.describe_drive(name)).is_ok();
    let env = f.env();
    let r = run(&["drive", "delete", "footage"], &env, Some("footag\n")).fails(1);
    assert!(r.stderr.contains("Delete drive footage? Type its name to confirm:") && r.stderr.contains("error: drive footage was not deleted"), "{}", r.stderr);
    let r = run(&["drive", "delete", "footage", "--json"], &env, None).fails(1);
    assert_eq!(r.error()["code"], "NotConfirmed", "no answer is no");
    assert!(live("footage"));
    let r = run(&["drive", "delete", "footage"], &env, Some("footage\n")).ok();
    assert!(r.out().starts_with("Deleted drive footage. `void drive undelete footage` recovers it"), "{}", r.out());
    assert!(!live("footage"));
    let text = f.void(&["drive", "undelete", "footage"]).ok().out();
    let id = f.block(f.client.describe_drive("footage")).unwrap().drive_id;
    assert_eq!(text.trim(), format!("Recovered drive footage ({id})."));
    assert_eq!(f.text("footage", "a.txt"), "kept");

    assert_eq!(f.void(&["drive", "delete", "footage", "-y", "--json"]).ok().json(), json!({ "drive": "footage", "hard": false }));
    assert_eq!(f.void(&["drive", "undelete", "footage", "--json"]).ok().json(), json!({ "driveId": id, "alias": "footage" }));
    let r = run(&["drive", "delete", "footage", "--hard"], &env, Some("footage\n")).ok();
    assert!(r.stderr.contains("Permanently delete drive footage?"), "{}", r.stderr);
    assert_eq!(r.out().trim(), "Deleted drive footage permanently.");
    let r = f.void(&["drive", "undelete", "footage", "--json"]).fails(1);
    assert_eq!(r.error()["code"], "NoSuchBucket", "a hard delete can't be undone");
    let r = f.void(&["drive", "delete", "nowhere", "--yes"]).fails(1);
    assert!(r.stderr.starts_with("error: drive nowhere: 404 NoSuchBucket"), "{}", r.stderr);
}

#[test]
fn fork_copies_a_drive_at_once() {
    let f = Fixture::new("fork");
    f.drive("footage");
    f.put("footage", "a.txt", "one");
    let id = f.block(f.client.describe_drive("footage")).unwrap().drive_id;
    let text = f.void(&["fork", "footage", "footage-try"]).ok().out();
    let fork = f.block(f.client.describe_drive("footage-try")).unwrap();
    let point = fork.fork_of.as_ref().unwrap().fork_point.clone().unwrap();
    assert_eq!(text.trim(), format!("Forked footage as footage-try ({}) at {point}. Its history up to then is shared; from now on the two are independent.", fork.drive_id));
    f.put("footage-try", "a.txt", "two");
    assert_eq!(f.text("footage", "a.txt"), "one");
    let v = f.void(&["fork", &id, "second", "--json"]).ok().json();
    let second = f.block(f.client.describe_drive("second")).unwrap();
    assert_eq!(v, json!({ "driveId": second.drive_id, "alias": "second", "source": id, "sourceId": id, "forkPoint": second.fork_of.unwrap().fork_point }));
    let r = f.void(&["fork", "nowhere", "third", "--json"]).fails(1);
    assert_eq!(r.error()["code"], "NoSuchBucket");
    assert!(r.error()["message"].as_str().unwrap().starts_with("fork of nowhere as third: 404 NoSuchBucket"));
}

#[test]
fn history_lists_a_files_versions() {
    let f = Fixture::new("history");
    f.drive("footage");
    let v1 = f.put("footage", "cuts/a.txt", "one");
    let v2 = f.put("footage", "cuts/a.txt", "two!");
    let restored = f.block(f.client.restore_version("footage", "cuts/a.txt", &v1, Default::default())).unwrap().version_id;
    let renamed = f.block(f.client.rename("footage", "cuts/a.txt", "cuts/b.txt", RenameOptions::default())).unwrap().version_id;
    let history = f.block(f.client.list_versions("footage", "cuts/b.txt", false)).unwrap();
    let v = f.void(&["history", "footage", "cuts/b.txt", "--json"]).ok().json();
    assert_eq!(v, json!({ "drive": "footage", "key": "cuts/b.txt", "kind": "file", "versions": history }));
    assert_eq!(f.void(&["history", "--bucket", "footage", "/cuts/b.txt", "--json"]).ok().json(), v, "SpaceFS's form, and a leading /");
    let text = f.void(&["history", "footage", "cuts/b.txt"]).ok().out();
    let t = |i: usize| history[i].last_modified.as_str();
    assert_eq!(lines(&text), [
        "footage:cuts/b.txt".to_string(),
        "VERSION TIME SIZE OPERATION".into(),
        format!("{v1} {} 3 B put", t(0)),
        format!("{v2} {} 4 B put", t(1)),
        format!("{restored} {} 3 B restore (from {v1}) current", t(2)),
    ]);
    let all = f.void(&["history", "footage", "cuts/b.txt", "--all", "--json"]).ok().json();
    let ops: Vec<&str> = all["versions"].as_array().unwrap().iter().map(|v| v["operation"].as_str().unwrap()).collect();
    assert_eq!(ops, ["put", "put", "restore", "rename"]);
    assert_eq!(all["versions"][3]["versionId"].as_str(), Some(renamed.as_str()));

    let r = f.void(&["history", "footage", "cuts/zzz.txt", "--json"]).fails(1);
    assert_eq!((r.error()["code"].as_str(), r.error()["status"].as_u64()), (Some("NoSuchKey"), Some(404)));
    let r = f.void(&["history", "nowhere", "a.txt"]).fails(1);
    assert!(r.stderr.starts_with("error: a.txt: 404 NoSuchBucket"), "{}", r.stderr);
    let r = f.void(&["history", "footage"]).fails(2);
    assert!(r.stderr.contains("give the drive and the path within it"), "{}", r.stderr);
    let r = f.void(&["history", "--bucket", "footage", "a", "b", "--json"]).fails(2);
    assert_eq!(r.error()["code"], "Usage");
}

#[test]
fn history_of_a_folder_lists_every_file_in_it() {
    let f = Fixture::new("folder-history");
    f.drive("footage");
    let a1 = f.put("footage", "cuts/a.txt", "a1");
    let b1 = f.put("footage", "cuts/sub/b.txt", "b1");
    let a2 = f.put("footage", "cuts/a.txt", "a2!");
    f.put("footage", "other.txt", "not in the folder");
    f.put("footage", "cuts/empty/", "");
    let a = f.block(f.client.list_versions("footage", "cuts/a.txt", false)).unwrap();
    let b = f.block(f.client.list_versions("footage", "cuts/sub/b.txt", false)).unwrap();
    let v = f.void(&["history", "footage", "cuts", "--json"]).ok().json();
    let with_key = |k: &str, v: &voidfs_sdk::VersionEntry| {
        let mut j = serde_json::to_value(v).unwrap();
        j["key"] = json!(k);
        j
    };
    assert_eq!(v, json!({ "drive": "footage", "key": "cuts/", "kind": "folder", "versions": [with_key("cuts/a.txt", &a[0]), with_key("cuts/sub/b.txt", &b[0]), with_key("cuts/a.txt", &a[1])] }));
    assert_eq!(f.void(&["history", "footage", "cuts/", "--json"]).ok().json(), v);
    let text = f.void(&["history", "footage", "cuts"]).ok().out();
    assert_eq!(lines(&text), [
        "footage:cuts/, every version of every file in it".to_string(),
        "TIME FILE VERSION SIZE OPERATION".into(),
        format!("{} a.txt {a1} 2 B put", a[0].last_modified),
        format!("{} sub/b.txt {b1} 2 B put current", b[0].last_modified),
        format!("{} a.txt {a2} 3 B put current", a[1].last_modified),
    ]);
    assert_eq!(f.void(&["history", "footage", "cuts/empty", "--json"]).ok().json()["versions"], json!([]));
    let r = f.void(&["history", "footage", "nothing/", "--json"]).fails(1);
    assert_eq!((r.error()["code"].as_str(), r.error()["status"].as_u64()), (Some("NoSuchKey"), Some(404)));
    // Each time is a point the folder can be rolled back to: here, before the empty folder was made.
    f.void(&["restore", "footage", "cuts", "--at", &b[0].last_modified]).ok();
    assert_eq!(f.text("footage", "cuts/a.txt"), "a1");
    let r = f.void(&["history", "footage", "cuts/empty/"]).fails(1);
    assert!(r.stderr.starts_with("error: cuts/empty/ was deleted at "), "{}", r.stderr);
}

#[test]
fn deleted_files_say_how_to_bring_them_back() {
    let f = Fixture::new("deleted");
    f.drive("footage");
    f.put("footage", "cuts/a.txt", "one");
    let last = f.put("footage", "cuts/a.txt", "two");
    f.block(f.client.delete_object("footage", "cuts/a.txt", Default::default())).unwrap();
    let deleted = f.block(f.client.list_deleted("footage", "cuts/a.txt")).unwrap();
    let hint = format!("cuts/a.txt was deleted at {}; `void restore footage cuts/a.txt --version {last}` brings it back", deleted[0].deleted_at);
    let out = f.path("out");
    for args in [&["history", "footage", "cuts/a.txt"][..], &["show", "footage", "cuts/a.txt"], &["restore", "footage", "cuts/a.txt", "--at", "2030-01-01T00:00:00Z"]] {
        let r = f.void(args).fails(1);
        assert_eq!(r.stderr.trim(), format!("error: {hint}"), "{args:?}");
        let mut with_json = args.to_vec();
        with_json.push("--json");
        if args[0] == "show" {
            with_json.extend(["-o", out.to_str().unwrap()]);
        }
        let e = f.void(&with_json).fails(1).error();
        assert_eq!((e["code"].as_str(), e["message"].as_str()), (Some("NoSuchKey"), Some(hint.as_str())), "{args:?}");
        assert_eq!(e["deleted"], serde_json::to_value(&deleted[0]).unwrap());
    }
    assert_eq!(f.void(&["show", "footage", "cuts/a.txt", "--version", &last]).ok().out(), "two", "a deleted file's versions can be read");
    f.void(&["restore", "footage", "cuts/a.txt", "--version", &last]).ok();
    assert_eq!(f.text("footage", "cuts/a.txt"), "two");
    // A name that needs quoting, quoted.
    f.put("footage", "my file.txt", "x");
    f.block(f.client.delete_object("footage", "my file.txt", Default::default())).unwrap();
    let r = f.void(&["history", "footage", "my file.txt"]).fails(1);
    assert!(r.stderr.contains("`void restore footage 'my file.txt' --version "), "{}", r.stderr);
    // A folder too, named without its `/`, with the command the hint gives.
    f.put("footage", "cuts/old/", "");
    f.block(f.client.delete_object("footage", "cuts/old/", Default::default())).unwrap();
    let r = f.void(&["history", "footage", "cuts/old"]).fails(1);
    let command = r.stderr.split('`').nth(1).unwrap_or_else(|| panic!("{}", r.stderr)).to_owned();
    assert!(command.starts_with("void restore footage cuts/old/ --version "), "{}", r.stderr);
    f.void(&command.split(' ').skip(1).collect::<Vec<_>>()).ok();
    assert_eq!(f.block(f.client.head_object("footage", "cuts/old/", ReadOptions::default())).unwrap().kind, Kind::Folder);
}

#[test]
fn files_a_folder_restore_took_out_come_back_with_the_hint() {
    let f = Fixture::new("restored-away");
    f.drive("footage");
    f.put("footage", "cuts/a.txt", "one");
    let at = f.block(f.client.list_versions("footage", "cuts/a.txt", false)).unwrap()[0].last_modified.clone();
    f.put("footage", "cuts/new.txt", "new");
    let last = f.put("footage", "cuts/new.txt", "newer");
    f.void(&["restore", "footage", "cuts", "--at", &at]).ok();
    let r = f.void(&["history", "footage", "cuts/new.txt"]).fails(1);
    let command = r.stderr.split('`').nth(1).unwrap_or_else(|| panic!("{}", r.stderr)).to_owned();
    assert_eq!(command, format!("void restore footage cuts/new.txt --version {last}"));
    f.void(&command.split(' ').skip(1).collect::<Vec<_>>()).ok();
    assert_eq!(f.text("footage", "cuts/new.txt"), "newer");
}

#[test]
fn show_prints_a_file_as_it_was() {
    let f = Fixture::new("show");
    f.drive("footage");
    let v1 = f.put("footage", "a.txt", "first");
    std::thread::sleep(Duration::from_millis(1100));
    let between = chrono::Utc::now().timestamp().to_string();
    std::thread::sleep(Duration::from_millis(1100));
    let body: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    f.block(f.client.put_object("footage", "a.txt", body.clone(), PutOptions::default())).unwrap();
    assert!(f.void(&["show", "footage", "a.txt"]).ok().stdout == body, "the whole file, byte for byte");
    assert_eq!(f.void(&["show", "--bucket", "footage", "a.txt", "--version", &v1]).ok().out(), "first");
    let history = f.block(f.client.list_versions("footage", "a.txt", false)).unwrap();
    assert_eq!(f.void(&["show", "footage", "a.txt", "--at", &history[0].last_modified]).ok().out(), "first", "a time from the history");
    assert_eq!(f.void(&["show", "footage", "a.txt", "--at", &between]).ok().out(), "first", "Unix seconds");
    assert!(f.void(&["show", "footage", "a.txt", "--at", &chrono::Utc::now().timestamp().to_string()]).ok().stdout == body);
    let r = f.void(&["show", "footage", "a.txt", "--at", "1"]).fails(1);
    assert_eq!(r.stderr.trim(), "error: a.txt did not exist at 1970-01-01T00:00:01.000000Z");
    let r = f.void(&["show", "footage", "a.txt", "--at", "1", "--json", "-o", "x"]).fails(1);
    assert_eq!(r.error()["code"], "NoSuchVersion");

    let out = f.path("out.txt");
    let o = out.to_str().unwrap();
    let text = f.void(&["show", "footage", "a.txt", "--version", &v1, "-o", o]).ok().out();
    assert_eq!(text.trim(), format!("Wrote 5 B to {o} (footage:a.txt, version {v1})."));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "first");
    let head = f.block(f.client.head_object("footage", "a.txt", ReadOptions::default())).unwrap();
    let v = f.void(&["show", "footage", "a.txt", "--json", "--output", o]).ok().json();
    assert_eq!(v, json!({ "drive": "footage", "key": "a.txt", "versionId": head.version_id, "etag": head.etag, "size": body.len(), "lastModified": head.last_modified, "mtime": head.mtime, "output": o }));
    assert!(std::fs::read(&out).unwrap() == body);
    assert_eq!(f.void(&["show", "footage", "a.txt", "--version", &v1, "-o", "-"]).ok().out(), "first", "-o - is stdout");
    // A read that fails leaves the file as it was, and nothing beside it.
    let r = f.void(&["show", "footage", "a.txt", "--version", "999.0", "-o", o, "--json"]).fails(1);
    assert_eq!(r.error()["code"], "NoSuchVersion");
    assert!(std::fs::read(&out).unwrap() == body);
    assert_eq!(std::fs::read_dir(&f.dir).unwrap().count(), 1, "no partial file left");

    let r = f.void(&["show", "footage", "a.txt", "--json"]).fails(2);
    assert!(r.error()["message"].as_str().unwrap().starts_with("--json needs -o <file>"), "{}", r.stderr);
    let r = f.void(&["show", "footage", "a.txt", "--at", "1", "--version", &v1, "--json"]).fails(2);
    assert_eq!(r.error()["code"], "Usage");
    let r = f.void(&["show", "footage", "a.txt", "--at", "last tuesday"]).fails(1);
    assert!(r.stderr.contains("--at \"last tuesday\": give a time as RFC 3339"), "{}", r.stderr);
    f.put("footage", "dir/x.txt", "x");
    let r = f.void(&["show", "footage", "dir/", "--json", "-o", o]).fails(1);
    assert_eq!(r.error()["message"], "dir/ is a folder: show prints a file. `void history footage dir/` lists its versions");
}

#[test]
fn show_stops_quietly_when_stdout_closes() {
    let f = Fixture::new("pipe");
    f.drive("footage");
    f.block(f.client.put_object("footage", "big", vec![7u8; 32 << 20], PutOptions::default())).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_void"));
    cmd.args(["show", "footage", "big"]).env_clear().stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    for (k, v) in f.env() {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let mut first = [0u8; 16];
    child.stdout.as_mut().unwrap().read_exact(&mut first).unwrap();
    drop(child.stdout.take());
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline, "void didn't stop");
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert_eq!((status.code(), err.as_str()), (Some(0), ""), "as `| head` would close it");
}

#[test]
fn restore_rolls_back_files_and_folders() {
    let f = Fixture::new("restore");
    f.drive("footage");
    let v1 = f.put("footage", "cuts/a.txt", "one");
    f.put("footage", "cuts/a.txt", "two");
    let text = f.void(&["restore", "footage", "cuts/a.txt", "--version", &v1]).ok().out();
    let head = f.block(f.client.head_object("footage", "cuts/a.txt", ReadOptions::default())).unwrap();
    assert_eq!(text.trim(), format!("Restored footage:cuts/a.txt to version {v1}, as version {}.", head.version_id));
    assert_eq!(f.text("footage", "cuts/a.txt"), "one");
    let history = f.block(f.client.list_versions("footage", "cuts/a.txt", false)).unwrap();
    let v = f.void(&["restore", "--bucket", "footage", "cuts/a.txt", "--at", &history[1].last_modified, "--json"]).ok().json();
    let head = f.block(f.client.head_object("footage", "cuts/a.txt", ReadOptions::default())).unwrap();
    assert_eq!(v, json!({ "drive": "footage", "key": "cuts/a.txt", "versionId": head.version_id, "restoredFrom": history[1].version_id, "at": history[1].last_modified, "etag": head.etag, "size": 3 }));
    assert_eq!(f.text("footage", "cuts/a.txt"), "two");

    // A folder, named without its `/`: changed files come back, new ones go.
    let at = f.block(f.client.list_versions("footage", "cuts/a.txt", false)).unwrap().last().unwrap().last_modified.clone();
    f.put("footage", "cuts/a.txt", "three");
    f.put("footage", "cuts/new.txt", "new");
    let text = f.void(&["restore", "footage", "cuts", "--at", &at]).ok().out();
    let folder = f.block(f.client.list_versions("footage", "cuts/", false)).unwrap();
    assert_eq!(text.trim(), format!("Restored footage:cuts/ to its state at {at}, as version {}.", folder.last().unwrap().version_id));
    assert_eq!(f.text("footage", "cuts/a.txt"), "two");
    assert!(f.block(f.client.head_object("footage", "cuts/new.txt", ReadOptions::default())).unwrap_err().is_not_found());
    let v = f.void(&["restore", "footage", "cuts/", "--at", &at, "--json"]).ok().json();
    assert_eq!((v["key"].as_str(), v["restoredFrom"].as_null(), v["at"].as_str()), (Some("cuts/"), Some(()), Some(at.as_str())));

    let r = f.void(&["restore", "footage", "cuts/a.txt", "--json"]).fails(2);
    assert_eq!(r.error()["code"], "Usage");
    assert!(f.void(&["restore", "footage", "cuts/a.txt"]).fails(2).stderr.contains("--at <TIME>|--version <ID>"));
    let r = f.void(&["restore", "footage", "cuts", "--at", "1"]).fails(1);
    assert_eq!(r.stderr.trim(), "error: cuts/ did not exist at 1970-01-01T00:00:01.000000Z");
    let r = f.void(&["restore", "footage", "cuts/a.txt", "--version", "999.0", "--json"]).fails(1);
    assert_eq!((r.error()["code"].as_str(), r.error()["status"].as_u64()), (Some("NoSuchVersion"), Some(404)));
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn upload_puts_files_and_folders() {
    let f = Fixture::new("upload");
    f.drive("footage");
    let clip = f.write("src/clip.mov", b"clip bytes");
    let when = std::time::UNIX_EPOCH + Duration::from_secs(1_577_934_245);
    std::fs::File::options().write(true).open(&clip).unwrap().set_modified(when).unwrap();
    #[cfg(unix)]
    set_mode(&clip, 0o640);
    f.write("src/renders/a/1.txt", b"one");
    f.write("src/renders/b.txt", b"bee");
    std::fs::create_dir_all(f.path("src/renders/empty")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&clip, f.path("src/renders/link")).unwrap();
    let (c, renders) = (clip.to_str().unwrap().to_owned(), f.path("src/renders").to_str().unwrap().to_owned());

    let r = f.void(&["upload", &c, &renders, "footage:/cuts"]).ok();
    let text = r.out();
    let mut got = lines(&text);
    let summary = got.pop().unwrap();
    got.sort();
    assert_eq!(got, [
        format!("uploaded {c} -> footage:cuts/clip.mov (10 B)"),
        format!("uploaded {renders}/a/1.txt -> footage:cuts/renders/a/1.txt (3 B)"),
        format!("uploaded {renders}/b.txt -> footage:cuts/renders/b.txt (3 B)"),
    ]);
    assert!(summary.starts_with("Uploaded 3 files (16 B) and 1 empty folder to footage:cuts/ in "), "{summary}");
    #[cfg(unix)]
    assert_eq!(r.stderr.trim(), format!("warning: skipped {renders}/link: a symbolic link"));
    assert_eq!(f.text("footage", "cuts/clip.mov"), "clip bytes");
    assert_eq!(f.text("footage", "cuts/renders/a/1.txt"), "one");
    assert_eq!(f.block(f.client.head_object("footage", "cuts/renders/empty/", ReadOptions::default())).unwrap().kind, Kind::Folder);
    let m = f.block(f.client.head_object("footage", "cuts/clip.mov", ReadOptions::default())).unwrap();
    assert_eq!(m.mtime.as_deref(), Some("2020-01-02T03:04:05.000000Z"), "the file's modification time is kept");
    #[cfg(unix)]
    assert_eq!(m.mode.as_deref(), Some("0640"), "and its permissions");

    f.write("src/renders/b.txt", b"bee 2");
    let v = f.void(&["upload", &renders, "footage:", "--json"]).ok().json();
    let b = f.block(f.client.head_object("footage", "renders/b.txt", ReadOptions::default())).unwrap();
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[1], json!({ "path": format!("{renders}/b.txt"), "key": "renders/b.txt", "size": 5, "versionId": b.version_id, "etag": b.etag }));
    let skipped = if cfg!(unix) { json!([{ "path": format!("{renders}/link"), "reason": "a symbolic link" }]) } else { json!([]) };
    let empty = f.block(f.client.head_object("footage", "renders/empty/", ReadOptions::default())).unwrap();
    assert_eq!(
        (&v["drive"], &v["folder"], &v["bytes"], &v["folders"], &v["skipped"], &v["failed"], &v["notAttempted"]),
        (&json!("footage"), &json!(""), &json!(8), &json!([{ "key": "renders/empty/", "versionId": empty.version_id }]), &skipped, &json!([]), &json!(0))
    );
    assert!(f.void(&["upload", &renders, "footage:", "--json"]).ok().stderr.is_empty(), "warnings stay out of JSON");
}

#[test]
fn upload_sends_large_files_in_parts() {
    let f = Fixture::new("multipart");
    f.drive("footage");
    let body: Vec<u8> = (0..(11 << 20) + 3).map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 11) as u8).collect();
    let big = f.write("big.bin", &body);
    let when = std::time::UNIX_EPOCH + Duration::from_secs(1_577_934_245);
    std::fs::File::options().write(true).open(&big).unwrap().set_modified(when).unwrap();
    #[cfg(unix)]
    set_mode(&big, 0o600);
    let small = f.write("small.bin", &body[..(6 << 20) - 1]);
    let v = f.void(&["upload", big.to_str().unwrap(), small.to_str().unwrap(), "footage:/media", "--json", "--multipart-threshold", "6", "--part-size", "5"]).ok().json();
    let files = v["files"].as_array().unwrap();
    assert_eq!((files[0]["parts"].as_u64(), files[0]["size"].as_u64()), (Some(3), Some(body.len() as u64)), "5, 5 and 1 MiB");
    assert!(files[1].get("parts").is_none(), "under the threshold: one put");
    let got = f.block(f.client.get_object("footage", "media/big.bin", Default::default())).unwrap();
    assert!(got.body == body, "the parts in order");
    assert_eq!(files[0]["versionId"].as_str(), Some(got.meta.version_id.as_str()));
    assert_eq!(got.meta.mtime.as_deref(), Some("2020-01-02T03:04:05.000000Z"), "attributes go with the upload's first request");
    #[cfg(unix)]
    assert_eq!(got.meta.mode.as_deref(), Some("0600"));
    assert_eq!(f.block(f.client.list_versions("footage", "media/big.bin", false)).unwrap().len(), 1, "one version");
    // Room for two parts in memory: the third waits for one to finish, which must not wait on it.
    let v = f.void(&["upload", big.to_str().unwrap(), "footage:/tight", "--json", "--multipart-threshold", "6", "--part-size", "5", "--memory-budget", "10"]).ok().json();
    assert_eq!(v["files"][0]["parts"], 3);
    assert!(f.block(f.client.get_object("footage", "tight/big.bin", Default::default())).unwrap().body == body);
    let uploads = f.block(f.client.s3().list_multipart_uploads().bucket("footage").send()).unwrap();
    assert!(uploads.uploads().is_empty(), "none left open");

    // A failed upload is aborted, not left open: a file where a folder would go fails it at the end.
    f.put("footage", "blocked", "a file");
    let r = f.void(&["upload", big.to_str().unwrap(), "footage:/blocked", "--multipart-threshold", "6", "--part-size", "5", "--json"]).fails(1);
    assert_eq!(r.json()["failed"][0]["error"]["code"], "PathConflict", "{}", r.out());
    let uploads = f.block(f.client.s3().list_multipart_uploads().bucket("footage").send()).unwrap();
    assert!(uploads.uploads().is_empty(), "the failed upload was aborted");
    let r = f.void(&["upload", big.to_str().unwrap(), "footage:", "--part-size", "4"]).fails(2);
    assert!(r.stderr.contains("--part-size is at least 5 MiB"), "{}", r.stderr);
}

#[test]
fn upload_stops_at_the_first_failure_and_says_so() {
    let f = Fixture::with_keys("upload-errors", vec![KeyInfo { id: "VFREADKEY23456723456".into(), secret: "r".repeat(40), scope: Scope::Read, drives: None }]);
    f.drive("footage");
    let a = f.write("a.txt", b"a");
    let a = a.to_str().unwrap();
    let r = f.void(&["upload", a, "footage"]).fails(2);
    assert!(r.stderr.contains("the destination \"footage\" is `<drive>:[/folder]`"), "{}", r.stderr);
    let r = f.void(&["upload", a, f.path("missing").to_str().unwrap(), "footage:", "--json"]).fails(1);
    assert_eq!(r.error()["code"], "LocalFileError");
    assert!(f.block(f.client.head_object("footage", "a.txt", ReadOptions::default())).unwrap_err().is_not_found(), "nothing uploaded when a source is missing");

    let r = f.void(&["upload", a, "nowhere:/x", "--json"]).fails(1);
    let v = r.json();
    assert_eq!((v["files"].as_array().unwrap().len(), v["failed"][0]["key"].as_str(), v["failed"][0]["error"]["code"].as_str()), (0, Some("x/a.txt"), Some("NoSuchBucket")));
    assert_eq!(r.error()["code"], "NoSuchBucket");
    assert!(r.error()["message"].as_str().unwrap().starts_with("1 of 1 failed; the first: "), "{}", r.stderr);

    let many: Vec<String> = (0..40).map(|i| f.write(&format!("many/{i:02}.txt"), b"x").to_str().unwrap().to_owned()).collect();
    let mut args: Vec<&str> = vec!["upload"];
    args.extend(many.iter().map(String::as_str));
    args.extend(["footage:", "--json"]);
    let read_env = [("VOIDFS_ENDPOINT", f.endpoint.clone()), ("VOIDFS_ACCESS_KEY_ID", "VFREADKEY23456723456".into()), ("VOIDFS_SECRET_ACCESS_KEY", "r".repeat(40))];
    let r = run(&args, &read_env, None).fails(1);
    let v = r.json();
    let failed = v["failed"].as_array().unwrap().len() as u64;
    assert_eq!(v["files"], json!([]));
    assert!(failed >= 1 && v["notAttempted"].as_u64().unwrap() > 0, "files not started once one failed: {v}");
    assert_eq!(failed + v["notAttempted"].as_u64().unwrap(), 40);
    assert_eq!(r.error()["code"], "AccessDenied");
    let text = run(&args[..args.len() - 1], &read_env, None).fails(1);
    assert!(text.out().contains(" failed; ") && text.out().contains(" not attempted."), "{}", text.out());
    assert!(text.stderr.starts_with(&format!("error: {failed} of 40 failed; the first: ")), "{}", text.stderr);
}

#[test]
fn configuration_comes_from_flags_a_key_file_or_the_environment() {
    let f = Fixture::new("config");
    f.drive("footage");
    let r = run(&["drives"], &[], None).fails(1);
    assert!(r.stderr.starts_with("error: no access key: set VOIDFS_ACCESS_KEY_ID and VOIDFS_SECRET_ACCESS_KEY, or pass --key-file"), "{}", r.stderr);
    assert_eq!(run(&["drives", "--json"], &[], None).fails(1).error()["code"], "NoCredentials");
    let flags = ["--endpoint", &f.endpoint, "--access-key-id", ADMIN_KEY_ID, "--secret-access-key", ADMIN_SECRET];
    let mut args = vec!["drives", "--json"];
    args.extend(flags);
    assert_eq!(run(&args, &[("VOIDFS_ENDPOINT", "http://127.0.0.1:1".into())], None).ok().json()["drives"][0]["alias"], "footage", "flags win");
    let key = f.write("admin.env", format!("VOIDFS_ACCESS_KEY_ID={ADMIN_KEY_ID}\nVOIDFS_SECRET_ACCESS_KEY={ADMIN_SECRET}\n").as_bytes());
    let env = [("VOIDFS_ENDPOINT", f.endpoint.clone()), ("VOIDFS_KEY_FILE", key.to_str().unwrap().to_owned()), ("VOIDFS_ACCESS_KEY_ID", "VFWRONGWRONGWRONG234".into()), ("VOIDFS_SECRET_ACCESS_KEY", "x".repeat(40))];
    run(&["drives"], &env, None).ok();
    let mut wrong = f.env();
    wrong[2].1 = "y".repeat(40);
    let r = run(&["drives", "--json"], &wrong, None).fails(1);
    assert_eq!((r.error()["code"].as_str(), r.error()["status"].as_u64()), (Some("SignatureDoesNotMatch"), Some(403)));
    let mut nowhere = f.env();
    nowhere[0].1 = "http://127.0.0.1:1".into();
    let r = run(&["drive", "show", "footage", "--json"], &nowhere, None).fails(1);
    assert_eq!((r.error()["code"].as_str(), r.error()["sent"].as_bool()), (Some("RequestFailed"), Some(false)));
}

#[test]
fn usage_errors_are_json_with_json() {
    let r = run(&["drive", "create", "--json"], &[], None).fails(2);
    let e = r.error();
    assert_eq!(e["code"], "Usage");
    assert!(e["message"].as_str().unwrap().contains("required arguments were not provided"), "{e}");
    assert_eq!(run(&["nosuch", "--json"], &[], None).fails(2).error()["code"], "Usage");
    let r = run(&["nosuch"], &[], None).fails(2);
    assert!(r.stderr.starts_with("error: unrecognized subcommand 'nosuch'"), "{}", r.stderr);
    let help = run(&["--help", "--json"], &[], None).ok().out();
    assert!(help.contains("Usage: void [OPTIONS] <COMMAND>"), "{help}");
    let help = run(&["show", "--help"], &[], None).ok().out();
    assert!(help.contains("--at <TIME>") && help.contains("--bucket <DRIVE>"), "{help}");
}
