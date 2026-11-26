#![feature(impl_trait_in_assoc_type)]
//! The three copy operations: what they move, what they merge, how they end,
//! and how they behave when a side waits, fills up or fails.
use bytes::{Buf, BufMut, Bytes, BytesMut};
use nitori_call::{Constructed, ReceiverAccess, call, drive, family_receiver};
use nitori_io::Filled;
use nitori_io::{
    ChunkSegment, Combine, LentBuffer, LentOutcome, LentSegment, ReadSegment, WriteSegment,
    adapters::ReadLimit,
    calls::{
        CopyBufferedError, CopyChunksError, CopyIntoLent, CopyIntoLentError, CopyTotal,
        copy_buffered, copy_chunks, copy_into_lent,
    },
    prelude::*,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    future::Future,
    num::NonZeroUsize,
    pin::{Pin, pin},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

/// Whether some of the bytes arrived early; merges to "some".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Early(bool);
impl Combine for Early {
    fn combine(self, next: Self) -> Self {
        Early(self.0 || next.0)
    }
}
/// The number of segments a sink accepted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Writes(u32);
impl Combine for Writes {
    fn combine(self, next: Self) -> Self {
        Writes(self.0 + next.0)
    }
}

/// A source of segments with a metadata each; a read or a chunk stops at the
/// segment's end.
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

/// A sink that accepts or lends at most two bytes at a time, writes each
/// segment with an [`Early`] mark it records with the segment's bytes, and
/// reports one write for each, and can fail once it holds enough bytes.
#[derive(Default)]
struct Tally {
    bytes: Vec<u8>,
    /// Each accepted segment and the options it was written with.
    segments: Vec<(Vec<u8>, Early)>,
    fail_at: Option<usize>,
}
impl Tally {
    fn failed(&self) -> bool {
        self.fail_at.is_some_and(|at| self.bytes.len() >= at)
    }
}
family_receiver!(impl [] for Tally);
impl Write<Early> for Tally {
    type Error = &'static str;
    type Report = Writes;
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut B,
        options: &Early,
    ) -> Poll<Result<Option<WriteSegment<Writes>>, &'static str>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if this.failed() {
            return Poll::Ready(Err("write failed"));
        }
        let count = input.remaining().min(2);
        let mut copy = vec![0; count];
        input.copy_to_slice(&mut copy);
        this.bytes.extend(&copy);
        if count > 0 {
            this.segments.push((copy, *options));
        }
        Poll::Ready(Ok(NonZeroUsize::new(count).map(|count| WriteSegment {
            count,
            report: Writes(1),
        })))
    }
}
impl WriteLent<Early> for Tally {
    type Buffer<'l> = &'l mut Vec<u8>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<Writes, T, A>>, &'static str>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<&'l mut Vec<u8>>) -> Result<Filled<T, Early>, A>,
    {
        let this = host.get_pin_mut().get_mut();
        if this.failed() {
            return Poll::Ready(Err("write failed"));
        }
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
            (Ok(Filled { value, options }), Some(count)) => {
                this.segments.push((this.bytes[before..].to_vec(), options));
                LentOutcome::Kept(LentSegment {
                    count,
                    report: Writes(1),
                    value,
                })
            }
            (Ok(Filled { value, .. }), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => LentOutcome::GaveUp(abandoned),
        };
        if !matches!(outcome, LentOutcome::Kept(_)) {
            this.bytes.truncate(before);
        }
        Poll::Ready(Ok(Some(outcome)))
    }
}

/// A source of plain bytes, two at a time, that fails once it has given
/// `fail_after` bytes.
struct Failing {
    data: Bytes,
    given: usize,
    fail_after: usize,
    polls: Rc<Cell<usize>>,
}
impl Failing {
    fn new(data: &'static [u8], fail_after: usize) -> Self {
        Self {
            data: Bytes::from_static(data),
            given: 0,
            fail_after,
            polls: Rc::default(),
        }
    }
    fn take(&mut self, maximum: usize) -> Result<Option<Bytes>, &'static str> {
        self.polls.set(self.polls.get() + 1);
        if self.given >= self.fail_after {
            return Err("read failed");
        }
        let count = maximum.min(2).min(self.data.len());
        if count == 0 {
            return Ok(None);
        }
        self.given += count;
        Ok(Some(self.data.split_to(count)))
    }
}
family_receiver!(impl [] for Failing);
impl Read for Failing {
    type Error = &'static str;
    type Metadata = ();
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        mut out: &mut B,
    ) -> Poll<Result<Option<ReadSegment<()>>, &'static str>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        Poll::Ready(this.take(out.remaining_mut()).map(|bytes| {
            bytes.map(|bytes| {
                let count = nz(bytes.len());
                BufMut::put(&mut out, bytes);
                ReadSegment {
                    count,
                    metadata: (),
                }
            })
        }))
    }
}
impl ReadChunk for Failing {
    type Chunk = Bytes;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<ChunkSegment<Bytes, ()>>, &'static str>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        Poll::Ready(this.take(maximum.get()).map(|bytes| {
            bytes.map(|chunk| ChunkSegment {
                chunk,
                metadata: (),
            })
        }))
    }
}

type Host<S, K> = (S, K);

// Basic copies.

#[test]
fn copy_chunks_copies_every_byte_and_counts_them() {
    let mut hosts: Host<_, Vec<u8>> = (Bytes::from_static(b"hello, world"), Vec::new());
    let total = hosts.sync_copy_chunks_unpin(nz(5)).unwrap();
    assert_eq!(
        total,
        CopyTotal {
            count: 12,
            metadata: (),
            report: ()
        }
    );
    assert_eq!(hosts.1, b"hello, world");
    assert!(hosts.0.is_empty());
}

#[test]
fn copy_into_lent_copies_every_byte_and_counts_them() {
    let mut hosts: Host<_, BytesMut> = (Bytes::from_static(b"hello, world"), BytesMut::new());
    let total = hosts.sync_copy_into_lent_unpin(nz(5)).unwrap();
    assert_eq!(
        total,
        CopyTotal {
            count: 12,
            metadata: (),
            report: ()
        }
    );
    assert_eq!(&hosts.1[..], b"hello, world");
    assert!(hosts.0.is_empty());
}

#[test]
fn copy_buffered_copies_every_byte_and_counts_them() {
    let mut hosts: Host<_, Vec<u8>> = (&b"hello, world"[..], Vec::new());
    let mut buffer = [0; 5];
    let total = hosts.sync_copy_buffered_unpin(&mut buffer).unwrap();
    assert_eq!(
        total,
        CopyTotal {
            count: 12,
            metadata: (),
            report: ()
        }
    );
    assert_eq!(hosts.1, b"hello, world");
    assert!(hosts.0.is_empty());
}

#[test]
fn copying_an_empty_source_copies_nothing() {
    let mut hosts: Host<_, Vec<u8>> = (Bytes::new(), Vec::new());
    let empty = CopyTotal {
        count: 0,
        metadata: (),
        report: (),
    };
    assert_eq!(hosts.sync_copy_chunks_unpin(nz(4)).unwrap(), empty);
    assert_eq!(hosts.sync_copy_into_lent_unpin(nz(4)).unwrap(), empty);
    let mut buffer = [0; 4];
    assert_eq!(hosts.sync_copy_buffered_unpin(&mut buffer).unwrap(), empty);
    assert!(hosts.1.is_empty());
}

// Metadata and reports.

/// The three copies of the same segments into a sink that reports.
fn totals(segments: &[(&'static [u8], bool)]) -> [CopyTotal<Early, Writes>; 3] {
    let mut chunks = (Segmented::new(segments), Tally::default());
    let by_chunks = chunks.sync_copy_chunks_unpin(nz(10)).unwrap();
    let mut lent = (Segmented::new(segments), Tally::default());
    let by_lent = lent.sync_copy_into_lent_unpin(nz(10)).unwrap();
    let mut buffered = (Segmented::new(segments), Tally::default());
    let mut buffer = [0; 4];
    let by_buffer = buffered.sync_copy_buffered_unpin(&mut buffer).unwrap();
    let expected: Vec<u8> = segments
        .iter()
        .flat_map(|(bytes, _)| bytes.iter().copied())
        .collect();
    assert_eq!(chunks.1.bytes, expected);
    assert_eq!(lent.1.bytes, expected);
    assert_eq!(buffered.1.bytes, expected);
    [by_chunks, by_lent, by_buffer]
}

#[test]
fn copies_merge_the_metadata_and_the_reports() {
    // Two bytes per write: "abc" takes two writes, "de" one.
    for total in totals(&[(b"abc", false), (b"de", true)]) {
        assert_eq!(
            total,
            CopyTotal {
                count: 5,
                metadata: Early(true),
                report: Writes(3)
            }
        );
    }
}

#[test]
fn copies_merge_early_metadata_from_the_first_segment() {
    for total in totals(&[(b"ab", true), (b"cd", false)]) {
        assert_eq!(
            total,
            CopyTotal {
                count: 4,
                metadata: Early(true),
                report: Writes(2)
            }
        );
    }
}

#[test]
fn copies_of_segments_without_early_bytes_keep_the_default_metadata() {
    for total in totals(&[(b"ab", false), (b"cd", false)]) {
        assert_eq!(total.metadata, Early(false));
    }
}

#[test]
fn copying_nothing_gives_the_default_metadata_and_report() {
    for total in totals(&[]) {
        assert_eq!(
            total,
            CopyTotal {
                count: 0,
                metadata: Early(false),
                report: Writes(0)
            }
        );
    }
}

/// The segments each of the three copies wrote, with the options it
/// wrote them with.
fn segments_written(segments: &[(&'static [u8], bool)]) -> [Vec<(Vec<u8>, Early)>; 3] {
    let mut chunks = (Segmented::new(segments), Tally::default());
    chunks.sync_copy_chunks_unpin(nz(10)).unwrap();
    let mut lent = (Segmented::new(segments), Tally::default());
    lent.sync_copy_into_lent_unpin(nz(10)).unwrap();
    let mut buffered = (Segmented::new(segments), Tally::default());
    let mut buffer = [0; 4];
    buffered.sync_copy_buffered_unpin(&mut buffer).unwrap();
    [chunks.1.segments, lent.1.segments, buffered.1.segments]
}

#[test]
fn copies_write_each_segment_with_the_metadata_it_was_read_with() {
    // Two bytes per write: "cde" takes two writes, both with its metadata,
    // and no write joins bytes of two segments.
    let expected = vec![
        (b"ab".to_vec(), Early(true)),
        (b"cd".to_vec(), Early(false)),
        (b"e".to_vec(), Early(false)),
        (b"f".to_vec(), Early(true)),
    ];
    for written in segments_written(&[(b"ab", true), (b"cde", false), (b"f", true)]) {
        assert_eq!(written, expected);
    }
}

// A source that waits.

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A source of plain bytes, two at a time, that waits before every read,
/// registering the waker.
struct Waiting {
    data: Bytes,
    ready: bool,
    left: Rc<Cell<usize>>,
    waits: Rc<Cell<usize>>,
}
family_receiver!(impl [] for Waiting);
impl Read for Waiting {
    type Error = !;
    type Metadata = ();
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        mut out: &mut B,
    ) -> Poll<Result<Option<ReadSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if !this.ready {
            this.ready = true;
            this.waits.set(this.waits.get() + 1);
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.ready = false;
        let count = out.remaining_mut().min(2).min(this.data.len());
        let Some(segment) = NonZeroUsize::new(count) else {
            return Poll::Ready(Ok(None));
        };
        BufMut::put(&mut out, this.data.split_to(count));
        this.left.set(this.data.len());
        Poll::Ready(Ok(Some(ReadSegment {
            count: segment,
            metadata: (),
        })))
    }
}

/// A sink holding its bytes where the test can see them between polls, which
/// counts the fills that gave up.
struct Shared {
    bytes: Rc<RefCell<Vec<u8>>>,
    gave_up: Rc<Cell<usize>>,
}
family_receiver!(impl [] for Shared);
impl Write<()> for Shared {
    type Error = !;
    type Report = ();
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        input: &mut B,
        _: &(),
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let count = input.remaining();
        let mut copy = vec![0; count];
        input.copy_to_slice(&mut copy);
        this.bytes.borrow_mut().extend(copy);
        Poll::Ready(Ok(
            NonZeroUsize::new(count).map(|count| WriteSegment { count, report: () })
        ))
    }
}
impl WriteLent<()> for Shared {
    type Buffer<'l> = &'l mut Vec<u8>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<(), T, A>>, !>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<&'l mut Vec<u8>>) -> Result<Filled<T, ()>, A>,
    {
        let this = host.get_pin_mut().get_mut();
        let mut bytes = this.bytes.borrow_mut();
        let before = bytes.len();
        let mut lent = LentBuffer::new(&mut *bytes, wanted).expect("room");
        let result = fill(&mut lent);
        let count = lent
            .into_storage()
            .len()
            .checked_sub(before)
            .filter(|count| *count <= wanted.get())
            .and_then(NonZeroUsize::new);
        let outcome = match (result, count) {
            (Ok(Filled { value, .. }), Some(count)) => LentOutcome::Kept(LentSegment {
                count,
                report: (),
                value,
            }),
            (Ok(Filled { value, .. }), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => {
                this.gave_up.set(this.gave_up.get() + 1);
                LentOutcome::GaveUp(abandoned)
            }
        };
        if !matches!(outcome, LentOutcome::Kept(_)) {
            bytes.truncate(before);
        }
        Poll::Ready(Ok(Some(outcome)))
    }
}

#[test]
fn copy_into_lent_with_a_waiting_source_loses_and_repeats_nothing() {
    const DATA: &[u8] = b"abcdefg";
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let (left, waits, gave_up) = (
        Rc::new(Cell::new(DATA.len())),
        Rc::new(Cell::new(0)),
        Rc::new(Cell::new(0)),
    );
    let mut source = Waiting {
        data: Bytes::from_static(DATA),
        ready: false,
        left: left.clone(),
        waits: waits.clone(),
    };
    let mut sink = Shared {
        bytes: bytes.clone(),
        gave_up: gave_up.clone(),
    };
    let wake = Arc::new(Wakes::default());
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    let mut copy = pin!(drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink))
        ),
        CopyIntoLent::new(nz(8)),
    ));
    let mut pending_polls = 0;
    let total = loop {
        let wakes_before = wake.0.load(Ordering::Relaxed);
        let polled = copy.as_mut().poll(&mut cx);
        // Whatever the poll returned, every byte is in the source or in the
        // sink, once, in order.
        let held = bytes.borrow().clone();
        assert!(
            DATA.starts_with(&held),
            "the sink holds a prefix, got {held:?}"
        );
        assert_eq!(held.len() + left.get(), DATA.len());
        match polled {
            Poll::Ready(total) => break total.unwrap(),
            Poll::Pending => {
                pending_polls += 1;
                assert!(
                    wake.0.load(Ordering::Relaxed) > wakes_before,
                    "Pending without a wakeup"
                );
                assert!(pending_polls < 100, "the copy does not finish");
            }
        }
    };
    assert_eq!(
        total,
        CopyTotal {
            count: 7,
            metadata: (),
            report: ()
        }
    );
    assert_eq!(&*bytes.borrow(), DATA);
    // Four reads of bytes and the read that found the end each waited once.
    assert_eq!(waits.get(), 5);
    assert_eq!(pending_polls, 5);
    // Every wait and the end gave the loan up, and so kept nothing.
    assert_eq!(gave_up.get(), 5 + 1);
}

// A sink that fills up.

#[test]
fn copy_chunks_into_a_small_slice_reports_zero_progress_with_what_fit() {
    let mut storage = [0; 3];
    let mut hosts = (Bytes::from_static(b"abcdef"), &mut storage[..]);
    let error = hosts.sync_copy_chunks_unpin(nz(4)).unwrap_err();
    assert_eq!(error.completed(), 3);
    let CopyChunksError::WriteZero { source } = error;
    assert_eq!(source.completed, 3);
    assert_eq!(storage, *b"abc");
}

#[test]
fn copy_into_lent_into_a_small_slice_reports_zero_progress_with_what_fit() {
    let mut storage = [0; 3];
    let mut hosts = (Bytes::from_static(b"abcdef"), &mut storage[..]);
    let error = hosts.sync_copy_into_lent_unpin(nz(4)).unwrap_err();
    assert_eq!(error.completed(), 3);
    assert!(matches!(error, CopyIntoLentError::WriteZero { .. }));
    // The sink lent nothing once it was full, so one byte was read to tell
    // the end from zero progress and dropped; the rest is in the source.
    assert_eq!(hosts.0, &b"ef"[..]);
    assert_eq!(storage, *b"abc");
}

#[test]
fn copy_buffered_into_a_small_slice_reports_zero_progress_with_what_fit() {
    let mut storage = [0; 3];
    let mut hosts = (&b"abcdef"[..], &mut storage[..]);
    let mut buffer = [0; 4];
    let error = hosts.sync_copy_buffered_unpin(&mut buffer).unwrap_err();
    assert_eq!(error.completed(), 3);
    assert!(matches!(error, CopyBufferedError::WriteZero { .. }));
    assert_eq!(storage, *b"abc");
}

// Failures.

#[test]
fn copy_chunks_reports_the_read_failure_with_the_bytes_copied_before() {
    let mut hosts = (Failing::new(b"abcdef", 4), Vec::new());
    let error = hosts.sync_copy_chunks_unpin(nz(8)).unwrap_err();
    assert_eq!(error.completed(), 4);
    assert!(matches!(
        error,
        CopyChunksError::Read {
            source: "read failed",
            completed: 4
        }
    ));
    assert_eq!(hosts.1, b"abcd");
}

#[test]
fn copy_into_lent_reports_the_read_failure_with_the_bytes_copied_before() {
    let mut hosts = (Failing::new(b"abcdef", 4), Vec::new());
    let error = hosts.sync_copy_into_lent_unpin(nz(8)).unwrap_err();
    assert_eq!(error.completed(), 4);
    assert!(matches!(
        error,
        CopyIntoLentError::Read {
            source: "read failed",
            completed: 4
        }
    ));
    // The failed fill kept nothing.
    assert_eq!(hosts.1, b"abcd");
}

#[test]
fn copy_buffered_reports_the_read_failure_with_the_bytes_copied_before() {
    let mut hosts = (Failing::new(b"abcdef", 4), Vec::new());
    let mut buffer = [0; 8];
    let error = hosts.sync_copy_buffered_unpin(&mut buffer).unwrap_err();
    assert_eq!(error.completed(), 4);
    assert!(matches!(
        error,
        CopyBufferedError::Read {
            source: "read failed",
            completed: 4
        }
    ));
    assert_eq!(hosts.1, b"abcd");
}

#[test]
fn copy_chunks_reports_the_write_failure_with_the_bytes_copied_before() {
    let sink = Tally {
        fail_at: Some(4),
        ..Tally::default()
    };
    let mut hosts = (Segmented::new(&[(b"abcdef", false)]), sink);
    let error = hosts.sync_copy_chunks_unpin(nz(8)).unwrap_err();
    assert_eq!(error.completed(), 4);
    assert!(matches!(
        error,
        CopyChunksError::Write {
            source: "write failed",
            completed: 4
        }
    ));
    assert_eq!(hosts.1.bytes, b"abcd");
}

#[test]
fn copy_into_lent_reports_the_write_failure_with_the_bytes_copied_before() {
    let sink = Tally {
        fail_at: Some(4),
        ..Tally::default()
    };
    let mut hosts = (Segmented::new(&[(b"abcdef", false)]), sink);
    let error = hosts.sync_copy_into_lent_unpin(nz(8)).unwrap_err();
    assert_eq!(error.completed(), 4);
    assert!(matches!(
        error,
        CopyIntoLentError::Write {
            source: "write failed",
            completed: 4
        }
    ));
    // The sink failed before lending, so the source kept the rest.
    assert_eq!(
        hosts.0.segments.front().map(|(bytes, _)| bytes.clone()),
        Some(Bytes::from_static(b"ef"))
    );
}

#[test]
fn copy_buffered_reports_the_write_failure_with_the_bytes_copied_before() {
    let sink = Tally {
        fail_at: Some(4),
        ..Tally::default()
    };
    let mut hosts = (Segmented::new(&[(b"abcdef", false)]), sink);
    let mut buffer = [0; 8];
    let error = hosts.sync_copy_buffered_unpin(&mut buffer).unwrap_err();
    assert_eq!(error.completed(), 4);
    assert!(matches!(
        error,
        CopyBufferedError::Write {
            source: "write failed",
            completed: 4
        }
    ));
    assert_eq!(hosts.1.bytes, b"abcd");
}

#[test]
fn copy_buffered_with_an_empty_buffer_fails_before_any_io() {
    let source = Segmented::new(&[(b"abc", false)]);
    let polls = source.polls.clone();
    let mut hosts = (source, Tally::default());
    let error = hosts.sync_copy_buffered_unpin(&mut []).unwrap_err();
    assert_eq!(error.completed(), 0);
    let CopyBufferedError::InsufficientCapacity { source } = error else {
        panic!("expected insufficient capacity");
    };
    assert_eq!((source.required(), source.available()), (1, 0));
    assert_eq!(polls.get(), 0);
    assert!(hosts.1.bytes.is_empty());
}

// A fixed length.

#[call(sync)]
async fn chunks_exactly<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    length: usize,
) -> u64
where
    R: ReadChunk<Metadata = (), Error = !>,
    W: Write<(), Report = (), Error = !>,
{
    let mut limited = source.compose(ReadLimit(length));
    copy_chunks(&mut limited, sink, nz(4)).await.unwrap().count
}
#[call(sync)]
async fn lent_exactly<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    length: usize,
) -> u64
where
    R: Read<Metadata = (), Error = !>,
    W: WriteLent<(), Report = (), Error = !>,
{
    let mut limited = source.compose(ReadLimit(length));
    copy_into_lent(&mut limited, sink, nz(4))
        .await
        .unwrap()
        .count
}
#[call(sync)]
async fn buffered_exactly<R, W>(
    source: &mut impl ReceiverAccess<Family = R>,
    sink: &mut impl ReceiverAccess<Family = W>,
    length: usize,
) -> u64
where
    R: Read<Metadata = (), Error = !>,
    W: Write<(), Report = (), Error = !>,
{
    let mut limited = source.compose(ReadLimit(length));
    let mut buffer = [0; 4];
    copy_buffered(&mut limited, sink, &mut buffer)
        .await
        .unwrap()
        .count
}

#[test]
fn copy_chunks_after_a_read_limit_copies_exactly_the_length() {
    let mut hosts = (Bytes::from_static(b"abcdefgh"), Vec::new());
    assert_eq!(hosts.sync_chunks_exactly_unpin(6), 6);
    assert_eq!(hosts.1, b"abcdef");
    assert_eq!(hosts.0, &b"gh"[..]);
}

#[test]
fn copy_into_lent_after_a_read_limit_copies_exactly_the_length() {
    let mut hosts = (Bytes::from_static(b"abcdefgh"), BytesMut::new());
    assert_eq!(hosts.sync_lent_exactly_unpin(6), 6);
    assert_eq!(&hosts.1[..], b"abcdef");
    assert_eq!(hosts.0, &b"gh"[..]);
}

#[test]
fn copy_buffered_after_a_read_limit_copies_exactly_the_length() {
    let mut hosts = (Bytes::from_static(b"abcdefgh"), Vec::new());
    assert_eq!(hosts.sync_buffered_exactly_unpin(6), 6);
    assert_eq!(hosts.1, b"abcdef");
    assert_eq!(hosts.0, &b"gh"[..]);
}

#[test]
fn a_read_limit_longer_than_the_source_copies_what_there_is() {
    let mut hosts = (Bytes::from_static(b"abc"), Vec::new());
    assert_eq!(hosts.sync_chunks_exactly_unpin(10), 3);
    assert_eq!(hosts.1, b"abc");
}
