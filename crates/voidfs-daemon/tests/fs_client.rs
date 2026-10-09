// SPDX-License-Identifier: Apache-2.0
//! Hostile daemon answers through a real Unix socket exercise the typed client's bounds.

use std::convert::Infallible;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Response, header};
use axum::Router;
use bytes::Bytes;
use serde_json::{Value, json};
use voidfs_daemon::api::fs;
use voidfs_daemon::{DaemonClient, FsClientError};

#[derive(Clone)]
struct Reply { chunks: Vec<Bytes>, streamed: bool }

impl Reply {
    fn bytes(bytes: impl Into<Bytes>) -> Self { Self { chunks: vec![bytes.into()], streamed: false } }
    fn json(value: &Value) -> Self { Self::bytes(serde_json::to_vec(value).unwrap()) }
    fn stream(chunks: Vec<Bytes>) -> Self { Self { chunks, streamed: true } }
    fn response(self) -> Response<Body> {
        let mut response = Response::builder().header(header::CONTENT_TYPE, "application/json");
        let body = if self.streamed {
            Body::from_stream(futures::stream::iter(self.chunks.into_iter().map(Ok::<_, Infallible>)))
        } else {
            let bytes = self.chunks.into_iter().next().unwrap();
            response = response.header(header::CONTENT_LENGTH, bytes.len());
            Body::from(bytes)
        };
        response.body(body).unwrap()
    }
}

#[derive(Clone)]
struct Replies { session: Reply, call: Reply }

async fn answer(State(replies): State<Replies>, request: Request) -> Response<Body> {
    if request.uri().path() == "/v1/fs/sessions" { replies.session.response() } else { replies.call.response() }
}

struct Fake { _dir: tempfile::TempDir, client: DaemonClient, task: tokio::task::JoinHandle<()> }

impl Drop for Fake { fn drop(&mut self) { self.task.abort(); } }

async fn fake(session: Reply, call: Reply) -> Fake {
    let dir = tempfile::Builder::new().prefix("vdfake").tempdir_in(std::env::temp_dir()).unwrap();
    let socket = dir.path().join("s");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let app = Router::new().fallback(answer).with_state(Replies { session, call });
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    Fake { _dir: dir, client: DaemonClient::new(&socket), task }
}

fn info() -> Value {
    json!({"version":fs::VERSION,"id":"1-a","drive":"drive","root":1,"generation":1,
        "metadataGeneration":7,"readOnly":true,"maxIo":fs::MAX_IO,"maxEntries":fs::MAX_ENTRIES,
        "capabilities":{"hardLinks":false,"exchange":false,"exclusiveRename":true,"clone":false,"locks":"local",
        "caseSensitive":true,"nfcNames":true,"persistentIds":true,"xattrs":true,"maxXattrBytes":65536,"maxNameBytes":255,"maxPathBytes":1024}})
}

fn attr() -> Value {
    json!({"ino":1,"kind":"file","size":1,"mtime":"1969-01-01T00:00:00Z","mode":420,"generation":1,
        "sync":"Saved","object_id":"o","version_id":"v","etag":"e","has_xattrs":false,"target":null})
}

#[track_caller]
fn failed<T: std::fmt::Debug>(result: Result<T, FsClientError>, message: &str, case: &str) {
    match result {
        Err(FsClientError::Failed(reason)) => assert!(reason.contains(message), "{case}: {reason}"),
        other => panic!("{case}: expected failure containing {message:?}, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_session_identities_and_finite_response_limits_are_refused() {
    for (field, value) in [("version", json!(fs::VERSION + 1)), ("generation", json!(0)), ("id", json!("")),
        ("id", json!("x/y?z")), ("id", json!("a".repeat(257))), ("maxIo", json!(0)),
        ("maxIo", json!(fs::MAX_IO + 1)), ("maxEntries", json!(0)), ("maxEntries", json!(fs::MAX_ENTRIES + 1))]
    {
        let mut session = info();
        session[field] = value;
        let f = fake(Reply::json(&session), Reply::bytes("{}")).await;
        failed(f.client.session("drive", true).await, "invalid filesystem session", field);
    }
    let mut bare = info();
    bare.as_object_mut().unwrap().remove("capabilities");
    let f = fake(Reply::json(&bare), Reply::bytes("{}")).await;
    failed(f.client.session("drive", true).await, "capabilities", "a session without capabilities");
    let good = info();
    let mut huge = vec![b' '; fs::MAX_RESPONSE + 1];
    huge.extend_from_slice(&serde_json::to_vec(&good).unwrap());
    for streamed in [false, true] {
        let reply = if streamed { Reply::stream(vec![Bytes::from(huge.clone())]) } else { Reply::bytes(huge.clone()) };
        let f = fake(reply, Reply::bytes("{}")).await;
        failed(f.client.session("drive", true).await, "size limit", "oversized JSON");
        let reply = if streamed { Reply::stream(vec![Bytes::from_static(b"12345")]) } else { Reply::bytes("12345") };
        let f = fake(Reply::json(&good), reply).await;
        let session = f.client.session("drive", true).await.unwrap();
        failed(session.read(1, 0, 4).await, "size limit", "oversized binary read");
    }
    let f = fake(Reply::json(&good), Reply::json(&json!({"written":2}))).await;
    let session = f.client.session("drive", true).await.unwrap();
    failed(session.write(1, 0, Bytes::from_static(b"x")).await, "bytes sent", "oversized write count");
    let f = fake(Reply::json(&good), Reply::json(&json!({"entries":[["a",attr()],["b",attr()]],"generation":7}))).await;
    let session = f.client.session("drive", true).await.unwrap();
    failed(session.readdir(1, None, 1).await, "requested entry limit", "oversized directory page");
}

fn event(generation: u64, resync: bool, all: bool) -> Value {
    json!({"generation":generation,"seq":9,"resync":resync,"invalidations":if all { json!([{"kind":"all"}]) }
        else { json!([{"kind":"object","key":"café"}]) },"inodes":[]})
}

fn line(event: &Value) -> Vec<u8> { let mut bytes = serde_json::to_vec(event).unwrap(); bytes.push(b'\n'); bytes }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalidation_frames_require_full_resync_and_increasing_generations() {
    let first = line(&event(7, true, true));
    let next = line(&event(8, false, false));
    let split = next.iter().position(|b| *b == 0xc3).unwrap() + 1;
    let f = fake(Reply::json(&info()), Reply::stream(vec![Bytes::from(first.clone()), Bytes::copy_from_slice(&next[..split]), Bytes::copy_from_slice(&next[split..])])).await;
    let session = f.client.session("drive", true).await.unwrap();
    for _ in 0..2 {
        let mut watch = session.watch().await.unwrap();
        assert_eq!(watch.next().await.unwrap().unwrap().generation, 7);
        let next = watch.next().await.unwrap().unwrap();
        assert_eq!((next.generation, next.invalidations), (8, vec![fs::Invalidation::Object("café".into())]));
        assert!(watch.next().await.is_none());
    }
    let mut huge = vec![b' '; fs::MAX_RESPONSE + 1];
    huge.extend_from_slice(&first);
    let cases = [
        ("missing resync flag", line(&event(7, false, true)), 0, "initial resync"),
        ("missing all", line(&event(7, true, false)), 0, "initial resync"),
        ("initial generation before session", line(&event(6, true, true)), 0, "generation regressed"),
        ("equal generation", [first.clone(), line(&event(7, false, false))].concat(), 1, "generation regressed"),
        ("lower generation", [first.clone(), line(&event(6, false, false))].concat(), 1, "generation regressed"),
        ("malformed JSON", b"{\n".to_vec(), 0, "invalidation:"),
        ("missing newline", serde_json::to_vec(&event(7, true, true)).unwrap(), 0, "before its newline"),
        ("oversized frame", huge, 0, "response limit"),
    ];
    for (case, bytes, good_events, message) in cases {
        let f = fake(Reply::json(&info()), Reply::stream(vec![Bytes::from(bytes)])).await;
        let session = f.client.session("drive", true).await.unwrap();
        let mut watch = session.watch().await.unwrap();
        for _ in 0..good_events { watch.next().await.unwrap().unwrap(); }
        let result = tokio::time::timeout(Duration::from_secs(5), watch.next()).await.expect("bounded fake stream").expect(case);
        failed(result, message, case);
        assert!(watch.next().await.is_none(), "{case}: a rejected watch is terminal");
    }
}
