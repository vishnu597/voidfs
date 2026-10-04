// SPDX-License-Identifier: Apache-2.0
//! Publishing a run of journal entries: one key's changes, in order, coalesced into as few
//! requests as they allow, each guarded by the version it was based on.
//!
//! **The `412` rule** (item 3's design): when the object changed since the change was based on
//! it, the local version is published anyway, since every version stays in the history, and the
//! conflict is recorded. A put first checks whether the version it collided with is its own,
//! written by an attempt whose answer was lost: each put carries a marker naming its entry.

use std::collections::BTreeMap;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use bytes::Bytes;
use futures::StreamExt;
use tokio::sync::{Notify, Semaphore};
use voidfs_sdk::{AttributesUpdate, Client, Edit, PutOptions, Preconditions, ReadOptions, RenameOptions, WriteOptions};

use crate::error::{Error, Result};
use crate::journal::{self, Attrs, Entry, Op};
use crate::store::Store;

/// The user metadata a put carries: `<state id>.<entry id>`.
pub(crate) const MARKER: &str = "voidfs-entry";

/// What a publish is guarded by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Guard {
    None,
    Absent,
    Version(String),
}

impl Guard {
    fn version(&self) -> Option<String> {
        match self {
            Guard::Version(v) => Some(v.clone()),
            _ => None,
        }
    }
}

/// Why a publish stopped before it finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Why {
    Pause,
    Cancel,
}

/// Asks a running publish to stop at its next request.
#[derive(Clone, Default)]
pub(crate) struct Stop(Arc<(AtomicU8, Notify)>);

impl Stop {
    pub fn signal(&self, why: Why) {
        // A cancel wins over a pause.
        let v = if why == Why::Cancel { 2 } else { 1 };
        self.0.0.fetch_max(v, Ordering::SeqCst);
        self.0.1.notify_waiters();
    }

    pub fn why(&self) -> Option<Why> {
        match self.0.0.load(Ordering::SeqCst) {
            0 => None,
            1 => Some(Why::Pause),
            _ => Some(Why::Cancel),
        }
    }

    /// `f`, unless a stop comes first: then `f` is dropped, which drops its request.
    async fn or<T>(&self, f: impl Future<Output = T>) -> Result<T, Why> {
        let notified = self.0.1.notified();
        if let Some(w) = self.why() {
            return Err(w);
        }
        tokio::select! {
            r = f => Ok(r),
            _ = notified => Err(self.why().unwrap_or(Why::Pause)),
        }
    }
}

pub(crate) enum Outcome {
    Done { version: Option<String>, conflict: Option<String> },
    Failed { error: String, transient: bool },
    Stopped(Why),
}

impl From<Why> for Outcome {
    fn from(w: Why) -> Outcome {
        Outcome::Stopped(w)
    }
}

impl From<Error> for Outcome {
    fn from(e: Error) -> Outcome {
        let transient = match &e {
            Error::Fetch(e) => e.status().is_none_or(|s| s >= 500 || s == 429),
            Error::Io(_) | Error::Db(_) => true,
            _ => false,
        };
        Outcome::Failed { error: e.to_string(), transient }
    }
}

impl From<voidfs_sdk::Error> for Outcome {
    fn from(e: voidfs_sdk::Error) -> Outcome {
        Error::from(e).into()
    }
}

/// What a publish needs from the queue.
pub(crate) struct Ctx {
    pub client: Client,
    pub store: Arc<Store>,
    pub state_id: String,
    pub part_size: u64,
    pub multipart_from: u64,
    pub parts_at_once: usize,
    /// Files from this size may go as direct uploads.
    pub direct_from: u64,
    /// Bodies in flight, in KiB, of `memory_kib` at most.
    pub memory: Arc<Semaphore>,
    pub memory_kib: u32,
    pub stop: Stop,
    /// Bytes sent, for progress.
    pub sent: Arc<AtomicU64>,
    /// The put's entry was being sent when the client stopped: it may have landed.
    pub may_have_landed: bool,
    /// The multipart upload the entry has open, which the queue keeps with the entry.
    pub upload_id: Arc<std::sync::Mutex<Option<String>>>,
}

fn is_412(e: &voidfs_sdk::Error) -> bool {
    e.status() == Some(412)
}

/// Runs `f` on a blocking thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await?
}

/// Publishes `run`, which the queue built: a put and the writes after it, or writes alone, or
/// one change of another kind.
pub(crate) async fn publish(ctx: &Ctx, run: &[Entry], guard: Guard) -> Outcome {
    let first = &run[0];
    let r = match first.op {
        Op::Put => put(ctx, run, guard).await,
        Op::Write | Op::Truncate => patch(ctx, run, guard).await,
        Op::Rename => rename(ctx, first, guard).await,
        Op::Delete => delete(ctx, first, guard).await,
        Op::Folder => folder(ctx, first).await,
        Op::Attrs => attrs(ctx, first, guard).await,
    };
    match r {
        Ok(o) | Err(o) => o,
    }
}

type Step<T> = std::result::Result<T, Outcome>;

fn done(version: Option<String>, conflict: Option<String>) -> Step<Outcome> {
    Ok(Outcome::Done { version, conflict })
}

fn conflict(e: &voidfs_sdk::Error) -> Option<String> {
    Some(e.current_version_id().unwrap_or("?").to_owned())
}

/// The version at the key, if its put was this entry's.
async fn landed(ctx: &Ctx, e: &Entry) -> Step<Option<String>> {
    match ctx.stop.or(ctx.client.head_object(&e.drive, &e.key, ReadOptions::default())).await? {
        Ok(m) if m.metadata.get(MARKER).is_some_and(|v| *v == marker(ctx, e)) => Ok(Some(m.version_id)),
        Ok(_) => Ok(None),
        Err(err) if err.status() == Some(404) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn marker(ctx: &Ctx, e: &Entry) -> String {
    format!("{}.{}", ctx.state_id, e.id)
}

fn put_opts(ctx: &Ctx, e: &Entry, attrs: &Attrs, guard: &Guard) -> PutOptions {
    PutOptions {
        content_type: attrs.content_type.clone(),
        if_version: guard.version(),
        if_match: None,
        if_none_match_any: *guard == Guard::Absent,
        mtime: attrs.mtime.clone(),
        mode: attrs.mode,
        metadata: BTreeMap::from([(MARKER.to_owned(), marker(ctx, e))]),
    }
}

/// Sets what a put can't carry: extended attributes, as a version of their own.
async fn xattrs_after(ctx: &Ctx, e: &Entry, attrs: &Attrs, version: String) -> Step<String> {
    if attrs.xattrs.is_empty() && attrs.remove_xattrs.is_empty() {
        return Ok(version);
    }
    let update = AttributesUpdate {
        set_xattrs: attrs.xattrs.iter().map(|(k, v)| (k.clone(), Bytes::from(v.clone()))).collect(),
        remove_xattrs: attrs.remove_xattrs.clone(),
        ..Default::default()
    };
    let w = ctx.stop.or(ctx.client.set_attributes(&e.drive, &e.key, update, Preconditions::if_version(version))).await??;
    Ok(w.version_id)
}

async fn put(ctx: &Ctx, run: &[Entry], guard: Guard) -> Step<Outcome> {
    let e = &run[0];
    if ctx.may_have_landed
        && let Some(v) = landed(ctx, e).await?
    {
        return done(Some(xattrs_after(ctx, e, &e.attrs, v).await?), None);
    }
    let src = e.source.clone().ok_or_else(|| Outcome::Failed { error: "a put without bytes".into(), transient: false })?;
    let size = match blocking(move || Ok(std::fs::metadata(&src)?.len())).await {
        Ok(n) => n,
        Err(err) => return Err(Outcome::Failed { error: format!("{}: {err}", e.key), transient: false }),
    };
    // A new file is new bytes, which an ordinary put sends as fast.
    if run.len() == 1 && size >= ctx.direct_from && guard != Guard::Absent
        && let Some(o) = direct(ctx, e, size, &guard).await?
    {
        return Ok(o);
    }
    if run.len() == 1 && size >= ctx.multipart_from {
        return multipart(ctx, e, size, guard).await;
    }
    // Small enough to hold: the put, with the writes after it applied.
    let mut body = read_all(e.source.clone().unwrap()).await?;
    for w in &run[1..] {
        apply(&mut body, w).await?;
    }
    let body = Bytes::from(body);
    let _mem = ctx.hold(body.len() as u64).await?;
    let attrs = &e.attrs;
    let sent = body.len() as u64;
    let r = ctx.stop.or(ctx.client.put_object(&e.drive, &e.key, body.clone(), put_opts(ctx, e, attrs, &guard))).await?;
    let (version, clash) = match r {
        Ok(w) => (w.version_id, None),
        Err(err) if is_412(&err) => {
            // Our own put, answered after we stopped waiting, or someone else's.
            if let Some(v) = landed(ctx, e).await? {
                (v, None)
            } else {
                let w = ctx.stop.or(ctx.client.put_object(&e.drive, &e.key, body, put_opts(ctx, e, attrs, &Guard::None))).await??;
                (w.version_id, conflict(&err))
            }
        }
        Err(err) => return Err(err.into()),
    };
    ctx.sent.store(sent, Ordering::Relaxed);
    done(Some(xattrs_after(ctx, e, attrs, version).await?), clash)
}

impl Ctx {
    /// Waits until `n` more bytes may be held, or the whole budget for a body larger than it.
    async fn hold(&self, n: u64) -> Step<tokio::sync::SemaphorePermit<'_>> {
        let kib = n.div_ceil(1024).clamp(1, self.memory_kib.max(1) as u64) as u32;
        self.memory.acquire_many(kib).await.map_err(|_| Outcome::Stopped(Why::Pause))
    }
}

async fn read_all(path: std::path::PathBuf) -> Step<Vec<u8>> {
    tokio::fs::read(&path).await.map_err(|err| Outcome::Failed { error: format!("{}: {err}", path.display()), transient: false })
}

/// Applies a write or a truncate to a body in memory.
async fn apply(body: &mut Vec<u8>, w: &Entry) -> Step<()> {
    match w.op {
        Op::Write => {
            let data = read_all(w.source.clone().unwrap_or_default()).await?;
            let end = w.offset as usize + data.len();
            if body.len() < end {
                body.resize(end, 0);
            }
            body[w.offset as usize..end].copy_from_slice(&data);
        }
        Op::Truncate => body.resize(w.length as usize, 0),
        _ => {}
    }
    Ok(())
}

async fn patch(ctx: &Ctx, run: &[Entry], guard: Guard) -> Step<Outcome> {
    let e = &run[0];
    let mut edits = Vec::new();
    let mut size = None;
    for w in run {
        match w.op {
            Op::Write => edits.push(Edit::new(w.offset, read_all(w.source.clone().unwrap_or_default()).await?)),
            Op::Truncate => size = Some(w.length),
            _ => {}
        }
    }
    let total: u64 = edits.iter().map(|e| e.data.len() as u64).sum();
    let _mem = ctx.hold(total).await?;
    let opts = WriteOptions { size, if_version: guard.version(), ..Default::default() };
    let r = if edits.is_empty() {
        // A truncate alone: an empty write at 0 with the size.
        ctx.stop.or(ctx.client.write_at(&e.drive, &e.key, 0, Bytes::new(), opts)).await?
    } else {
        ctx.stop.or(ctx.client.patch(&e.drive, &e.key, &edits, opts)).await?
    };
    match r {
        Ok(w) => {
            ctx.sent.store(total, Ordering::Relaxed);
            done(Some(w.version_id), None)
        }
        Err(err) if is_412(&err) => {
            // The local version is the base with these edits: build it, and put it.
            let base = guard.version().unwrap_or_default();
            let o = ctx.stop.or(ctx.client.get_object(&e.drive, &e.key, ReadOptions { version_id: Some(base), ..Default::default() })).await??;
            let mut body = o.body.to_vec();
            for w in run {
                apply(&mut body, w).await?;
            }
            let w = ctx.stop.or(ctx.client.put_object(&e.drive, &e.key, body, put_opts(ctx, e, &Attrs::default(), &Guard::None))).await??;
            done(Some(w.version_id), conflict(&err))
        }
        Err(err) => Err(err.into()),
    }
}

async fn rename(ctx: &Ctx, e: &Entry, guard: Guard) -> Step<Outcome> {
    let to = e.to_key.clone().unwrap_or_default();
    let opts = |g: &Guard| RenameOptions { replace: e.replace, if_version: g.version(), if_match: None };
    match ctx.stop.or(ctx.client.rename(&e.drive, &e.key, &to, opts(&guard))).await? {
        Ok(w) => done(Some(w.version_id), None),
        Err(err) if is_412(&err) => {
            let w = ctx.stop.or(ctx.client.rename(&e.drive, &e.key, &to, opts(&Guard::None))).await??;
            done(Some(w.version_id), conflict(&err))
        }
        // Moved already, by an attempt whose answer was lost.
        Err(err) if err.status() == Some(404) && ctx.may_have_landed => done(None, None),
        Err(err) => Err(err.into()),
    }
}

async fn delete(ctx: &Ctx, e: &Entry, guard: Guard) -> Step<Outcome> {
    let pre = |g: &Guard| Preconditions { if_version: g.version(), if_match: None };
    match ctx.stop.or(ctx.client.delete_object(&e.drive, &e.key, pre(&guard))).await? {
        Ok(v) => done(v, None),
        Err(err) if is_412(&err) => {
            let v = ctx.stop.or(ctx.client.delete_object(&e.drive, &e.key, pre(&Guard::None))).await??;
            done(v, conflict(&err))
        }
        Err(err) => Err(err.into()),
    }
}

async fn folder(ctx: &Ctx, e: &Entry) -> Step<Outcome> {
    let key = if e.key.ends_with('/') { e.key.clone() } else { format!("{}/", e.key) };
    let opts = PutOptions { mtime: e.attrs.mtime.clone(), mode: e.attrs.mode, ..Default::default() };
    let w = ctx.stop.or(ctx.client.put_object(&e.drive, &key, Bytes::new(), opts)).await??;
    done(Some(w.version_id), None)
}

async fn attrs(ctx: &Ctx, e: &Entry, guard: Guard) -> Step<Outcome> {
    let a = &e.attrs;
    let update = || AttributesUpdate {
        mtime: a.mtime.clone(),
        mode: a.mode,
        set_xattrs: a.xattrs.iter().map(|(k, v)| (k.clone(), Bytes::from(v.clone()))).collect(),
        remove_xattrs: a.remove_xattrs.clone(),
        flags: None,
        content_type: a.content_type.clone(),
    };
    let pre = |g: &Guard| Preconditions { if_version: g.version(), if_match: None };
    match ctx.stop.or(ctx.client.set_attributes(&e.drive, &e.key, update(), pre(&guard))).await? {
        Ok(w) => done(Some(w.version_id), None),
        Err(err) if is_412(&err) => {
            let w = ctx.stop.or(ctx.client.set_attributes(&e.drive, &e.key, update(), pre(&Guard::None))).await??;
            done(Some(w.version_id), conflict(&err))
        }
        Err(err) => Err(err.into()),
    }
}

/// A file cut into shards as format §4.1 says (each shard's hash and length, in order), and the
/// SHA-256 of the whole, read in pieces so that it is never held whole.
fn cut(path: &std::path::Path) -> Result<(Vec<(voidfs_core::ids::ShardHash, u64)>, String)> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut chunker = voidfs_core::chunk::StreamChunker::new(voidfs_core::chunk::Params::DEFAULT);
    let mut whole = sha2::Sha256::new();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        whole.update(&buf[..n]);
        out.extend(chunker.push(&buf[..n]).into_iter().map(|s| (s.hash, s.bytes.len() as u64)));
    }
    out.extend(chunker.finish().into_iter().map(|s| (s.hash, s.bytes.len() as u64)));
    Ok((out, hex::encode(whole.finalize())))
}

/// A file sent as a direct upload (protocol §4.11): only the shards the drive's pool lacks go,
/// straight to the bucket, and a commit makes the version, guarded and marked as a put is. `None`
/// when it should go the ordinary way: the server doesn't offer direct uploads, the pool holds
/// less than half of the file, or a step failed with anything but a `409` or a `412`.
async fn direct(ctx: &Ctx, e: &Entry, size: u64, guard: &Guard) -> Step<Option<Outcome>> {
    let src = e.source.clone().unwrap_or_default();
    let path = src.clone();
    let (shards, sha) = blocking(move || cut(&path)).await?;
    if shards.len() > voidfs_sdk::direct::MAX_SHARDS {
        return Ok(None);
    }
    let stands = |err: &voidfs_sdk::Error| err.status() == Some(409);
    let plan = match ctx.stop.or(ctx.client.plan_upload(&e.drive, &e.key, &shards)).await? {
        Ok(p) => p,
        Err(err) if stands(&err) => return Err(err.into()),
        Err(_) => return Ok(None),
    };
    let missing: u64 = plan.upload.iter().map(|p| p.length).sum();
    if missing * 2 > size {
        return Ok(None);
    }
    ctx.sent.store(size - missing, Ordering::Relaxed);
    let mut at = std::collections::HashMap::new();
    let mut off = 0;
    for (h, n) in &shards {
        at.entry(h.to_hex()).or_insert(off);
        off += n;
    }
    let file = Arc::new(std::fs::File::open(&src).map_err(|err| Outcome::Failed { error: format!("{}: {err}", src.display()), transient: false })?);
    let sends = futures::stream::iter(plan.upload.iter().cloned()).map(|p| {
        let (file, off) = (file.clone(), at.get(&p.hash).copied().unwrap_or(0));
        async move {
            let len = p.length;
            let _mem = ctx.hold(len).await?;
            let body = blocking(move || {
                let mut buf = vec![0u8; len as usize];
                file.read_exact_at(&mut buf, off)?;
                Ok(Bytes::from(buf))
            })
            .await?;
            let sent = ctx.stop.or(ctx.client.upload_shard(&p, body)).await?;
            if sent.is_ok() {
                ctx.sent.fetch_add(len, Ordering::Relaxed);
            }
            Ok::<_, Outcome>(sent.is_ok())
        }
    });
    let mut sends = sends.buffer_unordered(voidfs_sdk::direct::SHARD_UPLOADS);
    while let Some(r) = sends.next().await {
        if !r? {
            return Ok(None);
        }
    }
    drop(sends);
    let commit = |g: &Guard| ctx.client.commit_upload(&e.drive, &e.key, &plan.token, &shards, &sha, put_opts(ctx, e, &e.attrs, g));
    let (version, clash) = match ctx.stop.or(commit(guard)).await? {
        Ok(w) => (w.version_id, None),
        Err(err) if is_412(&err) => match landed(ctx, e).await? {
            Some(v) => (v, None),
            None => match ctx.stop.or(commit(&Guard::None)).await? {
                Ok(w) => (w.version_id, conflict(&err)),
                Err(again) if stands(&again) => return Err(again.into()),
                Err(_) => return Ok(None),
            },
        },
        Err(err) if stands(&err) => return Err(err.into()),
        Err(_) => return Ok(None),
    };
    done(Some(xattrs_after(ctx, e, &e.attrs, version).await?), clash).map(Some)
}

/// The part size for `size` bytes: `part_size`, or larger to stay within 10,000 parts.
fn part_size(ctx: &Ctx, size: u64) -> u64 {
    ctx.part_size.max(size.div_ceil(10_000))
}

/// A large file in parts. The upload's id and each finished part are recorded as they happen,
/// so that a publish stopped by a pause or a restart goes on from where it was.
async fn multipart(ctx: &Ctx, e: &Entry, size: u64, guard: Guard) -> Step<Outcome> {
    let src = e.source.clone().unwrap_or_default();
    let ps = part_size(ctx, size);
    let count = size.div_ceil(ps).max(1) as u32;
    let mut have: BTreeMap<u32, (String, u64)> = BTreeMap::new();
    let mut upload = ctx.upload_id.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(id) = &upload {
        // Check what the server holds against what was recorded.
        let (store, eid) = (ctx.store.clone(), e.id);
        let recorded = blocking(move || store.with(|c| journal::parts(c, eid))).await?;
        match ctx.stop.or(ctx.client.list_parts(&e.drive, &e.key, id)).await? {
            Ok(server) => {
                for (n, etag, len) in recorded {
                    if server.iter().any(|p| p.number == n && p.etag == etag && p.size == len) {
                        have.insert(n, (etag, len));
                    }
                }
            }
            Err(err) if err.code() == Some("NoSuchUpload") => {
                if let Some(v) = landed(ctx, e).await? {
                    return done(Some(xattrs_after(ctx, e, &e.attrs, v).await?), None);
                }
                upload = None;
                *ctx.upload_id.lock().unwrap_or_else(|p| p.into_inner()) = None;
            }
            Err(err) => return Err(err.into()),
        }
    }
    let id = match upload {
        Some(id) => id,
        None => {
            let id = ctx.stop.or(ctx.client.create_multipart_upload(&e.drive, &e.key, put_opts(ctx, e, &e.attrs, &Guard::None))).await??;
            let (store, eid, id2) = (ctx.store.clone(), e.id, id.clone());
            blocking(move || {
                store.with(|c| {
                    journal::clear_parts(c, eid)?;
                    c.execute("UPDATE entries SET upload_id = ?2 WHERE id = ?1", rusqlite::params![eid, id2]).map(drop)
                })
            })
            .await?;
            *ctx.upload_id.lock().unwrap_or_else(|p| p.into_inner()) = Some(id.clone());
            id
        }
    };
    ctx.sent.store(have.values().map(|(_, n)| n).sum(), Ordering::Relaxed);
    let missing: Vec<u32> = (1..=count).filter(|n| !have.contains_key(n)).collect();
    let file = Arc::new(std::fs::File::open(&src).map_err(|err| Outcome::Failed { error: format!("{}: {err}", src.display()), transient: false })?);
    let sends = futures::stream::iter(missing).map(|n| {
        let file = file.clone();
        let id = id.clone();
        async move {
            let off = (n as u64 - 1) * ps;
            let len = ps.min(size - off);
            let _mem = ctx.hold(len).await?;
            let body = blocking(move || {
                let mut buf = vec![0u8; len as usize];
                file.read_exact_at(&mut buf, off)?;
                Ok(Bytes::from(buf))
            })
            .await?;
            let etag = ctx.stop.or(ctx.client.upload_part(&e.drive, &e.key, &id, n, body)).await??;
            let (store, eid, etag2) = (ctx.store.clone(), e.id, etag.clone());
            blocking(move || store.with(|c| journal::add_part(c, eid, n, &etag2, len))).await?;
            ctx.sent.fetch_add(len, Ordering::Relaxed);
            Ok::<_, Outcome>((n, etag, len))
        }
    });
    let mut sends = sends.buffer_unordered(ctx.parts_at_once.max(1));
    while let Some(r) = sends.next().await {
        let (n, etag, len) = r?;
        have.insert(n, (etag, len));
    }
    let parts: Vec<(u32, String)> = have.iter().map(|(n, (etag, _))| (*n, etag.clone())).collect();
    let pre = |g: &Guard| Preconditions { if_version: g.version(), if_match: None };
    // A completion takes a version or an ETag to check, not "nothing there": a new file's goes
    // unguarded.
    let guard = if guard == Guard::Absent { Guard::None } else { guard };
    let (version, clash) = match ctx.stop.or(ctx.client.complete_multipart_upload(&e.drive, &e.key, &id, &parts, pre(&guard))).await? {
        Ok(w) => (w.version_id, None),
        Err(err) if is_412(&err) => {
            let w = ctx.stop.or(ctx.client.complete_multipart_upload(&e.drive, &e.key, &id, &parts, pre(&Guard::None))).await??;
            (w.version_id, conflict(&err))
        }
        Err(err) if err.code() == Some("NoSuchUpload") => match landed(ctx, e).await? {
            Some(v) => (v, None),
            None => return Err(err.into()),
        },
        Err(err) => return Err(err.into()),
    };
    done(Some(xattrs_after(ctx, e, &e.attrs, version).await?), clash)
}
