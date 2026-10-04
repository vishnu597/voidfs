// SPDX-License-Identifier: Apache-2.0
//! Cold reads through the block cache, through the API against straight from the bucket with
//! storage credentials (step 4, item 6; protocol §5.5), against a running server.
//!
//!   VOIDFS_ENDPOINT=… VOIDFS_ACCESS_KEY_ID=… VOIDFS_SECRET_ACCESS_KEY=… \
//!   cargo run --release -p voidfs-client --example bucketread -- --drive bench [--key media/read.bin]
//!       [--size-mib 64] [--rounds 3] [--server-pid <pid>]
//!
//! The file is written first if the drive doesn't have it at that size: random bytes. Each round
//! reads it twice, once each way, alternating which goes first (A B, then B A). Each read starts
//! cold: a new, empty cache and fetcher, and, with `--server-pid`, the server's caches dropped
//! (SIGUSR1). A read is the first 4 KiB, then the whole file 1 MiB at a time, each read waiting
//! for the one before (as Finder's copy reads), with the cache's read-ahead; then the first 4 KiB
//! of a second file of 8 MiB, with the same fetcher and cache, as the next file a mount opens in a
//! drive it reads already. Each read is printed as one JSON line: the times to the first 4 KiB, to
//! the whole file, and to the next file's first 4 KiB, and what the fetcher did (from the bucket
//! or through the API, credentials asked for, shards and their bytes).

use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use voidfs_client::{ApiFetcher, BucketConfig, BucketFetcher, Cache, CacheConfig, Content, Fetch, Store};
use voidfs_sdk::{Client, ReadOptions};

struct Args {
    drive: String,
    key: String,
    size: u64,
    rounds: usize,
    server_pid: Option<u32>,
}

fn args() -> Args {
    let mut a = Args { drive: String::new(), key: "media/read.bin".into(), size: 64 << 20, rounds: 3, server_pid: None };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--drive" => a.drive = value(),
            "--key" => a.key = value(),
            "--size-mib" => a.size = value().parse::<u64>().expect("--size-mib") << 20,
            "--rounds" => a.rounds = value().parse().expect("--rounds"),
            "--server-pid" => a.server_pid = Some(value().parse().expect("--server-pid")),
            other => panic!("unknown argument {other}"),
        }
    }
    assert!(!a.drive.is_empty(), "--drive is required");
    a
}

/// Random bytes from a seed, without a dependency (xorshift64*).
fn random_bytes(n: u64, seed: u64) -> Bytes {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed;
    let mut out = Vec::with_capacity(n as usize + 8);
    while (out.len() as u64) < n {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        out.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    out.truncate(n as usize);
    out.into()
}

fn drop_server_caches(pid: Option<u32>) {
    if let Some(pid) = pid {
        let ok = std::process::Command::new("kill").args(["-USR1", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "could not signal the server ({pid})");
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[tokio::main]
async fn main() {
    let a = args();
    let client = Client::from_env().expect("VOIDFS_ENDPOINT, VOIDFS_ACCESS_KEY_ID and VOIDFS_SECRET_ACCESS_KEY");
    if client.describe_drive(&a.drive).await.is_err() {
        client.create_drive(&a.drive, Default::default()).await.expect("create the drive");
    }
    let mut files = Vec::new();
    for (key, size, seed) in [(a.key.clone(), a.size, 1), (format!("{}.next", a.key), 8 << 20, 2)] {
        let meta = match client.head_object(&a.drive, &key, ReadOptions::default()).await {
            Ok(m) if m.size == size => m,
            _ => {
                client.put_object(&a.drive, &key, random_bytes(size, seed), Default::default()).await.expect("write the file");
                client.head_object(&a.drive, &key, ReadOptions::default()).await.expect("head the file")
            }
        };
        files.push(Content::of(&a.drive, &key, &meta));
    }
    let (content, next) = (files[0].clone(), files[1].clone());
    let cfg = CacheConfig { min_free_bytes: 0, ..CacheConfig::default() };
    for round in 0..a.rounds {
        let order = if round % 2 == 0 { ["api", "bucket"] } else { ["bucket", "api"] };
        for way in order {
            drop_server_caches(a.server_pid);
            let dir = tempfile::tempdir().expect("a state directory");
            let bucket = Arc::new(BucketFetcher::new(client.clone(), BucketConfig::default()));
            let fetcher: Arc<dyn Fetch> = if way == "bucket" { bucket.clone() } else { Arc::new(ApiFetcher::new(client.clone())) };
            let cache = Cache::open(Arc::new(Store::open(dir.path()).expect("state")), fetcher, cfg.clone()).await.expect("cache");
            let started = Instant::now();
            let first = cache.read(&content, 0, 4096).await.expect("the first 4 KiB");
            let first_ms = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(first.len(), 4096);
            let mut reader = cache.reader(content.clone());
            let mut off = 0;
            while off < a.size {
                let n = (1 << 20).min(a.size - off);
                let got = reader.read(off, n).await.expect("a read");
                assert_eq!(got.len() as u64, n);
                off += n;
            }
            let all_ms = started.elapsed().as_secs_f64() * 1000.0;
            let t = Instant::now();
            assert_eq!(cache.read(&next, 0, 4096).await.expect("the next file's first 4 KiB").len(), 4096);
            let next_ms = t.elapsed().as_secs_f64() * 1000.0;
            let u = bucket.usage();
            println!(
                "{}",
                serde_json::json!({
                    "round": round, "way": way, "size_mib": a.size >> 20,
                    "first_4k_ms": (first_ms * 10.0).round() / 10.0, "whole_ms": all_ms.round(), "next_first_4k_ms": (next_ms * 10.0).round() / 10.0,
                    "mb_per_s": ((a.size as f64 / 1e6) / (all_ms / 1000.0) * 10.0).round() / 10.0,
                    "bucket_reads": u.bucket_reads, "api_reads": u.api_reads, "credentials": u.credentials,
                    "shards": u.shards, "shard_mb": (u.shard_bytes as f64 / 1e6 * 10.0).round() / 10.0,
                })
            );
            cache.settle().await;
        }
    }
}
