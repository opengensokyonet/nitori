//! In-memory hosts and transparent pointer forwarding.
use crate::{Read, ReadChunk, Write};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use core::{
    convert::Infallible,
    num::NonZeroUsize,
    ops::DerefMut,
    pin::Pin,
    task::{Context, Poll},
};
use std::{collections::VecDeque, io::Cursor};

fn read_buffer<B: Buf + ?Sized, O: BufMut + ?Sized>(source: &mut B, mut out: &mut O) -> usize {
    let count = source.remaining().min(out.remaining_mut());
    BufMut::put(&mut out, source.take(count));
    count
}
fn write_buffer<B: BufMut + ?Sized, I: Buf + ?Sized>(mut sink: &mut B, input: &mut I) -> usize {
    let count = sink.remaining_mut().min(input.remaining());
    BufMut::put(&mut sink, input.take(count));
    count
}
macro_rules! memory_read {
    ($($ty:ty),* $(,)?) => {$ (
        impl Read for $ty {
            type Error = Infallible;
            fn poll_read<O: BufMut + ?Sized>(self: Pin<&mut Self>, _: &mut Context<'_>, out: &mut O) -> Poll<Result<usize, Self::Error>> {
                Poll::Ready(Ok(read_buffer(self.get_mut(), out)))
            }
        }
    )*};
}
memory_read!(&[u8], Bytes, BytesMut, VecDeque<u8>);
macro_rules! memory_write {
    ($($ty:ty),* $(,)?) => {$ (
        impl Write for $ty {
            type Error = Infallible;
            fn poll_write<I: Buf + ?Sized>(self: Pin<&mut Self>, _: &mut Context<'_>, input: &mut I) -> Poll<Result<usize, Self::Error>> {
                Poll::Ready(Ok(write_buffer(self.get_mut(), input)))
            }
        }
    )*};
}
memory_write!(&mut [u8], Vec<u8>, BytesMut);
impl Write for VecDeque<u8> {
    type Error = Infallible;
    fn poll_write<I: Buf + ?Sized>(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Self::Error>> {
        let count = input.remaining();
        let sink = self.get_mut();
        while input.has_remaining() {
            let chunk = input.chunk();
            sink.extend(chunk);
            let length = chunk.len();
            input.advance(length);
        }
        Poll::Ready(Ok(count))
    }
}
impl<T: AsRef<[u8]> + Unpin> Read for Cursor<T> {
    type Error = Infallible;
    fn poll_read<O: BufMut + ?Sized>(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<usize, Self::Error>> {
        Poll::Ready(Ok(read_buffer(self.get_mut(), out)))
    }
}
impl<T: Unpin> Write for Cursor<T>
where
    Cursor<T>: std::io::Write,
{
    type Error = std::io::Error;
    fn poll_write<I: Buf + ?Sized>(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Self::Error>> {
        let count = std::io::Write::write(self.get_mut(), input.chunk())?;
        input.advance(count);
        Poll::Ready(Ok(count))
    }
}
impl<'a> ReadChunk for &'a [u8] {
    type Chunk = &'a [u8];
    fn poll_read_chunk(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Self::Chunk>, Self::Error>> {
        let source = self.get_mut();
        if source.is_empty() {
            return Poll::Ready(Ok(None));
        }
        let (chunk, tail) = source.split_at(maximum.get().min(source.len()));
        *source = tail;
        Poll::Ready(Ok(Some(chunk)))
    }
}
macro_rules! split_chunk {
    ($($ty:ty),*) => {$ (
        impl ReadChunk for $ty {
            type Chunk = Self;
            fn poll_read_chunk(self: Pin<&mut Self>, _: &mut Context<'_>, maximum: NonZeroUsize) -> Poll<Result<Option<Self::Chunk>, Self::Error>> {
                let source = self.get_mut();
                let count = maximum.get().min(source.len());
                Poll::Ready(Ok((count != 0).then(|| source.split_to(count))))
            }
        }
    )*};
}
split_chunk!(Bytes, BytesMut);
macro_rules! forward {
    ([$($generics:tt)*] $ty:ty, $host:ty, $this:ident => $target:expr) => {
        impl<$($generics)*> Read for $ty where $host: Read {
            type Error = <$host as Read>::Error;
            fn poll_read<O: BufMut + ?Sized>(self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut O) -> Poll<Result<usize, Self::Error>> {
                let $this = self; $target.poll_read(cx, out)
            }
        }
        impl<$($generics)*> ReadChunk for $ty where $host: ReadChunk {
            type Chunk = <$host as ReadChunk>::Chunk;
            fn poll_read_chunk(self: Pin<&mut Self>, cx: &mut Context<'_>, maximum: NonZeroUsize) -> Poll<Result<Option<Self::Chunk>, Self::Error>> {
                let $this = self; $target.poll_read_chunk(cx, maximum)
            }
        }
        impl<$($generics)*> Write for $ty where $host: Write {
            type Error = <$host as Write>::Error;
            fn poll_write<I: Buf + ?Sized>(self: Pin<&mut Self>, cx: &mut Context<'_>, input: &mut I) -> Poll<Result<usize, Self::Error>> {
                let $this = self; $target.poll_write(cx, input)
            }
        }
    };
}
forward!([T: ?Sized + Unpin] &mut T, T, this => Pin::new(&mut **this.get_mut()));
forward!([T: ?Sized + Unpin] Box<T>, T, this => Pin::new(&mut **this.get_mut()));
forward!([P: DerefMut + Unpin] Pin<P>, P::Target, this => this.get_mut().as_mut());
