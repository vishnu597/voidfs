// SPDX-License-Identifier: Apache-2.0
//! The block cache reading straight from the bucket with storage credentials (protocol §5.5),
//! against a server in this process and the stand-in bucket that mints and enforces them.

mod common;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use common::client_for;
use voidfs_client::{BucketConfig, BucketFetcher, Cache, CacheConfig, Content, Error, Store};
use voidfs_sdk::{Client, Config, ReadOptions};
use voidfs_server::test_server::{Rules, TestServer};

const MIB: u64 = 1 << 20;

/// Random-looking bytes, different for each seed, which content-defined chunking cuts into shards
/// of about 2 MiB (format §4.1).
fn bytes_of(seed: u64, n: u64) -> Bytes {
    use sha2::Digest;
    let mut out = Vec::with_capacity(n as usize + 32);
    let mut i = 0u64;
    while (out.len() as u64) < n {
        out.extend_from_slice(&sha2::Sha256::digest([seed.to_be_bytes(), i.to_be_bytes()].concat()));
        i += 1;
    }
    out.truncate(n as usize);
    out.into()
}

fn plenty(_: &std::path::Path) -> std::io::Result<u64> {
    Ok(1 << 50)
}

fn config() -> CacheConfig {
    CacheConfig { block_size: MIB, max_bytes: 64 * MIB, min_free_bytes: 0, memory_bytes: 16 * MIB, read_ahead_bytes: 4 * MIB, free_space: Some(plenty), ..Default::default() }
}

async fn setup(rules: Option<Rules>) -> (TestServer, Client) {
    let s = match rules {
        Some(r) => TestServer::with_storage_credentials(Vec::new(), r).await.unwrap(),
        None => TestServer::start().await.unwrap(),
    };
    let c = client_for(&s.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    (s, c)
}

async fn cache_with(dir: &std::path::Path, fetcher: Arc<BucketFetcher>) -> Cache {
    Cache::open(Arc::new(Store::open(dir).unwrap()), fetcher, config()).await.unwrap()
}

async fn content(c: &Client, key: &str) -> Content {
    Content::of("drv", key, &c.head_object("drv", key, ReadOptions::default()).await.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_come_from_the_bucket() {
    let (s, c) = setup(Some(Rules::default())).await;
    let bucket = s.bucket.clone().unwrap();
    // The server's check at start was refused what it tried that the credentials don't reach.
    let refused = bucket.credential_use().refused;
    // Several shards, a file of one, and an empty file.
    let big = bytes_of(1, 9 * MIB + 12_345);
    c.put_object("drv", "dir/big.bin", big.clone(), Default::default()).await.unwrap();
    let small = bytes_of(2, 100);
    c.put_object("drv", "small.txt", small.clone(), Default::default()).await.unwrap();
    c.put_object("drv", "empty", Bytes::new(), Default::default()).await.unwrap();
    let fetcher = Arc::new(BucketFetcher::new(c.clone(), BucketConfig::default()));
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_with(dir.path(), fetcher.clone()).await;
    let f = content(&c, "dir/big.bin").await;
    for (off, len) in [(0, 10), (MIB - 5, 10), (2 * MIB + 12_345, MIB), (0, big.len() as u64), (big.len() as u64 - 7, 7)] {
        assert_eq!(cache.read(&f, off, len).await.unwrap(), big.slice(off as usize..(off + len) as usize), "{off}+{len}");
    }
    assert_eq!(cache.read(&content(&c, "small.txt").await, 0, 100).await.unwrap(), small);
    assert_eq!(cache.read(&content(&c, "empty").await, 0, 0).await.unwrap(), Bytes::new());
    let u = fetcher.usage();
    assert_eq!((u.api_reads, u.credentials, u.loads), (0, 1, 1), "{u:?}");
    assert!(u.bucket_reads >= 10 && u.shards >= 4, "{u:?}");
    assert!(bucket.credential_use().reads > u.shards, "the drive's state too: {:?}", bucket.credential_use());
    assert_eq!(bucket.credential_use().refused, refused, "nothing refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn versions_newer_than_the_state_and_older_ones_are_read_from_the_bucket() {
    let (_s, c) = setup(Some(Rules::default())).await;
    let v1 = bytes_of(3, 2 * MIB + 3);
    c.put_object("drv", "a.bin", v1.clone(), Default::default()).await.unwrap();
    let old = content(&c, "a.bin").await;
    let fetcher = Arc::new(BucketFetcher::new(c.clone(), BucketConfig::default()));
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_with(dir.path(), fetcher.clone()).await;
    assert_eq!(cache.read(&old, 0, 10).await.unwrap(), v1.slice(..10));
    // Written after the fetcher read the drive's state: it reads the log after it.
    let v2 = bytes_of(4, 3 * MIB);
    c.put_object("drv", "a.bin", v2.clone(), Default::default()).await.unwrap();
    c.put_object("drv", "new.bin", bytes_of(5, MIB), Default::default()).await.unwrap();
    let new = content(&c, "a.bin").await;
    assert_eq!(cache.read(&new, MIB, MIB).await.unwrap(), v2.slice(MIB as usize..2 * MIB as usize));
    assert_eq!(cache.read(&content(&c, "new.bin").await, 0, MIB).await.unwrap(), bytes_of(5, MIB));
    // Without a version id, the current content, checked against the ETag (a block the cache
    // doesn't have).
    let current = Content { version_id: String::new(), etag: old.etag.clone(), ..old.clone() };
    let e = cache.read(&current, 2 * MIB, 1).await.unwrap_err();
    assert!(matches!(&e, Error::Changed { expected, .. } if *expected == old.etag), "{e}");
    assert_eq!(cache.read(&old, MIB, MIB + 3).await.unwrap(), v1.slice(MIB as usize..), "the old version, from the history");
    let u = fetcher.usage();
    assert_eq!((u.api_reads, u.loads), (0, 1), "{u:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_without_credentials_is_read_through_the_api() {
    let (_s, c) = setup(None).await;
    let src = bytes_of(6, 2 * MIB);
    c.put_object("drv", "a.bin", src.clone(), Default::default()).await.unwrap();
    let fetcher = Arc::new(BucketFetcher::new(c.clone(), BucketConfig::default()));
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_with(dir.path(), fetcher.clone()).await;
    let f = content(&c, "a.bin").await;
    assert_eq!(cache.read(&f, 0, 2 * MIB).await.unwrap(), src);
    let u = fetcher.usage();
    assert_eq!((u.bucket_reads, u.credentials), (0, 1), "asked once, then read through the API: {u:?}");
    assert_eq!(u.api_reads, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn credentials_are_renewed_before_they_expire() {
    let (s, c) = setup(Some(Rules { lifetime: Some(Duration::from_secs(3)), ..Rules::default() })).await;
    let bucket = s.bucket.clone().unwrap();
    let refused = bucket.credential_use().refused;
    let src = bytes_of(7, 4 * MIB);
    c.put_object("drv", "a.bin", src.clone(), Default::default()).await.unwrap();
    let fetcher = Arc::new(BucketFetcher::new(c.clone(), BucketConfig { renew_before: Duration::from_millis(1500), ..BucketConfig::default() }));
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_with(dir.path(), fetcher.clone()).await;
    let f = content(&c, "a.bin").await;
    assert_eq!(cache.read(&f, 0, 10).await.unwrap(), src.slice(..10));
    // With 1.5 s or less left, they are asked for again before the next read uses them.
    tokio::time::sleep(Duration::from_millis(1700)).await;
    assert_eq!(cache.read(&f, 3 * MIB, 10).await.unwrap(), src.slice(3 * MIB as usize..3 * MIB as usize + 10));
    let u = fetcher.usage();
    assert_eq!((u.credentials, u.loads, u.api_reads), (2, 1, 0), "renewed, the same generation: {u:?}");
    assert_eq!(bucket.credential_use().refused, refused, "no request with credentials about to expire");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn credentials_the_bucket_refuses_are_asked_for_again() {
    // Credentials of a minute, which the server mints again for each answer (less than half of
    // its own lifetime left) and the client keeps.
    let (s, c) = setup(Some(Rules { lifetime: Some(Duration::from_secs(60)), ..Rules::default() })).await;
    let bucket = s.bucket.clone().unwrap();
    let refused = bucket.credential_use().refused;
    let src = bytes_of(8, 4 * MIB);
    c.put_object("drv", "a.bin", src.clone(), Default::default()).await.unwrap();
    let fetcher = Arc::new(BucketFetcher::new(c.clone(), BucketConfig { renew_before: Duration::from_secs(1), ..BucketConfig::default() }));
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_with(dir.path(), fetcher.clone()).await;
    let f = content(&c, "a.bin").await;
    assert_eq!(cache.read(&f, 0, 10).await.unwrap(), src.slice(..10));
    bucket.revoke();
    assert_eq!(cache.read(&f, 3 * MIB, 10).await.unwrap(), src.slice(3 * MIB as usize..3 * MIB as usize + 10));
    let u = fetcher.usage();
    assert_eq!((u.credentials, u.api_reads), (2, 0), "{u:?}");
    assert_eq!(bucket.credential_use().refused, refused + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shard_that_does_not_match_its_hash_is_read_through_the_api() {
    let (s, c) = setup(Some(Rules::default())).await;
    let src = bytes_of(9, 3 * MIB);
    c.put_object("drv", "a.bin", src.clone(), Default::default()).await.unwrap();
    // Every shard in the bucket replaced by other bytes; the server still has them in its cache.
    for l in s.pool.store.list_recursive("shards/").await.unwrap() {
        let path = format!("shards/{}", l.name);
        let mut b = s.pool.store.get(&path).await.unwrap().unwrap().to_vec();
        b[0] ^= 0xff;
        s.pool.store.put(&path, b.into()).await.unwrap();
    }
    let fetcher = Arc::new(BucketFetcher::new(c.clone(), BucketConfig::default()));
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_with(dir.path(), fetcher.clone()).await;
    let f = content(&c, "a.bin").await;
    assert_eq!(cache.read(&f, 0, 3 * MIB).await.unwrap(), src, "never the bucket's wrong bytes");
    let u = fetcher.usage();
    assert_eq!((u.bucket_reads, u.api_reads), (0, 3), "{u:?}");
}
