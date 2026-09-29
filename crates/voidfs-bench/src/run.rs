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

    /// voidfs's requests to the bucket so far, by operation.
    async fn bucket_requests(&self) -> anyhow::Result<BTreeMap<String, u64>> {
        let text = self.http.get(&self.url).send().await?.error_for_status()?.text().await?;
        Ok(bucket_requests(&text))
    }
}

/// The `voidfs_bucket_requests_total` series of a scrape, by operation.
fn bucket_requests(text: &str) -> BTreeMap<String, u64> {
    text.lines()
        .filter_map(|l| {
            let (op, value) = l.strip_prefix("voidfs_bucket_requests_total{op=\"")?.split_once("\"} ")?;
            Some((op.to_owned(), value.parse::<f64>().ok()? as u64))
        })
        .collect()
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
            let (_, errors, _) = measure(t, st, s, index, Phase::Warmup, set.warmups, set).await;
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
            let (lat, errors, wall) = measure(&targets[i], st, s, index, Phase::Round(round), ops, set).await;
            let stats = RoundStats::new(lat, &errors, wall, s.bytes_per_op(), set.samples);
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
            Ok(after) => results[i].bucket_requests = Some(after.into_iter().map(|(op, n)| (op.clone(), n.saturating_sub(before.get(&op).copied().unwrap_or(0)))).collect()),
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

/// Runs `ops` operations, one worker per slot. Returns the latencies in milliseconds,
/// the errors, and the wall-clock time.
async fn measure(t: &Arc<Target>, st: &mut State, s: &'static Scenario, index: usize, phase: Phase, ops: usize, set: &Settings) -> (Vec<f64>, Vec<String>, Duration) {
    let next = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let slots = std::mem::take(&mut st.slots);
    let tasks: Vec<_> = slots
        .into_iter()
        .enumerate()
        .map(|(w, mut slot)| {
            let (t, place, shared, next) = (t.clone(), st.place.clone(), st.shared.clone(), next.clone());
            let nonce = set.nonce;
            tokio::spawn(async move {
                let (mut lat, mut errors) = (Vec::new(), Vec::new());
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= ops {
                        break;
                    }
                    let seed = data::seed(&[nonce, index as u64, phase.code(), i as u64, w as u64]);
                    match op(&t, &place, s, &shared, &mut slot, phase, i, seed).await {
                        Ok(d) => lat.push(d.as_secs_f64() * 1000.0),
                        Err(e) => errors.push(format!("{e:#}")),
                    }
                }
                (slot, lat, errors)
            })
        })
        .collect();
    let (mut lat, mut errors) = (Vec::new(), Vec::new());
    for task in tasks {
        let (slot, l, e) = task.await.expect("a worker panicked");
        st.slots.push(slot);
        lat.extend(l);
        errors.extend(e);
    }
    (lat, errors, started.elapsed())
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
                    voidfs_bucket_request_errors_total{op=\"get\"} 1\n";
        let got = bucket_requests(text);
        assert_eq!(got.into_iter().collect::<Vec<_>>(), [("get".to_owned(), 12), ("put_new".to_owned(), 3)]);
    }
}
