// SPDX-License-Identifier: Apache-2.0
//! The daemon in this process, on a socket of its own, asked through `DaemonClient`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use voidfs_daemon::api::Build;
use voidfs_daemon::{ClientError, Daemon, DaemonClient, DaemonConfig, Error};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

/// A state directory short enough for its socket's path.
fn state_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(&format!("vdd{tag}")).tempdir_in(std::env::temp_dir()).unwrap()
}

fn config(server: &TestServer, dir: &Path) -> DaemonConfig {
    let client = voidfs_sdk::Config {
        endpoint: server.endpoint.replace("127.0.0.1", "localhost"),
        access_key_id: ADMIN_KEY_ID.into(),
        secret_access_key: ADMIN_SECRET.into(),
        ..voidfs_sdk::Config::default()
    };
    DaemonConfig::new(dir, client, Build { version: "9.9.9".into(), commit: "abcdef1".into(), commit_date: "2026-10-03".into() })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn it_answers_on_its_socket_and_lets_go_when_stopped() {
    let server = TestServer::start().await.unwrap();
    let dir = state_dir("a");
    let d = Daemon::start(config(&server, dir.path())).await.unwrap();
    let c = DaemonClient::new(d.socket());
    assert_eq!(d.socket(), dir.path().join("daemon.sock"));

    let s = c.status().await.unwrap();
    assert_eq!((s.daemon.pid, s.daemon.build.version.as_str(), s.daemon.access_key_id.as_str()), (std::process::id(), "9.9.9", ADMIN_KEY_ID));
    assert_eq!(s.connection.link, voidfs_daemon::api::Link::Online);
    assert_eq!(s.uploads, voidfs_daemon::api::Uploads::default());
    let i = c.info().await.unwrap();
    assert_eq!((i.restart_safe, i.memory_only_bytes, i.journal.unpublished), (true, 0, 0));
    assert_eq!(i.build.to_string(), "9.9.9 (abcdef1, 2026-10-03)");

    match c.get::<serde_json::Value>("/v1/nothing").await {
        Err(ClientError::Api { status: 404, code, message }) => assert_eq!((code.as_str(), message.as_str()), ("NotFound", "no such request: GET /v1/nothing")),
        other => panic!("{other:?}"),
    }

    let asked = tokio::spawn({
        let c = c.clone();
        async move { c.stop().await.unwrap() }
    });
    tokio::time::timeout(Duration::from_secs(10), d.stop_asked()).await.expect("the stop request reaches the daemon");
    assert!(asked.await.unwrap().stopping, "it answers before it stops");
    d.stop().await;
    assert!(!dir.path().join("daemon.sock").exists());
    assert!(matches!(c.status().await, Err(ClientError::NotRunning { .. })));
    // The state directory is free again.
    drop(voidfs_client::Store::open(dir.path()).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_daemon_waits_for_the_first_to_let_go() {
    let server = TestServer::start().await.unwrap();
    let dir = state_dir("b");
    let first = Daemon::start(config(&server, dir.path())).await.unwrap();
    let t = Instant::now();
    match Daemon::start(config(&server, dir.path())).await {
        Err(Error::Locked(d)) => assert_eq!(d, dir.path()),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("two daemons on one state directory"),
    }
    assert!(t.elapsed() >= Duration::from_secs(1), "it waits a little for one that is stopping");
    assert!(DaemonClient::new(first.socket()).status().await.is_ok(), "the first is untouched");

    // A daemon starting while the first stops gets the directory, and keeps its socket.
    let cfg = config(&server, dir.path());
    let second = tokio::spawn(Daemon::start(cfg));
    tokio::time::sleep(Duration::from_millis(100)).await;
    first.stop().await;
    let second = second.await.unwrap().unwrap();
    let s = DaemonClient::new(second.socket()).status().await.expect("the second daemon's socket answers");
    assert_eq!(s.daemon.pid, std::process::id());
    second.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sockets_are_made_where_they_fit() {
    let server = TestServer::start().await.unwrap();
    let dir = state_dir("c");
    let long: PathBuf = dir.path().join("y".repeat(voidfs_daemon::MAX_SOCKET_PATH));
    match Daemon::start(config(&server, &long)).await {
        Err(Error::Socket(m)) => assert!(m.contains("bytes, more than the") && m.contains("VOIDFS_STATE_DIR"), "{m}"),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("a socket path too long for a Unix socket"),
    }
    // It let go of the directory it couldn't serve.
    drop(voidfs_client::Store::open(&long).unwrap());
}
