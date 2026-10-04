// SPDX-License-Identifier: Apache-2.0
//! The HTTP front end: the S3 subset and the `x-voidfs-*` extensions (protocol §3–§5).

mod bucket;
mod chunked;
mod error;
mod host;
mod object;
mod upload;
mod util;

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use http::{HeaderMap, Method, Request};

pub use error::S3Error;
pub use host::{Domains, parse_domain};
use util::{Ctx, Query};

use crate::metrics::{S3Metrics, S3Op};
use crate::pool::Pool;
use crate::sigv4::{self, Keys};

pub struct App {
    pub pool: Arc<Pool>,
    pub keys: Keys,
    /// Domains for virtual-host addressing; empty for path-style only.
    pub domains: Domains,
    pub metrics: S3Metrics,
    /// Multipart uploads being completed or aborted, and those completed until their staging
    /// records are deleted.
    pub uploads: object::Uploads,
    /// The shard reads GETs share past their own window.
    pub read_ahead: object::ReadAhead,
    /// Direct uploads (protocol §4.11), where the store presigns PUTs that bind a shard's
    /// checksum; otherwise their requests answer `501`.
    pub direct: Option<Arc<crate::direct::Direct>>,
    /// Storage credentials (protocol §5.5), where the store mints them scoped to a drive's reader;
    /// otherwise their requests answer `501`.
    pub credentials: Option<Arc<crate::credentials::Credentials>>,
}

pub fn router(app: Arc<App>) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(app)
}

async fn handle(State(app): State<Arc<App>>, req: Request<Body>) -> Response {
    let started = Instant::now();
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let is_head = req.method() == Method::HEAD;
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let mut op = S3Op::Other;
    let mut resp = match route(&app, req, &mut op).await {
        Ok(r) => r,
        Err(e) => {
            if e.status.is_server_error() {
                tracing::error!("{method} {path}: {} {}", e.code, e.message);
            }
            e.into_response(&request_id, is_head)
        }
    };
    let h = resp.headers_mut();
    h.insert("x-voidfs-protocol", http::HeaderValue::from_static("1"));
    h.insert("x-amz-request-id", http::HeaderValue::from_str(&request_id).unwrap());
    h.insert("server", http::HeaderValue::from_static("voidfs"));
    app.metrics.record(op, resp.status().as_u16(), started.elapsed());
    resp
}

/// Routes a request, and sets `op` to what it does, for metrics, before the signature is
/// checked.
async fn route(app: &Arc<App>, req: Request<Body>, op: &mut S3Op) -> Result<Response, S3Error> {
    let (parts, body) = req.into_parts();
    let virtual_host = app.domains.drive(&parts.uri, &parts.headers);
    let query = Query::parse(parts.uri.query().unwrap_or(""));
    *op = operation(&parts.method, parts.uri.path(), virtual_host.is_some(), &query, &parts.headers);
    let auth = sigv4::verify(parts.method.as_str(), &parts.uri, &parts.headers, &app.keys, chrono::Utc::now()).map_err(S3Error::auth)?;
    // The signature covers the request as sent, host included, so the drive the host names is
    // used only now.
    let (bucket, key) = match &virtual_host {
        Some(drive) => (Some(drive.clone()), util::split_key(parts.uri.path())?),
        None => util::split_path(parts.uri.path())?,
    };
    let ctx = Ctx { method: parts.method, bucket, key, virtual_host: virtual_host.is_some(), query, headers: parts.headers, auth };
    match (&ctx.bucket, &ctx.key) {
        (None, _) => bucket::service(app, &ctx).await,
        (Some(_), None) => bucket::dispatch(app, &ctx, body).await,
        (Some(_), Some(_)) => object::dispatch(app, &ctx, body).await,
    }
}

/// What a request does, from its method, path, query and headers, as `bucket::dispatch` and
/// `object::dispatch` route it.
fn operation(method: &Method, path: &str, virtual_host: bool, q: &Query, headers: &HeaderMap) -> S3Op {
    let rest = path.strip_prefix('/').unwrap_or(path);
    let (drive, object) = match (virtual_host, rest.split_once('/')) {
        (true, _) => (true, !rest.is_empty()),
        (false, None) => (!rest.is_empty(), false),
        (false, Some((_, key))) => (true, !key.is_empty()),
    };
    let extension = q.0.keys().any(|k| k.starts_with("x-voidfs-"));
    match (drive, object) {
        (false, _) if *method == Method::GET => S3Op::ListDrives,
        (false, _) => S3Op::Other,
        (true, false) => match *method {
            Method::GET if q.has("uploads") => S3Op::Multipart,
            Method::GET if extension => S3Op::Extension,
            Method::GET if q.has("versioning") || q.has("location") => S3Op::Drive,
            Method::GET => S3Op::List,
            Method::POST if q.has("delete") => S3Op::Delete,
            Method::POST if q.has("x-voidfs-undelete") => S3Op::Drive,
            Method::PUT | Method::HEAD | Method::DELETE => S3Op::Drive,
            _ => S3Op::Other,
        },
        (true, true) => match *method {
            _ if q.has("uploadId") || q.has("uploads") => S3Op::Multipart,
            _ if extension => S3Op::Extension,
            Method::GET => S3Op::Get,
            Method::HEAD => S3Op::Head,
            Method::PUT if headers.contains_key("x-amz-copy-source") => S3Op::Copy,
            Method::PUT => S3Op::Put,
            Method::DELETE => S3Op::Delete,
            _ => S3Op::Other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(method: &str, path: &str, query: &str) -> S3Op {
        operation(&method.parse().unwrap(), path, false, &Query::parse(query), &HeaderMap::new())
    }

    #[test]
    fn requests_are_classified_by_what_they_do() {
        assert_eq!(op("GET", "/", ""), S3Op::ListDrives);
        assert_eq!(op("POST", "/", ""), S3Op::Other);
        assert_eq!(op("PUT", "/photos", ""), S3Op::Drive);
        assert_eq!(op("HEAD", "/photos/", ""), S3Op::Drive);
        assert_eq!(op("DELETE", "/photos", ""), S3Op::Drive);
        assert_eq!(op("POST", "/photos", "x-voidfs-undelete"), S3Op::Drive);
        assert_eq!(op("GET", "/photos", "versioning"), S3Op::Drive);
        assert_eq!(op("GET", "/photos", "list-type=2&prefix=a%2F"), S3Op::List);
        assert_eq!(op("GET", "/photos", "versions"), S3Op::List);
        assert_eq!(op("GET", "/photos", "uploads"), S3Op::Multipart);
        assert_eq!(op("GET", "/photos", "x-voidfs-changes&since=3"), S3Op::Extension);
        assert_eq!(op("POST", "/photos", "delete"), S3Op::Delete);
        assert_eq!(op("POST", "/photos", ""), S3Op::Other);
        assert_eq!(op("GET", "/photos/a/b.jpg", ""), S3Op::Get);
        assert_eq!(op("GET", "/photos/a/b.jpg", "versionId=1.0"), S3Op::Get);
        assert_eq!(op("HEAD", "/photos/a/b.jpg", ""), S3Op::Head);
        assert_eq!(op("PUT", "/photos/a/b.jpg", ""), S3Op::Put);
        assert_eq!(op("DELETE", "/photos/a/b.jpg", ""), S3Op::Delete);
        assert_eq!(op("POST", "/photos/a/b.jpg", "uploads"), S3Op::Multipart);
        assert_eq!(op("PUT", "/photos/a/b.jpg", "partNumber=1&uploadId=u"), S3Op::Multipart);
        assert_eq!(op("DELETE", "/photos/a/b.jpg", "uploadId=u"), S3Op::Multipart);
        assert_eq!(op("GET", "/photos/a/b.jpg", "uploadId=u"), S3Op::Multipart);
        assert_eq!(op("PUT", "/photos/a/b.jpg", "x-voidfs-rename"), S3Op::Extension);
        assert_eq!(op("POST", "/photos/a/b.jpg", "x-voidfs-patch"), S3Op::Extension);
        assert_eq!(op("GET", "/photos/a/b.jpg", "x-voidfs-versions"), S3Op::Extension);
        assert_eq!(op("POST", "/photos/a/b.jpg", ""), S3Op::Other);
        assert_eq!(op("PATCH", "/photos/a/b.jpg", ""), S3Op::Other);
        let mut copy = HeaderMap::new();
        copy.insert("x-amz-copy-source", "/photos/c.jpg".parse().unwrap());
        assert_eq!(operation(&Method::PUT, "/photos/a/b.jpg", false, &Query::parse(""), &copy), S3Op::Copy);
        // Virtual-host style: the path holds only the key.
        let vh = |method: &str, path: &str| operation(&method.parse().unwrap(), path, true, &Query::parse(""), &HeaderMap::new());
        assert_eq!(vh("GET", "/"), S3Op::List);
        assert_eq!(vh("PUT", "/"), S3Op::Drive);
        assert_eq!(vh("GET", "/b.jpg"), S3Op::Get);
        assert_eq!(vh("GET", "/a/b.jpg"), S3Op::Get);
    }
}
