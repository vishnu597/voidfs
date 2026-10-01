// SPDX-License-Identifier: Apache-2.0
//! Prometheus metrics, which the admin listener serves ([`crate::admin`]).
//!
//! Each part of the server keeps its series in a registry of its own, gathered when the admin
//! listener is scraped. Every label takes a few fixed values, never a drive or a key, and every
//! series is resolved to its labels when it is created, so that recording is an atomic add or a
//! few, with no lookup on the request path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use prometheus::proto::MetricFamily;
use prometheus::{Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry};

/// Upper bounds of the latency histograms, in seconds.
const LATENCY: &[f64] = &[0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];
/// Upper bounds of the transactions-per-log-entry histogram: a batch holds at most 256.
const BATCH: &[f64] = &[1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0];
/// Upper bounds of the hold histogram, in seconds: a hold takes at most 2 ms.
const HOLD: &[f64] = &[0.0001, 0.00025, 0.0005, 0.001, 0.0015, 0.002];

fn counters(r: &Registry, name: &str, help: &str, labels: &[&str]) -> IntCounterVec {
    let v = IntCounterVec::new(Opts::new(name, help), labels).expect("a valid counter");
    r.register(Box::new(v.clone())).expect("a metric registered once");
    v
}

fn counter(r: &Registry, name: &str, help: &str) -> IntCounter {
    let c = IntCounter::new(name, help).expect("a valid counter");
    r.register(Box::new(c.clone())).expect("a metric registered once");
    c
}

fn gauges(r: &Registry, name: &str, help: &str, labels: &[&str]) -> IntGaugeVec {
    let v = IntGaugeVec::new(Opts::new(name, help), labels).expect("a valid gauge");
    r.register(Box::new(v.clone())).expect("a metric registered once");
    v
}

fn gauge(r: &Registry, name: &str, help: &str) -> IntGauge {
    let g = IntGauge::new(name, help).expect("a valid gauge");
    r.register(Box::new(g.clone())).expect("a metric registered once");
    g
}

fn histograms(r: &Registry, name: &str, help: &str, buckets: &[f64], labels: &[&str]) -> HistogramVec {
    let v = HistogramVec::new(HistogramOpts::new(name, help).buckets(buckets.to_vec()), labels).expect("a valid histogram");
    r.register(Box::new(v.clone())).expect("a metric registered once");
    v
}

fn histogram(r: &Registry, name: &str, help: &str, buckets: &[f64]) -> Histogram {
    let h = Histogram::with_opts(HistogramOpts::new(name, help).buckets(buckets.to_vec())).expect("a valid histogram");
    r.register(Box::new(h.clone())).expect("a metric registered once");
    h
}

// ---------------------------------------------------------------------------------------------
// S3 requests

/// What a request to the S3 port does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum S3Op {
    /// ListBuckets.
    ListDrives,
    /// Creating, deleting, undeleting or heading a drive, and its settings.
    Drive,
    List,
    Get,
    Head,
    Put,
    Copy,
    /// DeleteObject and DeleteObjects.
    Delete,
    /// Creating, uploading to, listing, completing and aborting multipart uploads.
    Multipart,
    /// The `x-voidfs-*` extensions (protocol §4–§5).
    Extension,
    /// Anything else, such as what is not supported.
    Other,
}

impl S3Op {
    const ALL: [S3Op; 11] = [
        S3Op::ListDrives, S3Op::Drive, S3Op::List, S3Op::Get, S3Op::Head, S3Op::Put, S3Op::Copy, S3Op::Delete, S3Op::Multipart,
        S3Op::Extension, S3Op::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            S3Op::ListDrives => "list_drives",
            S3Op::Drive => "drive",
            S3Op::List => "list",
            S3Op::Get => "get",
            S3Op::Head => "head",
            S3Op::Put => "put",
            S3Op::Copy => "copy",
            S3Op::Delete => "delete",
            S3Op::Multipart => "multipart",
            S3Op::Extension => "extension",
            S3Op::Other => "other",
        }
    }
}

const STATUS_CLASSES: [&str; 4] = ["2xx", "3xx", "4xx", "5xx"];

pub struct S3Metrics {
    registry: Registry,
    /// For each [`S3Op`]: its latency, and its requests by [`STATUS_CLASSES`].
    ops: Vec<(Histogram, [IntCounter; 4])>,
}

impl S3Metrics {
    pub fn new() -> S3Metrics {
        let r = Registry::new();
        let requests = counters(&r, "voidfs_s3_requests_total", "Requests to the S3 port, by operation and status class.", &["op", "status"]);
        let latency = histograms(
            &r,
            "voidfs_s3_request_duration_seconds",
            "Time from a request's arrival to its response's headers, by operation. A GET's body streams after that.",
            LATENCY,
            &["op"],
        );
        let ops = S3Op::ALL.iter().map(|op| (latency.with_label_values(&[op.name()]), STATUS_CLASSES.map(|s| requests.with_label_values(&[op.name(), s])))).collect();
        S3Metrics { registry: r, ops }
    }

    pub fn record(&self, op: S3Op, status: u16, elapsed: Duration) {
        let (latency, by_status) = &self.ops[op as usize];
        latency.observe(elapsed.as_secs_f64());
        by_status[usize::from(status / 100).clamp(2, 5) - 2].inc();
    }

    pub fn gather(&self) -> Vec<MetricFamily> {
        self.registry.gather()
    }
}

// ---------------------------------------------------------------------------------------------
// Bucket requests

/// A request to the store: one request to a bucket, except that a listing or a deletion by
/// prefix may take several.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketOp {
    Get,
    Head,
    Put,
    /// A create-if-absent write (format §7.2).
    PutNew,
    Delete,
    DeletePrefix,
    List,
}

impl BucketOp {
    const ALL: [BucketOp; 7] = [BucketOp::Get, BucketOp::Head, BucketOp::Put, BucketOp::PutNew, BucketOp::Delete, BucketOp::DeletePrefix, BucketOp::List];

    fn name(self) -> &'static str {
        match self {
            BucketOp::Get => "get",
            BucketOp::Head => "head",
            BucketOp::Put => "put",
            BucketOp::PutNew => "put_new",
            BucketOp::Delete => "delete",
            BucketOp::DeletePrefix => "delete_prefix",
            BucketOp::List => "list",
        }
    }
}

struct BucketSeries {
    requests: IntCounter,
    errors: IntCounter,
    latency: Histogram,
}

pub struct BucketMetrics {
    registry: Registry,
    ops: Vec<BucketSeries>,
    base: Instant,
    /// When the last request that succeeded, and the last that failed, ended: nanoseconds after
    /// `base`, plus one, or 0 if none has yet.
    last_ok: AtomicU64,
    last_err: AtomicU64,
}

impl Default for BucketMetrics {
    fn default() -> Self {
        let r = Registry::new();
        let help = "Requests to the bucket (or to the fs: or memory store), by operation.";
        let requests = counters(&r, "voidfs_bucket_requests_total", help, &["op"]);
        let errors = counters(&r, "voidfs_bucket_request_errors_total", "Requests to the bucket that failed, by operation. A missing object is not a failure.", &["op"]);
        let latency = histograms(&r, "voidfs_bucket_request_duration_seconds", "Latency of requests to the bucket, by operation.", LATENCY, &["op"]);
        let ops = BucketOp::ALL
            .iter()
            .map(|op| BucketSeries { requests: requests.with_label_values(&[op.name()]), errors: errors.with_label_values(&[op.name()]), latency: latency.with_label_values(&[op.name()]) })
            .collect();
        BucketMetrics { registry: r, ops, base: Instant::now(), last_ok: AtomicU64::new(0), last_err: AtomicU64::new(0) }
    }
}

impl BucketMetrics {
    fn at(&self, t: Instant) -> u64 {
        u64::try_from(t.saturating_duration_since(self.base).as_nanos()).unwrap_or(u64::MAX - 1) + 1
    }

    /// Records a request that started at `started` and has just ended.
    pub fn record(&self, op: BucketOp, started: Instant, ok: bool) {
        let now = Instant::now();
        let s = &self.ops[op as usize];
        s.requests.inc();
        s.latency.observe(now.saturating_duration_since(started).as_secs_f64());
        if ok {
            self.last_ok.fetch_max(self.at(now), Ordering::Relaxed);
        } else {
            s.errors.inc();
            self.last_err.fetch_max(self.at(now), Ordering::Relaxed);
        }
    }

    /// Whether the last request to end succeeded, and ended less than `within` ago.
    pub fn answered_within(&self, within: Duration) -> bool {
        let (ok, err) = (self.last_ok.load(Ordering::Relaxed), self.last_err.load(Ordering::Relaxed));
        ok != 0 && ok > err && self.at(Instant::now()).saturating_sub(ok) < u64::try_from(within.as_nanos()).unwrap_or(u64::MAX)
    }

    pub fn gather(&self) -> Vec<MetricFamily> {
        self.registry.gather()
    }

    #[cfg(test)]
    pub fn requests(&self, op: BucketOp) -> u64 {
        self.ops[op as usize].requests.get()
    }
}

// ---------------------------------------------------------------------------------------------
// The pool: caches, commits, garbage collection, drives

/// A shard or page cache's series. Hits, misses and evictions are counted as they happen; the
/// gauges are set when the metrics are gathered. Every read is a hit, a miss or coalesced, and
/// every miss is one request to the bucket.
pub struct CacheMetrics {
    pub hits: IntCounter,
    pub misses: IntCounter,
    /// Reads that missed while another read was fetching the same object, and waited for it.
    pub coalesced: IntCounter,
    pub evictions: IntCounter,
    pub bytes: IntGauge,
    pub entries: IntGauge,
    pub capacity: IntGauge,
}

/// How a step of garbage collection ended: [`crate::gc::Outcome`], or a failure.
pub const GC_OUTCOMES: [&str; 9] = ["nothing", "nothing_to_collect", "proposed", "waiting", "deleted", "abandoned", "busy", "dry_run", "failed"];
/// The phases of a run in `gc/pending.json`, or none.
pub const GC_PHASES: [&str; 4] = ["none", "marking", "waiting", "deleting"];

pub struct PoolMetrics {
    registry: Registry,
    pub shards: CacheMetrics,
    pub pages: CacheMetrics,
    /// Times both caches were emptied (SIGUSR1).
    pub cache_drops: IntCounter,
    /// Transactions in each log entry written.
    pub batch: Histogram,
    /// How long each log entry took to write.
    pub log_write: Histogram,
    /// How long log entries were held for the requests the entry before answered.
    pub hold: Histogram,
    pub commits_written: IntCounter,
    /// Log entries another server wrote first, so that the batch was planned again.
    pub commits_lost: IntCounter,
    pub commits_failed: IntCounter,
    pub checkpoints_written: IntCounter,
    pub checkpoints_failed: IntCounter,
    /// How long each checkpoint took to write, in the background.
    pub checkpoint_write: Histogram,
    /// Shards checkpoints stored for the data extents of their rows (format §8.2).
    pub checkpoint_spilled: IntCounter,
    /// How long putting what a checkpoint spilled into the drive's state held its commit lock.
    pub checkpoint_swap: Histogram,
    pub drives_live: IntGauge,
    pub drives_deleted: IntGauge,
    /// One per [`GC_PHASES`], 1 for the run's phase as last read.
    pub gc_phase: [IntGauge; 4],
    gc_steps: [IntCounter; 9],
    /// One per [`GC_OUTCOMES`], 1 for the last step's.
    gc_last: [IntGauge; 9],
    gc_last_time: IntGauge,
    gc_deleted_objects: IntCounter,
    gc_deleted_bytes: IntCounter,
}

impl PoolMetrics {
    pub fn new(cache_bytes: u64, page_cache_bytes: u64) -> PoolMetrics {
        let r = Registry::new();
        let hits = counters(&r, "voidfs_cache_hits_total", "Reads of a shard or page found in memory.", &["cache"]);
        let misses = counters(&r, "voidfs_cache_misses_total", "Reads of a shard or page that went to the bucket.", &["cache"]);
        let coalesced = counters(
            &r,
            "voidfs_cache_coalesced_total",
            "Reads of a shard or page that waited for a fetch another read had started, rather than go to the bucket again: neither hits nor misses.",
            &["cache"],
        );
        let evictions = counters(&r, "voidfs_cache_evictions_total", "Entries evicted to make room.", &["cache"]);
        let bytes = gauges(&r, "voidfs_cache_bytes", "Bytes held by the cache.", &["cache"]);
        let entries = gauges(&r, "voidfs_cache_entries", "Entries held by the cache.", &["cache"]);
        let capacity = gauges(&r, "voidfs_cache_capacity_bytes", "The most the cache holds (--cache-mib, and an eighth of it for pages).", &["cache"]);
        let cache = |name: &str, max: u64| {
            let c = CacheMetrics {
                hits: hits.with_label_values(&[name]),
                misses: misses.with_label_values(&[name]),
                coalesced: coalesced.with_label_values(&[name]),
                evictions: evictions.with_label_values(&[name]),
                bytes: bytes.with_label_values(&[name]),
                entries: entries.with_label_values(&[name]),
                capacity: capacity.with_label_values(&[name]),
            };
            c.capacity.set(i64::try_from(max).unwrap_or(i64::MAX));
            c
        };
        let (shards, pages) = (cache("shard", cache_bytes), cache("page", page_cache_bytes));
        let batch = histogram(&r, "voidfs_commit_transactions", "Transactions in each log entry written (group commit).", BATCH);
        let log_write = histogram(&r, "voidfs_commit_log_write_seconds", "Time to write each log entry, whether or not it was written.", LATENCY);
        let hold = histogram(&r, "voidfs_commit_hold_seconds", "Time a log entry was held after the one before it, for the requests that one answered to come back.", HOLD);
        let commits = counters(&r, "voidfs_commits_total", "Log entries: written, lost to another server's entry (then planned again), or failed.", &["outcome"]);
        let checkpoints = counters(&r, "voidfs_checkpoints_total", "Checkpoints written, and attempts that failed.", &["outcome"]);
        let checkpoint_write = histogram(&r, "voidfs_checkpoint_write_seconds", "Time to write each checkpoint, in the background, whether or not it was written.", LATENCY);
        let checkpoint_spilled = counter(&r, "voidfs_checkpoint_spilled_shards_total", "Shards checkpoints stored for content held in data extents, which checkpoints never carry.");
        let checkpoint_swap = histogram(&r, "voidfs_checkpoint_swap_seconds", "Time each checkpoint held its drive's commit lock to put the shards it stored in place of data extents.", LATENCY);
        let drives = gauges(&r, "voidfs_drives", "Drives this server has open: live, and soft-deleted.", &["state"]);
        let phase = gauges(&r, "voidfs_gc_phase", "The phase of the garbage-collection run in gc/pending.json, as last read (every minute): 1 for the current one.", &["phase"]);
        let steps = counters(&r, "voidfs_gc_steps_total", "Steps of --gc-interval garbage collection, by outcome.", &["outcome"]);
        let last = gauges(&r, "voidfs_gc_last_step", "The outcome of the last step of --gc-interval garbage collection: 1 for it.", &["outcome"]);
        PoolMetrics {
            shards,
            pages,
            cache_drops: counter(&r, "voidfs_cache_drops_total", "Times the shard and page caches were emptied, on SIGUSR1."),
            batch,
            log_write,
            hold,
            commits_written: commits.with_label_values(&["written"]),
            commits_lost: commits.with_label_values(&["lost_race"]),
            commits_failed: commits.with_label_values(&["failed"]),
            checkpoints_written: checkpoints.with_label_values(&["written"]),
            checkpoints_failed: checkpoints.with_label_values(&["failed"]),
            checkpoint_write,
            checkpoint_spilled,
            checkpoint_swap,
            drives_live: drives.with_label_values(&["live"]),
            drives_deleted: drives.with_label_values(&["deleted"]),
            gc_phase: GC_PHASES.map(|p| phase.with_label_values(&[p])),
            gc_steps: GC_OUTCOMES.map(|o| steps.with_label_values(&[o])),
            gc_last: GC_OUTCOMES.map(|o| last.with_label_values(&[o])),
            gc_last_time: gauge(&r, "voidfs_gc_last_step_timestamp_seconds", "When the last step of --gc-interval garbage collection ended, in Unix time."),
            gc_deleted_objects: counter(&r, "voidfs_gc_deleted_objects_total", "Shards and pages garbage collection deleted."),
            gc_deleted_bytes: counter(&r, "voidfs_gc_deleted_bytes_total", "Bytes of the shards and pages garbage collection deleted."),
            registry: r,
        }
    }

    /// Records a step of garbage collection that ended with `outcome`, one of [`GC_OUTCOMES`],
    /// having deleted `deleted` objects of `bytes` bytes.
    pub fn gc_step(&self, outcome: &str, deleted: usize, bytes: u64) {
        let i = GC_OUTCOMES.iter().position(|o| *o == outcome).unwrap_or(GC_OUTCOMES.len() - 1);
        self.gc_steps[i].inc();
        for (j, g) in self.gc_last.iter().enumerate() {
            g.set(i64::from(j == i));
        }
        self.gc_last_time.set(chrono::Utc::now().timestamp());
        self.gc_deleted_objects.inc_by(deleted as u64);
        self.gc_deleted_bytes.inc_by(bytes);
    }

    pub fn gather(&self) -> Vec<MetricFamily> {
        self.registry.gather()
    }
}

// ---------------------------------------------------------------------------------------------
// The process

pub struct ProcessMetrics {
    registry: Registry,
    started: Instant,
    uptime: IntGauge,
}

impl ProcessMetrics {
    pub fn new() -> ProcessMetrics {
        let r = Registry::new();
        let info = gauges(&r, "voidfs_build_info", "voidfs-server's version: always 1.", &["version"]);
        info.with_label_values(&[env!("CARGO_PKG_VERSION")]).set(1);
        let uptime = gauge(&r, "voidfs_uptime_seconds", "Seconds since the server started.");
        ProcessMetrics { registry: r, started: Instant::now(), uptime }
    }

    pub fn gather(&self) -> Vec<MetricFamily> {
        self.uptime.set(i64::try_from(self.started.elapsed().as_secs()).unwrap_or(i64::MAX));
        self.registry.gather()
    }
}

/// The value of the sample `series` (its name and labels as the text format writes them) in
/// `text`.
#[cfg(test)]
pub fn sample(text: &str, series: &str) -> Option<f64> {
    text.lines().find_map(|l| l.strip_prefix(series)?.strip_prefix(' ')?.parse().ok())
}

/// The text format of the given families, sorted by name.
pub fn encode(mut families: Vec<MetricFamily>) -> String {
    families.sort_by(|a, b| a.name().cmp(b.name()));
    let mut out = String::new();
    prometheus::TextEncoder::new().encode_utf8(&families, &mut out).expect("metrics encode");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_requests_are_counted_by_operation_and_status_class() {
        let m = S3Metrics::new();
        m.record(S3Op::Get, 200, Duration::from_micros(300));
        m.record(S3Op::Get, 206, Duration::from_millis(3));
        m.record(S3Op::Get, 304, Duration::from_millis(3));
        m.record(S3Op::Put, 403, Duration::from_millis(1));
        m.record(S3Op::Other, 501, Duration::from_millis(1));
        let text = encode(m.gather());
        assert_eq!(sample(&text, r#"voidfs_s3_requests_total{op="get",status="2xx"}"#), Some(2.0));
        assert_eq!(sample(&text, r#"voidfs_s3_requests_total{op="get",status="3xx"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_s3_requests_total{op="put",status="4xx"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_s3_requests_total{op="other",status="5xx"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_s3_requests_total{op="copy",status="2xx"}"#), Some(0.0), "every series is there from the start");
        assert_eq!(sample(&text, r#"voidfs_s3_request_duration_seconds_bucket{op="get",le="0.0005"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_s3_request_duration_seconds_bucket{op="get",le="0.005"}"#), Some(3.0));
        assert_eq!(sample(&text, r#"voidfs_s3_request_duration_seconds_count{op="get"}"#), Some(3.0));
        assert_eq!(text.lines().filter(|l| l.starts_with("voidfs_s3_requests_total{")).count(), 11 * 4);
    }

    #[test]
    fn bucket_answers_decide_readiness() {
        let m = BucketMetrics::default();
        assert!(!m.answered_within(Duration::from_secs(30)), "nothing has answered yet");
        m.record(BucketOp::Get, Instant::now(), true);
        assert!(m.answered_within(Duration::from_secs(30)));
        assert!(!m.answered_within(Duration::ZERO));
        m.record(BucketOp::Put, Instant::now(), false);
        assert!(!m.answered_within(Duration::from_secs(30)), "the last request failed");
        m.record(BucketOp::Head, Instant::now(), true);
        assert!(m.answered_within(Duration::from_secs(30)));
        let text = encode(m.gather());
        assert_eq!(sample(&text, r#"voidfs_bucket_requests_total{op="put"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_bucket_request_errors_total{op="put"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_bucket_request_errors_total{op="get"}"#), Some(0.0));
        assert_eq!(sample(&text, r#"voidfs_bucket_request_duration_seconds_count{op="head"}"#), Some(1.0));
    }

    #[test]
    fn gc_steps_record_the_last_outcome() {
        let m = PoolMetrics::new(1 << 20, 1 << 17);
        m.gc_step("proposed", 0, 0);
        m.gc_step("deleted", 3, 300);
        m.gc_step("no such outcome", 0, 0);
        let text = encode(m.gather());
        assert_eq!(sample(&text, r#"voidfs_gc_steps_total{outcome="deleted"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_gc_steps_total{outcome="failed"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_gc_last_step{outcome="failed"}"#), Some(1.0));
        assert_eq!(sample(&text, r#"voidfs_gc_last_step{outcome="deleted"}"#), Some(0.0));
        assert_eq!(sample(&text, "voidfs_gc_deleted_objects_total"), Some(3.0));
        assert_eq!(sample(&text, "voidfs_gc_deleted_bytes_total"), Some(300.0));
        assert_eq!(sample(&text, r#"voidfs_cache_capacity_bytes{cache="shard"}"#), Some(1048576.0));
    }

    #[test]
    fn the_process_reports_its_version_and_uptime() {
        let text = encode(ProcessMetrics::new().gather());
        assert_eq!(sample(&text, &format!(r#"voidfs_build_info{{version="{}"}}"#, env!("CARGO_PKG_VERSION"))), Some(1.0));
        assert_eq!(sample(&text, "voidfs_uptime_seconds"), Some(0.0));
        assert!(text.contains("# TYPE voidfs_uptime_seconds gauge"));
    }
}
