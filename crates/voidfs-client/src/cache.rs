// SPDX-License-Identifier: Apache-2.0
//! The block cache (step 4, item 3).
//!
//! Content is cached in blocks of one content version, 8 MiB by default (SpaceFS's block size),
//! keyed by drive, ETag and block number. An ETag names one content, so a block never goes stale.
//! - **Memory:** the blocks fetched most recently, up to 192 MiB (SpaceFS's in-memory cache).
//! - **Disk:** a file per block in `<state>/cache/`, with its index in the state database. A
//!   CRC-32C of every 64 KiB is taken when the block is filled and checked on every read of it, so
//!   a random read checks only what it reads; a block that fails is dropped and fetched again.
//! - **Limits:** at most `max_bytes` (20 GiB, as SpaceFS's), and never less than `min_free_bytes`
//!   free on the volume. Filling evicts what was used longest ago; pinned content is never
//!   evicted. When nothing more can go, a block isn't stored: the cache stops filling rather
//!   than fill the disk.
//! - **Fetching:** concurrent reads of one block wait on one fetch, at most `fetches` at once.
//! - **Read-ahead:** a [`Reader`] that reads forward fetches the blocks ahead of it, up to
//!   `read_ahead_bytes`, so a reader that waits for each read before the next (Finder's copy)
//!   still streams, whatever adapter the mount uses.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::{Bytes, BytesMut};
use futures::{FutureExt, StreamExt};
use futures::future::{BoxFuture, Shared};
use rusqlite::params;
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, Semaphore, oneshot};

use crate::error::{Error, Result};
use crate::fetch::{Content, Fetch};
use crate::store::Store;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;
/// Bytes per checksum.
const SUM_CHUNK: u64 = 64 * KIB;
/// A disk hit records its time in the index at most this often per block.
const TOUCH_EVERY_SECS: i64 = 60;

#[derive(Clone, Debug)]
pub struct CacheConfig {
    pub max_bytes: u64,
    /// Below this much free space on the cache's volume, the cache evicts, and then stops
    /// filling.
    pub min_free_bytes: u64,
    pub memory_bytes: u64,
    pub block_size: u64,
    /// How far ahead of a forward reader to fetch, at most.
    pub read_ahead_bytes: u64,
    /// Fetches at once, reads and read-ahead together.
    pub fetches: usize,
    /// Of those, at most this many for read-ahead, so that reads always have room.
    pub prefetches: usize,
    /// Free bytes on the volume that holds `path`. `None` asks the volume; tests set it.
    pub free_space: Option<fn(&Path) -> io::Result<u64>>,
}

impl Default for CacheConfig {
    fn default() -> CacheConfig {
        CacheConfig {
            max_bytes: 20 * GIB,
            min_free_bytes: 5 * GIB,
            memory_bytes: 192 * MIB,
            block_size: 8 * MIB,
            read_ahead_bytes: 64 * MIB,
            fetches: 16,
            prefetches: 8,
            free_space: None,
        }
    }
}

/// What the cache holds and has done since it opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub disk_bytes: u64,
    pub disk_blocks: u64,
    pub pinned_bytes: u64,
    pub memory_bytes: u64,
    pub fetches: u64,
    pub fetched_bytes: u64,
    pub memory_hits: u64,
    pub disk_hits: u64,
    /// Blocks on disk that failed their checksum, or went missing, and were dropped.
    pub dropped: u64,
    pub evicted: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct BlockKey {
    drive: Arc<str>,
    etag: Arc<str>,
    index: u64,
}

impl BlockKey {
    fn of(c: &Content, index: u64) -> BlockKey {
        BlockKey { drive: c.drive.as_str().into(), etag: c.etag.as_str().into(), index }
    }

    /// `<dir>/<aa>/<sha256 of drive, ETag and block>`.
    fn path(&self, dir: &Path) -> PathBuf {
        let mut h = Sha256::new();
        h.update(self.drive.as_bytes());
        h.update([0]);
        h.update(self.etag.as_bytes());
        h.update([0]);
        h.update(self.index.to_be_bytes());
        let name = hex::encode(h.finalize());
        dir.join(&name[..2]).join(name)
    }

    fn content(&self) -> (Arc<str>, Arc<str>) {
        (self.drive.clone(), self.etag.clone())
    }
}

/// Least recently used first.
struct Lru<V> {
    map: HashMap<BlockKey, (u64, V)>,
    order: BTreeMap<u64, BlockKey>,
    tick: u64,
}

impl<V> Lru<V> {
    fn new() -> Self {
        Lru { map: HashMap::new(), order: BTreeMap::new(), tick: 0 }
    }

    /// The value, now the most recently used.
    fn get(&mut self, k: &BlockKey) -> Option<&V> {
        let (t, _) = self.map.get_mut(k)?;
        self.order.remove(t);
        self.tick += 1;
        *t = self.tick;
        self.order.insert(self.tick, k.clone());
        self.map.get(k).map(|(_, v)| v)
    }

    fn insert(&mut self, k: BlockKey, v: V) -> Option<V> {
        let old = self.remove(&k);
        self.tick += 1;
        self.order.insert(self.tick, k.clone());
        self.map.insert(k, (self.tick, v));
        old
    }

    fn remove(&mut self, k: &BlockKey) -> Option<V> {
        let (t, v) = self.map.remove(k)?;
        self.order.remove(&t);
        Some(v)
    }

    fn oldest(&self) -> impl Iterator<Item = &BlockKey> {
        self.order.values()
    }
}

#[derive(Clone)]
struct DiskEntry {
    len: u64,
    sums: Arc<[u32]>,
    /// `last_used` as the index has it, in Unix seconds.
    stored_used: i64,
}

/// Pinned contents: drive and ETag.
type Pins = HashSet<(Arc<str>, Arc<str>)>;

struct State {
    disk: Lru<DiskEntry>,
    disk_bytes: u64,
    pins: Pins,
    mem: Lru<Bytes>,
    mem_bytes: u64,
}

type Fill = Shared<BoxFuture<'static, Result<Bytes>>>;

#[derive(Default)]
struct Stats {
    fetches: AtomicU64,
    fetched_bytes: AtomicU64,
    memory_hits: AtomicU64,
    disk_hits: AtomicU64,
    dropped: AtomicU64,
    evicted: AtomicU64,
}

struct Inner {
    cfg: CacheConfig,
    store: Arc<Store>,
    dir: PathBuf,
    fetcher: Arc<dyn Fetch>,
    state: Mutex<State>,
    fills: Mutex<HashMap<BlockKey, Fill>>,
    /// Fills still running, for [`Cache::settle`]. Apart from the rest, so that a fill can let go
    /// of the cache before it says it is done.
    busy: Arc<Busy>,
    fetch_permits: Semaphore,
    prefetch_permits: Semaphore,
    stats: Stats,
}

#[derive(Default)]
struct Busy {
    n: AtomicUsize,
    settled: Notify,
}

/// The block cache. Cloning shares it.
#[derive(Clone)]
pub struct Cache(Arc<Inner>);

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn sums_of(data: &[u8]) -> Vec<u32> {
    data.chunks(SUM_CHUNK as usize).map(crc32c::crc32c).collect()
}

fn sums_to_blob(sums: &[u32]) -> Vec<u8> {
    sums.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn sums_from_blob(b: &[u8]) -> Option<Arc<[u32]>> {
    if !b.len().is_multiple_of(4) {
        return None;
    }
    Some(b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect())
}

impl Cache {
    /// Opens the cache in `<store>/cache`, with its index from the store. Files the index doesn't
    /// list are removed, and so are rows whose file is missing or the wrong size.
    pub async fn open(store: Arc<Store>, fetcher: Arc<dyn Fetch>, cfg: CacheConfig) -> Result<Cache> {
        if cfg.block_size == 0 || cfg.fetches == 0 {
            return Err(Error::Invalid("the block size and the number of fetches must be more than 0".into()));
        }
        let dir = store.dir().join("cache");
        let s2 = store.clone();
        let d2 = dir.clone();
        let (disk, disk_bytes, pins) = tokio::task::spawn_blocking(move || load(&s2, &d2)).await??;
        let inner = Inner {
            fetch_permits: Semaphore::new(cfg.fetches),
            prefetch_permits: Semaphore::new(cfg.prefetches.clamp(1, cfg.fetches)),
            cfg,
            store,
            dir,
            fetcher,
            state: Mutex::new(State { disk, disk_bytes, pins, mem: Lru::new(), mem_bytes: 0 }),
            fills: Mutex::new(HashMap::new()),
            busy: Arc::default(),
            stats: Stats::default(),
        };
        let cache = Cache(Arc::new(inner));
        let c2 = cache.clone();
        tokio::task::spawn_blocking(move || c2.make_room(0)).await??;
        Ok(cache)
    }

    pub fn config(&self) -> &CacheConfig {
        &self.0.cfg
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Reads `len` bytes at `offset` of `c`, fewer where it ends.
    pub async fn read(&self, c: &Content, offset: u64, len: u64) -> Result<Bytes> {
        if offset >= c.size || len == 0 {
            return Ok(Bytes::new());
        }
        let bs = self.0.cfg.block_size;
        let end = offset.saturating_add(len).min(c.size);
        let (first, last) = (offset / bs, (end - 1) / bs);
        let part = |i: u64| {
            let lo = offset.max(i * bs) - i * bs;
            let hi = end.min((i + 1) * bs) - i * bs;
            self.read_block(c, i, lo, hi)
        };
        if first == last {
            return part(first).await;
        }
        let parts = futures::future::try_join_all((first..=last).map(part)).await?;
        let mut out = BytesMut::with_capacity((end - offset) as usize);
        for p in parts {
            out.extend_from_slice(&p);
        }
        Ok(out.freeze())
    }

    /// A reader of `c` that reads ahead while it is read forward.
    pub fn reader(&self, c: Content) -> Reader {
        Reader { cache: self.clone(), content: c, next: 0, last_start: 0, ahead: 0, requested_to: 0, started: false }
    }

    fn block_len(&self, c: &Content, index: u64) -> u64 {
        let bs = self.0.cfg.block_size;
        bs.min(c.size - index * bs)
    }

    /// Bytes `lo..hi` of block `index`.
    async fn read_block(&self, c: &Content, index: u64, lo: u64, hi: u64) -> Result<Bytes> {
        let key = BlockKey::of(c, index);
        let on_disk = {
            let mut st = self.state();
            // A use from memory is a use: it keeps the block's file from eviction too.
            let on_disk = st.disk.get(&key).cloned();
            if let Some(b) = st.mem.get(&key) {
                self.0.stats.memory_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(b.slice(lo as usize..hi as usize));
            }
            on_disk
        };
        if let Some(e) = on_disk {
            let this = self.clone();
            let k = key.clone();
            match tokio::task::spawn_blocking(move || this.read_disk(&k, &e, lo, hi)).await? {
                Ok(b) => {
                    self.0.stats.disk_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(b);
                }
                Err(_) => self.drop_block(&key).await,
            }
        }
        let block = self.fill(c, key, false).await?;
        Ok(block.slice(lo as usize..hi as usize))
    }

    /// Reads and checks the 64 KiB pieces of a block file that hold `lo..hi`.
    fn read_disk(&self, key: &BlockKey, e: &DiskEntry, lo: u64, hi: u64) -> io::Result<Bytes> {
        let file = std::fs::File::open(key.path(&self.0.dir))?;
        if file.metadata()?.len() != e.len {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "block file has the wrong size"));
        }
        let (first, last) = (lo / SUM_CHUNK, (hi - 1) / SUM_CHUNK);
        let start = first * SUM_CHUNK;
        let stop = ((last + 1) * SUM_CHUNK).min(e.len);
        let mut buf = vec![0u8; (stop - start) as usize];
        file.read_exact_at(&mut buf, start)?;
        for (i, piece) in buf.chunks(SUM_CHUNK as usize).enumerate() {
            if e.sums.get(first as usize + i) != Some(&crc32c::crc32c(piece)) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "block failed its checksum"));
            }
        }
        let now = now_secs();
        if now - e.stored_used >= TOUCH_EVERY_SECS {
            self.touch(key, now);
        }
        Ok(Bytes::from(buf).slice((lo - start) as usize..(hi - start) as usize))
    }

    /// Records a disk hit's time in the index (best effort: it orders eviction after a restart).
    fn touch(&self, key: &BlockKey, now: i64) {
        if let Some(e) = self.state().disk.map.get_mut(key) {
            e.1.stored_used = now;
        }
        let _ = self.0.store.with(|c| {
            c.execute("UPDATE cache_blocks SET last_used = ?4 WHERE drive = ?1 AND etag = ?2 AND block = ?3", params![&*key.drive, &*key.etag, key.index as i64, now])
        });
    }

    /// Forgets a block whose file failed, so that the next read fetches it again.
    async fn drop_block(&self, key: &BlockKey) {
        let removed = {
            let mut st = self.state();
            let e = st.disk.remove(key);
            if let Some(e) = &e {
                st.disk_bytes -= e.len;
            }
            e.is_some()
        };
        if removed {
            self.0.stats.dropped.fetch_add(1, Ordering::Relaxed);
            let this = self.clone();
            let k = key.clone();
            let _ = tokio::task::spawn_blocking(move || this.delete_files(&[k])).await;
        }
    }

    /// The block, fetched once however many readers want it. The fetch runs as its own task, so
    /// a reader that gives up doesn't stop it, and read-ahead runs while nobody waits.
    fn fill(&self, c: &Content, key: BlockKey, ahead: bool) -> Fill {
        let mut fills = self.0.fills.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(f) = fills.get(&key) {
            return f.clone();
        }
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        let c = c.clone();
        let k = key.clone();
        let busy = self.0.busy.clone();
        busy.n.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            let r = this.fetch(&c, &k, ahead).await;
            if let Ok(b) = &r {
                this.remember(&k, b.clone());
            }
            let _ = tx.send(r.clone());
            if let Ok(b) = r {
                let this2 = this.clone();
                let k2 = k.clone();
                let _ = tokio::task::spawn_blocking(move || this2.store_block(&k2, &b)).await;
            }
            this.0.fills.lock().unwrap_or_else(|p| p.into_inner()).remove(&k);
            // Let go of the cache, and so of the store, first: whoever settles and then drops the
            // cache can open the state directory again at once.
            drop(this);
            if busy.n.fetch_sub(1, Ordering::SeqCst) == 1 {
                busy.settled.notify_waiters();
            }
        });
        let f = async move { rx.await.unwrap_or_else(|_| Err(Error::Invalid("the fetch stopped".into()))) }.boxed().shared();
        fills.insert(key, f.clone());
        f
    }

    async fn fetch(&self, c: &Content, key: &BlockKey, ahead: bool) -> Result<Bytes> {
        let _ahead = if ahead { Some(self.0.prefetch_permits.acquire().await.map_err(|_| Error::Invalid("closed".into()))?) } else { None };
        let _permit = self.0.fetch_permits.acquire().await.map_err(|_| Error::Invalid("closed".into()))?;
        let offset = key.index * self.0.cfg.block_size;
        let len = self.block_len(c, key.index);
        let b = self.0.fetcher.fetch(c, offset, len).await?;
        if b.len() as u64 != len {
            return Err(Error::ShortRead { key: c.key.clone(), offset, expected: len, got: b.len() as u64 });
        }
        self.0.stats.fetches.fetch_add(1, Ordering::Relaxed);
        self.0.stats.fetched_bytes.fetch_add(len, Ordering::Relaxed);
        Ok(b)
    }

    /// Keeps a fetched block in memory, evicting the oldest past the memory budget.
    fn remember(&self, key: &BlockKey, b: Bytes) {
        let mut st = self.state();
        let n = b.len() as u64;
        if n > self.0.cfg.memory_bytes {
            return;
        }
        if let Some(old) = st.mem.insert(key.clone(), b) {
            st.mem_bytes -= old.len() as u64;
        }
        st.mem_bytes += n;
        while st.mem_bytes > self.0.cfg.memory_bytes {
            let Some(k) = st.mem.oldest().next().cloned() else { break };
            if let Some(v) = st.mem.remove(&k) {
                st.mem_bytes -= v.len() as u64;
            }
        }
    }

    fn free_space(&self) -> io::Result<u64> {
        std::fs::create_dir_all(&self.0.dir)?;
        match self.0.cfg.free_space {
            Some(f) => f(&self.0.dir),
            None => fs4::available_space(&self.0.dir),
        }
    }

    /// Evicts until `need` more bytes fit under the cap and above the floor. Whether they do.
    fn make_room(&self, need: u64) -> Result<bool> {
        let free = self.free_space()?;
        let cfg = &self.0.cfg;
        let (victims, fits) = {
            let mut st = self.state();
            let mut freed = 0;
            let fits = |st: &State, freed: u64| st.disk_bytes + need <= cfg.max_bytes && (free + freed).saturating_sub(need) >= cfg.min_free_bytes;
            let mut victims = Vec::new();
            let candidates: Vec<BlockKey> = st.disk.oldest().filter(|k| !st.pins.contains(&k.content())).cloned().collect();
            for k in candidates {
                if fits(&st, freed) {
                    break;
                }
                if let Some(e) = st.disk.remove(&k) {
                    st.disk_bytes -= e.len;
                    freed += e.len;
                    victims.push(k);
                }
            }
            let ok = fits(&st, freed);
            (victims, ok)
        };
        self.0.stats.evicted.fetch_add(victims.len() as u64, Ordering::Relaxed);
        self.delete_files(&victims)?;
        Ok(fits)
    }

    /// Removes blocks' files and rows.
    fn delete_files(&self, keys: &[BlockKey]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        for k in keys {
            match std::fs::remove_file(k.path(&self.0.dir)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        self.0.store.with(|c| {
            let tx = c.transaction()?;
            {
                let mut del = tx.prepare_cached("DELETE FROM cache_blocks WHERE drive = ?1 AND etag = ?2 AND block = ?3")?;
                for k in keys {
                    del.execute(params![&*k.drive, &*k.etag, k.index as i64])?;
                }
            }
            tx.commit()
        })
    }

    /// Writes a fetched block to disk, if there is room for it.
    fn store_block(&self, key: &BlockKey, b: &Bytes) -> Result<()> {
        let len = b.len() as u64;
        if self.state().disk.map.contains_key(key) || !self.make_room(len)? {
            return Ok(());
        }
        let path = key.path(&self.0.dir);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, b)?;
        std::fs::rename(&tmp, &path)?;
        let sums: Arc<[u32]> = sums_of(b).into();
        let now = now_secs();
        let blob = sums_to_blob(&sums);
        self.0.store.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO cache_blocks(drive, etag, block, len, sums, last_used) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![&*key.drive, &*key.etag, key.index as i64, len as i64, blob, now],
            )
        })?;
        let mut st = self.state();
        if st.disk.insert(key.clone(), DiskEntry { len, sums, stored_used: now }).is_none() {
            st.disk_bytes += len;
        }
        Ok(())
    }

    /// Fetches `c`'s block `index` in the background, unless the cache has it or is fetching it.
    pub fn prefetch(&self, c: &Content, index: u64) {
        if index * self.0.cfg.block_size >= c.size {
            return;
        }
        let key = BlockKey::of(c, index);
        {
            let st = self.state();
            if st.mem.map.contains_key(&key) || st.disk.map.contains_key(&key) {
                return;
            }
        }
        drop(self.fill(c, key, true));
    }

    /// Fetches every block of `c` the cache lacks, `fetches` at a time: a pinned file kept whole.
    pub async fn download(&self, c: &Content) -> Result<()> {
        let n = c.size.div_ceil(self.0.cfg.block_size);
        let reads = futures::stream::iter(0..n).map(|i| {
            let len = self.block_len(c, i);
            async move { self.read_block(c, i, 0, len).await.map(drop) }
        });
        let mut s = reads.buffer_unordered(self.0.cfg.fetches);
        while let Some(r) = s.next().await {
            r?;
        }
        Ok(())
    }

    /// Never evicts the blocks of this content.
    pub async fn pin(&self, drive: &str, etag: &str) -> Result<()> {
        let (d, e) = (drive.to_owned(), etag.to_owned());
        let store = self.0.store.clone();
        tokio::task::spawn_blocking(move || store.with(|c| c.execute("INSERT OR IGNORE INTO cache_pins(drive, etag) VALUES (?1, ?2)", params![d, e]))).await??;
        self.state().pins.insert((drive.into(), etag.into()));
        Ok(())
    }

    pub async fn unpin(&self, drive: &str, etag: &str) -> Result<()> {
        let (d, e) = (drive.to_owned(), etag.to_owned());
        let store = self.0.store.clone();
        tokio::task::spawn_blocking(move || store.with(|c| c.execute("DELETE FROM cache_pins WHERE drive = ?1 AND etag = ?2", params![d, e]))).await??;
        self.state().pins.remove(&(Arc::from(drive), Arc::from(etag)));
        Ok(())
    }

    pub fn is_pinned(&self, drive: &str, etag: &str) -> bool {
        self.state().pins.contains(&(Arc::from(drive), Arc::from(etag)))
    }

    /// Evicts to the cap and the floor now, as a fill would (for the daemon to call when the
    /// volume fills up from elsewhere).
    pub async fn trim(&self) -> Result<()> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.make_room(0)).await??;
        Ok(())
    }

    /// Removes everything but pinned content, from memory and disk (SpaceFS's Clear Cache).
    pub async fn clear(&self) -> Result<()> {
        let victims: Vec<BlockKey> = {
            let mut st = self.state();
            st.mem = Lru::new();
            st.mem_bytes = 0;
            let keys: Vec<BlockKey> = st.disk.oldest().filter(|k| !st.pins.contains(&k.content())).cloned().collect();
            for k in &keys {
                if let Some(e) = st.disk.remove(k) {
                    st.disk_bytes -= e.len;
                }
            }
            keys
        };
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.delete_files(&victims)).await?
    }

    /// Waits until every fill that has started has also been written to disk (or skipped), and
    /// has let go of the cache.
    pub async fn settle(&self) {
        loop {
            let n = self.0.busy.settled.notified();
            if self.0.busy.n.load(Ordering::SeqCst) == 0 {
                return;
            }
            n.await;
        }
    }

    pub fn usage(&self) -> Usage {
        let st = self.state();
        let s = &self.0.stats;
        Usage {
            disk_bytes: st.disk_bytes,
            disk_blocks: st.disk.map.len() as u64,
            pinned_bytes: st.disk.map.iter().filter(|(k, _)| st.pins.contains(&k.content())).map(|(_, (_, e))| e.len).sum(),
            memory_bytes: st.mem_bytes,
            fetches: s.fetches.load(Ordering::Relaxed),
            fetched_bytes: s.fetched_bytes.load(Ordering::Relaxed),
            memory_hits: s.memory_hits.load(Ordering::Relaxed),
            disk_hits: s.disk_hits.load(Ordering::Relaxed),
            dropped: s.dropped.load(Ordering::Relaxed),
            evicted: s.evicted.load(Ordering::Relaxed),
        }
    }

    /// Forgets the memory tier (tests use it to read from disk).
    pub fn drop_memory(&self) {
        let mut st = self.state();
        st.mem = Lru::new();
        st.mem_bytes = 0;
    }
}

/// Loads the index, oldest first, keeping rows whose file is there and the right size, and
/// removes files no row lists.
fn load(store: &Store, dir: &Path) -> Result<(Lru<DiskEntry>, u64, Pins)> {
    std::fs::create_dir_all(dir)?;
    type Row = (String, String, i64, i64, Vec<u8>, i64);
    let rows: Vec<Row> = store.with(|c| {
        let mut q = c.prepare("SELECT drive, etag, block, len, sums, last_used FROM cache_blocks ORDER BY last_used")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect()
    })?;
    let pins: Vec<(String, String)> = store.with(|c| {
        let mut q = c.prepare("SELECT drive, etag FROM cache_pins")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    })?;
    let mut lru = Lru::new();
    let mut bytes = 0;
    let mut stale = Vec::new();
    let mut keep = HashSet::new();
    for (drive, etag, block, len, sums, used) in rows {
        let key = BlockKey { drive: drive.into(), etag: etag.into(), index: block as u64 };
        let path = key.path(dir);
        let ok = std::fs::metadata(&path).is_ok_and(|m| m.len() == len as u64);
        match sums_from_blob(&sums).filter(|_| ok) {
            Some(sums) => {
                keep.insert(path);
                bytes += len as u64;
                lru.insert(key, DiskEntry { len: len as u64, sums, stored_used: used });
            }
            None => stale.push(key),
        }
    }
    if !stale.is_empty() {
        store.with(|c| {
            let tx = c.transaction()?;
            for k in &stale {
                tx.execute("DELETE FROM cache_blocks WHERE drive = ?1 AND etag = ?2 AND block = ?3", params![&*k.drive, &*k.etag, k.index as i64])?;
            }
            tx.commit()
        })?;
    }
    for sub in std::fs::read_dir(dir)? {
        let sub = sub?.path();
        if !sub.is_dir() {
            std::fs::remove_file(&sub)?;
            continue;
        }
        for f in std::fs::read_dir(&sub)? {
            let f = f?.path();
            if !keep.contains(&f) {
                std::fs::remove_file(&f)?;
            }
        }
    }
    let pins = pins.into_iter().map(|(d, e)| (Arc::from(d), Arc::from(e))).collect();
    Ok((lru, bytes, pins))
}

/// Reads one content version, fetching ahead while it is read forward: SpaceFS's read-ahead, in
/// the client core, so that the mount gets it whichever adapter serves it.
///
/// A read is forward if it starts at or after the last one's start and no more than a block past
/// where the last one ended (the kernel's reads of one stream overlap and arrive a little out of
/// order). Each forward read doubles how far ahead the reader fetches, from one block up to
/// `read_ahead_bytes`; any other read starts again from nothing.
pub struct Reader {
    cache: Cache,
    content: Content,
    next: u64,
    last_start: u64,
    ahead: u64,
    /// Blocks before this have been asked for.
    requested_to: u64,
    started: bool,
}

impl Reader {
    pub fn content(&self) -> &Content {
        &self.content
    }

    pub async fn read(&mut self, offset: u64, len: u64) -> Result<Bytes> {
        let bs = self.cache.0.cfg.block_size;
        let forward = self.started && offset >= self.last_start && offset <= self.next.saturating_add(bs);
        if forward {
            self.ahead = (self.ahead * 2).clamp(bs, self.cache.0.cfg.read_ahead_bytes.max(bs));
        } else {
            self.ahead = 0;
            self.requested_to = 0;
        }
        self.started = true;
        self.last_start = offset;
        self.next = offset.saturating_add(len).min(self.content.size);
        if self.ahead > 0 {
            let blocks = self.content.size.div_ceil(bs);
            let from = (self.next / bs).max(self.requested_to);
            let until = self.next.saturating_add(self.ahead).div_ceil(bs).min(blocks);
            for i in from..until {
                self.cache.prefetch(&self.content, i);
            }
            self.requested_to = self.requested_to.max(until);
        }
        self.cache.read(&self.content, offset, len).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    const BLOCK: u64 = 256 * KIB;

    /// Bytes with no period, different for each seed: a piece read from the wrong place differs.
    fn bytes_of(seed: u64, n: u64) -> Bytes {
        (0..n).map(|i| (i.wrapping_add(seed << 40).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8).collect::<Vec<_>>().into()
    }

    /// Serves contents by ETag, logging every fetch; can be slowed down or told to fail.
    #[derive(Default)]
    struct Fake {
        data: Mutex<HashMap<String, Bytes>>,
        log: Mutex<Vec<(String, u64)>>,
        delay: Mutex<Duration>,
        fail: AtomicU32,
        now: AtomicU32,
        most: AtomicU32,
    }

    impl Fake {
        fn add(&self, etag: &str, b: Bytes) -> Content {
            let size = b.len() as u64;
            self.data.lock().unwrap().insert(etag.into(), b);
            Content { drive: "d".into(), key: format!("{etag}.bin"), version_id: format!("v-{etag}"), etag: etag.into(), size }
        }

        fn fetched(&self) -> Vec<(String, u64)> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Fetch for Fake {
        fn fetch<'a>(&'a self, c: &'a Content, offset: u64, len: u64) -> BoxFuture<'a, Result<Bytes>> {
            Box::pin(async move {
                let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(n, Ordering::SeqCst);
                let delay = *self.delay.lock().unwrap();
                tokio::time::sleep(delay).await;
                self.now.fetch_sub(1, Ordering::SeqCst);
                self.log.lock().unwrap().push((c.etag.clone(), offset / BLOCK));
                if self.fail.load(Ordering::SeqCst) > 0 {
                    self.fail.fetch_sub(1, Ordering::SeqCst);
                    return Err(Error::Invalid("injected".into()));
                }
                let all = self.data.lock().unwrap()[&c.etag].clone();
                Ok(all.slice(offset as usize..(offset + len) as usize))
            })
        }
    }

    fn plenty(_: &Path) -> io::Result<u64> {
        Ok(1 << 50)
    }

    fn config() -> CacheConfig {
        CacheConfig { block_size: BLOCK, max_bytes: 64 * BLOCK, min_free_bytes: 0, memory_bytes: 64 * BLOCK, read_ahead_bytes: 4 * BLOCK, free_space: Some(plenty), ..Default::default() }
    }

    async fn open(dir: &Path, fake: &Arc<Fake>, cfg: CacheConfig) -> Cache {
        Cache::open(Arc::new(Store::open(dir).unwrap()), fake.clone(), cfg).await.unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reads_return_the_contents_bytes_and_fetch_each_block_once() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let src = bytes_of(1, 10 * BLOCK + 1234);
        let c = fake.add("a", src.clone());
        let cache = open(dir.path(), &fake, config()).await;
        let ranges = [(0, 1), (5, 70_000), (BLOCK - 3, 10), (3 * BLOCK + 65_535, 3 * BLOCK), (10 * BLOCK, 5000), (0, 11 * BLOCK), (10 * BLOCK + 1233, 9)];
        for (off, len) in ranges {
            let got = cache.read(&c, off, len).await.unwrap();
            let end = (off + len).min(src.len() as u64);
            assert_eq!(got, src.slice(off as usize..end as usize), "{off}+{len}");
        }
        assert!(cache.read(&c, src.len() as u64, 10).await.unwrap().is_empty(), "past the end");
        assert!(cache.read(&c, 0, 0).await.unwrap().is_empty());
        let mut blocks: Vec<u64> = fake.fetched().into_iter().map(|(_, b)| b).collect();
        blocks.sort();
        assert_eq!(blocks, (0..11).collect::<Vec<_>>(), "every block once");
        cache.settle().await;
        let u = cache.usage();
        assert_eq!((u.disk_blocks, u.disk_bytes, u.fetches), (11, src.len() as u64, 11));
        // From memory, then from disk, still without fetching.
        cache.read(&c, 2 * BLOCK, 100).await.unwrap();
        cache.drop_memory();
        assert_eq!(cache.read(&c, 2 * BLOCK + 70_000, 100_000).await.unwrap(), src.slice((2 * BLOCK + 70_000) as usize..(2 * BLOCK + 170_000) as usize));
        let u = cache.usage();
        assert_eq!((u.fetches, u.memory_hits > 0, u.disk_hits), (11, true, 1));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn readers_of_one_block_share_one_fetch_and_one_failure() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        *fake.delay.lock().unwrap() = Duration::from_millis(50);
        let c = fake.add("a", bytes_of(2, 3 * BLOCK));
        let cache = open(dir.path(), &fake, config()).await;
        let reads = (0..16).map(|i| cache.read(&c, BLOCK + i * 100, 50));
        let got = futures::future::try_join_all(reads).await.unwrap();
        assert!(got.iter().all(|g| g.len() == 50));
        assert_eq!(fake.fetched(), [("a".into(), 1)], "one fetch for sixteen readers");

        fake.fail.store(1, Ordering::SeqCst);
        let reads = (0..4).map(|_| cache.read(&c, 2 * BLOCK, 10));
        let results = futures::future::join_all(reads).await;
        assert!(results.iter().all(|r| r.is_err()), "every waiter gets the failure");
        assert_eq!(fake.fetched().len(), 2, "still one fetch");
        assert_eq!(cache.read(&c, 2 * BLOCK, 10).await.unwrap().len(), 10, "the next read tries again");
        assert_eq!(fake.fetched().len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_cap_evicts_what_was_used_longest_ago_but_never_pinned_content() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let a = fake.add("a", bytes_of(3, 4 * BLOCK));
        let p = fake.add("p", bytes_of(4, BLOCK));
        let cfg = CacheConfig { max_bytes: 3 * BLOCK, ..config() };
        let cache = open(dir.path(), &fake, cfg).await;
        cache.pin("d", "p").await.unwrap();
        cache.read(&p, 0, 1).await.unwrap();
        cache.settle().await;
        for i in 0..2 {
            cache.read(&a, i * BLOCK, 1).await.unwrap();
            cache.settle().await;
        }
        assert_eq!(cache.usage().disk_blocks, 3);
        cache.read(&a, 0, 1).await.unwrap(); // block 0 is now more recent than block 1
        cache.read(&a, 2 * BLOCK, 1).await.unwrap();
        cache.settle().await;
        let u = cache.usage();
        assert_eq!((u.disk_blocks, u.disk_bytes, u.pinned_bytes, u.evicted), (3, 3 * BLOCK, BLOCK, 1));
        cache.drop_memory();
        let before = fake.fetched().len();
        for (c, off) in [(&a, 0), (&a, 2 * BLOCK), (&p, 0)] {
            cache.read(c, off, 1).await.unwrap();
        }
        assert_eq!(fake.fetched().len(), before, "the pinned block and the two most recent are on disk");
        cache.read(&a, BLOCK, 1).await.unwrap();
        assert_eq!(fake.fetched().len(), before + 1, "block 1, used longest ago, went");

        // Clearing keeps pinned content; unpinned, it can go.
        cache.settle().await;
        cache.clear().await.unwrap();
        assert_eq!(cache.usage().disk_bytes, BLOCK);
        cache.unpin("d", "p").await.unwrap();
        cache.clear().await.unwrap();
        assert_eq!(cache.usage().disk_bytes, 0);
        assert!(!cache.is_pinned("d", "p"));
    }

    static FREE: AtomicU64 = AtomicU64::new(0);

    fn free(_: &Path) -> io::Result<u64> {
        Ok(FREE.load(Ordering::SeqCst))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn below_the_free_space_floor_it_evicts_then_stops_filling() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let src = bytes_of(5, 6 * BLOCK);
        let a = fake.add("a", src.clone());
        let floor = 100 * BLOCK;
        FREE.store(floor + 50 * BLOCK, Ordering::SeqCst);
        let cfg = CacheConfig { min_free_bytes: floor, free_space: Some(free), ..config() };
        let cache = open(dir.path(), &fake, cfg).await;
        for i in 0..3 {
            cache.read(&a, i * BLOCK, 1).await.unwrap();
        }
        cache.settle().await;
        assert_eq!(cache.usage().disk_blocks, 3);
        // The volume fills up from elsewhere: half a block above the floor.
        FREE.store(floor + BLOCK / 2, Ordering::SeqCst);
        cache.read(&a, 3 * BLOCK, 1).await.unwrap();
        cache.settle().await;
        assert_eq!((cache.usage().disk_blocks, cache.usage().evicted), (3, 1), "one evicted to fit one");
        // Now below the floor: a trim evicts everything it may.
        FREE.store(floor - BLOCK, Ordering::SeqCst);
        cache.pin("d", "a").await.unwrap();
        cache.trim().await.unwrap();
        assert_eq!(cache.usage().disk_blocks, 3, "pinned blocks stay");
        cache.read(&a, 4 * BLOCK, BLOCK).await.unwrap();
        cache.settle().await;
        assert_eq!(cache.usage().disk_blocks, 3, "no room and nothing to evict: not stored");
        cache.drop_memory();
        assert_eq!(cache.read(&a, 4 * BLOCK + 7, 9).await.unwrap(), src.slice((4 * BLOCK + 7) as usize..(4 * BLOCK + 16) as usize), "but read all the same");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_block_file_that_fails_its_checksum_or_is_gone_is_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let src = bytes_of(6, 2 * BLOCK);
        let a = fake.add("a", src.clone());
        let cache = open(dir.path(), &fake, config()).await;
        cache.read(&a, 0, 2 * BLOCK).await.unwrap();
        cache.settle().await;
        cache.drop_memory();
        let path = BlockKey::of(&a, 0).path(&dir.path().join("cache"));
        let mut raw = std::fs::read(&path).unwrap();
        raw[100_000] ^= 1;
        std::fs::write(&path, raw).unwrap();
        assert_eq!(cache.read(&a, 99_990, 20).await.unwrap(), src.slice(99_990..100_010));
        assert_eq!((cache.usage().dropped, fake.fetched().len()), (1, 3));
        std::fs::remove_file(BlockKey::of(&a, 1).path(&dir.path().join("cache"))).unwrap();
        assert_eq!(cache.read(&a, BLOCK + 5, 5).await.unwrap(), src.slice((BLOCK + 5) as usize..(BLOCK + 10) as usize));
        assert_eq!((cache.usage().dropped, fake.fetched().len()), (2, 4));
        // A corrupt piece the read doesn't touch isn't noticed until a read does.
        cache.settle().await;
        cache.drop_memory();
        let path = BlockKey::of(&a, 0).path(&dir.path().join("cache"));
        let mut raw = std::fs::read(&path).unwrap();
        raw[BLOCK as usize - 1] ^= 1;
        std::fs::write(&path, raw).unwrap();
        cache.read(&a, 0, 10).await.unwrap();
        assert_eq!(fake.fetched().len(), 4);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_index_and_pins_survive_a_restart_and_strays_go() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let a = fake.add("a", bytes_of(7, 3 * BLOCK));
        {
            let cache = open(dir.path(), &fake, config()).await;
            cache.read(&a, 0, 3 * BLOCK).await.unwrap();
            cache.pin("d", "a").await.unwrap();
            cache.settle().await;
        }
        let cdir = dir.path().join("cache");
        std::fs::remove_file(BlockKey::of(&a, 2).path(&cdir)).unwrap();
        std::fs::create_dir_all(cdir.join("zz")).unwrap();
        std::fs::write(cdir.join("zz/stray"), b"x").unwrap();
        std::fs::write(BlockKey::of(&a, 0).path(&cdir).with_extension("tmp"), b"half").unwrap();
        let cache = open(dir.path(), &fake, config()).await;
        let u = cache.usage();
        assert_eq!((u.disk_blocks, u.pinned_bytes), (2, 2 * BLOCK));
        assert!(cache.is_pinned("d", "a"));
        assert!(!cdir.join("zz/stray").exists() && !BlockKey::of(&a, 0).path(&cdir).with_extension("tmp").exists());
        let before = fake.fetched().len();
        cache.read(&a, 0, 2 * BLOCK).await.unwrap();
        assert_eq!(fake.fetched().len(), before, "both blocks from disk");
        cache.read(&a, 2 * BLOCK, 1).await.unwrap();
        assert_eq!(fake.fetched().len(), before + 1, "the missing one fetched");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_forward_reader_fetches_ahead_and_overlaps_its_fetches() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        *fake.delay.lock().unwrap() = Duration::from_millis(20);
        let src = bytes_of(8, 16 * BLOCK);
        let a = fake.add("a", src.clone());
        let cache = open(dir.path(), &fake, config()).await;
        let mut r = cache.reader(a.clone());
        let step = 64 * KIB;
        let mut off = 0;
        while off < src.len() as u64 {
            assert_eq!(r.read(off, step).await.unwrap(), src.slice(off as usize..(off + step) as usize));
            off += step;
        }
        let blocks: Vec<u64> = fake.fetched().into_iter().map(|(_, b)| b).collect();
        assert_eq!(blocks.len(), 16, "each block once: {blocks:?}");
        assert!(fake.most.load(Ordering::SeqCst) >= 3, "fetches overlapped: at most {} at once", fake.most.load(Ordering::SeqCst));
        let hits = cache.usage().memory_hits;
        assert!(hits >= 48, "most of the 64 reads found their block already there: {hits}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_reader_that_jumps_about_fetches_only_what_it_reads() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let a = fake.add("a", bytes_of(9, 16 * BLOCK));
        let cache = open(dir.path(), &fake, config()).await;
        let mut r = cache.reader(a);
        for b in [9, 2, 14, 5, 0, 11] {
            r.read(b * BLOCK + 10, 100).await.unwrap();
        }
        cache.settle().await;
        let mut blocks: Vec<u64> = fake.fetched().into_iter().map(|(_, b)| b).collect();
        blocks.sort();
        assert_eq!(blocks, [0, 2, 5, 9, 11, 14]);
        // Forward again: it reads ahead from where it is.
        r.read(11 * BLOCK + 200, 100).await.unwrap();
        r.read(11 * BLOCK + 300, 100).await.unwrap();
        cache.settle().await;
        let fetched: Vec<u64> = fake.fetched().into_iter().map(|(_, b)| b).collect();
        assert!(fetched.contains(&12), "{fetched:?}");
    }
}
