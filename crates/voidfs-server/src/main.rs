// SPDX-License-Identifier: Apache-2.0
//! `voidfs-server`: serves voidfs drives over the S3 protocol from a pool in your bucket.

mod admin;
mod clock;
mod gc;
mod metrics;
mod pool;
mod probe;
mod s3;
mod sigv4;
mod store;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use rand::RngExt;
use voidfs_core::model::CommitGuard;

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
    /// Address for the admin endpoints, /healthz, /readyz and /metrics (Prometheus). Off unless
    /// given. Nothing on it is authenticated: keep it on loopback or a private network.
    #[arg(long, env = "VOIDFS_ADMIN_LISTEN")]
    admin_listen: Option<SocketAddr>,
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
    /// How the pool stops two servers from writing the same commit (format §7). With
    /// `create-if-absent`, the bucket must honour conditional writes, which is checked at start.
    /// `external` is for buckets that don't: then at most one server, and at most one garbage
    /// collector, may write the pool at a time. A new pool keeps the guard it is created with,
    /// and an existing pool opens only with its own.
    #[arg(long, env = "VOIDFS_COMMIT_GUARD", value_enum, default_value = "create-if-absent", global = true)]
    commit_guard: Guard,
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
    /// Also serve `<drive>.<domain>/<key>` (virtual-host addressing) under this domain, which
    /// needs a wildcard DNS name `*.<domain>` pointing at the server (repeatable). Requests to
    /// the domain itself, or to any other host, stay path-style (`/<drive>/<key>`).
    #[arg(long = "virtual-host-domain", env = "VOIDFS_VIRTUAL_HOST_DOMAIN", value_delimiter = ',', value_parser = s3::parse_domain)]
    virtual_host_domains: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Collect garbage: delete shards and pages that nothing references (format §12).
    ///
    /// Each run does whichever phase is due. Phase 1 proposes unreferenced objects; phase 2,
    /// at least the grace period later, deletes those still unreferenced. Drives soft-deleted
    /// longer than their window are hard-deleted, and stale multipart uploads aborted, first.
    Gc(GcArgs),
    /// Check what the bucket supports, and report whether a server could write the pool there.
    ///
    /// Checks create-if-absent writes, lifecycle rules, versioning, object lock, CORS, presigned
    /// URLs, modification times and the bucket's clock. It stores nothing: the one write, which
    /// creates voidfs.json again, must be refused by the bucket. Exits with status 1 if a server
    /// started with the same options would refuse to open the pool.
    Probe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Guard {
    CreateIfAbsent,
    External,
}

impl From<Guard> for CommitGuard {
    fn from(g: Guard) -> CommitGuard {
        match g {
            Guard::CreateIfAbsent => CommitGuard::CreateIfAbsent,
            Guard::External => CommitGuard::External,
        }
    }
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

/// The store `--store` names, and for a bucket, requests for its configuration.
fn open_store(args: &Args) -> anyhow::Result<(Store, Option<probe::Bucket>)> {
    let spec = args.store.as_str();
    if spec == "memory" {
        return Ok((Store::memory()?, None));
    }
    if let Some(dir) = spec.strip_prefix("fs:") {
        return Ok((Store::local(dir)?, None));
    }
    if let Some(rest) = spec.strip_prefix("s3:") {
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        let credentials = match (&args.s3_access_key_id, &args.s3_secret_access_key) {
            (Some(id), Some(secret)) => Some((id.as_str(), secret.as_str())),
            (None, None) => None,
            _ => bail!("give both --s3-access-key-id and --s3-secret-access-key, or neither"),
        };
        let store = Store::s3(bucket, &format!("/{prefix}"), args.s3_endpoint.as_deref(), &args.s3_region, credentials)?;
        let api = probe::Bucket::new(bucket, prefix, args.s3_endpoint.as_deref(), &args.s3_region, credentials)?;
        return Ok((store, Some(api)));
    }
    bail!("--store must be memory, fs:<directory> or s3:<bucket>[/<prefix>]")
}

/// Opens the pool for writing, after checking that the bucket won't lose its objects and
/// honours the commit guard.
async fn open_pool(args: &Args) -> anyhow::Result<Arc<pool::Pool>> {
    let (store, bucket) = open_store(args)?;
    if let Some(b) = &bucket {
        probe::check_bucket(b).await.context("checking the bucket")?;
    }
    pool::Pool::open_as(store, args.cache_mib * 1024 * 1024, clock::Clock::System, args.commit_guard.into()).await.context("opening the pool")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let args = Args::parse();
    match &args.command {
        Some(Command::Gc(g)) => {
            let opts = gc::Options { grace: g.grace, offline: g.offline, dry_run: g.dry_run, expire_deleted_drives: g.expire_deleted_drives.0, abort_uploads: g.abort_uploads.0 };
            let pool = open_pool(&args).await?;
            let report = gc::step(&pool, &opts).await?;
            println!("{report}");
            return Ok(());
        }
        Some(Command::Probe) => {
            let (store, bucket) = open_store(&args)?;
            let report = probe::report(&store, bucket.as_ref(), args.commit_guard.into()).await?;
            println!("{report}");
            std::process::exit(if report.refusals.is_empty() { 0 } else { 1 });
        }
        None => {}
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

    // Before the pool opens, which can take a while: liveness answers meanwhile, and readiness
    // says why not.
    let admin = admin::Admin::new();
    if let Some(addr) = args.admin_listen {
        if same_port(addr, args.listen) {
            bail!("--admin-listen {addr} would share the S3 port, --listen {}", args.listen);
        }
        let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("listening on {addr} for the admin endpoints"))?;
        tracing::info!("admin endpoints on http://{addr}: /healthz, /readyz, /metrics (not authenticated)");
        let router = admin::router(admin.clone());
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router).await {
                tracing::error!("the admin listener stopped: {e}");
            }
        });
    }

    let pool = open_pool(&args).await?;
    tracing::info!("pool {} open with {} drives, commit guard {}", pool.desc.pool_id, pool.list_drives().len(), probe::guard_name(pool.desc.commit_guard));
    if let Some(every) = args.gc_interval {
        tracing::info!("collecting garbage every {}s", every.as_secs());
        tokio::spawn(gc::run_periodically(pool.clone(), gc::Options::default(), every));
    }
    for d in &args.virtual_host_domains {
        tracing::info!("serving virtual-host requests to *.{d}");
    }
    let app = Arc::new(s3::App { pool, keys, domains: s3::Domains::new(args.virtual_host_domains), metrics: metrics::S3Metrics::new() });
    let listener = tokio::net::TcpListener::bind(args.listen).await.with_context(|| format!("listening on {}", args.listen))?;
    tracing::info!("serving on http://{}", args.listen);
    admin.serving(app.clone());
    axum::serve(listener, s3::router(app)).with_graceful_shutdown(async move {
        shutdown_signal().await;
        tracing::info!("stopping: finishing the requests in progress");
        admin.stopping();
    })
    .await?;
    Ok(())
}

/// Resolves on Ctrl-C, or on SIGTERM, which `docker stop` and Kubernetes send.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                return;
            }
            Err(e) => tracing::warn!("SIGTERM will not stop the server gracefully: {e}"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// Whether listening on both would take the same port: the same address, or the same port where
/// either is every address.
fn same_port(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() != 0 && a.port() == b.port() && (a.ip() == b.ip() || a.ip().is_unspecified() || b.ip().is_unspecified())
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

    #[test]
    fn the_commit_guard_and_probe_parse() {
        let a = Args::try_parse_from(["voidfs-server"]).unwrap();
        assert_eq!(a.commit_guard, Guard::CreateIfAbsent);
        let a = Args::try_parse_from(["voidfs-server", "--commit-guard", "external"]).unwrap();
        assert_eq!(CommitGuard::from(a.commit_guard), CommitGuard::External);
        let a = Args::try_parse_from(["voidfs-server", "probe", "--store", "s3:b/pool", "--commit-guard", "external"]).unwrap();
        assert!(matches!(a.command, Some(Command::Probe)));
        assert_eq!(a.commit_guard, Guard::External);
        assert!(Args::try_parse_from(["voidfs-server", "--commit-guard", "none"]).is_err());
        let a = Args::try_parse_from(["voidfs-server", "gc", "--commit-guard", "external"]).unwrap();
        assert_eq!(a.commit_guard, Guard::External);
    }

    #[test]
    fn the_admin_listener_is_off_by_default_and_apart_from_s3() {
        let a = Args::try_parse_from(["voidfs-server"]).unwrap();
        assert_eq!(a.admin_listen, None);
        let a = Args::try_parse_from(["voidfs-server", "--admin-listen", "127.0.0.1:9001"]).unwrap();
        assert_eq!(a.admin_listen, Some("127.0.0.1:9001".parse().unwrap()));
        let addr = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(same_port(addr("127.0.0.1:9000"), addr("127.0.0.1:9000")));
        assert!(same_port(addr("0.0.0.0:9000"), addr("127.0.0.1:9000")));
        assert!(same_port(addr("127.0.0.1:9000"), addr("[::]:9000")));
        assert!(!same_port(addr("127.0.0.1:9001"), addr("127.0.0.1:9000")));
        assert!(!same_port(addr("127.0.0.1:9000"), addr("127.0.0.2:9000")));
        assert!(!same_port(addr("127.0.0.1:0"), addr("127.0.0.1:0")));
    }

    #[test]
    fn virtual_host_domains_parse() {
        assert!(Args::try_parse_from(["voidfs-server"]).unwrap().virtual_host_domains.is_empty());
        let a = Args::try_parse_from(["voidfs-server", "--virtual-host-domain", "S3.Example.com.", "--virtual-host-domain", "localhost"]).unwrap();
        assert_eq!(a.virtual_host_domains, ["s3.example.com", "localhost"]);
        assert!(Args::try_parse_from(["voidfs-server", "--virtual-host-domain", "*.example.com"]).is_err());
    }
}
