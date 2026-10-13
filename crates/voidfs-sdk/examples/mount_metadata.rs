// SPDX-License-Identifier: Apache-2.0
//! Server attributes and retained deleted entries for application-matrix evidence.
//!     mount_metadata <drive> attrs <key>
//!     mount_metadata <drive> deleted <prefix>

use voidfs_sdk::{Client, Config, ReadOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [drive, op, key] = args.as_slice() else { return Err("usage: mount_metadata <drive> attrs <key> | deleted <prefix>".into()); };
    let client = Client::new(Config::from_env()?)?;
    let result = match op.as_str() {
        "attrs" => serde_json::to_value(client.attributes(drive, key, ReadOptions::default()).await?)?,
        "deleted" => serde_json::to_value(client.list_deleted(drive, key).await?)?,
        _ => return Err("expected attrs or deleted".into()),
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
