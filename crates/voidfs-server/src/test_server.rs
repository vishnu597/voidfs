// SPDX-License-Identifier: Apache-2.0
//! A server over a memory pool, in this process, on a free port of 127.0.0.1: what the tests of the
//! SDK, the CLI and the client run against. It serves the S3 port as the binary does, with the
//! keys it is given.

use std::net::SocketAddr;
use std::sync::Arc;

use crate::pool::Pool;
use crate::s3::{App, Domains};
use crate::sigv4::{KeyInfo, Keys, Scope};
use crate::store::Store;

/// The admin key every test server accepts.
pub const ADMIN_KEY_ID: &str = "VFTESTADMINKEY234567";
pub const ADMIN_SECRET: &str = "testsecrettestsecrettestsecrettestsecret";

pub struct TestServer {
    /// `http://127.0.0.1:<port>`.
    pub endpoint: String,
    pub addr: SocketAddr,
    pub pool: Arc<Pool>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// A server that accepts [`ADMIN_KEY_ID`].
    pub async fn start() -> anyhow::Result<TestServer> {
        TestServer::with_keys(Vec::new()).await
    }

    /// A server that accepts [`ADMIN_KEY_ID`] and `keys`.
    pub async fn with_keys(keys: Vec<KeyInfo>) -> anyhow::Result<TestServer> {
        let mut all = Keys::default();
        all.insert(KeyInfo { id: ADMIN_KEY_ID.into(), secret: ADMIN_SECRET.into(), scope: Scope::Admin, drives: None });
        for k in keys {
            all.insert(k);
        }
        let pool = Pool::open(Store::memory()?, 64 << 20).await?;
        let app = Arc::new(App { pool: pool.clone(), keys: all, domains: Domains::new(Vec::new()), metrics: crate::metrics::S3Metrics::new(), uploads: Default::default(), read_ahead: Default::default() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, crate::s3::router(app)).await;
        });
        Ok(TestServer { endpoint: format!("http://{addr}"), addr, pool, task })
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
