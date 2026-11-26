//! Futures that decode from or encode into a borrowed source or sink are
//! `Send` when the source or sink and its family are: base values,
//! composite values with metadata, values read from a type that keeps the
//! source, and values written with options, through generic accesses and
//! concrete ones.
use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{AsAccess, Composed, ConstructedAccess, ReceiverAccess};
use nitori_codec::prelude::*;
use nitori_io::{
    Combine, Read, ReadSegment, Value, Write, WriteSegment,
    adapters::{Limited, ReadLimit},
    calls::ReadArrayError,
};

/// Checks that `future` is `Send`, then runs it.
fn send<T: Future + Send>(future: T) -> T::Output {
    nitori_call::__private::run_sync(future)
}

/// Whether any of the bytes arrived in 0-RTT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ZeroRtt(bool);
impl Combine for ZeroRtt {
    fn combine(self, next: Self) -> Self {
        Self(self.0 || next.0)
    }
}

/// A received stream whose bytes all arrived in 0-RTT.
struct Received {
    data: &'static [u8],
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
        Poll::Ready(Ok(Some(ReadSegment {
            count: NonZeroUsize::MIN,
            metadata: ZeroRtt(true),
        })))
    }
}

type Plain = ConstructedAccess<&'static [u8]>;
type Early = ConstructedAccess<Received>;

/// A composite value: two base fields with the metadata of their bytes.
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
    type Error = ReadArrayError<<S::Family as Read>::Error>;
    async fn decode_from(mut io: S) -> Result<Self, Self::Error> {
        Ok(Pair {
            first: Value::decode_from(io.access()).await?,
            second: Value::decode_from(io.access()).await?,
        })
    }
}

/// A type that keeps the source and how much of it is left, as a payload
/// decoded in stages does.
struct Payload<S> {
    io: Composed<S, ReadLimit<u64>>,
}
impl<S: AsAccess<Family: Read>> AsAccess for &mut Payload<S> {
    type Family = Limited<nitori_call::Lent<S::Family>, ReadLimit<u64>>;
    type Access = Composed<S, ReadLimit<u64>>;
    fn access(&mut self) -> &mut Self::Access {
        &mut self.io
    }
}

async fn array_through<I: ReceiverAccess<Family: Read<Metadata = ()>>>(
    io: &mut I,
) -> Option<[u8; 2]> {
    io.decode::<[u8; 2]>().await.ok()
}

async fn pair_through<M, I>(io: &mut I) -> Option<Pair<M>>
where
    M: Combine + Unpin,
    I: ReceiverAccess<Family: Read<Metadata = M>>,
{
    io.decode::<Pair<M>>().await.ok()
}

async fn from_payload<M, I>(io: &mut I) -> Option<(Pair<M>, [u8; 1])>
where
    M: Combine + Unpin,
    I: ReceiverAccess<Family: Read<Metadata = M>>,
{
    let mut payload = Payload {
        io: Composed::new(&mut *io, ReadLimit(3)),
    };
    let pair = Pair::decode_from(&mut payload).await.ok()?;
    let last = Value::<[u8; 1], M>::decode_from(&mut payload).await.ok()?;
    Some((pair, last.value))
}

async fn encode_through<I: ReceiverAccess<Family: Write<(), Report = ()>>>(io: &mut I) -> bool {
    io.encode(*b"ab").await.is_ok() && io.encode(&b"cd"[..]).await.is_ok()
}

#[test]
fn an_array_from_a_borrowed_concrete_source_is_send() {
    let mut io: Plain = ConstructedAccess::new(b"abc");
    assert_eq!(send(io.decode::<[u8; 2]>()).unwrap(), *b"ab");
    assert_eq!(send(<[u8; 1]>::decode_from(&mut &mut io)).unwrap(), *b"c");
}

#[test]
fn an_array_through_a_generic_access_is_send() {
    let mut io: Plain = ConstructedAccess::new(b"abcd");
    assert_eq!(send(array_through(&mut io)), Some(*b"ab"));
    assert_eq!(send(array_through(&mut &mut io)), Some(*b"cd"));
}

#[test]
fn a_composite_with_metadata_from_a_borrowed_source_is_send() {
    let mut io: Early = ConstructedAccess::new(Received { data: b"abcd" });
    let pair = send(io.decode::<Pair<ZeroRtt>>()).unwrap();
    assert_eq!(pair.first.metadata, ZeroRtt(true));
    let pair = send(pair_through(&mut &mut io)).unwrap();
    assert_eq!((pair.first.value, pair.second.value), (*b"c", *b"d"));
}

#[test]
fn values_from_a_type_that_keeps_a_borrowed_source_are_send() {
    let mut io: Early = ConstructedAccess::new(Received { data: b"abcd" });
    let (pair, last) = send(from_payload(&mut io)).unwrap();
    assert_eq!((pair.first.value, last), (*b"a", *b"c"));
    let mut plain: Plain = ConstructedAccess::new(b"xyz");
    let (pair, last) = send(from_payload(&mut &mut plain)).unwrap();
    assert_eq!((pair.second.value, last), (*b"y", *b"z"));
}

#[test]
fn encoding_into_a_borrowed_sink_is_send() {
    let mut io = ConstructedAccess::new(Vec::new());
    send(io.encode(*b"xy")).unwrap();
    assert!(send(encode_through(&mut io)));
    assert!(send(encode_through(&mut &mut io)));
    assert_eq!(io.into_inner(), b"xyabcdabcd");
}

/// How urgently a segment is to be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Priority(u8);

/// A sink that takes priorities and records each segment with its own.
#[derive(Default)]
struct Prioritized {
    segments: Vec<(Vec<u8>, Priority)>,
}
nitori_call::family_receiver!(impl [] for Prioritized);
impl Write<Priority> for Prioritized {
    type Error = !;
    type Report = ();
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut B,
        options: &Priority,
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let Some(count) = NonZeroUsize::new(input.remaining()) else {
            return Poll::Ready(Ok(None));
        };
        let mut bytes = vec![0; count.get()];
        input.copy_to_slice(&mut bytes);
        this.segments.push((bytes, *options));
        Poll::Ready(Ok(Some(WriteSegment { count, report: () })))
    }
}

async fn value_through<P, I>(io: &mut I, options: P) -> bool
where
    P: Unpin,
    I: ReceiverAccess<Family: Write<P, Report = ()>>,
{
    io.encode(Value {
        value: *b"ab",
        metadata: options,
    })
    .await
    .is_ok()
}

#[test]
fn a_value_with_options_into_a_borrowed_sink_is_send() {
    let mut io = ConstructedAccess::new(Prioritized::default());
    send(io.encode(Value {
        value: *b"xy",
        metadata: Priority(1),
    }))
    .unwrap();
    assert!(send(value_through(&mut io, Priority(2))));
    assert!(send(value_through(&mut &mut io, Priority(3))));
    assert_eq!(
        io.into_inner().segments,
        [
            (b"xy".to_vec(), Priority(1)),
            (b"ab".to_vec(), Priority(2)),
            (b"ab".to_vec(), Priority(3)),
        ]
    );
}
