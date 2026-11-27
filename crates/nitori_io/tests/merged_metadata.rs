//! Reading through merged metadata: the composed access reports none, and
//! the state holds what the reads carried.
#![feature(impl_trait_in_assoc_type)]
use bytes::{Buf, BufMut, Bytes};
use core::{
    num::NonZeroUsize,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::{AccessPlan, ProvideAccess, Receiver, ReceiverAccess, call};
use nitori_io::{
    ChunkSegment, Combine, ExactLen, Infinite, Read, ReadChunk, ReadSegment, Skip,
    adapters::{MergedMetadata, Merging},
    prelude::*,
};

/// Whether any of the bytes arrived in 0-RTT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ZeroRtt(bool);
impl Combine for ZeroRtt {
    fn combine(self, next: Self) -> Self {
        Self(self.0 || next.0)
    }
}

/// A received stream that hands out at most two bytes at a time; its first
/// `early` bytes arrived in 0-RTT.
struct Received {
    data: Bytes,
    early: usize,
}
impl Received {
    fn new(data: &'static [u8], early: usize) -> Self {
        Self {
            data: Bytes::from_static(data),
            early,
        }
    }
    /// The next segment's length, at most `room`, and its metadata.
    fn next(&mut self, room: usize) -> Option<(NonZeroUsize, ZeroRtt)> {
        let early = self.early > 0;
        let bound = if early { self.early } else { 2 };
        let count = NonZeroUsize::new(self.data.len().min(bound).min(2).min(room))?;
        self.early = self.early.saturating_sub(count.get());
        Some((count, ZeroRtt(early)))
    }
}
nitori_call::family_receiver!(impl [] for Received);
impl Read for Received {
    type Error = !;
    type Metadata = ZeroRtt;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<Option<ReadSegment<ZeroRtt>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        Poll::Ready(Ok(this.next(out.remaining_mut()).map(
            |(count, metadata)| {
                out.put_slice(&this.data.split_to(count.get()));
                ReadSegment { count, metadata }
            },
        )))
    }
}
impl ReadChunk for Received {
    type Chunk = Bytes;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<ChunkSegment<Bytes, ZeroRtt>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        Poll::Ready(Ok(this.next(maximum.get()).map(|(count, metadata)| {
            ChunkSegment {
                chunk: this.data.split_to(count.get()),
                metadata,
            }
        })))
    }
}
impl Skip for Received {
    type Count = usize;
    fn poll_skip<'v, M: nitori_io::Min<usize, Output = usize> + Clone>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        maximum: M,
    ) -> Poll<Result<Option<usize>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let count = maximum.min(this.data.len());
        this.data.advance(count);
        this.early = this.early.saturating_sub(count);
        Poll::Ready(Ok((count > 0).then_some(count)))
    }
}
impl ExactLen for Received {
    type Length = usize;
    type LengthError = !;
    fn poll_remaining_len<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
    ) -> Poll<Result<usize, !>>
    where
        Self: 'v,
    {
        Poll::Ready(Ok(host.get_pin_mut().data.len()))
    }
}

/// Reads `N` bytes from a source without metadata.
#[call(sync)]
async fn plain<const N: usize, F: Read<Metadata = ()>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> [u8; N]
where
    <F as Read>::Error: core::fmt::Debug,
{
    io.read_array::<N>().await.unwrap().value
}

#[call(sync)]
async fn merged<F: Read<Metadata = ZeroRtt>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> ([u8; 3], ZeroRtt, [u8; 3], ZeroRtt)
where
    <F as Read>::Error: core::fmt::Debug,
{
    let mut first = MergedMetadata::new(&mut *io, Default::default());
    let bytes = plain::<3, _>(&mut first).await;
    let early = first.into_parts().merged;
    let mut second = MergedMetadata::new(&mut *io, Default::default());
    let later = plain::<3, _>(&mut second).await;
    (bytes, early, later, second.into_parts().merged)
}

#[test]
fn reads_through_the_state_merge_their_metadata_there() {
    let mut stream = Received::new(b"abcdef", 2);
    assert_eq!(
        stream.sync_merged_unpin(),
        (*b"abc", ZeroRtt(true), *b"def", ZeroRtt(false))
    );
}

#[call(sync)]
async fn chunks_skip_and_length<F>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (Option<Bytes>, Option<usize>, usize, ZeroRtt)
where
    F: ReadChunk<Metadata = ZeroRtt, Chunk = Bytes>
        + Skip<Count = usize>
        + ExactLen<Length = usize, LengthError = !>,
    <F as Read>::Error: core::fmt::Debug,
{
    let mut merging = MergedMetadata::new(&mut *io, Default::default());
    let chunk = merging.read_chunk(NonZeroUsize::MIN).await.unwrap();
    let chunk = chunk.map(
        |ChunkSegment {
             chunk,
             metadata: (),
         }| chunk,
    );
    let skipped = merging.skip(Infinite).await.unwrap();
    let Ok(left) = merging.remaining_len().await;
    (chunk, skipped, left, merging.into_parts().merged)
}

#[test]
fn chunks_merge_while_skips_and_lengths_pass_through() {
    let mut stream = Received::new(b"abc", 1);
    assert_eq!(
        stream.sync_chunks_skip_and_length_unpin(),
        (Some(Bytes::from_static(b"a")), Some(2), 0, ZeroRtt(true))
    );
}

#[call(sync)]
async fn skip_all<F: Skip<Metadata = ZeroRtt, Count = usize>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (Option<usize>, ZeroRtt)
where
    <F as Read>::Error: core::fmt::Debug,
{
    let mut merging = MergedMetadata::new(&mut *io, Default::default());
    let skipped = merging.skip(Infinite).await.unwrap();
    (skipped, merging.into_parts().merged)
}

#[test]
fn skipped_bytes_merge_nothing() {
    let mut stream = Received::new(b"abc", 3);
    assert_eq!(stream.sync_skip_all_unpin(), (Some(3), ZeroRtt(false)));
}

#[test]
fn state_kept_with_a_receiver_merges_every_call() {
    let mut io = MergedMetadata::new(Received::new(b"abcd", 1), ZeroRtt::default());
    assert_eq!(io.sync_plain_unpin::<2>(), *b"ab");
    assert_eq!(io.sync_plain_unpin::<2>(), *b"cd");
    assert_eq!(io.into_parts().merged, ZeroRtt(true));
}

/// A type of the caller's that lends an access to the stream it keeps.
struct Holder<A> {
    io: A,
}
impl<A: ReceiverAccess> ProvideAccess for Holder<A> {
    type AccessFamily = A::Family;
    type Lentness = <A as ProvideAccess>::Lentness;
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = A::Family, Lentness = Self::Lentness> {
        self.io.access_plan()
    }
}

#[call(sync)]
async fn read_through_holder<F: Read<Metadata = ZeroRtt, Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> ([u8; 1], [u8; 2], ZeroRtt) {
    let mut holder = Holder { io };
    let mut state = ZeroRtt::default();
    let mut source = MergedMetadata::borrowing(&mut holder, &mut state);
    let first = source.read_array::<1>().await.unwrap();
    let rest = source.read_array::<2>().await.unwrap();
    (first.value, rest.value, state)
}

#[test]
fn a_merging_wrapper_borrows_an_access_and_accumulator() {
    let result = Received::new(b"xyz", 2).sync_read_through_holder_unpin();
    assert_eq!(result, (*b"x", *b"yz", ZeroRtt(true)));
}

/// The indices of the segments, in the order merged: a metadata whose
/// merge is not commutative.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Order(Vec<u8>);
impl Combine for Order {
    fn combine(mut self, next: Self) -> Self {
        self.0.extend(next.0);
        self
    }
}

/// A source of one-byte segments, each numbered in order.
struct Numbered {
    data: Bytes,
    next: u8,
}
nitori_call::family_receiver!(impl [] for Numbered);
impl Numbered {
    fn take(&mut self, room: usize) -> Option<(Bytes, Order)> {
        if room == 0 || self.data.is_empty() {
            return None;
        }
        let index = self.next;
        self.next += 1;
        Some((self.data.split_to(1), Order(vec![index])))
    }
}
impl Read for Numbered {
    type Error = !;
    type Metadata = Order;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<Option<ReadSegment<Order>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        Poll::Ready(Ok(this.take(out.remaining_mut()).map(
            |(byte, metadata)| {
                out.put_slice(&byte);
                ReadSegment {
                    count: NonZeroUsize::MIN,
                    metadata,
                }
            },
        )))
    }
}
impl ReadChunk for Numbered {
    type Chunk = Bytes;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<ChunkSegment<Bytes, Order>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        Poll::Ready(Ok(this
            .take(maximum.get())
            .map(|(chunk, metadata)| ChunkSegment { chunk, metadata })))
    }
}

#[call(sync)]
async fn in_order<F: ReadChunk<Metadata = Order>>(io: &mut impl ReceiverAccess<Family = F>) -> Order
where
    <F as Read>::Error: core::fmt::Debug,
{
    let mut merging = MergedMetadata::new(&mut *io, Default::default());
    merging.read_array::<2>().await.unwrap();
    merging.read_chunk(NonZeroUsize::MIN).await.unwrap();
    merging.into_parts().merged
}

#[test]
fn segments_merge_in_the_order_they_were_read() {
    let mut source = Numbered {
        data: Bytes::from_static(b"abc"),
        next: 0,
    };
    assert_eq!(source.sync_in_order_unpin(), Order(vec![0, 1, 2]));
}

/// A source that answers Pending, then fails, then reads a byte.
struct Flaky {
    polls: usize,
}
nitori_call::family_receiver!(impl [] for Flaky);
impl Read for Flaky {
    type Error = ();
    type Metadata = Order;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<Option<ReadSegment<Order>>, ()>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        this.polls += 1;
        match this.polls {
            1 => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            2 => Poll::Ready(Err(())),
            _ => {
                out.put_u8(b'x');
                Poll::Ready(Ok(Some(ReadSegment {
                    count: NonZeroUsize::MIN,
                    metadata: Order(vec![9]),
                })))
            }
        }
    }
}

#[test]
fn pending_and_errors_merge_nothing() {
    let mut host = Flaky { polls: 0 };
    let mut state = Order::default();
    let mut output = Vec::new();
    for want in [Some(vec![]), Some(vec![]), Some(vec![9])] {
        let view = MergedMetadata::borrowing(Pin::new(&mut host).view(), &mut state);
        let _ = <Merging<Flaky, Order> as Read>::poll_read(
            pin!(view),
            &mut Context::from_waker(Waker::noop()),
            &mut output,
        );
        assert_eq!(Some(state.0.clone()), want);
    }
    assert_eq!(output, b"x");
}

#[test]
fn a_borrowed_quota_and_accumulator_survive_a_call_on_a_generic_receiver() {
    use core::future::Future;
    use nitori_io::adapters::ReadLimit;

    fn require_send<F: Future + Send>(future: F) -> F {
        future
    }

    fn read<R>(
        receiver: &mut R,
        quota: &mut u16,
        merged: &mut ZeroRtt,
    ) -> impl Future<Output = [u8; 2]> + Send
    where
        R: Receiver<Family: Read<Metadata = ZeroRtt, Error = !>> + Unpin + Send,
    {
        require_send(async move {
            let limited = ReadLimit::borrowing(receiver, quota);
            let mut source = MergedMetadata::borrowing(limited, merged);
            source.read_array_unpin::<2>().await.unwrap().value
        })
    }

    let mut source = Received::new(b"abc", 1);
    let mut quota = 3;
    let mut merged = ZeroRtt::default();
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(
        pin!(read(&mut source, &mut quota, &mut merged)).poll(&mut cx),
        Poll::Ready(*b"ab")
    );
    assert_eq!(source.data, b"c".as_slice());
    assert_eq!(quota, 1);
    assert_eq!(merged, ZeroRtt(true));
}
