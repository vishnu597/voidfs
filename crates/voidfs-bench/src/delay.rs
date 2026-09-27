// SPDX-License-Identifier: Apache-2.0
//! A TCP relay that adds a fixed one-way delay in each direction, to emulate a bucket some
//! distance away on one machine.
//!
//! Bytes are forwarded in order, each chunk `delay` after it arrived, without limiting
//! bandwidth: a request and its response pay one extra round trip of `2 × delay`, as they would
//! across a network. Put it between both the harness's bare target and voidfs-server on one
//! side, and the S3 server on the other, and both targets see the same distant bucket.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;

/// Chunks queued per direction; with 256 KiB reads that bounds each direction at 64 MiB.
const QUEUE: usize = 256;
const READ: usize = 256 * 1024;

pub async fn serve(listen: SocketAddr, upstream: String, delay: Duration) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen).await.with_context(|| format!("listening on {listen}"))?;
    eprintln!("relaying {listen} to {upstream}, adding {} ms each way", delay.as_secs_f64() * 1000.0);
    loop {
        let (inbound, _) = listener.accept().await?;
        let upstream = upstream.clone();
        tokio::spawn(async move {
            let outbound = match TcpStream::connect(&upstream).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cannot reach {upstream}: {e}");
                    return;
                }
            };
            let _ = inbound.set_nodelay(true);
            let _ = outbound.set_nodelay(true);
            let (ri, wi) = inbound.into_split();
            let (ro, wo) = outbound.into_split();
            tokio::join!(pipe(ri, wo, delay), pipe(ro, wi, delay));
        });
    }
}

async fn pipe(mut from: tokio::net::tcp::OwnedReadHalf, mut to: tokio::net::tcp::OwnedWriteHalf, delay: Duration) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Instant, Bytes)>(QUEUE);
    let reader = async move {
        let mut buf = vec![0u8; READ];
        while let Ok(n) = from.read(&mut buf).await {
            if n == 0 || tx.send((Instant::now() + delay, Bytes::copy_from_slice(&buf[..n]))).await.is_err() {
                break;
            }
        }
    };
    let writer = async move {
        while let Some((at, chunk)) = rx.recv().await {
            tokio::time::sleep_until(at).await;
            if to.write_all(&chunk).await.is_err() {
                return;
            }
        }
        let _ = to.shutdown().await;
    };
    tokio::join!(reader, writer);
}
