// SPDX-License-Identifier: Apache-2.0
//! `voidfs-server`: serves voidfs drives over the S3 protocol from a pool in your bucket.

mod clock;
mod gc;
mod pool;
mod s3;
mod sigv4;
mod store;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use rand::RngExt;

use crate::sigv4::{KeyInfo, Keys, Scope};
use crate::store::Store;

/// Serve voidfs drives over S3 from a pool in your own storage.
#[derive(Parser)]
#[command(version)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Where the pool lives: `memory`, `fs:<directory>`, or `s3:<bucket>[/<prefix>]`.
    #[arg(long, env = "VOIDFS_STORE", default_value = "fs:./voidfs-data", global = true)]
    store: String,
    /// Address to listen on.
    #[arg(long, env = "VOIDFS_LISTEN", default_value = "127.0.0.1:9000")]
    listen: SocketAddr,
    /// Endpoint of an S3-compatible service, for R2, MinIO and others.
    #[arg(long, env = "VOIDFS_S3_ENDPOINT", global = true)]
    s3_endpoint: Option<String>,
    /// Access key id for the bucket. Without it, the usual AWS credential sources are used.
    #[arg(long, env = "VOIDFS_S3_ACCESS_KEY_ID", global = true)]
    s3_access_key_id: Option<String>,
    /// Secret for the bucket's access key.
    #[arg(long, env = "VOIDFS_S3_SECRET_ACCESS_KEY", hide_env_values = true, global = true)]
    s3_secret_access_key: Option<String>,
    /// Region of the bucket (`auto` for R2).
    #[arg(long, env = "VOIDFS_S3_REGION", default_value = "us-east-1", global = true)]
    s3_region: String,
    /// Admin access key id clients sign with. Generated and printed if not given.
    #[arg(long, env = "VOIDFS_ACCESS_KEY_ID")]
    access_key_id: Option<String>,
    /// Secret for the admin access key.
    #[arg(long, env = "VOIDFS_SECRET_ACCESS_KEY", hide_env_values = true)]
    secret_access_key: Option<String>,
    /// Extra keys as `id:secret:scope`, scope one of read, write, admin (repeatable).
    #[arg(long = "key", env = "VOIDFS_KEYS", value_delimiter = ',')]
    keys: Vec<String>,
    /// Memory for the shard cache, in MiB.
    #[arg(long, env = "VOIDFS_CACHE_MIB", default_value_t = 512, global = true)]
    cache_mib: u64,
    /// Collect garbage every this often (for example `1h`), with a 24-hour grace period. Off by
    /// default; `voidfs-server gc` does one step on demand.
    #[arg(long, env = "VOIDFS_GC_INTERVAL", value_parser = clock::parse_duration)]
    gc_interval: Option<Duration>,
}

#[derive(Subcommand)]
enum Command {
    /// Collect garbage: delete shards and pages that nothing references (format §12).
    ///
    /// Each run does whichever phase is due. Phase 1 proposes unreferenced objects; phase 2,
    /// at least the grace period later, deletes those still unreferenced. Drives soft-deleted
    /// longer than their window are hard-deleted, and stale multipart uploads aborted, first.
    Gc(GcArgs),
}

#[derive(clap::Args)]
struct GcArgs {
    /// Report what would be collected and expired, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// How long an unreferenced object must stay untouched before it is deleted.
    #[arg(long, default_value = "24h", value_parser = clock::parse_duration)]
    grace: Duration,
    /// No server is writing to the pool. Allows a grace under 24 hours; with `--grace 0`, one
    /// step does a whole run.
    #[arg(long)]
    offline: bool,
    /// Hard-delete drives soft-deleted longer than this, or `never`.
    #[arg(long, default_value = "30d", value_parser = Limit::parse)]
    expire_deleted_drives: Limit,
    /// Abort multipart uploads open longer than this, or `never`.
    #[arg(long, default_value = "7d", value_parser = Limit::parse)]
    abort_uploads: Limit,
}

/// A duration, or `never`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Limit(Option<Duration>);

impl Limit {
    fn parse(s: &str) -> Result<Limit, String> {
        if s == "never" { Ok(Limit(None)) } else { clock::parse_duration(s).map(|d| Limit(Some(d))) }
    }
}

fn random(alphabet: &[u8], n: usize) -> String {
    let mut rng = rand::rng();
    (0..n).map(|_| alphabet[rng.random_range(0..alphabet.len())] as char).collect()
}

fn open_store(args: &Args) -> anyhow::Result<Store> {
    let spec = args.store.as_str();
    if spec == "memory" {
        return Store::memory();
    }
    if let Some(dir) = spec.strip_prefix("fs:") {
        return Store::local(dir);
    }
    if let Some(rest) = spec.strip_prefix("s3:") {
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        let credentials = match (&args.s3_access_key_id, &args.s3_secret_access_key) {
            (Some(id), Some(secret)) => Some((id.as_str(), secret.as_str())),
            (None, None) => None,
            _ => bail!("give both --s3-access-key-id and --s3-secret-access-key, or neither"),
        };
        return Store::s3(bucket, &format!("/{prefix}"), args.s3_endpoint.as_deref(), &args.s3_region, credentials);
    }
    bail!("--store must be memory, fs:<directory> or s3:<bucket>[/<prefix>]")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let args = Args::parse();
    if let Some(Command::Gc(g)) = &args.command {
        let opts = gc::Options { grace: g.grace, offline: g.offline, dry_run: g.dry_run, expire_deleted_drives: g.expire_deleted_drives.0, abort_uploads: g.abort_uploads.0 };
        let pool = pool::Pool::open(open_store(&args)?, args.cache_mib * 1024 * 1024).await.context("opening the pool")?;
        let report = gc::step(&pool, &opts).await?;
        println!("{report}");
        return Ok(());
    }

    let mut keys = Keys::default();
    let (id, secret) = match (&args.access_key_id, &args.secret_access_key) {
        (Some(i), Some(s)) => (i.clone(), s.clone()),
        (None, None) => {
            let id = format!("VF{}", random(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567", 18));
            let secret = random(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/", 40);
            eprintln!("No admin key was given, so one was generated for this run:\n  access key id:     {id}\n  secret access key: {secret}");
            (id, secret)
        }
        _ => bail!("give both --access-key-id and --secret-access-key, or neither"),
    };
    keys.insert(KeyInfo { id, secret, scope: Scope::Admin, drives: None });
    for k in &args.keys {
        let mut parts = k.splitn(3, ':');
        let (Some(id), Some(secret), Some(scope)) = (parts.next(), parts.next(), parts.next()) else {
            bail!("--key must be id:secret:scope");
        };
        keys.insert(KeyInfo { id: id.into(), secret: secret.into(), scope: scope.parse().map_err(anyhow::Error::msg)?, drives: None });
    }

    let store = open_store(&args)?;
    let pool = pool::Pool::open(store, args.cache_mib * 1024 * 1024).await.context("opening the pool")?;
    tracing::info!("pool {} open with {} drives", pool.desc.pool_id, pool.list_drives().len());
    if let Some(every) = args.gc_interval {
        tracing::info!("collecting garbage every {}s", every.as_secs());
        tokio::spawn(gc::run_periodically(pool.clone(), gc::Options::default(), every));
    }
    let app = Arc::new(s3::App { pool, keys });
    let listener = tokio::net::TcpListener::bind(args.listen).await.with_context(|| format!("listening on {}", args.listen))?;
    tracing::info!("serving on http://{}", args.listen);
    axum::serve(listener, s3::router(app)).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_consistent() {
        Args::command().debug_assert();
    }

    #[test]
    fn gc_options_parse() {
        let a = Args::try_parse_from(["voidfs-server", "gc", "--dry-run", "--store", "memory"]).unwrap();
        let Some(Command::Gc(g)) = a.command else { panic!("not gc") };
        assert!(g.dry_run && !g.offline);
        assert_eq!(g.grace, Duration::from_secs(86_400));
        assert_eq!(g.expire_deleted_drives, Limit(Some(Duration::from_secs(30 * 86_400))));
        assert_eq!(g.abort_uploads, Limit(Some(Duration::from_secs(7 * 86_400))));
        assert_eq!(a.store, "memory");
        let a = Args::try_parse_from(["voidfs-server", "gc", "--offline", "--grace", "0", "--expire-deleted-drives", "never", "--abort-uploads", "36h"]).unwrap();
        let Some(Command::Gc(g)) = a.command else { panic!("not gc") };
        assert!(g.offline);
        assert_eq!(g.grace, Duration::ZERO);
        assert_eq!(g.expire_deleted_drives, Limit(None));
        assert_eq!(g.abort_uploads, Limit(Some(Duration::from_secs(36 * 3600))));
        let a = Args::try_parse_from(["voidfs-server", "--gc-interval", "1h"]).unwrap();
        assert!(a.command.is_none());
        assert_eq!(a.gc_interval, Some(Duration::from_secs(3600)));
    }
}
