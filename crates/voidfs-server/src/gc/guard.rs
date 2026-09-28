// SPDX-License-Identifier: Apache-2.0
//! The writers' side of garbage collection (format §12.4): when a shard or page may be
//! referenced without uploading it, and when an upload is not enough on its own.
//!
//! The pool keeps a copy of `gc/pending.json` (the **view**), re-read at least every
//! [`REFRESH_EVERY`] and before any write that would rely on a copy older than
//! [`VIEW_MAX_AGE`]. Objects checked under §12.4 option 2 go into the **reuse set**; each
//! re-read renews them, drops the run's candidates, and forgets everything if the re-reads were
//! more than [`RENEW_WITHIN`] apart. Candidates of a run that is `waiting` are rewritten and
//! then **rescued** once a second read shows the run still waiting (option 3).
//!
//! The reuse set is separate from the shard cache: the cache holds bytes for reading, and
//! holding a shard's bytes says nothing about whether the shard is still in the bucket. It is a
//! plain set under the guard's lock, emptied when full: a cache with its own housekeeping, such
//! as moka's, blocks the async worker that runs it, which on this path delays commits.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail};
use bytes::Bytes;
use voidfs_core::ids::ShardHash;

use super::{PENDING, PendingRecord, Phase};
use crate::clock::Clock;
use crate::store::Store;

/// How often the pool re-reads `gc/pending.json` in the background.
pub const REFRESH_EVERY: Duration = Duration::from_secs(60);
/// A write re-reads `gc/pending.json` first if its copy is older than this.
pub const VIEW_MAX_AGE: Duration = Duration::from_secs(5 * 60);
/// Re-reads further apart than this forget every check. The format allows an hour (§12.4).
pub const RENEW_WITHIN: Duration = Duration::from_secs(50 * 60);
/// A write commits within this long of the check it relies on: half the minimum grace period
/// (§12.4).
pub const COMMIT_WITHIN: Duration = Duration::from_secs(12 * 3600);
/// How long a write waits for a run that is deleting objects it needs.
const DELETING_WAIT: Duration = Duration::from_secs(120);
const DELETING_POLL: Duration = Duration::from_millis(250);

/// A copy of `gc/pending.json`.
#[derive(Debug)]
pub struct View {
    /// When the read that produced it started (monotonic clock).
    pub read_at: Duration,
    /// Changes whenever the run or its phase does.
    generation: u64,
    pub run: Option<RunView>,
}

#[derive(Debug)]
pub struct RunView {
    pub id: String,
    pub phase: Phase,
    pub candidates: HashSet<ShardHash>,
}

impl View {
    fn candidate(&self, h: &ShardHash) -> Option<&RunView> {
        self.run.as_ref().filter(|r| r.candidates.contains(h))
    }

    fn same_run(&self, other: &Option<RunView>) -> bool {
        match (&self.run, other) {
            (None, None) => true,
            (Some(a), Some(b)) => a.id == b.id && a.phase == b.phase,
            _ => false,
        }
    }
}

struct State {
    /// `None` until the first successful read.
    view: Option<Arc<View>>,
    /// Objects rescued from the view's run.
    rescued: HashSet<ShardHash>,
    /// Objects checked under option 2 and renewed by every re-read since.
    reuse: HashSet<ShardHash>,
}

pub struct Guard {
    state: Mutex<State>,
    /// How many objects [`State::reuse`] holds before it is emptied.
    capacity: usize,
    refreshing: tokio::sync::Mutex<()>,
}

/// Whether an object is a shard or a page, for its path.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Shard,
    Page,
}

impl Kind {
    pub fn path(self, h: &ShardHash) -> String {
        match self {
            Kind::Shard => format!("shards/{}", h.object_path()),
            Kind::Page => format!("pages/{}", h.object_path()),
        }
    }
}

impl Guard {
    /// `capacity` is how many checked objects to remember; forgetting one only costs an upload.
    pub fn new(capacity: usize) -> Guard {
        Guard { state: Mutex::new(State { view: None, rescued: HashSet::new(), reuse: HashSet::new() }), capacity, refreshing: tokio::sync::Mutex::new(()) }
    }

    /// Re-reads `gc/pending.json` and installs it as the view.
    pub async fn refresh(&self, store: &Store, clock: &Clock) -> anyhow::Result<Arc<View>> {
        let _one_at_a_time = self.refreshing.lock().await;
        let read_at = clock.mono();
        let run = match store.get(PENDING).await? {
            None => None,
            Some(b) => {
                let rec: PendingRecord = serde_json::from_slice(&b).map_err(|e| anyhow!("{PENDING} is not a run record this server understands: {e}"))?;
                if rec.format != 1 {
                    bail!("{PENDING} has format {}, which this server does not implement", rec.format);
                }
                Some(RunView { id: rec.run, phase: rec.phase, candidates: rec.candidates.into_iter().collect() })
            }
        };
        let mut st = self.state.lock().unwrap();
        // Re-reads further apart than RENEW_WITHIN, or none before, forget every check.
        let (generation, same, renewed) = match &st.view {
            Some(v) if read_at.saturating_sub(v.read_at) <= RENEW_WITHIN => (v.generation, v.same_run(&run), true),
            Some(v) => (v.generation, false, false),
            None => (0, false, false),
        };
        if !renewed {
            st.reuse.clear();
        }
        if !same {
            if let Some(r) = &run {
                for h in &r.candidates {
                    st.reuse.remove(h);
                }
            }
            if st.view.as_ref().and_then(|v| v.run.as_ref()).map(|r| &r.id) != run.as_ref().map(|r| &r.id) {
                st.rescued.clear();
            }
        }
        let view = Arc::new(View { read_at, generation: if same { generation } else { generation + 1 }, run });
        st.view = Some(view.clone());
        Ok(view)
    }

    /// The view, re-read first if it is older than [`VIEW_MAX_AGE`].
    pub async fn view(&self, store: &Store, clock: &Clock) -> anyhow::Result<Arc<View>> {
        if let Some(v) = self.current()
            && clock.mono().saturating_sub(v.read_at) <= VIEW_MAX_AGE
        {
            return Ok(v);
        }
        self.refresh(store, clock).await
    }

    fn current(&self) -> Option<Arc<View>> {
        self.state.lock().unwrap().view.clone()
    }

    /// The view's generation, taken before a read so that [`Guard::confirmed`] can tell whether
    /// the view changed while the read was in flight.
    pub fn generation(&self) -> Option<u64> {
        self.current().map(|v| v.generation)
    }

    /// Records that `h` was read or written successfully by a request that started under the
    /// view of generation `before` (option 2).
    pub fn confirmed(&self, h: ShardHash, before: Option<u64>) {
        let mut st = self.state.lock().unwrap();
        if let Some(v) = &st.view
            && Some(v.generation) == before
            && v.candidate(&h).is_none()
        {
            if st.reuse.len() >= self.capacity {
                st.reuse.clear();
            }
            st.reuse.insert(h);
        }
    }

    /// Makes every object in `items` safe for a commit to reference (§12.4): uploads those
    /// not already checked, rewrites and rescues candidates of a waiting run, and waits out a
    /// run that is deleting any of them. The caller must commit within [`COMMIT_WITHIN`].
    pub async fn admit(&self, store: &Store, clock: &Clock, kind: Kind, items: &[(ShardHash, Bytes)]) -> anyhow::Result<()> {
        let started = clock.mono();
        let mut waits = 0u32;
        loop {
            // Classify against the newest view, which is at least as fresh as this one.
            self.view(store, clock).await?;
            let (mut upload, mut rescue, mut deleting) = (Vec::new(), Vec::new(), false);
            let (generation, run_id) = {
                let st = self.state.lock().unwrap();
                let view = st.view.as_ref().expect("a view was just read");
                for item in items {
                    match view.candidate(&item.0) {
                        Some(run) if run.phase == Phase::Deleting => deleting = true,
                        Some(_) if st.rescued.contains(&item.0) => {}
                        Some(_) => rescue.push(item),
                        None if st.reuse.contains(&item.0) => {}
                        None => upload.push(item),
                    }
                }
                (view.generation, view.run.as_ref().map(|r| r.id.clone()))
            };
            if deleting {
                // A run is deleting some of these; wait for it to finish, then upload them.
                waits += 1;
                if clock.is_manual() {
                    tokio::task::yield_now().await;
                } else {
                    tokio::time::sleep(DELETING_POLL).await;
                }
                if (!clock.is_manual() && clock.mono().saturating_sub(started) > DELETING_WAIT) || waits > 10_000 {
                    bail!("garbage collection is deleting objects this write needs; try again shortly");
                }
                self.refresh(store, clock).await?;
                continue;
            }
            let puts = upload.iter().chain(&rescue).map(|(h, b)| async move { store.put(&kind.path(h), b.clone()).await });
            for r in futures::future::join_all(puts).await {
                r?;
            }
            for (h, _) in &upload {
                self.confirmed(*h, Some(generation));
            }
            if rescue.is_empty() {
                return Ok(());
            }
            // Option 3: the rewrites count only if the run had not started deleting when they
            // were done, which a read after them shows.
            let after = self.refresh(store, clock).await?;
            match &after.run {
                Some(r) if Some(&r.id) == run_id.as_ref() && r.phase == Phase::Waiting => {
                    // Rescued objects stay safe from this run for as long as it lasts.
                    let mut st = self.state.lock().unwrap();
                    if st.view.as_ref().and_then(|v| v.run.as_ref()).map(|r| &r.id) == run_id.as_ref() {
                        st.rescued.extend(rescue.iter().map(|(h, _)| *h));
                    }
                    return Ok(());
                }
                // Deleting, or another run: start again.
                _ => continue,
            }
        }
    }
}
