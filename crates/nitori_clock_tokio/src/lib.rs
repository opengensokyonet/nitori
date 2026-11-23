#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

use core::time::Duration;
use std::time::Instant;

use nitori_clock::MonotonicClock;

/// The clock of the tokio runtime it is used in.
///
/// Its instants are the standard library's, converted from tokio's own, so
/// that they take part in arithmetic everywhere. It is read and slept on
/// from inside the runtime.
///
/// [`sleep_until`](MonotonicClock::sleep_until) panics outside a tokio
/// runtime, or in one without its time driver enabled, as
/// [`tokio::time::sleep_until`] does.
///
/// Tokio rounds the instant of a sleep up to its next millisecond, so
/// [`checked_add`](MonotonicClock::checked_add) gives `None` also for a sum
/// less than a millisecond before the last instant the standard library
/// represents: every instant it gives can be slept until.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioClock;

impl MonotonicClock for TokioClock {
    type Instant = Instant;
    type Sleep = tokio::time::Sleep;

    fn now(&self) -> Instant {
        tokio::time::Instant::now().into_std()
    }

    fn checked_add(&self, at: Instant, by: Duration) -> Option<Instant> {
        let sum = at.checked_add(by)?;
        // Tokio's timer adds up to a millisecond less a nanosecond to round
        // the instant up to its tick, and panics when that overflows.
        sum.checked_add(Duration::from_millis(1))?;
        Some(sum)
    }

    fn sleep_until(&self, at: Instant) -> tokio::time::Sleep {
        tokio::time::sleep_until(tokio::time::Instant::from_std(at))
    }

    /// One millisecond: tokio's timer runs at millisecond granularity.
    fn timer_granularity(&self) -> Duration {
        Duration::from_millis(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a runtime with time builds")
    }

    #[test]
    fn a_sleep_ends_no_earlier_than_its_instant() {
        runtime().block_on(async {
            let clock = TokioClock;
            let start = clock.now();
            let at = start + Duration::from_millis(5);
            clock.sleep_until(at).await;
            assert!(clock.now() >= at);
        });
    }

    #[test]
    fn a_sleep_until_the_past_ends_at_once() {
        runtime().block_on(async {
            let clock = TokioClock;
            let now = clock.now();
            clock.sleep_until(now).await;
            assert!(clock.now() >= now);
        });
    }

    #[test]
    fn checked_add_is_none_past_the_last_instant() {
        let clock = TokioClock;
        assert_eq!(clock.checked_add(clock.now(), Duration::MAX), None);
    }

    #[test]
    fn checked_add_agrees_with_plus() {
        let clock = TokioClock;
        let now = clock.now();
        let by = Duration::from_secs(3);
        assert_eq!(clock.checked_add(now, by), Some(now + by));
    }
}
