//! In-memory hosts and transparent pointer forwarding.
use crate::{Read, ReadChunk, Write};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use core::{
    convert::Infallible,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::Direct;
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
        impl Read for Direct<$ty> {
            type Error = Infallible;
            fn poll_read<'visit, O: BufMut + ?Sized>(host: Pin<&mut Self::ReceiverView<'visit>>, _: &mut Context<'_>, out: &mut O) -> Poll<Result<usize, Self::Error>>  where Self: 'visit {
 let this = host.get_mut().0.as_mut();
                Poll::Ready(Ok(read_buffer(this.get_mut(), out)))
            }
        }
    )*};
}
memory_read!(&[u8], Bytes, BytesMut, VecDeque<u8>);
macro_rules! memory_write {
    ($($ty:ty),* $(,)?) => {$ (
        impl Write for Direct<$ty> {
            type Error = Infallible;
            fn poll_write<'visit, I: Buf + ?Sized>(host: Pin<&mut Self::ReceiverView<'visit>>, _: &mut Context<'_>, input: &mut I) -> Poll<Result<usize, Self::Error>>  where Self: 'visit {
 let this = host.get_mut().0.as_mut();
                Poll::Ready(Ok(write_buffer(this.get_mut(), input)))
            }
        }
    )*};
}
memory_write!(&mut [u8], Vec<u8>, BytesMut);
impl Write for Direct<VecDeque<u8>> {
    type Error = Infallible;
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        let count = input.remaining();
        let sink = this.get_mut();
        while input.has_remaining() {
            let chunk = input.chunk();
            sink.extend(chunk);
            let length = chunk.len();
            input.advance(length);
        }
        Poll::Ready(Ok(count))
    }
}
impl<T: AsRef<[u8]> + Unpin> Read for Direct<Cursor<T>> {
    type Error = Infallible;
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        Poll::Ready(Ok(read_buffer(this.get_mut(), out)))
    }
}
impl<T: Unpin> Write for Direct<Cursor<T>>
where
    Cursor<T>: std::io::Write,
{
    type Error = std::io::Error;
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        input: &mut I,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        let count = std::io::Write::write(this.get_mut(), input.chunk())?;
        input.advance(count);
        Poll::Ready(Ok(count))
    }
}
impl<'a> ReadChunk for Direct<&'a [u8]> {
    type Chunk = &'a [u8];
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Self::Chunk>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_mut().0.as_mut();
        let source = this.get_mut();
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
        impl ReadChunk for Direct<$ty> {
            type Chunk = $ty;
            fn poll_read_chunk<'visit>(host: Pin<&mut Self::ReceiverView<'visit>>, _: &mut Context<'_>, maximum: NonZeroUsize) -> Poll<Result<Option<Self::Chunk>, Self::Error>>  where Self: 'visit {
 let this = host.get_mut().0.as_mut();
                let source = this.get_mut();
                let count = maximum.get().min(source.len());
                Poll::Ready(Ok((count != 0).then(|| source.split_to(count))))
            }
        }
    )*};
}
split_chunk!(Bytes, BytesMut);
