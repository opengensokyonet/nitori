//! Values with the metadata of their bytes: base values wrapped in
//! `Value`, inside composite values that take the metadata type as a
//! parameter, decoded in stages or whole.
#![feature(impl_trait_in_assoc_type)]
use bytes::BufMut;
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{AsAccess, Composed, ConstructedAccess, Lent, ReceiverAccess, call};
use nitori_codec::{payload::Chunks, prelude::*};
use nitori_io::{
    Combine, ExactLen, Read, ReadChunk, ReadSegment, Value, Write,
    adapters::{Limited, ReadLimit},
    calls::ReadArrayError,
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

/// A received stream that hands out one byte at a time; its first `early`
/// bytes arrived in 0-RTT.
struct Received {
    data: &'static [u8],
    early: usize,
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
        let Some((&byte, rest)) = this.data.split_first().filter(|_| out.has_remaining_mut())
        else {
            return Poll::Ready(Ok(None));
        };
        out.put_u8(byte);
        this.data = rest;
        let early = this.early > 0;
        this.early = this.early.saturating_sub(1);
        Poll::Ready(Ok(Some(ReadSegment {
            count: NonZeroUsize::MIN,
            metadata: ZeroRtt(early),
        })))
    }
}

fn stream(data: &'static [u8], early: usize) -> ConstructedAccess<Received> {
    ConstructedAccess::new(Received { data, early })
}

#[test]
fn a_base_value_is_read_with_the_metadata_of_its_bytes() {
    let mut io = stream(b"abcd", 3);
    let first = nitori_call::__private::run_sync(io.decode::<Value<[u8; 2], ZeroRtt>>());
    let second = nitori_call::__private::run_sync(io.decode::<Value<[u8; 2], ZeroRtt>>());
    assert_eq!(
        (first.unwrap(), second.unwrap()),
        (
            Value {
                value: *b"ab",
                metadata: ZeroRtt(true)
            },
            Value {
                value: *b"cd",
                metadata: ZeroRtt(true)
            }
        )
    );
    let mut later = stream(b"ef", 0);
    let third = nitori_call::__private::run_sync(later.decode::<Value<[u8; 2], ZeroRtt>>());
    assert_eq!(third.unwrap().metadata, ZeroRtt(false));
}

/// The payload of a frame decoded in stages: the source it is read from and
/// what is left of it.
struct Payload<S> {
    io: Composed<S, ReadLimit<u64>>,
}
impl<S: AsAccess<Family: Read>> AsAccess for &mut Payload<S> {
    type Family = Limited<Lent<S::Family>, ReadLimit<u64>>;
    type Access = Composed<S, ReadLimit<u64>>;
    fn access(&mut self) -> &mut Self::Access {
        &mut self.io
    }
}

/// A composite value: each base field with the metadata of its bytes.
struct Frame<P, M = ()> {
    kind: Value<[u8; 1], M>,
    length: Value<[u8; 1], M>,
    payload: P,
}

type FrameError<S> = ReadArrayError<<<S as AsAccess>::Family as Read>::Error>;

/// In stages: the header now, the payload from the source it keeps.
impl<S, M> DecodeFrom<S> for Frame<Payload<S>, M>
where
    S: AsAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
{
    type Error = FrameError<S>;
    async fn decode_from(mut io: S) -> Result<Self, Self::Error> {
        let kind = Value::<[u8; 1], M>::decode_from(io.access()).await?;
        let length = Value::<[u8; 1], M>::decode_from(io.access()).await?;
        let [left] = length.value;
        Ok(Frame {
            kind,
            length,
            payload: Payload {
                io: Composed::new(io, ReadLimit(u64::from(left))),
            },
        })
    }
}

/// Whole: the payload read by a nested decoder.
impl<S, M, P> DecodeFrom<S> for Frame<P, M>
where
    S: AsAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
    P: for<'a> DecodeFrom<&'a mut Payload<S>, Error = FrameError<S>>,
{
    type Error = FrameError<S>;
    async fn decode_from(io: S) -> Result<Self, Self::Error> {
        let Frame {
            kind,
            length,
            payload: mut staged,
        } = Frame::<Payload<S>, M>::decode_from(io).await?;
        let payload = P::decode_from(&mut staged).await?;
        Ok(Frame {
            kind,
            length,
            payload,
        })
    }
}

/// A composite payload of two base fields.
#[derive(Debug, PartialEq)]
struct Pair<M = ()> {
    first: Value<[u8; 1], M>,
    second: Value<[u8; 1], M>,
}
impl<S, M> DecodeFrom<S> for Pair<M>
where
    S: AsAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
{
    type Error = FrameError<S>;
    async fn decode_from(mut io: S) -> Result<Self, Self::Error> {
        Ok(Pair {
            first: Value::decode_from(io.access()).await?,
            second: Value::decode_from(io.access()).await?,
        })
    }
}

#[test]
fn a_frame_in_stages_keeps_its_header_and_the_payload_reports_its_own() {
    // The type byte and the length byte arrived in 0-RTT, the payload after.
    let mut io = stream(b"\x07\x02xyz", 2);
    let mut frame =
        nitori_call::__private::run_sync(io.decode::<Frame<Payload<_>, ZeroRtt>>()).unwrap();
    assert_eq!(
        (frame.kind, frame.length),
        (
            Value {
                value: [7],
                metadata: ZeroRtt(true)
            },
            Value {
                value: [2],
                metadata: ZeroRtt(true)
            }
        )
    );
    let mut payload = &mut frame.payload;
    let byte = nitori_call::__private::run_sync(payload.access().read_array::<1>()).unwrap();
    assert_eq!((byte.value, byte.metadata), (*b"x", ZeroRtt(false)));
    let next = nitori_call::__private::run_sync(Value::<[u8; 1], ZeroRtt>::decode_from(
        &mut frame.payload,
    ))
    .unwrap();
    assert_eq!(
        next,
        Value {
            value: *b"y",
            metadata: ZeroRtt(false)
        }
    );
    // The payload ends at its length.
    let mut payload = &mut frame.payload;
    let end = nitori_call::__private::run_sync(payload.access().read_array::<1>());
    assert!(matches!(end, Err(ReadArrayError::Incomplete { .. })));
}

#[test]
fn a_composite_payload_takes_the_same_metadata_without_another_wrapper() {
    // Only the type byte arrived in 0-RTT.
    let mut io = stream(b"\x07\x02ab", 1);
    let frame =
        nitori_call::__private::run_sync(io.decode::<Frame<Pair<ZeroRtt>, ZeroRtt>>()).unwrap();
    assert_eq!(frame.kind.metadata, ZeroRtt(true));
    assert_eq!(frame.length.metadata, ZeroRtt(false));
    assert_eq!(
        frame.payload,
        Pair {
            first: Value {
                value: *b"a",
                metadata: ZeroRtt(false)
            },
            second: Value {
                value: *b"b",
                metadata: ZeroRtt(false)
            },
        }
    );
}

#[test]
fn without_metadata_the_same_types_decode_from_memory() {
    let mut input = &b"\x07\x02ab rest"[..];
    let frame = input.sync_decode_unpin::<Frame<Pair>>().unwrap();
    assert_eq!(
        (
            frame.kind.value,
            frame.payload.first.value,
            frame.payload.second.value
        ),
        ([7], *b"a", *b"b")
    );
    assert_eq!(input, b" rest");
}

#[test]
fn a_value_without_write_options_encodes_into_memory() {
    let mut output = Vec::new();
    output
        .sync_encode_unpin(Value {
            value: *b"ab",
            metadata: (),
        })
        .unwrap();
    output
        .sync_encode_unpin(Value {
            value: &b"c"[..],
            metadata: (),
        })
        .unwrap();
    assert_eq!(output, b"abc");
    let mut value = Value {
        value: *b"abcd",
        metadata: ZeroRtt(false),
    };
    let Ok(length) = nitori_call::__private::run_sync(value.encoded_len());
    assert_eq!(length, 4);
}

#[call(sync)]
async fn forward<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
) -> u64
where
    R: ReadChunk<Metadata = ()> + ExactLen<Length = usize>,
    W: Write<(), Report = ()>,
    <R as Read>::Error: core::fmt::Debug,
    <W as Write<()>>::Error: core::fmt::Debug,
{
    let Ok(mut frame) = source.decode::<Frame<Payload<_>>>().await else {
        panic!("the frame header is in the input");
    };
    let mut payload = Chunks {
        source: &mut frame.payload,
        chunk_maximum: NonZeroUsize::MIN,
    };
    let Ok(length) = payload.encoded_len().await else {
        panic!("a payload knows its length");
    };
    sink.encode(frame.kind).await.unwrap();
    sink.encode([u8::try_from(length).unwrap()]).await.unwrap();
    sink.encode(payload).await.unwrap().count
}

#[test]
fn a_payload_in_stages_is_forwarded_chunk_by_chunk() {
    let mut input = &b"\x07\x03abcd"[..];
    let mut output = Vec::new();
    assert_eq!((&mut input, &mut output).sync_forward_unpin(), 3);
    assert_eq!(output, b"\x07\x03abc");
    assert_eq!(input, b"d");
}

/// The indices of the segments, in the order merged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Order(Vec<u8>);
impl Combine for Order {
    fn combine(mut self, next: Self) -> Self {
        self.0.extend(next.0);
        self
    }
}

/// A source of numbered one-byte segments that answers Pending before each.
struct Waiting {
    data: &'static [u8],
    next: u8,
    ready: bool,
}
nitori_call::family_receiver!(impl [] for Waiting);
impl Read for Waiting {
    type Error = !;
    type Metadata = Order;
    fn poll_read<'v, O: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut O,
    ) -> Poll<Result<Option<ReadSegment<Order>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if !core::mem::replace(&mut this.ready, false) {
            this.ready = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let Some((&byte, rest)) = this.data.split_first().filter(|_| out.has_remaining_mut())
        else {
            return Poll::Ready(Ok(None));
        };
        out.put_u8(byte);
        this.data = rest;
        let index = this.next;
        this.next += 1;
        Poll::Ready(Ok(Some(ReadSegment {
            count: NonZeroUsize::MIN,
            metadata: Order(vec![index]),
        })))
    }
}

/// Poll to the end with a waker that counts its wakeups; returns the output
/// and the Pending polls, each of which must have woken.
fn finish<F: core::future::Future>(future: F) -> (F::Output, usize) {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Wake, Waker},
    };
    struct Wakes(AtomicUsize);
    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut future = core::pin::pin!(future);
    for pendings in 0..100 {
        let before = wakes.0.load(Ordering::Relaxed);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return (output, pendings),
            Poll::Pending => assert!(wakes.0.load(Ordering::Relaxed) > before),
        }
    }
    panic!("the decoding does not finish");
}

#[test]
fn metadata_merges_once_per_segment_in_order_across_pending() {
    let mut io = ConstructedAccess::new(Waiting {
        data: b"abc",
        next: 0,
        ready: false,
    });
    let (decoded, pendings) = finish(io.decode::<Value<[u8; 3], Order>>());
    assert_eq!(
        decoded.unwrap(),
        Value {
            value: *b"abc",
            metadata: Order(vec![0, 1, 2])
        }
    );
    assert_eq!(pendings, 3);
}
