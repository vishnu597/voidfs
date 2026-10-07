// SPDX-License-Identifier: Apache-2.0
//! The change-feed client (step 4, item 3): a drive's changes, as invalidations of what a mount
//! caches about it (listings and attributes), from the position of the listing it holds.
//!
//! The SDK's [`voidfs_sdk::ChangeWatch`] reconnects a broken stream from the last position it
//! delivered. On `410 ChangesExpired` the watcher relists (the root's first page gives a current
//! position), says everything of the drive is stale, and watches again from there. Its stream
//! connecting and breaking count for [`Connectivity`].

use std::time::Duration;

use tokio::sync::mpsc;
use voidfs_sdk::{Change, ChangeWatchEvent, Client, Kind};

use crate::connectivity::Connectivity;

/// What a change makes stale.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Invalidation {
    /// The object at `key`: its attributes and content, and its entry in its folder's listing.
    Object(String),
    /// Everything at or under the folder `prefix` (ending in `/`): a folder renamed, deleted or
    /// restored.
    Subtree(String),
    /// Everything of the drive: its changes expired and it was relisted.
    All,
}

/// One position's changes, or a relisting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedEvent {
    pub drive: String,
    /// The position this brings the watcher to.
    pub seq: u64,
    pub invalidations: Vec<Invalidation>,
}

fn folder_key(key: &str) -> String {
    if key.ends_with('/') { key.to_owned() } else { format!("{key}/") }
}

/// What a batch's changes make stale.
pub fn invalidations(changes: &[Change]) -> Vec<Invalidation> {
    let mut out = Vec::new();
    for c in changes {
        let folder = c.kind == Kind::Folder || c.key.ends_with('/');
        let mut both = |key: &str| {
            out.push(Invalidation::Object(key.to_owned()));
            if folder {
                out.push(Invalidation::Subtree(folder_key(key)));
            }
        };
        // A change that moved an object carries where it was: a rename, or a folder restore that
        // moves one back (RFC 0004).
        if let Some(from) = &c.from_key {
            both(from);
        }
        match c.op.as_str() {
            // A folder rename, delete or restore is one change for the folder, not one for each
            // object in it (protocol §5.6).
            "rename" | "delete" | "restore" => both(&c.key),
            _ => out.push(Invalidation::Object(c.key.clone())),
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A drive watched in the background. Dropped, it stops.
pub struct FeedWatch {
    rx: mpsc::Receiver<FeedEvent>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FeedWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FeedWatch {
    /// Watches `drive` from position `since` (a listing's `seq`). `conn`, if given, learns from
    /// the stream whether the server is there.
    pub fn start(client: Client, drive: &str, since: u64, conn: Option<Connectivity>) -> FeedWatch {
        Self::start_mode(client, drive, since, conn, false)
    }

    /// Watches with a full invalidation on every reopened stream, including idle reconnects.
    /// Such events retain the last delivered sequence position.
    pub fn start_with_resync(client: Client, drive: &str, since: u64, conn: Option<Connectivity>) -> FeedWatch {
        Self::start_mode(client, drive, since, conn, true)
    }

    fn start_mode(client: Client, drive: &str, since: u64, conn: Option<Connectivity>, resync: bool) -> FeedWatch {
        let (tx, rx) = mpsc::channel(256);
        let drive = drive.to_owned();
        let task = tokio::spawn(run(client, drive, since, conn, tx, resync));
        FeedWatch { rx, task }
    }

    /// The next event, waiting for one; `None` once the watch has stopped.
    pub async fn next(&mut self) -> Option<FeedEvent> {
        self.rx.recv().await
    }
}

async fn run(client: Client, drive: String, since: u64, conn: Option<Connectivity>, tx: mpsc::Sender<FeedEvent>, resync: bool) {
    let mut watch = client.watch_changes(&drive, since);
    let mut failures: u32 = 0;
    loop {
        let event = match watch.next_event().await {
            Ok(ChangeWatchEvent::Connected { since, reconnect }) => {
                failures = 0;
                if let Some(c) = &conn { c.answered(false); }
                if !reconnect || !resync { continue; }
                FeedEvent { drive: drive.clone(), seq: since, invalidations: vec![Invalidation::All] }
            },
            Ok(ChangeWatchEvent::Changes(batch)) => {
                failures = 0;
                if let Some(c) = &conn {
                    c.answered(false);
                }
                FeedEvent { drive: drive.clone(), seq: batch.seq, invalidations: invalidations(&batch.changes) }
            }
            Err(e) if e.status() == Some(410) || e.code() == Some("ChangesExpired") => match client.list_folder_page(&drive, "", None).await {
                Ok(page) => {
                    watch = client.watch_changes(&drive, page.seq);
                    FeedEvent { drive: drive.clone(), seq: page.seq, invalidations: vec![Invalidation::All] }
                }
                Err(_) => {
                    failures += 1;
                    tokio::time::sleep(backoff(failures)).await;
                    continue;
                }
            },
            Err(e) => {
                // The SDK already tried again; wait before the next attempt, longer each time.
                if let Some(c) = &conn
                    && e.status().is_none()
                {
                    c.unanswered();
                }
                failures += 1;
                tokio::time::sleep(backoff(failures)).await;
                continue;
            }
        };
        if tx.send(event).await.is_err() {
            return;
        }
    }
}

fn backoff(n: u32) -> Duration {
    Duration::from_millis(250u64 << n.min(7)).min(Duration::from_secs(30))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(op: &str, key: &str, from: Option<&str>, kind: Kind) -> Change {
        Change { op: op.into(), key: key.into(), from_key: from.map(Into::into), object_id: "o".into(), version_id: "1.0".into(), kind, seq: None, time: None }
    }

    #[test]
    fn changes_name_what_they_make_stale() {
        use Invalidation::*;
        let got = invalidations(&[
            change("create", "a/", None, Kind::Folder),
            change("put", "a/x", None, Kind::File),
            change("rename", "b/y", Some("a/x"), Kind::File),
            change("rename", "c/", Some("a/"), Kind::Folder),
            change("delete", "z", None, Kind::File),
            change("restore", "d/", None, Kind::Folder),
            change("restore", "d/m", Some("q/m"), Kind::File),
            change("write", "a/x", None, Kind::File),
        ]);
        assert_eq!(got, [
            Object("a/".into()),
            Object("a/x".into()),
            Object("b/y".into()),
            Object("c/".into()),
            Object("d/".into()),
            Object("d/m".into()),
            Object("q/m".into()),
            Object("z".into()),
            Subtree("a/".into()),
            Subtree("c/".into()),
            Subtree("d/".into()),
        ]);
    }
}
