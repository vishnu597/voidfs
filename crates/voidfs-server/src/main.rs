// SPDX-License-Identifier: Apache-2.0
//! `voidfs-server`: serves voidfs drives over the S3 protocol from a pool in your bucket.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use rand::RngExt;
use voidfs_core::model::CommitGuard;
use voidfs_server::sigv4::{KeyInfo, Keys, Scope};
use voidfs_server::store::Store;
use voidfs_server::{admin, clock, gc, metrics, pool, probe, s3};

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
    /// A feature of the on-bucket format that a pool this server creates uses from the start
    /// (repeatable): `inline-data` holds files of up to 4 KiB in the log, so that a small write
    /// takes one request to the bucket instead of two; `multi-object-versions` gives a version to
    /// every object it changes, so a folder restore is in each restored file's history. Servers
    /// and readers that don't implement a feature refuse a pool that has it. An existing pool keeps its features; add one with
    /// `voidfs-server pool enable`.
    #[arg(long = "new-pool-feature", env = "VOIDFS_NEW_POOL_FEATURES", value_delimiter = ',', global = true)]
    new_pool_features: Vec<String>,
    /// Admin access key id clients sign with. Generated and printed if not given.
    #[arg(long, env = "VOIDFS_ACCESS_KEY_ID")]
    access_key_id: Option<String>,
    /// Secret for the admin access key.
    #[arg(long, env = "VOIDFS_SECRET_ACCESS_KEY", hide_env_values = true)]
    secret_access_key: Option<String>,
    /// Extra keys as `id:secret:scope`, scope one of read, write, admin (repeatable).
    #[arg(long = "key", env = "VOIDFS_KEYS", hide_env_values = true, value_delimiter = ',')]
    keys: Vec<String>,
    /// Memory for the shard cache, in MiB.
    #[arg(long, env = "VOIDFS_CACHE_MIB", default_value_t = 512, global = true)]
    cache_mib: u64,
    /// Collect garbage every this often (for example `1h`), with a 24-hour grace period. Off by
    /// default; `voidfs-server gc` does one step on demand.
    #[arg(long, env = "VOIDFS_GC_INTERVAL", value_parser = clock::parse_duration)]
    gc_interval: Option<Duration>,
    /// Direct uploads (protocol §4.11): clients PUT the shards a drive lacks straight to the
    /// bucket, with URLs the server presigns. `on` offers them where the bucket refuses bytes that
    /// don't match a URL's checksum, which is checked at start (with PUTs of a small valid
    /// shard); elsewhere, and with `off`, their requests answer 501 and clients put as usual.
    #[arg(long, env = "VOIDFS_DIRECT_UPLOADS", value_enum, default_value = "on")]
    direct_uploads: Switch,
    /// Storage credentials (protocol §5.5): short-lived, read-only credentials to a drive's
    /// storage, so that a mount reads shards and metadata straight from the bucket. `on` offers
    /// them where the service mints credentials that read only what a drive's reader needs,
    /// checked at start (MinIO, AWS with --storage-credentials-role, R2 with --r2-api-token); elsewhere, and
    /// with `off`, their requests answer 501 and clients read through the server.
    #[arg(long, env = "VOIDFS_STORAGE_CREDENTIALS", value_enum, default_value = "on")]
    storage_credentials: Switch,
    /// The IAM role whose credentials AWS STS narrows to a drive: it must allow reading the pool
    /// (s3:GetObject and s3:ListBucket), and trust the bucket's credentials to assume it.
    #[arg(long, env = "VOIDFS_STORAGE_CREDENTIALS_ROLE", global = true)]
    storage_credentials_role: Option<String>,
    /// The STS endpoint that mints storage credentials. Default: AWS's regional one for an AWS
    /// bucket, the bucket's endpoint for MinIO, or Cloudflare's API base for R2
    /// (https://api.cloudflare.com/client/v4).
    #[arg(long, env = "VOIDFS_STS_ENDPOINT", global = true)]
    sts_endpoint: Option<String>,
    /// Account-level R2 API token with Workers R2 Storage write access, used to mint read-only
    /// temporary credentials. The bucket's static access key id is their parent.
    #[arg(long, env = "VOIDFS_R2_API_TOKEN", hide_env_values = true, global = true)]
    r2_api_token: Option<String>,
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
    /// Change the pool in the store.
    #[command(subcommand)]
    Pool(PoolCommand),
    /// Check what the bucket supports, and report whether a server could write the pool there.
    ///
    /// Checks create-if-absent writes, lifecycle rules, versioning, object lock, CORS, presigned
    /// URLs and what presigned PUTs bind, modification times and the bucket's clock. It stores
    /// nothing a pool must not hold: creating voidfs.json again must be refused by the bucket, and
    /// the presigned PUTs send a small valid shard, which garbage collection removes. Exits with
    /// status 1 if a server started with the same options would refuse to open the pool.
    Probe,
}

#[derive(Subcommand)]
enum PoolCommand {
    /// Add a feature of the on-bucket format to the pool (format §3.1).
    ///
    /// Servers and readers that don't implement the feature refuse the pool from then on, and
    /// servers use it once they restart, since they read the pool's descriptor when they start.
    /// So run this only once every server that writes the pool implements it, then restart them.
    /// A feature cannot be removed.
    Enable {
        /// The feature: `inline-data` holds files of up to 4 KiB in the log (RFC 0003);
        /// `multi-object-versions` gives a version to every object it changes (RFC 0004).
        feature: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Switch {
    On,
    Off,
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
/// honours the commit guard. Returns the bucket's own requests too, for a bucket.
async fn open_pool(args: &Args) -> anyhow::Result<(Arc<pool::Pool>, Option<probe::Bucket>)> {
    let (store, bucket) = open_store(args)?;
    if let Some(b) = &bucket {
        probe::check_bucket(b).await.context("checking the bucket")?;
    }
    // An empty value, as from VOIDFS_NEW_POOL_FEATURES= in the environment, names none.
    let features: Vec<String> = args.new_pool_features.iter().filter(|f| !f.is_empty()).cloned().collect();
    let pool = pool::Pool::open_creating(store, args.cache_mib * 1024 * 1024, clock::Clock::System, args.commit_guard.into(), &features).await.context("opening the pool")?;
    Ok((pool, bucket))
}

/// Direct uploads, if `--direct-uploads on` and the store presigns PUTs that bind a shard's
/// checksum (protocol §4.11, §9).
async fn direct_uploads(args: &Args, bucket: Option<probe::Bucket>) -> Option<Arc<voidfs_server::direct::Direct>> {
    if args.direct_uploads == Switch::Off {
        tracing::info!("direct uploads are off (--direct-uploads off)");
        return None;
    }
    let Some(bucket) = bucket else {
        tracing::info!("direct uploads are not offered: the store is not a bucket that can presign uploads");
        return None;
    };
    let http = bucket.http().clone();
    let (checks, direct) = probe::offer(Box::new(bucket), &http).await;
    match &direct {
        Some(_) => tracing::info!("presigned PUTs: {checks}"),
        None => tracing::warn!("presigned PUTs: {checks}"),
    }
    direct
}

fn mint_options(args: &Args) -> voidfs_server::credentials::MintOptions {
    voidfs_server::credentials::MintOptions { role: args.storage_credentials_role.clone(), endpoint: args.sts_endpoint.clone(), r2_api_token: args.r2_api_token.clone() }
}

/// Storage credentials, if `--storage-credentials on` and the service mints credentials
/// scoped to a drive's reader (protocol §5.5, §9).
async fn storage_credentials(args: &Args, bucket: Option<&probe::Bucket>) -> Option<Arc<voidfs_server::credentials::Credentials>> {
    if args.storage_credentials == Switch::Off {
        tracing::info!("storage credentials are off (--storage-credentials off)");
        return None;
    }
    let Some(bucket) = bucket else {
        tracing::info!("storage credentials are not offered: the store is not a bucket");
        return None;
    };
    let mint = match bucket.mint(&mint_options(args)) {
        Ok(m) => m,
        Err(why) => {
            tracing::info!("storage credentials are not offered: {why}");
            return None;
        }
    };
    let (checks, offered) = voidfs_server::credentials::offer(mint, bucket.location(), bucket.http()).await;
    // A bucket without a mint is common; one that mints credentials reaching too far is not.
    match (&offered, &checks.refused) {
        (None, None) => tracing::warn!("storage credentials: {checks}"),
        _ => tracing::info!("storage credentials: {checks}"),
    }
    offered
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let args = Args::parse();
    match &args.command {
        Some(Command::Gc(g)) => {
            let opts = gc::Options { grace: g.grace, offline: g.offline, dry_run: g.dry_run, expire_deleted_drives: g.expire_deleted_drives.0, abort_uploads: g.abort_uploads.0 };
            let (pool, _) = open_pool(&args).await?;
            let report = gc::step(&pool, &opts).await?;
            println!("{report}");
            return Ok(());
        }
        Some(Command::Pool(PoolCommand::Enable { feature })) => {
            let (store, _) = open_store(&args)?;
            if pool::enable_feature(&store, feature).await? {
                println!("The pool lists {feature} now. Restart its servers to use it: they read voidfs.json when they start.");
            } else {
                println!("The pool already lists {feature}.");
            }
            return Ok(());
        }
        Some(Command::Probe) => {
            let (store, bucket) = open_store(&args)?;
            let report = probe::report(&store, bucket.as_ref(), args.commit_guard.into(), &mint_options(&args)).await?;
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

    // Listening from here on, so that a SIGUSR1 sent while the pool opens does not stop the
    // process, as it does by default.
    #[cfg(unix)]
    let usr1 = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()).map_err(|e| tracing::warn!("SIGUSR1 will not drop the caches: {e}")).ok();

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

    let (pool, bucket) = open_pool(&args).await?;
    tracing::info!(
        "pool {} open with {} drives, commit guard {}, features {:?}",
        pool.desc.pool_id,
        pool.list_drives().len(),
        probe::guard_name(pool.desc.commit_guard),
        pool.desc.features.incompatible
    );
    #[cfg(unix)]
    if let Some(usr1) = usr1 {
        tokio::spawn(drop_caches_on(usr1, pool.clone()));
    }
    if let Some(every) = args.gc_interval {
        tracing::info!("collecting garbage every {}s", every.as_secs());
        tokio::spawn(gc::run_periodically(pool.clone(), gc::Options::default(), every));
    }
    for d in &args.virtual_host_domains {
        tracing::info!("serving virtual-host requests to *.{d}");
    }
    let credentials = storage_credentials(&args, bucket.as_ref()).await;
    let direct = direct_uploads(&args, bucket).await;
    let app = Arc::new(s3::App {
        pool: pool.clone(),
        keys,
        domains: s3::Domains::new(args.virtual_host_domains),
        metrics: metrics::S3Metrics::new(),
        uploads: Default::default(),
        read_ahead: Default::default(),
        direct,
        credentials,
    });
    let listener = tokio::net::TcpListener::bind(args.listen).await.with_context(|| format!("listening on {}", args.listen))?;
    tracing::info!("serving on http://{}", args.listen);
    admin.serving(app.clone());
    axum::serve(listener, s3::router(app.clone())).with_graceful_shutdown(async move {
        shutdown_signal().await;
        tracing::info!("stopping: finishing the requests in progress");
        admin.stopping();
    })
    .await?;
    // Completed uploads answer before their staging records are deleted (format §11).
    app.uploads.finish(&pool).await;
    // One cut short would be harmless (format §8.1), but the next start replays less log.
    pool.finish_checkpoints().await;
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

/// Empties the shard and page caches whenever `signals` fires: SIGUSR1, which operators and the
/// benchmark's `--cold` send to measure cold reads. It is not part of the S3 surface.
#[cfg(unix)]
async fn drop_caches_on(mut signals: tokio::signal::unix::Signal, pool: Arc<pool::Pool>) {
    while signals.recv().await.is_some() {
        let d = pool.drop_caches();
        tracing::info!("SIGUSR1: dropped the caches: {} shards ({} MiB), {} pages ({} KiB)", d.shards, d.shard_bytes >> 20, d.pages, d.page_bytes >> 10);
    }
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
    fn secret_environment_values_are_hidden_in_help() {
        let command = Args::command();
        for (id, env) in [
            ("s3_secret_access_key", "VOIDFS_S3_SECRET_ACCESS_KEY"),
            ("secret_access_key", "VOIDFS_SECRET_ACCESS_KEY"),
            ("keys", "VOIDFS_KEYS"),
            ("r2_api_token", "VOIDFS_R2_API_TOKEN"),
        ] {
            let arg = command.get_arguments().find(|arg| arg.get_id() == id).unwrap_or_else(|| panic!("missing secret argument {id}"));
            assert_eq!(arg.get_env(), Some(std::ffi::OsStr::new(env)), "environment for {id}");
            assert!(arg.is_hide_env_values_set(), "{id} must hide its environment value in help");
        }
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
    fn the_r2_token_is_global_and_hidden_in_help() {
        use clap::CommandFactory;
        let a = Args::try_parse_from(["voidfs-server", "probe", "--r2-api-token", "private-api-token"]).unwrap();
        assert!(a.r2_api_token.as_deref() == Some("private-api-token"));
        assert!(mint_options(&a).r2_api_token.as_deref() == Some("private-api-token"));
        let command = Args::command();
        let token = command.get_arguments().find(|a| a.get_id() == "r2_api_token").unwrap();
        assert!(token.is_hide_env_values_set());
        assert!(token.is_global_set());
    }

    #[test]
    fn pool_features_parse() {
        assert!(Args::try_parse_from(["voidfs-server"]).unwrap().new_pool_features.is_empty());
        let a = Args::try_parse_from(["voidfs-server", "--new-pool-feature", "inline-data"]).unwrap();
        assert_eq!(a.new_pool_features, ["inline-data"]);
        let a = Args::try_parse_from(["voidfs-server", "pool", "enable", "inline-data", "--store", "fs:/tmp/p"]).unwrap();
        assert!(matches!(a.command, Some(Command::Pool(PoolCommand::Enable { ref feature })) if feature == "inline-data"));
        assert_eq!(a.store, "fs:/tmp/p");
        assert!(Args::try_parse_from(["voidfs-server", "pool", "enable"]).is_err());
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

    /// SIGUSR1, sent here to this test process, empties the caches of the pool it serves.
    #[cfg(unix)]
    #[tokio::test]
    async fn sigusr1_drops_the_caches() {
        use voidfs_server::metrics::{encode, sample};
        let pool = pool::Pool::open(Store::memory().unwrap(), 1 << 20).await.unwrap();
        let usr1 = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()).unwrap();
        tokio::spawn(drop_caches_on(usr1, pool.clone()));
        let data = bytes::Bytes::from_static(b"cached");
        pool.write_shards(&[voidfs_core::chunk::Shard { hash: voidfs_core::ids::ShardHash::of(&data), bytes: data }]).await.unwrap();
        let entries = |text: &str| sample(text, r#"voidfs_cache_entries{cache="shard"}"#);
        assert_eq!(entries(&encode(pool.gather_metrics())), Some(1.0));
        assert!(std::process::Command::new("kill").args(["-USR1", &std::process::id().to_string()]).status().unwrap().success());
        let mut text = String::new();
        for _ in 0..200 {
            text = encode(pool.gather_metrics());
            if sample(&text, "voidfs_cache_drops_total") == Some(1.0) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(sample(&text, "voidfs_cache_drops_total"), Some(1.0), "dropped once");
        assert_eq!(entries(&text), Some(0.0));
    }

    #[test]
    fn virtual_host_domains_parse() {
        assert!(Args::try_parse_from(["voidfs-server"]).unwrap().virtual_host_domains.is_empty());
        let a = Args::try_parse_from(["voidfs-server", "--virtual-host-domain", "S3.Example.com.", "--virtual-host-domain", "localhost"]).unwrap();
        assert_eq!(a.virtual_host_domains, ["s3.example.com", "localhost"]);
        assert!(Args::try_parse_from(["voidfs-server", "--virtual-host-domain", "*.example.com"]).is_err());
    }
}
