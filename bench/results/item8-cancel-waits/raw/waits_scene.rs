// SPDX-License-Identifier: Apache-2.0
//! A mount's changes waiting behind a conflict, made through a daemon session as a mount makes
//! them, for the menu's transfers self-test. Built as a temporary example of voidfs-daemon.
//!
//!     waits_scene <daemon socket> <drive>
//!
//! The other computer's save goes through the SDK, with `VOIDFS_ENDPOINT` and the key from the
//! environment. It leaves, in a new folder `waits-<ms>`: `conflict.txt` in conflict, two later
//! saves of it queued behind the conflict, and `free.bin`, 64 KiB queued under a 2 KiB/s limit.

use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
use voidfs_daemon::DaemonClient;
use voidfs_daemon::api::Scope;

async fn save(s: &voidfs_daemon::FsClient, ino: u64, text: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let fh = s.open(ino, true).await?.fh;
    s.truncate(fh, 0).await?;
    s.write(fh, 0, Bytes::copy_from_slice(text)).await?;
    s.close(fh).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [socket, drive] = &args[..] else { return Err("usage: waits_scene <daemon socket> <drive>".into()) };
    let client = DaemonClient::new(Path::new(socket));
    let remote = voidfs_sdk::Client::new(voidfs_sdk::Config::from_env()?)?;
    let s = client.session(drive, false).await?;
    let dir = format!("waits-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis());
    let folder = s.mkdir(s.info().root, &dir, 0o755).await?.ino;
    let conflict = s.create(folder, "conflict.txt", 0o644).await?.ino;
    save(&s, conflict, b"base\n").await?;
    let key = format!("{dir}/conflict.txt");
    let unsent = |l: &voidfs_daemon::api::UploadList, key: &str| l.queue.items.iter().any(|i| i.key == key && i.state.to_send());
    for _ in 0..300 {
        if !unsent(&client.uploads(false, None).await?, &key) { break; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("{key} saved");

    // This Mac's edit waits while another computer saves the file: the guarded put is refused.
    client.pause(&Scope::Drive(drive.clone())).await?;
    save(&s, conflict, b"mine\n").await?;
    remote.put_object(drive, &key, "theirs\n", Default::default()).await?;
    println!("the other computer saved {key}");
    client.resume(&Scope::Drive(drive.clone())).await?;
    let mut held = false;
    for _ in 0..300 {
        held = client.uploads(false, None).await?.queue.items.iter().any(|i| i.key == key && i.conflict.is_some() && i.state.to_send());
        if held { break; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !held { return Err(format!("{key} didn't become a conflict").into()); }
    println!("{key} is in conflict");

    // Saved again after the conflict: these wait for it.
    save(&s, conflict, b"again\n").await?;
    save(&s, conflict, b"and again\n").await?;
    // An unrelated change that can be cancelled, held by a limit.
    client.limit(Some(2048)).await?;
    let free = s.create(folder, "free.bin", 0o644).await?.ino;
    let fh = s.open(free, true).await?.fh;
    s.write(fh, 0, Bytes::from(vec![7u8; 64 * 1024])).await?;
    s.close(fh).await?;
    s.release().await?;
    println!("folder {dir}");
    Ok(())
}
