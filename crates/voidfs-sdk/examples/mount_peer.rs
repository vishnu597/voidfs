// SPDX-License-Identifier: Apache-2.0
//! Another machine's changes, for testing a mount's coherence: puts, deletes and renames through
//! the SDK, each printed with the time its commit returned.
//!
//! The server and key come from `VOIDFS_ENDPOINT`, `VOIDFS_ACCESS_KEY_ID` and
//! `VOIDFS_SECRET_ACCESS_KEY`.
//!
//!     mount_peer <drive> put <key> <text>
//!     mount_peer <drive> fill <key> <bytes> <byte value>
//!     mount_peer <drive> delete <key>
//!     mount_peer <drive> rename <from> <to>

use voidfs_sdk::{Client, Config, PutOptions, RenameOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let client = Client::new(Config::from_env()?)?;
    let (drive, op) = (&args[0], args[1].as_str());
    match (op, &args[2..]) {
        ("put", [key, text]) => { client.put_object(drive, key, text.clone(), PutOptions::default()).await?; },
        ("fill", [key, size, value]) => { client.put_object(drive, key, vec![value.parse::<u8>()?; size.parse()?], PutOptions::default()).await?; },
        ("delete", [key]) => { client.delete_object(drive, key, Default::default()).await?; },
        ("rename", [from, to]) => { client.rename(drive, from, to, RenameOptions { replace: true, ..Default::default() }).await?; },
        _ => return Err("usage: mount_peer <drive> put <key> <text> | fill <key> <bytes> <byte> | delete <key> | rename <from> <to>".into()),
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    println!("{op} committed at {}.{:06}", now.as_secs(), now.subsec_micros());
    Ok(())
}
