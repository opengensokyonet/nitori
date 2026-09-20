#![feature(coroutine_trait, read_le)]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};

pub mod calls;
pub mod error;
pub mod helpers;

/// Fill a caller-selected buffer, advancing it by the returned byte count.
///
/// On a nonempty destination, `Ok(0)` means EOF. On an empty destination it
/// means no progress and says nothing about EOF. Empty requests still reach
/// the host, which can report its terminal error before returning zero.
/// `Pending` and `Err` must not consume source bytes or advance the destination.
/// Report a completed prefix before a subsequent error. Register the waker
/// before returning `Pending`; retain no pointer into this temporary borrow.
pub trait Read {
    type Error;
    fn poll_read<Output: BufMut + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Self::Error>>;
}

/// Transfer an owned buffer from the same stream as [`Read`].
///
/// `Some` contains between one and `maximum` bytes; `None` means EOF.
/// A chunk need not be contiguous or zero-copy and does not borrow this poll's
/// host access. `Pending` and `Err` consume nothing. There are no recursive
/// defaults: use an explicit conversion helper when one path is not native.
pub trait ReadChunk: Read {
    type Chunk: Buf;
    fn poll_read_chunk(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Self::Chunk>, Self::Error>>;
}

/// Accept a prefix of a caller-selected buffer and advance its cursor.
///
/// The return count equals the decrease in `remaining()`. `Pending` and `Err`
/// leave the cursor and accepted byte count unchanged. Waiting is `Pending`,
/// never `Ok(0)`; [`calls::WriteAll`] reports zero progress as an error.
/// Acceptance does not imply flushing or remote delivery. Empty requests reach
/// the host. The buffer borrow lasts only for this poll.
pub trait Write {
    type Error;
    fn poll_write<Input: Buf + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &mut Input,
    ) -> Poll<Result<usize, Self::Error>>;
}
