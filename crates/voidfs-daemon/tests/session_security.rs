// SPDX-License-Identifier: Apache-2.0
//! Kernel-authenticated session ownership through distinct client processes.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use bytes::Bytes;
use voidfs_client::mount::FsError;
use voidfs_daemon::api::{Build, Scope, fs};
use voidfs_daemon::{Daemon, DaemonClient, DaemonConfig};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

fn request(socket: &Path, method: &str, route: &str, generation: u32, body: &str) -> (u16, Vec<u8>) {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(stream, "{method} {route} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/octet-stream\r\nx-voidfs-generation: {generation}\r\nx-voidfs-peer-pid: {}\r\nContent-Length: {}\r\n\r\n{body}", std::env::var("VOIDFS_SESSION_OWNER_PID").unwrap_or_default(), body.len()).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let boundary = bytes.windows(4).position(|value| value == b"\r\n\r\n").unwrap() + 4;
    let status = std::str::from_utf8(&bytes[..boundary]).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, bytes[boundary..].to_vec())
}

fn child(socket: &Path) {
    let (status, _) = request(socket, "GET", "/v1/status", 0, "");
    assert_eq!(status, 200, "the same user remains authorized for daemon controls");
    let (status, body) = request(socket, "POST", "/v1/fs/sessions", 0, r#"{"version":1,"drive":"drive","readOnly":true}"#);
    assert_eq!(status, 200, "a different process may create its own session");
    let own: fs::SessionInfo = serde_json::from_slice(&body).unwrap();
    let id = std::env::var("VOIDFS_SESSION_FOREIGN_ID").unwrap();
    let generation = std::env::var("VOIDFS_SESSION_GENERATION").unwrap().parse().unwrap();
    let root = std::env::var("VOIDFS_SESSION_ROOT").unwrap();
    let fh = std::env::var("VOIDFS_SESSION_FH").unwrap();
    for (method, operation, body) in [
        ("POST", "getattr".to_owned(), format!("{{\"ino\":{root}}}")),
        ("GET", format!("read?fh={fh}&offset=0&length=4"), String::new()),
        ("PUT", format!("write?fh={fh}&offset=0"), "evil".to_owned()),
        ("GET", "watch".to_owned(), String::new()),
        ("POST", "create".to_owned(), format!("{{\"parent\":{root},\"name\":\"evil\",\"mode\":420}}")),
        ("POST", "rename".to_owned(), format!("{{\"fromParent\":{root},\"fromName\":\"data\",\"toParent\":{root},\"toName\":\"evil\",\"how\":\"replace\"}}")),
        ("POST", "setattr".to_owned(), format!("{{\"ino\":{root},\"mode\":511}}")),
        ("PUT", format!("setxattr?ino={root}&name=evil&how=set"), "evil".to_owned()),
        ("GET", format!("getxattr?ino={root}&name=evil"), String::new()),
        ("POST", "conflict".to_owned(), format!("{{\"ino\":{root}}}")),
        ("POST", "release".to_owned(), "{}".to_owned()),
    ] {
        let (status, body) = request(socket, method, &format!("/v1/fs/{id}/{operation}"), generation, &body);
        assert_eq!(status, 403, "another process must not use a known session ID: {operation}");
        let error: fs::FsErrorBody = serde_json::from_slice(&body).unwrap();
        assert_eq!((error.error.code.as_str(), error.error.errno), ("Permission", FsError::Permission.errno()));
    }
    let (status, _) = request(socket, "POST", &format!("/v1/fs/{}/release", own.id), own.generation, "{}");
    assert_eq!(status, 200, "reconnected sockets from the owning process remain accepted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_have_random_ids_and_are_owned_by_the_connecting_process() {
    if let Some(socket) = std::env::var_os("VOIDFS_SESSION_SECURITY_CHILD") { child(Path::new(&socket)); return; }
    let server = TestServer::start().await.unwrap();
    let cfg = voidfs_sdk::Config { endpoint: server.endpoint.replace("127.0.0.1", "localhost"), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..Default::default() };
    let remote = voidfs_sdk::Client::new(cfg.clone()).unwrap();
    remote.create_drive("drive", Default::default()).await.unwrap();
    remote.put_object("drive", "data", Bytes::from_static(b"safe"), Default::default()).await.unwrap();
    let dir = tempfile::Builder::new().prefix("vdsecure").tempdir_in(std::env::temp_dir()).unwrap();
    let daemon = Daemon::start(DaemonConfig::new(dir.path(), cfg, Build::default())).await.unwrap();
    let client = DaemonClient::new(daemon.socket());
    client.pause(&Scope::All).await.unwrap();
    let session = client.session("drive", false).await.unwrap();
    assert_eq!(session.info().id.len(), 64, "session identifiers contain 256 random bits");
    assert!(session.info().id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let other = client.session("drive", true).await.unwrap();
    assert_ne!(session.info().id, other.info().id);
    let attr = session.lookup(session.info().root, "data").await.unwrap();
    let fh = session.open(attr.ino, true).await.unwrap().fh;
    let executable = std::env::current_exe().unwrap();
    let socket = daemon.socket().to_owned();
    let info = session.info().clone();
    let output = tokio::time::timeout(Duration::from_secs(45), tokio::task::spawn_blocking(move || {
        Command::new(executable).args(["--exact", "sessions_have_random_ids_and_are_owned_by_the_connecting_process", "--nocapture"])
            .env("VOIDFS_SESSION_SECURITY_CHILD", socket)
            .env("VOIDFS_SESSION_FOREIGN_ID", info.id)
            .env("VOIDFS_SESSION_GENERATION", info.generation.to_string())
            .env("VOIDFS_SESSION_ROOT", info.root.to_string())
            .env("VOIDFS_SESSION_FH", fh.to_string())
            .env("VOIDFS_SESSION_OWNER_PID", std::process::id().to_string())
            .output().unwrap()
    })).await.expect("the second process finishes within its socket deadlines").unwrap();
    assert!(output.status.success(), "foreign process checks failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let socket = daemon.socket().to_owned();
    let info = session.info().clone();
    let (status, body) = tokio::task::spawn_blocking(move || request(&socket, "GET", &format!("/v1/fs/{}/read?fh={fh}&offset=0&length=4", info.id), info.generation, "")).await.unwrap();
    assert_eq!((status, body.as_slice()), (200, b"safe".as_slice()), "a fresh connection from the session owner is valid, and foreign writes did not land");
    let root = session.info().root;
    assert_eq!(session.readdir(root, None, 16).await.unwrap().entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["data"], "foreign namespace calls did not land");
    session.close(fh).await.unwrap();
    session.release().await.unwrap();
    other.release().await.unwrap();
    daemon.stop().await;
}
