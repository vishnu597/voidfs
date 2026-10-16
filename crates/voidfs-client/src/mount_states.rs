// SPDX-License-Identifier: Apache-2.0
//! The sync states of one folder's entries, for status badges (step 5, item 8). They come from
//! this Mac's namespace alone: no listing, attribute request or other call reaches the server,
//! so a badge costs nothing remote however many files a folder has.

use super::*;

/// Entries in one page at most.
pub const MAX_STATES: usize = 1024;

/// A folder's entries that are not [`Sync::Saved`], from [`Session::folder_states`]. Every other
/// name in the folder is saved: whatever isn't is a change made on this Mac, and those are in the
/// local view whether or not the folder is listed now. A folder's state, its own and its
/// entries', counts everything below it: the most pressing of its unsaved descendants
/// ([`Sync::Conflict`], then [`Sync::Error`], [`Sync::Saving`] and [`Sync::Pending`]).
/// [`Sync::Local`] files count for nothing above them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderStates {
    pub state: Sync,
    /// In UTF-8 byte order, after the request's cursor.
    pub entries: Vec<(String, Sync)>,
    /// The last name of a page that stopped short, for the next page's cursor.
    pub next: Option<String>,
    /// The folder's listing generation, which a new listing or an invalidation advances.
    pub generation: u64,
    /// The request's `unless` generation still holds: nothing else was read.
    pub unchanged: bool,
}

/// The children of a folder whose own state is not saved, by name.
const UNSAVED_CHILDREN: &str = "SELECT x.name, i.sync FROM (
        SELECT name, ino FROM mount_overlay WHERE parent=?1 AND ino IS NOT NULL
        UNION ALL SELECT name, ino FROM mount_names n WHERE parent=?1 AND NOT EXISTS
        (SELECT 1 FROM mount_overlay o WHERE o.parent=n.parent AND o.name=n.name)) x
    JOIN mount_inodes i ON i.ino=x.ino WHERE i.sync<>'saved'";

/// Every inode of the drive with unpublished changes, from the index that holds only those.
const UNSAVED: &str = "SELECT ino, sync FROM mount_inodes INDEXED BY mount_pending_remote_key
    WHERE drive=?1 AND sync<>'saved' AND sync<>'local'";

const PARENT: &str = "SELECT parent, name FROM mount_overlay WHERE ino=?1 UNION ALL
    SELECT parent, name FROM mount_names n WHERE ino=?1 AND NOT EXISTS
    (SELECT 1 FROM mount_overlay o WHERE o.parent=n.parent AND o.name=n.name) LIMIT 1";

/// How far up a walk goes before it gives up on a chain as broken.
const MAX_DEPTH: usize = 4096;

/// How pressing a state is, for a folder's.
fn rank(sync: Sync) -> u8 {
    match sync { Sync::Saved | Sync::Local => 0, Sync::Pending => 1, Sync::Saving => 2, Sync::Error => 3, Sync::Conflict => 4 }
}

fn worse(a: Sync, b: Sync) -> Sync { if rank(b) > rank(a) { b } else { a } }

fn parse(sync: &str) -> rusqlite::Result<Sync> {
    Ok(match sync {
        "saved" => Sync::Saved, "pending" => Sync::Pending, "saving" => Sync::Saving, "conflict" => Sync::Conflict, "error" => Sync::Error,
        "local" => Sync::Local,
        _ => return Err(rusqlite::Error::InvalidQuery),
    })
}

/// A child by its exact name, or by its only equivalent spelling, in the local view.
fn child(c: &Connection, parent: Ino, name: &str) -> rusqlite::Result<Option<Ino>> {
    if let Some(ino) = named_child(c, parent, name)? { return Ok(Some(ino)); }
    let candidates = equivalent_names(c, parent, &name.nfc().collect::<String>())?;
    Ok(if candidates.len() == 1 { candidates.first().copied() } else { None })
}

impl Session {
    /// The states of `folder`'s entries, `folder` being its path below the drive's top (`""`
    /// for the top itself, `/` between names). It reads this Mac's state only: a folder not
    /// listed now is not listed for it. `after` is an exclusive name cursor; a page holds `limit` entries at most
    /// ([`MAX_STATES`]) and stops before its names exceed `max_bytes`. With `unless`, a folder
    /// whose listing generation is still that answers [`FolderStates::unchanged`] at once; the
    /// caller compares what else may have changed (the drive's own generation).
    pub async fn folder_states(&self, folder: &str, after: Option<&str>, limit: usize, max_bytes: usize, unless: Option<u64>) -> Result<FolderStates> {
        if folder.len() > voidfs_core::names::MAX_KEY_BYTES || after.is_some_and(|a| a.len() > voidfs_core::names::MAX_NAME_BYTES) {
            return Err(FsError::InvalidArgument);
        }
        let names = if folder.is_empty() { Vec::new() } else { folder.split('/').map(str::to_owned).collect::<Vec<_>>() };
        if names.iter().any(|name| !valid_name(name)) { return Err(FsError::InvalidName); }
        let (after, limit) = (after.map(str::to_owned), limit.clamp(1, MAX_STATES));
        self.db(move |c, d, root| {
            // One read transaction: the names, the states and the walk see one snapshot.
            let tx = c.transaction()?;
            let found = (|| -> rusqlite::Result<Result<FolderStates>> {
                let mut at = root;
                for name in &names {
                    match child(&tx, at, name)? {
                        Some(ino) => at = ino,
                        None => return Ok(Err(FsError::NotFound)),
                    }
                }
                let Some(n) = node(&tx, d, at)? else { return Ok(Err(FsError::Stale)); };
                if n.entry.kind != Kind::Folder { return Ok(Err(FsError::NotDir)); }
                let generation: u64 = tx.query_row("SELECT generation FROM mount_dirs WHERE ino=?1", [at], |r| r.get(0)).optional()?.unwrap_or(0);
                if unless == Some(generation) {
                    return Ok(Ok(FolderStates { state: Sync::Saved, entries: Vec::new(), next: None, generation, unchanged: true }));
                }
                let mut entries = std::collections::BTreeMap::<String, Sync>::new();
                for row in tx.prepare_cached(UNSAVED_CHILDREN)?.query_map([at], |r| Ok((r.get::<_, String>(0)?, parse(&r.get::<_, String>(1)?)?)))? {
                    let (name, sync) = row?;
                    entries.insert(name, sync);
                }
                // Each unsaved inode walks up to the folder, if it is below it, and makes the child
                // it is under at least as pressing. Unsaved inodes are few, and the walks share
                // what they learn: which child of the folder (if any) each inode they met is under.
                let mut state = if n.sync == Sync::Local { Sync::Saved } else { n.sync };
                let mut under: HashMap<Ino, Option<(Ino, String)>> = HashMap::new();
                let unsaved = tx.prepare_cached(UNSAVED)?.query_map([d], |r| Ok((r.get::<_, Ino>(0)?, parse(&r.get::<_, String>(1)?)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut parent = tx.prepare_cached(PARENT)?;
                for (ino, sync) in unsaved {
                    if ino == at { state = worse(state, sync); continue; }
                    let mut path = Vec::new();
                    let mut seen = HashSet::new();
                    let mut here = ino;
                    let below = loop {
                        if let Some(known) = under.get(&here) { break known.clone(); }
                        if here == root || path.len() >= MAX_DEPTH || !seen.insert(here) { break None; }
                        let Some((up, name)) = parent.query_row([here], |r| Ok((r.get::<_, Ino>(0)?, r.get::<_, String>(1)?))).optional()? else { break None; };
                        path.push(here);
                        if up == at { break Some((here, name)); }
                        here = up;
                    };
                    for visited in path { under.insert(visited, below.clone()); }
                    if let Some((_, name)) = below {
                        state = worse(state, sync);
                        let entry = entries.entry(name).or_insert(Sync::Saved);
                        *entry = worse(*entry, sync);
                    }
                }
                let mut page = Vec::new();
                let mut bytes = 0usize;
                let mut next = None;
                let start = after.as_deref().map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
                for (name, sync) in entries.range::<str, _>((start, std::ops::Bound::Unbounded)) {
                    let size = serde_json::to_string(name).map_or(usize::MAX, |s| s.len()) + 16;
                    if page.len() == limit || (!page.is_empty() && bytes.saturating_add(size) > max_bytes) {
                        next = page.last().map(|(name, _): &(String, Sync)| name.clone());
                        break;
                    }
                    bytes = bytes.saturating_add(size);
                    page.push((name.clone(), *sync));
                }
                Ok(Ok(FolderStates { state, entries: page, next, generation, unchanged: false }))
            })();
            drop(tx);
            found
        }).await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (tempfile::TempDir, Arc<Store>, Arc<Session>, crate::Queue) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let client = Client::new(voidfs_sdk::Config { endpoint: "http://localhost:9".into(), access_key_id: "test".into(), secret_access_key: "test".into(), ..Default::default() }).unwrap();
        let connectivity = Connectivity::default();
        for _ in 0..3 { connectivity.unanswered(); }
        let cache = Cache::open(store.clone(), Arc::new(crate::ApiFetcher::new(client.clone())), crate::CacheConfig::default()).await.unwrap();
        let queue = crate::Queue::open(store.clone(), client.clone(), crate::QueueConfig { connectivity: Some(connectivity.clone()), ..Default::default() }).await.unwrap();
        queue.pause(crate::Scope::All).await.unwrap();
        let core = Arc::new(Session::new_writable_with_config(store.clone(), client, cache, queue.clone(), "drive", connectivity,
            StagingConfig { min_free_bytes: 0, quiet_period: None, ..Default::default() }).await.unwrap());
        store.with(|c| c.execute("UPDATE mount_dirs SET listed=1, seq=1 WHERE ino=?1", [core.root()])).unwrap();
        (dir, store, core, queue)
    }

    /// Marks names as saved, or as one of the other states, as publication and its failures do.
    fn set(store: &Store, inos: &[Ino], sync: &str) {
        for ino in inos { store.with(|c| c.execute("UPDATE mount_inodes SET sync=?2 WHERE ino=?1", params![ino, sync])).unwrap(); }
    }

    fn states(entries: &[(&str, Sync)]) -> Vec<(String, Sync)> { entries.iter().map(|(name, sync)| ((*name).to_owned(), *sync)).collect() }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_folder_lists_its_unsaved_entries_and_counts_what_is_below_each() {
        let (_dir, store, core, queue) = fixture().await;
        let root = core.root();
        let saved = core.create(root, "saved", 0o644).await.unwrap().ino;
        let pending = core.create(root, "pending", 0o644).await.unwrap().ino;
        let ds = core.create(root, ".DS_Store", 0o644).await.unwrap().ino;
        let folder = core.mkdir(root, "folder", 0o755).await.unwrap().ino;
        let deep = core.mkdir(folder, "deep", 0o755).await.unwrap().ino;
        let failed = core.create(deep, "failed", 0o644).await.unwrap().ino;
        let saving = core.create(folder, "saving", 0o644).await.unwrap().ino;
        let quiet = core.mkdir(root, "quiet", 0o755).await.unwrap().ino;
        let inside = core.create(quiet, "inside", 0o644).await.unwrap().ino;
        set(&store, &[saved, folder, deep, quiet, inside], "saved");
        set(&store, &[failed], "error");
        set(&store, &[saving], "saving");
        let top = core.folder_states("", None, MAX_STATES, usize::MAX, None).await.unwrap();
        assert!(!top.unchanged && top.next.is_none());
        assert_eq!(top.entries, states(&[(".DS_Store", Sync::Local), ("folder", Sync::Error), ("pending", Sync::Pending)]),
            "saved names are left out; a folder takes the most pressing state below it; a local file stays local");
        assert_eq!(top.state, Sync::Error);
        let inner = core.folder_states("folder", None, MAX_STATES, usize::MAX, None).await.unwrap();
        assert_eq!(inner.entries, states(&[("deep", Sync::Error), ("saving", Sync::Saving)]));
        assert_eq!(inner.state, Sync::Error);
        let leaf = core.folder_states("folder/deep", None, MAX_STATES, usize::MAX, None).await.unwrap();
        assert_eq!((leaf.entries, leaf.state), (states(&[("failed", Sync::Error)]), Sync::Error));
        let calm = core.folder_states("quiet", None, MAX_STATES, usize::MAX, None).await.unwrap();
        assert!(calm.entries.is_empty() && calm.state == Sync::Saved, "a folder whose names are all saved lists nothing");
        set(&store, &[pending], "conflict");
        let top = core.folder_states("", None, MAX_STATES, usize::MAX, None).await.unwrap();
        assert_eq!((top.entries[2].1, top.state), (Sync::Conflict, Sync::Conflict), "a conflict outranks an error");
        set(&store, &[pending, failed, saving, ds], "saved");
        let top = core.folder_states("", None, MAX_STATES, usize::MAX, None).await.unwrap();
        assert_eq!((top.entries, top.state), (Vec::new(), Sync::Saved));
        assert_eq!(core.folder_states("pending", None, 8, usize::MAX, None).await, Err(FsError::NotDir));
        assert_eq!(core.folder_states("missing", None, 8, usize::MAX, None).await, Err(FsError::NotFound));
        assert_eq!(core.folder_states("folder/../folder", None, 8, usize::MAX, None).await, Err(FsError::InvalidName));
        assert_eq!(core.folder_states("folder/", None, 8, usize::MAX, None).await, Err(FsError::InvalidName));
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn renames_deletions_and_equivalent_spellings_follow_the_local_view() {
        let (_dir, store, core, queue) = fixture().await;
        let root = core.root();
        let left = core.mkdir(root, "left", 0o755).await.unwrap().ino;
        let right = core.mkdir(root, "café", 0o755).await.unwrap().ino;
        let file = core.create(left, "file", 0o644).await.unwrap().ino;
        set(&store, &[left, right], "saved");
        assert_eq!(core.folder_states("", None, 8, usize::MAX, None).await.unwrap().entries, states(&[("left", Sync::Pending)]));
        core.rename(left, "file", right, "moved", RenameMode::Exclusive).await.unwrap();
        let top = core.folder_states("", None, 8, usize::MAX, None).await.unwrap();
        assert_eq!(top.entries, states(&[("café", Sync::Pending)]), "the change counts where the local view shows it");
        let nfd = core.folder_states("cafe\u{301}", None, 8, usize::MAX, None).await.unwrap();
        assert_eq!(nfd.entries, states(&[("moved", Sync::Pending)]), "a path in another normalization finds the folder");
        set(&store, &[file], "saved");
        core.unlink(right, "moved").await.unwrap();
        set(&store, &[file], "pending");
        let top = core.folder_states("", None, 8, usize::MAX, None).await.unwrap();
        assert!(top.entries.is_empty() && top.state == Sync::Saved, "an unlinked inode is under no folder: {top:?}");
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pages_follow_the_cursor_and_the_byte_budget() {
        let (_dir, _store, core, queue) = fixture().await;
        let root = core.root();
        for n in 0..10 { core.create(root, &format!("f{n}"), 0o644).await.unwrap(); }
        let first = core.folder_states("", None, 4, usize::MAX, None).await.unwrap();
        assert_eq!(first.entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["f0", "f1", "f2", "f3"]);
        assert_eq!(first.next.as_deref(), Some("f3"));
        let rest = core.folder_states("", first.next.as_deref(), MAX_STATES, usize::MAX, None).await.unwrap();
        assert_eq!(rest.entries.len(), 6);
        assert!(rest.next.is_none());
        let small = core.folder_states("", None, MAX_STATES, 40, None).await.unwrap();
        assert_eq!((small.entries.len(), small.next.as_deref()), (2, Some("f1")), "two 20-byte entries fit 40 bytes");
        let tiny = core.folder_states("", None, MAX_STATES, 1, None).await.unwrap();
        assert_eq!(tiny.entries.len(), 1, "a page holds one entry at least");
        let unchanged = core.folder_states("", None, MAX_STATES, usize::MAX, Some(first.generation)).await.unwrap();
        assert!(unchanged.unchanged && unchanged.entries.is_empty());
        core.create(root, "another", 0o644).await.unwrap();
        let changed = core.folder_states("", None, MAX_STATES, usize::MAX, Some(first.generation)).await.unwrap();
        assert!(!changed.unchanged && changed.generation > first.generation, "a new name advances the listing generation");
        assert_eq!(changed.entries.len(), 11);
        queue.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_folder_no_longer_listed_still_answers_its_local_changes() {
        let (_dir, store, core, queue) = fixture().await;
        let root = core.root();
        let made = core.mkdir(root, "made", 0o755).await.unwrap().ino;
        let inner = core.create(made, "inner", 0o644).await.unwrap().ino;
        set(&store, &[made], "saved");
        // Published, then unlisted, as the feed's echo of its own publication leaves it.
        store.with(|c| {
            c.execute("UPDATE mount_inodes SET object_id='remote-id', attrs=json_set(attrs, '$.objectId', 'remote-id') WHERE ino=?1", [made])?;
            c.execute("UPDATE mount_dirs SET listed=0, seq=NULL, generation=generation+1 WHERE ino=?1", [made])
        }).unwrap();
        let unlisted = core.folder_states("made", None, 8, usize::MAX, None).await.unwrap();
        assert_eq!((unlisted.entries, unlisted.state), (states(&[("inner", Sync::Pending)]), Sync::Pending), "its unsaved names are still on this Mac");
        set(&store, &[inner], "saved");
        let calm = core.folder_states("made", None, 8, usize::MAX, None).await.unwrap();
        assert_eq!((calm.entries, calm.state), (Vec::new(), Sync::Saved), "and everything else is saved");
        queue.close().await;
    }

    #[test]
    fn badge_queries_search_the_parent_and_pending_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.with(|c| {
            let plan = |sql: &str| -> rusqlite::Result<String> {
                let mut q = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
                let count = q.parameter_count();
                let rows = q.query_map(rusqlite::params_from_iter(std::iter::repeat_n(1i64, count)), |r| r.get::<_, String>(3))?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.join("\n"))
            };
            let children = plan(UNSAVED_CHILDREN)?;
            assert!(children.contains("SEARCH n USING PRIMARY KEY (parent=?)") && children.contains("SEARCH mount_overlay USING PRIMARY KEY (parent=?)"), "{children}");
            assert!(children.contains("SEARCH i USING INTEGER PRIMARY KEY (rowid=?)"), "{children}");
            let unsaved = plan(UNSAVED)?;
            assert!(unsaved.contains("mount_pending_remote_key (drive=?)"), "{unsaved}");
            let parent = plan(PARENT)?;
            assert!(parent.contains("mount_overlay_ino (ino=?)") && parent.contains("SEARCH n USING INDEX"), "{parent}");
            Ok(())
        }).unwrap();
    }
}
