// SPDX-License-Identifier: Apache-2.0
//! Object-level operations (protocol §3, §4).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use futures::StreamExt;
use http::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use voidfs_core::chunk::{Shard, StreamChunker};
use voidfs_core::content::{self, EditError, Edited, Source};
use voidfs_core::ids::{ObjectId, ShardHash, Timestamp, VersionId};
use voidfs_core::model::{Attrs, ContentDescriptor, Extent, HistoryRow, Kind, MAX_DATA_BYTES, ObjectRecord, Op};
use voidfs_core::names::Key;
use voidfs_core::ops::{self, AttrsPatch};
use voidfs_core::state::DriveState;

use super::util::{
    BodyReader, Ctx, MAX_EXTENSION_BODY, MAX_PUT, MAX_SMALL_BODY, S3_NS, empty, format_mode, http_date, iso, json, parse_mode, xml, xml_escape,
};
use super::{App, S3Error};
use crate::pool::{CommitError, Drive, Pool};
use crate::sigv4::Scope;

const OBJECT_KNOWN: &[&str] = &[
    "versionId", "partNumber", "uploadId", "uploads", "x-voidfs-versions", "x-voidfs-attrs", "x-voidfs-all", "x-voidfs-write",
    "x-voidfs-splice", "x-voidfs-patch", "x-voidfs-rename", "x-voidfs-restore", "max-keys", "continuation-token", "max-parts",
    "part-number-marker",
];

pub async fn dispatch(app: &Arc<App>, ctx: &Ctx, body: Body) -> Result<Response, S3Error> {
    if let Some(unknown) = ctx.query.unknown(OBJECT_KNOWN) {
        return Err(S3Error::not_implemented(format!("?{unknown} is not supported")));
    }
    let q = &ctx.query;
    let d = ctx.drive(app)?;
    match ctx.method {
        Method::GET | Method::HEAD => {
            ctx.require(Scope::Read)?;
            if q.has("x-voidfs-versions") && ctx.method == Method::GET {
                versions(ctx, &d)
            } else if q.has("x-voidfs-attrs") && ctx.method == Method::GET {
                get_attrs(ctx, &d)
            } else if q.has("uploadId") {
                list_parts(app, ctx, &d).await
            } else {
                get(app, ctx, &d).await
            }
        }
        Method::PUT => {
            ctx.require(Scope::Write)?;
            if q.has("partNumber") && q.has("uploadId") {
                upload_part(app, ctx, &d, body).await
            } else if q.has("x-voidfs-write") {
                write(app, ctx, &d, body).await
            } else if q.has("x-voidfs-splice") {
                splice(app, ctx, &d, body).await
            } else if q.has("x-voidfs-rename") {
                rename(app, ctx, &d).await
            } else if ctx.header("x-amz-copy-source").is_some() {
                copy(app, ctx, &d).await
            } else {
                put(app, ctx, &d, body).await
            }
        }
        Method::POST => {
            ctx.require(Scope::Write)?;
            if q.has("uploads") {
                create_upload(app, ctx, &d).await
            } else if q.has("uploadId") {
                complete_upload(app, ctx, &d, body).await
            } else if q.has("x-voidfs-patch") {
                patch(app, ctx, &d, body).await
            } else if q.has("x-voidfs-restore") {
                restore(app, ctx, &d).await
            } else if q.has("x-voidfs-attrs") {
                post_attrs(app, ctx, &d, body).await
            } else {
                Err(S3Error::not_implemented("POST with these parameters is not supported"))
            }
        }
        Method::DELETE => {
            ctx.require(Scope::Write)?;
            if q.has("uploadId") {
                abort_upload(app, ctx, &d).await
            } else if q.has("versionId") {
                Err(S3Error::not_implemented("history is immutable; versions cannot be deleted"))
            } else {
                delete(app, ctx, &d).await
            }
        }
        _ => Err(S3Error::not_implemented(format!("{} is not supported", ctx.method))),
    }
}

// ---------------------------------------------------------------------------------------------
// Reads

/// What a read serves: the current record, or a past version.
struct View {
    oid: ObjectId,
    kind: Kind,
    version: VersionId,
    time: Timestamp,
    size: u64,
    etag: String,
    content: Option<ContentDescriptor>,
    attrs: Attrs,
}

impl View {
    fn of_record(r: &ObjectRecord) -> View {
        View {
            oid: r.oid.clone(),
            kind: r.kind,
            version: r.head,
            time: r.time,
            size: r.size,
            etag: r.etag.clone(),
            content: r.content.clone(),
            attrs: r.attrs.clone(),
        }
    }

    fn of_row(s: &DriveState, r: &HistoryRow) -> View {
        View {
            oid: r.oid.clone(),
            kind: s.kind(&r.oid).unwrap_or(Kind::File),
            version: r.version,
            time: r.time,
            size: r.size,
            etag: r.etag.clone(),
            content: r.content.clone(),
            attrs: r.attrs.clone(),
        }
    }
}

fn parse_key(k: &str) -> Result<Key, S3Error> {
    Key::parse(k).map_err(|e| S3Error::invalid(e.to_string()))
}

fn resolve_view(ctx: &Ctx, s: &DriveState) -> Result<View, S3Error> {
    let key = parse_key(ctx.key())?;
    let no_version = || S3Error::new(404, "NoSuchVersion", "The specified version does not exist");
    let version = ctx.query.get("versionId").filter(|v| *v != "null");
    let as_of = ctx.header("x-voidfs-as-of");
    if version.is_some() && as_of.is_some() {
        return Err(S3Error::invalid("give versionId or x-voidfs-as-of, not both"));
    }
    if let Some(v) = version {
        let v: VersionId = v.parse().map_err(|_| no_version())?;
        let row = s.version(&v).ok_or_else(no_version)?;
        // The version must belong to the object at this key, or to a deleted object last there.
        let at_key = s.lookup(&key);
        let ok = at_key.as_ref() == Some(&row.oid) || s.removed_row(&row.oid).is_some_and(|r| r.key == key.render());
        if !ok {
            return Err(no_version());
        }
        return Ok(View::of_row(s, row));
    }
    let oid = s.lookup(&key).ok_or_else(S3Error::no_key)?;
    if let Some(t) = as_of {
        let t: Timestamp = t.parse().map_err(|_| S3Error::invalid("x-voidfs-as-of is not an RFC 3339 timestamp"))?;
        let row = s.as_of(&oid, t).ok_or_else(no_version)?;
        return Ok(View::of_row(s, row));
    }
    Ok(View::of_record(s.record(&oid).ok_or_else(S3Error::no_key)?))
}

fn object_headers(mut b: http::response::Builder, v: &View) -> http::response::Builder {
    b = b
        .header("etag", &v.etag)
        .header("last-modified", http_date(v.time))
        .header("x-amz-version-id", v.version.to_string())
        .header("x-voidfs-object-id", v.oid.as_str())
        .header("x-voidfs-kind", match v.kind {
            Kind::File => "file",
            Kind::Folder => "folder",
            Kind::Symlink => "symlink",
        })
        .header("x-voidfs-mtime", v.attrs.mtime.unwrap_or(v.time).to_string())
        .header("x-voidfs-mode", format_mode(v.attrs.mode, v.kind))
        .header("accept-ranges", "bytes")
        .header("content-type", v.attrs.content_type.as_deref().unwrap_or(if v.kind == Kind::Folder {
            "application/x-directory"
        } else {
            "binary/octet-stream"
        }));
    for (k, val) in &v.attrs.meta {
        b = b.header(format!("x-amz-meta-{k}"), val);
    }
    b
}

fn parse_range(h: &str, size: u64) -> Result<Option<(u64, u64)>, S3Error> {
    let Some(spec) = h.strip_prefix("bytes=") else { return Ok(None) };
    if spec.contains(',') {
        return Ok(None); // multiple ranges: serve the whole object, as S3 does
    }
    let invalid = || S3Error::new(416, "InvalidRange", "The requested range is not satisfiable").header("content-range", format!("bytes */{size}"));
    let (a, b) = spec.split_once('-').ok_or_else(invalid)?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().map_err(|_| invalid())?;
            if n == 0 {
                return Err(invalid());
            }
            (size.saturating_sub(n), size.saturating_sub(1))
        }
        (s, "") => (s.parse().map_err(|_| invalid())?, size.saturating_sub(1)),
        (s, e) => {
            let s: u64 = s.parse().map_err(|_| invalid())?;
            let e: u64 = e.parse().map_err(|_| invalid())?;
            if e < s {
                return Ok(None);
            }
            (s, e.min(size.saturating_sub(1)))
        }
    };
    if size == 0 || start >= size {
        return Err(invalid());
    }
    Ok(Some((start, end)))
}

fn not_modified(v: &View) -> Response {
    object_headers(empty(304), v).body(Body::empty()).unwrap()
}

async fn get(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let s = d.snapshot();
    let v = resolve_view(ctx, &s)?;
    let secs = v.time.datetime().timestamp();
    let parse_date = |h: &str| chrono::DateTime::parse_from_rfc2822(h).ok().map(|t| t.timestamp());
    if let Some(m) = ctx.header("if-match") {
        if !m.split(',').any(|e| e.trim() == "*" || e.trim() == v.etag) {
            return Err(S3Error::new(412, "PreconditionFailed", "If-Match did not match"));
        }
    } else if let Some(t) = ctx.header("if-unmodified-since").and_then(parse_date)
        && secs > t {
            return Err(S3Error::new(412, "PreconditionFailed", "the object was modified"));
        }
    if let Some(m) = ctx.header("if-none-match") {
        if m.split(',').any(|e| e.trim() == "*" || e.trim() == v.etag) {
            return Ok(not_modified(&v));
        }
    } else if let Some(t) = ctx.header("if-modified-since").and_then(parse_date)
        && secs <= t {
            return Ok(not_modified(&v));
        }

    let range = match ctx.header("range") {
        Some(h) if v.kind == Kind::File => parse_range(h, v.size)?,
        _ => None,
    };
    let (status, start, len) = match range {
        Some((a, b)) => (206, a, b - a + 1),
        None => (200, 0, v.size),
    };
    let mut b = object_headers(empty(status), &v).header("content-length", len);
    if let Some((a, e)) = range {
        b = b.header("content-range", format!("bytes {a}-{e}/{}", v.size));
    }
    if ctx.method == Method::HEAD || len == 0 {
        return Ok(b.body(Body::empty()).unwrap());
    }
    let extents = app.pool.extents(v.content.as_ref().unwrap_or(&ContentDescriptor::empty())).await?;
    let plan = content::read_plan(&extents, start, len).map_err(|e| S3Error::internal(e.to_string()))?;
    let pool = app.pool.clone();
    let stream = futures::stream::iter(plan)
        .map(move |piece| {
            let pool = pool.clone();
            async move {
                match piece.source {
                    Source::Shard(h) => {
                        let bytes = pool.shard(&h).await.map_err(std::io::Error::other)?;
                        Ok::<_, std::io::Error>(vec![bytes.slice(piece.offset as usize..(piece.offset + piece.len) as usize)])
                    }
                    Source::Data(b) => Ok(vec![b]),
                    Source::Zeros => {
                        const ZEROS: usize = 1 << 20;
                        let mut left = piece.len as usize;
                        let mut out = Vec::new();
                        while left > 0 {
                            let n = left.min(ZEROS);
                            out.push(Bytes::from(vec![0u8; n]));
                            left -= n;
                        }
                        Ok(out)
                    }
                }
            }
        })
        .buffered(8)
        .flat_map(|r: Result<Vec<Bytes>, std::io::Error>| match r {
            Ok(v) => futures::stream::iter(v.into_iter().map(Ok).collect::<Vec<_>>()),
            Err(e) => futures::stream::iter(vec![Err(e)]),
        });
    Ok(b.body(Body::from_stream(stream)).unwrap())
}

// ---------------------------------------------------------------------------------------------
// Whole-object writes

/// The longest a streamed body may take. What it references must be committed within 12 hours
/// of the garbage-collection check its first shards relied on (format §12.4).
const MAX_INGEST: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
/// Shard uploads a body keeps in flight while it is read on...
const INGEST_UPLOADS: usize = 16;
/// ...and their size, at most, unless one shard alone is larger.
const INGEST_BYTES: usize = 32 << 20;
/// Shards at least this large are hashed on a blocking thread, so that the shards of one body
/// hash in parallel while it is read on: a 2 MiB shard takes about a millisecond.
const HASH_ELSEWHERE: usize = 256 << 10;
/// Blocking threads hashing shards at once, across requests: one per CPU. Past that, a shard is
/// hashed where its upload runs, which is as fast when there is no idle core anyway.
static HASHERS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(std::thread::available_parallelism().map_or(4, |n| n.get()))));

/// Streams a body into shards; returns the content's extents. The body is read and cut while
/// the shards cut from it so far hash and upload, up to [`INGEST_UPLOADS`] and [`INGEST_BYTES`]
/// at once.
///
/// With `inline`, a body of at most [`MAX_DATA_BYTES`] is held in its descriptor instead, and
/// uploads nothing (format §5): the body is read that far before anything is cut.
async fn ingest(pool: &Pool, mut reader: BodyReader, inline: bool) -> Result<Vec<Extent>, S3Error> {
    let started = pool.clock.mono();
    let mut chunker = Some(StreamChunker::new(pool.params));
    // One for each shard cut, in order, filled in once it is uploaded.
    let mut extents: Vec<Option<Extent>> = Vec::new();
    // Shards cut and not uploading yet, with their places in `extents`, and the uploads in
    // flight, which answer their extents.
    let mut cut: VecDeque<(usize, Bytes)> = VecDeque::new();
    if inline {
        let mut head = bytes::BytesMut::new();
        while head.len() <= MAX_DATA_BYTES {
            let Some(data) = reader.next().await? else {
                // Nothing is committed until the whole body matched its signature.
                reader.finish()?;
                return Ok(content::inline(&head));
            };
            head.extend_from_slice(&data);
        }
        for b in chunker.as_mut().expect("still reading").push_unhashed(&head) {
            cut.push_back((extents.len(), b));
            extents.push(None);
        }
    }
    let mut uploads = futures::stream::FuturesUnordered::new();
    let mut in_flight = 0;
    loop {
        while let Some((_, b)) = cut.front()
            && (uploads.is_empty() || (uploads.len() < INGEST_UPLOADS && in_flight + b.len() <= INGEST_BYTES))
        {
            let (i, bytes) = cut.pop_front().expect("a shard was just seen");
            in_flight += bytes.len();
            uploads.push(async move {
                let hasher = if bytes.len() < HASH_ELSEWHERE { None } else { HASHERS.clone().try_acquire_owned().ok() };
                let s = match hasher {
                    Some(permit) => {
                        tokio::task::spawn_blocking(move || {
                            let s = Shard::new(bytes);
                            drop(permit);
                            s
                        })
                        .await?
                    }
                    None => Shard::new(bytes),
                };
                pool.write_shards(std::slice::from_ref(&s)).await?;
                anyhow::Ok((i, Extent::Shard { s: s.hash, n: s.bytes.len() as u64 }))
            });
        }
        // The body is read on only once what it gave so far is uploading.
        let reading = chunker.is_some() && cut.is_empty();
        if !reading && uploads.is_empty() {
            break;
        }
        // Reading the body is cancel-safe: a piece is taken from it only when it is returned.
        // An error returns at once, and drops the uploads in flight.
        tokio::select! {
            Some(done) = uploads.next(), if !uploads.is_empty() => {
                let (i, e) = done?;
                in_flight -= e.len() as usize;
                extents[i] = Some(e);
            }
            data = reader.next(), if reading => {
                let shards = match data? {
                    Some(data) => chunker.as_mut().expect("still reading").push_unhashed(&data),
                    None => chunker.take().expect("still reading").finish_unhashed(),
                };
                for b in shards {
                    cut.push_back((extents.len(), b));
                    extents.push(None);
                }
            }
        }
    }
    let extents: Vec<Extent> = extents.into_iter().map(|e| e.expect("every shard was uploaded")).collect();
    // Nothing is committed until the whole body matched its signature.
    reader.finish()?;
    if pool.clock.mono().saturating_sub(started) > MAX_INGEST {
        return Err(S3Error::new(400, "RequestTimeout", "the upload took longer than 6 hours"));
    }
    Ok(content::normalize(extents))
}

fn mutation_response(v: VersionId, s: &DriveState, key: &str) -> http::response::Builder {
    let mut b = empty(200).header("x-amz-version-id", v.to_string());
    if let Some(r) = Key::parse(key).ok().and_then(|k| s.lookup(&k)).and_then(|o| s.record(&o)) {
        b = b.header("etag", &r.etag).header("x-voidfs-size", r.size);
    }
    b
}

async fn commit(app: &App, d: &Arc<Drive>, plan: impl FnMut(&DriveState) -> Result<voidfs_core::model::Txn, CommitError> + Send + 'static) -> Result<(VersionId, Arc<DriveState>), S3Error> {
    Ok(app.pool.commit(d, plan).await?)
}

async fn put(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, body: Body) -> Result<Response, S3Error> {
    let key = ctx.key().to_owned();
    let attrs = ctx.attrs_for_put()?;
    let pre = ctx.precondition();
    let reader = BodyReader::new(body, ctx, MAX_PUT)?;
    let extents = ingest(&app.pool, reader, app.pool.inline_data).await?;
    let desc = app.pool.describe(extents).await?;
    let actor = ctx.actor();
    let (v, s) = commit(app, d, move |st| Ok(ops::put(st, &key, desc.clone(), attrs.clone(), Op::Put, &pre, &actor)?)).await?;
    Ok(mutation_response(v, &s, ctx.key()).body(Body::empty()).unwrap())
}

async fn copy(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>) -> Result<Response, S3Error> {
    let raw = ctx.header("x-amz-copy-source").unwrap_or_default();
    let decoded = percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned();
    let (path, src_version) = match decoded.split_once("?versionId=") {
        Some((p, v)) => (p.to_owned(), Some(v.to_owned())),
        None => (decoded, None),
    };
    let path = path.trim_start_matches('/');
    let (src_bucket, src_key) = path.split_once('/').ok_or_else(|| S3Error::invalid("x-amz-copy-source must be /drive/key"))?;
    let src = ctx.drive_named(app, src_bucket)?;
    let ss = src.snapshot();
    let k = parse_key(src_key)?;
    let (row_version, content, size, src_attrs, kind) = match &src_version {
        Some(v) => {
            let v: VersionId = v.parse().map_err(|_| S3Error::new(404, "NoSuchVersion", "no such version"))?;
            let r = ss.version(&v).ok_or_else(|| S3Error::new(404, "NoSuchVersion", "no such version"))?;
            (r.version, r.content.clone(), r.size, r.attrs.clone(), ss.kind(&r.oid).unwrap_or(Kind::File))
        }
        None => {
            let oid = ss.lookup(&k).ok_or_else(S3Error::no_key)?;
            let r = ss.record(&oid).ok_or_else(S3Error::no_key)?;
            (r.head, r.content.clone(), r.size, r.attrs.clone(), r.kind)
        }
    };
    if kind != Kind::File {
        return Err(S3Error::invalid("only files can be copied"));
    }
    let _ = size;
    let attrs = if ctx.header("x-amz-metadata-directive").is_some_and(|m| m.eq_ignore_ascii_case("REPLACE")) {
        ctx.attrs_for_put()?
    } else {
        Attrs { content_type: src_attrs.content_type, meta: src_attrs.meta, mode: src_attrs.mode, ..Default::default() }
    };
    let key = ctx.key().to_owned();
    let pre = ctx.precondition();
    let actor = ctx.actor();
    let desc = content.unwrap_or_else(ContentDescriptor::empty);
    let (v, s) = commit(app, d, move |st| Ok(ops::put(st, &key, desc.clone(), attrs.clone(), Op::Copy, &pre, &actor)?)).await?;
    let r = s.lookup(&parse_key(ctx.key())?).and_then(|o| s.record(&o).cloned()).ok_or_else(S3Error::no_key)?;
    let body = format!(
        "<CopyObjectResult xmlns=\"{S3_NS}\"><LastModified>{}</LastModified><ETag>{}</ETag></CopyObjectResult>",
        iso(r.time),
        xml_escape(&r.etag)
    );
    let mut resp = xml(200, body);
    let h = resp.headers_mut();
    h.insert("x-amz-version-id", v.to_string().parse().unwrap());
    h.insert("x-amz-copy-source-version-id", row_version.to_string().parse().unwrap());
    Ok(resp)
}

async fn delete(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>) -> Result<Response, S3Error> {
    let key = ctx.key().to_owned();
    let pre = ctx.precondition();
    let actor = ctx.actor();
    let r = app
        .pool
        .commit(d, move |st| match ops::delete(st, &key, &pre, &actor)? {
            Some(t) => Ok(t),
            None => Err(CommitError::Op(ops::OpError::NoSuchKey)),
        })
        .await;
    match r {
        Ok((v, _)) => Ok(empty(204).header("x-amz-version-id", v.to_string()).body(Body::empty()).unwrap()),
        Err(CommitError::Op(ops::OpError::NoSuchKey)) => Ok(empty(204).body(Body::empty()).unwrap()),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize)]
struct DeleteRequest {
    keys: Vec<(String, Option<String>)>,
    quiet: bool,
}

fn parse_delete(body: &[u8]) -> Result<DeleteRequest, S3Error> {
    let text = std::str::from_utf8(body).map_err(|_| S3Error::new(400, "MalformedXML", "not UTF-8"))?;
    let doc = roxmltree::Document::parse(text).map_err(|e| S3Error::new(400, "MalformedXML", e.to_string()))?;
    let root = doc.root_element();
    let mut keys = Vec::new();
    let mut quiet = false;
    for n in root.children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "Quiet" => quiet = n.text().is_some_and(|t| t.trim() == "true"),
            "Object" => {
                let child = |name: &str| n.children().find(|c| c.tag_name().name() == name).and_then(|c| c.text()).map(str::to_owned);
                let key = child("Key").ok_or_else(|| S3Error::new(400, "MalformedXML", "Object without Key"))?;
                keys.push((key, child("VersionId")));
            }
            _ => {}
        }
    }
    if keys.len() > 1000 {
        return Err(S3Error::new(400, "MalformedXML", "at most 1000 keys per request"));
    }
    Ok(DeleteRequest { keys, quiet })
}

pub async fn delete_objects(app: &Arc<App>, ctx: &Ctx, body: Body) -> Result<Response, S3Error> {
    ctx.require(Scope::Write)?;
    let d = ctx.drive(app)?;
    let body = BodyReader::new(body, ctx, MAX_SMALL_BODY)?.read_all().await?;
    let req = parse_delete(&body)?;
    let actor = ctx.actor();
    // Queued together and in order: each key is deleted after the ones before it, as if one at a
    // time, and they share log entries.
    let plans = req.keys.iter().filter(|(_, version)| version.is_none()).map(|(key, _)| {
        let (key, actor) = (key.clone(), actor.clone());
        move |st: &DriveState| match ops::delete(st, &key, &ops::Precondition::default(), &actor)? {
            Some(t) => Ok(t),
            None => Err(CommitError::Op(ops::OpError::NoSuchKey)),
        }
    });
    let mut answers = app.pool.commit_all(&d, plans).await.into_iter();
    let mut out = String::new();
    for (key, version) in &req.keys {
        if version.is_some() {
            out.push_str(&format!(
                "<Error><Key>{}</Key><Code>NotImplemented</Code><Message>history is immutable</Message></Error>",
                xml_escape(key)
            ));
            continue;
        }
        match answers.next().expect("an answer per key") {
            Ok(_) | Err(CommitError::Op(ops::OpError::NoSuchKey)) => {
                if !req.quiet {
                    out.push_str(&format!("<Deleted><Key>{}</Key></Deleted>", xml_escape(key)));
                }
            }
            Err(e) => {
                let e: S3Error = e.into();
                out.push_str(&format!("<Error><Key>{}</Key><Code>{}</Code><Message>{}</Message></Error>", xml_escape(key), e.code, xml_escape(&e.message)));
            }
        }
    }
    Ok(xml(200, format!("<DeleteResult xmlns=\"{S3_NS}\">{out}</DeleteResult>")))
}

// ---------------------------------------------------------------------------------------------
// In-place edits (protocol §4.1–§4.3)

enum Edit {
    Write { offset: u64, data: Bytes },
    Splice { offset: u64, remove: u64, data: Bytes },
    Patch { body: Bytes },
}

fn header_u64(ctx: &Ctx, name: &str) -> Result<Option<u64>, S3Error> {
    ctx.header(name).map(|v| v.parse::<u64>().map_err(|_| S3Error::invalid(format!("{name} must be a non-negative integer")))).transpose()
}

/// Runs an edit against the current content, fetching shards on demand.
fn compute(extents: &[Extent], edit: &Edit, size: Option<u64>, fetched: &HashMap<ShardHash, Bytes>, pool: &Pool) -> Result<Edited, EditError> {
    let p = pool.params;
    let mut made: HashMap<ShardHash, Bytes> = HashMap::new();
    let mut src = |h: &ShardHash| fetched.get(h).cloned();
    let mut e = match edit {
        Edit::Write { offset, data } => content::write_at(extents, *offset, data, p, &mut src)?,
        Edit::Splice { offset, remove, data } => content::splice(extents, *offset, *remove, data, p, &mut src)?,
        Edit::Patch { body } => {
            let edits = voidfs_core::patch::decode(body).expect("validated before");
            content::apply_edits(extents, &edits, p, &mut src)?
        }
    };
    if let Some(n) = size {
        for s in &e.new_shards {
            made.insert(s.hash, s.bytes.clone());
        }
        let mut both = |h: &ShardHash| made.get(h).cloned().or_else(|| fetched.get(h).cloned());
        let resized = content::set_size(&e.extents, n, p, &mut both)?;
        let used: std::collections::HashSet<_> = resized.extents.iter().filter_map(Extent::shard).collect();
        let mut all: HashMap<ShardHash, Bytes> = made;
        for s in resized.new_shards {
            all.insert(s.hash, s.bytes);
        }
        e = Edited {
            extents: resized.extents,
            new_shards: all.into_iter().filter(|(h, _)| used.contains(h)).map(|(hash, bytes)| Shard { hash, bytes }).collect(),
        };
    }
    Ok(e)
}

/// Where an edit's result is held (format §5): content of at most [`MAX_DATA_BYTES`] in one data
/// extent, if the pool has them, and anything else in shards and zeros only. Content of zeros
/// alone needs neither, and stays as it is.
async fn held(pool: &Pool, edited: Edited, fetched: &mut HashMap<ShardHash, Bytes>) -> Result<Edited, S3Error> {
    let size = content::size(&edited.extents);
    if pool.inline_data && size <= MAX_DATA_BYTES as u64 && edited.extents.iter().any(|e| !matches!(e, Extent::Zero { .. })) {
        for s in &edited.new_shards {
            fetched.insert(s.hash, s.bytes.clone());
        }
        let bytes = loop {
            match content::materialize(&edited.extents, &mut |h: &ShardHash| fetched.get(h).cloned()) {
                Ok(b) => break b,
                Err(EditError::MissingShard(h)) => {
                    fetched.insert(h, pool.shard(&h).await?);
                }
                Err(e) => return Err(S3Error::internal(e.to_string())),
            }
        };
        return Ok(Edited { extents: content::inline(&bytes), new_shards: Vec::new() });
    }
    if voidfs_core::model::data_len(&edited.extents) == 0 {
        return Ok(edited);
    }
    let spilled = content::spill(&edited.extents);
    let mut new_shards = edited.new_shards;
    for s in spilled.new_shards {
        if !new_shards.iter().any(|n| n.hash == s.hash) {
            new_shards.push(s);
        }
    }
    Ok(Edited { extents: spilled.extents, new_shards })
}

fn edit_range(edit: &Edit, size: u64) -> Vec<(u64, u64)> {
    match edit {
        Edit::Write { offset, data } => vec![((*offset).min(size), offset + data.len() as u64)],
        Edit::Splice { offset, remove, .. } => vec![(*offset, offset + remove)],
        Edit::Patch { body } => voidfs_core::patch::decode(body)
            .map(|es| es.iter().map(|e| (e.offset.min(size), e.offset + e.data.len() as u64)).collect())
            .unwrap_or_default(),
    }
}

async fn run_edit(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, edit: Edit, size: Option<u64>) -> Result<Response, S3Error> {
    let key = ctx.key().to_owned();
    let pre = ctx.precondition();
    let patch = ctx.attrs_patch()?;
    let actor = ctx.actor();
    let create = matches!(edit, Edit::Write { .. });
    for _ in 0..8 {
        let snap = d.snapshot();
        let target = ops::edit_target(&snap, &key, &pre, create)?;
        let base_head = target.map(|r| r.head);
        let extents = match target {
            Some(r) => app.pool.extents(r.content.as_ref().unwrap_or(&ContentDescriptor::empty())).await?,
            None => Vec::new(),
        };
        let total = content::size(&extents);
        if let Edit::Splice { offset, remove, .. } = &edit
            && offset.checked_add(*remove).is_none_or(|end| end > total) {
                return Err(S3Error::invalid(format!("the splice runs past the end of the {total}-byte object")));
            }
        let mut wanted: Vec<ShardHash> = Vec::new();
        for (a, b) in edit_range(&edit, total) {
            wanted.extend(content::needed_shards(&extents, a, b.min(total).max(a)));
        }
        if size.is_some_and(|n| n < total) {
            wanted.extend(content::needed_shards(&extents, size.unwrap(), total));
        }
        wanted.sort();
        wanted.dedup();
        let mut fetched = app.pool.fetch(&wanted).await?;
        let edited = loop {
            match compute(&extents, &edit, size, &fetched, &app.pool) {
                Ok(e) => break e,
                Err(EditError::MissingShard(h)) => {
                    fetched.insert(h, app.pool.shard(&h).await?);
                }
                Err(e) => return Err(S3Error::invalid(e.to_string())),
            }
        };
        let edited = held(&app.pool, edited, &mut fetched).await?;
        app.pool.write_shards(&edited.new_shards).await?;
        let desc = app.pool.describe(edited.extents).await?;
        let (key, patch, pre, actor) = (key.clone(), patch.clone(), pre.clone(), actor.clone());
        let r = app
            .pool
            .commit(d, move |st| {
                let head = Key::parse(&key).ok().and_then(|k| st.lookup(&k)).and_then(|o| st.record(&o)).map(|r| r.head);
                if head != base_head {
                    return Err(CommitError::Retry);
                }
                Ok(ops::write(st, &key, desc.clone(), &patch, &pre, &actor)?)
            })
            .await;
        match r {
            Ok((v, s)) => return Ok(mutation_response(v, &s, ctx.key()).body(Body::empty()).unwrap()),
            Err(CommitError::Retry) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(CommitError::Retry.into())
}

async fn write(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, body: Body) -> Result<Response, S3Error> {
    let offset = header_u64(ctx, "x-voidfs-offset")?.ok_or_else(|| S3Error::invalid("x-voidfs-offset is required"))?;
    let size = header_u64(ctx, "x-voidfs-size")?;
    let data = BodyReader::new(body, ctx, MAX_EXTENSION_BODY)?.read_all().await?;
    run_edit(app, ctx, d, Edit::Write { offset, data }, size).await
}

async fn splice(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, body: Body) -> Result<Response, S3Error> {
    let offset = header_u64(ctx, "x-voidfs-offset")?.ok_or_else(|| S3Error::invalid("x-voidfs-offset is required"))?;
    let remove = header_u64(ctx, "x-voidfs-remove")?.unwrap_or(0);
    let data = BodyReader::new(body, ctx, MAX_EXTENSION_BODY)?.read_all().await?;
    if remove == 0 && data.is_empty() {
        return Err(S3Error::invalid("a splice must insert or remove bytes"));
    }
    run_edit(app, ctx, d, Edit::Splice { offset, remove, data }, None).await
}

async fn patch(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, body: Body) -> Result<Response, S3Error> {
    let size = header_u64(ctx, "x-voidfs-size")?;
    let body = BodyReader::new(body, ctx, MAX_EXTENSION_BODY)?.read_all().await?;
    voidfs_core::patch::decode(&body).map_err(|e| S3Error::new(400, "InvalidPatch", e.to_string()))?;
    run_edit(app, ctx, d, Edit::Patch { body }, size).await
}

// ---------------------------------------------------------------------------------------------
// Rename, restore, attributes, history

async fn rename(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>) -> Result<Response, S3Error> {
    let raw = ctx.header("x-voidfs-source").ok_or_else(|| S3Error::invalid("x-voidfs-source is required"))?;
    let src = percent_encoding::percent_decode_str(raw).decode_utf8().map_err(|_| S3Error::invalid("x-voidfs-source is not UTF-8"))?.into_owned();
    let replace = ctx.header("x-voidfs-replace").is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let dst = ctx.key().to_owned();
    let pre = ctx.precondition();
    let patch = ctx.attrs_patch()?;
    let actor = ctx.actor();
    let (v, s) = commit(app, d, move |st| Ok(ops::rename(st, &src, &dst, replace, &patch, &pre, &actor)?)).await?;
    Ok(mutation_response(v, &s, ctx.key()).body(Body::empty()).unwrap())
}

async fn restore(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>) -> Result<Response, S3Error> {
    let key = ctx.key().to_owned();
    let pre = ctx.precondition();
    let actor = ctx.actor();
    let version = ctx.query.get("versionId");
    let as_of = ctx.header("x-voidfs-as-of");
    match (version, as_of) {
        (Some(_), Some(_)) => Err(S3Error::invalid("give versionId or x-voidfs-as-of, not both")),
        (None, None) => Err(S3Error::invalid("versionId or x-voidfs-as-of is required")),
        (Some(v), None) => {
            let v: VersionId = v.parse().map_err(|_| S3Error::new(404, "NoSuchVersion", "no such version"))?;
            let (nv, s) = commit(app, d, move |st| Ok(ops::restore(st, &key, v, &pre, &actor)?)).await?;
            Ok(mutation_response(nv, &s, ctx.key()).header("x-voidfs-restored-from", v.to_string()).body(Body::empty()).unwrap())
        }
        (None, Some(t)) => {
            let t: Timestamp = t.parse().map_err(|_| S3Error::invalid("x-voidfs-as-of is not an RFC 3339 timestamp"))?;
            if key.ends_with('/') {
                let then = app.pool.state_at(d, t).await.map_err(|e| S3Error::invalid(format!("{e:#}")))?;
                let (nv, s) = commit(app, d, move |st| Ok(ops::restore_subtree(st, &then, &key, &actor)?)).await?;
                Ok(mutation_response(nv, &s, ctx.key()).body(Body::empty()).unwrap())
            } else {
                let snap = d.snapshot();
                let oid = snap.lookup(&parse_key(&key)?).ok_or_else(S3Error::no_key)?;
                let v = snap.as_of(&oid, t).map(|r| r.version).ok_or_else(|| S3Error::new(404, "NoSuchVersion", "the object did not exist then"))?;
                let (nv, s) = commit(app, d, move |st| Ok(ops::restore(st, &key, v, &pre, &actor)?)).await?;
                Ok(mutation_response(nv, &s, ctx.key()).header("x-voidfs-restored-from", v.to_string()).body(Body::empty()).unwrap())
            }
        }
    }
}

fn get_attrs(ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let s = d.snapshot();
    let v = resolve_view(ctx, &s)?;
    Ok(json(&json!({
        "objectId": v.oid,
        "kind": v.kind,
        "versionId": v.version.to_string(),
        "mtime": v.attrs.mtime.unwrap_or(v.time),
        "mode": format_mode(v.attrs.mode, v.kind),
        "xattrs": v.attrs.xattrs,
        "flags": v.attrs.flags,
        "contentType": v.attrs.content_type,
        "meta": v.attrs.meta,
    })))
}

#[derive(Deserialize, Default)]
struct XattrsChange {
    #[serde(default)]
    set: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    remove: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttrsBody {
    #[serde(default)]
    mtime: Option<Timestamp>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    xattrs: Option<XattrsChange>,
    #[serde(default)]
    flags: Option<Vec<String>>,
    #[serde(default)]
    content_type: Option<String>,
}

async fn post_attrs(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, body: Body) -> Result<Response, S3Error> {
    let body = BodyReader::new(body, ctx, MAX_SMALL_BODY)?.read_all().await?;
    let b: AttrsBody = serde_json::from_slice(&body).map_err(|e| S3Error::invalid(format!("attributes body: {e}")))?;
    let x = b.xattrs.unwrap_or_default();
    let patch = AttrsPatch {
        mtime: b.mtime,
        mode: b.mode.as_deref().map(parse_mode).transpose()?,
        content_type: b.content_type,
        xattrs_set: x.set,
        xattrs_remove: x.remove,
        flags: b.flags,
    };
    let key = ctx.key().to_owned();
    let pre = ctx.precondition();
    let actor = ctx.actor();
    let (v, s) = commit(app, d, move |st| Ok(ops::set_attrs(st, &key, &patch, &pre, &actor)?)).await?;
    Ok(mutation_response(v, &s, ctx.key()).body(Body::empty()).unwrap())
}

/// `?x-voidfs-versions` (protocol §4.4).
fn versions(ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let s = d.snapshot();
    let key = parse_key(ctx.key())?;
    let oid = s.lookup(&key).ok_or_else(S3Error::no_key)?;
    let all = ctx.query.get("x-voidfs-all").is_some_and(|v| v == "true");
    let rows: Vec<&HistoryRow> = s.history(&oid).map(|h| h.iter().filter(|r| all || r.op.follows_content()).collect()).unwrap_or_default();
    let max = ctx.max_keys()?;
    let start: usize = ctx.query.get("continuation-token").and_then(|t| t.parse().ok()).unwrap_or(0);
    let last = rows.len().saturating_sub(1);
    let page: Vec<_> = rows
        .iter()
        .enumerate()
        .skip(start)
        .take(max)
        .map(|(i, r)| {
            let mut v = json!({
                "versionId": r.version.to_string(),
                "isLatest": i == last,
                "size": r.size,
                "etag": r.etag,
                "lastModified": r.time,
                "operation": r.op.as_str(),
            });
            if let Some(rf) = r.restored_from {
                v["restoredFrom"] = json!(rf.to_string());
            }
            v
        })
        .collect();
    let next = (start + max < rows.len()).then(|| (start + max).to_string());
    Ok(json(&json!({ "key": key.render(), "versions": page, "nextContinuationToken": next })))
}

// ---------------------------------------------------------------------------------------------
// Multipart uploads (protocol §3, format §11)

#[derive(Serialize, Deserialize)]
struct UploadRecord {
    key: String,
    created: Timestamp,
    actor: voidfs_core::model::Actor,
    attrs: Attrs,
}

#[derive(Serialize, Deserialize)]
struct PartRecord {
    part: u32,
    size: u64,
    etag: String,
    extents: Vec<Extent>,
}

const MIN_PART: u64 = 5 * 1024 * 1024;

fn upload_dir(d: &Drive, id: &str) -> Result<String, S3Error> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err(S3Error::new(404, "NoSuchUpload", "no such upload"));
    }
    Ok(format!("drives/{}/uploads/{id}/", d.id))
}

async fn load_upload(app: &App, d: &Drive, id: &str, key: &str) -> Result<(String, UploadRecord), S3Error> {
    let dir = upload_dir(d, id)?;
    let bytes = app.pool.store.get(&format!("{dir}upload.json")).await?.ok_or_else(|| S3Error::new(404, "NoSuchUpload", "no such upload"))?;
    let rec: UploadRecord = serde_json::from_slice(&bytes).map_err(anyhow::Error::from)?;
    if rec.key != key {
        return Err(S3Error::new(404, "NoSuchUpload", "the upload is for another key"));
    }
    Ok((dir, rec))
}

async fn create_upload(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let key = ctx.key().to_owned();
    if parse_key(&key)?.is_folder() {
        return Err(S3Error::invalid("multipart uploads create files, not folders"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let rec = UploadRecord { key: key.clone(), created: app.pool.clock.now(), actor: ctx.actor(), attrs: ctx.attrs_for_put()? };
    let dir = upload_dir(d, &id)?;
    app.pool.store.put(&format!("{dir}upload.json"), Bytes::from(serde_json::to_vec(&rec).map_err(anyhow::Error::from)?)).await?;
    Ok(xml(
        200,
        format!(
            "<InitiateMultipartUploadResult xmlns=\"{S3_NS}\"><Bucket>{}</Bucket><Key>{}</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>",
            xml_escape(ctx.bucket()),
            xml_escape(&key)
        ),
    ))
}

async fn upload_part(app: &Arc<App>, ctx: &Ctx, d: &Drive, body: Body) -> Result<Response, S3Error> {
    let n: u32 = ctx.query.get("partNumber").and_then(|p| p.parse().ok()).filter(|p| (1..=10_000).contains(p)).ok_or_else(|| S3Error::invalid("partNumber must be 1 to 10000"))?;
    let (dir, _) = load_upload(app, d, ctx.query.get("uploadId").unwrap_or_default(), ctx.key()).await?;
    let extents = if let Some(src) = ctx.header("x-amz-copy-source") {
        if ctx.header("x-amz-copy-source-range").is_some() {
            return Err(S3Error::not_implemented("UploadPartCopy with a range is not supported yet"));
        }
        let decoded = percent_encoding::percent_decode_str(src).decode_utf8_lossy().into_owned();
        let (b, k) = decoded.trim_start_matches('/').split_once('/').ok_or_else(|| S3Error::invalid("bad x-amz-copy-source"))?;
        let sd = ctx.drive_named(app, b)?;
        let ss = sd.snapshot();
        let oid = ss.lookup(&parse_key(k)?).ok_or_else(S3Error::no_key)?;
        let r = ss.record(&oid).ok_or_else(S3Error::no_key)?;
        let extents = app.pool.extents(r.content.as_ref().unwrap_or(&ContentDescriptor::empty())).await?;
        // A part holds no data extents (format §11): a small source's bytes go to a shard.
        let spilled = content::spill(&extents);
        app.pool.write_shards(&spilled.new_shards).await?;
        spilled.extents
    } else {
        ingest(&app.pool, BodyReader::new(body, ctx, MAX_PUT)?, false).await?
    };
    let size = content::size(&extents);
    let etag = ContentDescriptor::Inline { extents: extents.clone() }.etag();
    let rec = PartRecord { part: n, size, etag: etag.clone(), extents };
    app.pool.store.put(&format!("{dir}{n:05}.json"), Bytes::from(serde_json::to_vec(&rec).map_err(anyhow::Error::from)?)).await?;
    if ctx.header("x-amz-copy-source").is_some() {
        return Ok(xml(200, format!("<CopyPartResult xmlns=\"{S3_NS}\"><ETag>{}</ETag><LastModified>{}</LastModified></CopyPartResult>", xml_escape(&etag), iso(Timestamp::now()))));
    }
    Ok(empty(200).header("etag", etag).body(Body::empty()).unwrap())
}

async fn parts_of(app: &App, dir: &str) -> Result<Vec<PartRecord>, S3Error> {
    let mut out = Vec::new();
    for name in app.pool.store.list_files(dir, None).await? {
        if name == "upload.json" {
            continue;
        }
        if let Some(b) = app.pool.store.get(&format!("{dir}{name}")).await? {
            let p = serde_json::from_slice::<PartRecord>(&b).map_err(anyhow::Error::from)?;
            if voidfs_core::model::data_len(&p.extents) > 0 {
                return Err(S3Error::internal(format!("{dir}{name} holds a data extent, which part records never do (format §11)")));
            }
            out.push(p);
        }
    }
    out.sort_by_key(|p| p.part);
    Ok(out)
}

async fn complete_upload(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, body: Body) -> Result<Response, S3Error> {
    let key = ctx.key().to_owned();
    let (dir, rec) = load_upload(app, d, ctx.query.get("uploadId").unwrap_or_default(), &key).await?;
    let body = BodyReader::new(body, ctx, MAX_SMALL_BODY)?.read_all().await?;
    let text = std::str::from_utf8(&body).map_err(|_| S3Error::new(400, "MalformedXML", "not UTF-8"))?;
    let doc = roxmltree::Document::parse(text).map_err(|e| S3Error::new(400, "MalformedXML", e.to_string()))?;
    let mut wanted = Vec::new();
    for p in doc.root_element().children().filter(|n| n.tag_name().name() == "Part") {
        let child = |name: &str| p.children().find(|c| c.tag_name().name() == name).and_then(|c| c.text()).unwrap_or_default().trim().to_owned();
        let n: u32 = child("PartNumber").parse().map_err(|_| S3Error::new(400, "MalformedXML", "bad PartNumber"))?;
        wanted.push((n, child("ETag")));
    }
    if wanted.is_empty() {
        return Err(S3Error::new(400, "MalformedXML", "no parts"));
    }
    if wanted.windows(2).any(|w| w[0].0 >= w[1].0) {
        return Err(S3Error::new(400, "InvalidPartOrder", "parts must be listed in ascending order"));
    }
    let have: HashMap<u32, PartRecord> = parts_of(app, &dir).await?.into_iter().map(|p| (p.part, p)).collect();
    let mut extents = Vec::new();
    for (i, (n, etag)) in wanted.iter().enumerate() {
        let p = have.get(n).ok_or_else(|| S3Error::new(400, "InvalidPart", format!("part {n} was not uploaded")))?;
        if p.etag.trim_matches('"') != etag.trim_matches('"') {
            return Err(S3Error::new(400, "InvalidPart", format!("part {n} has a different ETag")));
        }
        if i + 1 < wanted.len() && p.size < MIN_PART {
            return Err(S3Error::new(400, "EntityTooSmall", format!("part {n} is smaller than 5 MiB")));
        }
        extents.extend(p.extents.iter().cloned());
    }
    let desc = app.pool.describe(content::normalize(extents)).await?;
    let pre = ctx.precondition();
    let actor = ctx.actor();
    let attrs = rec.attrs;
    let (v, s) = commit(app, d, move |st| Ok(ops::put(st, &key, desc.clone(), attrs.clone(), Op::Put, &pre, &actor)?)).await?;
    let _ = app.pool.store.delete_prefix(&dir).await;
    let key = ctx.key();
    let etag = s.lookup(&parse_key(key)?).and_then(|o| s.record(&o).map(|r| r.etag.clone())).unwrap_or_default();
    let mut resp = xml(
        200,
        format!(
            "<CompleteMultipartUploadResult xmlns=\"{S3_NS}\"><Location>{}</Location><Bucket>{}</Bucket><Key>{}</Key><ETag>{}</ETag></CompleteMultipartUploadResult>",
            xml_escape(&ctx.location(Some(key))),
            xml_escape(ctx.bucket()),
            xml_escape(key),
            xml_escape(&etag)
        ),
    );
    resp.headers_mut().insert("x-amz-version-id", v.to_string().parse().unwrap());
    Ok(resp)
}

async fn abort_upload(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let (dir, _) = load_upload(app, d, ctx.query.get("uploadId").unwrap_or_default(), ctx.key()).await?;
    app.pool.store.delete_prefix(&dir).await?;
    Ok(empty(204).body(Body::empty()).unwrap())
}

async fn list_parts(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let id = ctx.query.get("uploadId").unwrap_or_default();
    let (dir, _) = load_upload(app, d, id, ctx.key()).await?;
    let mut parts = String::new();
    for p in parts_of(app, &dir).await? {
        parts.push_str(&format!("<Part><PartNumber>{}</PartNumber><ETag>{}</ETag><Size>{}</Size></Part>", p.part, xml_escape(&p.etag), p.size));
    }
    Ok(xml(
        200,
        format!(
            "<ListPartsResult xmlns=\"{S3_NS}\"><Bucket>{}</Bucket><Key>{}</Key><UploadId>{}</UploadId><IsTruncated>false</IsTruncated>{parts}</ListPartsResult>",
            xml_escape(ctx.bucket()),
            xml_escape(ctx.key()),
            xml_escape(id)
        ),
    ))
}

pub async fn list_uploads(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let mut out = String::new();
    for id in app.pool.store.list_dirs(&format!("drives/{}/uploads/", d.id)).await? {
        if let Some(b) = app.pool.store.get(&format!("drives/{}/uploads/{id}/upload.json", d.id)).await?
            && let Ok(r) = serde_json::from_slice::<UploadRecord>(&b) {
                out.push_str(&format!("<Upload><Key>{}</Key><UploadId>{id}</UploadId><Initiated>{}</Initiated></Upload>", xml_escape(&r.key), iso(r.created)));
            }
    }
    Ok(xml(200, format!("<ListMultipartUploadsResult xmlns=\"{S3_NS}\"><Bucket>{}</Bucket><IsTruncated>false</IsTruncated>{out}</ListMultipartUploadsResult>", xml_escape(ctx.bucket()))))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use futures::FutureExt;
    use rand::{RngExt, SeedableRng};
    use voidfs_core::model::{Chunking, CommitGuard, Features, PoolDescriptor};

    use super::*;
    use crate::sigv4::{Authenticated, KeyInfo, Payload};
    use crate::store::{Fault, MemOp, MemStore, Store};

    type Sender = futures::channel::mpsc::UnboundedSender<Result<Bytes, std::io::Error>>;

    fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        (0..len).map(|_| rng.random()).collect()
    }

    /// A pool that cuts shards of 64 bytes to 1 KiB, so that a small body makes many.
    async fn pool(mem: &Arc<MemStore>) -> Arc<Pool> {
        pool_cutting(mem, Chunking { min: 64, avg: 256, max: 1024, ..Chunking::default() }).await
    }

    async fn pool_cutting(mem: &Arc<MemStore>, chunking: Chunking) -> Arc<Pool> {
        let desc = PoolDescriptor {
            format: voidfs_core::FORMAT_VERSION,
            pool_id: "p-ingest".into(),
            created: Timestamp::now(),
            features: Features { compatible: vec![], incompatible: vec![] },
            chunking,
            hash: "sha256".into(),
            commit_guard: CommitGuard::CreateIfAbsent,
        };
        let store = Store::mem(mem.clone());
        store.put(crate::probe::DESCRIPTOR, Bytes::from(serde_json::to_vec(&desc).unwrap())).await.unwrap();
        Pool::open(store, 1 << 20).await.unwrap()
    }

    /// A body read as a request with `payload` would be, which arrives through the sender.
    fn body(payload: Payload) -> (BodyReader, Sender) {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let key = KeyInfo { id: "k".into(), secret: "s".into(), scope: crate::sigv4::Scope::Admin, drives: None };
        let auth = Authenticated { key, payload, signing_key: [0; 32], scope: String::new(), amz_date: String::new(), seed_signature: String::new() };
        let ctx = Ctx {
            method: Method::PUT,
            bucket: Some("d".into()),
            key: Some("k".into()),
            virtual_host: false,
            query: super::super::util::Query::parse(""),
            headers: http::HeaderMap::new(),
            auth,
        };
        (BodyReader::new(Body::from_stream(rx), &ctx, MAX_PUT).unwrap(), tx)
    }

    fn shard_puts(op: MemOp, path: &str) -> bool {
        op == MemOp::Put && path.starts_with("shards/")
    }

    /// Makes shard uploads wait until the sender says `true`, except that `fail` picks, by
    /// count from 1, uploads that fail at once instead. Counts the uploads started.
    fn hold(mem: &MemStore, fail: impl Fn(usize) -> bool + Send + Sync + 'static) -> (tokio::sync::watch::Sender<bool>, Arc<AtomicUsize>) {
        let (release, released) = tokio::sync::watch::channel(false);
        let started = Arc::new(AtomicUsize::new(0));
        let n = started.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            if !shard_puts(op, path) {
                return futures::future::ready(Fault::None).boxed();
            }
            if fail(n.fetch_add(1, Ordering::SeqCst) + 1) {
                return futures::future::ready(Fault::Fail).boxed();
            }
            let mut released = released.clone();
            async move {
                let _ = released.wait_for(|r| *r).await;
                Fault::None
            }
            .boxed()
        })));
        (release, started)
    }

    async fn reached(n: &AtomicUsize, at_least: usize) {
        while n.load(Ordering::SeqCst) < at_least {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Shards upload while the body is still arriving, no more than [`INGEST_UPLOADS`] at once,
    /// and the extents are those of the whole body cut in one pass.
    #[tokio::test]
    async fn a_body_uploads_while_it_is_read() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let pool = pool(&mem).await;
        let data = Bytes::from(random_bytes(7, 200_000));
        let (reader, tx) = body(Payload::Unsigned);
        let (release, started) = hold(&mem, |_| false);
        let ingesting = tokio::spawn({
            let pool = pool.clone();
            async move { ingest(&pool, reader, true).await }
        });
        // Half the body, which cuts far more shards than the window holds.
        for piece in data[..100_000].chunks(4096) {
            tx.unbounded_send(Ok(Bytes::copy_from_slice(piece))).unwrap();
        }
        tokio::time::timeout(Duration::from_secs(10), reached(&started, INGEST_UPLOADS)).await.expect("uploads start while the body is read");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(started.load(Ordering::SeqCst), INGEST_UPLOADS, "a full window, and no more, before the body has ended");
        release.send(true).unwrap();
        for piece in data[100_000..].chunks(4096) {
            tx.unbounded_send(Ok(Bytes::copy_from_slice(piece))).unwrap();
        }
        drop(tx);
        let extents = ingesting.await.unwrap().unwrap();
        let whole = voidfs_core::content::from_bytes(&data, pool.params);
        assert_eq!(extents, content::normalize(whole.extents));
        for e in &extents {
            let Extent::Shard { s, .. } = e else { panic!("{e:?}") };
            assert!(pool.store.exists(&format!("shards/{}", s.object_path())).await.unwrap());
        }
    }

    /// Shards large enough to hash on other threads, uploads that finish out of order: the
    /// extents are still the body's, in order.
    #[tokio::test]
    async fn shards_hashed_elsewhere_keep_their_order() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let pool = pool_cutting(&mem, Chunking { min: HASH_ELSEWHERE as u32, avg: 512 << 10, max: 1 << 20, ..Chunking::default() }).await;
        let delays = Arc::new(std::sync::Mutex::new(rand::rngs::StdRng::seed_from_u64(3)));
        mem.set_hook(Some(Arc::new(move |op, path| {
            let wait = if shard_puts(op, path) { delays.lock().unwrap().random_range(0..20) } else { 0 };
            async move {
                tokio::time::sleep(Duration::from_millis(wait)).await;
                Fault::None
            }
            .boxed()
        })));
        let data = Bytes::from(random_bytes(10, 12 << 20));
        let (reader, tx) = body(Payload::Unsigned);
        for piece in data.chunks(64 << 10) {
            tx.unbounded_send(Ok(Bytes::copy_from_slice(piece))).unwrap();
        }
        drop(tx);
        let extents = ingest(&pool, reader, true).await.unwrap();
        let whole = voidfs_core::content::from_bytes(&data, pool.params);
        assert!(whole.extents.len() > INGEST_UPLOADS, "{} shards", whole.extents.len());
        assert_eq!(extents, content::normalize(whole.extents));
    }

    /// An upload that fails fails the body at once, and the uploads still in flight are dropped.
    #[tokio::test]
    async fn a_failed_upload_stops_the_rest() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let pool = pool(&mem).await;
        let data = random_bytes(8, 100_000);
        let (reader, tx) = body(Payload::Unsigned);
        let (release, started) = hold(&mem, |n| n == 3);
        tx.unbounded_send(Ok(Bytes::from(data))).unwrap();
        drop(tx);
        let failed = tokio::time::timeout(Duration::from_secs(10), ingest(&pool, reader, true)).await.expect("the failure does not wait for the rest");
        assert_eq!(failed.unwrap_err().status, 500);
        let n = started.load(Ordering::SeqCst);
        assert!((3..=INGEST_UPLOADS).contains(&n), "{n} uploads started");
        release.send(true).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(pool.store.list_recursive("shards/").await.unwrap().is_empty(), "none of the dropped uploads went on");
    }

    // -----------------------------------------------------------------------------------------
    // Content held in descriptors (format §5)

    /// An app over `mem` whose pool has the default chunking and lists `features`, with a drive
    /// `d`.
    async fn app_with(mem: &Arc<MemStore>, features: &[&str]) -> (Arc<App>, Arc<Drive>) {
        let features: Vec<String> = features.iter().map(|f| (*f).to_owned()).collect();
        let store = Store::mem(mem.clone());
        let pool = Pool::open_creating(store, 1 << 20, crate::clock::Clock::System, CommitGuard::CreateIfAbsent, &features).await.unwrap();
        let d = pool.create_drive("d", None).await.unwrap();
        (Arc::new(App { pool, keys: crate::sigv4::Keys::default(), domains: super::super::Domains::new(Vec::new()), metrics: crate::metrics::S3Metrics::new() }), d)
    }

    /// Sends a request for `key` of drive `d` as the admin key, with `payload` as what its
    /// signature says of the body.
    async fn send_as(app: &Arc<App>, method: Method, key: &str, query: &str, headers: &[(&str, &str)], body: &[u8], payload: Payload) -> Result<Response, S3Error> {
        let key_info = KeyInfo { id: "k".into(), secret: "s".into(), scope: crate::sigv4::Scope::Admin, drives: None };
        let auth = Authenticated { key: key_info, payload, signing_key: [0; 32], scope: String::new(), amz_date: String::new(), seed_signature: String::new() };
        let mut map = http::HeaderMap::new();
        for (k, v) in headers {
            map.insert(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        let ctx = Ctx { method, bucket: Some("d".into()), key: Some(key.into()), virtual_host: false, query: super::super::util::Query::parse(query), headers: map, auth };
        dispatch(app, &ctx, Body::from(body.to_vec())).await
    }

    async fn send(app: &Arc<App>, method: Method, key: &str, query: &str, headers: &[(&str, &str)], body: &[u8]) -> Response {
        send_as(app, method, key, query, headers, body, Payload::Unsigned).await.unwrap_or_else(|e| panic!("{} {}", e.code, e.message))
    }

    async fn read_back(app: &Arc<App>, key: &str) -> Vec<u8> {
        let r = send(app, Method::GET, key, "", &[], b"").await;
        axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    fn content_of(d: &Drive, key: &str) -> Vec<Extent> {
        let s = d.snapshot();
        match s.record(&s.lookup(&Key::parse(key).unwrap()).unwrap()).unwrap().content.clone().unwrap() {
            ContentDescriptor::Inline { extents } => extents,
            t => panic!("{t:?}"),
        }
    }

    /// Records the writes to `mem` from now on, as `shard`, `log` or the path.
    fn writes(mem: &MemStore) -> Arc<std::sync::Mutex<Vec<String>>> {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            if matches!(op, MemOp::Put | MemOp::PutNew) {
                let what = if path.starts_with("shards/") { "shard".to_owned() } else if path.contains("/log/") { "log".to_owned() } else { path.to_owned() };
                log.lock().unwrap().push(what);
            }
            futures::future::ready(Fault::None).boxed()
        })));
        seen
    }

    fn taken(w: &std::sync::Mutex<Vec<String>>) -> Vec<String> {
        std::mem::take(&mut *w.lock().unwrap())
    }

    /// With `inline-data`, a put of at most 4,096 bytes writes one object to the bucket, the log
    /// entry, and its ETag is the one the same bytes have in a shard. Past that, and without the
    /// feature, the shard goes first.
    #[tokio::test]
    async fn a_small_put_is_one_request_to_the_bucket() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let (app, d) = app_with(&mem, &[voidfs_core::model::INLINE_DATA]).await;
        let w = writes(&mem);
        for n in [0, 1, 4095, 4096, 4097] {
            let data = random_bytes(n as u64, n);
            let key = format!("k{n}");
            let r = send(&app, Method::PUT, &key, "", &[], &data).await;
            let in_shards = content::from_bytes(&Bytes::from(data.clone()), app.pool.params);
            assert_eq!(r.headers()["etag"], ContentDescriptor::Inline { extents: in_shards.extents.clone() }.etag(), "{n} bytes");
            if n <= MAX_DATA_BYTES {
                assert_eq!(taken(&w), ["log"], "{n} bytes");
                assert_eq!(content_of(&d, &key), content::inline(&data));
            } else {
                assert_eq!(taken(&w), ["shard", "log"], "{n} bytes");
                assert_eq!(content_of(&d, &key), in_shards.extents);
            }
            assert_eq!(read_back(&app, &key).await, data);
        }
        // Held apart from the request's buffers.
        let Extent::Data { d: held } = &content_of(&d, "k4096")[0] else { panic!() };
        assert_eq!(held.len(), 4096);
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let (app, d) = app_with(&mem, &[]).await;
        let w = writes(&mem);
        send(&app, Method::PUT, "k", "", &[], &random_bytes(1, 4096)).await;
        assert_eq!(taken(&w), ["shard", "log"], "without the feature");
        assert!(matches!(content_of(&d, "k")[..], [Extent::Shard { .. }]));
    }

    /// A small body is checked against its signature before anything commits it.
    #[tokio::test]
    async fn a_small_put_that_does_not_match_its_signature_commits_nothing() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let (app, d) = app_with(&mem, &[voidfs_core::model::INLINE_DATA]).await;
        let w = writes(&mem);
        let e = send_as(&app, Method::PUT, "k", "", &[], b"small", Payload::Sha256([0; 32])).await.unwrap_err();
        assert_eq!(e.code, "XAmzContentSHA256Mismatch");
        assert!(taken(&w).is_empty());
        assert_eq!(d.snapshot().seq(), 0);
        let good: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(b"small").into();
        send_as(&app, Method::PUT, "k", "", &[], b"small", Payload::Sha256(good)).await.unwrap();
        assert_eq!(taken(&w), ["log"]);
    }

    /// An edit whose result is at most 4,096 bytes is held in the descriptor, with no shard
    /// uploaded; one that grows past it gets shards; content of zeros alone stays zeros.
    #[tokio::test]
    async fn edits_hold_small_results_in_the_descriptor() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let (app, d) = app_with(&mem, &[voidfs_core::model::INLINE_DATA]).await;
        let mut want = random_bytes(3, 4096);
        send(&app, Method::PUT, "f", "", &[], &want).await;
        let w = writes(&mem);
        let data = |d: &Drive| content_of(d, "f").iter().any(|e| matches!(e, Extent::Data { .. }));
        // Over the limit.
        send(&app, Method::PUT, "f", "x-voidfs-write", &[("x-voidfs-offset", "4094")], b"WXYZ").await;
        want.truncate(4094);
        want.extend_from_slice(b"WXYZ");
        assert_eq!(taken(&w), ["shard", "log"]);
        assert!(!data(&d));
        // Back under it: the shard is read, and nothing but the log entry is written.
        app.pool.forget(&content_of(&d, "f")[0].shard().unwrap());
        send(&app, Method::PUT, "f", "x-voidfs-write", &[("x-voidfs-offset", "0"), ("x-voidfs-size", "4000")], b"").await;
        want.truncate(4000);
        assert_eq!(taken(&w), ["log"]);
        assert_eq!(content_of(&d, "f"), content::inline(&want));
        // A splice over it, and a patch back under it.
        send(&app, Method::PUT, "f", "x-voidfs-splice", &[("x-voidfs-offset", "100")], &[b'a'; 200]).await;
        want.splice(100..100, [b'a'; 200]);
        assert_eq!(taken(&w), ["shard", "log"]);
        let patch = voidfs_core::patch::encode(&[voidfs_core::patch::Edit { offset: 10, data: b"patched" }]);
        send(&app, Method::POST, "f", "x-voidfs-patch", &[("x-voidfs-size", "3900")], &patch).await;
        want[10..17].copy_from_slice(b"patched");
        want.truncate(3900);
        assert_eq!(taken(&w), ["log"]);
        assert!(data(&d));
        // A write past the end leaves zeros in it, still under.
        send(&app, Method::PUT, "f", "x-voidfs-write", &[("x-voidfs-offset", "4000")], b"Q").await;
        want.resize(4000, 0);
        want.push(b'Q');
        assert_eq!(taken(&w), ["log"]);
        assert_eq!(content_of(&d, "f"), content::inline(&want));
        assert_eq!(read_back(&app, "f").await, want);
        // Zeros alone are stored as zeros.
        send(&app, Method::PUT, "z", "x-voidfs-write", &[("x-voidfs-offset", "0"), ("x-voidfs-size", "4096")], b"").await;
        assert_eq!(content_of(&d, "z"), [Extent::Zero { z: 4096 }]);
        assert_eq!(taken(&w), ["log"]);
        // A write far past the end of a small file leaves its data extent untouched by the edit,
        // and over the limit: the extent goes to a shard of its own.
        send(&app, Method::PUT, "g", "", &[], &[7; 100]).await;
        taken(&w);
        send(&app, Method::PUT, "g", "x-voidfs-write", &[("x-voidfs-offset", "5000")], b"!").await;
        assert_eq!(taken(&w), ["shard", "shard", "log"]);
        assert!(matches!(content_of(&d, "g")[..], [Extent::Shard { n: 100, .. }, Extent::Zero { z: 4900 }, Extent::Shard { n: 1, .. }]));
        assert_eq!(read_back(&app, "g").await, [&[7; 100][..], &[0; 4900], b"!"].concat());
    }

    /// A server of a pool without the feature never holds content in a descriptor, even where
    /// the content it edits has some (the feature was added after it started).
    #[tokio::test]
    async fn without_the_feature_edits_hold_nothing_in_the_descriptor() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let (app, d) = app_with(&mem, &[]).await;
        let small = random_bytes(4, 100);
        let (key, desc) = ("f".to_owned(), ContentDescriptor::Inline { extents: content::inline(&small) });
        app.pool
            .commit(&d, move |st| Ok(ops::put(st, &key, desc.clone(), Attrs::default(), Op::Put, &Default::default(), &voidfs_core::model::Actor::system())?))
            .await
            .unwrap();
        let w = writes(&mem);
        // Past a gap, so that the edit itself leaves the data extent as it is.
        send(&app, Method::PUT, "f", "x-voidfs-write", &[("x-voidfs-offset", "200")], b"!").await;
        assert_eq!(taken(&w), ["shard", "shard", "log"]);
        assert!(matches!(content_of(&d, "f")[..], [Extent::Shard { n: 100, .. }, Extent::Zero { z: 100 }, Extent::Shard { n: 1, .. }]));
        assert_eq!(read_back(&app, "f").await, [&small[..], &[0; 100], b"!"].concat());
    }

    /// A copy keeps the descriptor, data extents and all; a multipart part copied from a small
    /// file holds a shard instead (format §11), and so does the completed upload.
    #[tokio::test]
    async fn copies_keep_the_descriptor_and_parts_never_hold_data() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let (app, d) = app_with(&mem, &[voidfs_core::model::INLINE_DATA]).await;
        let small = random_bytes(5, 3000);
        let put = send(&app, Method::PUT, "src", "", &[], &small).await;
        let w = writes(&mem);
        send(&app, Method::PUT, "dst", "", &[("x-amz-copy-source", "/d/src")], b"").await;
        assert_eq!(taken(&w), ["log"]);
        assert_eq!(content_of(&d, "dst"), content_of(&d, "src"));
        let r = send(&app, Method::HEAD, "dst", "", &[], b"").await;
        assert_eq!(r.headers()["etag"], put.headers()["etag"]);
        let created = send(&app, Method::POST, "mp", "uploads", &[], b"").await;
        let xml = String::from_utf8(axum::body::to_bytes(created.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        let id = xml.split("<UploadId>").nth(1).unwrap().split("</UploadId>").next().unwrap().to_owned();
        taken(&w);
        let part = send(&app, Method::PUT, "mp", &format!("partNumber=1&uploadId={id}"), &[("x-amz-copy-source", "/d/src")], b"").await;
        let body = String::from_utf8(axum::body::to_bytes(part.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        let etag = body.split("<ETag>").nth(1).unwrap().split("</ETag>").next().unwrap().replace("&quot;", "\"");
        let written = taken(&w);
        assert_eq!(written[0], "shard", "{written:?}");
        let record = mem.peek(&format!("drives/{}/uploads/{id}/00001.json", d.id)).unwrap();
        let record: PartRecord = serde_json::from_slice(&record).unwrap();
        assert_eq!(record.extents, [Extent::Shard { s: ShardHash::of(&small), n: 3000 }]);
        let done = format!("<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>{etag}</ETag></Part></CompleteMultipartUpload>");
        send(&app, Method::POST, "mp", &format!("uploadId={id}"), &[], done.as_bytes()).await;
        assert_eq!(content_of(&d, "mp"), record.extents);
        assert_eq!(read_back(&app, "mp").await, small);
    }

    /// A part record that holds a data extent is not one this server wrote (format §11): the
    /// upload does not complete from it.
    #[tokio::test]
    async fn a_part_record_with_a_data_extent_is_refused() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let (app, d) = app_with(&mem, &[voidfs_core::model::INLINE_DATA]).await;
        let created = send(&app, Method::POST, "mp", "uploads", &[], b"").await;
        let xml = String::from_utf8(axum::body::to_bytes(created.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        let id = xml.split("<UploadId>").nth(1).unwrap().split("</UploadId>").next().unwrap().to_owned();
        let rec = PartRecord { part: 1, size: 2, etag: "\"x\"".into(), extents: content::inline(b"hi") };
        app.pool.store.put(&format!("drives/{}/uploads/{id}/00001.json", d.id), Bytes::from(serde_json::to_vec(&rec).unwrap())).await.unwrap();
        let done = "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>\"x\"</ETag></Part></CompleteMultipartUpload>";
        let e = send_as(&app, Method::POST, "mp", &format!("uploadId={id}"), &[], done.as_bytes(), Payload::Unsigned).await.unwrap_err();
        assert!(e.message.contains("part records never do"), "{}", e.message);
        assert!(d.snapshot().lookup(&Key::parse("mp").unwrap()).is_none());
    }

    /// A body that does not match its signature fails after it is uploaded, before anything
    /// could commit it.
    #[tokio::test]
    async fn a_body_that_does_not_match_its_signature_fails() {
        let mem = Arc::new(MemStore::new(crate::clock::Clock::System));
        let pool = pool(&mem).await;
        let (reader, tx) = body(Payload::Sha256([0; 32]));
        tx.unbounded_send(Ok(Bytes::from(random_bytes(9, 10_000)))).unwrap();
        drop(tx);
        assert_eq!(ingest(&pool, reader, true).await.unwrap_err().code, "XAmzContentSHA256Mismatch");
    }
}
