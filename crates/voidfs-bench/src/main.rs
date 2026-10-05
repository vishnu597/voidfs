// SPDX-License-Identifier: Apache-2.0
//! `voidfs-bench`: SpaceFS's 49 published benchmark scenarios, run through a voidfs server and
//! against the bare bucket underneath it (docs/PARITY.md, step 1).

mod data;
mod delay;
mod report;
mod run;
mod scenarios;
mod target;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use clap::{Parser, Subcommand};

use crate::report::{Environment, RunFile, SpacefsRef};
use crate::scenarios::{ALL, Scenario};
use crate::target::{Checksums, Endpoint, Target};

/// Run SpaceFS's benchmark scenarios against voidfs and the bare bucket beneath it.
///
/// Values taken from the environment are never printed, not even by --help.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the scenarios and write JSON and Markdown results.
    Run(Box<RunArgs>),
    /// List the scenarios with SpaceFS's published figures.
    List {
        #[command(flatten)]
        filter: Filter,
    },
    /// List, and with --yes delete, every object under a prefix of the bucket, such as a whole
    /// voidfs pool. To reclaim only what the pool no longer references, use `voidfs-server gc`.
    Purge {
        #[command(flatten)]
        bucket: Bucket,
        /// The prefix, ending in `/`, for example voidfs-bench/pool/
        #[arg(long)]
        prefix: String,
        /// Delete; without it, only count.
        #[arg(long)]
        yes: bool,
    },
    /// Relay TCP to an S3 server with a fixed delay each way, and optionally a bandwidth cap, to
    /// emulate a distant bucket.
    Delay {
        #[arg(long, default_value = "127.0.0.1:7071")]
        listen: std::net::SocketAddr,
        /// The S3 server, as host:port.
        #[arg(long, default_value = "127.0.0.1:7070")]
        upstream: String,
        /// Delay added in each direction; a round trip gains twice this.
        #[arg(long)]
        one_way_ms: f64,
        /// The most it carries: `s3`, fitted to the bare bucket in SpaceFS's run, or
        /// `DOWN/UP/TOTAL` in MB/s, down and up per connection and in all each way (`-` for no
        /// limit). Unlimited if not given.
        #[arg(long, value_parser = delay::Bandwidth::parse)]
        bandwidth: Option<delay::Bandwidth>,
    },
    /// Render a results file as a Markdown table.
    Report {
        file: PathBuf,
        /// Add the column that sets voidfs's speed-up against SpaceFS's (only meaningful for a
        /// run in SpaceFS's setup).
        #[arg(long)]
        like_spacefs: bool,
    },
}

#[derive(clap::Args)]
struct RunArgs {
    /// Targets to run, in order: voidfs, bare, or both.
    #[arg(long, value_delimiter = ',', default_value = "voidfs,bare")]
    targets: Vec<String>,

    /// The voidfs server, for example http://127.0.0.1:9000
    #[arg(long, env = "VOIDFS_ENDPOINT", hide_env_values = true)]
    voidfs_endpoint: Option<String>,
    /// Admin-scoped voidfs access key id (each scenario creates and deletes a drive).
    #[arg(long, env = "VOIDFS_ACCESS_KEY_ID", hide_env_values = true)]
    voidfs_access_key_id: Option<String>,
    #[arg(long, env = "VOIDFS_SECRET_ACCESS_KEY", hide_env_values = true)]
    voidfs_secret_access_key: Option<String>,
    /// Drives are named `<prefix>-<run>-<nn>`.
    #[arg(long, default_value = "vfbench")]
    voidfs_drive_prefix: String,
    /// voidfs-server's metrics, for example http://127.0.0.1:9001/metrics (its `--admin-listen`).
    /// With it, each scenario records the requests voidfs sent to the bucket in its measured
    /// rounds.
    #[arg(long, env = "VOIDFS_METRICS_URL")]
    voidfs_metrics: Option<String>,
    /// Measure cold reads: run each round in waves of one operation per worker, and drop
    /// voidfs-server's caches before each of its waves, so that every measured operation starts
    /// with them empty. Needs `--voidfs-pid` and `--voidfs-metrics`: the server on this machine,
    /// SIGUSR1 to drop, its metrics to confirm. The bare target runs the same waves.
    #[arg(long, requires_all = ["voidfs_pid", "voidfs_metrics"])]
    cold: bool,
    /// voidfs-server's process id, for `--cold`.
    #[arg(long, env = "VOIDFS_SERVER_PID")]
    voidfs_pid: Option<u32>,

    #[command(flatten)]
    bucket: Bucket,
    /// Key prefix for the bare side's objects. Keep it apart from the voidfs pool's prefix.
    #[arg(long, default_value = "voidfs-bench/bare/")]
    bare_prefix: String,

    #[command(flatten)]
    filter: Filter,
    #[arg(long, default_value_t = 2)]
    rounds: usize,
    #[arg(long, default_value_t = 3)]
    warmups: usize,
    /// Run every scenario at this concurrency instead of its own. For diagnosis: the results
    /// no longer follow SpaceFS's scenarios.
    #[arg(long)]
    concurrency: Option<usize>,
    /// Requests in flight at once per target (SpaceFS: 64 connections).
    #[arg(long, default_value_t = 64)]
    connections: usize,
    /// Multiply every scenario's operations per round (for quick runs, for example 0.25).
    #[arg(long, default_value_t = 1.0)]
    ops_scale: f64,
    /// Upload checksums, the same for both targets.
    #[arg(long, value_enum, default_value = "sdk-default")]
    checksums: Checksums,
    /// This run reproduces SpaceFS's setup: add the voidfs-against-SpaceFS column.
    #[arg(long)]
    like_spacefs: bool,
    /// Describe the run, for example `--label client="GCP n2-standard-8, us-east4"` (repeatable).
    #[arg(long = "label", value_parser = parse_label)]
    labels: Vec<(String, String)>,
    /// Skip reading objects back after edits, renames and moves.
    #[arg(long)]
    no_verify: bool,
    /// Leave endpoints and bucket names out of the output (for results you will share).
    #[arg(long)]
    redact: bool,
    /// Leave every object and drive in place.
    #[arg(long)]
    keep: bool,
    /// Record every operation's latency in the JSON.
    #[arg(long)]
    samples: bool,
    /// Where to write `<name>.json` and `<name>.md`.
    #[arg(long, default_value = ".")]
    out_dir: PathBuf,
    /// Base name of the result files (default: the run id).
    #[arg(long)]
    name: Option<String>,
}

/// The bucket, reached directly: the bare target, with the variables voidfs-server reads.
#[derive(clap::Args)]
struct Bucket {
    /// The bucket's S3 endpoint; leave out for AWS S3 itself.
    #[arg(long, env = "VOIDFS_S3_ENDPOINT", hide_env_values = true)]
    bare_endpoint: Option<String>,
    /// The bucket, the same one the voidfs server's pool lives in.
    #[arg(long, env = "VOIDFS_S3_BUCKET", hide_env_values = true)]
    bare_bucket: Option<String>,
    #[arg(long, env = "VOIDFS_S3_REGION", default_value = "us-east-1", hide_env_values = true)]
    bare_region: String,
    #[arg(long, env = "VOIDFS_S3_ACCESS_KEY_ID", hide_env_values = true)]
    bare_access_key_id: Option<String>,
    #[arg(long, env = "VOIDFS_S3_SECRET_ACCESS_KEY", hide_env_values = true)]
    bare_secret_access_key: Option<String>,
}

impl Bucket {
    fn target(&self, prefix: &str, connections: usize, checksums: Checksums) -> anyhow::Result<Target> {
        let need = |v: &Option<String>, what: &str| v.clone().ok_or_else(|| anyhow!("{what} is required (flag or environment)"));
        Ok(Target::bare(
            Endpoint {
                url: self.bare_endpoint.clone(),
                region: self.bare_region.clone(),
                access_key_id: need(&self.bare_access_key_id, "--bare-access-key-id")?,
                secret_access_key: need(&self.bare_secret_access_key, "--bare-secret-access-key")?,
            },
            &need(&self.bare_bucket, "--bare-bucket")?,
            prefix,
            connections,
            checksums,
        ))
    }
}

/// Which scenarios to run.
#[derive(clap::Args)]
struct Filter {
    /// Only scenarios whose id or name contains this text (repeatable).
    #[arg(long = "scenario")]
    scenarios: Vec<String>,
    /// Leave out scenarios whose id or name contains this text (repeatable), for example
    /// `--exclude 32m --exclude 64m --exclude 256m` for small objects only.
    #[arg(long = "exclude")]
    excludes: Vec<String>,
}

fn parse_label(s: &str) -> Result<(String, String), String> {
    s.split_once('=').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())).ok_or_else(|| "a label is key=value".into())
}

fn select(f: &Filter) -> Vec<(usize, &'static Scenario)> {
    let hit = |s: &Scenario, t: &String| s.id.contains(t.as_str()) || s.name.contains(t.as_str());
    ALL.iter()
        .enumerate()
        .filter(|(_, s)| (f.scenarios.is_empty() || f.scenarios.iter().any(|t| hit(s, t))) && !f.excludes.iter().any(|t| hit(s, t)))
        .collect()
}

fn write_results(run: &RunFile, dir: &Path, name: &str) -> anyhow::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let json = dir.join(format!("{name}.json"));
    let md = dir.join(format!("{name}.md"));
    std::fs::write(&json, serde_json::to_vec_pretty(run)?).with_context(|| format!("writing {}", json.display()))?;
    std::fs::write(&md, report::markdown(run)).with_context(|| format!("writing {}", md.display()))?;
    Ok((json, md))
}

async fn run(args: RunArgs) -> anyhow::Result<ExitCode> {
    let selected = select(&args.filter);
    if selected.is_empty() {
        bail!("no scenario matches {:?} without {:?}", args.filter.scenarios, args.filter.excludes);
    }
    let need = |v: &Option<String>, what: &str| v.clone().ok_or_else(|| anyhow!("{what} is required (flag or environment)"));
    let mut targets: Vec<Arc<Target>> = Vec::new();
    let mut described = BTreeMap::new();
    for name in &args.targets {
        let t = match name.as_str() {
            "voidfs" => Target::voidfs(
                Endpoint {
                    url: Some(need(&args.voidfs_endpoint, "--voidfs-endpoint")?),
                    region: "us-east-1".into(),
                    access_key_id: need(&args.voidfs_access_key_id, "--voidfs-access-key-id")?,
                    secret_access_key: need(&args.voidfs_secret_access_key, "--voidfs-secret-access-key")?,
                },
                &args.voidfs_drive_prefix,
                args.connections,
                args.checksums,
            )?,
            "bare" => args.bucket.target(&args.bare_prefix, args.connections, args.checksums)?,
            other => bail!("unknown target {other:?}: use voidfs or bare"),
        };
        let describe = match (args.redact, name.as_str()) {
            (false, _) => t.describe.clone(),
            (true, "voidfs") => "a voidfs server (endpoint not recorded)".into(),
            (true, _) => "the bucket reached directly (endpoint and name not recorded)".into(),
        };
        described.insert(t.name().to_owned(), describe);
        targets.push(Arc::new(t));
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let nonce = data::seed(&[now.as_nanos() as u64, u64::from(std::process::id())]);
    let tag: String = {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        (0..6).map(|i| ALPHABET[(data::seed(&[nonce, i]) % ALPHABET.len() as u64) as usize] as char).collect()
    };
    let started = chrono::Utc::now();
    let run_id = started.format("%Y%m%dT%H%M%SZ").to_string();
    let name = args.name.clone().unwrap_or_else(|| run_id.clone());
    let settings = run::Settings {
        rounds: args.rounds,
        concurrency: args.concurrency,
        warmups: args.warmups,
        ops_scale: args.ops_scale,
        verify: !args.no_verify,
        keep: args.keep,
        samples: args.samples,
        nonce,
        tag: tag.clone(),
        metrics: args.voidfs_metrics.clone().map(run::MetricsSource::new),
        cold: match (args.cold, args.voidfs_pid, &args.voidfs_metrics) {
            (true, Some(pid), Some(url)) => Some(run::Cold::new(pid, run::MetricsSource::new(url.clone()))),
            _ => None,
        },
    };
    let mut file = RunFile {
        harness: format!("voidfs-bench {}", env!("CARGO_PKG_VERSION")),
        run_id: run_id.clone(),
        started: started.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        finished: None,
        settings: report::Settings {
            rounds: args.rounds,
            warmups: args.warmups,
            concurrency: args.concurrency,
            connections: args.connections,
            ops_scale: args.ops_scale,
            checksums: format!("{:?}", args.checksums),
            like_spacefs: args.like_spacefs,
            cold: args.cold,
        },
        environment: Environment {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            cpus: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
            labels: args.labels.iter().cloned().collect(),
        },
        targets: described,
        spacefs: SpacefsRef { source: report::SPACEFS_SOURCE.into(), run: report::SPACEFS_RUN.into() },
        scenarios: Vec::new(),
    };

    eprintln!("run {run_id} (tag {tag}): {} scenarios against {}", selected.len(), args.targets.join(" and "));
    // With --redact, endpoints and bucket names are scrubbed from everything written or shown.
    let mut hidden: Vec<String> = Vec::new();
    if args.redact {
        for url in [&args.bucket.bare_endpoint, &args.voidfs_endpoint].into_iter().flatten() {
            hidden.push(url.clone());
            if let Ok(uri) = url.parse::<http::Uri>()
                && let Some(host) = uri.host()
            {
                hidden.push(host.to_owned());
            }
        }
        hidden.extend(args.bucket.bare_bucket.iter().cloned());
        hidden.retain(|h| !h.is_empty());
        hidden.sort_by_key(|h| std::cmp::Reverse(h.len()));
    }
    let scrub = |text: &str| hidden.iter().fold(text.to_owned(), |t, h| t.replace(h.as_str(), "<hidden>"));
    let log = |line: &str| {
        eprintln!("{}", scrub(line));
        let _ = std::io::stderr().flush();
    };
    for (n, (index, s)) in selected.iter().enumerate() {
        eprintln!("[{}/{}] {} ({})", n + 1, selected.len(), s.name, s.id);
        let r = run::scenario(&targets, *index, s, &settings, &log).await;
        let v = r.results.get("voidfs").and_then(|x| x.p50_ms);
        let b = r.results.get("bare").and_then(|x| x.p50_ms);
        eprintln!(
            "  => voidfs {} ms, bare {} ms: {} (SpaceFS: {})",
            v.map(report::ms).unwrap_or_else(|| "–".into()),
            b.map(report::ms).unwrap_or_else(|| "–".into()),
            r.speedup().map(report::result).unwrap_or_else(|| "–".into()),
            report::result(s.spacefs.speedup())
        );
        let mut r = r;
        for t in r.results.values_mut() {
            for e in [&mut t.setup_error, &mut t.verify_error].into_iter().chain(t.rounds.iter_mut().map(|x| &mut x.first_error)) {
                *e = e.as_deref().map(scrub);
            }
        }
        file.scenarios.push(r);
        // Rewritten after every scenario, so an interrupted run keeps what it measured.
        write_results(&file, &args.out_dir, &name)?;
    }
    file.finished = Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    let (json, md) = write_results(&file, &args.out_dir, &name)?;
    println!("{}", report::markdown(&file));
    eprintln!("wrote {} and {}", json.display(), md.display());
    let failed = file.scenarios.iter().any(|s| s.results.values().any(|r| r.p50_ms.is_none() || r.verified == Some(false)));
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    match Cli::parse().command {
        Command::Run(args) => run(*args).await,
        Command::List { filter } => {
            println!("{:26} {:40} {:>4} {:>5} {:>9} {:>9}  result", "id", "SpaceFS name", "conc", "ops", "SpaceFS", "bare");
            for (_, s) in select(&filter) {
                println!(
                    "{:26} {:40} {:>4} {:>5} {:>9} {:>9}  {}",
                    s.id,
                    s.name,
                    s.concurrency,
                    s.ops,
                    report::ms(s.spacefs.layer),
                    report::ms(s.spacefs.bare),
                    report::result(s.spacefs.speedup())
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Purge { bucket, prefix, yes } => {
            if prefix.is_empty() || !prefix.ends_with('/') {
                bail!("the prefix must be a folder: non-empty and ending in /");
            }
            let t = bucket.target("", 64, Checksums::SdkDefault)?;
            let place = t.place_at(&prefix);
            let keys = t.list(&place, "").await?;
            println!("{} objects under {prefix}", keys.len());
            if yes && !keys.is_empty() {
                t.delete_many(&place, &keys).await?;
                println!("deleted them");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Delay { listen, upstream, one_way_ms, bandwidth } => {
            let link = delay::Link { delay: std::time::Duration::from_secs_f64(one_way_ms / 1000.0), bandwidth: bandwidth.unwrap_or_default() };
            delay::serve(listen, upstream, link).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Report { file, like_spacefs } => {
            let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let mut run: RunFile = serde_json::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
            run.settings.like_spacefs |= like_spacefs;
            print!("{}", report::markdown(&run));
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn secret_environment_values_are_hidden_in_help() {
        let command = Cli::command();
        for (subcommand, secrets) in [
            ("run", &[("voidfs_secret_access_key", "VOIDFS_SECRET_ACCESS_KEY"), ("bare_secret_access_key", "VOIDFS_S3_SECRET_ACCESS_KEY")][..]),
            ("purge", &[("bare_secret_access_key", "VOIDFS_S3_SECRET_ACCESS_KEY")][..]),
        ] {
            let command = command.find_subcommand(subcommand).unwrap_or_else(|| panic!("missing subcommand {subcommand}"));
            for &(id, env) in secrets {
                let arg = command.get_arguments().find(|arg| arg.get_id() == id).unwrap_or_else(|| panic!("missing secret argument {subcommand} {id}"));
                assert_eq!(arg.get_env(), Some(std::ffi::OsStr::new(env)), "environment for {subcommand} {id}");
                assert!(arg.is_hide_env_values_set(), "{subcommand} {id} must hide its environment value in help");
            }
        }
    }
}
