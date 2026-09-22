//! Capability methods on actual hosts and operation methods on real receivers.
use crate::{Read, ReadChunk, Write};
use bytes::{Buf, BufMut};
use nitori_call::{AdaptedCall, Child, Receiver, TargetExt};
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

pub trait TargetReadExt: TargetExt
where
    Self::Target: Read,
{
    fn read<'a, B: BufMut + ?Sized>(
        self,
        out: &'a mut B,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::Read<'a, B>>> {
        self.operation(crate::calls::Read::new(out))
    }
    fn read_exact<'a, B: BufMut + ?Sized>(
        self,
        out: &'a mut B,
        length: usize,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::ReadExact<'a, B>>> {
        self.operation(crate::calls::ReadExact::new(out, length))
    }
    fn read_to_end<'a, B: BufMut + ?Sized>(
        self,
        out: &'a mut B,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::ReadToEnd<'a, B>>> {
        self.operation(crate::calls::ReadToEnd::new(out))
    }
    fn read_array<const N: usize>(
        self,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::ReadArray<N>>> {
        self.operation(crate::calls::ReadArray::new())
    }
    fn read_le<T: crate::calls::EndianValue>(
        self,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::ReadLe<T>>>
    where
        crate::calls::ReadLe<T>: nitori_call::CallOn<Self::Target>,
    {
        self.operation(crate::calls::ReadLe::new())
    }
    fn read_be<T: crate::calls::EndianValue>(
        self,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::ReadBe<T>>>
    where
        crate::calls::ReadBe<T>: nitori_call::CallOn<Self::Target>,
    {
        self.operation(crate::calls::ReadBe::new())
    }
}
impl<R: TargetExt> TargetReadExt for R where R::Target: Read {}
pub trait TargetChunkExt: TargetExt
where
    Self::Target: ReadChunk,
{
    fn read_chunk(
        self,
        maximum: NonZeroUsize,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::ReadChunk>> {
        self.operation(crate::calls::ReadChunk::new(maximum))
    }
    fn read_chunks(
        self,
        length: usize,
        maximum: NonZeroUsize,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::ReadChunks>> {
        self.operation(crate::calls::ReadChunks::new(length, maximum))
    }
    fn read_chunks_exact(
        self,
        length: usize,
        maximum: NonZeroUsize,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::ReadChunksExact>> {
        self.operation(crate::calls::ReadChunksExact::new(length, maximum))
    }
}
impl<R: TargetExt> TargetChunkExt for R where R::Target: ReadChunk {}
pub trait TargetWriteExt: TargetExt
where
    Self::Target: Write,
{
    fn write<B: Buf>(
        self,
        input: B,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::Write<B>>> {
        self.operation(crate::calls::Write::new(input))
    }
    fn write_all<B: Buf>(
        self,
        input: B,
    ) -> Child<Self::Root, AdaptedCall<Self, crate::calls::WriteAll<B>>> {
        self.operation(crate::calls::WriteAll::new(input))
    }
}
impl<R: TargetExt> TargetWriteExt for R where R::Target: Write {}

/// A chunk or EOF, preserving the family's stable chunk and error types.
pub type ChunkResult<F> = Result<Option<<F as ReadChunk>::Chunk>, <F as Read>::Error>;

// Keep host borrowing in the ordinary BoundCall adapter; receiver methods above
// retain only their receiver and child state.
macro_rules! host_operation {
    ($name:ident, $unpin:ident, [$($generic:tt)*], ($($arg:ident: $arg_ty:ty),*), $call:ty) => {
        fn $name<'host, $($generic)*>(self: Pin<&'host mut Self>, $($arg: $arg_ty),*)
            -> <Self as nitori_call::ResourceDriver>::Execution<'host, $call>
        where $call: nitori_call::CallOn<Self::Family> {
            nitori_call::ResourceDriver::execute(self, <$call>::new($($arg),*))
        }
        fn $unpin<'host, $($generic)*>(&'host mut self, $($arg: $arg_ty),*)
            -> <Self as nitori_call::ResourceDriver>::Execution<'host, $call>
        where Self: Unpin, $call: nitori_call::CallOn<Self::Family> {
            nitori_call::ResourceDriver::execute(Pin::new(self), <$call>::new($($arg),*))
        }
    };
}
// Avoid a second layer of method inference: raw methods construct directly.
pub trait ReadExt: nitori_call::ResourceDriver
where
    Self::Family: Read,
{
    host_operation!(read, read_unpin, [B: BufMut + ?Sized], (out: &'host mut B), crate::calls::Read<'host,B>);
    host_operation!(read_exact, read_exact_unpin, [B: BufMut + ?Sized], (out: &'host mut B, length:usize), crate::calls::ReadExact<'host,B>);
    host_operation!(read_to_end, read_to_end_unpin, [B: BufMut + ?Sized], (out: &'host mut B), crate::calls::ReadToEnd<'host,B>);
    host_operation!(read_array, read_array_unpin, [const N:usize], (), crate::calls::ReadArray<N>);
    host_operation!(read_le, read_le_unpin, [T:crate::calls::EndianValue], (), crate::calls::ReadLe<T>);
    host_operation!(read_be, read_be_unpin, [T:crate::calls::EndianValue], (), crate::calls::ReadBe<T>);
}
impl<H: nitori_call::ResourceDriver + ?Sized> ReadExt for H where H::Family: Read {}
pub trait ChunkExt: nitori_call::ResourceDriver
where
    Self::Family: ReadChunk,
{
    host_operation!(read_chunk, read_chunk_unpin, [], (maximum:NonZeroUsize), crate::calls::ReadChunk);
    host_operation!(read_chunks, read_chunks_unpin, [], (length:usize, maximum:NonZeroUsize), crate::calls::ReadChunks);
    host_operation!(read_chunks_exact, read_chunks_exact_unpin, [], (length:usize, maximum:NonZeroUsize), crate::calls::ReadChunksExact);
}
impl<H: nitori_call::ResourceDriver + ?Sized> ChunkExt for H where H::Family: ReadChunk {}
pub trait WriteExt: nitori_call::ResourceDriver
where
    Self::Family: Write,
{
    host_operation!(write, write_unpin, [B:Buf], (input:B), crate::calls::Write<B>);
    host_operation!(write_all, write_all_unpin, [B:Buf], (input:B), crate::calls::WriteAll<B>);
}
impl<H: nitori_call::ResourceDriver + ?Sized> WriteExt for H where H::Family: Write {}
