// SPDX-License-Identifier: Apache-2.0
//! Object-level operations (protocol §3, §4).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use futures::StreamExt;
use http::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use voidfs_core::chunk::{Shard, StreamChunker};
use voidfs_core::content::{self, EditError, Edited};
use voidfs_core::ids::{ObjectId, ShardHash, Timestamp, VersionId};
use voidfs_core::model::{Attrs, ContentDescriptor, Extent, HistoryRow, Kind, ObjectRecord, Op};
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
                match piece.shard {
                    Some(h) => {
                        let bytes = pool.shard(&h).await.map_err(std::io::Error::other)?;
                        Ok::<_, std::io::Error>(vec![bytes.slice(piece.offset as usize..(piece.offset + piece.len) as usize)])
                    }
                    None => {
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

/// Streams a body into shards; returns the content's extents.
async fn ingest(pool: &Pool, mut reader: BodyReader) -> Result<Vec<Extent>, S3Error> {
    let mut chunker = StreamChunker::new(pool.params);
    let mut shards: Vec<Shard> = Vec::new();
    let mut extents = Vec::new();
    let store = async |shards: &mut Vec<Shard>, extents: &mut Vec<Extent>| -> Result<(), S3Error> {
        if shards.is_empty() {
            return Ok(());
        }
        pool.write_shards(shards).await?;
        extents.extend(shards.iter().map(|s| Extent::Shard { s: s.hash, n: s.bytes.len() as u64 }));
        shards.clear();
        Ok(())
    };
    while let Some(data) = reader.next().await? {
        shards.extend(chunker.push(&data));
        if shards.len() >= 4 {
            store(&mut shards, &mut extents).await?;
        }
    }
    shards.extend(chunker.finish());
    store(&mut shards, &mut extents).await?;
    // Nothing is committed until the whole body matched its signature.
    reader.finish()?;
    Ok(content::normalize(extents))
}

fn mutation_response(v: VersionId, s: &DriveState, key: &str) -> http::response::Builder {
    let mut b = empty(200).header("x-amz-version-id", v.to_string());
    if let Some(r) = Key::parse(key).ok().and_then(|k| s.lookup(&k)).and_then(|o| s.record(&o)) {
        b = b.header("etag", &r.etag).header("x-voidfs-size", r.size);
    }
    b
}

async fn commit(app: &App, d: &Drive, plan: impl FnOnce(&DriveState) -> Result<voidfs_core::model::Txn, CommitError>) -> Result<(VersionId, Arc<DriveState>), S3Error> {
    Ok(app.pool.commit(d, plan).await?)
}

async fn put(app: &Arc<App>, ctx: &Ctx, d: &Drive, body: Body) -> Result<Response, S3Error> {
    let key = ctx.key().to_owned();
    let attrs = ctx.attrs_for_put()?;
    let pre = ctx.precondition();
    let reader = BodyReader::new(body, ctx, MAX_PUT)?;
    let extents = ingest(&app.pool, reader).await?;
    let desc = app.pool.describe(extents).await?;
    let actor = ctx.actor();
    let (v, s) = commit(app, d, |st| Ok(ops::put(st, &key, desc, attrs, Op::Put, &pre, &actor)?)).await?;
    Ok(mutation_response(v, &s, &key).body(Body::empty()).unwrap())
}

async fn copy(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
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
    let (v, s) = commit(app, d, |st| Ok(ops::put(st, &key, desc, attrs, Op::Copy, &pre, &actor)?)).await?;
    let r = s.lookup(&parse_key(&key)?).and_then(|o| s.record(&o).cloned()).ok_or_else(S3Error::no_key)?;
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

async fn delete(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let key = ctx.key().to_owned();
    let pre = ctx.precondition();
    let actor = ctx.actor();
    let r = app
        .pool
        .commit(d, |st| match ops::delete(st, &key, &pre, &actor)? {
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
    let mut out = String::new();
    for (key, version) in req.keys {
        if version.is_some() {
            out.push_str(&format!(
                "<Error><Key>{}</Key><Code>NotImplemented</Code><Message>history is immutable</Message></Error>",
                xml_escape(&key)
            ));
            continue;
        }
        let pre = Default::default();
        let r = app
            .pool
            .commit(&d, |st| match ops::delete(st, &key, &pre, &actor)? {
                Some(t) => Ok(t),
                None => Err(CommitError::Op(ops::OpError::NoSuchKey)),
            })
            .await;
        match r {
            Ok(_) | Err(CommitError::Op(ops::OpError::NoSuchKey)) => {
                if !req.quiet {
                    out.push_str(&format!("<Deleted><Key>{}</Key></Deleted>", xml_escape(&key)));
                }
            }
            Err(e) => {
                let e: S3Error = e.into();
                out.push_str(&format!("<Error><Key>{}</Key><Code>{}</Code><Message>{}</Message></Error>", xml_escape(&key), e.code, xml_escape(&e.message)));
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

fn edit_range(edit: &Edit, size: u64) -> Vec<(u64, u64)> {
    match edit {
        Edit::Write { offset, data } => vec![((*offset).min(size), offset + data.len() as u64)],
        Edit::Splice { offset, remove, .. } => vec![(*offset, offset + remove)],
        Edit::Patch { body } => voidfs_core::patch::decode(body)
            .map(|es| es.iter().map(|e| (e.offset.min(size), e.offset + e.data.len() as u64)).collect())
            .unwrap_or_default(),
    }
}

async fn run_edit(app: &Arc<App>, ctx: &Ctx, d: &Drive, edit: Edit, size: Option<u64>) -> Result<Response, S3Error> {
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
        app.pool.write_shards(&edited.new_shards).await?;
        let desc = app.pool.describe(edited.extents).await?;
        let r = app
            .pool
            .commit(d, |st| {
                let head = Key::parse(&key).ok().and_then(|k| st.lookup(&k)).and_then(|o| st.record(&o)).map(|r| r.head);
                if head != base_head {
                    return Err(CommitError::Retry);
                }
                Ok(ops::write(st, &key, desc, &patch, &pre, &actor)?)
            })
            .await;
        match r {
            Ok((v, s)) => return Ok(mutation_response(v, &s, &key).body(Body::empty()).unwrap()),
            Err(CommitError::Retry) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(CommitError::Retry.into())
}

async fn write(app: &Arc<App>, ctx: &Ctx, d: &Drive, body: Body) -> Result<Response, S3Error> {
    let offset = header_u64(ctx, "x-voidfs-offset")?.ok_or_else(|| S3Error::invalid("x-voidfs-offset is required"))?;
    let size = header_u64(ctx, "x-voidfs-size")?;
    let data = BodyReader::new(body, ctx, MAX_EXTENSION_BODY)?.read_all().await?;
    run_edit(app, ctx, d, Edit::Write { offset, data }, size).await
}

async fn splice(app: &Arc<App>, ctx: &Ctx, d: &Drive, body: Body) -> Result<Response, S3Error> {
    let offset = header_u64(ctx, "x-voidfs-offset")?.ok_or_else(|| S3Error::invalid("x-voidfs-offset is required"))?;
    let remove = header_u64(ctx, "x-voidfs-remove")?.unwrap_or(0);
    let data = BodyReader::new(body, ctx, MAX_EXTENSION_BODY)?.read_all().await?;
    if remove == 0 && data.is_empty() {
        return Err(S3Error::invalid("a splice must insert or remove bytes"));
    }
    run_edit(app, ctx, d, Edit::Splice { offset, remove, data }, None).await
}

async fn patch(app: &Arc<App>, ctx: &Ctx, d: &Drive, body: Body) -> Result<Response, S3Error> {
    let size = header_u64(ctx, "x-voidfs-size")?;
    let body = BodyReader::new(body, ctx, MAX_EXTENSION_BODY)?.read_all().await?;
    voidfs_core::patch::decode(&body).map_err(|e| S3Error::new(400, "InvalidPatch", e.to_string()))?;
    run_edit(app, ctx, d, Edit::Patch { body }, size).await
}

// ---------------------------------------------------------------------------------------------
// Rename, restore, attributes, history

async fn rename(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let raw = ctx.header("x-voidfs-source").ok_or_else(|| S3Error::invalid("x-voidfs-source is required"))?;
    let src = percent_encoding::percent_decode_str(raw).decode_utf8().map_err(|_| S3Error::invalid("x-voidfs-source is not UTF-8"))?.into_owned();
    let replace = ctx.header("x-voidfs-replace").is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let dst = ctx.key().to_owned();
    let pre = ctx.precondition();
    let patch = ctx.attrs_patch()?;
    let actor = ctx.actor();
    let (v, s) = commit(app, d, |st| Ok(ops::rename(st, &src, &dst, replace, &patch, &pre, &actor)?)).await?;
    Ok(mutation_response(v, &s, &dst).body(Body::empty()).unwrap())
}

async fn restore(app: &Arc<App>, ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
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
            let (nv, s) = commit(app, d, |st| Ok(ops::restore(st, &key, v, &pre, &actor)?)).await?;
            Ok(mutation_response(nv, &s, &key).header("x-voidfs-restored-from", v.to_string()).body(Body::empty()).unwrap())
        }
        (None, Some(t)) => {
            let t: Timestamp = t.parse().map_err(|_| S3Error::invalid("x-voidfs-as-of is not an RFC 3339 timestamp"))?;
            if key.ends_with('/') {
                let then = app.pool.state_at(d, t).await.map_err(|e| S3Error::invalid(format!("{e:#}")))?;
                let (nv, s) = commit(app, d, |st| Ok(ops::restore_subtree(st, &then, &key, &actor)?)).await?;
                Ok(mutation_response(nv, &s, &key).body(Body::empty()).unwrap())
            } else {
                let snap = d.snapshot();
                let oid = snap.lookup(&parse_key(&key)?).ok_or_else(S3Error::no_key)?;
                let v = snap.as_of(&oid, t).map(|r| r.version).ok_or_else(|| S3Error::new(404, "NoSuchVersion", "the object did not exist then"))?;
                let (nv, s) = commit(app, d, |st| Ok(ops::restore(st, &key, v, &pre, &actor)?)).await?;
                Ok(mutation_response(nv, &s, &key).header("x-voidfs-restored-from", v.to_string()).body(Body::empty()).unwrap())
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

async fn post_attrs(app: &Arc<App>, ctx: &Ctx, d: &Drive, body: Body) -> Result<Response, S3Error> {
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
    let (v, s) = commit(app, d, |st| Ok(ops::set_attrs(st, &key, &patch, &pre, &actor)?)).await?;
    Ok(mutation_response(v, &s, &key).body(Body::empty()).unwrap())
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
    let rec = UploadRecord { key: key.clone(), created: Timestamp::now(), actor: ctx.actor(), attrs: ctx.attrs_for_put()? };
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
        app.pool.extents(r.content.as_ref().unwrap_or(&ContentDescriptor::empty())).await?
    } else {
        ingest(&app.pool, BodyReader::new(body, ctx, MAX_PUT)?).await?
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
            out.push(serde_json::from_slice::<PartRecord>(&b).map_err(anyhow::Error::from)?);
        }
    }
    out.sort_by_key(|p| p.part);
    Ok(out)
}

async fn complete_upload(app: &Arc<App>, ctx: &Ctx, d: &Drive, body: Body) -> Result<Response, S3Error> {
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
        extents.extend(p.extents.iter().copied());
    }
    let desc = app.pool.describe(content::normalize(extents)).await?;
    let pre = ctx.precondition();
    let actor = ctx.actor();
    let (v, s) = commit(app, d, |st| Ok(ops::put(st, &key, desc, rec.attrs, Op::Put, &pre, &actor)?)).await?;
    let _ = app.pool.store.delete_prefix(&dir).await;
    let etag = s.lookup(&parse_key(&key)?).and_then(|o| s.record(&o).map(|r| r.etag.clone())).unwrap_or_default();
    let mut resp = xml(
        200,
        format!(
            "<CompleteMultipartUploadResult xmlns=\"{S3_NS}\"><Location>/{}/{}</Location><Bucket>{}</Bucket><Key>{}</Key><ETag>{}</ETag></CompleteMultipartUploadResult>",
            xml_escape(ctx.bucket()),
            xml_escape(&key),
            xml_escape(ctx.bucket()),
            xml_escape(&key),
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
