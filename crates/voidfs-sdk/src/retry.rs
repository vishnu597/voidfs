// SPDX-License-Identifier: Apache-2.0
//! When a failed request is sent again (protocol §6, informative): server errors, timeouts and
//! broken connections are retried for requests that are idempotent in effect. An unguarded
//! splice is not, and is sent again only when it cannot have reached the server.

use std::time::Duration;

use rand::RngExt;

use crate::Error;

/// Whether a request may be sent again once it may have reached the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Replay {
    /// Applying it twice has the effect of applying it once: reads, whole-object puts, offset
    /// writes and patches, and anything guarded by a precondition, whose second attempt fails
    /// with `412` instead of applying twice.
    Safe,
    /// Applying it twice is not applying it once: an unguarded insert or removal (§4.3) shifts
    /// bytes again, and an unguarded rename could move a newer object that took the source's name.
    /// Sent again only after a failure to connect.
    OnlyIfUnsent,
}

/// Whether `e`, from an attempt of a request that replays as `replay`, is worth another attempt.
pub(crate) fn may_retry(e: &Error, replay: Replay) -> bool {
    match e {
        Error::Transport { sent: false, .. } => true,
        _ if replay == Replay::OnlyIfUnsent => false,
        Error::Transport { .. } => true,
        Error::Service(s) => matches!(s.status, 429 | 500 | 502 | 503 | 504),
        Error::Decode(_) | Error::Invalid(_) => false,
    }
}

/// How long to wait before attempt `attempt + 1`, after `attempt` failed with `e`: exponential from
/// 100 ms with jitter, at most 5 s, or the server's `Retry-After` if longer (at most 20 s).
pub(crate) fn delay(attempt: u32, e: &Error) -> Duration {
    let cap = Duration::from_millis(100).saturating_mul(1u32 << attempt.saturating_sub(1).min(6)).min(Duration::from_secs(5));
    let jittered = cap.mul_f64(rand::rng().random_range(0.5..=1.0));
    match e {
        Error::Service(s) => s.retry_after.map_or(jittered, |r| r.min(Duration::from_secs(20)).max(jittered)),
        _ => jittered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServiceError;

    fn status(status: u16) -> Error {
        Error::Service(ServiceError { status, code: String::new(), message: String::new(), request_id: None, current_version_id: None, retry_after: None })
    }

    #[test]
    fn idempotent_requests_retry_server_errors_and_broken_connections() {
        for s in [429, 500, 502, 503, 504] {
            assert!(may_retry(&status(s), Replay::Safe), "{s}");
        }
        for s in [400, 403, 404, 409, 412, 413, 501] {
            assert!(!may_retry(&status(s), Replay::Safe), "{s}");
        }
        assert!(may_retry(&Error::Transport { message: String::new(), sent: true }, Replay::Safe));
        assert!(may_retry(&Error::Transport { message: String::new(), sent: false }, Replay::Safe));
        assert!(!may_retry(&Error::Decode(String::new()), Replay::Safe));
        assert!(!may_retry(&Error::Invalid(String::new()), Replay::Safe));
    }

    #[test]
    fn unguarded_inserts_and_removals_retry_only_what_was_never_sent() {
        assert!(may_retry(&Error::Transport { message: String::new(), sent: false }, Replay::OnlyIfUnsent));
        assert!(!may_retry(&Error::Transport { message: String::new(), sent: true }, Replay::OnlyIfUnsent));
        for s in [500, 502, 503, 504] {
            assert!(!may_retry(&status(s), Replay::OnlyIfUnsent), "{s}");
        }
    }

    #[test]
    fn delays_grow_and_honour_retry_after() {
        let e = status(500);
        assert!((50..=100).contains(&delay(1, &e).as_millis()));
        assert!((100..=200).contains(&delay(2, &e).as_millis()));
        assert!(delay(30, &e) <= Duration::from_secs(5));
        let mut slow = ServiceError { status: 503, code: "SlowDown".into(), message: String::new(), request_id: None, current_version_id: None, retry_after: Some(Duration::from_secs(2)) };
        assert_eq!(delay(1, &Error::Service(slow.clone())), Duration::from_secs(2));
        slow.retry_after = Some(Duration::from_secs(3600));
        assert_eq!(delay(1, &Error::Service(slow)), Duration::from_secs(20));
    }
}
