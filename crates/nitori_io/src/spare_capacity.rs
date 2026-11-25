//! The spare capacity of a vector, lent as a [`BufMut`] that cannot reach
//! the bytes already written.
#![allow(unsafe_code)]
use alloc::vec::Vec;
use bytes::{BufMut, buf::UninitSlice};
use core::{fmt, num::NonZeroUsize};

/// The spare capacity after a vector's written bytes, at most a granted
/// count, lent as a [`BufMut`]. What is put into it is not part of the
/// vector until the lending sink keeps it; the bytes written before are out of
/// reach.
pub struct VecSpareCapacity<'a> {
    vec: &'a mut Vec<u8>,
    /// Bytes after `vec.len()` initialized through `advance_mut`.
    filled: usize,
    granted: usize,
}

impl<'a> VecSpareCapacity<'a> {
    /// Reserve room for `granted` bytes after the written ones and lend it.
    pub(crate) fn new(vec: &'a mut Vec<u8>, granted: NonZeroUsize) -> Self {
        vec.reserve(granted.get());
        Self {
            vec,
            filled: 0,
            granted: granted.get(),
        }
    }

    /// The bytes put so far.
    pub(crate) fn filled(&self) -> usize {
        self.filled
    }

    /// Make the filled bytes part of the vector, and return their count.
    pub(crate) fn keep(self) -> usize {
        let Self { vec, filled, .. } = self;
        // SAFETY: `filled` only grows through `advance_mut`, whose callers
        // guarantee that the advanced bytes of `chunk_mut` were initialized;
        // `chunk_mut` is the spare capacity right after `vec.len()`, which
        // nothing else changed while this was lent, and `filled` stays within
        // the reserved `granted` bytes.
        unsafe { vec.set_len(vec.len() + filled) };
        filled
    }
}

// SAFETY: `chunk_mut` is uninitialized memory owned by the vector's spare
// capacity, `remaining_mut` bytes long, and `advance_mut` counts what the
// caller initialized without exceeding it.
unsafe impl BufMut for VecSpareCapacity<'_> {
    fn remaining_mut(&self) -> usize {
        self.granted - self.filled
    }

    unsafe fn advance_mut(&mut self, count: usize) {
        assert!(
            count <= self.remaining_mut(),
            "advanced past the lent capacity"
        );
        self.filled += count;
    }

    fn chunk_mut(&mut self) -> &mut UninitSlice {
        let (filled, granted) = (self.filled, self.granted);
        UninitSlice::uninit(&mut self.vec.spare_capacity_mut()[filled..granted])
    }
}

impl fmt::Debug for VecSpareCapacity<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VecSpareCapacity")
            .field("filled", &self.filled)
            .field("granted", &self.granted)
            .finish_non_exhaustive()
    }
}
