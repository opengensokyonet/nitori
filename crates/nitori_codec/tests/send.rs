//! Futures that decode from or encode into a borrowed source or sink are
//! `Send` when the source or sink and its family are: base values,
//! composite values with metadata, values read from a type that keeps the
//! source, and values written with options, through generic accesses and
//! concrete ones.
#![feature(impl_trait_in_assoc_type)]
use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{AccessPlan, ProvideAccess, ReceiverAccess, TrackValues, call, track_values};
use nitori_codec::prelude::*;
use nitori_io::{
    AtEnd, Combine, Read, ReadSegment, Value, Write, WriteSegment,
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

/// A composite value: two base fields with the metadata of their bytes.
#[derive(Debug, PartialEq)]
struct Pair<M = ()> {
    first: Value<[u8; 1], M>,
    second: Value<[u8; 1], M>,
}
impl<S, M> DecodeOn<S> for Pair<M>
where
    S: ReceiverAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
{
    type Error = ReadArrayError<<S::Family as Read>::Error>;
    async fn decode_on(io: &mut S) -> Result<Self, Self::Error> {
        Ok(Pair {
            first: io.decode::<Value<[u8; 1], M>>().await?,
            second: io.decode::<Value<[u8; 1], M>>().await?,
        })
    }
}

/// A type that keeps the source and how much of it is left, as a payload
/// decoded in stages does.
struct Payload<S> {
    io: ReadLimit<S, u64>,
}
impl<S: ReceiverAccess<Family: Read>> ProvideAccess for Payload<S> {
    type AccessFamily = Limited<nitori_call::Lent<S::Family>, u64>;
    type Lentness = <S as ProvideAccess>::Lentness;
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        self.io.access_plan()
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
        io: ReadLimit::new(&mut *io, 3u64),
    };
    let pair = Pair::decode_from(&mut payload).await.ok()?;
    let last = Value::<[u8; 1], M>::decode_from(&mut payload).await.ok()?;
    Some((pair, last.value))
}

async fn encode_through<I: ReceiverAccess<Family: Write<(), Report = ()>>>(io: &mut I) -> bool {
    io.encode(*b"ab").await.is_ok() && io.encode(&b"cd"[..]).await.is_ok()
}

#[call]
async fn arrays<F: Read<Metadata = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> ([u8; 2], [u8; 1]) {
    let first = io.decode::<[u8; 2]>().await.unwrap();
    let last = <[u8; 1]>::decode_from(&mut &mut *io).await.unwrap();
    (first, last)
}

#[test]
fn an_array_from_a_borrowed_concrete_source_is_send() {
    let wire = *b"abc";
    let mut io = wire.as_slice();
    assert_eq!(send(io.arrays_unpin()), (*b"ab", *b"c"));
}

#[call]
async fn generic_arrays<F: Read<Metadata = ()>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (Option<[u8; 2]>, Option<[u8; 2]>) {
    (array_through(io).await, array_through(&mut &mut *io).await)
}

#[test]
fn an_array_through_a_generic_access_is_send() {
    let mut io = &b"abcd"[..];
    assert_eq!(
        send(io.generic_arrays_unpin()),
        (Some(*b"ab"), Some(*b"cd"))
    );
}

#[call]
async fn pairs<F: Read<Metadata = ZeroRtt, Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (Pair<ZeroRtt>, Pair<ZeroRtt>) {
    let first = io.decode::<Pair<ZeroRtt>>().await.unwrap();
    let second = pair_through(&mut &mut *io).await.unwrap();
    (first, second)
}

#[test]
fn a_composite_with_metadata_from_a_borrowed_source_is_send() {
    let mut io = Received { data: b"abcd" };
    let (first, second) = send(io.pairs_unpin());
    assert_eq!(first.first.metadata, ZeroRtt(true));
    assert_eq!((second.first.value, second.second.value), (*b"c", *b"d"));
}

#[call]
async fn through_early_payload<F: Read<Metadata = ZeroRtt>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Option<(Pair<ZeroRtt>, [u8; 1])> {
    from_payload(io).await
}

#[call]
async fn through_plain_payload<F: Read<Metadata = ()>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Option<(Pair, [u8; 1])> {
    from_payload(&mut &mut *io).await
}

#[test]
fn values_from_a_type_that_keeps_a_borrowed_source_are_send() {
    let mut io = Received { data: b"abcd" };
    let (pair, last) = send(io.through_early_payload_unpin()).unwrap();
    assert_eq!((pair.first.value, last), (*b"a", *b"c"));
    let mut plain = &b"xyz"[..];
    let (pair, last) = send(plain.through_plain_payload_unpin()).unwrap();
    assert_eq!((pair.second.value, last), (*b"y", *b"z"));
}

#[call]
async fn encode_bytes<F: Write<(), Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) {
    io.encode(*b"xy").await.unwrap();
    assert!(encode_through(io).await);
    assert!(encode_through(&mut &mut *io).await);
}

#[test]
fn encoding_into_a_borrowed_sink_is_send() {
    let mut io = Vec::new();
    send(io.encode_bytes_unpin());
    assert_eq!(io, b"xyabcdabcd");
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

#[call]
async fn encode_prioritized<F: Write<Priority, Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) {
    io.encode(Value {
        value: *b"xy",
        metadata: Priority(1),
    })
    .await
    .unwrap();
    assert!(value_through(io, Priority(2)).await);
    assert!(value_through(&mut &mut *io, Priority(3)).await);
}

#[test]
fn a_value_with_options_into_a_borrowed_sink_is_send() {
    let mut io = Prioritized::default();
    send(io.encode_prioritized_unpin());
    assert_eq!(
        io.segments,
        [
            (b"xy".to_vec(), Priority(1)),
            (b"ab".to_vec(), Priority(2)),
            (b"ab".to_vec(), Priority(3)),
        ]
    );
}

#[derive(Default)]
struct Completed(usize);

impl TrackValues for Completed {
    fn decode_finished(&mut self) {
        self.0 += 1;
    }

    fn encode_finished(&mut self) {
        self.0 += 1;
    }
}

struct Tracked<A> {
    io: A,
    completed: Completed,
}

impl<A: ReceiverAccess> ProvideAccess for Tracked<A> {
    type AccessFamily = A::Family;
    type Lentness = A::Lentness;

    fn access_plan(
        &mut self,
    ) -> impl AccessPlan<'_, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        track_values(self.io.access_plan(), &mut self.completed)
    }
}

#[call]
async fn tracked_option<F: Read<Metadata = (), Error = !> + AtEnd<RemainderError = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> (Option<[u8; 1]>, Option<[u8; 1]>, usize) {
    let mut tracked = Tracked {
        io,
        completed: Completed::default(),
    };
    let found = tracked.decode::<Option<[u8; 1]>>().await.unwrap();
    let end = tracked.decode::<Option<[u8; 1]>>().await.unwrap();
    (found, end, tracked.completed.0)
}

#[test]
fn decoding_an_option_through_a_tracked_access_is_send() {
    let wire = *b"a";
    let mut input = wire.as_slice();
    assert_eq!(send(input.tracked_option_unpin()), (Some(*b"a"), None, 3));
}

#[call]
async fn tracked_encode<F: Write<(), Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> usize {
    let mut tracked = Tracked {
        io,
        completed: Completed::default(),
    };
    tracked.encode(*b"ab").await.unwrap();
    tracked.completed.0
}

#[test]
fn encoding_through_a_tracked_access_is_send() {
    let mut output = Vec::new();
    assert_eq!(send(output.tracked_encode_unpin()), 1);
    assert_eq!(output, b"ab");
}
