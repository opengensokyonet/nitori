#![cfg_attr(not(test), no_std)]
#![feature(impl_trait_in_assoc_type, yield_expr)]
#![deny(unsafe_code)]
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

/// Fill a caller-selected buffer, advancing it by the segment's byte count.
///
/// `Some` reports one segment: the bytes this poll put into the destination
/// and their [`Metadata`](Read::Metadata). A segment has a single metadata
/// value, so a fill stops where the attributes of the source's bytes change,
/// even if that leaves the destination short. On a nonempty destination,
/// `None` means EOF. On an empty destination it means no progress and says
/// nothing about EOF. Empty requests still reach the host, which can report
/// its terminal error first. `Pending` and `Err` must not consume source
/// bytes or advance the destination. Report a completed prefix before a
/// subsequent error. Register the waker before returning `Pending`; retain
/// no pointer into this temporary borrow.
#[nitori_call::capability(sync)]
pub trait Read: nitori_call::ReceiverFamily {
    type Error;
    /// The attributes shared by the bytes of one segment, such as whether
    /// they arrived in 0-RTT; `()` for a source whose bytes have none. An
    /// adapter that forwards a source names the source's metadata.
    type Metadata;
    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<ReadResult<Self>>
    where
        Self: 'visit;
}

/// Transfer an owned buffer from the same stream as [`Read`].
///
/// `Some` contains a chunk of between one and `maximum` bytes with the
/// [`Metadata`](Read::Metadata) of all of them: a chunk ends where the
/// attributes of the source's bytes change. `None` means EOF. A chunk need
/// not be contiguous or zero-copy and does not borrow this poll's host
/// access. `Pending` and `Err` consume nothing. There are no recursive
/// defaults: use an explicit conversion helper when one path is not native.
#[nitori_call::capability(sync)]
pub trait ReadChunk: Read {
    type Chunk: Buf;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<ChunkResult<Self>>
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
/// implementation for readers; compose [`adapters::ReadSkip`] or
/// [`adapters::ReadChunkSkip`] on one instead. `Count` is chosen by the
/// source and needs no arithmetic here; operations that add or compare counts
/// state their own bounds, as [`calls::skip_exact`] does.
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
/// `Some` reports one segment: the accepted count, which equals the decrease
/// in `remaining()`, and the [`Report`](Write::Report) the write produced.
/// `None` means nothing was accepted: the input is empty, or the sink
/// accepts no byte, such as a full fixed-size buffer or a composed budget
/// such as [`adapters::WriteLimit`] at its boundary, which
/// [`calls::write_all`] reports as zero progress. Waiting is `Pending`. `Pending` and `Err` leave
/// the cursor and accepted byte count unchanged. Acceptance does not imply
/// flushing or remote delivery. Empty requests reach the host. The buffer
/// borrow lasts only for this poll. What the caller wants done with the bytes
/// (send parameters, a destination) is not part of this capability.
#[nitori_call::capability(sync)]
pub trait Write: nitori_call::ReceiverFamily {
    type Error;
    /// What one accepted segment produced, beyond its count; `()` for a
    /// sink that reports nothing. An adapter that forwards a sink names the
    /// sink's report.
    type Report;
    fn poll_write<'visit, Input: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut Input,
    ) -> Poll<WriteResult<Self>>
    where
        Self: 'visit;
}

/// Lend a writable region of the sink's own storage for the caller to fill,
/// and keep what was put there, as one segment of the stream that [`Write`]
/// writes.
///
/// In one poll the sink grants between one and `wanted` bytes, lends a
/// [`LentBuffer`] that holds at most the granted count, and calls `fill`
/// with it. `fill` puts a prefix into the buffer and returns its value, or
/// gives up with its own `A`. The sink then counts what it keeps from its own
/// storage, whatever `fill` reports, and returns the rest of the grant:
/// [`LentOutcome::Kept`] reports the count, at least one, the
/// [`Report`](Write::Report) and the value. When no byte was put, or the
/// loan was broken, the sink keeps nothing and hands the value back in
/// [`LentOutcome::NotKept`]. When `fill` gives up,
/// [`LentOutcome::GaveUp`], nothing is written, whatever it put into the
/// buffer.
///
/// `Ok(None)` means the sink accepts no byte, as [`Write`] answers `None`
/// to nonempty input: a full fixed-size buffer, or a composed budget such as
/// [`adapters::WriteLimit`] at its boundary. Waiting is `Pending`.
///
/// `fill` is called at most once per poll, and not at all on `Pending`,
/// `None` or `Err`, which write nothing; the sink drops it then. It cannot
/// await: a source it reads from polls synchronously, and a fill that would
/// wait gives up, keeping nothing. Register the waker before returning
/// `Pending`. There is no generated operation for this capability, since
/// `fill` lives for one poll; an operation calls `poll_write_lent` inside
/// the sink access's `poll_access`.
///
/// Every sink that lends also implements [`Write`]; neither is derived from
/// the other. A source that can produce bytes into a buffer, such as a
/// [`Read`] source or a generator, pairs with this capability, as a
/// [`ReadChunk`] source pairs with [`Write`].
pub trait WriteLent: Write {
    /// The storage the sink lends, seen as a [`BufMut`], such as
    /// `&'l mut BytesMut` for the spare capacity at its end.
    type Buffer<'l>: BufMut;
    fn poll_write_lent<'visit, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<WriteLentResult<Self, T, A>>
    where
        Self: 'visit,
        Fill: for<'l> FnOnce(&mut LentBuffer<Self::Buffer<'l>>) -> Result<T, A>;
}

mod combine;
pub use combine::Combine;
mod ext;
pub use ext::*;
mod lent_buffer;
pub use lent_buffer::{LentBuffer, LentOutcome};
mod segment;
mod spare_capacity;
pub use segment::{ChunkSegment, LentSegment, ReadSegment, WriteSegment};
pub use spare_capacity::VecSpareCapacity;
pub use split::{SplitTo, SplitToError};
