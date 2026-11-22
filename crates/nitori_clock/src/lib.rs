#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

use core::{
    fmt::Debug,
    future::Future,
    ops::{Add, Sub},
    time::Duration,
};

/// A monotonic clock and its timer.
///
/// It gives a monotonic instant and a timer, not calendar time. A driver
/// reads it at the start of each poll and sleeps on it until its next
/// deadline.
///
/// A clone is another handle to the same clock: the code that composes a
/// driver can give one to each part that reports or waits for instants.
pub trait MonotonicClock: Clone {
    /// A point in time.
    ///
    /// Subtracting gives the time between two instants. It is used only
    /// after comparing them, never where the result would be below zero,
    /// so what a type does there (`std::time::Instant` saturates,
    /// `Duration` panics) does not matter. Adding is checked on the clock
    /// ([`checked_add`](MonotonicClock::checked_add)) where the duration
    /// has no bound.
    type Instant: Ord
        + Copy
        + Add<Duration, Output = Self::Instant>
        + Sub<Output = Duration>
        + Debug;
    /// A timer that ends at an instant; it need not be resettable.
    type Sleep: Future<Output = ()>;
    /// The current instant.
    fn now(&self) -> Self::Instant;
    /// `at` plus `by`, or `None` when the sum is past the last instant the
    /// type can represent.
    fn checked_add(&self, at: Self::Instant, by: Duration) -> Option<Self::Instant>;
    /// A timer that ends at `at`.
    fn sleep_until(&self, at: Self::Instant) -> Self::Sleep;
    /// The precision `sleep_until` achieves.
    fn timer_granularity(&self) -> Duration;
}
