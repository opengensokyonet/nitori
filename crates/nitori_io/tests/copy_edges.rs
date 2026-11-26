#![feature(impl_trait_in_assoc_type)]
// The generated operations name both families' associated types.
#![allow(clippy::type_complexity)]
//! The edges of the three copies: a sink that is exactly full, a budget that
//! ends mid-copy, what is lost to the probe of a full sink, a source that
//! reports bytes it did not put, reports that merge through a write budget,
//! and a sink composed with a skip state.
use bytes::{Buf, BufMut, Bytes};
use nitori_call::{ReceiverAccess, call, family_receiver};
use nitori_io::Filled;
use nitori_io::{
    Combine, LentBuffer, LentOutcome, LentSegment, ReadSegment, WriteSegment,
    adapters::{ReadChunkSkip, ReadLimit, ReadSkip, WriteLimit},
    calls::{
        CopyBufferedError, CopyChunksError, CopyIntoLentError, CopyTotal, copy_buffered,
        copy_chunks, copy_into_lent,
    },
    prelude::*,
};
use std::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

const DATA: &[u8] = b"abcdefghijkl";

/// The number of segments a sink accepted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Writes(u32);
impl Combine for Writes {
    fn combine(self, next: Self) -> Self {
        Writes(self.0 + next.0)
    }
}

/// A sink that accepts or lends at most two bytes at a time and reports one
/// write for each.
#[derive(Default)]
struct Tally {
    bytes: Vec<u8>,
}
family_receiver!(impl [] for Tally);
impl Write<()> for Tally {
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
        let count = input.remaining().min(2);
        let mut copy = vec![0; count];
        input.copy_to_slice(&mut copy);
        this.bytes.extend(copy);
        Poll::Ready(Ok(NonZeroUsize::new(count).map(|count| WriteSegment {
            count,
            report: Writes(1),
        })))
    }
}
impl WriteLent<()> for Tally {
    type Buffer<'l> = &'l mut Vec<u8>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<Writes, T, A>>, !>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<&'l mut Vec<u8>>) -> Result<Filled<T, ()>, A>,
    {
        let this = host.get_pin_mut().get_mut();
        let before = this.bytes.len();
        let granted = wanted.min(nz(2));
        let mut lent = LentBuffer::new(&mut this.bytes, granted).expect("room");
        let result = fill(&mut lent);
        let count = lent
            .into_storage()
            .len()
            .checked_sub(before)
            .filter(|count| *count <= granted.get())
            .and_then(NonZeroUsize::new);
        let outcome = match (result, count) {
            (Ok(Filled { value, .. }), Some(count)) => LentOutcome::Kept(LentSegment {
                count,
                report: Writes(1),
                value,
            }),
            (Ok(Filled { value, .. }), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => LentOutcome::GaveUp(abandoned),
        };
        if !matches!(outcome, LentOutcome::Kept(_)) {
            this.bytes.truncate(before);
        }
        Poll::Ready(Ok(Some(outcome)))
    }
}

/// A source that first gives honest bytes, then claims to have put bytes
/// into the destination without putting any.
struct Liar {
    honest: Bytes,
    lies: usize,
}
family_receiver!(impl [] for Liar);
impl Read for Liar {
    type Error = !;
    type Metadata = ();
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        mut out: &mut B,
    ) -> Poll<Result<Option<ReadSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if !this.honest.is_empty() {
            let count = out.remaining_mut().min(this.honest.len());
            BufMut::put(&mut out, this.honest.split_to(count));
            return Poll::Ready(Ok(Some(ReadSegment {
                count: nz(count),
                metadata: (),
            })));
        }
        if this.lies > 0 {
            this.lies -= 1;
            return Poll::Ready(Ok(Some(ReadSegment {
                count: nz(3),
                metadata: (),
            })));
        }
        Poll::Ready(Ok(None))
    }
}

// The copies, each behind the budgets of both sides: "reads" bytes may be
// read and "writes" bytes written. Each returns what is left of both.

#[call(sync)]
async fn chunks_with<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    reads: usize,
    writes: usize,
    chunk: NonZeroUsize,
) -> (
    Result<CopyTotal<(), <W as Write<()>>::Report>, CopyChunksError<!, !>>,
    usize,
    usize,
)
where
    R: ReadChunk<Metadata = (), Error = !>,
    W: Write<(), Error = !, Report: Combine>,
{
    let mut limited_source = source.compose(ReadLimit(reads));
    let mut limited_sink = sink.compose(WriteLimit(writes));
    let result = copy_chunks(&mut limited_source, &mut limited_sink, chunk).await;
    (
        result,
        limited_source.into_state().0,
        limited_sink.into_state().0,
    )
}
#[call(sync)]
async fn lent_with<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    reads: usize,
    writes: usize,
    chunk: NonZeroUsize,
) -> (
    Result<CopyTotal<(), <W as Write<()>>::Report>, CopyIntoLentError<!, !>>,
    usize,
    usize,
)
where
    R: Read<Metadata = (), Error = !>,
    W: WriteLent<(), Error = !, Report: Combine>,
{
    let mut limited_source = source.compose(ReadLimit(reads));
    let mut limited_sink = sink.compose(WriteLimit(writes));
    let result = copy_into_lent(&mut limited_source, &mut limited_sink, chunk).await;
    (
        result,
        limited_source.into_state().0,
        limited_sink.into_state().0,
    )
}
#[call(sync)]
async fn buffered_with<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    reads: usize,
    writes: usize,
    buffer: &mut [u8],
) -> (
    Result<CopyTotal<(), <W as Write<()>>::Report>, CopyBufferedError<!, !>>,
    usize,
    usize,
)
where
    R: Read<Metadata = (), Error = !>,
    W: Write<(), Error = !, Report: Combine>,
{
    let mut limited_source = source.compose(ReadLimit(reads));
    let mut limited_sink = sink.compose(WriteLimit(writes));
    let result = copy_buffered(&mut limited_source, &mut limited_sink, buffer).await;
    (
        result,
        limited_source.into_state().0,
        limited_sink.into_state().0,
    )
}

#[derive(Debug, PartialEq, Eq)]
enum Ended<P> {
    Done { count: u64, report: P },
    WriteZero { completed: u64 },
}
macro_rules! ended {
    ($result:expr, $error:ident) => {
        match $result {
            Ok(total) => Ended::Done {
                count: total.count,
                report: total.report,
            },
            Err($error::WriteZero { source }) => Ended::WriteZero {
                completed: source.completed,
            },
            #[allow(unreachable_patterns)]
            Err(other) => panic!("unexpected failure: {other:?}"),
        }
    };
}

/// How one copy ended, and where everything is afterwards.
#[derive(Debug)]
struct Run<P> {
    ended: Ended<P>,
    reads_left: usize,
    writes_left: usize,
    /// Bytes still in the source.
    source_left: usize,
    /// What the sink holds.
    written: Vec<u8>,
}

/// One copy of "data" into a fresh sink: "init" is bound to "store" and the
/// sink is built from it; "written" reads the sink's bytes afterwards.
macro_rules! run {
    ($method:ident, $error:ident, $data:expr, $reads:expr, $writes:expr, $arg:expr;
     $store:ident = $init:expr; $hosts:ident => $sink:expr; $written:expr) => {{
        #[allow(unused_mut, unused_variables)]
        let mut $store = $init;
        let mut $hosts = (Bytes::from_static($data), $sink);
        let (result, reads_left, writes_left) = $hosts.$method($reads, $writes, $arg);
        let source_left = $hosts.0.len();
        Run {
            ended: ended!(result, $error),
            reads_left,
            writes_left,
            source_left,
            written: $written,
        }
    }};
}

/// The three copies of "data" into a slice of "room" bytes.
fn runs_into_slice(data: &'static [u8], room: usize, reads: usize, writes: usize) -> [Run<()>; 3] {
    [
        run!(sync_chunks_with_unpin, CopyChunksError, data, reads, writes, nz(4);
            storage = vec![0u8; room]; hosts => &mut storage[..]; storage.to_vec()),
        run!(sync_lent_with_unpin, CopyIntoLentError, data, reads, writes, nz(4);
            storage = vec![0u8; room]; hosts => &mut storage[..]; storage.to_vec()),
        run!(sync_buffered_with_unpin, CopyBufferedError, data, reads, writes, &mut [0u8; 4];
            storage = vec![0u8; room]; hosts => &mut storage[..]; storage.to_vec()),
    ]
}
/// The three copies of "data" into a vector.
fn runs_into_vec(data: &'static [u8], reads: usize, writes: usize) -> [Run<()>; 3] {
    [
        run!(sync_chunks_with_unpin, CopyChunksError, data, reads, writes, nz(4);
            unused = (); hosts => Vec::<u8>::new(); hosts.1.clone()),
        run!(sync_lent_with_unpin, CopyIntoLentError, data, reads, writes, nz(4);
            unused = (); hosts => Vec::<u8>::new(); hosts.1.clone()),
        run!(sync_buffered_with_unpin, CopyBufferedError, data, reads, writes, &mut [0u8; 4];
            unused = (); hosts => Vec::<u8>::new(); hosts.1.clone()),
    ]
}
/// The three copies of "data" into a sink that reports.
fn runs_into_tally(data: &'static [u8], writes: usize) -> [Run<Writes>; 3] {
    [
        run!(sync_chunks_with_unpin, CopyChunksError, data, usize::MAX, writes, nz(4);
            unused = (); hosts => Tally::default(); hosts.1.bytes.clone()),
        run!(sync_lent_with_unpin, CopyIntoLentError, data, usize::MAX, writes, nz(4);
            unused = (); hosts => Tally::default(); hosts.1.bytes.clone()),
        run!(sync_buffered_with_unpin, CopyBufferedError, data, usize::MAX, writes, &mut [0u8; 4];
            unused = (); hosts => Tally::default(); hosts.1.bytes.clone()),
    ]
}

// A sink that is exactly full.

#[test]
fn a_source_that_exactly_fills_a_slice_is_copied_by_every_copy() {
    for length in [1, 4, 5, 12] {
        for run in runs_into_slice(&DATA[..length], length, usize::MAX, usize::MAX) {
            assert_eq!(
                run.ended,
                Ended::Done {
                    count: length as u64,
                    report: ()
                },
                "length {length}"
            );
            assert_eq!(run.written, DATA[..length], "length {length}");
            assert_eq!(run.source_left, 0);
        }
    }
}

#[test]
fn a_read_limit_ends_a_copy_into_a_slice_exactly_when_it_is_full() {
    // The source has more, but the read limit ends it where the slice is full.
    for run in runs_into_slice(DATA, 5, 5, usize::MAX) {
        assert_eq!(
            run.ended,
            Ended::Done {
                count: 5,
                report: ()
            }
        );
        assert_eq!(run.written, DATA[..5]);
        assert_eq!(run.reads_left, 0);
        // Bytes beyond the limit stay in the source; the probe of the full
        // slice reads at the limit, which answers EOF without reading.
        assert_eq!(run.source_left, DATA.len() - 5);
    }
}

#[test]
fn a_write_limit_that_the_source_just_fits_is_a_success_in_every_copy() {
    for run in runs_into_vec(&DATA[..6], usize::MAX, 6) {
        assert_eq!(
            run.ended,
            Ended::Done {
                count: 6,
                report: ()
            }
        );
        assert_eq!(run.written, DATA[..6]);
        assert_eq!(run.writes_left, 0);
        assert_eq!(run.source_left, 0);
    }
}

#[test]
fn a_read_limit_and_a_write_limit_that_meet_are_a_success_in_every_copy() {
    for run in runs_into_vec(DATA, 7, 7) {
        assert_eq!(
            run.ended,
            Ended::Done {
                count: 7,
                report: ()
            }
        );
        assert_eq!((run.reads_left, run.writes_left), (0, 0));
        assert_eq!(run.written, DATA[..7]);
        assert_eq!(run.source_left, DATA.len() - 7);
    }
}

#[test]
fn a_source_longer_than_the_slice_is_zero_progress_with_what_fit() {
    let [chunks, lent, buffered] = runs_into_slice(&DATA[..8], 5, usize::MAX, usize::MAX);
    for run in [&chunks, &lent, &buffered] {
        assert_eq!(run.ended, Ended::WriteZero { completed: 5 });
        assert_eq!(run.written, DATA[..5]);
    }
    // The lending copy read one byte from the source to tell its end from
    // zero progress, and that byte is gone: 8 - 5 - 1.
    assert_eq!(lent.source_left, 2);
}

#[test]
fn a_full_slice_and_a_source_with_one_more_byte_lose_exactly_that_byte() {
    let [chunks, lent, buffered] = runs_into_slice(&DATA[..4], 3, usize::MAX, usize::MAX);
    for run in [&chunks, &lent, &buffered] {
        assert_eq!(run.ended, Ended::WriteZero { completed: 3 });
        assert_eq!(run.written, DATA[..3]);
    }
    assert_eq!(lent.source_left, 0);
}

// A budget that ends in the middle of the copy.

#[test]
fn a_write_limit_that_ends_mid_copy_reports_zero_progress_with_the_count_so_far() {
    // Limit 5 over 12 bytes in chunks of four: the sink takes four, then one,
    // then answers none at the boundary.
    let [chunks, lent, buffered] = runs_into_vec(DATA, usize::MAX, 5);
    for run in [&chunks, &lent, &buffered] {
        assert_eq!(run.ended, Ended::WriteZero { completed: 5 });
        assert_eq!(run.written, DATA[..5]);
        assert_eq!(run.writes_left, 0);
    }
    // The lending copy asked the sink before reading: it read 5 bytes into
    // the loans, then one byte as the probe.
    assert_eq!(lent.source_left, DATA.len() - 5 - 1);
}

#[test]
fn a_write_limit_that_ends_mid_copy_ends_a_copy_with_a_source_that_has_just_ended() {
    let [chunks, lent, buffered] = runs_into_vec(&DATA[..5], usize::MAX, 5);
    for run in [&chunks, &lent, &buffered] {
        assert_eq!(
            run.ended,
            Ended::Done {
                count: 5,
                report: ()
            }
        );
    }
}

// Reports through a write budget.

#[test]
fn reports_are_merged_through_a_write_limit_when_the_source_fits() {
    // Two bytes per write: five bytes take three writes.
    for run in runs_into_tally(&DATA[..5], 5) {
        assert_eq!(
            run.ended,
            Ended::Done {
                count: 5,
                report: Writes(3)
            }
        );
        assert_eq!(run.written, DATA[..5]);
        assert_eq!(run.writes_left, 0);
    }
}

#[test]
fn reports_are_merged_without_a_limit_and_a_longer_source_is_zero_progress_with_one() {
    for run in runs_into_tally(DATA, usize::MAX) {
        assert_eq!(
            run.ended,
            Ended::Done {
                count: 12,
                report: Writes(6)
            }
        );
    }
    for run in runs_into_tally(&DATA[..7], 5) {
        assert_eq!(run.ended, Ended::WriteZero { completed: 5 });
        assert_eq!(run.written, DATA[..5]);
    }
}

// A source that reports bytes it did not put.

#[test]
fn copy_into_lent_is_zero_progress_when_the_source_reports_bytes_it_did_not_put() {
    let mut hosts = (
        Liar {
            honest: Bytes::from_static(b"ab"),
            lies: 1,
        },
        Vec::new(),
    );
    let error = hosts.sync_copy_into_lent_unpin(nz(4)).unwrap_err();
    assert_eq!(error.completed(), 2);
    assert!(matches!(error, CopyIntoLentError::WriteZero { .. }));
    // The loan that was not kept left nothing behind.
    assert_eq!(hosts.1, b"ab");
}

#[test]
fn copy_into_lent_over_a_slice_is_zero_progress_when_the_source_lies() {
    let mut storage = [0; 4];
    let mut hosts = (
        Liar {
            honest: Bytes::from_static(b"ab"),
            lies: 2,
        },
        &mut storage[..],
    );
    let error = hosts.sync_copy_into_lent_unpin(nz(4)).unwrap_err();
    assert_eq!(error.completed(), 2);
    assert!(matches!(error, CopyIntoLentError::WriteZero { .. }));
    // The slice advanced only past the honest bytes.
    assert_eq!(hosts.1.len(), 2);
    assert_eq!(&storage[..2], b"ab");
}

#[test]
fn a_source_that_lies_from_the_start_copies_nothing() {
    let mut hosts = (
        Liar {
            honest: Bytes::new(),
            lies: 1,
        },
        Vec::new(),
    );
    let error = hosts.sync_copy_into_lent_unpin(nz(4)).unwrap_err();
    assert_eq!(error.completed(), 0);
    assert!(hosts.1.is_empty());
}

// A sink composed with a skip state.

#[call(sync)]
async fn lent_into_skippable<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    budget: usize,
) -> Result<u64, CopyIntoLentError<!, !>>
where
    R: Read<Metadata = (), Error = !>,
    W: WriteLent<(), Report = (), Error = !>,
{
    let mut limited = sink.compose(WriteLimit(budget));
    let mut by_reading = limited.compose(ReadSkip);
    let mut by_chunks = by_reading.compose(ReadChunkSkip);
    Ok(copy_into_lent(source, &mut by_chunks, nz(4)).await?.count)
}

#[test]
fn copy_into_lent_lends_through_sinks_composed_with_skip_states() {
    let mut hosts = (Bytes::from_static(DATA), Vec::new());
    assert_eq!(
        hosts.sync_lent_into_skippable_unpin(usize::MAX).unwrap(),
        12
    );
    assert_eq!(hosts.1, DATA);
    let mut storage = [0; 12];
    let mut hosts = (Bytes::from_static(DATA), &mut storage[..]);
    assert_eq!(
        hosts.sync_lent_into_skippable_unpin(usize::MAX).unwrap(),
        12
    );
    assert!(hosts.1.is_empty());
    assert_eq!(storage, *DATA);
}

#[test]
fn a_sink_composed_with_skip_states_forwards_its_boundary_to_copy_into_lent() {
    let mut hosts = (Bytes::from_static(DATA), Vec::new());
    let error = hosts.sync_lent_into_skippable_unpin(5).unwrap_err();
    assert_eq!(error.completed(), 5);
    assert!(matches!(error, CopyIntoLentError::WriteZero { .. }));
    assert_eq!(hosts.1, DATA[..5]);
}
