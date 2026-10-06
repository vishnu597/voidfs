// SPDX-License-Identifier: Apache-2.0
//! Synthetic protocol answers cover failures a conforming test server cannot produce.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::sync::Notify;
use voidfs_client::mount::{FsError, Session};
use voidfs_client::{ApiFetcher, Cache, CacheConfig, Connectivity, Invalidation, Store};
use voidfs_sdk::{Client, Config};

struct Answer {
    page: Value,
    gate: Option<Arc<Notify>>,
}

impl From<Value> for Answer {
    fn from(page: Value) -> Self { Self { page, gate: None } }
}

struct FixtureState {
    answers: Mutex<VecDeque<Answer>>,
    requests: Mutex<Vec<HashMap<String, String>>>,
    requested: Notify,
    delay: Mutex<Duration>,
    route_by_prefix: Mutex<bool>,
}

struct Fixture {
    endpoint: String,
    state: Arc<FixtureState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) { self.task.abort(); }
}

impl Fixture {
    async fn new(answers: Vec<Answer>) -> Self {
        let state = Arc::new(FixtureState {
            answers: Mutex::new(answers.into()),
            requests: Mutex::new(Vec::new()),
            requested: Notify::new(),
            delay: Mutex::new(Duration::ZERO),
            route_by_prefix: Mutex::new(false),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().fallback(answer).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        Self { endpoint, state, task }
    }

    fn client(&self) -> Client {
        Client::new(Config {
            endpoint: self.endpoint.clone(),
            access_key_id: "fixture".into(),
            secret_access_key: "fixture".into(),
            max_attempts: 1,
            timeout: Duration::from_secs(5),
            ..Default::default()
        }).unwrap()
    }

    async fn session(&self, store: Arc<Store>, connectivity: Connectivity) -> Session {
        let client = self.client();
        let cache = Cache::open(store.clone(), Arc::new(ApiFetcher::new(client.clone()).with_connectivity(connectivity.clone())),
            CacheConfig { min_free_bytes: 0, ..Default::default() }).await.unwrap();
        Session::new(store, client, cache, "drv", connectivity).await.unwrap()
    }

    fn requests(&self) -> Vec<HashMap<String, String>> {
        self.state.requests.lock().unwrap().clone()
    }

    async fn wait_for_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let requested = self.state.requested.notified();
                if self.state.requests.lock().unwrap().len() >= count { return; }
                requested.await;
            }
        }).await.expect("fixture received the expected request");
    }
}

async fn answer(State(state): State<Arc<FixtureState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let prefix = query.get("prefix").cloned().unwrap_or_default();
    state.requests.lock().unwrap().push(query);
    let answer = {
        let mut answers = state.answers.lock().unwrap();
        if *state.route_by_prefix.lock().unwrap() {
            let position = answers.iter().position(|a| a.page["prefix"].as_str() == Some(prefix.as_str()));
            position.and_then(|i| answers.remove(i))
        } else { answers.pop_front() }
    };
    let delay = *state.delay.lock().unwrap();
    state.requested.notify_one();
    let Some(answer) = answer else { return StatusCode::INTERNAL_SERVER_ERROR.into_response(); };
    if !delay.is_zero() { tokio::time::sleep(delay).await; }
    if let Some(gate) = answer.gate { gate.notified().await; }
    Json(answer.page).into_response()
}

fn entry(name: &str, object_id: &str) -> Value {
    json!({
        "name": name, "kind": "file", "objectId": object_id,
        "versionId": "version-1", "size": 8, "etag": "\"etag\"",
        "mtime": "2026-10-05T12:00:00Z", "mode": "0644", "hasXattrs": false
    })
}

fn page(seq: u64, entries: Vec<Value>, next: Option<&str>) -> Value {
    json!({ "prefix": "", "seq": seq, "entries": entries, "nextContinuationToken": next })
}

fn folder_page(prefix: &str, seq: u64, entries: Vec<Value>) -> Value {
    json!({ "prefix": prefix, "seq": seq, "entries": entries, "nextContinuationToken": null })
}

fn folder(name: &str, object_id: &str) -> Value {
    json!({ "name": name, "kind": "folder", "objectId": object_id,
        "versionId": "version-1", "mtime": "2026-10-05T12:00:00Z", "mode": "0755" })
}

#[tokio::test]
async fn malformed_listing_keeps_the_last_complete_snapshot() {
    let mut bad_time = entry("bad", "object-bad");
    bad_time["mtime"] = json!("not a timestamp");
    let mut bad_mode = entry("bad", "object-bad");
    bad_mode["mode"] = json!("0999");
    let mut unknown_kind = entry("bad", "object-bad");
    unknown_kind["kind"] = json!("future-kind");
    let mut bad_folder = entry("folder", "object-bad");
    bad_folder["kind"] = json!("folder");
    let mut missing_size = entry("bad", "object-bad");
    missing_size.as_object_mut().unwrap().remove("size");
    let mut missing_version = entry("bad", "object-bad");
    missing_version.as_object_mut().unwrap().remove("versionId");
    let mut missing_etag = entry("bad", "object-bad");
    missing_etag.as_object_mut().unwrap().remove("etag");
    let mut missing_target = entry("link", "object-link");
    missing_target["kind"] = json!("symlink");
    let mut wrong_prefix = page(2, vec![entry("bad", "object-bad")], None);
    wrong_prefix["prefix"] = json!("different/");
    let bad_pages = vec![
        page(2, vec![bad_time], None),
        page(2, vec![bad_mode], None),
        page(2, vec![unknown_kind], None),
        page(2, vec![bad_folder], None),
        page(2, vec![missing_size], None),
        page(2, vec![missing_version], None),
        page(2, vec![missing_etag], None),
        page(2, vec![missing_target], None),
        page(2, vec![entry(&"x".repeat(256), "object-bad")], None),
        page(2, vec![entry("bad/", "object-bad")], None),
        page(2, vec![entry("a/b", "object-bad")], None),
        page(2, vec![entry("bad", "")], None),
        page(2, vec![entry("duplicate", "object-a"), entry("duplicate", "object-b")], None),
        page(2, vec![entry("a", "same-object"), entry("b", "same-object")], None),
        page(2, vec![folder("kept/", "object-kept")], None),
        wrong_prefix,
    ];
    for bad in bad_pages {
        let fixture = Fixture::new(vec![page(1, vec![entry("kept", "object-kept")], None).into(), bad.clone().into()]).await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let connectivity = Connectivity::default();
        let session = fixture.session(store, connectivity.clone()).await;
        let before = session.readdir(session.root(), None, 100).await.unwrap();
        session.invalidate(&[Invalidation::All]).await.unwrap();
        let result = session.readdir(session.root(), None, 100).await;
        assert!(matches!(result, Err(FsError::Io(_) | FsError::Unsupported)), "bad answer accepted: {bad}; {result:?}");
        for _ in 0..3 { connectivity.unanswered(); }
        let after = session.readdir(session.root(), None, 100).await.unwrap();
        assert_eq!(after.len(), 1, "failed refresh changed the snapshot: {bad}");
        assert_eq!(after[0].0, "kept");
        assert_eq!(after[0].1.ino, before[0].1.ino);
        assert_eq!(after[0].1.kind, before[0].1.kind);
        assert_eq!(after[0].1.size, before[0].1.size);
        assert_eq!(fixture.requests().len(), 2);
    }
}

#[tokio::test]
async fn changing_sequence_between_pages_restarts_the_whole_listing() {
    let fixture = Fixture::new(vec![
        page(1, vec![entry("old", "object-old")], Some("old")).into(),
        page(2, vec![entry("mixed", "object-mixed")], None).into(),
        page(2, vec![entry("fresh", "object-fresh")], None).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let session = fixture.session(Arc::new(Store::open(dir.path()).unwrap()), Connectivity::default()).await;
    let rows = session.readdir(session.root(), None, 100).await.unwrap();
    assert_eq!(rows.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["fresh"]);
    let requests = fixture.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1].get("continuation-token").map(String::as_str), Some("old"));
    assert!(!requests[2].contains_key("continuation-token"), "retry must restart at the first page");
}

#[tokio::test]
async fn consistent_pages_populate_one_complete_listing() {
    let fixture = Fixture::new(vec![
        page(4, vec![entry("a", "object-a")], Some("a")).into(),
        page(4, vec![entry("b", "object-b")], None).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let session = fixture.session(Arc::new(Store::open(dir.path()).unwrap()), Connectivity::default()).await;
    let rows = session.readdir(session.root(), None, 100).await.unwrap();
    assert_eq!(rows.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_ne!(rows[0].1.ino, rows[1].1.ino);
    assert_eq!(session.lookup(session.root(), "b").await.unwrap().ino, rows[1].1.ino);
    assert_eq!(fixture.requests().len(), 2, "later pages must be part of the cached complete snapshot");
}

#[tokio::test]
async fn continuous_page_churn_returns_again_after_bounded_retries() {
    let mut answers = Vec::new();
    for seq in 1..=4 {
        answers.push(page(seq, vec![entry("a", "object-a")], Some("a")).into());
        answers.push(page(seq + 1, vec![entry("b", "object-b")], None).into());
    }
    let fixture = Fixture::new(answers).await;
    let dir = tempfile::tempdir().unwrap();
    let session = fixture.session(Arc::new(Store::open(dir.path()).unwrap()), Connectivity::default()).await;
    assert_eq!(session.readdir(session.root(), None, 100).await.unwrap_err(), FsError::Again);
    assert_eq!(fixture.requests().len(), 8);
}

#[tokio::test]
async fn repeated_continuation_token_fails_without_unbounded_requests() {
    let fixture = Fixture::new(vec![
        page(1, vec![entry("a", "object-a")], Some("a")).into(),
        page(1, vec![entry("b", "object-b")], Some("a")).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let session = fixture.session(Arc::new(Store::open(dir.path()).unwrap()), Connectivity::default()).await;
    assert!(matches!(session.readdir(session.root(), None, 100).await, Err(FsError::Io(_))));
    assert_eq!(fixture.requests().len(), 2, "the repeated token must be rejected before requesting it again");
}

#[tokio::test]
async fn invalidation_during_a_fetch_rejects_the_old_response() {
    let gate = Arc::new(Notify::new());
    let fixture = Fixture::new(vec![
        Answer { page: page(1, vec![entry("old", "object-old")], None), gate: Some(gate.clone()) },
        page(2, vec![entry("fresh", "object-fresh")], None).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let session = Arc::new(fixture.session(Arc::new(Store::open(dir.path()).unwrap()), Connectivity::default()).await);
    let listing = tokio::spawn({
        let session = session.clone();
        async move { session.readdir(session.root(), None, 100).await }
    });
    fixture.wait_for_requests(1).await;
    session.invalidate(&[Invalidation::All]).await.unwrap();
    gate.notify_one();
    let rows = listing.await.unwrap().unwrap();
    assert_eq!(rows.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["fresh"]);
    assert_eq!(fixture.requests().len(), 2, "invalidated metadata must be fetched again");
}

#[tokio::test]
async fn two_sessions_cannot_replace_a_newer_listing_with_an_old_response() {
    let gate = Arc::new(Notify::new());
    let fixture = Fixture::new(vec![
        Answer { page: page(1, vec![entry("old", "object-old")], None), gate: Some(gate.clone()) },
        page(2, vec![entry("fresh", "object-fresh")], None).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let first = Arc::new(fixture.session(store.clone(), Connectivity::default()).await);
    let second = fixture.session(store, Connectivity::default()).await;
    let pending = tokio::spawn({
        let first = first.clone();
        async move { first.readdir(first.root(), None, 100).await }
    });
    fixture.wait_for_requests(1).await;
    let current = second.readdir(second.root(), None, 100).await.unwrap();
    assert_eq!(current[0].0, "fresh");
    gate.notify_one();
    let late = pending.await.unwrap().unwrap();
    assert_eq!(late.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["fresh"]);
    assert_eq!(late[0].1.ino, current[0].1.ino);
    assert_eq!(fixture.requests().len(), 2, "the later accepted snapshot should satisfy the rejected fetch");
}

#[tokio::test]
async fn a_late_source_listing_cannot_move_an_object_back_from_its_destination() {
    let gate = Arc::new(Notify::new());
    let fixture = Fixture::new(vec![
        page(1, vec![folder("src/", "object-src"), folder("dest/", "object-dest")], None).into(),
        Answer { page: folder_page("src/", 1, vec![entry("item", "object-moved")]), gate: Some(gate.clone()) },
        folder_page("dest/", 2, vec![entry("item", "object-moved")]).into(),
        folder_page("src/", 2, Vec::new()).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let first = Arc::new(fixture.session(store.clone(), Connectivity::default()).await);
    let second = fixture.session(store, Connectivity::default()).await;
    first.readdir(first.root(), None, 100).await.unwrap();
    let src = first.lookup(first.root(), "src").await.unwrap().ino;
    let dest = first.lookup(first.root(), "dest").await.unwrap().ino;
    let pending = tokio::spawn({
        let first = first.clone();
        async move { first.readdir(src, None, 100).await }
    });
    fixture.wait_for_requests(2).await;
    let moved = second.readdir(dest, None, 100).await.unwrap();
    assert_eq!(moved[0].0, "item");
    gate.notify_one();
    assert!(pending.await.unwrap().unwrap().is_empty(), "an old source snapshot must be refetched");
    assert_eq!(first.lookup(src, "item").await.unwrap_err(), FsError::NotFound);
    assert_eq!(second.lookup(dest, "item").await.unwrap().ino, moved[0].1.ino);
    let requests = fixture.requests();
    assert_eq!(requests.iter().map(|q| q.get("prefix").map(String::as_str).unwrap()).collect::<Vec<_>>(), ["", "src/", "dest/", "src/"]);
}

#[tokio::test]
async fn a_listing_cannot_relink_an_ancestor_as_its_own_descendant() {
    let fixture = Fixture::new(vec![
        page(1, vec![folder("a/", "object-a")], None).into(),
        folder_page("a/", 1, vec![folder("b/", "object-b")]).into(),
        folder_page("a/b/", 1, vec![folder("loop/", "object-a")]).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let session = fixture.session(Arc::new(Store::open(dir.path()).unwrap()), Connectivity::default()).await;
    let a = session.lookup(session.root(), "a").await.unwrap().ino;
    let b = session.lookup(a, "b").await.unwrap().ino;
    assert!(matches!(session.readdir(b, None, 100).await, Err(FsError::Io(_))), "cyclic entries must be rejected atomically");
    assert_eq!(session.lookup(session.root(), "a").await.unwrap().ino, a);
    assert_eq!(session.lookup(a, "b").await.unwrap().ino, b);
    assert_eq!(fixture.requests().len(), 3);
}

#[tokio::test]
async fn different_directory_refreshes_overlap_and_one_directory_shares_its_fetch() {
    let gate = Arc::new(Notify::new());
    let fixture = Fixture::new(vec![
        page(1, vec![folder("a/", "object-a"), folder("b/", "object-b")], None).into(),
        Answer { page: folder_page("a/", 1, vec![entry("one", "object-one")]), gate: Some(gate.clone()) },
        folder_page("b/", 1, vec![entry("two", "object-two")]).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let session = Arc::new(fixture.session(Arc::new(Store::open(dir.path()).unwrap()), Connectivity::default()).await);
    let rows = session.readdir(session.root(), None, 100).await.unwrap();
    let a = rows[0].1.ino;
    let b = rows[1].1.ino;
    let first = tokio::spawn({ let session = session.clone(); async move { session.readdir(a, None, 100).await } });
    fixture.wait_for_requests(2).await;
    let same = tokio::spawn({ let session = session.clone(); async move { session.readdir(a, None, 100).await } });
    let different = tokio::spawn({ let session = session.clone(); async move { session.readdir(b, None, 100).await } });
    let other = tokio::time::timeout(Duration::from_secs(2), different).await.expect("b must finish while a's listing is gated").unwrap().unwrap();
    assert_eq!(other[0].0, "two");
    gate.notify_one();
    let first = first.await.unwrap().unwrap();
    assert_eq!(same.await.unwrap().unwrap(), first);
    assert_eq!(first[0].0, "one");
    assert_eq!(fixture.requests().len(), 3, "two callers of a must share one listing");
}

#[tokio::test]
async fn subtree_invalidation_visits_only_effective_descendants_and_their_ancestors() {
    let fixture = Fixture::new(vec![
        page(1, vec![folder("a/", "object-a"), folder("b/", "object-b")], None).into(),
        folder_page("a/", 1, vec![folder("sub/", "object-sub"), entry("file", "object-file")]).into(),
        folder_page("a/sub/", 1, vec![entry("leaf", "object-leaf")]).into(),
        folder_page("b/", 1, vec![entry("other", "object-other")]).into(),
        page(2, vec![folder("a/", "object-a"), folder("b/", "object-b")], None).into(),
    ]).await;
    let dir = tempfile::tempdir().unwrap();
    let session = fixture.session(Arc::new(Store::open(dir.path()).unwrap()), Connectivity::default()).await;
    let a = session.lookup(session.root(), "a").await.unwrap().ino;
    let b = session.lookup(session.root(), "b").await.unwrap().ino;
    let sub = session.lookup(a, "sub").await.unwrap().ino;
    let file = session.lookup(a, "file").await.unwrap().ino;
    let leaf = session.lookup(sub, "leaf").await.unwrap().ino;
    let other = session.lookup(b, "other").await.unwrap();
    let affected = session.invalidate(&[Invalidation::Subtree("a/".into())]).await.unwrap();
    let mut expected = vec![session.root(), a, sub, file, leaf];
    expected.sort_unstable();
    assert_eq!(affected, expected);
    assert_eq!(session.lookup(b, "other").await.unwrap(), other);
    assert_eq!(fixture.requests().len(), 5, "b's complete listing stays fresh");
    assert_eq!(session.invalidate(&[Invalidation::Object("b/new".into())]).await.unwrap(), vec![session.root(), b]);
}

#[tokio::test]
#[ignore = "reproducible PR measurement: cargo test -p voidfs-client --test mount_listing directory_listing_measurement_10k_and_100k -- --ignored --nocapture"]
async fn directory_listing_measurement_10k_and_100k() {
    fn median(mut samples: Vec<Duration>) -> f64 { samples.sort_unstable(); samples[samples.len() / 2].as_secs_f64() * 1000.0 }
    for count in [10_000, 100_000] {
        let mut entries = vec![folder("a/", "object-a"), folder("b/", "object-b")];
        entries.extend((0..count).map(|i| entry(&format!("file-{i:06}"), &format!("object-{i:06}"))));
        let mut answers = vec![page(1, entries, None).into()];
        for _ in 0..6 { answers.extend([folder_page("a/", 1, Vec::new()).into(), folder_page("b/", 1, Vec::new()).into()]); }
        let fixture = Fixture::new(answers).await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let session = fixture.session(store.clone(), Connectivity::default()).await;
        let rows = session.readdir(session.root(), None, 2).await.unwrap();
        let (a, b) = (rows[0].1.ino, rows[1].1.ino);
        *fixture.state.delay.lock().unwrap() = Duration::from_millis(100);
        *fixture.state.route_by_prefix.lock().unwrap() = true;
        let c = rusqlite::Connection::open(store.dir().join("state.sqlite")).unwrap();
        let mut before = Vec::new(); let mut after = Vec::new();
        for _ in 0..3 {
            c.execute("UPDATE mount_dirs SET listed=0 WHERE ino IN (?1, ?2)", rusqlite::params![a, b]).unwrap();
            let start = std::time::Instant::now();
            // The previous single session mutex let these same calls run only one at a time.
            session.readdir(a, None, 1).await.unwrap();
            session.readdir(b, None, 1).await.unwrap();
            before.push(start.elapsed());
            c.execute("UPDATE mount_dirs SET listed=0 WHERE ino IN (?1, ?2)", rusqlite::params![a, b]).unwrap();
            let start = std::time::Instant::now();
            // Route the synthetic responses by the request prefix, regardless of task ordering.
            let results = tokio::join!(session.readdir(a, None, 1), session.readdir(b, None, 1));
            results.0.unwrap(); results.1.unwrap();
            after.push(start.elapsed());
        }
        println!("entries={count} two_listings_with_100ms_server_delay_ms_before={:.3} after={:.3}", median(before), median(after));
    }
}
