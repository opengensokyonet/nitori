//! Splitting a buffer into consecutive independent pieces without copying.
use bytes::BytesMut;

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
    /// # Panics
    ///
    /// Panics when `at` exceeds the length of `self`.
    fn split_to(&mut self, at: usize) -> Self;
}

/// Zero-copy: both pieces share the allocation, and each is writable on its own.
impl SplitTo for BytesMut {
    fn split_to(&mut self, at: usize) -> Self {
        BytesMut::split_to(self, at)
    }
}

/// The borrowed form: the piece and the rest are subslices of the same bytes.
impl SplitTo for &[u8] {
    fn split_to(&mut self, at: usize) -> Self {
        let (head, tail) = self.split_at(at);
        *self = tail;
        head
    }
}
