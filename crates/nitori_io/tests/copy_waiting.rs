//! The copies with a side that waits: a sink that answers Pending before
//! accepting or lending, and a source that answers Pending before reading.
//! After each resumption every byte is where it should be, once and in order.
use bytes::{Buf, BufMut, Bytes};
use nitori_call::{Constructed, drive, family_receiver};
use nitori_io::{
    ChunkSegment, LentBuffer, LentOutcome, LentSegment, ReadSegment, WriteSegment,
    calls::{CopyBuffered, CopyChunks, CopyIntoLent},
    prelude::*,
};
use std::{
    cell::{Cell, RefCell},
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

const DATA: &[u8] = b"abcdefghijklmnopq";

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Poll the operation to its end; "after_poll" runs after each poll. Returns
/// the output and the number of Pending polls, each of which registered the
/// waker.
fn finish<F: Future>(operation: F, mut after_poll: impl FnMut()) -> (F::Output, usize) {
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut operation = pin!(operation);
    let mut pendings = 0;
    loop {
        let before = wakes.0.load(Ordering::Relaxed);
        let polled = operation.as_mut().poll(&mut cx);
        after_poll();
        match polled {
            Poll::Ready(output) => return (output, pendings),
            Poll::Pending => {
                pendings += 1;
                assert!(
                    wakes.0.load(Ordering::Relaxed) > before,
                    "Pending without a wakeup"
                );
                assert!(pendings < 500, "the operation does not finish");
            }
        }
    }
}

/// A source that hands out at most "step" bytes at a time and, when it
/// "waits", answers Pending before every read.
struct Waiting {
    data: Bytes,
    waits: bool,
    ready: bool,
    step: usize,
    /// Polls of the source, Pending ones included.
    polls: Rc<Cell<usize>>,
    /// Bytes still in the source.
    left: Rc<Cell<usize>>,
}
impl Waiting {
    fn new(waits: bool, step: usize) -> Self {
        Self {
            data: Bytes::from_static(DATA),
            waits,
            ready: false,
            step,
            polls: Rc::default(),
            left: Rc::new(Cell::new(DATA.len())),
        }
    }
    fn take(&mut self, cx: &mut Context<'_>, maximum: usize) -> Poll<Option<Bytes>> {
        self.polls.set(self.polls.get() + 1);
        if self.waits && !self.ready {
            self.ready = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.ready = false;
        let count = maximum.min(self.step).min(self.data.len());
        if count == 0 {
            return Poll::Ready(None);
        }
        let taken = self.data.split_to(count);
        self.left.set(self.data.len());
        Poll::Ready(Some(taken))
    }
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
        let Poll::Ready(taken) = this.take(cx, out.remaining_mut()) else {
            return Poll::Pending;
        };
        Poll::Ready(Ok(taken.map(|bytes| {
            let count = nz(bytes.len());
            BufMut::put(&mut out, bytes);
            ReadSegment {
                count,
                metadata: (),
            }
        })))
    }
}
impl ReadChunk for Waiting {
    type Chunk = Bytes;
    fn poll_read_chunk<'v>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<ChunkSegment<Bytes, ()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        let Poll::Ready(taken) = this.take(cx, maximum.get()) else {
            return Poll::Pending;
        };
        Poll::Ready(Ok(taken.map(|chunk| ChunkSegment {
            chunk,
            metadata: (),
        })))
    }
}

/// A sink holding its bytes where the test can see them between polls,
/// accepting or lending two bytes at a time and, when it "stutters",
/// answering Pending before every write or loan.
struct Observed {
    bytes: Rc<RefCell<Vec<u8>>>,
    stutters: bool,
    ready: bool,
    /// Polls of the sink, Pending ones included.
    polls: Rc<Cell<usize>>,
    /// Calls of the fill.
    fills: Rc<Cell<usize>>,
}
impl Observed {
    fn new(stutters: bool) -> Self {
        Self {
            bytes: Rc::default(),
            stutters,
            ready: false,
            polls: Rc::default(),
            fills: Rc::default(),
        }
    }
    /// Whether this poll has to wait, with the waker registered.
    fn waits(&mut self, cx: &mut Context<'_>) -> bool {
        self.polls.set(self.polls.get() + 1);
        if self.stutters && !self.ready {
            self.ready = true;
            cx.waker().wake_by_ref();
            return true;
        }
        self.ready = false;
        false
    }
}
family_receiver!(impl [] for Observed);
impl Write for Observed {
    type Error = !;
    type Report = ();
    fn poll_write<'v, B: Buf + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        input: &mut B,
    ) -> Poll<Result<Option<WriteSegment<()>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if this.waits(cx) {
            return Poll::Pending;
        }
        let count = input.remaining().min(2);
        let mut copy = vec![0; count];
        input.copy_to_slice(&mut copy);
        this.bytes.borrow_mut().extend(copy);
        Poll::Ready(Ok(
            NonZeroUsize::new(count).map(|count| WriteSegment { count, report: () })
        ))
    }
}
impl WriteLent for Observed {
    type Buffer<'l> = &'l mut Vec<u8>;
    fn poll_write_lent<'v, T, A, Fill>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        wanted: NonZeroUsize,
        fill: Fill,
    ) -> Poll<Result<Option<LentOutcome<(), T, A>>, !>>
    where
        Self: 'v,
        Fill: for<'l> FnOnce(&mut LentBuffer<&'l mut Vec<u8>>) -> Result<T, A>,
    {
        let this = host.get_pin_mut().get_mut();
        if this.waits(cx) {
            return Poll::Pending;
        }
        this.fills.set(this.fills.get() + 1);
        let mut bytes = this.bytes.borrow_mut();
        let before = bytes.len();
        let granted = wanted.min(nz(2));
        let mut lent = LentBuffer::new(&mut *bytes, granted).expect("room");
        let result = fill(&mut lent);
        let count = lent
            .into_storage()
            .len()
            .checked_sub(before)
            .filter(|count| *count <= granted.get())
            .and_then(NonZeroUsize::new);
        let outcome = match (result, count) {
            (Ok(value), Some(count)) => LentOutcome::Kept(LentSegment {
                count,
                report: (),
                value,
            }),
            (Ok(value), None) => LentOutcome::NotKept(value),
            (Err(abandoned), _) => LentOutcome::GaveUp(abandoned),
        };
        if !matches!(outcome, LentOutcome::Kept(_)) {
            bytes.truncate(before);
        }
        Poll::Ready(Ok(Some(outcome)))
    }
}

/// What the sink holds is a prefix of the data, and what is in the source
/// and the sink together is at most the data: the rest is in flight in the
/// operation. With "nothing in flight" it is all of the data.
fn assert_placed(sink: &Rc<RefCell<Vec<u8>>>, source_left: &Rc<Cell<usize>>, in_flight: bool) {
    let held = sink.borrow().clone();
    assert!(
        DATA.starts_with(&held),
        "the sink holds a prefix, got {held:?}"
    );
    let total = held.len() + source_left.get();
    if in_flight {
        assert!(total <= DATA.len(), "a byte was taken twice");
    } else {
        assert_eq!(total, DATA.len(), "a byte was lost");
    }
}

// A sink that waits.

#[test]
fn copy_chunks_with_a_waiting_sink_repeats_and_loses_nothing() {
    let mut source = Waiting::new(false, 5);
    let mut sink = Observed::new(true);
    let (bytes, left) = (sink.bytes.clone(), source.left.clone());
    let operation = drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink)),
        ),
        CopyChunks::new(nz(5)),
    );
    // A chunk of five takes three writes of at most two: the sink waits in
    // the middle of a chunk.
    let (total, pendings) = finish(operation, || assert_placed(&bytes, &left, true));
    assert_eq!(total.unwrap().count, DATA.len() as u64);
    assert_eq!(&*bytes.borrow(), DATA);
    // One wait before each of the ten writes: 3 + 3 + 3 + 1.
    assert_eq!(pendings, 10);
}

#[test]
fn copy_buffered_with_a_waiting_sink_repeats_and_loses_nothing() {
    let mut source = Waiting::new(false, 5);
    let mut sink = Observed::new(true);
    let (bytes, left) = (sink.bytes.clone(), source.left.clone());
    let mut buffer = [0; 5];
    let operation = drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink)),
        ),
        CopyBuffered::new(&mut buffer),
    );
    let (total, pendings) = finish(operation, || assert_placed(&bytes, &left, true));
    assert_eq!(total.unwrap().count, DATA.len() as u64);
    assert_eq!(&*bytes.borrow(), DATA);
    assert_eq!(pendings, 10);
}

#[test]
fn copy_into_lent_with_a_sink_that_waits_before_the_fill_does_not_read_the_source() {
    let mut source = Waiting::new(false, 5);
    let mut sink = Observed::new(true);
    let (bytes, left) = (sink.bytes.clone(), source.left.clone());
    let (source_polls, fills, sink_polls) =
        (source.polls.clone(), sink.fills.clone(), sink.polls.clone());
    let operation = drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink)),
        ),
        CopyIntoLent::new(nz(5)),
    );
    let mut polled = 0;
    let (total, pendings) = finish(operation, || {
        polled += 1;
        // Every poll that waits does so before the fill: until the first
        // loan is called the source has not been polled.
        if polled == 1 {
            assert_eq!(
                (sink_polls.get(), fills.get(), source_polls.get()),
                (1, 0, 0)
            );
        }
        // No byte is in flight: each is in the source or in the sink.
        assert_placed(&bytes, &left, false);
    });
    assert_eq!(total.unwrap().count, DATA.len() as u64);
    assert_eq!(&*bytes.borrow(), DATA);
    // Nine loans kept bytes; the tenth found the end. Each waited once.
    assert_eq!(pendings, 10);
    assert_eq!(fills.get(), 10);
}

// A source that waits.

#[test]
fn copy_chunks_with_a_waiting_source_repeats_and_loses_nothing() {
    let mut source = Waiting::new(true, 3);
    let mut sink = Observed::new(false);
    let (bytes, left, source_polls) = (
        sink.bytes.clone(),
        source.left.clone(),
        source.polls.clone(),
    );
    let operation = drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink)),
        ),
        CopyChunks::new(nz(8)),
    );
    // The sink takes two bytes of a chunk of three, and a chunk is written
    // before the next is read: only the source's wait is outside a write.
    let (total, pendings) = finish(operation, || assert_placed(&bytes, &left, true));
    assert_eq!(total.unwrap().count, DATA.len() as u64);
    assert_eq!(&*bytes.borrow(), DATA);
    // Six chunks of three or fewer and the end each waited once, and a wait
    // polls the source twice.
    assert_eq!(pendings, 7);
    assert_eq!(source_polls.get(), 14);
}

#[test]
fn copy_buffered_with_a_waiting_source_repeats_and_loses_nothing() {
    let mut source = Waiting::new(true, 3);
    let mut sink = Observed::new(false);
    let (bytes, left, source_polls) = (
        sink.bytes.clone(),
        source.left.clone(),
        source.polls.clone(),
    );
    let mut buffer = [0; 8];
    let operation = drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink)),
        ),
        CopyBuffered::new(&mut buffer),
    );
    let (total, pendings) = finish(operation, || assert_placed(&bytes, &left, true));
    assert_eq!(total.unwrap().count, DATA.len() as u64);
    assert_eq!(&*bytes.borrow(), DATA);
    assert_eq!(pendings, 7);
    assert_eq!(source_polls.get(), 14);
}

// Both sides wait.

#[test]
fn copy_chunks_and_copy_buffered_with_both_sides_waiting_repeat_and_lose_nothing() {
    let mut source = Waiting::new(true, 4);
    let mut sink = Observed::new(true);
    let (bytes, left) = (sink.bytes.clone(), source.left.clone());
    let operation = drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink)),
        ),
        CopyChunks::new(nz(8)),
    );
    let (total, pendings) = finish(operation, || assert_placed(&bytes, &left, true));
    assert_eq!(total.unwrap().count, DATA.len() as u64);
    assert_eq!(&*bytes.borrow(), DATA);
    assert!(pendings > 0);

    let mut source = Waiting::new(true, 4);
    let mut sink = Observed::new(true);
    let (bytes, left) = (sink.bytes.clone(), source.left.clone());
    let mut buffer = [0; 8];
    let operation = drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink)),
        ),
        CopyBuffered::new(&mut buffer),
    );
    let (total, _) = finish(operation, || assert_placed(&bytes, &left, true));
    assert_eq!(total.unwrap().count, DATA.len() as u64);
    assert_eq!(&*bytes.borrow(), DATA);
}

#[test]
fn copy_into_lent_with_both_sides_waiting_repeats_and_loses_nothing() {
    let mut source = Waiting::new(true, 4);
    let mut sink = Observed::new(true);
    let (bytes, left) = (sink.bytes.clone(), source.left.clone());
    let operation = drive(
        (
            Constructed::new(Pin::new(&mut source)),
            Constructed::new(Pin::new(&mut sink)),
        ),
        CopyIntoLent::new(nz(8)),
    );
    let (total, pendings) = finish(operation, || assert_placed(&bytes, &left, false));
    assert_eq!(total.unwrap().count, DATA.len() as u64);
    assert_eq!(&*bytes.borrow(), DATA);
    assert!(pendings > 0);
}
