//! Synchronous capability polls on receivers.
use crate::{
    ChunkSegment, LentBuffer, LentOutcome, Read, ReadChunk, ReadSegment, Write, WriteLent,
    WriteSegment,
};
use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::Receiver;

pub trait PollReadExt: Receiver
where
    Self::Family: Read,
{
    fn poll_read<B: BufMut + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<ReadResult<Self::Family>> {
        Self::Family::poll_read(core::pin::pin!(self.view()), cx, out)
    }
    fn poll_read_chunk(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<ChunkResult<Self::Family>>
    where
        Self::Family: ReadChunk,
    {
        Self::Family::poll_read_chunk(core::pin::pin!(self.view()), cx, maximum)
    }
}
impl<H: Receiver + ?Sized> PollReadExt for H where H::Family: Read {}
pub trait PollWriteExt: Receiver
where
    Self::Family: Write,
{
    fn poll_write<B: Buf + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &mut B,
    ) -> Poll<WriteResult<Self::Family>> {
        Self::Family::poll_write(core::pin::pin!(self.view()), cx, input)
    }
    fn poll_write_lent<T, A, Fill>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<WriteLentResult<Self::Family, T, A>>
    where
        Self::Family: WriteLent,
        Fill: for<'l> FnOnce(
            &mut LentBuffer<<Self::Family as WriteLent>::Buffer<'l>>,
        ) -> Result<T, A>,
    {
        Self::Family::poll_write_lent(core::pin::pin!(self.view()), cx, wanted, fill)
    }
}
impl<H: Receiver + ?Sized> PollWriteExt for H where H::Family: Write {}

/// A segment or no progress, with the family's metadata and error types.
pub type ReadResult<F> = Result<Option<ReadSegment<<F as Read>::Metadata>>, <F as Read>::Error>;

/// A chunk or EOF, preserving the family's stable chunk, metadata and error
/// types.
pub type ChunkResult<F> = Result<
    Option<ChunkSegment<<F as ReadChunk>::Chunk, <F as Read>::Metadata>>,
    <F as Read>::Error,
>;

/// An accepted segment or no progress, with the family's report and error
/// types.
pub type WriteResult<F> = Result<Option<WriteSegment<<F as Write>::Report>>, <F as Write>::Error>;

/// What a loan came to, or no byte accepted, with the family's report and
/// error types.
pub type WriteLentResult<F, T, A> =
    Result<Option<LentOutcome<<F as Write>::Report, T, A>>, <F as Write>::Error>;
