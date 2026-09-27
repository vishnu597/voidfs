// SPDX-License-Identifier: Apache-2.0
//! The HTTP front end: the S3 subset and the `x-voidfs-*` extensions (protocol §3–§5).

mod bucket;
mod chunked;
mod error;
mod object;
mod util;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use http::{Method, Request};

pub use error::S3Error;
use util::{Ctx, Query};

use crate::pool::Pool;
use crate::sigv4::{self, Keys};

pub struct App {
    pub pool: Arc<Pool>,
    pub keys: Keys,
}

pub fn router(app: Arc<App>) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(app)
}

async fn handle(State(app): State<Arc<App>>, req: Request<Body>) -> Response {
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let is_head = req.method() == Method::HEAD;
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let mut resp = match route(&app, req).await {
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
    resp
}

async fn route(app: &Arc<App>, req: Request<Body>) -> Result<Response, S3Error> {
    let (parts, body) = req.into_parts();
    let auth = sigv4::verify(parts.method.as_str(), &parts.uri, &parts.headers, &app.keys, chrono::Utc::now()).map_err(S3Error::auth)?;
    let (bucket, key) = util::split_path(parts.uri.path())?;
    let query = Query::parse(parts.uri.query().unwrap_or(""));
    let ctx = Ctx { method: parts.method, bucket, key, query, headers: parts.headers, auth };
    match (&ctx.bucket, &ctx.key) {
        (None, _) => bucket::service(app, &ctx).await,
        (Some(_), None) => bucket::dispatch(app, &ctx, body).await,
        (Some(_), Some(_)) => object::dispatch(app, &ctx, body).await,
    }
}
