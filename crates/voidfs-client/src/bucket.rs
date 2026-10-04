// SPDX-License-Identifier: Apache-2.0
//! Reading content straight from the bucket with storage credentials (protocol §5.5; step 4,
//! item 6), so that the server carries no content bytes.
//!
//! [`BucketFetcher`] asks the server for a drive's credentials, reads the drive's state from the
//! bucket with the format's reader (`voidfs-format`: the pool's descriptor, the drive's newest
//! checkpoint and its log), and fetches the whole shards a block needs, each checked against its
//! hash (format §4). It reads through the API ([`ApiFetcher`]) instead where the server answers
//! `501`, or the drive's state can't be read from the bucket, for [`BucketConfig::api_for`]; and
//! for any read the bucket can't answer: a credential or network failure, a shard that doesn't
//! match its hash, a version the bucket's log doesn't have yet.
//! - **Credentials** are asked for again once less than [`BucketConfig::renew_before`] of them is
//!   left, and at once if the bucket refuses them.
//! - **What it derives from them**, the drive's state and the shards it holds in memory, is kept
//!   per drive and `accessGeneration`: credentials of another generation, or for another drive of
//!   that name, start afresh.
//! - **A version newer than the state** (its sequence number, or a current version whose ETag the
//!   state doesn't have) makes it read the log after the state first.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use voidfs_core::ids::{DriveId, ShardHash, VersionId};
use voidfs_core::model::{ContentDescriptor, Extent, PoolDescriptor};
use voidfs_core::names::Key;
use voidfs_core::state::DriveState;
use voidfs_sdk::{Storage, StorageCredentials};

use crate::connectivity::Connectivity;
use crate::error::{Error, Result};
use crate::fetch::{ApiFetcher, Content, Fetch};

#[derive(Clone, Debug)]
pub struct BucketConfig {
    /// Credentials with less than this left are asked for again before they are used.
    pub renew_before: Duration,
    /// After a `501`, or a drive's state that can't be read from the bucket, how long the drive
    /// is read through the API before credentials are asked for again.
    pub api_for: Duration,
    /// Shards held in memory per drive, so that the blocks on either side of a shard's middle
    /// fetch it once.
    pub shard_memory_bytes: u64,
    /// Content layouts (extent lists) held per drive.
    pub layouts: usize,
}

impl Default for BucketConfig {
    fn default() -> BucketConfig {
        BucketConfig { renew_before: Duration::from_secs(120), api_for: Duration::from_secs(600), shard_memory_bytes: 64 << 20, layouts: 64 }
    }
}

/// What the fetcher has done since it was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BucketUsage {
    /// Reads answered from the bucket, and through the API.
    pub bucket_reads: u64,
    pub api_reads: u64,
    /// Credentials asked for, and drive states loaded from the bucket.
    pub credentials: u64,
    pub loads: u64,
    /// Shards fetched from the bucket, and their bytes.
    pub shards: u64,
    pub shard_bytes: u64,
}

/// One drive as storage credentials reach it.
struct View {
    storage: Storage,
    generation: u64,
    drive: DriveId,
    pool: PoolDescriptor,
    expires: DateTime<Utc>,
    state: Mutex<Arc<DriveState>>,
    layouts: Mutex<Lru<String, Arc<Layout>>>,
    shards: Mutex<Lru<ShardHash, Bytes>>,
    /// Shards being fetched: a read that needs one waits for that fetch.
    fetching: Mutex<HashMap<ShardHash, Shared<BoxFuture<'static, Result<Bytes>>>>>,
}

enum Slot {
    Empty,
    /// The server answered `501` until then.
    Api(Instant),
    Bucket(Arc<View>),
}

/// The fetcher.
pub struct BucketFetcher {
    client: voidfs_sdk::Client,
    api: ApiFetcher,
    conn: Option<Connectivity>,
    cfg: BucketConfig,
    drives: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Slot>>>>,
    usage: Counters,
}

#[derive(Default)]
struct Counters {
    bucket_reads: AtomicU64,
    api_reads: AtomicU64,
    credentials: AtomicU64,
    loads: AtomicU64,
    shards: AtomicU64,
    shard_bytes: AtomicU64,
}

impl BucketFetcher {
    pub fn new(client: voidfs_sdk::Client, cfg: BucketConfig) -> BucketFetcher {
        BucketFetcher { api: ApiFetcher::new(client.clone()), client, conn: None, cfg, drives: Mutex::default(), usage: Counters::default() }
    }

    /// Fails at once with [`Error::Offline`] while `conn` says the server can't be reached.
    pub fn with_connectivity(mut self, conn: Connectivity) -> BucketFetcher {
        self.api = self.api.with_connectivity(conn.clone());
        self.conn = Some(conn);
        self
    }

    pub fn usage(&self) -> BucketUsage {
        let u = &self.usage;
        let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
        BucketUsage { bucket_reads: l(&u.bucket_reads), api_reads: l(&u.api_reads), credentials: l(&u.credentials), loads: l(&u.loads), shards: l(&u.shards), shard_bytes: l(&u.shard_bytes) }
    }

    fn slot(&self, drive: &str) -> Arc<tokio::sync::Mutex<Slot>> {
        self.drives.lock().unwrap().entry(drive.to_owned()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(Slot::Empty))).clone()
    }

    /// The drive as its credentials reach it, renewed if they are due or `refused`, or `None` to
    /// read through the API.
    async fn view(&self, drive: &str, refused: bool) -> Result<Option<Arc<View>>> {
        let slot = self.slot(drive);
        let mut held = slot.lock().await;
        match &*held {
            Slot::Api(until) if Instant::now() < *until => return Ok(None),
            Slot::Bucket(v) if !refused && !self.due(v) => return Ok(Some(v.clone())),
            _ => {}
        }
        self.usage.credentials.fetch_add(1, Ordering::Relaxed);
        let creds = match self.client.storage_credentials(drive).await {
            Ok(c) => c,
            Err(e) if e.status() == Some(501) => {
                *held = Slot::Api(Instant::now() + self.cfg.api_for);
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let view = match &*held {
            Slot::Bucket(old) if same_view(old.generation, &old.drive, &creds) => Arc::new(View {
                storage: self.client.storage(creds.clone())?,
                generation: old.generation,
                drive: old.drive.clone(),
                pool: old.pool.clone(),
                expires: expiry(&creds)?,
                state: Mutex::new(old.state.lock().unwrap().clone()),
                layouts: Mutex::new(std::mem::replace(&mut *old.layouts.lock().unwrap(), Lru::new(0))),
                shards: Mutex::new(std::mem::replace(&mut *old.shards.lock().unwrap(), Lru::new(0))),
                fetching: Mutex::default(),
            }),
            _ => match self.load(drive, creds).await {
                Ok(v) => Arc::new(v),
                // The bucket can't be reached from here, or doesn't hold what the server says.
                Err(_) => {
                    *held = Slot::Api(Instant::now() + self.cfg.api_for);
                    return Ok(None);
                }
            },
        };
        *held = Slot::Bucket(view.clone());
        Ok(Some(view))
    }

    fn due(&self, v: &View) -> bool {
        (v.expires - Utc::now()).to_std().map_or(true, |left| left < self.cfg.renew_before)
    }

    /// Reads the drive's state from the bucket, from scratch.
    async fn load(&self, name: &str, creds: StorageCredentials) -> Result<View> {
        let drive: DriveId = creds.drive_id.parse().map_err(|_| Error::Invalid(format!("the server named drive {name} {:?}", creds.drive_id)))?;
        let generation = creds.access_generation;
        let expires = expiry(&creds)?;
        let storage = self.client.storage(creds)?;
        let (pool, _, state) = voidfs_format::open_drive(&Bucket(&storage), &drive).await.map_err(format_error)?.ok_or_else(|| Error::Invalid(format!("drive {drive} isn't in the bucket")))?;
        self.usage.loads.fetch_add(1, Ordering::Relaxed);
        Ok(View {
            storage,
            generation,
            drive,
            pool,
            expires,
            state: Mutex::new(Arc::new(state)),
            layouts: Mutex::new(Lru::new(self.cfg.layouts as u64)),
            shards: Mutex::new(Lru::new(self.cfg.shard_memory_bytes)),
            fetching: Mutex::default(),
        })
    }

    /// `len` bytes at `offset` of `c` from the bucket, or `None` to read them through the API.
    async fn read_bucket(&self, c: &Content, offset: u64, len: u64) -> Result<Option<Bytes>> {
        let mut refused = false;
        loop {
            let Some(view) = self.view(&c.drive, refused).await? else { return Ok(None) };
            match self.read(&view, c, offset, len).await {
                Err(Error::Fetch(e)) if !refused && matches!(e.status(), Some(400 | 403)) => refused = true,
                r => return r,
            }
        }
    }

    async fn read(&self, view: &View, c: &Content, offset: u64, len: u64) -> Result<Option<Bytes>> {
        let Some(layout) = self.layout(view, c).await? else { return Ok(None) };
        let end = offset.checked_add(len).filter(|e| *e <= layout.size).ok_or_else(|| Error::ShortRead { key: c.key.clone(), offset, expected: len, got: layout.size.saturating_sub(offset) })?;
        let pieces = layout.pieces(offset, end);
        let wanted: Vec<ShardHash> = {
            let mut w: Vec<ShardHash> = pieces.iter().filter_map(|p| match p.0 { Extent::Shard { s, .. } => Some(*s), _ => None }).collect();
            w.dedup();
            w
        };
        let fetched = futures::future::try_join_all(wanted.iter().map(|h| self.shard(view, h))).await?;
        let shards: HashMap<ShardHash, Bytes> = wanted.into_iter().zip(fetched).collect();
        let mut out = BytesMut::with_capacity(len as usize);
        for (e, from, to) in pieces {
            match e {
                Extent::Shard { s, .. } => out.extend_from_slice(&shards[s][from as usize..to as usize]),
                Extent::Zero { .. } => out.resize(out.len() + (to - from) as usize, 0),
                Extent::Data { d } => out.extend_from_slice(&d[from as usize..to as usize]),
            }
        }
        self.usage.bucket_reads.fetch_add(1, Ordering::Relaxed);
        Ok(Some(out.freeze()))
    }

    /// Where `c`'s bytes are, from the drive's state, read further from the log if `c` is newer;
    /// `None` if the bucket's log doesn't have it yet.
    async fn layout(&self, view: &View, c: &Content) -> Result<Option<Arc<Layout>>> {
        let id = format!("{}\n{}", c.version_id, c.etag);
        if let Some(l) = view.layouts.lock().unwrap().get(&id) {
            return Ok(Some(l));
        }
        let key = Key::parse(&c.key).map_err(|e| Error::Invalid(format!("{}: {e}", c.key)))?;
        let version: Option<VersionId> = (!c.version_id.is_empty()).then(|| c.version_id.parse()).transpose().map_err(|_| Error::Invalid(format!("version id {:?}", c.version_id)))?;
        let mut state = view.state.lock().unwrap().clone();
        let mut found = resolve(&state, &key, version.as_ref());
        let newer = match (&found, &version) {
            (None, _) => true,
            (Some((_, etag)), _) => !c.etag.is_empty() && *etag != c.etag && version.is_none(),
        };
        if newer {
            let mut next = (*state).clone();
            voidfs_format::catch_up(&Bucket(&view.storage), &view.drive, &mut next).await.map_err(format_error)?;
            state = Arc::new(next);
            *view.state.lock().unwrap() = state.clone();
            found = resolve(&state, &key, version.as_ref());
        }
        let Some((desc, etag)) = found else { return Ok(None) };
        if !c.etag.is_empty() && etag != c.etag {
            return Err(Error::Changed { key: c.key.clone(), expected: c.etag.clone(), got: etag });
        }
        let extents = voidfs_format::extents(&Bucket(&view.storage), &desc).await.map_err(format_error)?;
        let layout = Arc::new(Layout::new(extents));
        // An ETag names one content; without one, the current content may change.
        if !c.etag.is_empty() {
            view.layouts.lock().unwrap().insert(id, layout.clone(), 1);
        }
        Ok(Some(layout))
    }

    /// A whole shard, from memory or the bucket, checked against its hash (format §4). Reads that
    /// need a shard being fetched wait for that fetch; a failure isn't kept.
    async fn shard(&self, view: &View, h: &ShardHash) -> Result<Bytes> {
        let fetch = {
            let mut fetching = view.fetching.lock().unwrap();
            if let Some(b) = view.shards.lock().unwrap().get(h) {
                return Ok(b);
            }
            fetching
                .entry(*h)
                .or_insert_with(|| {
                    let (storage, h) = (view.storage.clone(), *h);
                    async move {
                        let b = storage.get(&voidfs_format::shard_path(&h)).await?.ok_or_else(|| Error::Invalid(format!("shard {h} is missing from the bucket")))?;
                        if ShardHash::of(&b) != h {
                            return Err(Error::Invalid(format!("shard {h} in the bucket doesn't match its hash")));
                        }
                        Ok(b)
                    }
                    .boxed()
                    .shared()
                })
                .clone()
        };
        let r = fetch.await;
        let mut fetching = view.fetching.lock().unwrap();
        if fetching.remove(h).is_some()
            && let Ok(b) = &r
        {
            self.usage.shards.fetch_add(1, Ordering::Relaxed);
            self.usage.shard_bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
            view.shards.lock().unwrap().insert(*h, b.clone(), b.len() as u64);
        }
        r
    }
}

impl Fetch for BucketFetcher {
    fn fetch<'a>(&'a self, c: &'a Content, offset: u64, len: u64) -> BoxFuture<'a, Result<Bytes>> {
        Box::pin(async move {
            if let Some(conn) = &self.conn {
                conn.check()?;
            }
            match self.read_bucket(c, offset, len).await {
                Ok(Some(b)) => return Ok(b),
                Err(e @ (Error::Changed { .. } | Error::ShortRead { .. } | Error::Offline)) => return Err(e),
                Ok(None) | Err(_) => {}
            }
            self.usage.api_reads.fetch_add(1, Ordering::Relaxed);
            self.api.fetch(c, offset, len).await
        })
    }
}

/// Whether credentials keep what a view derived from earlier ones: the same drive, and the same
/// access rules (protocol §5.5).
fn same_view(generation: u64, drive: &DriveId, creds: &StorageCredentials) -> bool {
    creds.access_generation == generation && creds.drive_id == drive.as_str()
}

fn expiry(c: &StorageCredentials) -> Result<DateTime<Utc>> {
    let at = &c.storage.credentials.expires_at;
    DateTime::parse_from_rfc3339(at).map(|t| t.with_timezone(&Utc)).map_err(|_| Error::Invalid(format!("credentials expiring at {at:?}")))
}

fn format_error(e: anyhow::Error) -> Error {
    match e.downcast::<voidfs_sdk::Error>() {
        Ok(e) => e.into(),
        Err(e) => Error::Invalid(format!("{e:#}")),
    }
}

/// The descriptor and ETag of `key` at `version`, or now.
fn resolve(state: &DriveState, key: &Key, version: Option<&VersionId>) -> Option<(ContentDescriptor, String)> {
    match version {
        Some(v) => state.find_version(key, v).map(|r| (r.content.clone().unwrap_or_else(ContentDescriptor::empty), r.etag.clone())),
        None => {
            let r = state.record(&state.lookup(key)?)?;
            Some((r.content.clone().unwrap_or_else(ContentDescriptor::empty), r.etag.clone()))
        }
    }
}

/// The storage, as the format's reader reads it.
struct Bucket<'a>(&'a Storage);

impl voidfs_format::Source for Bucket<'_> {
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, anyhow::Result<Option<Bytes>>> {
        Box::pin(async move { Ok(self.0.get(path).await?) })
    }

    fn list<'a>(&'a self, dir: &'a str, after: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<Vec<String>>> {
        Box::pin(async move { Ok(self.0.list(dir, after).await?) })
    }
}

/// A content's extents and where each starts.
struct Layout {
    extents: Vec<Extent>,
    starts: Vec<u64>,
    size: u64,
}

impl Layout {
    fn new(extents: Vec<Extent>) -> Layout {
        let mut starts = Vec::with_capacity(extents.len());
        let mut at = 0;
        for e in &extents {
            starts.push(at);
            at += e.len();
        }
        Layout { extents, starts, size: at }
    }

    /// The extents that `[from, to)` covers, each with the part of it covered.
    fn pieces(&self, from: u64, to: u64) -> Vec<(&Extent, u64, u64)> {
        let mut out = Vec::new();
        let first = self.starts.partition_point(|s| *s <= from).saturating_sub(1);
        for (e, start) in self.extents[first..].iter().zip(&self.starts[first..]) {
            if *start >= to {
                break;
            }
            let lo = from.max(*start) - start;
            let hi = to.min(start + e.len()) - start;
            if hi > lo {
                out.push((e, lo, hi));
            }
        }
        out
    }
}

/// Least recently used first, within a total weight.
struct Lru<K, V> {
    entries: HashMap<K, (V, u64)>,
    order: VecDeque<K>,
    weight: u64,
    max: u64,
}

impl<K: Clone + Eq + std::hash::Hash, V: Clone> Lru<K, V> {
    fn new(max: u64) -> Lru<K, V> {
        Lru { entries: HashMap::new(), order: VecDeque::new(), weight: 0, max }
    }

    fn get(&mut self, k: &K) -> Option<V> {
        let v = self.entries.get(k)?.0.clone();
        if let Some(i) = self.order.iter().position(|x| x == k) {
            let k = self.order.remove(i).expect("listed");
            self.order.push_back(k);
        }
        Some(v)
    }

    fn insert(&mut self, k: K, v: V, weight: u64) {
        if weight > self.max {
            return;
        }
        if let Some((_, w)) = self.entries.insert(k.clone(), (v, weight)) {
            self.weight -= w;
            self.order.retain(|x| x != &k);
        }
        self.weight += weight;
        self.order.push_back(k);
        while self.weight > self.max {
            let Some(old) = self.order.pop_front() else { break };
            if let Some((_, w)) = self.entries.remove(&old) {
                self.weight -= w;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_covers_the_extents_it_overlaps() {
        let h = ShardHash::of(b"x");
        let l = Layout::new(vec![Extent::Shard { s: h, n: 10 }, Extent::Zero { z: 5 }, Extent::Data { d: Bytes::from_static(b"abc") }, Extent::Shard { s: h, n: 10 }]);
        assert_eq!(l.size, 28);
        let got = |from, to| l.pieces(from, to).into_iter().map(|(e, a, b)| (e.len(), a, b)).collect::<Vec<_>>();
        assert_eq!(got(0, 28), [(10, 0, 10), (5, 0, 5), (3, 0, 3), (10, 0, 10)]);
        assert_eq!(got(9, 16), [(10, 9, 10), (5, 0, 5), (3, 0, 1)], "across a boundary");
        assert_eq!(got(10, 15), [(5, 0, 5)], "exactly one extent");
        assert_eq!(got(27, 28), [(10, 9, 10)], "the last byte");
        assert_eq!(got(5, 5), [], "nothing");
    }

    #[test]
    fn credentials_keep_a_view_only_for_the_same_drive_and_rules() {
        let drive: DriveId = "d-00000000-0000-4000-8000-000000000000".parse().unwrap();
        let creds = |drive_id: &str, generation| -> StorageCredentials {
            serde_json::from_value(serde_json::json!({ "driveId": drive_id, "accessGeneration": generation,
                "storage": { "backend": "s3", "bucket": "b", "credentials": { "accessKeyId": "a", "secretAccessKey": "s", "expiresAt": "2030-01-01T00:00:00Z" } } }))
            .unwrap()
        };
        assert!(same_view(7, &drive, &creds(drive.as_str(), 7)));
        assert!(!same_view(7, &drive, &creds(drive.as_str(), 8)), "the drive's access rules changed");
        assert!(!same_view(7, &drive, &creds("d-00000000-0000-4000-8000-000000000001", 7)), "another drive of that name");
        assert_eq!(expiry(&creds(drive.as_str(), 7)).unwrap().to_rfc3339(), "2030-01-01T00:00:00+00:00");
    }

    #[test]
    fn the_least_recently_used_go_first() {
        let mut l: Lru<u32, u32> = Lru::new(10);
        l.insert(1, 1, 4);
        l.insert(2, 2, 4);
        assert_eq!(l.get(&1), Some(1));
        l.insert(3, 3, 4);
        assert_eq!((l.get(&1), l.get(&2), l.get(&3)), (Some(1), None, Some(3)), "2 was used longest ago");
        l.insert(4, 4, 11);
        assert_eq!(l.get(&4), None, "heavier than the whole");
    }
}
