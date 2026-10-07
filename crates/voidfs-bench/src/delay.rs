// SPDX-License-Identifier: Apache-2.0
//! A TCP relay that adds a fixed one-way delay in each direction, and optionally a bandwidth cap,
//! to emulate a bucket some distance away on one machine.
//!
//! Bytes are forwarded in order, each chunk `delay` after it arrived: a request and its response
//! pay one extra round trip of `2 × delay`, as they would across a network. Put it between both
//! the harness's bare target and voidfs-server on one side, and the S3 server on the other, and
//! both targets see the same distant bucket.
//!
//! Without a [`Bandwidth`], nothing limits the rate. With one, each chunk is also sent only once
//! a link at that rate would have carried it, after the chunks before it: each connection has
//! its own link in each direction, and every connection shares one more of the total rate in
//! each direction. S3 limits a single stream well below what a client machine can take in all,
//! which is what the two levels model.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;

/// Chunks queued per direction; with 256 KiB reads that bounds each direction at 64 MiB.
const QUEUE: usize = 256;
const READ: usize = 256 * 1024;
/// How far ahead of its rate the shared link may run. It takes whole chunks in the order
/// connections ask, so without this, a small chunk would wait for other connections' large
/// ones even while together they ask for less than the rate.
const BURST: Duration = Duration::from_millis(5);

/// Rates in bytes per second; `None` is unlimited.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bandwidth {
    /// Each connection, from the S3 server to its client: downloads.
    pub down: Option<f64>,
    /// Each connection, from the client to the S3 server: uploads.
    pub up: Option<f64>,
    /// All connections together, in each direction.
    pub total: Option<f64>,
}

impl Bandwidth {
    /// Fitted to the bare bucket's figures in SpaceFS's run, an n2-standard-8 in GCP us-east4
    /// reading and writing S3 in us-east-1 (bench/results/cold-reads/README.md): get and stream
    /// get for downloads, put 32 and 64 MiB for uploads, the multipart puts for the total, which
    /// only they reach. That the total holds for downloads too is an assumption: no bare row of
    /// theirs downloads more than eight streams at once.
    pub const S3: Bandwidth = Bandwidth { down: Some(95e6), up: Some(68e6), total: Some(1_000e6) };

    /// `s3`, or `DOWN/UP/TOTAL` in MB/s (10⁶ bytes), each a number or `-` for no limit.
    pub fn parse(s: &str) -> Result<Bandwidth, String> {
        if s == "s3" {
            return Ok(Bandwidth::S3);
        }
        let rate = |v: &str| -> Result<Option<f64>, String> {
            if v == "-" {
                return Ok(None);
            }
            match v.parse::<f64>() {
                Ok(r) if r > 0.0 && r.is_finite() => Ok(Some(r * 1e6)),
                _ => Err(format!("{v:?} is not a rate in MB/s, or -")),
            }
        };
        match s.split('/').collect::<Vec<_>>()[..] {
            [down, up, total] => Ok(Bandwidth { down: rate(down)?, up: rate(up)?, total: rate(total)? }),
            _ => Err("the bandwidth is s3, or DOWN/UP/TOTAL in MB/s (- for no limit)".into()),
        }
    }

    pub fn describe(&self) -> String {
        let r = |v: Option<f64>| v.map(|b| format!("{} MB/s", b / 1e6)).unwrap_or_else(|| "unlimited".into());
        format!("{} down and {} up per connection, {} in all each way", r(self.down), r(self.up), r(self.total))
    }
}

/// What the relay does to the bytes it carries.
#[derive(Clone, Copy, Debug, Default)]
pub struct Link {
    /// Added in each direction.
    pub delay: Duration,
    pub bandwidth: Bandwidth,
}

pub async fn serve(listen: SocketAddr, upstream: String, link: Link) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen).await.with_context(|| format!("listening on {listen}"))?;
    let rate = if link.bandwidth == Bandwidth::default() { "no bandwidth limit".into() } else { link.bandwidth.describe() };
    eprintln!("relaying {listen} to {upstream}, adding {} ms each way, {rate}", link.delay.as_secs_f64() * 1000.0);
    relay(listener, upstream, link).await
}

/// One link shared by every connection in one direction: when it is next free.
type Shared = Arc<Mutex<Instant>>;

pub async fn relay(listener: TcpListener, upstream: String, link: Link) -> anyhow::Result<()> {
    let total = |rate: Option<f64>| rate.map(|r| (r, Shared::new(Mutex::new(Instant::now()))));
    let (up_total, down_total) = (total(link.bandwidth.total), total(link.bandwidth.total));
    loop {
        let (inbound, _) = listener.accept().await?;
        let upstream = upstream.clone();
        let up = Pace::new(link.delay, link.bandwidth.up, up_total.clone());
        let down = Pace::new(link.delay, link.bandwidth.down, down_total.clone());
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
            tokio::join!(pipe(ri, wo, up), pipe(ro, wi, down));
        });
    }
}

/// When each chunk of one direction of one connection is sent.
struct Pace {
    delay: Duration,
    /// This connection's own rate.
    rate: Option<f64>,
    /// The rate and link every connection shares in this direction.
    total: Option<(f64, Shared)>,
    /// When the chunk before was sent.
    last: Instant,
}

impl Pace {
    fn new(delay: Duration, rate: Option<f64>, total: Option<(f64, Shared)>) -> Pace {
        Pace { delay, rate, total, last: Instant::now() }
    }

    /// When a chunk of `len` bytes that arrived at `arrived` is to be sent: `delay` after it
    /// arrived, and once both links have carried it after what they carried before. The time
    /// is reckoned from when the links were free, not from when the timer fired, so that a late
    /// wake-up does not slow a busy connection down.
    fn send_at(&mut self, arrived: Instant, len: usize) -> Instant {
        let start = (arrived + self.delay).max(self.last);
        let mut done = start;
        if let Some(rate) = self.rate {
            done = start + Duration::from_secs_f64(len as f64 / rate);
        }
        if let Some((rate, shared)) = &self.total {
            let mut free = shared.lock().unwrap();
            *free = (*free).max(start) + Duration::from_secs_f64(len as f64 / rate);
            done = done.max(*free - BURST);
        }
        self.last = done;
        done
    }
}

async fn pipe(mut from: tokio::net::tcp::OwnedReadHalf, mut to: tokio::net::tcp::OwnedWriteHalf, mut pace: Pace) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Instant, Bytes)>(QUEUE);
    let reader = async move {
        let mut buf = vec![0u8; READ];
        while let Ok(n) = from.read(&mut buf).await {
            if n == 0 || tx.send((Instant::now(), Bytes::copy_from_slice(&buf[..n]))).await.is_err() {
                break;
            }
        }
    };
    let writer = async move {
        while let Some((arrived, chunk)) = rx.recv().await {
            tokio::time::sleep_until(pace.send_at(arrived, chunk.len())).await;
            if to.write_all(&chunk).await.is_err() {
                return;
            }
        }
        let _ = to.shutdown().await;
    };
    tokio::join!(reader, writer);
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    const MB: usize = 1_000_000;

    /// Each timing test measures this many runs, and they run one at a time, so that a loaded
    /// machine only makes some runs slower.
    const RUNS: usize = 3;
    static TIMING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// An upstream that, per connection, reads `D` or `U` and a length: for `D` it sends that
    /// many bytes and closes; for `U` it reads that many, then answers one byte.
    async fn upstream() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut cmd = [0u8; 9];
                    s.read_exact(&mut cmd).await.unwrap();
                    let n = u64::from_be_bytes(cmd[1..].try_into().unwrap()) as usize;
                    if cmd[0] == b'D' {
                        s.write_all(&vec![7u8; n]).await.unwrap();
                    } else {
                        let mut buf = vec![0u8; n];
                        s.read_exact(&mut buf).await.unwrap();
                        s.write_all(b"k").await.unwrap();
                    }
                    let _ = s.shutdown().await;
                });
            }
        });
        addr
    }

    async fn relayed(link: Link) -> SocketAddr {
        let up = upstream().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(relay(listener, up.to_string(), link));
        addr
    }

    fn command(kind: u8, n: usize) -> Vec<u8> {
        let mut c = vec![kind];
        c.extend_from_slice(&(n as u64).to_be_bytes());
        c
    }

    /// Downloads `n` bytes: the time to the first byte, and to the last.
    async fn download(relay: SocketAddr, n: usize) -> (Duration, Duration) {
        let mut s = TcpStream::connect(relay).await.unwrap();
        let t0 = Instant::now();
        s.write_all(&command(b'D', n)).await.unwrap();
        let mut buf = vec![0u8; READ];
        let (mut got, mut first) = (0, None);
        while got < n {
            let k = s.read(&mut buf).await.unwrap();
            assert!(k > 0, "closed after {got} of {n} bytes");
            first.get_or_insert_with(|| t0.elapsed());
            got += k;
        }
        (first.unwrap(), t0.elapsed())
    }

    /// Uploads `n` bytes, until the answer comes back.
    async fn upload(relay: SocketAddr, n: usize) -> Duration {
        let mut s = TcpStream::connect(relay).await.unwrap();
        let t0 = Instant::now();
        s.write_all(&command(b'U', n)).await.unwrap();
        s.write_all(&vec![1u8; n]).await.unwrap();
        let mut ack = [0u8; 1];
        s.read_exact(&mut ack).await.unwrap();
        t0.elapsed()
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0
    }

    fn fastest(runs: &[Duration]) -> f64 {
        runs.iter().map(|d| ms(*d)).fold(f64::INFINITY, f64::min)
    }

    /// Within 10% below and 60% above. A timer fires late, never early, so every run must reach
    /// the floor. Load only adds time, so the fastest run is held to the ceiling, which still
    /// tells a relay from one at half its rate or with twice its delay.
    fn near(runs: &[Duration], want_ms: f64) -> bool {
        runs.iter().all(|d| ms(*d) >= 0.9 * want_ms) && fastest(runs) <= 1.6 * want_ms
    }

    #[tokio::test]
    async fn the_delay_is_added_each_way_and_nothing_else_by_default() {
        let _alone = TIMING.lock().await;
        let relay = relayed(Link { delay: Duration::from_millis(20), bandwidth: Bandwidth::default() }).await;
        let (mut firsts, mut alls) = (Vec::new(), Vec::new());
        for _ in 0..RUNS {
            firsts.push(download(relay, 1).await.0);
            let (first, all) = download(relay, 8 * MB).await;
            firsts.push(first);
            alls.push(all);
        }
        assert!(near(&firsts, 40.0), "a round trip gains 2 × 20 ms: {firsts:?}");
        assert!(fastest(&alls) < 40.0 + 60.0, "8 MB take no longer than a fast copy: {alls:?}");
    }

    #[tokio::test]
    async fn a_connection_is_held_to_its_rate_each_way_and_still_delayed() {
        let _alone = TIMING.lock().await;
        let bandwidth = Bandwidth { down: Some(40e6), up: Some(20e6), total: None };
        let relay = relayed(Link { delay: Duration::from_millis(10), bandwidth }).await;
        let (mut firsts, mut downs, mut ups) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..RUNS {
            let (first, all) = download(relay, 8 * MB).await;
            firsts.push(first);
            downs.push(all);
            ups.push(upload(relay, 4 * MB).await);
        }
        // The first byte waits a round trip of 20 ms, and not for the rate: 8 MB take 200 ms.
        assert!(firsts.iter().all(|f| ms(*f) >= 20.0) && fastest(&firsts) < 80.0, "the first byte still waits a round trip: {firsts:?}");
        assert!(near(&downs, 20.0 + 200.0), "8 MB at 40 MB/s: {downs:?}");
        assert!(near(&ups, 20.0 + 200.0), "4 MB at 20 MB/s: {ups:?}");
    }

    #[tokio::test]
    async fn connections_share_the_total() {
        let _alone = TIMING.lock().await;
        let bandwidth = Bandwidth { down: Some(40e6), up: None, total: Some(60e6) };
        let relay = relayed(Link { delay: Duration::from_millis(5), bandwidth }).await;
        let mut alls = Vec::new();
        for _ in 0..RUNS {
            let t0 = Instant::now();
            let each = futures::future::join_all((0..3).map(|_| download(relay, 4 * MB))).await;
            alls.push(t0.elapsed());
            // Alone, each would take 100 ms at 40 MB/s; together they share 60 MB/s.
            assert!(each.iter().all(|(_, d)| ms(*d) > 150.0), "none ran at its own rate: {each:?}");
        }
        assert!(near(&alls, 10.0 + 200.0), "12 MB at 60 MB/s in all: {alls:?}");
    }

    #[test]
    fn bandwidths_parse() {
        assert_eq!(Bandwidth::parse("s3"), Ok(Bandwidth::S3));
        assert_eq!(Bandwidth::parse("95/68/-"), Ok(Bandwidth { down: Some(95e6), up: Some(68e6), total: None }));
        assert_eq!(Bandwidth::parse("-/-/1000"), Ok(Bandwidth { down: None, up: None, total: Some(1e9) }));
        for bad in ["", "95", "95/68", "0/1/2", "a/1/2", "1/2/3/4"] {
            assert!(Bandwidth::parse(bad).is_err(), "{bad:?}");
        }
    }
}
