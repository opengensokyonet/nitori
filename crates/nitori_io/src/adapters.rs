//! IO wrappers for chunk allocation, byte budgets, skipping, metadata
//! accumulation, and write options mapping.
use crate::{
    ChunkResult, Filled, LentBuffer, PollReadExt as _, PollWriteExt as _, Read, ReadChunk,
    ReadResult, Skip, Write, WriteLent, WriteLentResult, WriteResult,
    helpers::poll_chunk_from_read,
};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{Direct, Lent, Receiver};

#[macro_use]
mod forward;
mod merged_metadata;
mod options;
mod read_limit;
mod skip;
mod write_limit;
pub use merged_metadata::{MergedMetadata, Merging};
pub use options::{FixedOptions, MapOptions, MappedOptions, OptionsMapping};
pub use read_limit::{Limited, ReadLimit};
pub use skip::{ReadChunkSkip, ReadSkip, Skippable};
pub use write_limit::{WriteLimit, WriteLimited};

pin_project_lite::pin_project! {
    /// Add copying chunk reads to any reader, with a shared underlying cursor.
    ///
    /// Each allocation requests at most `chunk_capacity` bytes (an allocator may
    /// round up). Pending retains empty storage, replacing it if a later larger
    /// request needs more capacity. Successful reads transfer its ownership. EOF and
    /// errors release it. Ordinary reads and writes pass through
    /// without prefetching, so alternating operations cannot hide an unread tail.
    /// Dropping the adapter releases scratch storage without consuming more input.
    /// Allocation uses the ordinary infallible `BytesMut` allocation policy.
    #[derive(Debug)]
    pub struct Chunked<T> {
        #[pin]
        inner: T,
        chunk_capacity: NonZeroUsize,
        storage: Option<BytesMut>,
    }
}
impl<T> Chunked<T> {
    pub fn new(inner: T, chunk_capacity: NonZeroUsize) -> Self {
        Self {
            inner,
            chunk_capacity,
            storage: None,
        }
    }
    pub fn get_ref(&self) -> &T {
        &self.inner
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut T> {
        self.project().inner
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
    pub fn chunk_capacity(&self) -> NonZeroUsize {
        self.chunk_capacity
    }
}
impl<T: Receiver> Read for Direct<Chunked<T>>
where
    T::Family: Read,
{
    type Error = <T::Family as Read>::Error;
    type Metadata = <T::Family as Read>::Metadata;
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<ReadResult<Self>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        this.project().inner.poll_read(cx, destination)
    }
}
impl<T: Receiver> ReadChunk for Direct<Chunked<T>>
where
    T::Family: Read,
{
    type Chunk = Bytes;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<ChunkResult<Self>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        let this = this.project();
        let maximum = maximum.min(*this.chunk_capacity);
        // Avoid geometric growth beyond the configured allocation limit when
        // a cancelled/suspended request is followed by a larger one.
        if this
            .storage
            .as_ref()
            .is_some_and(|storage| storage.capacity() < maximum.get())
        {
            *this.storage = None;
        }
        poll_chunk_from_read(
            this.inner,
            cx,
            maximum,
            this.storage,
            BytesMut::with_capacity,
            BytesMut::freeze,
        )
    }
}
/// Skips on the inner source. Chunk storage holds no data between polls, so
/// nothing buffered is passed over or left behind.
impl<T: Receiver> Skip for Direct<Chunked<T>>
where
    T::Family: Skip,
{
    type Count = <T::Family as Skip>::Count;
    fn poll_skip<'visit, M: crate::Min<Self::Count, Output = Self::Count> + Clone>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<Self::Count>, Self::Error>>
    where
        Self: 'visit,
    {
        let inner = host.get_pin_mut().project().inner;
        <T::Family as Skip>::poll_skip(core::pin::pin!(inner.view()), cx, maximum)
    }
}
impl<T: Receiver> crate::ExactLen for Direct<Chunked<T>>
where
    T::Family: crate::ExactLen,
{
    type Length = <T::Family as crate::ExactLen>::Length;
    type LengthError = <T::Family as crate::ExactLen>::LengthError;
    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Length, Self::LengthError>>
    where
        Self: 'v,
    {
        let inner = host.get_pin_mut().project().inner;
        <T::Family as crate::ExactLen>::poll_remaining_len(core::pin::pin!(inner.view()), cx)
    }
}
impl<T: Receiver, O> Write<O> for Direct<Chunked<T>>
where
    T::Family: Write<O>,
{
    type Error = <T::Family as Write<O>>::Error;
    type Report = <T::Family as Write<O>>::Report;
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut I,
        options: &O,
    ) -> Poll<WriteResult<Self, O>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        this.project().inner.poll_write(cx, input, options)
    }
}
impl<T: Receiver, O> WriteLent<O> for Direct<Chunked<T>>
where
    T::Family: WriteLent<O>,
{
    type Buffer<'l> = <T::Family as WriteLent<O>>::Buffer<'l>;
    fn poll_write_lent<'visit, U, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<WriteLentResult<Self, O, U, A>>
    where
        Self: 'visit,
        Fill: for<'l> FnOnce(&mut LentBuffer<Self::Buffer<'l>>) -> Result<Filled<U, O>, A>,
    {
        let this = host.get_pin_mut();
        this.project().inner.poll_write_lent(cx, wanted, fill)
    }
}

nitori_call::direct_receiver!(impl [T] for Chunked<T>);

// A loan forwards the capabilities of the view it lends.

/// Reads from the lent view.
impl<F: Read> Read for Lent<F> {
    type Error = F::Error;
    type Metadata = F::Metadata;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<ReadResult<Self>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_read(parent.into_pin(), cx, destination))
    }
}
/// Reads chunks from the lent view.
impl<F: ReadChunk> ReadChunk for Lent<F> {
    type Chunk = F::Chunk;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<ChunkResult<Self>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_read_chunk(parent.into_pin(), cx, maximum))
    }
}
/// Writes to the lent view.
impl<F: Write<O>, O> Write<O> for Lent<F> {
    type Error = F::Error;
    type Report = F::Report;
    fn poll_write<'v, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        input: &mut I,
        options: &O,
    ) -> Poll<WriteResult<Self, O>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_write(parent.into_pin(), cx, input, options))
    }
}
/// Lends the storage of the lent view.
impl<F: WriteLent<O>, O> WriteLent<O> for Lent<F> {
    type Buffer<'l> = F::Buffer<'l>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<WriteLentResult<Self, O, T, A>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<Self::Buffer<'l>>) -> Result<Filled<T, O>, A>,
    {
        host.get_mut()
            .with(|parent| F::poll_write_lent(parent.into_pin(), cx, wanted, fill))
    }
}
/// Skips on the lent view.
impl<F: Skip> Skip for Lent<F> {
    type Count = F::Count;
    fn poll_skip<'v, M: crate::Min<F::Count, Output = F::Count> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<F::Count>, Self::Error>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_skip(parent.into_pin(), cx, maximum))
    }
}
/// The exact length of the lent view.
impl<F: crate::ExactLen> crate::ExactLen for Lent<F> {
    type Length = F::Length;
    type LengthError = F::LengthError;
    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<F::Length, F::LengthError>>
    where
        Self: 'v,
    {
        host.get_mut()
            .with(|parent| F::poll_remaining_len(parent.into_pin(), cx))
    }
}
