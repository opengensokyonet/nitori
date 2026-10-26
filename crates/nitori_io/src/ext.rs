//! Synchronous capability polls on receivers.
use crate::{Read, ReadChunk, Write};
use bytes::{Buf, BufMut};
use nitori_call::Receiver;
use std::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};

pub trait PollReadExt: Receiver
where
    Self::Family: Read,
{
    fn poll_read<B: BufMut + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, <Self::Family as Read>::Error>> {
        Self::Family::poll_read(std::pin::pin!(self.view()), cx, out)
    }
    fn poll_read_chunk(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<ChunkResult<Self::Family>>
    where
        Self::Family: ReadChunk,
    {
        Self::Family::poll_read_chunk(std::pin::pin!(self.view()), cx, maximum)
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
    ) -> Poll<Result<usize, <Self::Family as Write>::Error>> {
        Self::Family::poll_write(std::pin::pin!(self.view()), cx, input)
    }
}
impl<H: Receiver + ?Sized> PollWriteExt for H where H::Family: Write {}

/// A chunk or EOF, preserving the family's stable chunk and error types.
pub type ChunkResult<F> = Result<Option<<F as ReadChunk>::Chunk>, <F as Read>::Error>;
