// SPDX-License-Identifier: Apache-2.0
//! Fills a drive with the folders step 5's item 9 measures Finder's requests against: 1,000 and
//! 10,000 small files, once without extended attributes and once with the ones a Mac's own files
//! carry (`apps/macos/scripts/finder-requests.py`):
//!
//! ```text
//! plain-1k/f0000.txt …     1,000 files, no xattrs
//! xattrs-1k/f0000.txt …    1,000 files, each with com.apple.provenance; every third also with
//!                          com.apple.quarantine and kMDItemWhereFroms (a download), every tenth
//!                          also with a colour label in com.apple.FinderInfo and a user tag
//! plain-10k/, xattrs-10k/  the same with 10,000 files
//! ```
//!
//! Given tags, it makes the four folders under each tag's folder instead (`l0/plain-1k/`, …):
//! Finder remembers a folder it has shown (its scroll position, say), so each Finder run needs
//! folders it has never opened.
//!
//! The server and key come from `VOIDFS_ENDPOINT`, `VOIDFS_ACCESS_KEY_ID` and
//! `VOIDFS_SECRET_ACCESS_KEY`.
//!
//!     cargo run --release -p voidfs-sdk --example finder_seed -- [drive [tag…]]

use std::collections::BTreeMap;

use bytes::Bytes;
use futures::StreamExt;
use voidfs_sdk::{AttributesUpdate, Client, Config, PutOptions};

/// The xattrs of file `i` in an xattr folder, as described above.
fn xattrs(i: usize) -> BTreeMap<String, Bytes> {
    let mut x = BTreeMap::new();
    // What the kernel writes: a version, a flag and an 8-byte identifier of the writing app.
    let mut provenance = vec![1u8, 2, 0];
    provenance.extend_from_slice(&(i as u64 ^ 0x5eed_f00d_cafe_beef).to_le_bytes());
    x.insert("com.apple.provenance".to_owned(), Bytes::from(provenance));
    if i.is_multiple_of(3) {
        x.insert("com.apple.quarantine".to_owned(), Bytes::from(format!("0083;6717a8b2;Safari;{i:08X}-0000-4000-8000-000000000000")));
        x.insert("com.apple.metadata:kMDItemWhereFroms".to_owned(), Bytes::from_static(b"bplist00\xa2\x01\x02_\x10\x1ahttps://example.com/f.txt_\x10\x13https://example.com/\x08\x0b(\x00\x00\x00\x00\x00\x00\x01\x01\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00>"));
    }
    if i.is_multiple_of(10) {
        let mut finder_info = [0u8; 32];
        finder_info[9] = 0x0c; // the red label
        x.insert("com.apple.FinderInfo".to_owned(), Bytes::copy_from_slice(&finder_info));
        x.insert("com.apple.metadata:_kMDItemUserTags".to_owned(), Bytes::from_static(b"bplist00\xa1\x01U Red\n6\x08\n\x00\x00\x00\x00\x00\x00\x01\x01\x00\x00\x00\x00\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x10"));
    }
    x
}

async fn folder(client: &Client, drive: &str, prefix: &str, count: usize, with_xattrs: bool) -> voidfs_sdk::Result<()> {
    let width = if count > 1000 { 5 } else { 4 };
    let files = (0..count).map(|i| {
        let (client, key) = (client.clone(), format!("{prefix}f{i:0width$}.txt"));
        async move {
            client.put_object(drive, &key, format!("file {i} of {count} in {prefix}\n"), PutOptions::default()).await?;
            if with_xattrs {
                client.set_attributes(drive, &key, AttributesUpdate { set_xattrs: xattrs(i), ..Default::default() }, Default::default()).await?;
            }
            Ok::<_, voidfs_sdk::Error>(())
        }
    });
    let mut done = futures::stream::iter(files).buffer_unordered(32);
    while let Some(result) = done.next().await { result?; }
    println!("{prefix}: {count} files{}", if with_xattrs { ", with xattrs" } else { "" });
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let drive = args.first().cloned().unwrap_or_else(|| "finder".into());
    let tags: Vec<String> = if args.len() > 1 { args[1..].iter().map(|t| format!("{}/", t.trim_matches('/'))).collect() } else { vec![String::new()] };
    let client = Client::new(Config::from_env()?)?;
    match client.create_drive(&drive, Default::default()).await {
        Ok(_) => println!("created drive {drive}"),
        Err(e) if e.status() == Some(409) => println!("drive {drive} exists"),
        Err(e) => return Err(e.into()),
    }
    for tag in &tags {
        for (name, count, with_xattrs) in [("plain-1k/", 1000, false), ("xattrs-1k/", 1000, true), ("plain-10k/", 10_000, false), ("xattrs-10k/", 10_000, true)] {
            folder(&client, &drive, &format!("{tag}{name}"), count, with_xattrs).await?;
        }
    }
    Ok(())
}
