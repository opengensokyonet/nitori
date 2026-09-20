//! Reusable failure primitives shared by IO operations.
use snafu::Snafu;

/// The source ended before the requested number of bytes was read.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(display("read incomplete after {completed} of {expected} bytes"))]
pub struct Incomplete {
    pub expected: usize,
    pub completed: usize,
}

/// The destination cannot accommodate the next required bytes.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(display(
    "destination capacity {available} is below required {required} after {completed} bytes"
))]
pub struct InsufficientCapacity {
    pub required: usize,
    pub available: usize,
    pub completed: usize,
}

/// A nonempty write succeeded without accepting any bytes.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(display("write made no progress after {completed} bytes"))]
pub struct WriteZero {
    pub completed: usize,
}
