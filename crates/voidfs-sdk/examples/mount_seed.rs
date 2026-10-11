// SPDX-License-Identifier: Apache-2.0
//! Fills a drive with the test tree the FSKit spike measured against (`apps/macos/scripts/seed.py`,
//! which needs boto3), so a mount can be compared with the spike's listings and hashes:
//!
//! ```text
//! hello.txt                    a small text file
//! many/f0000.txt … f0999.txt   1,000 small files
//! many10k/f00000.txt …         10,000 small files
//! media/big.bin                1 GiB of random bytes, also kept locally for comparison
//! docs/nested/deep/readme.md   a deep path
//! tagged.txt                   a file with extended attributes
//! ünïcödé – 名前.txt            a non-ASCII name, stored NFC
//! ```
//!
//! The server and key come from `VOIDFS_ENDPOINT`, `VOIDFS_ACCESS_KEY_ID` and
//! `VOIDFS_SECRET_ACCESS_KEY`.
//!
//!     cargo run --release -p voidfs-sdk --example mount_seed -- [drive] [--big <path>]

use std::collections::BTreeMap;
use std::io::{Read, Write};

use bytes::Bytes;
use futures::StreamExt;
use rand::{Rng, SeedableRng};
use voidfs_sdk::{AttributesUpdate, Client, Config, PutOptions};

const BIG: u64 = 1 << 30;
const PART: usize = 64 << 20;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

async fn many(client: &Client, drive: &str, prefix: &str, count: usize, width: usize) -> voidfs_sdk::Result<()> {
    let puts = (0..count).map(|i| {
        let (client, key) = (client.clone(), format!("{prefix}f{i:0width$}.txt"));
        async move { client.put_object(drive, &key, format!("file {i} of {count}\n"), PutOptions::default()).await }
    });
    let mut done = futures::stream::iter(puts).buffer_unordered(16);
    while let Some(result) = done.next().await { result?; }
    println!("{prefix}: {count} files");
    Ok(())
}

/// The 1 GiB file: the local copy if it has the right size, otherwise new random bytes.
fn big(path: &str) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() == BIG) { return Ok(()); }
    let mut rng = rand::rngs::StdRng::seed_from_u64(rand::rng().next_u64());
    let mut file = std::fs::File::create(path)?;
    let mut block = vec![0u8; 8 << 20];
    for _ in 0..BIG / block.len() as u64 {
        rng.fill_bytes(&mut block);
        file.write_all(&block)?;
    }
    file.sync_all()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let drive = std::env::args().nth(1).filter(|a| !a.starts_with("--")).unwrap_or_else(|| "spike".into());
    let local = arg("--big").unwrap_or_else(|| format!("{}/voidfs-seed-big.bin", std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into()).trim_end_matches('/')));
    let client = Client::new(Config::from_env()?)?;
    match client.create_drive(&drive, Default::default()).await {
        Ok(_) => println!("created drive {drive}"),
        Err(e) if e.status() == Some(409) => println!("drive {drive} exists"),
        Err(e) => return Err(e.into()),
    }
    for (key, body) in [("hello.txt", "hello from voidfs\n"), ("docs/nested/deep/readme.md", "# deep\n"), ("ünïcödé – 名前.txt", "unicode\n"), ("tagged.txt", "has xattrs\n")] {
        client.put_object(&drive, key, body, PutOptions::default()).await?;
    }
    let set_xattrs = BTreeMap::from([("user.voidfs.test".to_owned(), Bytes::from_static(b"hello")), ("com.apple.metadata:_kMDItemUserTags".to_owned(), Bytes::from_static(b"bplist00"))]);
    client.set_attributes(&drive, "tagged.txt", AttributesUpdate { set_xattrs, ..Default::default() }, Default::default()).await?;
    many(&client, &drive, "many/", 1000, 4).await?;
    many(&client, &drive, "many10k/", 10_000, 5).await?;
    big(&local)?;
    let upload = client.create_multipart_upload(&drive, "media/big.bin", PutOptions::default()).await?;
    let mut file = std::fs::File::open(&local)?;
    let mut parts = Vec::new();
    for number in 1..=(BIG as usize / PART) as u32 {
        let mut part = vec![0u8; PART];
        file.read_exact(&mut part)?;
        parts.push((number, client.upload_part(&drive, "media/big.bin", &upload, number, part).await?));
    }
    client.complete_multipart_upload(&drive, "media/big.bin", &upload, &parts, Default::default()).await?;
    println!("media/big.bin: {BIG} bytes, local copy at {local}");
    Ok(())
}
