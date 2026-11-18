//! The current time, as a capability.
//!
//! Ticket lifetimes and certificate validity depend on the calendar time,
//! which comes from a [`Clock`] given when an endpoint is built:
//! [`SystemClock`] reads the system's clock, and [`ManualClock`] holds a
//! time that is set by hand, so that tests and recorded traces can fix it.
//!
//! The time is a [`UtcDateTime`]: certificates name calendar dates, and
//! tickets outlive the process that issued them. It is not the monotonic
//! time a connection measures its timers with.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use time::UtcDateTime;

/// A source of the current calendar time.
pub trait Clock {
    /// The current time.
    fn now(&self) -> UtcDateTime;
}

impl<K: Clock + ?Sized> Clock for &K {
    fn now(&self) -> UtcDateTime {
        (**self).now()
    }
}

impl<K: Clock + ?Sized> Clock for Arc<K> {
    fn now(&self) -> UtcDateTime {
        (**self).now()
    }
}

/// The system's clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UtcDateTime {
        UtcDateTime::now()
    }
}

/// A clock whose time moves only when it is set or advanced.
#[derive(Debug)]
pub struct ManualClock {
    time: Mutex<UtcDateTime>,
}

impl ManualClock {
    /// A clock that reads `time`.
    pub fn new(time: UtcDateTime) -> Self {
        Self {
            time: Mutex::new(time),
        }
    }

    /// Sets the time to `time`.
    pub fn set(&self, time: UtcDateTime) {
        *self.time.lock().unwrap_or_else(PoisonError::into_inner) = time;
    }

    /// Moves the time forward by `by`.
    pub fn advance(&self, by: Duration) {
        let mut time = self.time.lock().unwrap_or_else(PoisonError::into_inner);
        *time += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> UtcDateTime {
        *self.time.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The time from `earlier` to `later`; `None` when `later` is before
/// `earlier`, as when a clock was set back.
pub(crate) fn elapsed(earlier: UtcDateTime, later: UtcDateTime) -> Option<Duration> {
    Duration::try_from(later - earlier).ok()
}

#[cfg(test)]
mod tests {
    use super::{Clock, ManualClock, SystemClock, elapsed};
    use std::time::Duration;
    use time::UtcDateTime;

    #[test]
    fn a_manual_clock_moves_when_told() {
        let start = UtcDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let clock = ManualClock::new(start);
        assert_eq!(clock.now(), start);
        clock.advance(Duration::from_secs(5));
        assert_eq!(Clock::now(&&clock), start + Duration::from_secs(5));
        clock.set(start);
        assert_eq!(clock.now(), start);
        assert!(SystemClock.now() > UtcDateTime::UNIX_EPOCH);
    }

    #[test]
    fn elapsed_is_none_backwards() {
        let start = UtcDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let later = start + Duration::from_millis(1500);
        assert_eq!(elapsed(start, later), Some(Duration::from_millis(1500)));
        assert_eq!(elapsed(later, start), None);
    }
}
