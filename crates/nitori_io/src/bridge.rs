//! One-way adapters from standard, Tokio, and futures IO into this crate.
//!
//! Reads use an initialized, bounded 8 KiB scratch buffer to support arbitrary
//! `BufMut` destinations without unsafe code. No data is prefetched or retained
//! across polls. Writes offer the input's first contiguous chunk and advance it
//! only after success. The original `std::io::Error` is preserved.
use crate::{Read, Write};
use bytes::{Buf, BufMut};
use core::{
    pin::Pin,
    task::{Context, Poll, ready},
};
use std::io;

fn read_into<O: BufMut + ?Sized>(
    destination: &mut O,
    read: impl FnOnce(&mut [u8]) -> Poll<io::Result<usize>>,
) -> Poll<io::Result<usize>> {
    let mut scratch = [0; 8192];
    let capacity = destination.remaining_mut().min(scratch.len());
    let count = ready!(read(&mut scratch[..capacity]))?;
    assert!(count <= capacity, "reader exceeded supplied capacity");
    destination.put_slice(&scratch[..count]);
    Poll::Ready(Ok(count))
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
impl<T: io::Read> Read for Std<T> {
    type Error = io::Error;
    fn poll_read<O: BufMut + ?Sized>(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<usize>> {
        read_into(destination, |buffer| {
            Poll::Ready(self.get_mut().inner.read(buffer))
        })
    }
}
impl<T: io::Write> Write for Std<T> {
    type Error = io::Error;
    fn poll_write<I: Buf + ?Sized>(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>> {
        let count = self.get_mut().inner.write(input.chunk())?;
        input.advance(count);
        Poll::Ready(Ok(count))
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
impl<T: tokio::io::AsyncRead> Read for Tokio<T> {
    type Error = io::Error;
    fn poll_read<O: BufMut + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<usize>> {
        read_into(destination, |buffer| {
            let mut buffer = tokio::io::ReadBuf::new(buffer);
            ready!(self.project().inner.poll_read(cx, &mut buffer))?;
            Poll::Ready(Ok(buffer.filled().len()))
        })
    }
}
#[cfg(feature = "tokio")]
impl<T: tokio::io::AsyncWrite> Write for Tokio<T> {
    type Error = io::Error;
    fn poll_write<I: Buf + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>> {
        let count = ready!(self.project().inner.poll_write(cx, input.chunk()))?;
        input.advance(count);
        Poll::Ready(Ok(count))
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
impl<T: futures_io::AsyncRead> Read for Futures<T> {
    type Error = io::Error;
    fn poll_read<O: BufMut + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<io::Result<usize>> {
        read_into(destination, |buffer| {
            self.project().inner.poll_read(cx, buffer)
        })
    }
}
#[cfg(feature = "futures")]
impl<T: futures_io::AsyncWrite> Write for Futures<T> {
    type Error = io::Error;
    fn poll_write<I: Buf + ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<io::Result<usize>> {
        let count = ready!(self.project().inner.poll_write(cx, input.chunk()))?;
        input.advance(count);
        Poll::Ready(Ok(count))
    }
}
