#![cfg_attr(not(test), no_std)]
#![feature(impl_trait_in_assoc_type, yield_expr)]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![warn(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

extern crate alloc;
#[cfg(all(feature = "std", not(test)))]
extern crate std;

use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};

pub mod adapters;
#[cfg(feature = "std")]
pub mod bridge;
mod impls;
mod length;
pub use length::{Infinite, Min};

pub mod calls;
pub mod error;
pub mod helpers;
pub mod prelude;
mod split;

/// Report the exact remaining byte count of this source layer at its cursor.
/// Querying a view is synchronous and consumes nothing. A successful read
/// subtracts the number of bytes consumed from a finite count. A known
/// infinite source can report [`Infinite`]. A source whose end cannot be
/// predicted need not implement this capability.
///
/// The count describes reading to normal EOF without an independent change
/// such as seeking or appending data. A subsequent IO failure is still
/// possible. Length queries do not require [`Read`] or [`ReadChunk`].
#[nitori_call::capability]
pub trait ExactLen: nitori_call::ReceiverFamily {
    /// This source's representation of its exact remaining byte length.
    type Length;
    /// Return the byte length from the current cursor to normal EOF.
    fn remaining_len<'visit>(source: Pin<&mut Self::ReceiverView<'visit>>) -> Self::Length
    where
        Self: 'visit;
}

/// Fill a caller-selected buffer, advancing it by the returned byte count.
///
/// On a nonempty destination, `Ok(0)` means EOF. On an empty destination it
/// means no progress and says nothing about EOF. Empty requests still reach
/// the host, which can report its terminal error before returning zero.
/// `Pending` and `Err` must not consume source bytes or advance the destination.
/// Report a completed prefix before a subsequent error. Register the waker
/// before returning `Pending`; retain no pointer into this temporary borrow.
#[nitori_call::capability(sync)]
pub trait Read: nitori_call::ReceiverFamily {
    type Error;
    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit;
}

/// Transfer an owned buffer from the same stream as [`Read`].
///
/// `Some` contains between one and `maximum` bytes; `None` means EOF.
/// A chunk need not be contiguous or zero-copy and does not borrow this poll's
/// host access. `Pending` and `Err` consume nothing. There are no recursive
/// defaults: use an explicit conversion helper when one path is not native.
#[nitori_call::capability(sync)]
pub trait ReadChunk: Read {
    type Chunk: Buf;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Self::Chunk>, <Self as Read>::Error>>
    where
        Self: 'visit;
}

/// Advance past bytes of the same stream as [`Read`] without delivering them.
///
/// `maximum` bounds this poll. It is a finite count of any width or
/// [`Infinite`]; the host takes `maximum.min(available)` as the number to
/// skip, where `available` is what it can skip now, so the result has the
/// source's own `Count` type. `Some(n)` reports the bytes this poll skipped,
/// at most `maximum`; `None` means EOF. As with [`Read`], the reported count
/// is exactly what the poll consumed, and `Pending` and `Err` consume
/// nothing, so a count kept above this source (a budget, a payload's
/// remainder) stays in step with it across cancellation. Register the waker
/// before returning `Pending`.
///
/// A zero maximum is an empty request, like an empty read buffer: `Some(0)`
/// means no progress and says nothing about EOF, and `Some(0)` is reported
/// for no other maximum. The request still reaches the host, which can report
/// its terminal error or EOF first. `None` always means EOF.
///
/// A source implements this when it can skip natively, without copying, such
/// as a memory buffer advancing its cursor. There is no blanket
/// implementation for readers. `Count` is chosen by the source and needs no
/// arithmetic here; code that adds or compares counts states its own bounds.
#[nitori_call::capability(sync)]
pub trait Skip: Read {
    /// This source's representation of a skipped byte count.
    type Count;
    // The bound `Clone` lets the generated operation offer the same maximum
    // again after Pending.
    fn poll_skip<'visit, M>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<Self::Count>, <Self as Read>::Error>>
    where
        Self: 'visit,
        M: Min<Self::Count, Output = Self::Count> + Clone;
}

/// Accept a prefix of a caller-selected buffer and advance its cursor.
///
/// The return count equals the decrease in `remaining()`. `Pending` and `Err`
/// leave the cursor and accepted byte count unchanged. Waiting is `Pending`,
/// never `Ok(0)`; [`calls::write_all`] reports zero progress as an error.
/// Acceptance does not imply flushing or remote delivery. Empty requests reach
/// the host. The buffer borrow lasts only for this poll.
#[nitori_call::capability(sync)]
pub trait Write: nitori_call::ReceiverFamily {
    type Error;
    fn poll_write<'visit, Input: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut Input,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit;
}

mod ext;
pub use ext::*;
pub use split::{SplitTo, SplitToError};
