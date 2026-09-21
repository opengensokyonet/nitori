//! Explicit capability adapters with caller-selected allocation limits.
use crate::{
    PollReadExt as _, PollWriteExt as _, Read, ReadChunk, Write, helpers::poll_chunk_from_read,
};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{Direct, Host};

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
impl<T: Host> Read for Direct<Chunked<T>>
where
    T::Family: Read,
{
    type Error = <T::Family as Read>::Error;
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        this.project().inner.poll_read(cx, destination)
    }
}
impl<T: Host> ReadChunk for Direct<Chunked<T>>
where
    T::Family: Read,
{
    type Chunk = Bytes;
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Bytes>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
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
impl<T: Host> Write for Direct<Chunked<T>>
where
    T::Family: Write,
{
    type Error = <T::Family as Write>::Error;
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        this.project().inner.poll_write(cx, input)
    }
}

nitori_call::direct_host!(impl [T] for Chunked<T>);
