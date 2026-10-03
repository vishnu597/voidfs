// SPDX-License-Identifier: Apache-2.0
//! `uploads [--watch] [--all]` and `uploads pause|resume|cancel|limit|clear`: the daemon's upload
//! queue.

use std::io::IsTerminal;

use voidfs_client::State;
use voidfs_daemon::api::{Scope, UploadList};

use crate::daemon::{Place, failure, uptime};
use crate::output::{Failure, Out, Result, count, size, stdout, table};

#[derive(clap::Args, Debug)]
pub struct UploadsArgs {
    #[command(subcommand)]
    pub command: Option<UploadsCommand>,
    /// Keep redrawing, every second, until interrupted (with --json, one document a line)
    #[arg(long)]
    pub watch: bool,
    /// List finished uploads too
    #[arg(long)]
    pub all: bool,
}

#[derive(clap::Subcommand, Debug)]
pub enum UploadsCommand {
    /// Pause uploads: what is uploading stops at its next request, and goes on from there when
    /// resumed. Kept across restarts
    Pause(Target),
    /// Resume paused uploads, and try failed ones again at once
    Resume(Target),
    /// Cancel uploads: nothing of them is published, and what was uploading stops
    Cancel(Target),
    /// Limit how fast uploads go, at once and across restarts: bytes a second, as `10MiB`, `500K`
    /// or `2MB/s`, or `unlimited`
    Limit {
        #[arg(value_name = "RATE")]
        rate: String,
    },
    /// Forget finished uploads
    Clear,
}

/// What a pause, a resume or a cancel applies to.
#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false)]
pub struct Target {
    /// One upload, by the id `void uploads` lists
    #[arg(value_name = "ID")]
    pub id: Option<i64>,
    /// A whole batch, by the id `void upload` printed
    #[arg(long, value_name = "ID")]
    pub batch: Option<i64>,
    /// Everything for one drive, by alias or id
    #[arg(long, value_name = "DRIVE")]
    pub drive: Option<String>,
    /// Everything
    #[arg(long)]
    pub all: bool,
}

impl Target {
    fn scope(&self) -> Scope {
        match (self.id, self.batch, &self.drive) {
            (Some(id), _, _) => Scope::Entry(id),
            (_, Some(b), _) => Scope::Batch(b),
            (_, _, Some(d)) => Scope::Drive(d.clone()),
            _ => Scope::All,
        }
    }

    fn what(&self) -> String {
        match self.scope() {
            Scope::Entry(id) => format!("upload {id}"),
            Scope::Batch(b) => format!("batch {b}"),
            Scope::Drive(d) => format!("uploads to {d}"),
            Scope::All => "all uploads".into(),
        }
    }
}

/// `10MiB`, `500K`, `2MB/s` or `unlimited`, as bytes a second; `None` is unlimited. K, M and G
/// are binary (as KiB, MiB and GiB), KB, MB and GB decimal.
pub fn parse_rate(s: &str) -> Result<Option<u64>> {
    let t = s.trim().to_ascii_lowercase();
    if matches!(t.as_str(), "unlimited" | "none" | "off") {
        return Ok(None);
    }
    let t = t.strip_suffix("/s").unwrap_or(&t);
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (number, unit) = t.split_at(split);
    let bad = || Failure::usage(format!("{s:?} is not a rate: bytes a second, as `10MiB`, `500K` or `2MB/s`, or `unlimited`"));
    let n: f64 = number.parse().map_err(|_| bad())?;
    let unit: f64 = match unit.trim() {
        "" | "b" => 1.0,
        "k" | "kib" => 1024.0,
        "kb" => 1e3,
        "m" | "mib" => 1024.0 * 1024.0,
        "mb" => 1e6,
        "g" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "gb" => 1e9,
        _ => return Err(bad()),
    };
    let v = (n * unit).round() as u64;
    Ok((v > 0).then_some(v))
}

pub async fn run(out: Out, args: &UploadsArgs) -> Result<()> {
    let place = Place::new()?;
    let c = &place.client;
    match &args.command {
        Some(UploadsCommand::Pause(t)) => {
            let a = c.pause(&t.scope()).await.map_err(failure)?;
            out.emit(&a, || format!("paused {} ({} unfinished)", t.what(), a.items))
        }
        Some(UploadsCommand::Resume(t)) => {
            let a = c.resume(&t.scope()).await.map_err(failure)?;
            out.emit(&a, || format!("resumed {} ({} unfinished)", t.what(), a.items))
        }
        Some(UploadsCommand::Cancel(t)) => {
            let a = c.cancel(&t.scope()).await.map_err(failure)?;
            out.emit(&a, || format!("cancelled {} ({} unfinished)", t.what(), a.items))
        }
        Some(UploadsCommand::Limit { rate }) => {
            let l = c.limit(parse_rate(rate)?).await.map_err(failure)?;
            out.emit(&l, || match l.bytes_per_second {
                Some(b) => format!("uploads limited to {}/s", size(b)),
                None => "uploads unlimited".into(),
            })
        }
        Some(UploadsCommand::Clear) => {
            let n = c.clear().await.map_err(failure)?;
            out.emit(&n, || format!("forgot {}", count(n.cleared as usize, "finished upload")))
        }
        None if args.watch => watch(&place, out, args.all).await,
        None => {
            let l = c.uploads(args.all, None).await.map_err(failure)?;
            out.emit(&l, || render(&l))
        }
    }
}

async fn watch(place: &Place, out: Out, all: bool) -> Result<()> {
    let mut w = place.client.watch_uploads(all, None).await.map_err(failure)?;
    let tty = std::io::stdout().is_terminal();
    while let Some(l) = w.next().await {
        let l = l.map_err(failure)?;
        if out.json {
            stdout(&serde_json::to_string(&l).expect("JSON"))?;
        } else if tty {
            // Clears the screen and draws again from the top.
            stdout(&format!("\x1b[H\x1b[2J{}", render(&l)))?;
        } else {
            stdout(&format!("{}\n", render(&l)))?;
        }
    }
    Ok(())
}

fn state(i: &voidfs_client::Item) -> String {
    match i.state {
        State::Failed => format!("failed: {}", i.error.as_deref().unwrap_or("")),
        _ if i.paused && !i.state.finished() => "paused".into(),
        State::Done if i.conflict.is_some() => "done, over a newer version".into(),
        s => s.as_str().into(),
    }
}

fn batch_state(b: &voidfs_client::BatchStatus) -> String {
    let open = b.items - b.done - b.failed - b.cancelled;
    let pct = (b.sent * 100).checked_div(b.bytes).unwrap_or(100);
    match (open, b.failed) {
        (0, 0) if b.cancelled == b.items => "cancelled".into(),
        (0, 0) if b.cancelled > 0 => format!("done, {} cancelled", b.cancelled),
        (0, 0) => "done".into(),
        (0, _) => "failed".into(),
        _ if b.paused => format!("paused at {pct}%"),
        _ => format!("{pct}%"),
    }
}

/// The queue as a table, with a line of totals.
pub fn render(l: &UploadList) -> String {
    let q = &l.queue;
    let mut parts = Vec::new();
    if q.items.is_empty() {
        parts.push("no uploads in progress".to_owned());
    } else {
        let rows: Vec<Vec<String>> = q
            .items
            .iter()
            .map(|i| {
                let pct = if i.state == State::Done || i.size == 0 { 100 } else { i.sent * 100 / i.size };
                vec![i.id.to_string(), i.drive.clone(), i.to_key.as_ref().map_or_else(|| i.key.clone(), |to| format!("{} -> {to}", i.key)), size(i.size), format!("{pct}%"), state(i)]
            })
            .collect();
        parts.push(table(&["ID", "DRIVE", "KEY", "SIZE", "SENT", "STATE"], &rows));
    }
    if !q.batches.is_empty() {
        let rows: Vec<Vec<String>> = q
            .batches
            .iter()
            .rev()
            .map(|b| vec![b.id.to_string(), b.label.clone(), b.items.to_string(), b.done.to_string(), b.failed.to_string(), size(b.bytes), batch_state(b)])
            .collect();
        parts.push(table(&["BATCH", "LABEL", "ITEMS", "DONE", "FAILED", "SIZE", "STATE"], &rows));
    }
    parts.push(totals(l));
    parts.join("\n\n")
}

/// `2 uploading, 3 queued · 1.2 GiB to send · 5.0 MiB/s · 4m 6s left · limit 10.0 MiB/s`.
fn totals(l: &UploadList) -> String {
    let q = &l.queue;
    let mut total = if q.unpublished == 0 {
        "nothing to send".to_owned()
    } else {
        let n = |s: State| q.items.iter().filter(|i| i.state == s).count();
        let mut t = format!("{} uploading, {} queued", n(State::Uploading), n(State::Queued));
        if n(State::Failed) > 0 {
            t.push_str(&format!(", {} failed", n(State::Failed)));
        }
        t.push_str(&format!(" · {} to send", size(q.unpublished_bytes)));
        if l.rate > 0 {
            t.push_str(&format!(" · {}/s", size(l.rate)));
        }
        if let Some(eta) = l.eta_secs.filter(|_| !q.paused) {
            t.push_str(&format!(" · {} left", uptime(eta)));
        }
        t
    };
    if q.paused {
        total.push_str(" · paused");
    }
    if !q.paused_drives.is_empty() {
        total.push_str(&format!(" · paused on {}", q.paused_drives.join(", ")));
    }
    if let Some(b) = q.bandwidth {
        total.push_str(&format!(" · limit {}/s", size(b)));
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_read_as_people_write_them() {
        assert_eq!(parse_rate("10MiB").unwrap(), Some(10 << 20));
        assert_eq!(parse_rate("10M/s").unwrap(), Some(10 << 20));
        assert_eq!(parse_rate("2MB/s").unwrap(), Some(2_000_000));
        assert_eq!(parse_rate("500k").unwrap(), Some(500 << 10));
        assert_eq!(parse_rate("1.5G").unwrap(), Some(3 << 29));
        assert_eq!(parse_rate("4096").unwrap(), Some(4096));
        assert_eq!(parse_rate("unlimited").unwrap(), None);
        assert_eq!(parse_rate("0").unwrap(), None);
        assert_eq!(parse_rate("fast").unwrap_err().exit, 2);
        assert_eq!(parse_rate("10 parsecs").unwrap_err().exit, 2);
    }
}
