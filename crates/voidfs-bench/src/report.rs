// SPDX-License-Identifier: Apache-2.0
//! Results: the JSON a run writes, and the Markdown table rendered from it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::scenarios::Published;

pub const SPACEFS_SOURCE: &str = "https://docs.spacefs.com/benchmarks/";
pub const SPACEFS_RUN: &str = "20260920T055107Z (build s3sdk@fff9779)";

#[derive(Serialize, Deserialize)]
pub struct RunFile {
    pub harness: String,
    pub run_id: String,
    pub started: String,
    #[serde(default)]
    pub finished: Option<String>,
    pub settings: Settings,
    pub environment: Environment,
    /// Target name → what it was pointed at.
    pub targets: BTreeMap<String, String>,
    pub spacefs: SpacefsRef,
    pub scenarios: Vec<ScenarioResult>,
}

#[derive(Serialize, Deserialize)]
pub struct Settings {
    pub rounds: usize,
    pub warmups: usize,
    /// Set when every scenario ran at one concurrency instead of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    pub connections: usize,
    pub ops_scale: f64,
    pub checksums: String,
    /// The run reproduces SpaceFS's published setup, so its ratios can be set against theirs.
    pub like_spacefs: bool,
    /// Every measured operation started with voidfs's caches empty (`--cold`): the rounds ran
    /// in waves, and the caches were dropped before each. Set against SpaceFS's cache-cleared
    /// figures, where they give one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cold: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Environment {
    pub os: String,
    pub arch: String,
    pub cpus: usize,
    /// Free-form `--label key=value` pairs: machine, regions, backend, commit.
    pub labels: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub struct SpacefsRef {
    pub source: String,
    pub run: String,
}

#[derive(Serialize, Deserialize)]
pub struct ScenarioResult {
    pub id: String,
    pub name: String,
    pub family: String,
    pub concurrency: usize,
    pub ops: usize,
    pub spacefs: Published,
    /// Target name → its result.
    pub results: BTreeMap<String, TargetResult>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct TargetResult {
    /// The figure: the median of the rounds' p50s, in milliseconds.
    pub p50_ms: Option<f64>,
    pub rounds: Vec<RoundStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_error: Option<String>,
    #[serde(default)]
    pub warmup_errors: usize,
    /// Whether the objects held what the operations should have left (edits, renames, moves,
    /// overwrites); absent where there is nothing to check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_error: Option<String>,
    /// Requests the harness sent to this target for the whole scenario, setup and cleanup
    /// included. For voidfs these reach the server; its own requests to the bucket are not seen.
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub bytes_up: u64,
    #[serde(default)]
    pub bytes_down: u64,
    /// For voidfs, with `--voidfs-metrics`: the requests it sent to the bucket during the
    /// measured rounds, by operation, from its metrics. That includes its read of
    /// `gc/pending.json` once a minute; the other target's rounds in between send it none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_requests: Option<BTreeMap<String, u64>>,
    /// The time those requests took, added up, by operation: over a round's wall-clock time,
    /// how many were in flight on average.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_seconds: Option<BTreeMap<String, f64>>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct RoundStats {
    pub ops: usize,
    pub errors: usize,
    pub p50_ms: Option<f64>,
    pub p90_ms: Option<f64>,
    pub p99_ms: Option<f64>,
    pub mean_ms: Option<f64>,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    /// Wall-clock time of the whole round.
    pub wall_ms: f64,
    /// Payload bytes moved per second over the round, where the scenario moves payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mib_per_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples_ms: Option<Vec<f64>>,
    /// With `--cold`: the waves of one operation per worker the round ran in...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waves: Option<usize>,
    /// ...and how many of them started with voidfs's caches dropped: all of voidfs's, none of
    /// the bare bucket's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drops: Option<usize>,
}

/// The `q` quantile of sorted values, interpolating linearly between neighbours.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    Some(quantile(&v, 0.5))
}

impl RoundStats {
    pub fn new(mut latencies: Vec<f64>, errors: &[String], wall: Duration, bytes_per_op: u64, keep_samples: bool) -> RoundStats {
        let wall_ms = wall.as_secs_f64() * 1000.0;
        let mut r = RoundStats { ops: latencies.len(), errors: errors.len(), wall_ms, first_error: errors.first().cloned(), ..Default::default() };
        if latencies.is_empty() {
            return r;
        }
        let samples = keep_samples.then(|| latencies.clone());
        latencies.sort_by(f64::total_cmp);
        r.p50_ms = Some(quantile(&latencies, 0.5));
        r.p90_ms = Some(quantile(&latencies, 0.9));
        r.p99_ms = Some(quantile(&latencies, 0.99));
        r.mean_ms = Some(latencies.iter().sum::<f64>() / latencies.len() as f64);
        r.min_ms = latencies.first().copied();
        r.max_ms = latencies.last().copied();
        if bytes_per_op > 0 && wall_ms > 0.0 {
            r.mib_per_s = Some((bytes_per_op * latencies.len() as u64) as f64 / (1 << 20) as f64 / (wall_ms / 1000.0));
        }
        r.samples_ms = samples;
        r
    }
}

impl TargetResult {
    pub fn finish(&mut self) {
        let p50s: Vec<f64> = self.rounds.iter().filter_map(|r| r.p50_ms).collect();
        self.p50_ms = median(&p50s);
    }

    pub fn errors(&self) -> usize {
        self.rounds.iter().map(|r| r.errors).sum::<usize>() + self.warmup_errors
    }
}

impl ScenarioResult {
    /// How many times faster voidfs was than the bare bucket (below 1: slower).
    pub fn speedup(&self) -> Option<f64> {
        let v = self.results.get("voidfs")?.p50_ms?;
        let b = self.results.get("bare")?.p50_ms?;
        (v > 0.0).then(|| b / v)
    }

    /// SpaceFS's speed-up for the same: with its cache cleared in a cold run, where they give
    /// that.
    pub fn theirs(&self, cold: bool) -> Option<f64> {
        if cold { self.spacefs.layer_cold.map(|c| self.spacefs.bare / c) } else { Some(self.spacefs.speedup()) }
    }
}

// ---------------------------------------------------------------------------------------------
// Markdown

/// Milliseconds the way SpaceFS prints them: `0.9`, `44.1`, `1,774`.
pub fn ms(v: f64) -> String {
    if v >= 100.0 {
        let n = v.round() as u64;
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    } else if v >= 1.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

fn factor(x: f64) -> String {
    if x >= 10.0 { format!("{x:.0}×") } else { format!("{x:.1}×") }
}

/// `34× faster`, `2.0× slower`.
pub fn result(speedup: f64) -> String {
    if speedup >= 1.0 { format!("{} faster", factor(speedup)) } else { format!("{} slower", factor(1.0 / speedup)) }
}

fn geomean(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| (xs.iter().map(|x| x.ln()).sum::<f64>() / xs.len() as f64).exp())
}

pub fn markdown(run: &RunFile) -> String {
    let mut out = String::new();
    let like = run.settings.like_spacefs;
    let cold = run.settings.cold;
    let _ = writeln!(out, "# voidfs against a bare bucket{}\n", if cold { ", cold" } else { "" });
    if !like {
        let _ = writeln!(
            out,
            "> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup \
             (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their \
             published result, shown for orientation only; compare voidfs with the bare bucket \
             beside it.\n"
        );
    }
    let _ = writeln!(out, "| | |\n|---|---|");
    let _ = writeln!(out, "| Run | `{}` |", run.run_id);
    let _ = writeln!(out, "| When | {} |", run.started);
    for (name, what) in &run.targets {
        let label = if name == "bare" { "Bare bucket" } else { "voidfs" };
        let _ = writeln!(out, "| {label} | {} |", what.replace('|', "\\|"));
    }
    for (k, v) in &run.environment.labels {
        let _ = writeln!(out, "| {k} | {} |", v.replace('|', "\\|"));
    }
    let _ = writeln!(out, "| Harness host | {} {}, {} CPUs |", run.environment.os, run.environment.arch, run.environment.cpus);
    let s = &run.settings;
    let concurrency = match s.concurrency {
        Some(n) => format!("concurrency {n} in every scenario (overridden: not SpaceFS's shape)"),
        None => "concurrency 8 unless the scenario names its own".into(),
    };
    let _ = writeln!(
        out,
        "| Shape | {} scenarios, {} rounds, {} warm-up operations each, {concurrency}, at most {} requests in flight |",
        run.scenarios.len(),
        s.rounds,
        s.warmups,
        s.connections
    );
    if s.ops_scale != 1.0 {
        let _ = writeln!(out, "| Operations | scaled by {} from the harness defaults |", s.ops_scale);
    }
    if cold {
        let _ = writeln!(
            out,
            "| Cold | Each round ran in waves of one operation per worker, each wave once the one before had ended; voidfs-server's shard and page caches were dropped before each of its waves. The bare bucket's own caches were not |"
        );
    }
    let _ = writeln!(out, "| Figure | Median of each round's p50, in milliseconds |");
    let _ = writeln!(
        out,
        "| SpaceFS column | Their published run {}, [{}]({}){} |\n",
        SPACEFS_RUN,
        SPACEFS_SOURCE,
        SPACEFS_SOURCE,
        if cold { ", with their cache cleared: given for four rows" } else { "" }
    );

    let mut rows: Vec<&ScenarioResult> = run.scenarios.iter().collect();
    rows.sort_by(|a, b| b.speedup().unwrap_or(-1.0).total_cmp(&a.speedup().unwrap_or(-1.0)));
    let measured: Vec<(f64, Option<f64>)> = rows.iter().filter_map(|r| r.speedup().map(|x| (x, r.theirs(cold)))).collect();
    if !measured.is_empty() {
        let faster = measured.iter().filter(|(x, _)| *x >= 1.0).count();
        let ours = geomean(&measured.iter().map(|m| m.0).collect::<Vec<_>>()).unwrap();
        let _ = write!(
            out,
            "voidfs is faster in **{faster} of {}** scenarios and slower in the other **{}**. \
             Geometric mean speed-up over the bare bucket: **{}**",
            measured.len(),
            measured.len() - faster,
            factor(ours)
        );
        let both: Vec<(f64, f64)> = measured.iter().filter_map(|(x, t)| Some((*x, (*t)?))).collect();
        match geomean(&both.iter().map(|m| m.1).collect::<Vec<_>>()) {
            Some(theirs) if both.len() == measured.len() => {
                let _ = writeln!(out, " (SpaceFS's, same rows: {}).\n", factor(theirs));
            }
            Some(theirs) => {
                let ours = geomean(&both.iter().map(|m| m.0).collect::<Vec<_>>()).unwrap();
                let _ = writeln!(out, "; over the {} rows SpaceFS gives a figure for, {} (SpaceFS's: {}).\n", both.len(), factor(ours), factor(theirs));
            }
            None => {
                let _ = writeln!(out, ".\n");
            }
        }
        if like && !both.is_empty() {
            let level = both.iter().filter(|(x, t)| x >= t).count();
            let _ = writeln!(out, "voidfs's speed-up matches or beats SpaceFS's in **{level} of {}** rows.\n", both.len());
        }
    }

    let _ = write!(out, "| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result{} |", if cold { ", cache cleared" } else { "" });
    let _ = writeln!(out, "{}", if like { " voidfs vs SpaceFS |" } else { "" });
    let _ = writeln!(out, "|---|--:|--:|---|---|{}", if like { "---|" } else { "" });
    let mut notes: Vec<String> = Vec::new();
    for r in rows {
        let cell = |t: &str| r.results.get(t).and_then(|x| x.p50_ms).map(ms).unwrap_or_else(|| "–".into());
        let mut name = r.name.clone();
        for (t, x) in &r.results {
            let mut why = Vec::new();
            if let Some(e) = &x.setup_error {
                why.push(format!("setup failed: {e}"));
            }
            if x.errors() > 0 {
                let first = x.rounds.iter().find_map(|r| r.first_error.clone()).unwrap_or_default();
                why.push(format!("{} failed operations ({first})", x.errors()));
            }
            if x.verified == Some(false) {
                why.push(format!("verification failed: {}", x.verify_error.clone().unwrap_or_default()));
            }
            if !why.is_empty() {
                notes.push(format!("{} on {t}: {}", r.name, why.join("; ")));
                name.push_str(&format!(" [^{}]", notes.len()));
            }
        }
        let ours = r.speedup().map(result).unwrap_or_else(|| "–".into());
        let theirs = r.theirs(cold);
        let _ = write!(out, "| {name} | {} | {} | {ours} | {} |", cell("bare"), cell("voidfs"), theirs.map(result).unwrap_or_else(|| "–".into()));
        if like {
            let gap = match (r.speedup(), theirs) {
                (Some(x), Some(t)) if x >= t => "level or ahead".to_string(),
                (Some(x), Some(t)) => format!("{} short", factor(t / x)),
                _ => "–".into(),
            };
            let _ = write!(out, " {gap} |");
        }
        let _ = writeln!(out);
    }
    if !notes.is_empty() {
        let _ = writeln!(out);
        for (i, n) in notes.iter().enumerate() {
            let short: String = n.replace('\n', " ").chars().take(300).collect();
            let _ = writeln!(out, "[^{}]: {short}", i + 1);
        }
    }
    bucket_requests(run, &mut out);
    out
}

/// A table of voidfs's requests to the bucket per operation, in the order the scenarios ran, if
/// the run recorded them.
fn bucket_requests(run: &RunFile, out: &mut String) {
    let rows: Vec<(&ScenarioResult, &BTreeMap<String, u64>, usize)> = run
        .scenarios
        .iter()
        .filter_map(|s| {
            let r = s.results.get("voidfs")?;
            Some((s, r.bucket_requests.as_ref()?, r.rounds.iter().map(|x| x.ops).sum::<usize>()))
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    let mut ops: Vec<&str> = rows.iter().flat_map(|(_, b, _)| b.iter().filter(|(_, n)| **n > 0).map(|(op, _)| op.as_str())).collect();
    ops.sort();
    ops.dedup();
    let _ = writeln!(out, "\n## voidfs's requests to the bucket\n");
    let _ = writeln!(out, "Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).\n");
    let _ = writeln!(out, "| Scenario | Operations | Requests per operation | {} |", ops.join(" | "));
    let _ = writeln!(out, "|---|--:|--:|{}", "--:|".repeat(ops.len()));
    for (s, b, n) in rows {
        let per = |x: u64| if n == 0 { "–".to_owned() } else { format!("{:.2}", x as f64 / n as f64) };
        let cells: Vec<String> = ops.iter().map(|op| b.get(*op).copied().filter(|x| *x > 0).map(per).unwrap_or_default()).collect();
        let _ = writeln!(out, "| {} | {n} | {} | {} |", s.name, per(b.values().sum()), cells.join(" | "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_spacefs() {
        assert_eq!(ms(1774.0), "1,774");
        assert_eq!(ms(99.0), "99.0");
        assert_eq!(ms(0.9), "0.90");
        assert_eq!(ms(3209.4), "3,209");
        assert_eq!(result(44.1 / 1.3), "34× faster");
        assert_eq!(result(27.7 / 84.6), "3.1× slower");
        assert_eq!(result(108.0 / 106.0), "1.0× faster");
    }

    /// A run of `ids` where voidfs took `voidfs` ms and the bare bucket `bare`.
    fn run_of(ids: &[&str], voidfs: f64, bare: f64, cold: bool) -> RunFile {
        let result = |p50: f64| TargetResult { p50_ms: Some(p50), rounds: vec![RoundStats { ops: 1, p50_ms: Some(p50), ..Default::default() }], ..Default::default() };
        let scenarios = ids
            .iter()
            .map(|id| {
                let s = crate::scenarios::ALL.iter().find(|s| s.id == *id).unwrap();
                let results = [("voidfs".to_owned(), result(voidfs)), ("bare".to_owned(), result(bare))].into_iter().collect();
                ScenarioResult { id: s.id.into(), name: s.name.into(), family: "reads".into(), concurrency: 8, ops: 32, spacefs: s.spacefs, results }
            })
            .collect();
        RunFile {
            harness: "test".into(),
            run_id: "r".into(),
            started: "now".into(),
            finished: None,
            settings: Settings { rounds: 2, warmups: 3, concurrency: None, connections: 64, ops_scale: 1.0, checksums: "SdkDefault".into(), like_spacefs: true, cold },
            environment: Environment { os: "macos".into(), arch: "aarch64".into(), cpus: 1, labels: BTreeMap::new() },
            targets: BTreeMap::new(),
            spacefs: SpacefsRef { source: SPACEFS_SOURCE.into(), run: SPACEFS_RUN.into() },
            scenarios,
        }
    }

    /// A cold run is set against SpaceFS's cache-cleared figures, and only where they give one.
    #[test]
    fn cold_runs_compare_with_spacefss_cold_figures() {
        let warm = markdown(&run_of(&["get-64m"], 300.0, 779.0, false));
        assert!(warm.contains("| get 64 MiB | 779 | 300 | 2.6× faster | 17× faster | 6.4× short |"), "{warm}");
        let cold = markdown(&run_of(&["get-64m", "stream-get-64m"], 290.0, 779.0, true));
        assert!(cold.contains("SpaceFS result, cache cleared"), "{cold}");
        assert!(cold.contains("| get 64 MiB | 779 | 290 | 2.7× faster | 2.6× faster | level or ahead |"), "779 / 299: {cold}");
        assert!(cold.contains("| stream get 64 MiB | 779 | 290 | 2.7× faster | – | – |"), "no cold figure: {cold}");
        assert!(cold.contains("over the 1 rows SpaceFS gives a figure for"), "{cold}");
        assert!(cold.contains("matches or beats SpaceFS's in **1 of 1** rows"), "{cold}");
    }

    /// Files written before `--cold` and the bandwidth cap still read, as warm runs.
    #[test]
    fn older_result_files_still_read() {
        let mut v = serde_json::to_value(run_of(&["get-4k"], 1.0, 12.0, false)).unwrap();
        let round = &mut v["scenarios"][0]["results"]["voidfs"]["rounds"][0];
        assert!(round.get("waves").is_none() && round.get("drops").is_none());
        assert!(v["settings"].get("cold").is_none(), "left out of warm runs, as before");
        let old: RunFile = serde_json::from_value(v).unwrap();
        assert!(!old.settings.cold);
        let cold: RunFile = serde_json::from_str(&serde_json::to_string(&run_of(&["get-4k"], 1.0, 12.0, true)).unwrap()).unwrap();
        assert!(cold.settings.cold);
    }

    #[test]
    fn quantiles_interpolate() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(quantile(&v, 0.5), 2.5);
        assert_eq!(median(&[3.0, 1.0]), Some(2.0));
        let r = RoundStats::new(vec![5.0, 1.0, 3.0], &[], Duration::from_millis(10), 0, false);
        assert_eq!((r.p50_ms, r.min_ms, r.max_ms), (Some(3.0), Some(1.0), Some(5.0)));
    }
}
