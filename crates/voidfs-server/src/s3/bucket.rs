// SPDX-License-Identifier: Apache-2.0
//! Drive-level operations (protocol §3, §4.9, §4.10, §5).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use base64::Engine;
use http::Method;
use serde_json::json;
use voidfs_core::ids::VersionId;
use voidfs_core::model::{Kind, ObjectRecord};
use voidfs_core::names::Key;
use voidfs_core::state::{DriveState, Listed};

use super::util::{Ctx, S3_NS, empty, format_mode, iso, json, xml, xml_escape};
use super::{App, S3Error, object};
use crate::pool::{CreateError, Drive, FeedBatch};
use crate::sigv4::Scope;

/// `GET /`: ListBuckets.
pub async fn service(app: &Arc<App>, ctx: &Ctx) -> Result<Response, S3Error> {
    if ctx.method != Method::GET {
        return Err(S3Error::not_implemented("only ListBuckets is served at /"));
    }
    ctx.require(Scope::Read)?;
    let mut buckets = String::new();
    for d in app.pool.list_drives() {
        if ctx.auth.key.reaches(&d.alias, d.id.as_str()) {
            buckets.push_str(&format!(
                "<Bucket><Name>{}</Name><CreationDate>{}</CreationDate></Bucket>",
                xml_escape(&d.alias),
                iso(d.desc.created)
            ));
        }
    }
    Ok(xml(
        200,
        format!("<ListAllMyBucketsResult xmlns=\"{S3_NS}\"><Owner><ID>voidfs</ID><DisplayName>voidfs</DisplayName></Owner><Buckets>{buckets}</Buckets></ListAllMyBucketsResult>"),
    ))
}

const GET_KNOWN: &[&str] = &[
    "list-type", "prefix", "delimiter", "max-keys", "continuation-token", "start-after", "encoding-type", "marker", "fetch-owner",
    "versions", "key-marker", "version-id-marker", "versioning", "location", "uploads", "upload-id-marker", "x-voidfs-drive",
    "x-voidfs-list", "x-voidfs-deleted", "x-voidfs-changes", "x-voidfs-credentials", "since",
];

pub async fn dispatch(app: &Arc<App>, ctx: &Ctx, body: Body) -> Result<Response, S3Error> {
    let q = &ctx.query;
    match ctx.method {
        Method::PUT if q.has("versioning") => Err(S3Error::not_implemented("versioning is always enabled")),
        Method::PUT if q.0.is_empty() => create(app, ctx).await,
        Method::HEAD => {
            ctx.require(Scope::Read)?;
            ctx.drive(app)?;
            Ok(empty(200).body(Body::empty()).unwrap())
        }
        Method::DELETE if q.0.is_empty() => delete(app, ctx).await,
        Method::POST if q.has("x-voidfs-undelete") => undelete(app, ctx).await,
        Method::POST if q.has("delete") => object::delete_objects(app, ctx, body).await,
        Method::GET => {
            if let Some(unknown) = q.unknown(GET_KNOWN) {
                return Err(S3Error::not_implemented(format!("?{unknown} is not supported")));
            }
            ctx.require(Scope::Read)?;
            let d = ctx.drive(app)?;
            if q.has("versioning") {
                Ok(xml(200, format!("<VersioningConfiguration xmlns=\"{S3_NS}\"><Status>Enabled</Status></VersioningConfiguration>")))
            } else if q.has("location") {
                Ok(xml(200, format!("<LocationConstraint xmlns=\"{S3_NS}\"/>")))
            } else if q.has("versions") {
                list_versions(ctx, &d)
            } else if q.has("uploads") {
                object::list_uploads(app, ctx, &d).await
            } else if q.has("x-voidfs-drive") {
                describe(app, &d)
            } else if q.has("x-voidfs-list") {
                list_attrs(ctx, &d)
            } else if q.has("x-voidfs-deleted") {
                list_deleted(ctx, &d)
            } else if q.has("x-voidfs-changes") {
                changes(ctx, &d).await
            } else if q.has("x-voidfs-credentials") {
                Err(S3Error::not_implemented("this deployment cannot issue storage credentials yet"))
            } else {
                list_objects(ctx, &d)
            }
        }
        _ => Err(S3Error::not_implemented(format!("{} on a drive with these parameters is not supported", ctx.method))),
    }
}

fn valid_bucket_name(n: &str) -> bool {
    (3..=63).contains(&n.len())
        && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && n.as_bytes()[0].is_ascii_alphanumeric()
        && n.as_bytes()[n.len() - 1].is_ascii_alphanumeric()
        && n.parse::<voidfs_core::DriveId>().is_err()
}

async fn create(app: &Arc<App>, ctx: &Ctx) -> Result<Response, S3Error> {
    ctx.require(Scope::Admin)?;
    let name = ctx.bucket();
    if !valid_bucket_name(name) {
        return Err(S3Error::new(400, "InvalidBucketName", format!("{name:?} is not a valid drive name")));
    }
    let source = match ctx.header("x-voidfs-fork-source") {
        None => None,
        Some(_) if ctx.header("x-voidfs-fork-mode").is_some_and(|m| m == "copy") => {
            return Err(S3Error::not_implemented("copy-mode forks are not supported yet"));
        }
        Some(src) => Some(ctx.drive_named(app, src)?),
    };
    let d = app.pool.create_drive(name, source.as_ref()).await.map_err(|e| match e {
        CreateError::Exists => S3Error::new(409, "BucketAlreadyOwnedByYou", "a drive with this name already exists"),
        CreateError::NotFound => S3Error::no_bucket(),
        CreateError::Other(e) => S3Error::from(e),
    })?;
    let mut b = empty(200).header("location", format!("/{}", d.alias)).header("x-voidfs-drive-id", d.id.as_str());
    if let Some(src) = &source {
        b = b.header("x-voidfs-fork-source-id", src.id.as_str()).header("x-voidfs-fork-point", d.desc.created.to_string());
    }
    Ok(b.body(Body::empty()).unwrap())
}

async fn delete(app: &Arc<App>, ctx: &Ctx) -> Result<Response, S3Error> {
    ctx.require(Scope::Admin)?;
    if ctx.header("x-voidfs-hard-delete").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        if let Some(d) = app.pool.drive(ctx.bucket())
            && !ctx.auth.key.reaches(&d.alias, d.id.as_str()) {
                return Err(S3Error::no_bucket());
            }
        return if app.pool.hard_delete(ctx.bucket()).await? { Ok(empty(204).body(Body::empty()).unwrap()) } else { Err(S3Error::no_bucket()) };
    }
    let d = ctx.drive(app)?;
    app.pool.soft_delete(&d).await?;
    Ok(empty(204).body(Body::empty()).unwrap())
}

async fn undelete(app: &Arc<App>, ctx: &Ctx) -> Result<Response, S3Error> {
    ctx.require(Scope::Admin)?;
    let d = app.pool.undelete(ctx.bucket()).await.map_err(|e| match e {
        CreateError::Exists => S3Error::new(409, "BucketAlreadyOwnedByYou", "a live drive already has this name"),
        CreateError::NotFound => S3Error::no_bucket(),
        CreateError::Other(e) => S3Error::from(e),
    })?;
    Ok(empty(200).header("x-voidfs-drive-id", d.id.as_str()).body(Body::empty()).unwrap())
}

fn describe(app: &Arc<App>, d: &Drive) -> Result<Response, S3Error> {
    let s = d.snapshot();
    let fork_of = d.desc.fork_of.as_ref().map(|f| json!({ "driveId": f.drive_id, "forkPoint": d.desc.created, "seq": f.seq }));
    let forks: Vec<String> = d.forks.read().unwrap().iter().filter(|f| app.pool.drive_by_id(f).is_some()).map(|f| f.to_string()).collect();
    Ok(json(&json!({
        "driveId": d.id,
        "alias": d.alias,
        "displayName": d.alias,
        "createdAt": d.desc.created,
        "forkOf": fork_of,
        "forks": forks,
        "seq": s.seq(),
        "usageBytes": s.live_bytes(),
    })))
}

fn encode_key(ctx: &Ctx, key: &str) -> String {
    if ctx.query.get("encoding-type") == Some("url") {
        const SET: &percent_encoding::AsciiSet =
            &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~').remove(b'/');
        percent_encoding::utf8_percent_encode(key, SET).to_string()
    } else {
        xml_escape(key)
    }
}

fn record_of<'a>(s: &'a DriveState, item: &Listed) -> Option<&'a ObjectRecord> {
    match item {
        Listed::Object { oid, .. } => s.record(oid),
        Listed::Prefix { .. } => None,
    }
}

/// ListObjectsV2 and ListObjects (v1).
fn list_objects(ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let q = &ctx.query;
    let v2 = q.get("list-type") == Some("2");
    let prefix = q.get("prefix").unwrap_or("");
    let delimiter = q.get("delimiter").filter(|d| !d.is_empty());
    if delimiter.is_some_and(|d| d != "/") {
        return Err(S3Error::not_implemented("only \"/\" is supported as a delimiter"));
    }
    let max = ctx.max_keys()?;
    let token = q.get("continuation-token").map(|t| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(t).ok().and_then(|b| String::from_utf8(b).ok()).ok_or(())
    });
    let after = match (v2, token) {
        (true, Some(Ok(t))) => Some(t),
        (true, Some(Err(()))) => return Err(S3Error::invalid("the continuation token is not valid")),
        (true, None) => q.get("start-after").map(str::to_owned),
        (false, _) => q.get("marker").map(str::to_owned),
    };
    let s = d.snapshot();
    let (items, truncated) = s.list(prefix, delimiter.is_some(), after.as_deref(), max);
    let mut contents = String::new();
    let mut prefixes = String::new();
    for item in &items {
        match item {
            Listed::Prefix { key } => prefixes.push_str(&format!("<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>", encode_key(ctx, key))),
            Listed::Object { key, .. } => {
                let r = record_of(&s, item);
                contents.push_str(&format!(
                    "<Contents><Key>{}</Key><LastModified>{}</LastModified><ETag>{}</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                    encode_key(ctx, key),
                    r.map(|r| iso(r.time)).unwrap_or_default(),
                    xml_escape(r.map(|r| r.etag.as_str()).unwrap_or("\"\"")),
                    r.map_or(0, |r| r.size)
                ));
            }
        }
    }
    let last = items.last().map(|i| i.key().to_owned());
    let mut head = format!(
        "<Name>{}</Name><Prefix>{}</Prefix><MaxKeys>{max}</MaxKeys><IsTruncated>{truncated}</IsTruncated>",
        xml_escape(&d.alias),
        encode_key(ctx, prefix)
    );
    if let Some(dl) = delimiter {
        head.push_str(&format!("<Delimiter>{}</Delimiter>", xml_escape(dl)));
    }
    if q.get("encoding-type") == Some("url") {
        head.push_str("<EncodingType>url</EncodingType>");
    }
    if v2 {
        head.push_str(&format!("<KeyCount>{}</KeyCount>", items.len()));
        if let Some(t) = q.get("continuation-token") {
            head.push_str(&format!("<ContinuationToken>{}</ContinuationToken>", xml_escape(t)));
        }
        if let Some(sa) = q.get("start-after") {
            head.push_str(&format!("<StartAfter>{}</StartAfter>", encode_key(ctx, sa)));
        }
        if truncated {
            let t = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(last.unwrap_or_default());
            head.push_str(&format!("<NextContinuationToken>{t}</NextContinuationToken>"));
        }
    } else {
        head.push_str(&format!("<Marker>{}</Marker>", encode_key(ctx, q.get("marker").unwrap_or(""))));
        if truncated && delimiter.is_some() {
            head.push_str(&format!("<NextMarker>{}</NextMarker>", encode_key(ctx, &last.unwrap_or_default())));
        }
    }
    Ok(xml(200, format!("<ListBucketResult xmlns=\"{S3_NS}\">{head}{contents}{prefixes}</ListBucketResult>")))
}

/// ListObjectVersions: every content version of matching keys, newest first per key.
fn list_versions(ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let q = &ctx.query;
    let prefix = q.get("prefix").unwrap_or("");
    let delimiter = q.get("delimiter").filter(|d| !d.is_empty());
    if delimiter.is_some_and(|d| d != "/") {
        return Err(S3Error::not_implemented("only \"/\" is supported as a delimiter"));
    }
    let max = ctx.max_keys()?;
    let key_marker = q.get("key-marker").filter(|k| !k.is_empty());
    let version_marker = q.get("version-id-marker").and_then(|v| v.parse::<VersionId>().ok());
    let s = d.snapshot();
    // Markers are applied below, so the marker key's remaining versions can be resumed.
    let (items, _) = s.list(prefix, delimiter.is_some(), None, usize::MAX / 2);
    let mut out = String::new();
    let mut prefixes = String::new();
    let mut count = 0;
    let mut truncated = false;
    let mut next: Option<(String, VersionId)> = None;
    'outer: for item in &items {
        match item {
            Listed::Prefix { key } => {
                if key_marker.is_some_and(|m| key.as_str() <= m) {
                    continue;
                }
                prefixes.push_str(&format!("<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>", encode_key(ctx, key)));
            }
            Listed::Object { key, oid } => {
                if key_marker.is_some_and(|m| key.as_str() < m) || (key_marker == Some(key.as_str()) && version_marker.is_none()) {
                    continue;
                }
                let Some(rows) = s.history(oid) else { continue };
                let head = s.record(oid).map(|r| r.head);
                let content_rows: Vec<_> = rows.iter().filter(|r| r.op.follows_content()).collect();
                let latest = content_rows.last().map(|r| r.version);
                for r in content_rows.iter().rev() {
                    if key_marker == Some(key.as_str()) && version_marker.is_some_and(|m| r.version >= m) {
                        continue;
                    }
                    if count == max {
                        truncated = true;
                        break 'outer;
                    }
                    let is_latest = Some(r.version) == latest && head.is_some();
                    out.push_str(&format!(
                        "<Version><Key>{}</Key><VersionId>{}</VersionId><IsLatest>{is_latest}</IsLatest><LastModified>{}</LastModified><ETag>{}</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Version>",
                        encode_key(ctx, key),
                        r.version,
                        iso(r.time),
                        xml_escape(&r.etag),
                        r.size
                    ));
                    count += 1;
                    next = Some((key.clone(), r.version));
                }
            }
        }
    }
    let mut head = format!(
        "<Name>{}</Name><Prefix>{}</Prefix><KeyMarker>{}</KeyMarker><VersionIdMarker>{}</VersionIdMarker><MaxKeys>{max}</MaxKeys><IsTruncated>{truncated}</IsTruncated>",
        xml_escape(&d.alias),
        encode_key(ctx, prefix),
        encode_key(ctx, key_marker.unwrap_or("")),
        q.get("version-id-marker").map(xml_escape).unwrap_or_default()
    );
    if truncated
        && let Some((k, v)) = next {
            head.push_str(&format!("<NextKeyMarker>{}</NextKeyMarker><NextVersionIdMarker>{v}</NextVersionIdMarker>", encode_key(ctx, &k)));
        }
    Ok(xml(200, format!("<ListVersionsResult xmlns=\"{S3_NS}\">{head}{out}{prefixes}</ListVersionsResult>")))
}

/// `?x-voidfs-list`: one folder's children with attributes (protocol §4.9).
fn list_attrs(ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let prefix = ctx.query.get("prefix").unwrap_or("");
    if !prefix.is_empty() && !prefix.ends_with('/') {
        return Err(S3Error::invalid("prefix must be a folder key ending in /"));
    }
    let key = Key::parse(prefix).map_err(|e| S3Error::invalid(e.to_string()))?;
    let s = d.snapshot();
    let folder = s.lookup(&key).ok_or_else(S3Error::no_key)?;
    let max = ctx.max_keys()?;
    let after = ctx.query.get("continuation-token").map(|t| t.as_bytes().to_vec());
    let mut entries = Vec::new();
    let mut more = false;
    for (seg, oid) in s.children(&folder) {
        if after.as_ref().is_some_and(|a| seg <= a.as_slice()) {
            continue;
        }
        if entries.len() == max {
            more = true;
            break;
        }
        let Some(r) = s.record(oid) else { continue };
        let name = String::from_utf8_lossy(seg).into_owned();
        let mut e = json!({
            "name": name,
            "kind": r.kind,
            "objectId": r.oid,
            "versionId": r.head.to_string(),
            "mtime": r.attrs.mtime.unwrap_or(r.time),
            "mode": format_mode(r.attrs.mode, r.kind),
            "hasXattrs": !r.attrs.xattrs.is_empty(),
        });
        if r.kind != Kind::Folder {
            e["size"] = json!(r.size);
            e["etag"] = json!(r.etag);
        }
        if let Some(t) = &r.target {
            e["target"] = json!(t);
        }
        entries.push(e);
    }
    let next = if more { entries.last().and_then(|e| e["name"].as_str().map(str::to_owned)) } else { None };
    Ok(json(&json!({ "prefix": prefix, "seq": s.seq(), "entries": entries, "nextContinuationToken": next })))
}

/// `?x-voidfs-deleted` (protocol §4.10).
fn list_deleted(ctx: &Ctx, d: &Drive) -> Result<Response, S3Error> {
    let prefix = ctx.query.get("prefix").unwrap_or("");
    let max = ctx.max_keys()?;
    let after = ctx.query.get("continuation-token");
    let s = d.snapshot();
    let mut rows: Vec<_> = s.removed(prefix, after).take(max + 1).collect();
    let more = rows.len() > max;
    rows.truncate(max);
    let next = if more { rows.last().map(|r| r.key.clone()) } else { None };
    let deleted: Vec<_> = rows
        .iter()
        .map(|r| json!({ "key": r.key, "objectId": r.oid, "deletedAt": r.time, "lastVersionId": r.last_version.to_string(), "size": r.size, "kind": r.kind }))
        .collect();
    Ok(json(&json!({ "deleted": deleted, "nextContinuationToken": next })))
}

fn feed_json(batches: &[FeedBatch], limit: usize) -> (Vec<serde_json::Value>, Option<u64>, bool) {
    let mut out = Vec::new();
    let mut last = None;
    for b in batches {
        if out.len() + b.changes.len() > limit && !out.is_empty() {
            return (out, last, true);
        }
        for c in &b.changes {
            let mut v = serde_json::to_value(c).unwrap();
            v["seq"] = json!(b.seq);
            v["time"] = json!(b.time);
            v["versionId"] = json!(c.version_id.to_string());
            out.push(v);
        }
        last = Some(b.seq);
    }
    (out, last, false)
}

/// `?x-voidfs-changes` (protocol §5.6): long poll, or Server-Sent Events.
async fn changes(ctx: &Ctx, d: &Arc<Drive>) -> Result<Response, S3Error> {
    let since: u64 = ctx.query.get("since").and_then(|s| s.parse().ok()).ok_or_else(|| S3Error::invalid("since must be a drive position"))?;
    let expired = || S3Error::new(410, "ChangesExpired", "changes that old are no longer held; relist and resume");
    if ctx.header("accept").is_some_and(|a| a.contains("text/event-stream")) {
        let d = d.clone();
        let mut rx = d.subscribe();
        let first = d.changes_since(since).ok_or_else(expired)?;
        let stream = async_stream(d, rx.clone(), since, first);
        rx.mark_unchanged();
        return Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))).into_response());
    }
    let wait: u64 = ctx.header("x-voidfs-wait").and_then(|w| w.parse().ok()).unwrap_or(0).min(60);
    let mut rx = d.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait);
    loop {
        let batches = d.changes_since(since).ok_or_else(expired)?;
        if !batches.is_empty() || tokio::time::Instant::now() >= deadline {
            let (changes, last, more) = feed_json(&batches, 1000);
            return Ok(json(&json!({ "seq": last.unwrap_or(since), "changes": changes, "more": more })));
        }
        if tokio::time::timeout_at(deadline, rx.changed()).await.is_err() {
            continue;
        }
    }
}

fn async_stream(
    d: Arc<Drive>,
    rx: tokio::sync::watch::Receiver<u64>,
    since: u64,
    first: Vec<FeedBatch>,
) -> impl futures::Stream<Item = Result<Event, std::convert::Infallible>> {
    let events = |batches: Vec<FeedBatch>| {
        batches
            .into_iter()
            .map(|b| {
                let (changes, _, _) = feed_json(std::slice::from_ref(&b), usize::MAX);
                Ok(Event::default().id(b.seq.to_string()).data(json!({ "seq": b.seq, "time": b.time, "changes": changes }).to_string()))
            })
            .collect::<Vec<_>>()
    };
    let last = first.last().map_or(since, |b| b.seq);
    let initial = futures::stream::iter(events(first));
    let live = futures::stream::unfold((d, rx, last), move |(d, mut rx, last)| async move {
        rx.changed().await.ok()?;
        let batches = d.changes_since(last)?;
        let new_last = batches.last().map_or(last, |b| b.seq);
        Some((futures::stream::iter(events(batches)), (d, rx, new_last)))
    });
    use futures::StreamExt;
    initial.chain(live.flatten())
}
