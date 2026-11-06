//! Reusable failure primitives shared by IO operations.
use snafu::Snafu;

/// The source ended before the requested number of bytes was read.
///
/// The completed count is always below the expected count.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(display("read incomplete after {completed} of {expected} bytes"))]
pub struct IncompleteError {
    expected: usize,
    completed: usize,
}

impl IncompleteError {
    /// Record that `completed` of `expected` bytes were read.
    ///
    /// # Errors
    ///
    /// Fails when `completed` is not below `expected`, because a read that
    /// reached its expected length is not incomplete.
    pub fn new(expected: usize, completed: usize) -> Result<Self, CompletedNotBelowExpectedError> {
        if completed < expected {
            Ok(Self {
                expected,
                completed,
            })
        } else {
            Err(CompletedNotBelowExpectedError {
                expected,
                completed,
            })
        }
    }

    /// The number of bytes the read was asked for.
    #[must_use]
    pub fn expected(&self) -> usize {
        self.expected
    }

    /// The number of bytes read before the source ended; below [`expected`](Self::expected).
    #[must_use]
    pub fn completed(&self) -> usize {
        self.completed
    }
}

/// The counts given to [`IncompleteError::new`] do not describe an incomplete
/// read: the completed count is not below the expected count.
#[derive(Debug, Snafu)]
#[snafu(display("{completed} completed bytes are not below {expected} expected bytes"))]
pub struct CompletedNotBelowExpectedError {
    expected: usize,
    completed: usize,
}

impl CompletedNotBelowExpectedError {
    /// The expected byte count that was given.
    #[must_use]
    pub fn expected(&self) -> usize {
        self.expected
    }

    /// The completed byte count that was given; not below [`expected`](Self::expected).
    #[must_use]
    pub fn completed(&self) -> usize {
        self.completed
    }
}

/// The destination has less room than the whole requested read needs.
///
/// Reported before any IO, so the available room is always below the required
/// length.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(display("destination capacity {available} is below required {required}"))]
pub struct InsufficientCapacityError {
    required: usize,
    available: usize,
}

impl InsufficientCapacityError {
    /// The number of bytes the read needs room for.
    #[must_use]
    pub fn required(&self) -> usize {
        self.required
    }

    /// The room the destination has; below [`required`](Self::required).
    #[must_use]
    pub fn available(&self) -> usize {
        self.available
    }
}

/// The destination filled up before the source was confirmed to be at its end.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(display("destination full after {completed} bytes, before the end of the source"))]
pub struct DestinationFullError {
    /// Bytes appended before the destination filled up.
    pub completed: usize,
}

/// A nonempty write succeeded without accepting any bytes.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[snafu(display("write made no progress after {completed} bytes"))]
pub struct WriteZeroError {
    /// Bytes accepted before the write that accepted none.
    pub completed: usize,
}
