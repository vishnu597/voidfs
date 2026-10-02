// SPDX-License-Identifier: Apache-2.0
//! `drives`, `drive create|show|delete|undelete` and `fork` (protocol §5).

use std::collections::HashMap;
use std::io::BufRead;

use futures::{StreamExt, TryStreamExt};
use serde_json::json;
use voidfs_sdk::{Client, CreateDrive, DriveInfo};

use crate::output::{Failure, Out, Result, shell_word, size, table};

/// Drives described at once.
const DESCRIBE_AT_ONCE: usize = 8;

pub async fn list(client: &Client, out: Out) -> Result<()> {
    let names = client.list_drives().await?;
    let infos: Vec<Option<DriveInfo>> = futures::stream::iter(names)
        .map(|d| async move {
            match client.describe_drive(&d.name).await {
                Ok(i) => Ok(Some(i)),
                // Deleted since it was listed.
                Err(e) if e.is_not_found() => Ok(None),
                Err(e) => Err(Failure::from(e).context(format!("drive {}", d.name))),
            }
        })
        .buffered(DESCRIBE_AT_ONCE)
        .try_collect()
        .await?;
    let drives: Vec<DriveInfo> = infos.into_iter().flatten().collect();
    out.emit(&json!({ "drives": drives }), || {
        if drives.is_empty() {
            return "No drives. Create one with `void drive create <name>`.".into();
        }
        let alias: HashMap<&str, &str> = drives.iter().map(|d| (d.drive_id.as_str(), d.alias.as_str())).collect();
        let rows: Vec<Vec<String>> = drives
            .iter()
            .map(|d| {
                let fork_of = d.fork_of.as_ref().map_or("-".into(), |f| alias.get(f.drive_id.as_str()).map_or(f.drive_id.clone(), |a| a.to_string()));
                vec![d.alias.clone(), d.drive_id.clone(), size(d.usage_bytes), d.created_at.as_deref().map_or("-".into(), seconds), fork_of]
            })
            .collect();
        table(&["NAME", "ID", "SIZE", "CREATED", "FORK OF"], &rows)
    })
}

/// An RFC 3339 time to the second: `2026-10-01T10:00:00Z`.
fn seconds(t: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(t).map_or_else(|_| t.to_owned(), |d| d.with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

pub async fn create(client: &Client, out: Out, name: &str, display_name: Option<String>) -> Result<()> {
    let made = client.create_drive(name, CreateDrive { display_name }).await.map_err(|e| Failure::from(e).context(format!("drive {name}")))?;
    out.emit(&json!({ "driveId": made.drive_id, "alias": name }), || format!("Created drive {name} ({}).", made.drive_id))
}

pub async fn show(client: &Client, out: Out, drive: &str) -> Result<()> {
    let d = client.describe_drive(drive).await.map_err(|e| Failure::from(e).context(format!("drive {drive}")))?;
    out.emit(&d, || {
        let mut lines = vec![d.alias.clone()];
        let mut field = |name: &str, value: String| lines.push(format!("  {name:<10}{value}"));
        field("id", d.drive_id.clone());
        if let Some(n) = d.display_name.as_ref().filter(|n| **n != d.alias) {
            field("name", n.clone());
        }
        if let Some(t) = &d.created_at {
            field("created", t.clone());
        }
        field("size", format!("{} ({} bytes)", size(d.usage_bytes), d.usage_bytes));
        if let Some(u) = d.unique_bytes {
            field("unique", format!("{} ({u} bytes)", size(u)));
        }
        if let Some(f) = &d.fork_of {
            field("fork of", match &f.fork_point {
                Some(p) => format!("{} at {p}", f.drive_id),
                None => f.drive_id.clone(),
            });
        }
        if !d.forks.is_empty() {
            field("forks", d.forks.join(", "));
        }
        field("position", d.seq.to_string());
        lines.join("\n")
    })
}

/// Deletes a drive once its name is typed back, unless `yes`. With `--json` the question isn't
/// printed, so that stderr holds JSON only, but the name is still read.
pub async fn delete(client: &Client, out: Out, drive: &str, hard: bool, yes: bool) -> Result<()> {
    if !yes {
        if !out.json {
            let what = if hard { "Permanently delete" } else { "Delete" };
            eprint!("{what} drive {drive}? Type its name to confirm: ");
        }
        let mut line = String::new();
        let read = std::io::stdin().lock().read_line(&mut line).unwrap_or(0);
        if read == 0 && !out.json {
            eprintln!();
        }
        if line.trim_end_matches(['\r', '\n']) != drive {
            return Err(Failure::new("NotConfirmed", format!("drive {drive} was not deleted: the name typed didn't match (pass -y to skip the question)")));
        }
    }
    client.delete_drive(drive, hard).await.map_err(|e| Failure::from(e).context(format!("drive {drive}")))?;
    out.emit(&json!({ "drive": drive, "hard": hard }), || {
        if hard { format!("Deleted drive {drive} permanently.") } else { format!("Deleted drive {drive}. `void drive undelete {}` recovers it for the retention window (30 days by default).", shell_word(drive)) }
    })
}

pub async fn undelete(client: &Client, out: Out, drive: &str) -> Result<()> {
    let d = client.undelete_drive(drive).await.map_err(|e| Failure::from(e).context(format!("drive {drive}")))?;
    out.emit(&json!({ "driveId": d.drive_id, "alias": drive }), || format!("Recovered drive {drive} ({}).", d.drive_id))
}

pub async fn fork(client: &Client, out: Out, source: &str, name: &str) -> Result<()> {
    let f = client.fork_drive(source, name).await.map_err(|e| Failure::from(e).context(format!("fork of {source} as {name}")))?;
    out.emit(&json!({ "driveId": f.drive_id, "alias": name, "source": source, "sourceId": f.source_id, "forkPoint": f.fork_point }), || {
        let at = f.fork_point.as_deref().map(|p| format!(" at {p}")).unwrap_or_default();
        format!("Forked {source} as {name} ({}){at}. Its history up to then is shared; from now on the two are independent.", f.drive_id)
    })
}
