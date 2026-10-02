// SPDX-License-Identifier: Apache-2.0
//! The block cache reading through the API, against a server in this process.

mod common;

use std::sync::Arc;

use bytes::Bytes;
use common::{Fault, Proxy, client_for, server};
use voidfs_client::{ApiFetcher, Cache, CacheConfig, Content, Error, Store};
use voidfs_sdk::{Client, Config, ReadOptions};

const MIB: u64 = 1 << 20;

/// Bytes with no period, different for each seed: a range read from the wrong place differs.
fn bytes_of(seed: u64, n: u64) -> Bytes {
    (0..n).map(|i| (i.wrapping_add(seed << 40).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8).collect::<Vec<_>>().into()
}

fn plenty(_: &std::path::Path) -> std::io::Result<u64> {
    Ok(1 << 50)
}

fn config() -> CacheConfig {
    CacheConfig { block_size: MIB, max_bytes: 64 * MIB, min_free_bytes: 0, memory_bytes: 16 * MIB, read_ahead_bytes: 4 * MIB, free_space: Some(plenty), ..Default::default() }
}

async fn cache(dir: &std::path::Path, client: &Client) -> Cache {
    Cache::open(Arc::new(Store::open(dir).unwrap()), Arc::new(ApiFetcher::new(client.clone())), config()).await.unwrap()
}

/// The current version of `key`, as the server describes it (the proxy drops a HEAD's length).
async fn content(c: &Client, key: &str) -> Content {
    Content::of("drv", key, &c.head_object("drv", key, ReadOptions::default()).await.unwrap())
}

fn gets(p: &Proxy) -> usize {
    p.seen().iter().filter(|r| r.starts_with("GET ") && r.contains("versionId=")).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_through_the_api_are_the_files_bytes_a_block_at_a_time() {
    let s = server().await;
    let proxy = Proxy::start(&s.endpoint).await;
    let c = client_for(&proxy.endpoint, Config::default());
    let direct = client_for(&s.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    let src = bytes_of(1, 3 * MIB + MIB / 2);
    c.put_object("drv", "dir/clip.mov", src.clone(), Default::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = cache(dir.path(), &c).await;
    let f = content(&direct, "dir/clip.mov").await;
    proxy.clear();
    for (off, len) in [(0, 10), (MIB - 5, 10), (2 * MIB + 12_345, MIB), (3 * MIB + 1, MIB), (0, 4 * MIB)] {
        let end = (off + len).min(src.len() as u64);
        assert_eq!(cache.read(&f, off, len).await.unwrap(), src.slice(off as usize..end as usize), "{off}+{len}");
    }
    assert_eq!(gets(&proxy), 4, "{:?}", proxy.seen());
    cache.settle().await;
    cache.drop_memory();
    cache.read(&f, 0, src.len() as u64).await.unwrap();
    assert_eq!(gets(&proxy), 4, "all from disk");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_version_reads_as_it_was_and_changed_content_is_an_error() {
    let s = server().await;
    let c = client_for(&s.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    let v1 = bytes_of(2, 2 * MIB + 3);
    c.put_object("drv", "a.bin", v1.clone(), Default::default()).await.unwrap();
    let old = content(&c, "a.bin").await;
    c.put_object("drv", "a.bin", bytes_of(3, 2 * MIB + 3), Default::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = cache(dir.path(), &c).await;
    assert_eq!(cache.read(&old, MIB, MIB + 3).await.unwrap(), v1.slice(MIB as usize..), "the version asked for");
    // Without a version id, the current content is read, and checked against the ETag.
    let current = Content { version_id: String::new(), etag: old.etag.clone(), ..old.clone() };
    let e = cache.read(&current, 0, 1).await.unwrap_err();
    assert!(matches!(&e, Error::Changed { expected, .. } if *expected == old.etag), "{e}");
    let gone = Content { version_id: "999.0".into(), ..old };
    match cache.read(&gone, 0, 1).await.unwrap_err() {
        Error::Fetch(e) => assert_eq!(e.code(), Some("NoSuchVersion")),
        e => panic!("{e}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_rides_out_what_the_sdk_retries_and_fails_after_it() {
    let s = server().await;
    let proxy = Proxy::start(&s.endpoint).await;
    let c = client_for(&proxy.endpoint, Config::default());
    let direct = client_for(&s.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    let src = bytes_of(4, 2 * MIB);
    c.put_object("drv", "b.bin", src.clone(), Default::default()).await.unwrap();
    let f = content(&direct, "b.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let cache = cache(dir.path(), &c).await;
    proxy.clear();
    proxy.fault(Fault::Status(503));
    assert_eq!(cache.read(&f, 10, 10).await.unwrap(), src.slice(10..20));
    assert_eq!((gets(&proxy), cache.usage().fetches), (2, 1), "retried once, one fetch");
    for _ in 0..3 {
        proxy.fault(Fault::Status(503));
    }
    match cache.read(&f, MIB, 10).await.unwrap_err() {
        Error::Fetch(e) => assert_eq!(e.status(), Some(503)),
        e => panic!("{e}"),
    }
    assert_eq!(cache.read(&f, MIB, 10).await.unwrap(), src.slice(MIB as usize..MIB as usize + 10), "the next read tries again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forward_reader_streams_a_file_fetching_each_block_once() {
    let s = server().await;
    let proxy = Proxy::start(&s.endpoint).await;
    let c = client_for(&proxy.endpoint, Config::default());
    let direct = client_for(&s.endpoint, Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    let src = bytes_of(5, 12 * MIB + 17);
    c.put_object("drv", "big.bin", src.clone(), Default::default()).await.unwrap();
    let f = content(&direct, "big.bin").await;
    let dir = tempfile::tempdir().unwrap();
    let cache = cache(dir.path(), &c).await;
    proxy.clear();
    let mut r = cache.reader(f);
    let mut off = 0;
    while off < src.len() as u64 {
        let got = r.read(off, 128 * 1024).await.unwrap();
        let end = (off as usize + 128 * 1024).min(src.len());
        assert!(got == src.slice(off as usize..end), "at {off}");
        off += 128 * 1024;
        if off == 2 * MIB {
            cache.settle().await;
            assert!(gets(&proxy) >= 4, "blocks ahead of the reader were fetched: {:?}", proxy.seen());
        }
    }
    assert_eq!(gets(&proxy), 13, "{:?}", proxy.seen());
    assert!(cache.usage().memory_hits > 80, "{:?}", cache.usage());
}
