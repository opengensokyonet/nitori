//! An instant `checked_add` gives is one tokio can sleep until.

use core::time::Duration;

use nitori_clock::MonotonicClock;
use nitori_clock_tokio::TokioClock;

/// The longest `by` for which `checked_add(at, by)` gives an instant.
fn longest_addition(clock: &TokioClock, at: std::time::Instant) -> Duration {
    const NANOS: u128 = 1_000_000_000;
    let duration = |nanos: u128| {
        Duration::new(
            u64::try_from(nanos / NANOS).unwrap(),
            u32::try_from(nanos % NANOS).unwrap(),
        )
    };
    let (mut fits, mut too_long) = (0, Duration::MAX.as_nanos() + 1);
    while too_long - fits > 1 {
        let middle = fits + (too_long - fits) / 2;
        match clock.checked_add(at, duration(middle)) {
            Some(_) => fits = middle,
            None => too_long = middle,
        }
    }
    duration(fits)
}

#[test]
fn the_last_instant_checked_add_gives_can_be_slept_until() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let clock = TokioClock;
        let now = clock.now();
        let last = clock
            .checked_add(now, longest_addition(&clock, now))
            .unwrap();
        let sleep = tokio::time::timeout(Duration::from_millis(5), clock.sleep_until(last));
        assert!(
            sleep.await.is_err(),
            "the sleep ends long after the timeout"
        );
    });
}
