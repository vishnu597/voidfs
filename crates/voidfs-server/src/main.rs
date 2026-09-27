// SPDX-License-Identifier: Apache-2.0
//! `voidfs-server`: serves voidfs drives over the S3 protocol from a pool in your bucket.

mod pool;
mod s3;
mod sigv4;
mod store;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::Parser;
use rand::RngExt;

use crate::sigv4::{KeyInfo, Keys, Scope};
use crate::store::Store;

/// Serve voidfs drives over S3 from a pool in your own storage.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Where the pool lives: `memory`, `fs:<directory>`, or `s3:<bucket>[/<prefix>]`.
    #[arg(long, env = "VOIDFS_STORE", default_value = "fs:./voidfs-data")]
    store: String,
    /// Address to listen on.
    #[arg(long, env = "VOIDFS_LISTEN", default_value = "127.0.0.1:9000")]
    listen: SocketAddr,
    /// Endpoint of an S3-compatible service, for R2, MinIO and others.
    #[arg(long, env = "VOIDFS_S3_ENDPOINT")]
    s3_endpoint: Option<String>,
    /// Access key id for the bucket. Without it, the usual AWS credential sources are used.
    #[arg(long, env = "VOIDFS_S3_ACCESS_KEY_ID")]
    s3_access_key_id: Option<String>,
    /// Secret for the bucket's access key.
    #[arg(long, env = "VOIDFS_S3_SECRET_ACCESS_KEY", hide_env_values = true)]
    s3_secret_access_key: Option<String>,
    /// Region of the bucket (`auto` for R2).
    #[arg(long, env = "VOIDFS_S3_REGION", default_value = "us-east-1")]
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
    #[arg(long, env = "VOIDFS_CACHE_MIB", default_value_t = 512)]
    cache_mib: u64,
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
    let app = Arc::new(s3::App { pool, keys });
    let listener = tokio::net::TcpListener::bind(args.listen).await.with_context(|| format!("listening on {}", args.listen))?;
    tracing::info!("serving on http://{}", args.listen);
    axum::serve(listener, s3::router(app)).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
