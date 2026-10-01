// SPDX-License-Identifier: Apache-2.0
//! Runs one scenario against every target: sets up its objects, warms up, measures the rounds,
//! checks the objects, and removes them.
//!
//! Each operation is timed from the first request to the last byte of the last response. Test
//! data is generated before the clock starts. Where workers need objects of their own (edits,
//! overwrites, renames, folder moves), worker `w` only ever touches object `w`, so no two
//! operations race on one key.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use bytes::Bytes;
use futures::StreamExt;
use sha2::{Digest, Sha256};

use crate::data::{self, Change, EDIT};
use crate::report::{RoundStats, ScenarioResult, TargetResult};
use crate::scenarios::{KIB, Kind, Scenario};
use crate::target::{Flavor, Place, Target};

/// Objects uploaded at once while setting up.
const SETUP_PARALLELISM: usize = 8;
/// Size of each object in the listing scenario.
const LIST_OBJECT: u64 = 4 * KIB;

pub struct Settings {
    pub rounds: usize,
    /// Every scenario's concurrency, instead of its own (for diagnosis, not for comparison).
    pub concurrency: Option<usize>,
    pub warmups: usize,
    pub ops_scale: f64,
    pub verify: bool,
    pub keep: bool,
    pub samples: bool,
    /// Mixed into every seed, so no two runs write the same bytes.
    pub nonce: u64,
    /// Short lowercase tag naming this run's drives and prefixes.
    pub tag: String,
    pub metrics: Option<MetricsSource>,
    /// Measure every operation with voidfs's caches empty (`--cold`).
    pub cold: Option<Cold>,
}

/// voidfs-server's metrics, read before and after each scenario's measured rounds.
pub struct MetricsSource {
    url: String,
    http: reqwest::Client,
}

impl MetricsSource {
    pub fn new(url: String) -> MetricsSource {
        MetricsSource { url, http: reqwest::Client::new() }
    }

    async fn scrape(&self) -> anyhow::Result<String> {
        Ok(self.http.get(&self.url).send().await?.error_for_status()?.text().await?)
    }

    /// voidfs's requests to the bucket so far, by operation, and the seconds they took.
    async fn bucket_requests(&self) -> anyhow::Result<(BTreeMap<String, u64>, BTreeMap<String, f64>)> {
        let text = self.scrape().await?;
        Ok((by_op(&text, "voidfs_bucket_requests_total").into_iter().map(|(op, n)| (op, n as u64)).collect(), by_op(&text, "voidfs_bucket_request_duration_seconds_sum")))
    }
}

/// A series of a scrape, by its `op` label.
fn by_op(text: &str, series: &str) -> BTreeMap<String, f64> {
    let prefix = format!("{series}{{op=\"");
    text.lines()
        .filter_map(|l| {
            let (op, value) = l.strip_prefix(prefix.as_str())?.split_once("\"} ")?;
            Some((op.to_owned(), value.parse::<f64>().ok()?))
        })
        .collect()
}

/// A series of a scrape that has no labels.
fn sample(text: &str, series: &str) -> Option<f64> {
    text.lines().find_map(|l| l.strip_prefix(series)?.strip_prefix(' ')?.parse().ok())
}

/// How long a drop of voidfs's caches may take to show in its metrics.
const DROP_WAIT: Duration = Duration::from_secs(5);

/// Drops voidfs-server's shard and page caches: SIGUSR1 to its process, which must be on this
/// machine, confirmed by its metrics.
pub struct Cold {
    pid: u32,
    metrics: MetricsSource,
}

impl Cold {
    pub fn new(pid: u32, metrics: MetricsSource) -> Cold {
        Cold { pid, metrics }
    }

    async fn drops(&self) -> anyhow::Result<f64> {
        sample(&self.metrics.scrape().await?, "voidfs_cache_drops_total").context("voidfs-server's metrics have no voidfs_cache_drops_total: it cannot drop its caches")
    }

    /// Drops the caches, and waits until the server's metrics count the drop.
    pub async fn drop_caches(&self) -> anyhow::Result<()> {
        let before = self.drops().await?;
        let pid = self.pid.to_string();
        let status = tokio::task::spawn_blocking(move || std::process::Command::new("kill").args(["-USR1", &pid]).status()).await?.context("running kill")?;
        ensure!(status.success(), "kill -USR1 {} failed: {status}", self.pid);
        let deadline = Instant::now() + DROP_WAIT;
        loop {
            if self.drops().await? > before {
                return Ok(());
            }
            ensure!(Instant::now() < deadline, "voidfs-server (pid {}) did not drop its caches within {} s", self.pid, DROP_WAIT.as_secs());
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
}

/// What every worker of a scenario reads.
#[derive(Default)]
struct Shared {
    key: String,
    keys: Vec<String>,
}

/// One worker's own object, for the scenarios that need one.
#[derive(Clone, Default)]
struct Slot {
    key: String,
    /// The other name a rename or move alternates with.
    alt: String,
    /// Whether the object is now at `alt`.
    flipped: bool,
    size: u64,
    /// The object's expected content, kept for worker 0 when verifying edits.
    model: Option<Vec<u8>>,
}

impl Slot {
    fn current(&self) -> &str {
        if self.flipped { &self.alt } else { &self.key }
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Warmup,
    Round(usize),
}

impl Phase {
    fn tag(self) -> String {
        match self {
            Phase::Warmup => "w".into(),
            Phase::Round(r) => format!("r{r}"),
        }
    }

    fn code(self) -> u64 {
        match self {
            Phase::Warmup => u64::MAX,
            Phase::Round(r) => r as u64,
        }
    }
}

struct State {
    place: Place,
    shared: Arc<Shared>,
    slots: Vec<Slot>,
}

pub async fn scenario(targets: &[Arc<Target>], index: usize, s: &'static Scenario, set: &Settings, log: &dyn Fn(&str)) -> ScenarioResult {
    let mut ops = ((s.ops as f64 * set.ops_scale).ceil() as usize).max(1);
    if let Kind::FanoutPut { count, .. } = s.kind {
        ops = ops.min(count);
    }
    let mut results: Vec<TargetResult> = targets.iter().map(|_| TargetResult::default()).collect();
    let mut states: Vec<Option<State>> = Vec::new();
    let before: Vec<_> = targets.iter().map(|t| t.counters.snapshot()).collect();

    for (t, res) in targets.iter().zip(results.iter_mut()) {
        let place = t.place(&set.tag, index, s.id);
        let started = Instant::now();
        match setup(t, &place, s, set, index).await {
            Ok((shared, slots)) => {
                log(&format!("  {:6} set up in {:.1} s", t.name(), started.elapsed().as_secs_f64()));
                states.push(Some(State { place, shared: Arc::new(shared), slots }));
            }
            Err(e) => {
                log(&format!("  {:6} setup failed: {e:#}", t.name()));
                res.setup_error = Some(format!("{e:#}"));
                if !set.keep {
                    let _ = t.destroy(&place).await;
                }
                states.push(None);
            }
        }
    }

    for (i, t) in targets.iter().enumerate() {
        if let Some(st) = states[i].as_mut() {
            let (_, errors, _, _) = measure(t, st, s, index, Phase::Warmup, set.warmups, set, Start::Steady).await;
            results[i].warmup_errors = errors.len();
            if let Some(e) = errors.first() {
                log(&format!("  {:6} warm-up error: {e}", t.name()));
            }
        }
    }

    let voidfs = targets.iter().position(|t| t.flavor == Flavor::Voidfs);
    let scraped = match (&set.metrics, voidfs) {
        (Some(m), Some(_)) => m.bucket_requests().await.map_err(|e| log(&format!("  reading voidfs's metrics failed: {e:#}"))).ok(),
        _ => None,
    };
    for round in 0..set.rounds {
        // Alternate which target goes first, so drift over time does not favour either.
        let mut order: Vec<usize> = (0..targets.len()).collect();
        if round % 2 == 1 {
            order.reverse();
        }
        for i in order {
            let Some(st) = states[i].as_mut() else { continue };
            let start = Start::of(set.cold.as_ref(), targets[i].flavor);
            let (lat, errors, wall, waves) = measure(&targets[i], st, s, index, Phase::Round(round), ops, set, start).await;
            let mut stats = RoundStats::new(lat, &errors, wall, s.bytes_per_op(), set.samples);
            (stats.waves, stats.drops) = waves;
            log(&format!(
                "  {:6} round {}: p50 {} ms, p90 {} ms, {} ops, {} errors{}",
                targets[i].name(),
                round + 1,
                stats.p50_ms.map(crate::report::ms).unwrap_or_else(|| "–".into()),
                stats.p90_ms.map(crate::report::ms).unwrap_or_else(|| "–".into()),
                stats.ops,
                stats.errors,
                stats.first_error.as_ref().map(|e| format!(" (first: {e})")).unwrap_or_default()
            ));
            results[i].rounds.push(stats);
        }
    }
    if let (Some(m), Some(i), Some(before)) = (&set.metrics, voidfs, scraped) {
        match m.bucket_requests().await {
            Ok((after, seconds)) => {
                results[i].bucket_requests = Some(after.into_iter().map(|(op, n)| (op.clone(), n.saturating_sub(before.0.get(&op).copied().unwrap_or(0)))).collect());
                results[i].bucket_seconds = Some(seconds.into_iter().map(|(op, x)| (op.clone(), (x - before.1.get(&op).copied().unwrap_or(0.0)).max(0.0))).collect());
            }
            Err(e) => log(&format!("  reading voidfs's metrics failed: {e:#}")),
        }
    }

    for (i, t) in targets.iter().enumerate() {
        let Some(st) = states[i].take() else { continue };
        if set.verify {
            match verify(t, &st, s).await {
                Ok(true) => results[i].verified = Some(true),
                Ok(false) => {}
                Err(e) => {
                    log(&format!("  {:6} verification failed: {e:#}", t.name()));
                    results[i].verified = Some(false);
                    results[i].verify_error = Some(format!("{e:#}"));
                }
            }
        }
        if !set.keep
            && let Err(e) = t.destroy(&st.place).await
        {
            log(&format!("  {:6} cleanup failed: {e:#}", t.name()));
        }
    }

    for ((t, r), (req, up, down)) in targets.iter().zip(results.iter_mut()).zip(before) {
        let (req2, up2, down2) = t.counters.snapshot();
        (r.requests, r.bytes_up, r.bytes_down) = (req2 - req, up2 - up, down2 - down);
        r.finish();
    }
    ScenarioResult {
        id: s.id.into(),
        name: s.name.into(),
        family: serde_json::to_value(s.family).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
        concurrency: set.concurrency.unwrap_or(s.concurrency),
        ops,
        spacefs: s.spacefs,
        results: targets.iter().map(|t| t.name().to_owned()).zip(results).collect(),
    }
}

/// How a round's operations start.
#[derive(Clone, Copy)]
enum Start<'a> {
    /// Each worker starts its next operation as soon as its last one ends.
    Steady,
    /// In waves of one operation per worker: a wave starts once the one before has ended, after
    /// dropping voidfs's caches if given (`--cold`).
    Waves(Option<&'a Cold>),
}

impl<'a> Start<'a> {
    /// A target's rounds: with `--cold`, every target's run in waves, and voidfs's caches are
    /// dropped before each of its own.
    fn of(cold: Option<&'a Cold>, flavor: Flavor) -> Start<'a> {
        match cold {
            Some(c) => Start::Waves((flavor == Flavor::Voidfs).then_some(c)),
            None => Start::Steady,
        }
    }
}

/// Runs `ops` operations, one worker per slot. Returns the latencies in milliseconds, the
/// errors, the wall-clock time, and in waves, how many ran and how many began with a drop.
#[allow(clippy::too_many_arguments)]
async fn measure(t: &Arc<Target>, st: &mut State, s: &'static Scenario, index: usize, phase: Phase, ops: usize, set: &Settings, start: Start<'_>) -> (Vec<f64>, Vec<String>, Duration, (Option<usize>, Option<usize>)) {
    let started = Instant::now();
    let (t, place, shared, nonce) = (t.clone(), st.place.clone(), st.shared.clone(), set.nonce);
    let run = move |(w, mut slot): (usize, Slot), i: usize| {
        let (t, place, shared) = (t.clone(), place.clone(), shared.clone());
        async move {
            let seed = data::seed(&[nonce, index as u64, phase.code(), i as u64, w as u64]);
            let r = op(&t, &place, s, &shared, &mut slot, phase, i, seed).await;
            ((w, slot), r)
        }
    };
    let workers: Vec<(usize, Slot)> = std::mem::take(&mut st.slots).into_iter().enumerate().collect();
    let done = match start {
        Start::Steady => schedule(workers, ops, false, async || Ok(()), run).await,
        Start::Waves(cold) => {
            schedule(
                workers,
                ops,
                true,
                async || match cold {
                    Some(c) => c.drop_caches().await,
                    None => Ok(()),
                },
                run,
            )
            .await
        }
    };
    st.slots = done.workers.into_iter().map(|(_, slot)| slot).collect();
    let (mut lat, mut errors) = (Vec::new(), Vec::new());
    for r in done.results {
        match r {
            Ok(d) => lat.push(d.as_secs_f64() * 1000.0),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if let Some(e) = done.failed {
        errors.push(format!("dropping voidfs's caches: {e:#}"));
    }
    let waves = match start {
        Start::Steady => (None, None),
        Start::Waves(cold) => (Some(done.waves), Some(if cold.is_some() { done.waves } else { 0 })),
    };
    (lat, errors, started.elapsed(), waves)
}

/// What [`schedule`] ran.
struct Scheduled<W, R> {
    /// The workers, in the order given.
    workers: Vec<W>,
    /// Each operation's result, in no particular order.
    results: Vec<R>,
    /// Why the operations stopped short, if they did: `before_wave` failed.
    failed: Option<anyhow::Error>,
    /// Waves started, each after `before_wave`.
    waves: usize,
}

/// Runs operations `0..ops` on `workers`, each running one at a time. Without `waves`, a worker
/// starts its next operation as soon as its last ends. With `waves`, each worker runs one per
/// wave, and a wave starts after `before_wave`, once the wave before has ended; if
/// `before_wave` fails, nothing more runs.
async fn schedule<W, R, F, Fut>(mut workers: Vec<W>, ops: usize, waves: bool, mut before_wave: impl AsyncFnMut() -> anyhow::Result<()>, op: F) -> Scheduled<W, R>
where
    W: Send + 'static,
    R: Send + 'static,
    F: Fn(W, usize) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = (W, R)> + Send + 'static,
{
    let width = workers.len();
    assert!(width > 0, "no workers");
    let (mut results, mut started, mut from) = (Vec::with_capacity(ops), 0, 0);
    while from < ops {
        if waves {
            if let Err(e) = before_wave().await {
                return Scheduled { workers, results, failed: Some(e), waves: started };
            }
            started += 1;
        }
        let to = if waves { (from + width).min(ops) } else { ops };
        let next = Arc::new(AtomicUsize::new(from));
        let tasks: Vec<_> = workers
            .into_iter()
            .map(|mut w| {
                let (next, op) = (next.clone(), op.clone());
                tokio::spawn(async move {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= to {
                            break;
                        }
                        let (back, r) = op(w, i).await;
                        w = back;
                        out.push(r);
                        if waves {
                            break;
                        }
                    }
                    (w, out)
                })
            })
            .collect();
        workers = Vec::with_capacity(width);
        for task in tasks {
            let (w, out) = task.await.expect("a worker panicked");
            workers.push(w);
            results.extend(out);
        }
        from = to;
    }
    Scheduled { workers, results, failed: None, waves: started }
}

/// Applies an edit to a downloaded object, off the async threads when the object is large.
async fn apply(change: Change, mut buf: Vec<u8>) -> Vec<u8> {
    if buf.len() < (1 << 20) {
        change.apply(&mut buf);
        return buf;
    }
    tokio::task::spawn_blocking(move || {
        change.apply(&mut buf);
        buf
    })
    .await
    .expect("applying an edit")
}

/// One timed operation.
#[allow(clippy::too_many_arguments)]
async fn op(t: &Target, p: &Place, s: &Scenario, sh: &Shared, slot: &mut Slot, phase: Phase, i: usize, seed: u64) -> anyhow::Result<Duration> {
    let elapsed = match s.kind {
        Kind::Get { size, stream } => {
            let t0 = Instant::now();
            let n = if stream { t.stream_get(p, &sh.key).await? } else { t.get(p, &sh.key).await?.len() as u64 };
            let d = t0.elapsed();
            ensure!(n == size, "read {n} of {size} bytes");
            d
        }
        Kind::Range { size, len } => {
            let offset = seed % ((size - len) / EDIT + 1) * EDIT;
            let t0 = Instant::now();
            let n = t.get_range(p, &sh.key, offset, len).await?.len() as u64;
            let d = t0.elapsed();
            ensure!(n == len, "read {n} of {len} bytes");
            d
        }
        Kind::Head { size } => {
            let t0 = Instant::now();
            let n = t.head(p, &sh.key).await?;
            let d = t0.elapsed();
            ensure!(n == size, "HEAD reports {n} bytes, expected {size}");
            d
        }
        Kind::List { keys } => {
            let t0 = Instant::now();
            let n = t.list(p, "list/").await?.len();
            let d = t0.elapsed();
            ensure!(n == keys, "listed {n} keys, expected {keys}");
            d
        }
        Kind::FanoutGet { count, size } => {
            let t0 = Instant::now();
            let n = t.get(p, &sh.keys[i % count]).await?.len() as u64;
            let d = t0.elapsed();
            ensure!(n == size, "read {n} of {size} bytes");
            d
        }
        Kind::Put { size } | Kind::FanoutPut { size, .. } => {
            let body = Bytes::from(data::random_async(seed, size as usize).await);
            let key = format!("new/{}/{i:05}", phase.tag());
            let t0 = Instant::now();
            t.put(p, &key, body).await?;
            t0.elapsed()
        }
        Kind::Overwrite { size } => {
            let body = Bytes::from(data::random_async(seed, size as usize).await);
            let t0 = Instant::now();
            t.put(p, &slot.key, body).await?;
            t0.elapsed()
        }
        Kind::Multipart { size, part } => {
            let body = Bytes::from(data::random_async(seed, size as usize).await);
            let key = format!("new/{}/{i:05}", phase.tag());
            let t0 = Instant::now();
            t.multipart(p, &key, body, part).await?;
            t0.elapsed()
        }
        Kind::Edit { edit, .. } => {
            let change = Change::plan(edit, slot.size, seed);
            let expect = change.size_after(slot.size);
            let d = match t.flavor {
                Flavor::Voidfs => {
                    let patch = change.patch_body().map(Bytes::from);
                    let t0 = Instant::now();
                    let got = match &change {
                        Change::Write { offset, data } => t.write_at(p, &slot.key, *offset, Bytes::copy_from_slice(data), None).await?,
                        Change::Splice { offset, remove, data } => t.splice(p, &slot.key, *offset, *remove, Bytes::copy_from_slice(data)).await?,
                        Change::Truncate { size } => t.write_at(p, &slot.key, 0, Bytes::new(), Some(*size)).await?,
                        Change::Patch { .. } => t.patch(p, &slot.key, patch.unwrap_or_default()).await?,
                    };
                    let d = t0.elapsed();
                    ensure!(got == expect, "voidfs reports {got} bytes after the edit, expected {expect}");
                    d
                }
                Flavor::Bare => {
                    let t0 = Instant::now();
                    let buf = t.get_vec(p, &slot.key, slot.size).await?;
                    ensure!(buf.len() as u64 == slot.size, "downloaded {} of {} bytes", buf.len(), slot.size);
                    let buf = apply(change.clone(), buf).await;
                    t.put(p, &slot.key, Bytes::from(buf)).await?;
                    t0.elapsed()
                }
            };
            slot.size = expect;
            if let Some(m) = slot.model.as_mut() {
                change.apply(m);
            }
            d
        }
        Kind::Rename { .. } => {
            let (src, dst) = if slot.flipped { (&slot.alt, &slot.key) } else { (&slot.key, &slot.alt) };
            let t0 = Instant::now();
            match t.flavor {
                Flavor::Voidfs => t.rename(p, src, dst).await?,
                Flavor::Bare => {
                    t.copy(p, src, dst).await?;
                    t.delete(p, src).await?;
                }
            }
            let d = t0.elapsed();
            slot.flipped = !slot.flipped;
            d
        }
        Kind::MoveDir { files, .. } => {
            let (src, dst) = if slot.flipped { (&slot.alt, &slot.key) } else { (&slot.key, &slot.alt) };
            let t0 = Instant::now();
            match t.flavor {
                Flavor::Voidfs => t.rename(p, src, dst).await?,
                Flavor::Bare => {
                    let n = t.move_prefix(p, src, dst).await?;
                    ensure!(n == files, "moved {n} objects, expected {files}");
                }
            }
            let d = t0.elapsed();
            slot.flipped = !slot.flipped;
            d
        }
    };
    Ok(elapsed)
}

/// Uploads a scenario's starting objects.
async fn setup(t: &Target, p: &Place, s: &Scenario, set: &Settings, index: usize) -> anyhow::Result<(Shared, Vec<Slot>)> {
    t.prepare(p).await?;
    let workers = set.concurrency.unwrap_or(s.concurrency);
    let mut shared = Shared::default();
    let mut slots = vec![Slot::default(); workers];
    let mut uploads: Vec<(String, u64)> = Vec::new();
    match s.kind {
        Kind::Get { size, .. } | Kind::Range { size, .. } | Kind::Head { size } => {
            shared.key = "obj".into();
            uploads.push((shared.key.clone(), size));
        }
        Kind::List { keys } => uploads.extend((0..keys).map(|k| (format!("list/k{k:04}"), LIST_OBJECT))),
        Kind::FanoutGet { count, size } => {
            shared.keys = (0..count).map(|k| format!("fan/o{k:04}")).collect();
            uploads.extend(shared.keys.iter().map(|k| (k.clone(), size)));
        }
        Kind::Put { .. } | Kind::FanoutPut { .. } | Kind::Multipart { .. } => {}
        Kind::Overwrite { size } | Kind::Edit { size, .. } => {
            for (w, slot) in slots.iter_mut().enumerate() {
                slot.key = format!("own/w{w:02}");
                slot.size = size;
                uploads.push((slot.key.clone(), size));
            }
        }
        Kind::Rename { size } => {
            for (w, slot) in slots.iter_mut().enumerate() {
                slot.key = format!("mv/w{w:02}/a");
                slot.alt = format!("mv/w{w:02}/b");
                slot.size = size;
                uploads.push((slot.key.clone(), size));
            }
        }
        Kind::MoveDir { files, size } => {
            for (w, slot) in slots.iter_mut().enumerate() {
                slot.key = format!("dir/w{w:02}/a/");
                slot.alt = format!("dir/w{w:02}/b/");
                uploads.extend((0..files).map(|f| (format!("dir/w{w:02}/a/f{f:03}"), size)));
            }
        }
    }
    let content_seed = |k: &str| data::seed(&[set.nonce, index as u64, 0x5e7u64, u64::from_le_bytes(Sha256::digest(k.as_bytes())[..8].try_into().unwrap())]);
    let results = futures::stream::iter(uploads.iter().map(|(k, size)| {
        let seed = content_seed(k);
        async move {
            let body = Bytes::from(data::random_async(seed, *size as usize).await);
            t.put(p, k, body).await.with_context(|| format!("uploading {k}"))
        }
    }))
    .buffer_unordered(SETUP_PARALLELISM)
    .collect::<Vec<_>>()
    .await;
    results.into_iter().collect::<anyhow::Result<Vec<()>>>()?;
    if set.verify
        && let Kind::Edit { size, .. } = s.kind
    {
        slots[0].model = Some(data::random_async(content_seed(&slots[0].key), size as usize).await);
    }
    Ok((shared, slots))
}

/// Checks that the objects hold what the operations should have left. `Ok(false)`: nothing to
/// check for this scenario.
async fn verify(t: &Target, st: &State, s: &Scenario) -> anyhow::Result<bool> {
    let p = &st.place;
    match s.kind {
        Kind::Edit { .. } | Kind::Overwrite { .. } | Kind::Rename { .. } => {
            for slot in &st.slots {
                let n = t.head(p, slot.current()).await.with_context(|| format!("HEAD {}", slot.current()))?;
                ensure!(n == slot.size, "{} is {n} bytes, expected {}", slot.current(), slot.size);
            }
            if let Some(model) = st.slots.first().and_then(|s| s.model.as_ref()) {
                let got = t.get(p, &st.slots[0].key).await?;
                ensure!(Sha256::digest(&got) == Sha256::digest(model), "{} does not hold the edited content", st.slots[0].key);
            }
            Ok(true)
        }
        Kind::MoveDir { files, .. } => {
            for slot in &st.slots {
                let n = t.list(p, slot.current()).await?.len();
                ensure!(n == files, "{} holds {n} objects, expected {files}", slot.current());
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_requests_are_read_from_a_scrape() {
        let text = "# HELP voidfs_bucket_requests_total Requests to the bucket.\n# TYPE voidfs_bucket_requests_total counter\n\
                    voidfs_bucket_requests_total{op=\"get\"} 12\nvoidfs_bucket_requests_total{op=\"put_new\"} 3\n\
                    voidfs_bucket_request_errors_total{op=\"get\"} 1\nvoidfs_bucket_request_duration_seconds_sum{op=\"get\"} 0.25\n\
                    voidfs_cache_drops_total 2\n";
        let got = by_op(text, "voidfs_bucket_requests_total");
        assert_eq!(got.into_iter().collect::<Vec<_>>(), [("get".to_owned(), 12.0), ("put_new".to_owned(), 3.0)]);
        assert_eq!(by_op(text, "voidfs_bucket_request_duration_seconds_sum").get("get"), Some(&0.25));
        assert_eq!(sample(text, "voidfs_cache_drops_total"), Some(2.0));
        assert_eq!(sample(text, "voidfs_cache_drops"), None);
    }

    /// Which operation ran, on which worker, after how many drops.
    type Ran = (usize, usize, usize);

    /// Operation `i` on worker `w`: records what [`Ran`], and takes 1–4 ms, except the first of
    /// each eight, which takes no time at all.
    fn timed(in_flight: &Arc<AtomicUsize>, drops: &Arc<AtomicUsize>) -> impl Fn(usize, usize) -> futures::future::BoxFuture<'static, (usize, Ran)> + Clone + Send + 'static {
        let (in_flight, drops) = (in_flight.clone(), drops.clone());
        move |w, i| {
            let (in_flight, drops) = (in_flight.clone(), drops.clone());
            Box::pin(async move {
                let seen = drops.load(Ordering::SeqCst);
                in_flight.fetch_add(1, Ordering::SeqCst);
                if i % 8 != 0 {
                    tokio::time::sleep(Duration::from_millis(1 + (i % 4) as u64)).await;
                }
                in_flight.fetch_sub(1, Ordering::SeqCst);
                (w, (i, w, seen))
            })
        }
    }

    /// `--cold`: each wave starts after a drop, once the wave before has ended, and runs one
    /// operation per worker. Every operation runs once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn waves_start_after_a_drop_once_the_one_before_has_ended() {
        let (in_flight, drops) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let drop = async || {
            assert_eq!(in_flight.load(Ordering::SeqCst), 0, "a drop while a wave runs");
            drops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let done = schedule((0..8).collect(), 20, true, drop, timed(&in_flight, &drops)).await;
        assert!(done.failed.is_none());
        assert_eq!((done.waves, drops.load(Ordering::SeqCst)), (3, 3));
        assert_eq!(done.workers, (0..8).collect::<Vec<_>>(), "the workers come back in order");
        let mut ran = done.results;
        ran.sort();
        assert_eq!(ran.iter().map(|r| r.0).collect::<Vec<_>>(), (0..20).collect::<Vec<_>>(), "each operation once");
        for (i, _, seen) in &ran {
            assert_eq!(*seen, i / 8 + 1, "operation {i} started after its own wave's drop");
        }
        for wave in ran.chunks(8) {
            let mut workers: Vec<usize> = wave.iter().map(|r| r.1).collect();
            workers.sort();
            workers.dedup();
            assert_eq!(workers.len(), wave.len(), "one operation per worker in a wave: {wave:?}");
        }
    }

    /// Without `--cold`, workers take operations as they come, and nothing is dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn steady_rounds_drop_nothing() {
        let (in_flight, drops) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let calls = AtomicUsize::new(0);
        let done = schedule((0..8).collect(), 20, false, async || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }, timed(&in_flight, &drops)).await;
        assert_eq!((done.waves, calls.load(Ordering::SeqCst), done.results.len()), (0, 0, 20));
        assert!(done.results.iter().filter(|r| r.1 == 0).count() > 1, "worker 0 finishes its first at once, and takes another");
    }

    /// A drop that fails stops the round there.
    #[tokio::test]
    async fn a_failed_drop_stops_the_round() {
        let (in_flight, drops) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let calls = AtomicUsize::new(0);
        let drop = async || if calls.fetch_add(1, Ordering::SeqCst) == 1 { Err(anyhow::anyhow!("no")) } else { Ok(()) };
        let done = schedule((0..8).collect(), 20, true, drop, timed(&in_flight, &drops)).await;
        assert_eq!((done.waves, done.results.len(), done.workers.len()), (1, 8, 8));
        assert_eq!(done.failed.map(|e| e.to_string()), Some("no".into()));
    }

    /// `--cold` runs every target's rounds in waves, and drops only voidfs's caches.
    #[test]
    fn cold_rounds_run_in_waves_and_drop_voidfss_caches() {
        let cold = Cold::new(1, MetricsSource::new("http://127.0.0.1:9/metrics".into()));
        assert!(matches!(Start::of(Some(&cold), Flavor::Voidfs), Start::Waves(Some(c)) if std::ptr::eq(c, &cold)));
        assert!(matches!(Start::of(Some(&cold), Flavor::Bare), Start::Waves(None)));
        assert!(matches!(Start::of(None, Flavor::Voidfs), Start::Steady));
        assert!(matches!(Start::of(None, Flavor::Bare), Start::Steady));
    }

    /// A stand-in for voidfs-server's metrics: `voidfs_cache_drops_total` goes up 50 ms after
    /// each SIGUSR1 to this process, or the series is left out.
    async fn fake_server(with_drops: bool) -> (String, Arc<AtomicUsize>) {
        let mut usr1 = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let counted = drops.clone();
        tokio::spawn(async move {
            while usr1.recv().await.is_some() {
                tokio::time::sleep(Duration::from_millis(50)).await;
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/metrics", listener.local_addr().unwrap());
        let served = drops.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 1024];
                while !req.ends_with(b"\r\n\r\n") {
                    let n = s.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    req.extend_from_slice(&buf[..n]);
                }
                let body = if with_drops { format!("voidfs_uptime_seconds 1\nvoidfs_cache_drops_total {}\n", served.load(Ordering::SeqCst)) } else { "voidfs_uptime_seconds 1\n".into() };
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        (url, drops)
    }

    /// A drop signals the server and returns once its metrics count it. One the server cannot
    /// confirm is refused before anything is sent: SIGUSR1 stops a server that does not handle it.
    #[tokio::test]
    async fn a_drop_waits_for_the_server_to_count_it() {
        let (url, drops) = fake_server(true).await;
        let cold = Cold::new(std::process::id(), MetricsSource::new(url));
        cold.drop_caches().await.unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1, "returned before the drop");
        cold.drop_caches().await.unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        let (url, drops) = fake_server(false).await;
        let err = Cold::new(std::process::id(), MetricsSource::new(url)).drop_caches().await.unwrap_err();
        assert!(format!("{err:#}").contains("cannot drop its caches"), "{err:#}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(drops.load(Ordering::SeqCst), 0, "no signal sent");
    }
}
