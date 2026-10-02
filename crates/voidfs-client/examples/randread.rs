// SPDX-License-Identifier: Apache-2.0
//! Reads through the block cache against a running server, as the Mac head-to-head read through
//! the two apps (bench/results/mac-head-to-head): 300 random 4 KiB reads of a 1 GiB file of
//! random bytes, and the file read once from start to end, 1 MiB at a time with each read
//! waiting for the one before (as Finder's copy reads).
//!
//!   VOIDFS_ENDPOINT=… VOIDFS_ACCESS_KEY_ID=… VOIDFS_SECRET_ACCESS_KEY=… \
//!   cargo run --release -p voidfs-client --example randread -- --drive bench [--key media/big.bin]
//!       [--size-mib 1024] [--reads 300] [--seed 1] [--server-pid <pid>] [--state <dir>]
//!
//! Each pass is printed as one JSON line. `direct` reads straight through the SDK, one request a
//! read, as the spike's mount did; `cold` reads through a new, empty cache; `warm` repeats the
//! cold pass's reads with that cache. With `--server-pid`, the server's caches are dropped
//! (SIGUSR1) before each pass that isn't `warm`, so that its reads reach the bucket.

use std::sync::Arc;
use std::time::Instant;

use voidfs_client::{ApiFetcher, Cache, CacheConfig, Content, Store};
use voidfs_sdk::{Client, ReadOptions};

struct Args {
    drive: String,
    key: String,
    size: u64,
    reads: usize,
    seed: u64,
    server_pid: Option<u32>,
    state: Option<std::path::PathBuf>,
}

fn args() -> Args {
    let mut a = Args { drive: String::new(), key: "media/big.bin".into(), size: 1024 << 20, reads: 300, seed: 1, server_pid: None, state: None };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--drive" => a.drive = value(),
            "--key" => a.key = value(),
            "--size-mib" => a.size = value().parse::<u64>().expect("--size-mib") << 20,
            "--reads" => a.reads = value().parse().expect("--reads"),
            "--seed" => a.seed = value().parse().expect("--seed"),
            "--server-pid" => a.server_pid = Some(value().parse().expect("--server-pid")),
            "--state" => a.state = Some(value().into()),
            other => panic!("unknown argument {other}"),
        }
    }
    assert!(!a.drive.is_empty(), "--drive is required");
    a
}

/// xorshift64*, so that runs are repeatable without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn drop_server_caches(pid: Option<u32>) {
    if let Some(pid) = pid {
        let ok = std::process::Command::new("kill").args(["-USR1", &pid.to_string()]).status().is_ok_and(|s| s.success());
        assert!(ok, "could not signal the server");
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

fn report(pass: &str, mut ms: Vec<f64>, extra: String) {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "{{\"pass\":\"{pass}\",\"reads\":{},\"p50_ms\":{:.3},\"p90_ms\":{:.3},\"p99_ms\":{:.3},\"max_ms\":{:.3}{extra}}}",
        ms.len(),
        percentile(&ms, 0.5),
        percentile(&ms, 0.9),
        percentile(&ms, 0.99),
        ms[ms.len() - 1]
    );
}

#[tokio::main]
async fn main() {
    let a = args();
    let client = Client::from_env().expect("VOIDFS_ENDPOINT and a key");
    let head = match client.head_object(&a.drive, &a.key, ReadOptions::default()).await {
        Ok(m) if m.size == a.size => m,
        _ => {
            let mut rng = Rng(a.seed ^ 0x9E37_79B9_7F4A_7C15);
            let body: Vec<u8> = (0..a.size / 8).flat_map(|_| rng.next().to_le_bytes()).collect();
            let _ = client.create_drive(&a.drive, Default::default()).await;
            client.put_object(&a.drive, &a.key, body, Default::default()).await.expect("put the test file");
            client.head_object(&a.drive, &a.key, ReadOptions::default()).await.expect("head")
        }
    };
    let content = Content::of(&a.drive, &a.key, &head);
    let mut rng = Rng(a.seed);
    let offsets: Vec<u64> = (0..a.reads).map(|_| rng.next() % (a.size / 4096) * 4096).collect();
    let opts = ReadOptions { version_id: Some(head.version_id.clone()), ..Default::default() };

    // Direct: one ranged GET a read.
    drop_server_caches(a.server_pid);
    let mut ms = Vec::new();
    for &off in &offsets {
        let t = Instant::now();
        let b = client.read_range(&a.drive, &a.key, off, Some(4096), opts.clone()).await.expect("read");
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(b.len(), 4096);
    }
    report("direct random 4 KiB", ms, String::new());

    // Cold, then warm: through a new cache.
    let tmp = tempfile_dir(a.state.as_deref());
    let store = Arc::new(Store::open(&tmp).expect("state"));
    let cache = Cache::open(store, Arc::new(ApiFetcher::new(client.clone())), CacheConfig::default()).await.expect("cache");
    drop_server_caches(a.server_pid);
    let mut ms = Vec::new();
    for &off in &offsets {
        let t = Instant::now();
        let b = cache.read(&content, off, 4096).await.expect("read");
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(b.len(), 4096);
    }
    let u = cache.usage();
    report("cold random 4 KiB", ms, format!(",\"fetches\":{},\"fetched_mib\":{}", u.fetches, u.fetched_bytes >> 20));
    cache.settle().await;
    let mut ms = Vec::new();
    for &off in &offsets {
        let t = Instant::now();
        cache.read(&content, off, 4096).await.expect("read");
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let u2 = cache.usage();
    report("warm random 4 KiB", ms, format!(",\"fetches\":{},\"memory_hits\":{},\"disk_hits\":{}", u2.fetches - u.fetches, u2.memory_hits - u.memory_hits, u2.disk_hits - u.disk_hits));

    // Sequential, one read at a time: directly, then through a reader of a new cache.
    drop_server_caches(a.server_pid);
    let t = Instant::now();
    let mut off = 0;
    let mut ms = Vec::new();
    while off < a.size {
        let r = Instant::now();
        client.read_range(&a.drive, &a.key, off, Some(1 << 20), opts.clone()).await.expect("read");
        ms.push(r.elapsed().as_secs_f64() * 1000.0);
        off += 1 << 20;
    }
    let secs = t.elapsed().as_secs_f64();
    report("direct sequential 1 MiB", ms, format!(",\"mb_per_s\":{:.1}", a.size as f64 / 1e6 / secs));
    cache.clear().await.expect("clear");
    drop_server_caches(a.server_pid);
    let mut reader = cache.reader(content.clone());
    let t = Instant::now();
    let mut off = 0;
    let mut ms = Vec::new();
    while off < a.size {
        let r = Instant::now();
        reader.read(off, 1 << 20).await.expect("read");
        ms.push(r.elapsed().as_secs_f64() * 1000.0);
        off += 1 << 20;
    }
    let secs = t.elapsed().as_secs_f64();
    report("cold sequential 1 MiB, read-ahead", ms, format!(",\"mb_per_s\":{:.1}", a.size as f64 / 1e6 / secs));
    cache.settle().await;
    if a.state.is_none() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// `--state`, or a new directory under the system's temporary directory.
fn tempfile_dir(state: Option<&std::path::Path>) -> std::path::PathBuf {
    match state {
        Some(s) => s.to_owned(),
        None => {
            let d = std::env::temp_dir().join(format!("voidfs-randread-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            d
        }
    }
}
