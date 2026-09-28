// SPDX-License-Identifier: Apache-2.0
//! Time for the pool and its garbage collection: the system's, or a manual clock for tests and
//! simulations.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use voidfs_core::ids::Timestamp;

#[derive(Clone, Default)]
pub enum Clock {
    #[default]
    System,
    /// Time stands still until [`Clock::advance`] moves it. For tests and simulations.
    #[cfg_attr(not(test), allow(dead_code))]
    Manual(Arc<Mutex<Timestamp>>),
}

impl Clock {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn manual(start: Timestamp) -> Clock {
        Clock::Manual(Arc::new(Mutex::new(start)))
    }

    /// Wall-clock time: commit times, and anything compared with the bucket's clock.
    pub fn now(&self) -> Timestamp {
        match self {
            Clock::System => Timestamp::now(),
            Clock::Manual(t) => *t.lock().unwrap(),
        }
    }

    /// Monotonic time since an arbitrary origin, for measuring how long something took. It does
    /// not jump when the system's wall clock is corrected.
    pub fn mono(&self) -> Duration {
        match self {
            Clock::System => {
                static ORIGIN: OnceLock<Instant> = OnceLock::new();
                ORIGIN.get_or_init(Instant::now).elapsed()
            }
            Clock::Manual(t) => (t.lock().unwrap().datetime() - chrono::DateTime::UNIX_EPOCH).to_std().unwrap_or_default(),
        }
    }

    /// Moves a manual clock forward.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn advance(&self, by: Duration) {
        if let Clock::Manual(t) = self {
            let mut t = t.lock().unwrap();
            *t = Timestamp::from_datetime(t.datetime() + chrono::Duration::from_std(by).unwrap());
        }
    }

    pub fn is_manual(&self) -> bool {
        matches!(self, Clock::Manual(_))
    }
}

/// Parses durations such as `90s`, `15m`, `24h` or `30d`; a bare number is seconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (n, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => s.split_at(i),
        None => (s, "s"),
    };
    let n: u64 = n.parse().map_err(|_| format!("{s:?} is not a duration such as 90s, 15m, 24h or 30d"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return Err(format!("{s:?}: the unit must be s, m, h or d")),
    };
    Ok(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("15m").unwrap(), Duration::from_secs(900));
        assert_eq!(parse_duration("24h").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_duration("30d").unwrap(), Duration::from_secs(30 * 86_400));
        assert!(parse_duration("1w").is_err());
        assert!(parse_duration("h").is_err());
    }

    #[test]
    fn a_manual_clock_moves_only_when_told() {
        let c = Clock::manual("2026-09-27T00:00:00Z".parse().unwrap());
        let (t, m) = (c.now(), c.mono());
        c.advance(Duration::from_secs(3600));
        assert_eq!(c.now().datetime() - t.datetime(), chrono::Duration::hours(1));
        assert_eq!(c.mono() - m, Duration::from_secs(3600));
    }
}
