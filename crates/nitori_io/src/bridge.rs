//! Adapters from standard, Tokio, and futures IO into this crate, and from a
//! receiver access inside an operation to the async reader traits.
//!
//! Reads use an initialized, bounded 8 KiB scratch buffer to support arbitrary
//! `BufMut` destinations without unsafe code. No data is prefetched or retained
//! across polls. Writes offer the input's first contiguous chunk and advance it
//! only after success. The original `std::io::Error` is preserved.
use crate::{Read, ReadSegment, Write, WriteSegment};
use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};
use nitori_call::Direct;
use std::io;

fn read_into<O: BufMut + ?Sized>(
    destination: &mut O,
    read: impl FnOnce(&mut [u8]) -> Poll<io::Result<usize>>,
) -> Poll<io::Result<Option<ReadSegment<()>>>> {
    let mut scratch = [0; 8192];
    let capacity = destination.remaining_mut().min(scratch.len());
    let count = ready!(read(&mut scratch[..capacity]))?;
    assert!(count <= capacity, "reader exceeded supplied capacity");
    destination.put_slice(&scratch[..count]);
    Poll::Ready(Ok(NonZeroUsize::new(count).map(|count| ReadSegment {
        count,
        metadata: (),
    })))
}

/// The segment of `count` accepted bytes, or no progress.
fn written(count: usize) -> Option<WriteSegment<()>> {
    NonZeroUsize::new(count).map(|count| WriteSegment { count, report: () })
}

/// Adapt synchronous IO. Polling may block the calling thread.
///
/// `WouldBlock` and `Interrupted` remain errors, never `Pending`: this adapter
/// cannot register readiness notifications. Use an async host on executor threads.
/// Flush and shutdown remain explicit operations on the underlying object.
#[derive(Debug, Default)]
pub struct Std<T> {
    inner: T,
}
// The synchronous inner value is not structurally pinned.
impl<T> Unpin for Std<T> {}
impl<T> Std<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
    pub fn get_ref(&self) -> &T {
        &self.inner
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
}
impl<T> From<T> for Std<T> {
    fn from(inner: T) -> Self {
        Self::new(inner)
    }
}
impl<T: io::Read> Read for Direct<Std<T>> {
    type Error = io::Error;
    type Metadata = ();
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<Option<ReadSegment<()>>>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        read_into(destination, |buffer| {
            Poll::Ready(this.get_mut().inner.read(buffer))
        })
    }
}
impl<T: io::Write> Write for Direct<Std<T>> {
    type Error = io::Error;
    type Report = ();
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<Option<WriteSegment<()>>>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        let count = this.get_mut().inner.write(input.chunk())?;
        input.advance(count);
        Poll::Ready(Ok(written(count)))
    }
}

#[cfg(feature = "tokio")]
pin_project_lite::pin_project! {
    /// Adapt `tokio::io` IO without requiring the host to be `Unpin`.
    ///
    /// Available with the `tokio` feature. Pending and wakeups are delegated to
    /// the host; no runtime, background task, flush, or shutdown is introduced.
    #[derive(Debug, Default)]
    pub struct Tokio<T> {
        #[pin]
        inner: T,
    }
}
#[cfg(feature = "tokio")]
impl<T> Tokio<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
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
}
#[cfg(feature = "tokio")]
impl<T> From<T> for Tokio<T> {
    fn from(inner: T) -> Self {
        Self::new(inner)
    }
}
#[cfg(feature = "tokio")]
impl<T: tokio::io::AsyncRead> Read for Direct<Tokio<T>> {
    type Error = io::Error;
    type Metadata = ();
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<Option<ReadSegment<()>>>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        read_into(destination, |buffer| {
            let mut buffer = tokio::io::ReadBuf::new(buffer);
            ready!(this.project().inner.poll_read(cx, &mut buffer))?;
            Poll::Ready(Ok(buffer.filled().len()))
        })
    }
}
#[cfg(feature = "tokio")]
impl<T: tokio::io::AsyncWrite> Write for Direct<Tokio<T>> {
    type Error = io::Error;
    type Report = ();
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<Option<WriteSegment<()>>>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        let count = ready!(this.project().inner.poll_write(cx, input.chunk()))?;
        input.advance(count);
        Poll::Ready(Ok(written(count)))
    }
}

#[cfg(feature = "futures")]
pin_project_lite::pin_project! {
    /// Adapt `futures_io` IO without requiring the host to be `Unpin`.
    ///
    /// Available with the `futures` feature. Pending and wakeups are delegated to
    /// the host; no runtime, background task, flush, or shutdown is introduced.
    #[derive(Debug, Default)]
    pub struct Futures<T> {
        #[pin]
        inner: T,
    }
}
#[cfg(feature = "futures")]
impl<T> Futures<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
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
}
#[cfg(feature = "futures")]
impl<T> From<T> for Futures<T> {
    fn from(inner: T) -> Self {
        Self::new(inner)
    }
}
#[cfg(feature = "futures")]
impl<T: futures_io::AsyncRead> Read for Direct<Futures<T>> {
    type Error = io::Error;
    type Metadata = ();
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<Option<ReadSegment<()>>>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        read_into(destination, |buffer| {
            this.project().inner.poll_read(cx, buffer)
        })
    }
}
#[cfg(feature = "futures")]
impl<T: futures_io::AsyncWrite> Write for Direct<Futures<T>> {
    type Error = io::Error;
    type Report = ();
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<Option<WriteSegment<()>>>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        let count = ready!(this.project().inner.poll_write(cx, input.chunk()))?;
        input.advance(count);
        Poll::Ready(Ok(written(count)))
    }
}

nitori_call::direct_receiver!(impl [T] for Std<T>);
#[cfg(feature = "tokio")]
nitori_call::direct_receiver!(impl [T] for Tokio<T>);
#[cfg(feature = "futures")]
nitori_call::direct_receiver!(impl [T] for Futures<T>);

/// A receiver access read as `tokio::io::AsyncRead` or `futures_io::AsyncRead`,
/// so that codecs written for those traits can run inside an operation.
///
/// Each poll reads through the access, so the reader uses the view the
/// operation's call holds on that poll; it buffers nothing. Errors of the
/// family's [`Read`] are converted into `std::io::Error`. The async reader
/// traits carry no metadata, so the reader keeps it: the metadata of every
/// segment read is merged with [`Combine`](crate::Combine) into
/// [`metadata`](ReadAccess::metadata), and [`take_metadata`](ReadAccess::take_metadata)
/// hands it over, for example once per message a codec decoded. Available
/// with the `tokio` or `futures` feature.
#[cfg(any(feature = "tokio", feature = "futures"))]
#[derive(Debug)]
pub struct ReadAccess<A, M> {
    access: A,
    metadata: M,
}
#[cfg(any(feature = "tokio", feature = "futures"))]
impl<A, M> ReadAccess<A, M>
where
    A: nitori_call::ReceiverAccess<Family: Read<Metadata = M>>,
    M: crate::Combine,
{
    pub fn new(access: A) -> Self {
        Self {
            access,
            metadata: M::default(),
        }
    }
}
#[cfg(any(feature = "tokio", feature = "futures"))]
impl<A, M> ReadAccess<A, M> {
    pub fn get_ref(&self) -> &A {
        &self.access
    }
    pub fn get_mut(&mut self) -> &mut A {
        &mut self.access
    }
    /// The merged metadata of the segments read since the reader was made or
    /// the metadata was last taken.
    pub fn metadata(&self) -> &M {
        &self.metadata
    }
    /// Hand over the merged metadata, leaving what no segments merge to.
    pub fn take_metadata(&mut self) -> M
    where
        M: Default,
    {
        core::mem::take(&mut self.metadata)
    }
    /// The access, dropping the metadata not taken.
    pub fn into_inner(self) -> A {
        self.access
    }
}
#[cfg(any(feature = "tokio", feature = "futures"))]
impl<A, M> From<A> for ReadAccess<A, M>
where
    A: nitori_call::ReceiverAccess<Family: Read<Metadata = M>>,
    M: crate::Combine,
{
    fn from(access: A) -> Self {
        Self::new(access)
    }
}
#[cfg(any(feature = "tokio", feature = "futures"))]
impl<A, M> ReadAccess<A, M>
where
    A: nitori_call::ReceiverAccess<Family: Read<Error: Into<io::Error>, Metadata = M>> + Unpin,
    M: crate::Combine,
{
    fn poll_slice(&mut self, cx: &mut Context<'_>, buffer: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut destination = buffer;
        let segment = ready!(self.access.poll_access(cx, |access, cx| {
            <A::Family as Read>::poll_read(access.into_pin(), cx, &mut destination)
        }))
        .map_err(Into::into)?;
        Poll::Ready(Ok(segment.map_or(0, |segment| {
            let metadata = core::mem::take(&mut self.metadata);
            self.metadata = metadata.combine(segment.metadata);
            segment.count.get()
        })))
    }
}
#[cfg(feature = "tokio")]
impl<A, M> tokio::io::AsyncRead for ReadAccess<A, M>
where
    A: nitori_call::ReceiverAccess<Family: Read<Error: Into<io::Error>, Metadata = M>> + Unpin,
    M: crate::Combine + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // Zeroes the unfilled part once. Filling `ReadBuf` as a `BufMut`, which
        // needs Tokio's `io-util` feature, would avoid that.
        let count = ready!(self.get_mut().poll_slice(cx, buffer.initialize_unfilled()))?;
        buffer.advance(count);
        Poll::Ready(Ok(()))
    }
}
#[cfg(feature = "futures")]
impl<A, M> futures_io::AsyncRead for ReadAccess<A, M>
where
    A: nitori_call::ReceiverAccess<Family: Read<Error: Into<io::Error>, Metadata = M>> + Unpin,
    M: crate::Combine + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut().poll_slice(cx, buffer)
    }
}
