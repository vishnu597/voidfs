// SPDX-License-Identifier: Apache-2.0
//! What the `void` binary's tests share: a server in this process, an admin client for it, a
//! scratch folder, and a way to run the binary with a deadline.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use voidfs_sdk::{Client, Config};
use voidfs_server::sigv4::KeyInfo;
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

/// A server on its own runtime, an admin client for it, and a scratch folder.
pub struct Fixture {
    pub rt: tokio::runtime::Runtime,
    pub server: Option<TestServer>,
    /// `http://localhost:<port>`: a name, not an address, so that the AWS client underneath
    /// would address drives virtual-host style if it weren't told to use paths.
    pub endpoint: String,
    pub client: Client,
    pub dir: PathBuf,
}

impl Fixture {
    pub fn new(name: &str) -> Fixture {
        Fixture::with_keys(name, Vec::new())
    }

    pub fn with_keys(name: &str, keys: Vec<KeyInfo>) -> Fixture {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let server = rt.block_on(TestServer::with_keys(keys)).unwrap();
        let endpoint = format!("http://localhost:{}", server.addr.port());
        let client = Client::new(Config { endpoint: endpoint.clone(), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..Config::default() }).unwrap();
        let dir = std::env::temp_dir().join(format!("void-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture { rt, server: Some(server), endpoint, client, dir }
    }

    pub fn block<F: Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    pub fn env(&self) -> Vec<(&'static str, String)> {
        vec![("VOIDFS_ENDPOINT", self.endpoint.clone()), ("VOIDFS_ACCESS_KEY_ID", ADMIN_KEY_ID.into()), ("VOIDFS_SECRET_ACCESS_KEY", ADMIN_SECRET.into())]
    }

    /// `void <args>` with the admin key.
    pub fn void(&self, args: &[&str]) -> Run {
        run(args, &self.env(), None)
    }

    pub fn put(&self, drive: &str, key: &str, body: &str) -> String {
        self.block(self.client.put_object(drive, key, body.to_owned(), Default::default())).unwrap().version_id
    }

    pub fn text(&self, drive: &str, key: &str) -> String {
        String::from_utf8(self.block(self.client.get_object(drive, key, Default::default())).unwrap().body.to_vec()).unwrap()
    }

    pub fn drive(&self, name: &str) {
        self.block(self.client.create_drive(name, Default::default())).unwrap();
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    pub fn write(&self, rel: &str, body: &[u8]) -> PathBuf {
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
pub struct Run {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Run {
    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    #[track_caller]
    pub fn ok(self) -> Run {
        assert_eq!(self.status, 0, "stdout: {}\nstderr: {}", self.out(), self.stderr);
        self
    }

    #[track_caller]
    pub fn fails(self, status: i32) -> Run {
        assert_eq!(self.status, status, "stdout: {}\nstderr: {}", self.out(), self.stderr);
        self
    }

    /// The JSON document on stdout.
    #[track_caller]
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}\nstderr: {}", self.out(), self.stderr))
    }

    /// The `error` member of the JSON on stderr.
    #[track_caller]
    pub fn error(&self) -> Value {
        let v: Value = serde_json::from_str(&self.stderr).unwrap_or_else(|e| panic!("stderr is not JSON ({e}): {}", self.stderr));
        v["error"].clone()
    }
}

/// Runs the binary with only `env` for an environment, `stdin` fed and closed, and a deadline.
pub fn run(args: &[&str], env: &[(&str, String)], stdin: Option<&str>) -> Run {
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

pub fn lines(s: &str) -> Vec<String> {
    s.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

