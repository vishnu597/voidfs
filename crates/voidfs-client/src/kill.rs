// SPDX-License-Identifier: Apache-2.0
//! Named process kill points for the crash-recovery tests. A unit-test child process armed with
//! `VOIDFS_KILL_POINT=<name>` (and optionally `VOIDFS_KILL_AFTER=<n>`, the hit that fires) sends
//! itself `SIGKILL` there; other builds compile them to nothing.

#[cfg(test)]
pub(crate) fn point(name: &str) {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    static ARMED: OnceLock<Option<(String, u64)>> = OnceLock::new();
    static HITS: AtomicU64 = AtomicU64::new(0);
    let armed = ARMED.get_or_init(|| {
        let point = std::env::var("VOIDFS_KILL_POINT").ok()?;
        Some((point, std::env::var("VOIDFS_KILL_AFTER").ok().and_then(|n| n.parse().ok()).unwrap_or(1)))
    });
    let Some((point, after)) = armed else { return };
    if point != name || HITS.fetch_add(1, Ordering::SeqCst) + 1 != *after { return; }
    let _ = std::process::Command::new("kill").args(["-KILL", &std::process::id().to_string()]).status();
    loop { std::thread::park(); }
}

#[cfg(not(test))]
#[inline(always)]
pub(crate) fn point(_: &str) {}
