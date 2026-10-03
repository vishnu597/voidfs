// SPDX-License-Identifier: Apache-2.0
//! The upload queue on the socket: batches handed to the daemon, listed, watched, paused, resumed,
//! cancelled and limited.

use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use serde::Deserialize;
use voidfs_client::{Import, Item};

use crate::api::{Affected, Cleared, Limit, NewBatch, Queued, Scope, UploadList};
use crate::server::{Answer, Failure, Shared};

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub(crate) struct ListQuery {
    /// Finished items too.
    #[serde(default)]
    all: bool,
    batch: Option<i64>,
}

fn no_upload(what: String) -> Failure {
    Failure::new(StatusCode::NOT_FOUND, "NoSuchUpload", what)
}

impl Shared {
    async fn list(&self, q: &ListQuery) -> Result<UploadList, Failure> {
        let mut queue = self.queue.status().await?;
        if let Some(b) = q.batch {
            if !queue.batches.iter().any(|x| x.id == b) {
                return Err(no_upload(format!("no batch {b}")));
            }
            queue.items.retain(|i| i.batch == Some(b));
            queue.batches.retain(|x| x.id == b);
        }
        if !q.all {
            queue.items.retain(|i| !i.state.finished());
        }
        let rate = self.rate();
        let eta_secs = (rate > 0 && queue.unpublished_bytes > 0).then(|| queue.unpublished_bytes.div_ceil(rate));
        Ok(UploadList { queue, rate, eta_secs })
    }

    /// The drive as the server names it, so that a pause by drive finds its uploads whichever
    /// name they were queued by. Offline, it is taken as given.
    pub(crate) async fn drive(&self, name: &str) -> Result<String, Failure> {
        match self.client.describe_drive(name).await {
            Ok(d) => Ok(d.alias),
            Err(e) if e.status().is_some() => Err(e.into()),
            Err(_) => Ok(name.to_owned()),
        }
    }
}

pub(crate) async fn list(State(s): State<Arc<Shared>>, Query(q): Query<ListQuery>) -> Answer<UploadList> {
    Ok(Json(s.list(&q).await?))
}

/// One document a line, each second, until the client goes or the daemon stops.
pub(crate) async fn watch(State(s): State<Arc<Shared>>, Query(q): Query<ListQuery>) -> Result<Response, Failure> {
    fn line(l: &UploadList) -> Bytes {
        let mut b = serde_json::to_vec(l).expect("JSON");
        b.push(b'\n');
        Bytes::from(b)
    }
    let first = line(&s.list(&q).await?);
    let closing = s.closing.subscribe();
    let stream = futures::stream::unfold((s, closing, Some(first)), move |(s, mut closing, first)| async move {
        if let Some(b) = first {
            return Some((Ok::<_, Infallible>(b), (s, closing, None)));
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = closing.wait_for(|c| *c) => return None,
        }
        let l = s.list(&q).await.ok()?;
        Some((Ok(line(&l)), (s, closing, None)))
    });
    Ok(Response::builder().header(header::CONTENT_TYPE, "application/x-ndjson").body(Body::from_stream(stream)).expect("a response"))
}

pub(crate) async fn enqueue(State(s): State<Arc<Shared>>, Json(b): Json<NewBatch>) -> Answer<Queued> {
    let bad = |m: String| Failure::new(StatusCode::BAD_REQUEST, "InvalidArgument", m);
    if b.files.is_empty() {
        return Err(bad("nothing to upload".into()));
    }
    let mut bytes = 0;
    for f in &b.files {
        let path = Path::new(&f.path);
        if !path.is_absolute() {
            return Err(bad(format!("{}: not an absolute path", f.path)));
        }
        let meta = std::fs::metadata(path).map_err(|e| Failure::new(StatusCode::BAD_REQUEST, "LocalFileError", format!("{}: {e}", f.path)))?;
        if meta.is_file() {
            bytes += meta.len();
        } else if !meta.is_dir() {
            return Err(bad(format!("{}: not a file or folder", f.path)));
        }
        if f.key.is_empty() || f.key.starts_with('/') {
            return Err(bad(format!("{:?}: not a key to upload to", f.key)));
        }
    }
    let drive = s.drive(&b.drive).await?;
    let files: Vec<Import> = b.files.iter().map(|f| Import { path: f.path.clone().into(), drive: drive.clone(), key: f.key.clone() }).collect();
    let items = files.len() as u64;
    let batch = s.queue.import(&b.label, files).await?;
    Ok(Json(Queued { batch, drive, items, bytes }))
}

#[derive(Clone, Copy)]
enum Control {
    Pause,
    Resume,
    Cancel,
}

async fn control(s: &Shared, scope: Scope, what: Control) -> Answer<Affected> {
    let scope = match scope {
        Scope::Drive(d) => Scope::Drive(s.drive(&d).await?),
        other => other,
    };
    let q = s.queue.status().await?;
    match &scope {
        Scope::Batch(b) if !q.batches.iter().any(|x| x.id == *b) => return Err(no_upload(format!("no batch {b}"))),
        Scope::Entry(e) if !q.items.iter().any(|i| i.id == *e) => return Err(no_upload(format!("no upload {e}"))),
        _ => {}
    }
    let applies = |i: &Item| match &scope {
        Scope::All => true,
        Scope::Drive(d) => i.drive == *d,
        Scope::Batch(b) => i.batch == Some(*b),
        Scope::Entry(e) => i.id == *e,
    };
    let items = q.items.iter().filter(|i| !i.state.finished() && applies(i)).count() as u64;
    let scope = match scope {
        Scope::All => voidfs_client::Scope::All,
        Scope::Drive(d) => voidfs_client::Scope::Drive(d),
        Scope::Batch(b) => voidfs_client::Scope::Batch(b),
        Scope::Entry(e) => voidfs_client::Scope::Entry(e),
    };
    match what {
        Control::Pause => s.queue.pause(scope).await?,
        Control::Resume => s.queue.resume(scope).await?,
        Control::Cancel => s.queue.cancel(scope).await?,
    }
    Ok(Json(Affected { items }))
}

pub(crate) async fn pause(State(s): State<Arc<Shared>>, Json(scope): Json<Scope>) -> Answer<Affected> {
    control(&s, scope, Control::Pause).await
}

pub(crate) async fn resume(State(s): State<Arc<Shared>>, Json(scope): Json<Scope>) -> Answer<Affected> {
    control(&s, scope, Control::Resume).await
}

pub(crate) async fn cancel(State(s): State<Arc<Shared>>, Json(scope): Json<Scope>) -> Answer<Affected> {
    control(&s, scope, Control::Cancel).await
}

pub(crate) async fn limit(State(s): State<Arc<Shared>>, Json(l): Json<Limit>) -> Answer<Limit> {
    let bytes_per_second = l.bytes_per_second.filter(|b| *b > 0);
    s.queue.set_bandwidth(bytes_per_second).await?;
    Ok(Json(Limit { bytes_per_second }))
}

pub(crate) async fn clear(State(s): State<Arc<Shared>>) -> Answer<Cleared> {
    let cleared = s.queue.status().await?.items.iter().filter(|i| i.state.finished()).count() as u64;
    s.queue.clear_finished().await?;
    Ok(Json(Cleared { cleared }))
}
