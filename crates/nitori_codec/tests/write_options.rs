//! Values written with options: a base value through fixed options,
//! a composite value whose fields carry their own, and decoded values and
//! buffered payloads written with options mapped from their metadata.
#![feature(impl_trait_in_assoc_type)]
use bytes::{Buf, BufMut};
use core::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{ReceiverAccess, call};
use nitori_codec::{payload::Buffered, prelude::*};
use nitori_io::{
    Combine, Read, ReadSegment, Value, Write, WriteSegment,
    adapters::{FixedOptions, MapOptions, MappedOptions},
    calls::{ReadArrayError, WriteArrayError},
};

/// How urgently a segment is to be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Priority(u8);

/// Whether any of the bytes arrived in 0-RTT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ZeroRtt(bool);
impl Combine for ZeroRtt {
    fn combine(self, next: Self) -> Self {
        Self(self.0 || next.0)
    }
}

/// A sink that takes priorities, accepts at most two bytes at a time and
/// records each segment with the priority it was written with.
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
        let Some(count) = NonZeroUsize::new(input.remaining().min(2)) else {
            return Poll::Ready(Ok(None));
        };
        let mut bytes = vec![0; count.get()];
        input.copy_to_slice(&mut bytes);
        this.segments.push((bytes, *options));
        Poll::Ready(Ok(Some(WriteSegment { count, report: () })))
    }
}

fn segment(bytes: &[u8], priority: u8) -> (Vec<u8>, Priority) {
    (bytes.to_vec(), Priority(priority))
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

/// A composite value: two base fields, each with the metadata of its bytes
/// when read and the options of its bytes when written.
#[derive(Debug, PartialEq)]
struct Pair<O> {
    first: Value<[u8; 1], O>,
    second: Value<[u8; 1], O>,
}
impl<S, M> DecodeFrom<S> for Pair<M>
where
    S: ReceiverAccess<Family: Read<Metadata = M>>,
    M: Combine + Unpin,
{
    type Error = ReadArrayError<<S::Family as Read>::Error>;
    async fn decode_from(mut io: S) -> Result<Self, Self::Error> {
        Ok(Pair {
            first: Value::decode_from(&mut io).await?,
            second: Value::decode_from(&mut io).await?,
        })
    }
}
/// Writes each field with its own options, whatever they are.
impl<S, O> EncodeInto<S> for Pair<O>
where
    S: ReceiverAccess<Family: Write<O, Report: Combine>>,
    O: Unpin,
{
    type Output = <S::Family as Write<O>>::Report;
    type Error = WriteArrayError<<S::Family as Write<O>>::Error>;
    async fn encode_into(self, mut io: S) -> Result<Self::Output, Self::Error> {
        let first = self.first.encode_into(&mut io).await?;
        let second = self.second.encode_into(&mut io).await?;
        Ok(first.combine(second))
    }
}

#[test]
fn each_field_of_a_composite_is_written_with_its_own_options() {
    let mut sink = Prioritized::default();
    sink.sync_encode_unpin(Pair {
        first: Value {
            value: [1],
            metadata: Priority(0),
        },
        second: Value {
            value: [2],
            metadata: Priority(9),
        },
    })
    .unwrap();
    sink.sync_encode_unpin(Value {
        value: *b"abc",
        metadata: Priority(5),
    })
    .unwrap();
    assert_eq!(
        sink.segments,
        [
            segment(&[1], 0),
            segment(&[2], 9),
            segment(b"ab", 5),
            segment(b"c", 5),
        ]
    );
}

/// Lends one of two kept priorities by whether the bytes arrived in 0-RTT.
struct ByArrival {
    early: Priority,
    late: Priority,
}
impl MapOptions<ZeroRtt> for ByArrival {
    type Options = Priority;
    fn map_options<'a>(&'a mut self, options: &'a ZeroRtt) -> &'a Priority {
        if options.0 { &self.early } else { &self.late }
    }
}

#[test]
fn a_decoded_composite_is_written_with_options_mapped_from_its_metadata() {
    let mut source = Received {
        data: b"ab",
        early: 1,
    };
    let pair = source.sync_decode_unpin::<Pair<ZeroRtt>>().unwrap();
    assert_eq!(
        (pair.first.metadata, pair.second.metadata),
        (ZeroRtt(true), ZeroRtt(false))
    );
    let mut sink = MappedOptions::new(
        Prioritized::default(),
        ByArrival {
            early: Priority(0),
            late: Priority(9),
        },
    );
    sink.sync_encode_unpin(pair).unwrap();
    let (sink, _) = sink.into_parts();
    assert_eq!(sink.segments, [segment(b"a", 0), segment(b"b", 9)]);
}

#[call(sync)]
async fn write_payload<F, G>(
    source: &mut impl ReceiverAccess<Family = F>,
    sink: &mut impl ReceiverAccess<Family = G>,
) -> nitori_io::calls::CopyTotal<ZeroRtt, ()>
where
    F: Read<Metadata = ZeroRtt, Error = !>,
    G: Write<ZeroRtt, Report = (), Error = !>,
{
    sink.encode(Buffered {
        source,
        buffer: [0u8; 4],
    })
    .await
    .unwrap()
}

#[test]
fn a_buffered_payload_writes_each_segment_with_options_mapped_from_its_metadata() {
    let mut sink = MappedOptions::new(
        Prioritized::default(),
        ByArrival {
            early: Priority(0),
            late: Priority(9),
        },
    );
    let mut source = Received {
        data: b"abc",
        early: 1,
    };
    let total = (&mut source, &mut sink).sync_write_payload_unpin();
    assert_eq!((total.count, total.metadata), (3, ZeroRtt(true)));
    let (sink, _) = sink.into_parts();
    assert_eq!(
        sink.segments,
        [segment(b"a", 0), segment(b"b", 9), segment(b"c", 9)]
    );
}

#[call(sync)]
async fn tagged<F: Write<Priority, Report = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
    priority: Priority,
) {
    let mut fixed = MappedOptions::new(&mut *io, FixedOptions(priority));
    fixed.encode(*b"xyz").await.unwrap();
}

#[test]
fn a_base_value_is_written_with_fixed_options() {
    let mut sink = MappedOptions::new(Prioritized::default(), FixedOptions(Priority(4)));
    sink.sync_encode_unpin(*b"abc").unwrap();
    let (mut sink, _) = sink.into_parts();
    sink.sync_tagged_unpin(Priority(6));
    assert_eq!(
        sink.segments,
        [
            segment(b"ab", 4),
            segment(b"c", 4),
            segment(b"xy", 6),
            segment(b"z", 6),
        ]
    );
}
