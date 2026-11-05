//! Splitting a buffer into consecutive independent pieces without copying.
use bytes::{Bytes, BytesMut};
use snafu::{Snafu, ensure};

/// Asked to cut more bytes off a buffer than it holds.
#[derive(Debug, Snafu)]
#[snafu(display("cannot cut {at} bytes off a buffer of {length} bytes"))]
pub struct SplitToError {
    at: usize,
    length: usize,
}

impl SplitToError {
    /// The number of bytes that was asked for.
    pub fn at(&self) -> usize {
        self.at
    }

    /// The length of the buffer, which is below [`at`](Self::at).
    pub fn length(&self) -> usize {
        self.length
    }
}

/// A buffer that can cut its first bytes off as a piece of their own.
///
/// Only cutting from the start is defined, because that is what splitting a
/// datagram into packets needs. The trait does not require `AsRef<[u8]>` or
/// `AsMut<[u8]>`: code that must read the bytes adds the former, and code that
/// rewrites them in place adds the latter.
pub trait SplitTo: Sized {
    /// Cut the first `at` bytes off as a piece of their own; `self` keeps the
    /// rest.
    ///
    /// Fails, leaving `self` unchanged, when `at` exceeds the length of
    /// `self`.
    fn split_to(&mut self, at: usize) -> Result<Self, SplitToError>;
}

/// Zero-copy: both pieces share the allocation, and each is writable on its own.
impl SplitTo for BytesMut {
    fn split_to(&mut self, at: usize) -> Result<Self, SplitToError> {
        ensure!(
            at <= self.len(),
            SplitToSnafu {
                at,
                length: self.len()
            }
        );
        Ok(BytesMut::split_to(self, at))
    }
}

/// Zero-copy: both pieces share the allocation and are read-only.
impl SplitTo for Bytes {
    fn split_to(&mut self, at: usize) -> Result<Self, SplitToError> {
        ensure!(
            at <= self.len(),
            SplitToSnafu {
                at,
                length: self.len()
            }
        );
        Ok(Bytes::split_to(self, at))
    }
}

/// The borrowed form: the piece and the rest are subslices of the same bytes.
impl SplitTo for &[u8] {
    fn split_to(&mut self, at: usize) -> Result<Self, SplitToError> {
        ensure!(
            at <= self.len(),
            SplitToSnafu {
                at,
                length: self.len()
            }
        );
        let (head, tail) = self.split_at(at);
        *self = tail;
        Ok(head)
    }
}
