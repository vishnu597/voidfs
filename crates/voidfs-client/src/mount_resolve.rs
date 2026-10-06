// SPDX-License-Identifier: Apache-2.0
//! Finds the current or last deleted name of a snapshot whose original name has moved.

use std::collections::{HashSet, VecDeque};

use voidfs_sdk::{Client, FolderPage, Kind};

use crate::{Connectivity, Error};
use crate::mount::{FsError, Result};

/// Shared by every fallback in one read, including retries after namespace churn.
pub(crate) struct Budget { remaining: usize }

impl Default for Budget {
    fn default() -> Self { Self { remaining: 16 } }
}

impl Budget {
    fn take(&mut self) -> Result<()> {
        if self.remaining == 0 { return Err(FsError::Again); }
        self.remaining -= 1;
        Ok(())
    }
}

async fn page(client: &Client, drive: &str, prefix: &str, token: Option<&str>, conn: &Connectivity, budget: &mut Budget) -> Result<FolderPage> {
    conn.check()?;
    budget.take()?;
    let page = client.list_folder_page(drive, prefix, token).await.map_err(Error::from)?;
    if page.prefix != prefix { return Err(FsError::Io("snapshot lookup returned the wrong prefix".into())); }
    Ok(page)
}

/// Uses existing attribute/deleted listings only. The final root check prevents an incomplete
/// traversal across a rename from being mistaken for absence; GET still validates the snapshot.
pub(crate) async fn locate(client: &Client, drive: &str, object_id: &str, conn: &Connectivity, budget: &mut Budget) -> Result<Option<String>> {
    for _ in 0..4 {
        let first = page(client, drive, "", None, conn, budget).await?;
        let seq = first.seq;
        let mut next = None;
        let mut tokens = HashSet::new();
        let deleted_key = loop {
            conn.check()?;
            budget.take()?;
            let deleted = client.list_deleted_page(drive, "", next.as_deref()).await.map_err(Error::from)?;
            if let Some(entry) = deleted.deleted.into_iter().find(|entry| entry.object_id == object_id) { break Some(entry.key); }
            let Some(token) = deleted.next_continuation_token else { break None; };
            if !tokens.insert(token.clone()) { return Err(FsError::Io("snapshot lookup repeated its deleted continuation token".into())); }
            next = Some(token);
        };
        if let Some(key) = deleted_key {
            if page(client, drive, "", None, conn, budget).await?.seq == seq { return Ok(Some(key)); }
            continue;
        }
        let mut dirs = VecDeque::from([(String::new(), Some(first))]);
        let mut ids = HashSet::new();
        let mut changed = false;
        let mut found = None;
        'scan: while let Some((prefix, first)) = dirs.pop_front() {
            let mut next = None;
            let mut first = first;
            let mut tokens = HashSet::new();
            let mut names = HashSet::new();
            loop {
                let answer = match first.take() {
                    Some(answer) => answer,
                    None => match page(client, drive, &prefix, next.as_deref(), conn, budget).await {
                        Err(FsError::NotFound) => { changed = true; break 'scan; }
                        answer => answer?,
                    },
                };
                if answer.seq != seq { changed = true; break 'scan; }
                for entry in answer.entries {
                    if entry.kind == Kind::Unknown { return Err(FsError::Unsupported); }
                    let name = entry.name.trim_end_matches('/');
                    if name.is_empty() || name.len() > 255 || matches!(name, "." | "..") || name.contains(['/', '\0'])
                        || entry.name.ends_with('/') != (entry.kind == Kind::Folder) || entry.name.ends_with("//")
                        || entry.object_id.is_empty() || !names.insert(name.to_owned()) || !ids.insert(entry.object_id.clone())
                    { return Err(FsError::Io("snapshot lookup returned an invalid or duplicate entry".into())); }
                    let key = format!("{prefix}{}", entry.name);
                    if entry.object_id == object_id { found = Some(key); break 'scan; }
                    if entry.kind == Kind::Folder { dirs.push_back((key, None)); }
                }
                let Some(token) = answer.next_continuation_token else { break; };
                if !tokens.insert(token.clone()) { return Err(FsError::Io("snapshot lookup repeated its continuation token".into())); }
                next = Some(token);
            }
        }
        if !changed && page(client, drive, "", None, conn, budget).await?.seq == seq { return Ok(found); }
    }
    Err(FsError::Again)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use axum::Json;
    use axum::extract::{Query, State};
    use serde_json::{Value, json};
    use voidfs_sdk::Config;

    struct Fixture { client: Client, requests: Arc<Mutex<Vec<HashMap<String, String>>>>, task: tokio::task::JoinHandle<()> }

    impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

    impl Fixture {
        async fn new(answers: Vec<Value>) -> Self {
            Self::observed(answers, None).await
        }

        async fn observed(answers: Vec<Value>, observer: Option<Arc<dyn voidfs_sdk::Observe>>) -> Self {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let state = (Arc::new(Mutex::new(VecDeque::from(answers))), requests.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let app = axum::Router::new().fallback(answer).with_state(state);
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
            let client = Client::new(Config { endpoint, access_key_id: "fixture".into(), secret_access_key: "fixture".into(), max_attempts: 1, observer, ..Default::default() }).unwrap();
            Self { client, requests, task }
        }
    }

    type FixtureState = (Arc<Mutex<VecDeque<Value>>>, Arc<Mutex<Vec<HashMap<String, String>>>>);

    async fn answer(State((answers, requests)): State<FixtureState>, Query(query): Query<HashMap<String, String>>) -> Json<Value> {
        requests.lock().unwrap().push(query);
        Json(answers.lock().unwrap().pop_front().expect("unexpected request"))
    }

    fn listing(prefix: &str, seq: u64, entries: Vec<Value>, next: Option<&str>) -> Value {
        json!({ "prefix": prefix, "seq": seq, "entries": entries, "nextContinuationToken": next })
    }

    fn entry(name: &str, id: &str, kind: &str) -> Value { json!({ "name": name, "objectId": id, "kind": kind }) }

    fn deleted(entries: Vec<Value>) -> Value { json!({ "deleted": entries, "nextContinuationToken": null }) }

    fn deleted_page(next: &str) -> Value { json!({ "deleted": [], "nextContinuationToken": next }) }

    #[tokio::test]
    async fn finds_the_identity_in_an_unseen_folder_on_a_later_page() {
        let f = Fixture::new(vec![listing("", 1, vec![entry("new/", "folder", "folder")], None), deleted(vec![]),
            listing("new/", 1, vec![entry("other", "other", "file")], Some("other")),
            listing("new/", 1, vec![entry("moved", "wanted", "file")], None), listing("", 1, vec![], None)]).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await.unwrap().as_deref(), Some("new/moved"));
        let requests = f.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        assert_eq!(requests[3].get("continuation-token").map(String::as_str), Some("other"));
    }

    #[tokio::test]
    async fn finds_the_final_deleted_name_by_identity() {
        let f = Fixture::new(vec![listing("", 3, vec![], None), deleted(vec![
            json!({ "key": "wrong", "objectId": "other", "deletedAt": "time", "lastVersionId": "1.0", "size": 0 }),
            json!({ "key": "new/gone", "objectId": "wanted", "deletedAt": "time", "lastVersionId": "1.0", "size": 0 })]), listing("", 3, vec![], None)]).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await.unwrap().as_deref(), Some("new/gone"));
        assert_eq!(f.requests.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_deleted_identity_on_a_later_page_keeps_its_token_and_validates_the_root() {
        let f = Fixture::new(vec![listing("", 3, vec![], None), deleted_page("later+/="), deleted(vec![
            json!({ "key": "new/gone", "objectId": "wanted", "deletedAt": "time", "lastVersionId": "1.0", "size": 0 })]), listing("", 3, vec![], None)]).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await.unwrap().as_deref(), Some("new/gone"));
        let requests = f.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[2].get("continuation-token").map(String::as_str), Some("later+/="));
        assert!(requests[3].contains_key("x-voidfs-list"));
    }

    #[tokio::test]
    async fn connectivity_is_checked_before_each_deleted_page() {
        struct Disconnect { conn: Connectivity, calls: std::sync::atomic::AtomicUsize }
        impl voidfs_sdk::Observe for Disconnect {
            fn observe(&self, _: &voidfs_sdk::Observation) {
                if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 { for _ in 0..3 { self.conn.unanswered(); } }
            }
        }
        let conn = Connectivity::default();
        let observer = Arc::new(Disconnect { conn: conn.clone(), calls: Default::default() });
        let f = Fixture::observed(vec![listing("", 1, vec![], None), deleted_page("later"), deleted(vec![])], Some(observer)).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &conn, &mut Budget::default()).await.unwrap_err(), FsError::Offline);
        assert_eq!(f.requests.lock().unwrap().len(), 2, "the next deleted page must not be requested offline");
    }

    #[tokio::test]
    async fn retries_inconsistent_pages_and_the_final_root_generation() {
        let f = Fixture::new(vec![listing("", 1, vec![], Some("next")), deleted(vec![]), listing("", 2, vec![], None),
            listing("", 2, vec![], None), deleted(vec![]), listing("", 3, vec![], None),
            listing("", 3, vec![entry("moved", "wanted", "file")], None), deleted(vec![]), listing("", 3, vec![], None)]).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await.unwrap().as_deref(), Some("moved"));
        assert_eq!(f.requests.lock().unwrap().len(), 9);
    }

    #[tokio::test]
    async fn malformed_prefix_tokens_and_entries_are_rejected() {
        for answers in [vec![listing("wrong/", 1, vec![], None)],
            vec![listing("", 1, vec![], Some("repeat")), deleted(vec![]), listing("", 1, vec![], Some("repeat"))],
            vec![listing("", 1, vec![entry("bad/path", "other", "file")], None), deleted(vec![])],
            vec![listing("", 1, vec![entry("unknown", "other", "unknown")], None), deleted(vec![])]] {
            let f = Fixture::new(answers).await;
            assert!(matches!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await, Err(FsError::Io(_) | FsError::Unsupported)));
        }
    }

    #[tokio::test]
    async fn only_a_complete_stable_scan_reports_absence_and_offline_sends_no_request() {
        let f = Fixture::new(vec![listing("", 1, vec![], None), deleted(vec![]), listing("", 1, vec![], None)]).await;
        let conn = Connectivity::default();
        assert_eq!(locate(&f.client, "drv", "wanted", &conn, &mut Budget::default()).await.unwrap(), None);
        for _ in 0..3 { conn.unanswered(); }
        assert_eq!(locate(&f.client, "drv", "wanted", &conn, &mut Budget::default()).await.unwrap_err(), FsError::Offline);
        assert_eq!(f.requests.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_changing_drive_stops_after_four_attempts() {
        let f = Fixture::new((0..4).flat_map(|i| [listing("", i, vec![], None), deleted(vec![]), listing("", i + 1, vec![], None)]).collect()).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await.unwrap_err(), FsError::Again);
        assert_eq!(f.requests.lock().unwrap().len(), 12);
    }

    #[tokio::test]
    async fn folder_traversal_stops_at_the_shared_page_budget() {
        let folders = (0..15).map(|i| entry(&format!("folder-{i}/"), &format!("id-{i}"), "folder")).collect();
        let mut answers = vec![listing("", 1, folders, None), deleted(vec![])];
        answers.extend((0..15).map(|i| listing(&format!("folder-{i}/"), 1, vec![], None)));
        let f = Fixture::new(answers).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await.unwrap_err(), FsError::Again);
        let requests = f.requests.lock().unwrap();
        assert_eq!(requests.len(), 16);
        assert_eq!(requests.last().unwrap().get("prefix").map(String::as_str), Some("folder-13/"));
    }

    #[tokio::test]
    async fn deleted_pages_stop_at_the_budget_without_draining_the_listing() {
        let mut answers = vec![listing("", 1, vec![], None)];
        answers.extend((0..16).map(|i| deleted_page(&format!("page-{i}"))));
        let f = Fixture::new(answers).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await.unwrap_err(), FsError::Again);
        let requests = f.requests.lock().unwrap();
        assert_eq!(requests.len(), 16);
        assert_eq!(requests[1].get("continuation-token"), None);
        assert_eq!(requests.last().unwrap().get("continuation-token").map(String::as_str), Some("page-13"));
        assert!(requests[1..].iter().all(|query| query.contains_key("x-voidfs-deleted")));
    }

    #[tokio::test]
    async fn churn_does_not_replenish_the_page_budget() {
        let answers = (0..4).flat_map(|i| [listing("", i, vec![], Some("later")), deleted_page("first"), deleted_page("second"),
            deleted(vec![]), listing("", i + 1, vec![], None)]).collect();
        let f = Fixture::new(answers).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await.unwrap_err(), FsError::Again);
        assert_eq!(f.requests.lock().unwrap().len(), 16);
    }

    #[tokio::test]
    async fn final_validation_and_later_relocations_share_the_budget() {
        let answers = (0..6).flat_map(|_| [listing("", 1, vec![entry("moved", "wanted", "file")], None), deleted(vec![]), listing("", 1, vec![], None)]).collect();
        let f = Fixture::new(answers).await;
        let conn = Connectivity::default();
        let mut budget = Budget::default();
        for _ in 0..5 { assert_eq!(locate(&f.client, "drv", "wanted", &conn, &mut budget).await.unwrap().as_deref(), Some("moved")); }
        assert_eq!(locate(&f.client, "drv", "wanted", &conn, &mut budget).await.unwrap_err(), FsError::Again);
        assert_eq!(f.requests.lock().unwrap().len(), 16);
        let f = Fixture::new(vec![listing("", 1, vec![entry("moved", "wanted", "file")], None), deleted(vec![])]).await;
        assert_eq!(locate(&f.client, "drv", "wanted", &conn, &mut Budget { remaining: 2 }).await.unwrap_err(), FsError::Again);
        assert_eq!(f.requests.lock().unwrap().len(), 2, "a found key is not accepted without final root validation");
    }

    #[tokio::test]
    async fn repeated_deleted_continuation_tokens_are_rejected() {
        let f = Fixture::new(vec![listing("", 1, vec![], None), deleted_page("repeat"), deleted_page("repeat")]).await;
        assert!(matches!(locate(&f.client, "drv", "wanted", &Connectivity::default(), &mut Budget::default()).await, Err(FsError::Io(message)) if message.contains("deleted continuation token")));
        assert_eq!(f.requests.lock().unwrap().len(), 3);
    }
}
