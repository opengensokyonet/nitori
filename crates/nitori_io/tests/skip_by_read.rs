//! Skipping composed on readers: by reading into scratch storage, and by
//! dropping chunks, on an access and on a receiver.
#![feature(impl_trait_in_assoc_type)]
use bytes::{BufMut, Bytes};
use core::{
    convert::Infallible,
    num::NonZeroUsize,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::{Call, CallPoll, ComposedReceiver, Lend, Receiver, ReceiverAccess, call};
use nitori_io::{
    Infinite,
    adapters::{Chunked, Limited, ReadChunkSkip, ReadLimit, ReadSkip, Skippable},
    bridge::Std,
    calls::{SkipExact, SkipExactError},
    prelude::*,
};
use std::io::Cursor;

/// A reader without native skipping.
fn reader(bytes: &[u8]) -> Std<Cursor<Vec<u8>>> {
    Std::new(Cursor::new(bytes.to_vec()))
}

/// A chunk reader without native skipping, counting its chunk requests.
struct Chunks {
    bytes: Vec<u8>,
    chunk_requests: usize,
}
nitori_call::family_receiver!(impl [] for Chunks);
impl Read for Chunks {
    type Error = Infallible;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let count = Ord::min(out.remaining_mut(), this.bytes.len());
        out.put_slice(&this.bytes[..count]);
        this.bytes.drain(..count);
        Poll::Ready(Ok(count))
    }
}
impl ReadChunk for Chunks {
    type Chunk = Bytes;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Bytes>, Infallible>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        this.chunk_requests += 1;
        let count = Ord::min(maximum.get(), this.bytes.len());
        Poll::Ready(Ok((count != 0).then(|| {
            Bytes::from(this.bytes.drain(..count).collect::<Vec<_>>())
        })))
    }
}

#[test]
fn reading_skips_on_a_receiver_and_forwards_the_reads() {
    let mut io = ComposedReceiver::new(reader(b"abcdefgh"), ReadSkip);
    assert_eq!(io.sync_skip_unpin(2u8).unwrap(), Some(2usize));
    let mut out = [0; 2];
    assert_eq!(io.sync_read_unpin(&mut out.as_mut_slice()).unwrap(), 2);
    assert_eq!(&out, b"cd");
    assert_eq!(io.sync_skip_unpin(Infinite).unwrap(), Some(4));
    assert_eq!(io.sync_skip_unpin(Infinite).unwrap(), None);
    assert_eq!(io.into_parts().0.into_inner().position(), 8);
}

#[test]
fn reading_skips_at_most_its_scratch_storage_in_one_poll() {
    let mut io = ComposedReceiver::new(reader(&[7; 20_000]), ReadSkip);
    assert_eq!(io.sync_skip_unpin(Infinite).unwrap(), Some(8192usize));
    assert_eq!(io.sync_skip_unpin(u64::MAX).unwrap(), Some(8192));
    assert_eq!(
        io.sync_skip_unpin(Infinite).unwrap(),
        Some(20_000 - 2 * 8192)
    );
    assert_eq!(io.sync_skip_unpin(Infinite).unwrap(), None);
}

#[test]
fn a_zero_maximum_reaches_the_parent_as_an_empty_request() {
    // An empty read cannot tell EOF, so reading reports no progress.
    let mut io = ComposedReceiver::new(reader(b""), ReadSkip);
    assert_eq!(io.sync_skip_unpin(0u8).unwrap(), Some(0usize));
    let mut io = ComposedReceiver::new(Failing, ReadSkip);
    assert_eq!(io.sync_skip_unpin(0u8), Err(Broken));
    let mut io = ComposedReceiver::new(Failing, ReadChunkSkip);
    assert_eq!(io.sync_skip_unpin(0u8), Err(Broken));
    let mut io = ComposedReceiver::new(
        Chunks {
            bytes: b"ab".to_vec(),
            chunk_requests: 0,
        },
        ReadChunkSkip,
    );
    assert_eq!(io.sync_skip_unpin(0u8), Ok(Some(0usize)));
    assert_eq!(io.get_ref().0.chunk_requests, 0);
}

#[test]
fn chunks_skip_by_dropping_one_bounded_chunk_per_poll() {
    let mut io = ComposedReceiver::new(
        Chunks {
            bytes: b"abcdef".to_vec(),
            chunk_requests: 0,
        },
        ReadChunkSkip,
    );
    assert_eq!(io.sync_skip_unpin(2u8), Ok(Some(2usize)));
    let chunk = io.sync_read_chunk_unpin(NonZeroUsize::MIN).unwrap();
    assert_eq!(chunk.as_deref(), Some(&b"c"[..]));
    assert_eq!(io.sync_skip_unpin(Infinite), Ok(Some(3)));
    assert_eq!(io.sync_skip_unpin(u128::MAX), Ok(None));
    assert_eq!(io.get_ref().0.chunk_requests, 4);
}

#[test]
fn a_copying_chunk_adapter_can_skip_by_chunks() {
    let chunked = Chunked::new(reader(b"abcdef"), NonZeroUsize::new(4).unwrap());
    let mut io = ComposedReceiver::new(chunked, ReadChunkSkip);
    assert_eq!(io.sync_skip_unpin(Infinite).unwrap(), Some(4usize));
    assert_eq!(io.sync_skip_unpin(Infinite).unwrap(), Some(2));
    assert_eq!(io.sync_skip_unpin(Infinite).unwrap(), None);
}

#[test]
fn the_composed_family_forwards_the_exact_length() {
    let mut io = ComposedReceiver::new(&b"abc"[..], ReadSkip);
    let view = pin!(Pin::new(&mut io).view());
    assert_eq!(
        <Skippable<nitori_call::Direct<&[u8]>, ReadSkip> as ExactLen>::remaining_len(view),
        3usize
    );
}

/// Skips `length` bytes by reading, with the state composed on the access.
#[call(sync)]
async fn skip_by_reading<F: Read>(
    io: &mut impl ReceiverAccess<Family = F>,
    length: usize,
) -> Result<(), SkipExactError<F::Error, usize>> {
    io.compose(ReadSkip).skip_exact(length).await
}

/// Skips `length` bytes by chunks, with the state composed on the access,
/// then reads one byte.
#[call(sync)]
async fn skip_by_chunks_then_read<F: ReadChunk>(
    io: &mut impl ReceiverAccess<Family = F>,
    length: usize,
) -> Result<[u8; 1], nitori_io::calls::ReadArrayError<F::Error>> {
    if io.compose(ReadChunkSkip).skip_exact(length).await.is_err() {
        panic!("the source holds enough bytes");
    }
    io.read_array::<1>().await
}

#[test]
fn both_states_compose_on_an_access_too() {
    let mut input = reader(b"abcdef");
    assert!(input.sync_skip_by_reading_unpin(4).is_ok());
    assert_eq!(input.get_ref().position(), 4);
    let mut input = Chunks {
        bytes: b"abcdef".to_vec(),
        chunk_requests: 0,
    };
    assert_eq!(input.sync_skip_by_chunks_then_read_unpin(3).unwrap(), *b"d");
}

/// A reader that reads one byte on every second poll.
struct Delayed<'data> {
    bytes: &'data [u8],
    waiting: bool,
}
nitori_call::family_receiver!(impl ['data] for Delayed<'data>);
impl Read for Delayed<'_> {
    type Error = Infallible;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if !this.waiting {
            this.waiting = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.waiting = false;
        let count = Ord::min(Ord::min(out.remaining_mut(), this.bytes.len()), 1);
        out.put_slice(&this.bytes[..count]);
        this.bytes = &this.bytes[count..];
        Poll::Ready(Ok(count))
    }
}

type Framed<'data> = Limited<Skippable<Delayed<'data>, ReadSkip>, ReadLimit>;

#[test]
fn a_cancelled_skip_by_reading_leaves_a_budget_above_it_in_step() {
    let mut framed = ComposedReceiver::new(
        ComposedReceiver::new(
            Delayed {
                bytes: b"abcdef",
                waiting: false,
            },
            ReadSkip,
        ),
        ReadLimit(5usize),
    );
    let mut call = Box::pin(Call::<Framed<'_>, _>::new(SkipExact::new(4)));
    let mut cx = Context::from_waker(Waker::noop());
    // The second poll skips the first byte, then waits for the second.
    for _ in 0..2 {
        let poll = call.as_mut().poll_with(
            Some(Lend::Fixed(pin!(Pin::new(&mut framed).view()))),
            &mut cx,
        );
        assert!(matches!(poll, CallPoll::Pending));
    }
    drop(call);
    let (inner, budget) = framed.get_ref();
    assert_eq!(inner.get_ref().0.bytes, b"bcdef");
    assert_eq!(budget, &ReadLimit(4));
    let mut call = pin!(Call::<Framed<'_>, _>::new(SkipExact::new(4)));
    let result = loop {
        let poll = call.as_mut().poll_with(
            Some(Lend::Fixed(pin!(Pin::new(&mut framed).view()))),
            &mut cx,
        );
        match poll {
            CallPoll::Pending => continue,
            CallPoll::Ready(ControlFlow::Break(result)) => break result,
            _ => panic!("the call neither waits nor finishes"),
        }
    };
    assert!(result.is_ok());
    let (inner, budget) = framed.get_ref();
    assert_eq!(
        (inner.get_ref().0.bytes, budget),
        (&b"f"[..], &ReadLimit(0))
    );
}

/// Every read fails.
struct Failing;
#[derive(Debug, PartialEq)]
struct Broken;
nitori_call::family_receiver!(impl [] for Failing);
impl Read for Failing {
    type Error = Broken;
    fn poll_read<'v, B: BufMut + ?Sized>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        _: &mut B,
    ) -> Poll<Result<usize, Broken>>
    where
        Self: 'v,
    {
        Poll::Ready(Err(Broken))
    }
}
impl ReadChunk for Failing {
    type Chunk = &'static [u8];
    fn poll_read_chunk<'v>(
        _: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        _: NonZeroUsize,
    ) -> Poll<Result<Option<&'static [u8]>, Broken>>
    where
        Self: 'v,
    {
        Poll::Ready(Err(Broken))
    }
}
