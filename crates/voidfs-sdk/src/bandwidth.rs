// SPDX-License-Identifier: Apache-2.0
//! A shared limit on how fast request bodies go out: a token bucket that every request of the
//! clients that share it draws from, a piece of the body at a time. Its rate can change at any
//! moment, and the next piece goes at the new rate.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How much of a body is sent per draw on the bucket.
pub const PIECE: usize = 64 * 1024;

/// An upload bandwidth limit. Share it through [`crate::Config::upload_bandwidth`].
#[derive(Debug)]
pub struct Bandwidth {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    /// Bytes a second; `None` is unlimited.
    rate: Option<u64>,
    tokens: f64,
    last: Instant,
}

impl Bandwidth {
    pub fn new(bytes_per_second: Option<u64>) -> Bandwidth {
        Bandwidth { state: Mutex::new(State { rate: bytes_per_second.filter(|r| *r > 0), tokens: 0.0, last: Instant::now() }) }
    }

    /// The limit, in bytes a second; `None` or `Some(0)` lifts it.
    pub fn set(&self, bytes_per_second: Option<u64>) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.rate = bytes_per_second.filter(|r| *r > 0);
        s.tokens = s.tokens.min(Self::burst(s.rate));
        s.last = Instant::now();
    }

    pub fn get(&self) -> Option<u64> {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).rate
    }

    /// What may go at once: a quarter of a second's worth, and at least a piece.
    fn burst(rate: Option<u64>) -> f64 {
        rate.map_or(f64::MAX, |r| (r as f64 / 4.0).max(PIECE as f64))
    }

    /// Waits until `n` bytes may go.
    pub async fn take(&self, n: usize) {
        loop {
            let wait = {
                let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                let Some(rate) = s.rate else { return };
                let now = Instant::now();
                let burst = Self::burst(s.rate);
                s.tokens = (s.tokens + now.duration_since(s.last).as_secs_f64() * rate as f64).min(burst);
                s.last = now;
                if s.tokens >= n as f64 {
                    s.tokens -= n as f64;
                    return;
                }
                Duration::from_secs_f64((n as f64 - s.tokens) / rate as f64)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_rate_holds_across_takers_and_changes_at_once() {
        let bw = Arc::new(Bandwidth::new(Some(4 << 20)));
        let t = Instant::now();
        let takers = (0..4).map(|_| {
            let bw = bw.clone();
            tokio::spawn(async move {
                for _ in 0..16 {
                    bw.take(PIECE).await;
                }
            })
        });
        futures::future::join_all(takers).await;
        // 4 MiB at 4 MiB/s, starting with nothing in the bucket.
        let secs = t.elapsed().as_secs_f64();
        assert!((0.95..1.15).contains(&secs), "{secs}");
        bw.set(None);
        let t = Instant::now();
        for _ in 0..1000 {
            bw.take(PIECE).await;
        }
        assert!(t.elapsed() < Duration::from_millis(50), "unlimited");
        assert_eq!(bw.get(), None);
    }
}
