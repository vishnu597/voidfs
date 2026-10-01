// SPDX-License-Identifier: Apache-2.0
//! The admin listener (`--admin-listen`): liveness, readiness and metrics for operators, on a
//! port of its own, since on the S3 port every path names a drive.
//!
//! - `GET /healthz`: 200 while the process is up.
//! - `GET /readyz`: 200 once the pool is open and the S3 port listening, while the bucket
//!   answers; 503 otherwise, with the reason.
//! - `GET /metrics`: Prometheus's text format ([`crate::metrics`]).
//!
//! Nothing here is authenticated, as is usual for these endpoints, so the listener belongs on
//! loopback or a private network.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http::{StatusCode, header};

use crate::metrics::{ProcessMetrics, encode};
use crate::probe;
use crate::s3::App;

/// A request to the bucket that succeeded this recently, with none failing since, shows that
/// the bucket answers.
const FRESH: Duration = Duration::from_secs(30);
/// Otherwise `/readyz` sends a HEAD of `voidfs.json`, and answers with its result for this long.
const RECHECK_AFTER: Duration = Duration::from_secs(5);
/// How long that HEAD may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Admin {
    /// The server, once its pool is open and the S3 port listening.
    app: OnceLock<Arc<App>>,
    stopping: AtomicBool,
    process: ProcessMetrics,
    /// When the bucket was last checked, and what was found. Held while a check runs, so that
    /// probes arriving meanwhile wait for it rather than send their own.
    checked: tokio::sync::Mutex<Option<(Instant, Result<(), String>)>>,
    fresh: Duration,
    recheck_after: Duration,
}

impl Admin {
    pub fn new() -> Arc<Admin> {
        Admin::with_windows(FRESH, RECHECK_AFTER)
    }

    fn with_windows(fresh: Duration, recheck_after: Duration) -> Arc<Admin> {
        Arc::new(Admin { app: OnceLock::new(), stopping: AtomicBool::new(false), process: ProcessMetrics::new(), checked: tokio::sync::Mutex::new(None), fresh, recheck_after })
    }

    /// The pool is open and the S3 port listening.
    pub fn serving(&self, app: Arc<App>) {
        let _ = self.app.set(app);
    }

    /// The server is shutting down: it is no longer ready.
    pub fn stopping(&self) {
        self.stopping.store(true, Ordering::Relaxed);
    }

    /// Whether the server is ready to serve, and why not. Recent requests to the bucket answer
    /// for it if the last of them succeeded; otherwise a HEAD of `voidfs.json` does, at most one
    /// every [`RECHECK_AFTER`].
    async fn ready(&self) -> Result<(), String> {
        if self.stopping.load(Ordering::Relaxed) {
            return Err("shutting down".into());
        }
        let app = self.app.get().ok_or("the pool is not open yet")?;
        let store = &app.pool.store;
        if store.metrics.answered_within(self.fresh) {
            return Ok(());
        }
        let mut checked = self.checked.lock().await;
        // A check may have finished while this one waited.
        if store.metrics.answered_within(self.fresh) {
            return Ok(());
        }
        if let Some((at, found)) = &*checked
            && at.elapsed() < self.recheck_after
        {
            return found.clone();
        }
        let found = match tokio::time::timeout(CHECK_TIMEOUT, store.exists(probe::DESCRIPTOR)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(format!("the bucket did not answer: {e:#}")),
            Err(_) => Err(format!("the bucket did not answer within {} s", CHECK_TIMEOUT.as_secs())),
        };
        *checked = Some((Instant::now(), found.clone()));
        found
    }
}

pub fn router(admin: Arc<Admin>) -> axum::Router {
    axum::Router::new().route("/healthz", get(healthz)).route("/readyz", get(readyz)).route("/metrics", get(metrics)).with_state(admin)
}

async fn healthz() -> &'static str {
    "ok\n"
}

async fn readyz(State(admin): State<Arc<Admin>>) -> Response {
    match admin.ready().await {
        Ok(()) => "ready\n".into_response(),
        Err(why) => (StatusCode::SERVICE_UNAVAILABLE, format!("not ready: {why}\n")).into_response(),
    }
}

async fn metrics(State(admin): State<Arc<Admin>>) -> Response {
    let mut families = admin.process.gather();
    if let Some(app) = admin.app.get() {
        families.extend(app.metrics.gather());
        families.extend(app.pool.store.metrics.gather());
        families.extend(app.pool.gather_metrics());
    }
    ([(header::CONTENT_TYPE, prometheus::TEXT_FORMAT)], encode(families)).into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use futures::FutureExt;

    use super::*;
    use crate::clock::Clock;
    use crate::metrics::{S3Metrics, S3Op, sample};
    use crate::pool::Pool;
    use crate::s3::Domains;
    use crate::sigv4::Keys;
    use crate::store::{Fault, MemOp, MemStore, Store};

    async fn app(mem: &Arc<MemStore>) -> Arc<App> {
        let pool = Pool::open(Store::mem(mem.clone()), 1 << 20).await.unwrap();
        Arc::new(App { pool, keys: Keys::default(), domains: Domains::new(Vec::new()), metrics: S3Metrics::new(), uploads: Default::default(), read_ahead: Default::default() })
    }

    async fn get(admin: &Arc<Admin>, path: &str) -> (StatusCode, String) {
        let resp = match path {
            "/healthz" => healthz().await.into_response(),
            "/readyz" => readyz(State(admin.clone())).await,
            "/metrics" => metrics(State(admin.clone())).await,
            _ => unreachable!(),
        };
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    /// Makes every request to `mem` fail while `failing` is set, and counts the HEADs.
    fn flaky(mem: &MemStore) -> (Arc<AtomicBool>, Arc<AtomicUsize>) {
        let (failing, heads) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let (f, h) = (failing.clone(), heads.clone());
        mem.set_hook(Some(Arc::new(move |op, _| {
            if op == MemOp::Head {
                h.fetch_add(1, Ordering::SeqCst);
            }
            futures::future::ready(if f.load(Ordering::SeqCst) { Fault::Fail } else { Fault::None }).boxed()
        })));
        (failing, heads)
    }

    #[tokio::test]
    async fn readiness_follows_the_pool_and_the_bucket() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let admin = Admin::with_windows(Duration::from_secs(1), Duration::from_millis(200));
        assert_eq!(get(&admin, "/healthz").await, (StatusCode::OK, "ok\n".into()));
        assert_eq!(get(&admin, "/readyz").await, (StatusCode::SERVICE_UNAVAILABLE, "not ready: the pool is not open yet\n".into()));
        let app = app(&mem).await;
        admin.serving(app.clone());
        let (failing, heads) = flaky(&mem);
        // Opening the pool just read the bucket: that answers, with no request of its own.
        assert_eq!(get(&admin, "/readyz").await.0, StatusCode::OK);
        assert_eq!(heads.load(Ordering::SeqCst), 0);
        // The last request failed: a HEAD checks, and its failure is reused for a while.
        failing.store(true, Ordering::SeqCst);
        assert!(app.pool.store.get("anything").await.is_err());
        let (status, body) = get(&admin, "/readyz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.starts_with("not ready: the bucket did not answer: injected failure: Head voidfs.json"), "{body}");
        assert_eq!(get(&admin, "/readyz").await.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(heads.load(Ordering::SeqCst), 1, "one check, reused");
        // Once the check is old enough, the next probe checks again.
        failing.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(get(&admin, "/readyz").await.0, StatusCode::OK);
        assert_eq!(heads.load(Ordering::SeqCst), 2);
        // That check succeeded, so probes rely on it until it is no longer recent.
        assert_eq!(get(&admin, "/readyz").await.0, StatusCode::OK);
        assert_eq!(heads.load(Ordering::SeqCst), 2);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(get(&admin, "/readyz").await.0, StatusCode::OK);
        assert_eq!(heads.load(Ordering::SeqCst), 3, "an idle server checks");
        admin.stopping();
        assert_eq!(get(&admin, "/readyz").await, (StatusCode::SERVICE_UNAVAILABLE, "not ready: shutting down\n".into()));
    }

    #[tokio::test]
    async fn concurrent_probes_share_one_check() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let admin = Admin::with_windows(Duration::ZERO, Duration::from_secs(60));
        admin.serving(app(&mem).await);
        let (_, heads) = flaky(&mem);
        let probes: Vec<_> = (0..16).map(|_| get(&admin, "/readyz")).collect();
        assert!(futures::future::join_all(probes).await.iter().all(|(s, _)| *s == StatusCode::OK));
        assert_eq!(heads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn metrics_cover_the_process_and_once_open_the_server() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let admin = Admin::new();
        let (status, text) = get(&admin, "/metrics").await;
        assert_eq!(status, StatusCode::OK);
        assert!(sample(&text, "voidfs_uptime_seconds").is_some());
        assert!(!text.contains("voidfs_s3_requests_total"), "nothing of the server's before it serves");
        let app = app(&mem).await;
        app.metrics.record(S3Op::Get, 200, Duration::from_millis(1));
        admin.serving(app);
        let resp = metrics(State(admin.clone())).await;
        assert_eq!(resp.headers()[header::CONTENT_TYPE], prometheus::TEXT_FORMAT);
        let (_, text) = get(&admin, "/metrics").await;
        for series in [
            r#"voidfs_s3_requests_total{op="get",status="2xx"}"#,
            r#"voidfs_bucket_requests_total{op="get"}"#,
            r#"voidfs_cache_hits_total{cache="shard"}"#,
            r#"voidfs_gc_phase{phase="none"}"#,
            r#"voidfs_drives{state="live"}"#,
            "voidfs_commit_transactions_count",
            "voidfs_uptime_seconds",
        ] {
            assert!(sample(&text, series).is_some(), "{series} is missing");
        }
        assert_eq!(sample(&text, r#"voidfs_s3_requests_total{op="get",status="2xx"}"#), Some(1.0));
        assert!(text.lines().all(|l| l.is_empty() || l.starts_with("# ") || l.starts_with("voidfs_")), "every series is voidfs_");
    }
}
