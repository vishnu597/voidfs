// SPDX-License-Identifier: Apache-2.0
//! `upload <paths…> <drive>:[/folder] [--detach]`: handed to the daemon when one runs (or with
//! `--detach`, which starts one), and followed until it is up unless detached; else in the
//! foreground, with whole puts and multipart uploads through the AWS client for large files.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, Utc};
use futures::StreamExt;
use serde::Serialize;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use voidfs_daemon::api;
use voidfs_sdk::aws_sdk_s3::primitives::ByteStream;
use voidfs_sdk::aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use voidfs_sdk::{Client, PutOptions};

use crate::config::Connection;
use crate::daemon::{self, DaemonOptions, Place};
use crate::output::{Failure, Out, Result, count, size, stdout, warn};

const MIB: u64 = 1 << 20;
/// Files at least this large go up in parts.
pub const MULTIPART_THRESHOLD: u64 = 64 * MIB;
/// The smallest part, and the size of each but the last unless the file needs more than
/// `MAX_PARTS` of them.
pub const PART_SIZE: u64 = 16 * MIB;
const MAX_PARTS: u64 = 10_000;
/// The least S3 takes for any part but the last.
const MIN_PART_SIZE: u64 = 5 * MIB;
/// Files uploading at once, as SpaceFS's daemon publishes 16 objects at once.
const FILES_AT_ONCE: usize = 16;
/// Parts of one file uploading at once.
const PARTS_AT_ONCE: usize = 4;
/// Bytes held in memory for requests in flight, in MiB: a file or part waits for its share.
const BUDGET_MIB: u32 = 128;

#[derive(clap::Args, Debug, Clone)]
pub struct UploadArgs {
    /// Local files or folders, then the destination as `<drive>:[/folder]`, for example
    /// `void upload clip.mov renders/ footage:/cuts`. A folder goes up with its name and
    /// everything under it
    #[arg(required = true, num_args = 2.., value_name = "PATHS")]
    pub paths: Vec<PathBuf>,
    /// Hand the batch to the daemon and return at once, instead of following it until it is up
    /// (`void uploads` follows it from there). Starts the daemon if it isn't running
    #[arg(long)]
    pub detach: bool,
    /// Files of this many MiB and more go up in parts
    #[arg(long, hide = true, value_name = "MIB", default_value_t = MULTIPART_THRESHOLD / MIB)]
    pub multipart_threshold: u64,
    /// The size of each part, in MiB (at least 5)
    #[arg(long, hide = true, value_name = "MIB", default_value_t = PART_SIZE / MIB)]
    pub part_size: u64,
    /// Bytes held in memory for requests in flight, in MiB
    #[arg(long, hide = true, value_name = "MIB", default_value_t = BUDGET_MIB, value_parser = clap::value_parser!(u32).range(1..))]
    pub memory_budget: u32,
}

/// `<drive>:[/folder]`: the drive, and the folder's key (empty for the root, else ending in `/`).
pub fn parse_destination(s: &str) -> Result<(String, String)> {
    let Some((drive, folder)) = s.split_once(':').filter(|(d, _)| !d.is_empty()) else {
        return Err(Failure::usage(format!("the destination {s:?} is `<drive>:[/folder]`, for example `footage:/cuts`, and comes last")));
    };
    let folder = folder.trim_matches('/');
    Ok((drive.to_owned(), if folder.is_empty() { String::new() } else { format!("{folder}/") }))
}

/// A file to upload.
#[derive(Debug, Clone)]
struct Job {
    local: PathBuf,
    key: String,
    size: u64,
    mtime: Option<String>,
    mode: Option<u32>,
}

/// An empty folder, which no file's key would make.
#[derive(Debug, Clone)]
struct Folder {
    local: PathBuf,
    key: String,
}

/// What a walk of the sources found.
#[derive(Debug, Default)]
struct Plan {
    files: Vec<Job>,
    folders: Vec<Folder>,
    skipped: Vec<Skipped>,
}

#[derive(Debug, Serialize)]
struct Skipped {
    path: PathBuf,
    reason: &'static str,
}

fn attrs(m: &std::fs::Metadata) -> (Option<String>, Option<u32>) {
    let mtime = m.modified().ok().map(|t| DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Micros, true));
    #[cfg(unix)]
    let mode = Some(std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o7777);
    #[cfg(not(unix))]
    let mode = None;
    (mtime, mode)
}

fn utf8_name(path: &Path) -> Result<String> {
    let name = path.file_name().ok_or_else(|| Failure::invalid(format!("{}: has no name to upload under", path.display())))?;
    name.to_str().map(str::to_owned).ok_or_else(|| Failure::invalid(format!("{}: the name is not UTF-8", path.display())))
}

/// Walks the sources: a file goes to `<folder><name>`, a folder to `<folder><name>/…`. Symbolic
/// links inside folders, and anything but files and folders, are skipped.
fn plan(sources: &[PathBuf], folder: &str) -> Result<Plan> {
    let mut plan = Plan::default();
    for src in sources {
        let meta = std::fs::metadata(src).map_err(|e| Failure::local(src, e))?;
        let name = utf8_name(&std::fs::canonicalize(src).map_err(|e| Failure::local(src, e))?)?;
        if meta.is_dir() {
            walk(&mut plan, src, &format!("{folder}{name}/"))?;
        } else if meta.is_file() {
            let (mtime, mode) = attrs(&meta);
            plan.files.push(Job { local: src.clone(), key: format!("{folder}{name}"), size: meta.len(), mtime, mode });
        } else {
            plan.skipped.push(Skipped { path: src.clone(), reason: "not a file or folder" });
        }
    }
    let mut seen: HashMap<&str, &Path> = HashMap::new();
    for j in &plan.files {
        if let Some(other) = seen.insert(&j.key, &j.local) {
            return Err(Failure::invalid(format!("{} and {} would both upload to {}", other.display(), j.local.display(), j.key)));
        }
    }
    Ok(plan)
}

fn walk(plan: &mut Plan, dir: &Path, prefix: &str) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).and_then(|r| r.collect::<std::io::Result<Vec<_>>>()).map_err(|e| Failure::local(dir, e))?;
    entries.sort_by_key(|e| e.file_name());
    if entries.is_empty() {
        plan.folders.push(Folder { local: dir.to_owned(), key: prefix.to_owned() });
    }
    for e in entries {
        let path = e.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|err| Failure::local(&path, err))?;
        let Some(name) = e.file_name().to_str().map(str::to_owned) else {
            plan.skipped.push(Skipped { path, reason: "the name is not UTF-8" });
            continue;
        };
        if meta.is_dir() {
            walk(plan, &path, &format!("{prefix}{name}/"))?;
        } else if meta.is_file() {
            let (mtime, mode) = attrs(&meta);
            plan.files.push(Job { local: path, key: format!("{prefix}{name}"), size: meta.len(), mtime, mode });
        } else if meta.file_type().is_symlink() {
            plan.skipped.push(Skipped { path, reason: "a symbolic link" });
        } else {
            plan.skipped.push(Skipped { path, reason: "not a file or folder" });
        }
    }
    Ok(())
}

/// The size of each part of a `size`-byte file: `preferred`, or more if it would take more
/// than 10,000 parts.
pub fn part_size(size: u64, preferred: u64) -> u64 {
    let least = size.div_ceil(MAX_PARTS).div_ceil(MIB) * MIB;
    preferred.max(least)
}

/// A file uploaded.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Uploaded {
    path: PathBuf,
    key: String,
    size: u64,
    version_id: String,
    etag: Option<String>,
    /// The parts of a multipart upload; absent for a whole put.
    #[serde(skip_serializing_if = "Option::is_none")]
    parts: Option<usize>,
}

/// Progress for people, on stderr when it is a terminal: one line, rewritten.
struct Progress {
    tty: bool,
    files: usize,
    bytes: u64,
    done_files: AtomicUsize,
    done_bytes: AtomicU64,
}

impl Progress {
    fn add(&self, files: usize, bytes: u64) {
        let f = self.done_files.fetch_add(files, Ordering::Relaxed) + files;
        let b = self.done_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if self.tty {
            let _ = write!(std::io::stderr().lock(), "\r\x1b[2K{}/{} files, {} of {}", f, self.files, size(b), size(self.bytes));
        }
    }

    fn clear(&self) {
        if self.tty {
            let _ = write!(std::io::stderr().lock(), "\r\x1b[2K");
        }
    }
}

struct Shared {
    client: Client,
    drive: String,
    threshold: u64,
    part_size: u64,
    budget: Arc<Semaphore>,
    budget_mib: u32,
    stop: AtomicBool,
    progress: Arc<Progress>,
}

/// Waits for `bytes` of the memory budget. Whatever holds a share makes progress on its own (a
/// whole put, or a part's task), so that waiting for one never waits on the waiter.
async fn share(s: &Shared, bytes: u64) -> OwnedSemaphorePermit {
    let mib = bytes.div_ceil(MIB).clamp(1, s.budget_mib as u64) as u32;
    s.budget.clone().acquire_many_owned(mib).await.expect("the budget is never closed")
}

fn joined<T>(r: Option<std::result::Result<Result<T>, tokio::task::JoinError>>) -> Result<T> {
    r.expect("a part in flight").unwrap_or_else(|e| Err(Failure::new("InternalError", format!("a part's upload failed: {e}"))))
}

/// Up to `want` bytes, fewer only at the file's end.
async fn read_part(file: &mut tokio::fs::File, want: u64) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(want as usize);
    file.take(want).read_to_end(&mut buf).await?;
    Ok(buf)
}

async fn put_whole(s: &Shared, job: &Job) -> Result<Uploaded> {
    let _budget = share(s, job.size).await;
    let data = tokio::fs::read(&job.local).await.map_err(|e| Failure::local(&job.local, e))?;
    let len = data.len() as u64;
    let opts = PutOptions { mtime: job.mtime.clone(), mode: job.mode, ..Default::default() };
    let w = s.client.put_object(&s.drive, &job.key, data, opts).await?;
    s.progress.add(1, len);
    Ok(Uploaded { path: job.local.clone(), key: job.key.clone(), size: len, version_id: w.version_id, etag: w.etag, parts: None })
}

async fn put_parts(s: &Shared, job: &Job) -> Result<Uploaded> {
    let s3 = s.client.s3();
    let headers: Vec<(&'static str, String)> = [("x-voidfs-mtime", job.mtime.clone()), ("x-voidfs-mode", job.mode.map(|m| format!("{m:04o}")))].into_iter().filter_map(|(k, v)| Some((k, v?))).collect();
    let created = s3
        .create_multipart_upload()
        .bucket(&s.drive)
        .key(&job.key)
        .customize()
        .mutate_request(move |r| {
            for (k, v) in &headers {
                r.headers_mut().insert(*k, v.clone());
            }
        })
        .send()
        .await
        .map_err(voidfs_sdk::Error::from)?;
    let upload_id = created.upload_id().ok_or_else(|| Failure::new("UnexpectedResponse", "CreateMultipartUpload answered no upload id"))?.to_owned();
    let result = async {
        let mut file = tokio::fs::File::open(&job.local).await.map_err(|e| Failure::local(&job.local, e))?;
        let part_size = part_size(job.size, s.part_size);
        // Dropped on a failure, which stops the parts still uploading.
        let mut in_flight = tokio::task::JoinSet::new();
        let mut done: Vec<CompletedPart> = Vec::new();
        let mut total = 0u64;
        for number in 1.. {
            if s.stop.load(Ordering::Relaxed) {
                return Err(Failure::new("Cancelled", "stopped: another upload failed"));
            }
            let budget = share(s, part_size).await;
            let data = read_part(&mut file, part_size).await.map_err(|e| Failure::local(&job.local, e))?;
            if data.is_empty() && number > 1 {
                break;
            }
            let len = data.len() as u64;
            total += len;
            let last = len < part_size;
            let (s3, drive, key, id, progress) = (s3.clone(), s.drive.clone(), job.key.clone(), upload_id.clone(), s.progress.clone());
            in_flight.spawn(async move {
                let r = s3.upload_part().bucket(drive).key(key).upload_id(id).part_number(number).body(ByteStream::from(Bytes::from(data))).send().await;
                drop(budget);
                let etag = r.map_err(voidfs_sdk::Error::from)?.e_tag().map(str::to_owned).ok_or_else(|| Failure::new("UnexpectedResponse", format!("part {number} answered no ETag")))?;
                progress.add(0, len);
                Ok::<_, Failure>(CompletedPart::builder().part_number(number).e_tag(etag).build())
            });
            if in_flight.len() >= PARTS_AT_ONCE {
                done.push(joined(in_flight.join_next().await)?);
            }
            if last {
                break;
            }
        }
        while !in_flight.is_empty() {
            done.push(joined(in_flight.join_next().await)?);
        }
        done.sort_by_key(|p| p.part_number());
        let parts = done.len();
        let finished = s3
            .complete_multipart_upload()
            .bucket(&s.drive)
            .key(&job.key)
            .upload_id(&upload_id)
            .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(done)).build())
            .send()
            .await
            .map_err(voidfs_sdk::Error::from)?;
        let version_id = finished.version_id().ok_or_else(|| Failure::new("UnexpectedResponse", "CompleteMultipartUpload answered no version id"))?.to_owned();
        s.progress.add(1, 0);
        Ok(Uploaded { path: job.local.clone(), key: job.key.clone(), size: total, version_id, etag: finished.e_tag().map(str::to_owned), parts: Some(parts) })
    }
    .await;
    if result.is_err() {
        let _ = s3.abort_multipart_upload().bucket(&s.drive).key(&job.key).upload_id(&upload_id).send().await;
    }
    result
}

async fn upload_one(s: &Shared, job: &Job) -> Result<Uploaded> {
    let r = if job.size >= s.threshold { put_parts(s, job).await } else { put_whole(s, job).await };
    r.map_err(|e| e.context(format!("{} -> {}:{}", job.local.display(), s.drive, job.key)))
}

/// In the foreground, in this process.
pub async fn foreground(client: &Client, out: Out, args: &UploadArgs) -> Result<()> {
    let (dest, sources) = args.paths.split_last().expect("clap requires two paths");
    let (drive, folder) = parse_destination(&dest.to_string_lossy())?;
    if args.part_size < MIN_PART_SIZE / MIB {
        return Err(Failure::usage(format!("--part-size is at least {} MiB", MIN_PART_SIZE / MIB)));
    }
    let plan = plan(sources, &folder)?;
    for s in &plan.skipped {
        if !out.json {
            warn(&format!("skipped {}: {}", s.path.display(), s.reason));
        }
    }
    let started = Instant::now();
    let shared = Shared {
        client: client.clone(),
        drive: drive.clone(),
        threshold: args.multipart_threshold.saturating_mul(MIB),
        part_size: args.part_size * MIB,
        budget: Arc::new(Semaphore::new(args.memory_budget as usize)),
        budget_mib: args.memory_budget,
        stop: AtomicBool::new(false),
        progress: Arc::new(Progress { tty: std::io::stderr().is_terminal(), files: plan.files.len(), bytes: plan.files.iter().map(|j| j.size).sum(), done_files: AtomicUsize::new(0), done_bytes: AtomicU64::new(0) }),
    };
    let (s, d) = (&shared, drive.as_str());
    // At the first failure, files not yet started are left alone.
    let results: Vec<(usize, Option<Result<Uploaded>>)> = futures::stream::iter(plan.files.iter().enumerate())
        .map(|(i, job)| async move {
            if s.stop.load(Ordering::Relaxed) {
                return (i, None);
            }
            let r = upload_one(s, job).await;
            match &r {
                Err(_) => s.stop.store(true, Ordering::Relaxed),
                Ok(u) if !out.json => {
                    s.progress.clear();
                    let _ = stdout(&format!("uploaded {} -> {d}:{} ({})", u.path.display(), u.key, size(u.size)));
                }
                Ok(_) => {}
            }
            (i, Some(r))
        })
        .buffer_unordered(FILES_AT_ONCE)
        .collect()
        .await;
    shared.progress.clear();
    let mut results = results;
    results.sort_by_key(|(i, _)| *i);
    let (mut files, mut failed, mut not_attempted) = (Vec::new(), Vec::new(), 0);
    for (i, r) in results {
        match r {
            Some(Ok(u)) => files.push(u),
            Some(Err(f)) if !f.is("Cancelled") => failed.push((Some(plan.files[i].local.clone()), plan.files[i].key.clone(), f)),
            _ => not_attempted += 1,
        }
    }
    let mut folders = Vec::new();
    for key in plan.folders.iter().map(|f| &f.key) {
        if !failed.is_empty() {
            not_attempted += 1;
            continue;
        }
        match client.put_object(&drive, key, Bytes::new(), Default::default()).await {
            Ok(w) => folders.push(json!({ "key": key, "versionId": w.version_id })),
            Err(e) => failed.push((None, key.clone(), Failure::from(e).context(format!("{drive}:{key}")))),
        }
    }
    let bytes: u64 = files.iter().map(|f| f.size).sum();
    let failed_json: Vec<_> = failed.iter().map(|(p, k, f)| json!({ "path": p, "key": k, "error": f.to_json()["error"] })).collect();
    let summary = json!({
        "drive": drive, "folder": folder, "files": files, "folders": folders, "bytes": bytes,
        "skipped": plan.skipped, "failed": failed_json, "notAttempted": not_attempted,
    });
    let elapsed = started.elapsed().as_secs_f64();
    out.emit(&summary, || {
        let mut line = format!("Uploaded {} ({})", count(files.len(), "file"), size(bytes));
        if !folders.is_empty() {
            line.push_str(&format!(" and {}", count(folders.len(), "empty folder")));
        }
        line.push_str(&format!(" to {drive}:{folder} in {elapsed:.1} s."));
        if !failed.is_empty() || not_attempted > 0 {
            line.push_str(&format!(" {} failed; {} not attempted.", failed.len(), not_attempted));
        }
        line
    })?;
    match failed.into_iter().next() {
        None => Ok(()),
        Some((_, _, first)) => {
            let message = format!("{} of {} failed; the first: {}", failed_json.len(), plan.files.len() + plan.folders.len(), first.message);
            Err(Failure { message, ..first })
        }
    }
}

/// Hands the upload to the daemon if one runs, or with `--detach` (starting one); else uploads in
/// the foreground.
pub async fn upload(conn: &Connection, out: Out, args: &UploadArgs) -> Result<()> {
    let place = Place::new();
    let running = match &place {
        Ok(p) => p.status().await?,
        Err(_) => None,
    };
    match running {
        Some(s) => through_daemon(conn, out, args, &place?, &s).await,
        None if args.detach => {
            let place = place?;
            let (s, _) = daemon::ensure_started(conn, &DaemonOptions::default(), &place).await?;
            through_daemon(conn, out, args, &place, &s).await
        }
        None => foreground(&conn.client(|n| std::env::var(n).ok())?, out, args).await,
    }
}

/// The name a batch goes by: its first source's, as Space names one.
fn label(sources: &[PathBuf]) -> String {
    let name = |p: &PathBuf| std::fs::canonicalize(p).ok().and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| p.display().to_string());
    match sources {
        [one] => name(one),
        [first, rest @ ..] => format!("{} and {} more", name(first), rest.len()),
        [] => String::new(),
    }
}

async fn through_daemon(conn: &Connection, out: Out, args: &UploadArgs, place: &Place, status: &api::Status) -> Result<()> {
    // The daemon uploads with the connection it was started with: one given here must be it.
    if let Some(c) = conn.config(|n| std::env::var(n).ok())? {
        let d = &status.daemon;
        if c.endpoint.trim_end_matches('/') != d.endpoint.trim_end_matches('/') || c.access_key_id != d.access_key_id {
            return Err(Failure::new(
                "DaemonMismatch",
                format!(
                    "the daemon uploads to {} with key {}, not to {} with key {}: `void daemon restart` with this connection switches it",
                    d.endpoint, d.access_key_id, c.endpoint, c.access_key_id
                ),
            ));
        }
    }
    let (dest, sources) = args.paths.split_last().expect("clap requires two paths");
    let (drive, folder) = parse_destination(&dest.to_string_lossy())?;
    let plan = plan(sources, &folder)?;
    for s in &plan.skipped {
        if !out.json {
            warn(&format!("skipped {}: {}", s.path.display(), s.reason));
        }
    }
    let absolute = |p: &Path| std::path::absolute(p).map_err(|e| Failure::local(p, e));
    let mut files = Vec::new();
    let mut local: HashMap<String, PathBuf> = HashMap::new();
    for (path, key) in plan.files.iter().map(|j| (&j.local, &j.key)).chain(plan.folders.iter().map(|f| (&f.local, &f.key))) {
        let path = absolute(path)?;
        local.insert(key.clone(), path.clone());
        files.push(api::NewFile { path: path.display().to_string(), key: key.clone() });
    }
    if files.is_empty() {
        return Err(Failure::invalid("nothing to upload"));
    }
    let q = place.client.enqueue(&api::NewBatch { label: label(sources), drive, files }).await.map_err(daemon::failure)?;
    if args.detach {
        let n = plan.files.len();
        return out.emit(
            &json!({ "batch": q.batch, "drive": q.drive, "folder": folder, "items": q.items, "bytes": q.bytes, "skipped": plan.skipped, "detached": true }),
            || format!("Queued {} ({}) to {}:{folder} as batch {}: `void uploads --watch` follows it.", count(n, "file"), size(q.bytes), q.drive, q.batch),
        );
    }
    follow(place, out, &q, &folder, &plan.skipped, &local).await
}

/// How long a batch is followed while the daemon doesn't answer (it may be restarting).
const DAEMON_GONE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Follows a batch the daemon has until nothing of it is queued or uploading.
async fn follow(place: &Place, out: Out, q: &api::Queued, folder: &str, skipped: &[Skipped], local: &HashMap<String, PathBuf>) -> Result<()> {
    use voidfs_client::{Op, State};
    let started = Instant::now();
    let tty = std::io::stderr().is_terminal();
    let mut gone: Option<Instant> = None;
    let mut said = std::collections::HashSet::new();
    let list = loop {
        let list = match place.client.uploads(true, Some(q.batch)).await {
            Ok(l) => l,
            Err(voidfs_daemon::ClientError::NotRunning { .. }) if gone.get_or_insert_with(Instant::now).elapsed() < DAEMON_GONE_WAIT => {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            }
            Err(e) => {
                return Err(daemon::failure(e).context(format!("batch {} waits in the daemon's journal, and goes on when it starts again", q.batch)));
            }
        };
        gone = None;
        let items = &list.queue.items;
        for i in items.iter().filter(|i| i.state == State::Done && i.op == Op::Put) {
            if !out.json && said.insert(i.id) {
                clear_line(tty);
                let from = local.get(&i.key).map_or_else(|| i.key.clone(), |p| p.display().to_string());
                stdout(&format!("uploaded {from} -> {}:{} ({})", i.drive, i.key, size(i.size)))?;
            }
        }
        let open = items.iter().filter(|i| matches!(i.state, State::Queued | State::Uploading)).count();
        if open == 0 {
            break list;
        }
        if tty && !out.json {
            let (bytes, sent) = items.iter().fold((0, 0), |(b, s), i| (b + i.size, s + if i.state == State::Done { i.size } else { i.sent }));
            let paused = if items.iter().any(|i| i.paused && !i.state.finished()) { " · paused: `void uploads resume`" } else { "" };
            let pct = (sent * 100).checked_div(bytes).unwrap_or(100);
            eprint!("\r\x1b[2K{} of {} up · {pct}% · {}/s{paused}", items.len() - open, count(items.len(), "item"), size(list.rate));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    clear_line(tty);
    let items = &list.queue.items;
    let file = |i: &voidfs_client::Item| json!({ "path": local.get(&i.key), "key": i.key, "size": i.size, "versionId": i.version, "conflict": i.conflict });
    let files: Vec<_> = items.iter().filter(|i| i.state == State::Done && i.op == Op::Put).map(file).collect();
    let folders: Vec<_> = items.iter().filter(|i| i.state == State::Done && i.op == Op::Folder).map(|i| json!({ "key": i.key, "versionId": i.version })).collect();
    let failed: Vec<_> = items.iter().filter(|i| i.state == State::Failed).map(|i| json!({ "path": local.get(&i.key), "key": i.key, "error": i.error })).collect();
    let cancelled = items.iter().filter(|i| i.state == State::Cancelled).count();
    let bytes: u64 = items.iter().filter(|i| i.state == State::Done && i.op == Op::Put).map(|i| i.size).sum();
    let summary = json!({
        "batch": q.batch, "drive": q.drive, "folder": folder, "files": files, "folders": folders, "bytes": bytes,
        "skipped": skipped, "failed": failed, "cancelled": cancelled,
    });
    let elapsed = started.elapsed().as_secs_f64();
    out.emit(&summary, || {
        let mut line = format!("Uploaded {} ({})", count(files.len(), "file"), size(bytes));
        if !folders.is_empty() {
            line.push_str(&format!(" and {}", count(folders.len(), "empty folder")));
        }
        line.push_str(&format!(" to {}:{folder} in {elapsed:.1} s, as batch {}.", q.drive, q.batch));
        if cancelled > 0 {
            line.push_str(&format!(" {cancelled} cancelled."));
        }
        line
    })?;
    match items.iter().find(|i| i.state == State::Failed) {
        None => Ok(()),
        Some(first) => Err(Failure::new(
            "UploadFailed",
            format!(
                "{} of {} failed, and wait in the daemon (`void uploads resume --batch {}` tries them again); the first, {}: {}",
                failed.len(),
                items.len(),
                q.batch,
                first.key,
                first.error.as_deref().unwrap_or("failed")
            ),
        )),
    }
}

fn clear_line(tty: bool) {
    if tty {
        eprint!("\r\x1b[2K");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_name_a_drive_and_a_folder() {
        assert_eq!(parse_destination("footage:").unwrap(), ("footage".into(), "".into()));
        assert_eq!(parse_destination("footage:/").unwrap(), ("footage".into(), "".into()));
        assert_eq!(parse_destination("footage:/cuts").unwrap(), ("footage".into(), "cuts/".into()));
        assert_eq!(parse_destination("d-1:cuts/2026/").unwrap(), ("d-1".into(), "cuts/2026/".into()));
        assert_eq!(parse_destination("footage").unwrap_err().exit, 2);
        assert_eq!(parse_destination(":/cuts").unwrap_err().exit, 2);
    }

    #[test]
    fn parts_grow_so_that_no_file_takes_more_than_ten_thousand() {
        assert_eq!(part_size(100 * MIB, PART_SIZE), PART_SIZE);
        assert_eq!(part_size(160_000 * MIB, PART_SIZE), PART_SIZE);
        assert_eq!(part_size(160_000 * MIB + 1, PART_SIZE), 17 * MIB);
        let five_tib = 5u64 << 40;
        assert!(five_tib.div_ceil(part_size(five_tib, PART_SIZE)) <= MAX_PARTS);
    }

    #[test]
    fn folders_upload_under_their_name_with_empty_ones_kept_and_links_skipped() {
        let root = std::env::temp_dir().join(format!("void-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("renders/empty")).unwrap();
        std::fs::create_dir_all(root.join("renders/a")).unwrap();
        std::fs::write(root.join("renders/a/1.txt"), "1").unwrap();
        std::fs::write(root.join("clip.mov"), "clip").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("clip.mov"), root.join("renders/link")).unwrap();
        let p = plan(&[root.join("clip.mov"), root.join("renders/")], "cuts/").unwrap();
        let keys: Vec<&str> = p.files.iter().map(|j| j.key.as_str()).collect();
        assert_eq!(keys, ["cuts/clip.mov", "cuts/renders/a/1.txt"]);
        assert_eq!(p.folders.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), ["cuts/renders/empty/"]);
        assert_eq!(p.folders[0].local, root.join("renders/empty"));
        #[cfg(unix)]
        assert_eq!(p.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(), ["a symbolic link"]);
        assert_eq!(p.files[0].size, 4);
        assert!(p.files[0].mtime.as_deref().is_some_and(|t| t.ends_with('Z')));
        let e = plan(&[root.join("clip.mov"), root.join("clip.mov")], "").unwrap_err();
        assert!(e.message.contains("would both upload to clip.mov"), "{}", e.message);
        assert_eq!(plan(&[root.join("missing")], "").unwrap_err().code, "LocalFileError");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
