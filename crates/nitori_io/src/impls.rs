//! In-memory hosts and transparent pointer forwarding.
use crate::{
    ChunkSegment, Filled, LentBuffer, LentOutcome, LentSegment, Read, ReadChunk, ReadSegment, Skip,
    VecSpareCapacity, Write, WriteLent, WriteSegment,
};
use alloc::{collections::VecDeque, vec::Vec};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use core::{
    convert::Infallible,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::Direct;
#[cfg(feature = "std")]
use std::io::Cursor;

fn read_buffer<B: Buf + ?Sized, O: BufMut + ?Sized>(
    source: &mut B,
    mut out: &mut O,
) -> Option<ReadSegment<()>> {
    let count = source.remaining().min(out.remaining_mut());
    BufMut::put(&mut out, source.take(count));
    read_segment(count)
}
fn write_buffer<B: BufMut + ?Sized, I: Buf + ?Sized>(
    mut sink: &mut B,
    input: &mut I,
) -> Option<WriteSegment<()>> {
    let count = sink.remaining_mut().min(input.remaining());
    BufMut::put(&mut sink, input.take(count));
    write_segment(count)
}
/// The segment of `count` bytes without metadata, or no progress.
fn read_segment(count: usize) -> Option<ReadSegment<()>> {
    NonZeroUsize::new(count).map(|count| ReadSegment {
        count,
        metadata: (),
    })
}
/// The segment of `count` accepted bytes without a report, or no progress.
fn write_segment(count: usize) -> Option<WriteSegment<()>> {
    NonZeroUsize::new(count).map(|count| WriteSegment { count, report: () })
}
macro_rules! memory_read {
    ($($ty:ty),* $(,)?) => {$ (
        impl Read for Direct<$ty> {
            type Error = Infallible;
            type Metadata = ();
            fn poll_read<'visit, O: BufMut + ?Sized>(host: Pin<&mut Self::ReceiverView<'visit>>, _: &mut Context<'_>, out: &mut O) -> Poll<Result<Option<ReadSegment<()>>, Self::Error>>  where Self: 'visit {
 let this = host.get_pin_mut();
                Poll::Ready(Ok(read_buffer(this.get_mut(), out)))
            }
        }
    )*};
}
memory_read!(&[u8], Bytes, BytesMut, VecDeque<u8>);
/// Advance the buffer by at most `maximum` bytes, or report EOF.
fn skip_buffer<B: Buf + ?Sized, M: crate::Min<usize, Output = usize>>(
    source: &mut B,
    maximum: M,
) -> Option<usize> {
    let available = source.remaining();
    if available == 0 {
        return None;
    }
    let count = crate::Min::min(maximum, available);
    source.advance(count);
    Some(count)
}
macro_rules! memory_skip {
    ($($ty:ty),* $(,)?) => {$(
        impl Skip for Direct<$ty> {
            type Count = usize;
            fn poll_skip<'visit, M: crate::Min<usize, Output = usize> + Clone>(
                host: Pin<&mut Self::ReceiverView<'visit>>,
                _: &mut Context<'_>,
                maximum: M,
            ) -> Poll<Result<Option<usize>, Self::Error>>
            where
                Self: 'visit,
            {
                Poll::Ready(Ok(skip_buffer(host.get_pin_mut().get_mut(), maximum)))
            }
        }
    )*};
}
memory_skip!(&[u8], Bytes, BytesMut, VecDeque<u8>);
#[cfg(feature = "std")]
impl<T: AsRef<[u8]> + Unpin> Skip for Direct<Cursor<T>> {
    type Count = usize;
    fn poll_skip<'visit, M: crate::Min<usize, Output = usize> + Clone>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<usize>, Self::Error>>
    where
        Self: 'visit,
    {
        Poll::Ready(Ok(skip_buffer(host.get_pin_mut().get_mut(), maximum)))
    }
}
macro_rules! memory_write {
    ($($ty:ty),* $(,)?) => {$ (
        impl Write<()> for Direct<$ty> {
            type Error = !;
            type Report = ();
            fn poll_write<'visit, I: Buf + ?Sized>(host: Pin<&mut Self::ReceiverView<'visit>>, _: &mut Context<'_>, input: &mut I, (): &()) -> Poll<Result<Option<WriteSegment<()>>, !>>  where Self: 'visit {
 let this = host.get_pin_mut();
                Poll::Ready(Ok(write_buffer(this.get_mut(), input)))
            }
        }
    )*};
}
memory_write!(&mut [u8], Vec<u8>, BytesMut);
impl Write<()> for Direct<VecDeque<u8>> {
    type Error = !;
    type Report = ();
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        input: &mut I,
        (): &(),
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        let count = input.remaining();
        let sink = this.get_mut();
        while input.has_remaining() {
            let chunk = input.chunk();
            sink.extend(chunk);
            let length = chunk.len();
            input.advance(length);
        }
        Poll::Ready(Ok(write_segment(count)))
    }
}
#[cfg(feature = "std")]
impl<T: AsRef<[u8]> + Unpin> Read for Direct<Cursor<T>> {
    type Error = Infallible;
    type Metadata = ();
    fn poll_read<'visit, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<Option<ReadSegment<()>>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        Poll::Ready(Ok(read_buffer(this.get_mut(), out)))
    }
}
#[cfg(feature = "std")]
impl<T: Unpin> Write<()> for Direct<Cursor<T>>
where
    Cursor<T>: std::io::Write,
{
    type Error = std::io::Error;
    type Report = ();
    fn poll_write<'visit, I: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        input: &mut I,
        (): &(),
    ) -> Poll<Result<Option<WriteSegment<()>>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        let count = std::io::Write::write(this.get_mut(), input.chunk())?;
        input.advance(count);
        Poll::Ready(Ok(write_segment(count)))
    }
}
impl<'a> ReadChunk for Direct<&'a [u8]> {
    type Chunk = &'a [u8];
    fn poll_read_chunk<'visit>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<ChunkSegment<Self::Chunk, ()>>, Self::Error>>
    where
        Self: 'visit,
    {
        let this = host.get_pin_mut();
        let source = this.get_mut();
        if source.is_empty() {
            return Poll::Ready(Ok(None));
        }
        let (chunk, tail) = source.split_at(maximum.get().min(source.len()));
        *source = tail;
        Poll::Ready(Ok(Some(ChunkSegment {
            chunk,
            metadata: (),
        })))
    }
}
macro_rules! split_chunk {
    ($($ty:ty),*) => {$ (
        impl ReadChunk for Direct<$ty> {
            type Chunk = $ty;
            fn poll_read_chunk<'visit>(host: Pin<&mut Self::ReceiverView<'visit>>, _: &mut Context<'_>, maximum: NonZeroUsize) -> Poll<Result<Option<ChunkSegment<Self::Chunk, ()>>, Self::Error>>  where Self: 'visit {
 let this = host.get_pin_mut();
                let source = this.get_mut();
                let count = maximum.get().min(source.len());
                Poll::Ready(Ok((count != 0).then(|| ChunkSegment {
                    chunk: source.split_to(count),
                    metadata: (),
                })))
            }
        }
    )*};
}
split_chunk!(Bytes, BytesMut);

/// Lends the spare capacity after the written bytes, reserving the grant
/// first, and keeps the filled bytes there without copying them. The lent
/// buffer starts empty and is split off the written bytes, so the fill cannot
/// reach them. Accepts no byte only at its maximum length.
impl WriteLent<()> for Direct<BytesMut> {
    type Buffer<'l> = &'l mut BytesMut;
    fn poll_write_lent<'visit, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<(), T, A>>, Self::Error>>
    where
        Self: 'visit,
        Fill: for<'l> FnOnce(&mut LentBuffer<&'l mut BytesMut>) -> Result<Filled<T, ()>, A>,
    {
        let sink = host.get_pin_mut().get_mut();
        let Some(granted) = NonZeroUsize::new(wanted.get().min(sink.remaining_mut())) else {
            return Poll::Ready(Ok(None));
        };
        sink.reserve(granted.get());
        let mut spare = sink.split_off(sink.len());
        // Invariant: an empty buffer has room for any grant below its maximum
        // length.
        let mut lent = LentBuffer::new(&mut spare, granted).expect("the grant fits the room");
        let result = fill(&mut lent);
        // What is kept is what the lent buffer holds.
        let kept = NonZeroUsize::new(spare.len()).filter(|count| *count <= granted);
        let outcome = match (result, kept) {
            (Ok(Filled { value, options: () }), Some(count)) => LentOutcome::Kept(LentSegment {
                count,
                report: (),
                value,
            }),
            (Ok(Filled { value, options: () }), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => LentOutcome::GaveUp(abandoned),
        };
        if !matches!(outcome, LentOutcome::Kept(_)) {
            // Nothing of a loan not kept stays written.
            spare.clear();
        }
        // Joins without copying when the spare capacity was not reallocated.
        sink.unsplit(spare);
        Poll::Ready(Ok(Some(outcome)))
    }
}

/// Lends the spare capacity after the written bytes, reserving the grant
/// first, and keeps the filled bytes there without copying or zeroing them.
/// The lent buffer reaches only the spare capacity, not the bytes written
/// before. Accepts no byte only at its maximum length.
impl WriteLent<()> for Direct<Vec<u8>> {
    type Buffer<'l> = VecSpareCapacity<'l>;
    fn poll_write_lent<'visit, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<(), T, A>>, Self::Error>>
    where
        Self: 'visit,
        Fill: for<'l> FnOnce(&mut LentBuffer<VecSpareCapacity<'l>>) -> Result<Filled<T, ()>, A>,
    {
        let sink = host.get_pin_mut().get_mut();
        let Some(granted) = NonZeroUsize::new(wanted.get().min(sink.remaining_mut())) else {
            return Poll::Ready(Ok(None));
        };
        // Invariant: the spare capacity is lent for exactly the grant.
        let mut lent = LentBuffer::new(VecSpareCapacity::new(sink, granted), granted)
            .expect("the grant fits the room");
        let result = fill(&mut lent);
        let spare = lent.into_storage();
        let outcome = match (result, NonZeroUsize::new(spare.filled())) {
            (Ok(Filled { value, options: () }), Some(count)) => {
                spare.keep();
                LentOutcome::Kept(LentSegment {
                    count,
                    report: (),
                    value,
                })
            }
            // What was put and not kept stays outside the vector's length.
            (Ok(Filled { value, options: () }), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => LentOutcome::GaveUp(abandoned),
        };
        Poll::Ready(Ok(Some(outcome)))
    }
}

/// The bytes put into a lent slice that was `room` bytes long and ended at
/// `end`, at most `granted`: putting bytes advances the slice's start and
/// keeps its end, so any other slice left there is a broken loan.
fn kept_from_slice(
    rest: &[u8],
    end: *const u8,
    room: usize,
    granted: NonZeroUsize,
) -> Option<NonZeroUsize> {
    if rest.as_ptr_range().end != end {
        return None;
    }
    room.checked_sub(rest.len())
        .filter(|count| *count <= granted.get())
        .and_then(NonZeroUsize::new)
}

/// Lends the unwritten rest of the slice and advances past the filled bytes.
/// Accepts no byte once the slice is used up.
impl WriteLent<()> for Direct<&mut [u8]> {
    type Buffer<'l> = &'l mut [u8];
    fn poll_write_lent<'visit, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<(), T, A>>, Self::Error>>
    where
        Self: 'visit,
        Fill: for<'l> FnOnce(&mut LentBuffer<&'l mut [u8]>) -> Result<Filled<T, ()>, A>,
    {
        let sink = host.get_pin_mut().get_mut();
        let room = sink.len();
        let Some(granted) = NonZeroUsize::new(wanted.get().min(room)) else {
            return Poll::Ready(Ok(None));
        };
        let end = sink.as_ptr_range().end;
        // The fill writes through a reborrow, so the slice advances only past
        // what is kept. Invariant: the grant is at most the slice's length.
        let mut lent = LentBuffer::new(&mut **sink, granted).expect("the grant fits the room");
        let result = fill(&mut lent);
        let kept = kept_from_slice(lent.into_storage(), end, room, granted);
        let outcome = match (result, kept) {
            (Ok(Filled { value, options: () }), Some(count)) => {
                let rest = core::mem::take(sink);
                *sink = &mut rest[count.get()..];
                LentOutcome::Kept(LentSegment {
                    count,
                    report: (),
                    value,
                })
            }
            (Ok(Filled { value, options: () }), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => LentOutcome::GaveUp(abandoned),
        };
        Poll::Ready(Ok(Some(outcome)))
    }
}

macro_rules! memory_len {
    ($($ty:ty),* $(,)?) => {$(
        impl crate::AtEnd for Direct<$ty> {
            type RemainderError = !;

            fn poll_at_end<'v>(
                host: Pin<&mut Self::ReceiverView<'v>>,
                _: &mut Context<'_>,
            ) -> Poll<Result<bool, !>>
            where Self: 'v {
                Poll::Ready(Ok(!host.get_pin_mut().has_remaining()))
            }
        }

        impl crate::ExactLen for Direct<$ty> {
            type Length = usize;

            fn poll_remaining_len<'v>(
                host: Pin<&mut Self::ReceiverView<'v>>,
                _: &mut Context<'_>,
            ) -> Poll<Result<usize, !>>
            where Self: 'v {
                Poll::Ready(Ok(host.get_pin_mut().remaining()))
            }
        }
    )*};
}
memory_len!(&[u8], Bytes, BytesMut, VecDeque<u8>);

#[cfg(feature = "std")]
mod cursor_length {
    use super::*;
    use alloc::boxed::Box;

    // Cursor::remaining consults AsRef on each call. Implement the length
    // promise for byte storage whose AsRef exposes a stable length.
    #[allow(private_bounds)]
    impl<T: StableBytes + Unpin> crate::AtEnd for Direct<Cursor<T>> {
        type RemainderError = !;

        fn poll_at_end<'v>(
            host: Pin<&mut Self::ReceiverView<'v>>,
            _: &mut Context<'_>,
        ) -> Poll<Result<bool, !>>
        where
            Self: 'v,
        {
            Poll::Ready(Ok(!host.get_pin_mut().has_remaining()))
        }
    }

    #[allow(private_bounds)]
    impl<T: StableBytes + Unpin> crate::ExactLen for Direct<Cursor<T>> {
        type Length = usize;
        fn poll_remaining_len<'v>(
            host: Pin<&mut Self::ReceiverView<'v>>,
            _: &mut Context<'_>,
        ) -> Poll<Result<usize, !>>
        where
            Self: 'v,
        {
            Poll::Ready(Ok(host.get_pin_mut().remaining()))
        }
    }

    trait StableBytes: AsRef<[u8]> {}
    impl StableBytes for [u8] {}
    impl<const N: usize> StableBytes for [u8; N] {}
    impl StableBytes for Vec<u8> {}
    impl StableBytes for Box<[u8]> {}
    impl StableBytes for Bytes {}
    impl StableBytes for BytesMut {}
    impl<T: StableBytes + ?Sized> StableBytes for &T {}
    impl<T: StableBytes + ?Sized> StableBytes for &mut T {}
}
