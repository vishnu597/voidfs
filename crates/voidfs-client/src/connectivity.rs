// SPDX-License-Identifier: Apache-2.0
//! Whether the server can be reached (step 4, item 3): online, degraded or offline, from the
//! outcome of every request a client makes and the change feed's liveness. Offline, the cache
//! and the mount fail at once instead of waiting out timeouts and retries, and the upload queue
//! holds its publishes; a probe finds out when the server answers again.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::watch;
use voidfs_sdk::{Client, Observation, Observe};

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Link {
    Online,
    /// The server answers, but with errors or slowly.
    Degraded,
    /// Requests get no answer.
    Offline,
}

#[derive(Clone, Debug)]
pub struct ConnectivityConfig {
    /// Requests in a row that get no answer before the link is offline.
    pub offline_after: u32,
    /// Server errors (`5xx`, `429`) or slow answers within `window` before it is degraded.
    pub degraded_after: u32,
    pub window: Duration,
    /// An answer slower than this counts against the link.
    pub slow: Duration,
    /// The first wait between probes while offline; it doubles up to `probe_max`.
    pub probe_every: Duration,
    pub probe_max: Duration,
    /// How long a filesystem call waits on the server while the server answers nothing at all,
    /// to anyone: then the link is offline, and the call fails rather than wait out timeouts.
    pub patience: Duration,
}

impl Default for ConnectivityConfig {
    fn default() -> ConnectivityConfig {
        ConnectivityConfig {
            offline_after: 3,
            degraded_after: 3,
            window: Duration::from_secs(30),
            slow: Duration::from_secs(10),
            probe_every: Duration::from_secs(2),
            probe_max: Duration::from_secs(30),
            patience: Duration::from_secs(5),
        }
    }
}

struct Counts {
    /// Requests in a row that got no answer.
    unanswered: u32,
    /// When recent server errors and slow answers were.
    troubles: VecDeque<Instant>,
    /// When the server last answered anything.
    answer: Option<Instant>,
}

struct Inner {
    cfg: ConnectivityConfig,
    tx: watch::Sender<Link>,
    counts: Mutex<Counts>,
}

/// The link to the server. Cloning shares it; give it to a client as its observer
/// ([`voidfs_sdk::Config::observer`]) so that every request counts.
#[derive(Clone)]
pub struct Connectivity(Arc<Inner>);

impl std::fmt::Debug for Connectivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Connectivity").field(&self.link()).finish()
    }
}

impl Default for Connectivity {
    fn default() -> Connectivity {
        Connectivity::new(ConnectivityConfig::default())
    }
}

impl Connectivity {
    pub fn new(cfg: ConnectivityConfig) -> Connectivity {
        let (tx, _) = watch::channel(Link::Online);
        Connectivity(Arc::new(Inner { cfg, tx, counts: Mutex::new(Counts { unanswered: 0, troubles: VecDeque::new(), answer: None }) }))
    }

    pub fn link(&self) -> Link {
        *self.0.tx.borrow()
    }

    /// The link now and as it changes.
    pub fn watch(&self) -> watch::Receiver<Link> {
        self.0.tx.subscribe()
    }

    /// `Ok` unless offline: what a caller asks before a request it would otherwise wait for.
    pub fn check(&self) -> Result<()> {
        if self.link() == Link::Offline { Err(Error::Offline) } else { Ok(()) }
    }

    fn set(&self, link: Link) {
        self.0.tx.send_if_modified(|l| {
            let changed = *l != link;
            *l = link;
            changed
        });
    }

    /// An answer came: online again, or degraded while troubles are recent.
    pub fn answered(&self, trouble: bool) {
        let now = Instant::now();
        let mut c = self.0.counts.lock().unwrap_or_else(|p| p.into_inner());
        c.unanswered = 0;
        c.answer = Some(now);
        if trouble {
            c.troubles.push_back(now);
        }
        while c.troubles.front().is_some_and(|t| now.duration_since(*t) > self.0.cfg.window) {
            c.troubles.pop_front();
        }
        let link = if c.troubles.len() as u32 >= self.0.cfg.degraded_after { Link::Degraded } else { Link::Online };
        drop(c);
        self.set(link);
    }

    /// A request got no answer.
    pub fn unanswered(&self) {
        let mut c = self.0.counts.lock().unwrap_or_else(|p| p.into_inner());
        c.unanswered += 1;
        let offline = c.unanswered >= self.0.cfg.offline_after;
        drop(c);
        if offline {
            self.set(Link::Offline);
        }
    }

    /// [`ConnectivityConfig::patience`].
    pub fn patience(&self) -> Duration {
        self.0.cfg.patience
    }

    /// Whether the server answered anything, to any request, within the last `d`.
    pub fn answered_within(&self, d: Duration) -> bool {
        self.0.counts.lock().unwrap_or_else(|p| p.into_inner()).answer.is_some_and(|t| t.elapsed() <= d)
    }

    /// A wait heard nothing from the server for [`ConnectivityConfig::patience`], nor did anything
    /// else: the server is as good as unreachable (stopped, or a connection that went quiet), so
    /// the link is offline now rather than after its requests time out. The probe brings it back.
    pub fn silent(&self) {
        self.set(Link::Offline);
    }

    /// While offline, asks the server something small (`ListBuckets`), `probe_every` apart and
    /// further apart each time, until it answers.
    pub fn probe(&self, client: Client) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let mut rx = this.watch();
            loop {
                if *rx.borrow_and_update() != Link::Offline {
                    if rx.changed().await.is_err() {
                        return;
                    }
                    continue;
                }
                let mut wait = this.0.cfg.probe_every;
                while this.link() == Link::Offline {
                    tokio::time::sleep(wait).await;
                    // Through the AWS client, which the observer doesn't see: counted here.
                    match client.list_drives().await {
                        Ok(_) => this.answered(false),
                        Err(e) if e.status().is_some() => this.answered(true),
                        Err(_) => this.unanswered(),
                    }
                    wait = (wait * 2).min(this.0.cfg.probe_max);
                }
            }
        })
    }
}

impl Observe for Connectivity {
    fn observe(&self, o: &Observation) {
        match o.status {
            Some(s) => self.answered(s >= 500 || s == 429 || o.elapsed > self.0.cfg.slow),
            None => self.unanswered(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(status: Option<u16>) -> Observation {
        Observation { status, sent: true, elapsed: Duration::from_millis(5) }
    }

    #[test]
    fn silence_takes_the_link_offline_and_any_answer_counts_as_heard() {
        let c = Connectivity::new(ConnectivityConfig { patience: Duration::from_millis(50), ..Default::default() });
        assert_eq!(c.patience(), Duration::from_millis(50));
        assert!(!c.answered_within(Duration::from_secs(60)), "nothing heard yet");
        c.observe(&obs(Some(404)));
        assert!(c.answered_within(Duration::from_millis(50)));
        std::thread::sleep(Duration::from_millis(80));
        assert!(!c.answered_within(Duration::from_millis(50)));
        c.observe(&obs(None));
        assert!(!c.answered_within(Duration::from_millis(50)), "no answer is not an answer");
        c.silent();
        assert_eq!(c.link(), Link::Offline, "silence is offline at once, not after timeouts");
        c.answered(false);
        assert_eq!(c.link(), Link::Online);
    }

    #[test]
    fn the_link_follows_what_requests_get() {
        let c = Connectivity::new(ConnectivityConfig { offline_after: 3, degraded_after: 2, ..Default::default() });
        let rx = c.watch();
        c.observe(&obs(None));
        c.observe(&obs(None));
        assert_eq!((c.link(), c.check().is_ok()), (Link::Online, true), "two unanswered are not yet offline");
        c.observe(&obs(None));
        assert_eq!(c.link(), Link::Offline);
        assert!(matches!(c.check(), Err(Error::Offline)));
        assert!(rx.has_changed().unwrap());
        c.observe(&obs(Some(404)));
        assert_eq!(c.link(), Link::Online, "any answer is an answer");
        c.observe(&obs(Some(503)));
        assert_eq!(c.link(), Link::Online);
        c.observe(&Observation { status: Some(200), sent: true, elapsed: Duration::from_secs(11) });
        assert_eq!(c.link(), Link::Degraded, "a server error and a slow answer");
        c.observe(&obs(None));
        assert_eq!(c.link(), Link::Degraded, "one unanswered request isn't offline");
    }
}
