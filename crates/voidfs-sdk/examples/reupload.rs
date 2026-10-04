// SPDX-License-Identifier: Apache-2.0
//! Re-uploads of a large file changed in one place, as an ordinary put and as a direct upload
//! (protocol §4.11), alternating A B B A: the time each takes and the bytes the client sends.
//! With `--first-multipart`, the file is first uploaded in 16 MiB parts, and a direct upload's
//! plan says how much of it the pool holds then.
//!
//! The server and key come from `VOIDFS_ENDPOINT`, `VOIDFS_ACCESS_KEY_ID` and
//! `VOIDFS_SECRET_ACCESS_KEY`. One JSON line per upload, then a summary line.
//!
//!     cargo run --release -p voidfs-sdk --example reupload -- --size-mib 32 --rounds 6

use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use rand::{RngExt, SeedableRng};
use voidfs_sdk::{Bandwidth, Client, Config, PutOptions};

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() { f64::NAN } else { v[v.len() / 2] }
}

#[tokio::main]
async fn main() -> voidfs_sdk::Result<()> {
    let size = arg("--size-mib").map_or(32, |s| s.parse().unwrap()) << 20;
    let rounds: usize = arg("--rounds").map_or(4, |s| s.parse().unwrap());
    let first_multipart = std::env::args().any(|a| a == "--first-multipart");
    let bw = Arc::new(Bandwidth::new(None));
    let client = Client::new(Config { upload_bandwidth: Some(bw.clone()), ..Config::from_env()? })?;
    let drive = format!("reupload-{}", rand::rng().random_range(0..u32::MAX));
    client.create_drive(&drive, Default::default()).await?;
    let mut rng = rand::rngs::StdRng::seed_from_u64(arg("--seed").map_or(1, |s| s.parse().unwrap()));
    let mut cur: Vec<u8> = (0..size).map(|_| rng.random()).collect();
    if first_multipart {
        let id = client.create_multipart_upload(&drive, "f", PutOptions::default()).await?;
        let mut parts = Vec::new();
        for (i, part) in cur.chunks(16 << 20).enumerate() {
            let n = i as u32 + 1;
            parts.push((n, client.upload_part(&drive, "f", &id, n, Bytes::copy_from_slice(part)).await?));
        }
        client.complete_multipart_upload(&drive, "f", &id, &parts, Default::default()).await?;
        // The parts were cut at their boundaries: how much of the same content a plan holds.
        let shards = voidfs_sdk::direct::shards_of(&Bytes::from(cur.clone()));
        let plan = client.plan_upload(&drive, "f", &shards).await?;
        let listed: u64 = plan.upload.iter().map(|p| p.length).sum();
        println!("{}", serde_json::json!({ "after_multipart": { "shards": shards.len(), "held": plan.held, "bytes_to_send": listed } }));
    } else {
        client.put_object(&drive, "f", cur.clone(), PutOptions::default()).await?;
    }
    let (mut put_ms, mut put_bytes, mut direct_ms, mut direct_bytes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let order: Vec<bool> = (0..rounds).flat_map(|r| if r % 2 == 0 { [false, true] } else { [true, false] }).collect();
    for (i, direct) in order.into_iter().enumerate() {
        // One region of 4 KiB changes, somewhere in the middle half.
        let at = size / 4 + rng.random_range(0..size / 2 - 4096);
        rng.fill(&mut cur[at..at + 4096]);
        let body = Bytes::from(cur.clone());
        let (taken, started) = (bw.taken(), Instant::now());
        let (held, listed) = if direct {
            let shards = voidfs_sdk::direct::shards_of(&body);
            let plan = client.plan_upload(&drive, "f", &shards).await?;
            client.upload_planned(&plan, &body).await?;
            client.commit_upload(&drive, "f", &plan.token, &shards, &voidfs_core::ids::ShardHash::of(&body).to_hex(), PutOptions::default()).await?;
            (plan.held, shards.len())
        } else {
            client.put_object(&drive, "f", body, PutOptions::default()).await?;
            (0, 0)
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let sent = bw.taken() - taken;
        let how = if direct { "direct" } else { "put" };
        println!("{}", serde_json::json!({ "i": i, "how": how, "ms": (ms * 10.0).round() / 10.0, "sent": sent, "held": held, "shards": listed }));
        if direct {
            direct_ms.push(ms);
            direct_bytes.push(sent as f64);
        } else {
            put_ms.push(ms);
            put_bytes.push(sent as f64);
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "summary": { "size": size, "first_multipart": first_multipart, "uploads_each": put_ms.len(),
                "put": { "median_ms": median(put_ms).round(), "median_sent": median(put_bytes) },
                "direct": { "median_ms": median(direct_ms).round(), "median_sent": median(direct_bytes) } }
        })
    );
    client.delete_drive(&drive, true).await?;
    Ok(())
}
