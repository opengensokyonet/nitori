#![feature(impl_trait_in_assoc_type)]
//! Metadata of read segments and reports of write segments: what the sources
//! and sinks produce, and how the operations and adapters carry and merge it.
use bytes::{Buf, BufMut, Bytes};
use futures_core::Stream;
use nitori_call::{ReceiverAccess, call, family_receiver};
use nitori_io::{
    ChunkSegment, Combine, ReadSegment, Value, WriteSegment,
    adapters::{Chunked, ReadLimit, WriteLimit},
    calls::{ReadTotal, WriteAllError},
    helpers::{poll_chunk_from_read, poll_read_from_chunk},
    prelude::*,
};
use std::{
    cell::Cell,
    collections::VecDeque,
    num::NonZeroUsize,
    ops::ControlFlow,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}
fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}
fn ready<T>(poll: Poll<T>) -> T {
    match poll {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("unexpected Pending"),
    }
}

/// Whether some of the bytes arrived early; merges to "some".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Early(bool);
impl Combine for Early {
    fn combine(self, next: Self) -> Self {
        Early(self.0 || next.0)
    }
}

/// A source that hands out its bytes in segments, one metadata each. A fill
/// stops at the segment's end even when the destination has more room, and a
/// chunk never spans two segments.
struct Segmented {
    segments: VecDeque<(Bytes, Early)>,
    polls: Rc<Cell<usize>>,
}
impl Segmented {
    fn new(segments: &[(&'static [u8], bool)]) -> Self {
        Self {
            segments: segments
                .iter()
                .map(|&(bytes, early)| (Bytes::from_static(bytes), Early(early)))
                .collect(),
            polls: Rc::default(),
        }
    }
    /// Take at most `maximum` bytes of the first segment.
    fn take(&mut self, maximum: usize) -> Option<(Bytes, Early)> {
        self.polls.set(self.polls.get() + 1);
        let (front, early) = self.segments.front_mut()?;
        let count = maximum.min(front.len());
        if count == 0 {
            return None;
        }
        let early = *early;
        let taken = front.split_to(count);
        if front.is_empty() {
            self.segments.pop_front();
        }
        Some((taken, early))
    }
}
family_receiver!(impl [] for Segmented);
impl Read for Segmented {
    type Error = !;
    type Metadata = Early;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        mut out: &mut B,
    ) -> Poll<Result<Option<ReadSegment<Early>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let Some((bytes, metadata)) = this.take(out.remaining_mut()) else {
            return Poll::Ready(Ok(None));
        };
        let count = nz(bytes.len());
        BufMut::put(&mut out, bytes);
        Poll::Ready(Ok(Some(ReadSegment { count, metadata })))
    }
}
impl ReadChunk for Segmented {
    type Chunk = Bytes;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<ChunkSegment<Bytes, Early>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        Poll::Ready(Ok(this
            .take(maximum.get())
            .map(|(chunk, metadata)| ChunkSegment { chunk, metadata })))
    }
}

/// Counts one write per accepted segment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Writes(u32);
impl Combine for Writes {
    fn combine(self, next: Self) -> Self {
        Writes(self.0 + next.0)
    }
}

/// A sink that accepts at most two bytes per write and reports one write each.
#[derive(Default)]
struct Reporting {
    bytes: Vec<u8>,
    polls: Rc<Cell<usize>>,
}
family_receiver!(impl [] for Reporting);
impl Write<()> for Reporting {
    type Error = !;
    type Report = Writes;
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut B,
        _: &(),
    ) -> Poll<Result<Option<WriteSegment<Writes>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        this.polls.set(this.polls.get() + 1);
        let count = input.remaining().min(2);
        this.bytes.put(input.take(count));
        Poll::Ready(Ok(NonZeroUsize::new(count).map(|count| WriteSegment {
            count,
            report: Writes(1),
        })))
    }
}

// The test source itself.

#[test]
fn a_read_stops_at_the_segment_end_even_with_room_left() {
    let mut source = Segmented::new(&[(b"abc", false), (b"de", true)]);
    let mut out = [0; 8];
    let mut destination = &mut out[..];
    let first = ready(Pin::new(&mut source).poll_read(&mut cx(), &mut destination))
        .unwrap()
        .unwrap();
    assert_eq!((first.count.get(), first.metadata), (3, Early(false)));
    assert_eq!(destination.len(), 5);
    let second = ready(Pin::new(&mut source).poll_read(&mut cx(), &mut destination))
        .unwrap()
        .unwrap();
    assert_eq!((second.count.get(), second.metadata), (2, Early(true)));
    assert!(
        ready(Pin::new(&mut source).poll_read(&mut cx(), &mut destination))
            .unwrap()
            .is_none()
    );
    assert_eq!(&out[..5], b"abcde");
}

#[test]
fn a_chunk_does_not_span_segments() {
    let mut source = Segmented::new(&[(b"abc", false), (b"de", true)]);
    let first = ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(99)))
        .unwrap()
        .unwrap();
    assert_eq!(
        (&first.chunk[..], first.metadata),
        (&b"abc"[..], Early(false))
    );
    let second = ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(1)))
        .unwrap()
        .unwrap();
    assert_eq!(
        (&second.chunk[..], second.metadata),
        (&b"d"[..], Early(true))
    );
}

// Reading operations.

#[test]
fn read_exact_across_two_segments_returns_the_combined_metadata() {
    for (segments, expected) in [
        ([(&b"ab"[..], false), (b"cd", true)], Early(true)),
        ([(&b"ab"[..], true), (b"cd", false)], Early(true)),
        ([(&b"ab"[..], false), (b"cd", false)], Early(false)),
    ] {
        let mut source = Segmented::new(&segments);
        let mut out = [0; 4];
        let mut destination = &mut out[..];
        let metadata = source.sync_read_exact_unpin(&mut destination, 4).unwrap();
        assert_eq!(metadata, expected);
        assert_eq!(out, *b"abcd");
    }
}

#[test]
fn read_exact_of_nothing_is_the_default_and_never_polls_the_host() {
    let mut source = Segmented::new(&[(b"ab", true)]);
    let mut out = [0; 4];
    let mut destination = &mut out[..];
    let metadata = source.sync_read_exact_unpin(&mut destination, 0).unwrap();
    assert_eq!(metadata, Early::default());
    assert_eq!(source.polls.get(), 0);
    assert_eq!(destination.len(), 4);
}

#[test]
fn read_to_end_reports_the_count_and_the_combined_metadata() {
    let mut source = Segmented::new(&[(b"ab", false), (b"cde", true), (b"f", false)]);
    let mut out = Vec::new();
    let total = source.sync_read_to_end_unpin(&mut out).unwrap();
    assert_eq!(
        total,
        ReadTotal {
            count: 6,
            metadata: Early(true)
        }
    );
    assert_eq!(out, b"abcdef");
}

#[test]
fn read_to_end_of_an_empty_source_is_zero_with_default_metadata() {
    let mut source = Segmented::new(&[]);
    let mut out = Vec::new();
    let total = source.sync_read_to_end_unpin(&mut out).unwrap();
    assert_eq!(
        total,
        ReadTotal {
            count: 0,
            metadata: Early::default()
        }
    );
    assert!(out.is_empty());
}

#[test]
fn read_array_returns_the_value_with_its_combined_metadata() {
    let mut source = Segmented::new(&[(b"ab", true), (b"cdef", false)]);
    let read = source.sync_read_array_unpin::<4>().unwrap();
    assert_eq!(
        read,
        Value {
            value: *b"abcd",
            metadata: Early(true)
        }
    );
    // The rest of the second segment is a segment of its own.
    let read = source.sync_read_array_unpin::<2>().unwrap();
    assert_eq!(
        read,
        Value {
            value: *b"ef",
            metadata: Early(false)
        }
    );
}

#[test]
fn read_le_and_read_be_return_the_value_with_the_metadata() {
    let mut source = Segmented::new(&[
        (&[0x01], false),
        (&[0x02], true),
        (&[0x01], false),
        (&[0x02, 0x03, 0x04], false),
    ]);
    let read = source.sync_read_le_unpin::<u16>().unwrap();
    assert_eq!(
        read,
        Value {
            value: 0x0201,
            metadata: Early(true)
        }
    );
    let read = source.sync_read_be_unpin::<u32>().unwrap();
    assert_eq!(
        read,
        Value {
            value: 0x0102_0304,
            metadata: Early(false)
        }
    );
}

/// Collect what a stream of chunks yields and how it ends.
fn collect_chunks<S, R>(stream: S) -> (Vec<(Vec<u8>, Early)>, R)
where
    S: Stream<Item = ControlFlow<R, ChunkSegment<Bytes, Early>>>,
{
    let mut stream = pin!(stream);
    let mut chunks = Vec::new();
    loop {
        match ready(stream.as_mut().poll_next(&mut cx())) {
            Some(ControlFlow::Continue(segment)) => {
                chunks.push((segment.chunk.to_vec(), segment.metadata));
            }
            Some(ControlFlow::Break(result)) => return (chunks, result),
            None => panic!("the stream ends with its result"),
        }
    }
}

#[test]
fn read_chunks_yields_each_chunk_with_its_own_metadata() {
    let mut source = Segmented::new(&[(b"abc", false), (b"defg", true), (b"h", false)]);
    let (chunks, result) = collect_chunks(source.read_chunks_unpin(99, nz(3)));
    assert_eq!(result.unwrap(), 8);
    assert_eq!(
        chunks,
        [
            (b"abc".to_vec(), Early(false)),
            (b"def".to_vec(), Early(true)),
            (b"g".to_vec(), Early(true)),
            (b"h".to_vec(), Early(false)),
        ]
    );
}

#[test]
fn read_chunks_exact_yields_each_chunk_with_its_own_metadata() {
    let mut source = Segmented::new(&[(b"ab", true), (b"cdef", false)]);
    let (chunks, result) = collect_chunks(source.read_chunks_exact_unpin(5, nz(4)));
    result.unwrap();
    assert_eq!(
        chunks,
        [
            (b"ab".to_vec(), Early(true)),
            (b"cde".to_vec(), Early(false)),
        ]
    );
}

// Adapters and helpers.

#[test]
fn chunked_gives_a_chunk_the_metadata_of_the_read_that_filled_it() {
    let mut source = Chunked::new(Segmented::new(&[(b"abc", false), (b"defg", true)]), nz(16));
    let first = ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(99)))
        .unwrap()
        .unwrap();
    assert_eq!(
        (&first.chunk[..], first.metadata),
        (&b"abc"[..], Early(false))
    );
    let second = ready(Pin::new(&mut source).poll_read_chunk(&mut cx(), nz(2)))
        .unwrap()
        .unwrap();
    assert_eq!(
        (&second.chunk[..], second.metadata),
        (&b"de"[..], Early(true))
    );
}

#[test]
fn chunked_forwards_the_metadata_of_ordinary_reads() {
    let mut source = Chunked::new(Segmented::new(&[(b"ab", true), (b"cd", false)]), nz(16));
    let mut out = [0; 8];
    let mut destination = &mut out[..];
    let first = ready(Pin::new(&mut source).poll_read(&mut cx(), &mut destination))
        .unwrap()
        .unwrap();
    assert_eq!((first.count.get(), first.metadata), (2, Early(true)));
}

#[call(sync)]
async fn segments_limited<F: Read<Metadata = Early, Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
    budget: usize,
) -> Vec<Option<(usize, Early)>> {
    let mut limited = io.compose(ReadLimit(budget));
    let mut seen = Vec::new();
    loop {
        let mut buffer = [0; 16];
        let mut destination = &mut buffer[..];
        let segment = limited.read(&mut destination).await.unwrap();
        seen.push(segment.map(|segment| (segment.count.get(), segment.metadata)));
        if segment.is_none() {
            return seen;
        }
    }
}

#[test]
fn read_limit_forwards_the_metadata_and_ends_at_the_boundary() {
    let mut source = Segmented::new(&[(b"abc", false), (b"defg", true)]);
    assert_eq!(
        source.sync_segments_limited_unpin(5),
        [Some((3, Early(false))), Some((2, Early(true))), None]
    );
    // What the budget left is still there, with its metadata.
    let mut out = Vec::new();
    let total = source.sync_read_to_end_unpin(&mut out).unwrap();
    assert_eq!((out, total.metadata), (b"fg".to_vec(), Early(true)));
}

#[test]
fn poll_read_from_chunk_carries_the_chunks_metadata() {
    let mut source = Segmented::new(&[(b"abc", true), (b"de", false)]);
    let mut out = [0; 8];
    let mut destination = &mut out[..];
    let segment = ready(poll_read_from_chunk(
        Pin::new(&mut source),
        &mut cx(),
        &mut destination,
    ))
    .unwrap()
    .unwrap();
    assert_eq!((segment.count.get(), segment.metadata), (3, Early(true)));
    let segment = ready(poll_read_from_chunk(
        Pin::new(&mut source),
        &mut cx(),
        &mut destination,
    ))
    .unwrap()
    .unwrap();
    assert_eq!((segment.count.get(), segment.metadata), (2, Early(false)));
    assert_eq!(&out[..5], b"abcde");
}

#[test]
fn poll_chunk_from_read_carries_the_reads_metadata() {
    let mut source = Segmented::new(&[(b"abc", false), (b"de", true)]);
    let mut storage = None;
    let mut next = |source: &mut Segmented| {
        ready(poll_chunk_from_read(
            Pin::new(source),
            &mut cx(),
            nz(8),
            &mut storage,
            bytes::BytesMut::with_capacity,
            bytes::BytesMut::freeze,
        ))
        .unwrap()
        .unwrap()
    };
    let first = next(&mut source);
    assert_eq!(
        (&first.chunk[..], first.metadata),
        (&b"abc"[..], Early(false))
    );
    let second = next(&mut source);
    assert_eq!(
        (&second.chunk[..], second.metadata),
        (&b"de"[..], Early(true))
    );
}

// Writing operations.

#[test]
fn write_all_returns_the_combined_report_of_its_segments() {
    let mut sink = Reporting::default();
    // Two bytes per write: three segments for five bytes.
    let report = sink.sync_write_all_unpin(&b"abcde"[..], ()).unwrap();
    assert_eq!(report, Writes(3));
    assert_eq!(sink.bytes, b"abcde");
}

#[test]
fn write_all_of_nothing_is_the_default_report_and_never_polls_the_host() {
    let mut sink = Reporting::default();
    let report = sink.sync_write_all_unpin(&b""[..], ()).unwrap();
    assert_eq!(report, Writes::default());
    assert_eq!(sink.polls.get(), 0);
}

#[test]
fn write_le_write_be_and_write_array_return_their_reports() {
    let mut sink = Reporting::default();
    assert_eq!(sink.sync_write_le_unpin(0x0102_u16, ()).unwrap(), Writes(1));
    assert_eq!(
        sink.sync_write_be_unpin(0x0102_0304_u32, ()).unwrap(),
        Writes(2)
    );
    assert_eq!(sink.sync_write_array_unpin(*b"xyz", ()).unwrap(), Writes(2));
    assert_eq!(sink.bytes, [2, 1, 1, 2, 3, 4, b'x', b'y', b'z']);
}

/// Each write's count and report through the budget, then the `write_all` past it.
type LimitedWrites = (
    Vec<Option<(usize, Writes)>>,
    Result<Writes, WriteAllError<!>>,
);

#[call(sync)]
async fn write_limited<F: Write<(), Report = Writes, Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
    budget: usize,
) -> LimitedWrites {
    let mut limited = io.compose(WriteLimit(budget));
    let mut seen = Vec::new();
    let mut input = &b"abcde"[..];
    for _ in 0..3 {
        let segment = limited.write(&mut input, &()).await.unwrap();
        seen.push(segment.map(|segment| (segment.count.get(), segment.report)));
    }
    let result = limited.write_all(&b"fg"[..], ()).await;
    (seen, result)
}

#[test]
fn write_limit_answers_none_at_the_boundary_and_write_all_reports_zero_progress() {
    let mut sink = Reporting::default();
    let (seen, result) = sink.sync_write_limited_unpin(3);
    assert_eq!(seen, [Some((2, Writes(1))), Some((1, Writes(1))), None]);
    let error = result.unwrap_err();
    assert!(matches!(error, WriteAllError::WriteZero { .. }));
    assert_eq!(error.completed(), 0);
    assert_eq!(sink.bytes, b"abc");
}

#[test]
fn write_limit_does_not_poll_the_host_at_the_boundary() {
    let mut sink = Reporting::default();
    let (_, result) = sink.sync_write_limited_unpin(0);
    assert!(result.is_err());
    assert_eq!(sink.polls.get(), 0);
    assert!(sink.bytes.is_empty());
}
