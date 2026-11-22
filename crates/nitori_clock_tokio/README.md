# nitori_clock_tokio

The `nitori_clock` monotonic clock of a tokio runtime, `TokioClock`. It is
experimental and builds on stable Rust.

It reads tokio's time as standard library instants and sleeps on tokio's
timer, so it follows the runtime it is used in, including a runtime whose
time is paused. Its timer granularity is one millisecond.
