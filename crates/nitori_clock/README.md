# nitori_clock

A monotonic clock and its timer as a trait. It is experimental and builds
without the standard library and without `alloc`; the repository pins a
nightly toolchain for its other crates.

`MonotonicClock` gives the current instant and a timer that ends at an
instant. It is not calendar time. `nitori_driver` reads it before each item
it handles and each lending from outside, and lends what it read; the timer
call of `nitori_driver_timer` sleeps on it until the earliest instant a
component set. Which clock that is is chosen by the code that composes the
driver, so each runtime provides its own implementation;
`nitori_clock_tokio` provides the one for tokio.

Adding a duration that has no bound is checked on the clock
(`MonotonicClock::checked_add`), so that a deadline past the last
representable instant is `None` rather than a panic.
