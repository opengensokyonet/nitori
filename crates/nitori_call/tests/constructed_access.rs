//! A type that holds its access, defined once for every access, used inside
//! an operation and across awaits on a receiver held by a
//! `ConstructedAccess`.
#![feature(impl_trait_in_assoc_type, yield_expr)]
use nitori_call::{Composed, ConstructedAccess, LentAccess, ReceiverAccess, call, family_receiver};
use nitori_io::{Read, adapters::ReadLimit, prelude::*};
use std::{
    convert::Infallible,
    future::Future,
    marker::PhantomPinned,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    InRecord,
    Ended,
    Interrupted,
}

#[derive(Debug, PartialEq, Eq)]
enum NextError<E> {
    Read(E),
    Interrupted,
}

/// Records of one length byte and a payload. Holds its access, the current
/// record's length and what is left of its payload.
#[derive(Debug)]
struct RecordStream<I> {
    io: I,
    payload: ReadLimit<usize>,
    length: u8,
    phase: Phase,
}

type ReadError<A> = <<A as ReceiverAccess>::Family as Read>::Error;

async fn read_length<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
) -> Result<Option<u8>, ReadError<A>> {
    let mut byte = [0u8; 1];
    let read = io.read(&mut &mut byte[..]).await?;
    Ok(read.is_some().then_some(byte[0]))
}

async fn drain<A: ReceiverAccess<Family: Read>>(io: &mut A) -> Result<(), ReadError<A>> {
    let mut byte = [0u8; 1];
    while io.read(&mut &mut byte[..]).await?.is_some() {}
    Ok(())
}

impl<I: ReceiverAccess<Family: Read>> RecordStream<I> {
    async fn read(mut io: I) -> Result<Option<Self>, ReadError<I>> {
        Ok(read_length(&mut io).await?.map(|length| Self {
            io,
            payload: ReadLimit(usize::from(length)),
            length,
            phase: Phase::InRecord,
        }))
    }
    fn record(&mut self) -> Composed<&mut I, &mut ReadLimit<usize>> {
        Composed::new(&mut self.io, &mut self.payload)
    }
    async fn next_record(
        &mut self,
    ) -> Result<Option<Composed<&mut I, &mut ReadLimit<usize>>>, NextError<ReadError<I>>> {
        match self.phase {
            Phase::Ended => return Ok(None),
            Phase::Interrupted => return Err(NextError::Interrupted),
            Phase::InRecord => {}
        }
        drain(&mut Composed::new(&mut self.io, &mut self.payload))
            .await
            .map_err(NextError::Read)?;
        self.phase = Phase::Interrupted;
        match read_length(&mut self.io).await.map_err(NextError::Read)? {
            None => {
                self.phase = Phase::Ended;
                Ok(None)
            }
            Some(length) => {
                self.payload = ReadLimit(usize::from(length));
                self.length = length;
                self.phase = Phase::InRecord;
                Ok(Some(self.record()))
            }
        }
    }
    fn into_io(self) -> I {
        self.io
    }
}

/// A payload reader, written once for both.
#[call(sync)]
async fn read_word<F: Read<Metadata: nitori_io::Combine>, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Option<u16> {
    io.read_array::<2>()
        .await
        .ok()
        .map(|read| u16::from_be_bytes(read.value))
}

/// Inside an operation: the stream holds the operation's access.
#[call(sync)]
async fn words<F: Read<Metadata: nitori_io::Combine>, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Vec<Option<u16>> {
    let mut words = Vec::new();
    let Ok(Some(mut stream)) = RecordStream::read(&mut *io).await else {
        return words;
    };
    words.push(stream.record().read_word().await);
    while let Ok(Some(mut record)) = stream.next_record().await {
        words.push(record.read_word().await);
    }
    words
}

/// Each byte of the payload, as an item.
#[call(yields = u8)]
async fn bytes<F: Read<Metadata: nitori_io::Combine>, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> usize {
    let mut count = 0;
    while let Ok(nitori_io::Value { value: [byte], .. }) = io.read_array::<1>().await {
        yield byte;
        count += 1;
    }
    count
}

fn block<T>(future: impl Future<Output = T>) -> T {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..10_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
    panic!("the future did not finish")
}

/// Records [00 07], [00 08 ff], [], [00 09].
const INPUT: &[u8] = &[2, 0, 7, 3, 0, 8, 0xff, 0, 2, 0, 9];

#[test]
fn inside_an_operation() {
    let mut input = INPUT;
    assert_eq!(input.sync_words_unpin(), [Some(7), Some(8), None, Some(9)]);
    assert!(input.is_empty());
}

#[test]
fn across_awaits_on_a_receiver_held_by_value() {
    let mut stream = block(RecordStream::read(ConstructedAccess::new(INPUT)))
        .unwrap()
        .unwrap();
    assert_eq!(block(stream.record().read_word()), Some(7));
    let mut record = block(stream.next_record()).unwrap().unwrap();
    assert_eq!(block(record.read_array::<1>()).unwrap().value, [0]);
    // What is left of the record is kept in the stream.
    assert_eq!(block(stream.record().read_array::<1>()).unwrap().value, [8]);
    assert!(block(stream.next_record()).unwrap().is_some());
    assert_eq!(stream.length, 0);
    assert_eq!(block(stream.record().read_word()), None);
    assert!(block(stream.next_record()).unwrap().is_some());
    assert_eq!(block(stream.record().read_word()), Some(9));
    assert!(block(stream.next_record()).unwrap().is_none());
    assert_eq!(stream.phase, Phase::Ended);
    assert!(block(stream.next_record()).unwrap().is_none());
    assert!(stream.into_io().into_inner().is_empty());
}

#[test]
fn on_a_borrowed_receiver() {
    let mut input = INPUT;
    {
        let mut stream = block(RecordStream::read(ConstructedAccess::new(&mut input)))
            .unwrap()
            .unwrap();
        assert_eq!(block(stream.record().read_word()), Some(7));
    }
    assert_eq!(input, &INPUT[3..]);
}

#[test]
fn the_receiver_between_accesses() {
    let mut access = ConstructedAccess::new(INPUT);
    assert_eq!(block(access.read_array::<1>()).unwrap().value, [2]);
    assert_eq!(access.get_ref().len(), INPUT.len() - 1);
    *access.get_mut() = &INPUT[1..];
    assert_eq!(block(access.read_word()), Some(7));
}

/// One byte per read, each after a Pending.
struct Dribble<'a> {
    data: &'a [u8],
    ready: bool,
}
family_receiver!(impl ['a] for Dribble<'a>);
impl Read for Dribble<'_> {
    type Error = Infallible;
    type Metadata = ();
    fn poll_read<'v, O: bytes::BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<Result<Option<nitori_io::ReadSegment<Self::Metadata>>, !>>
    where
        Self: 'v,
    {
        let this = host.get_pin_mut().get_mut();
        if destination.remaining_mut() == 0 || this.data.is_empty() {
            return Poll::Ready(Ok(None));
        }
        if !this.ready {
            this.ready = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.ready = false;
        destination.put_u8(this.data[0]);
        this.data = &this.data[1..];
        Poll::Ready(Ok(segment(1)))
    }
}

#[test]
fn a_skip_dropped_midway_is_resumed() {
    let dribble = Dribble {
        data: INPUT,
        ready: false,
    };
    let mut stream = block(RecordStream::read(ConstructedAccess::new(dribble)))
        .unwrap()
        .unwrap();
    {
        let mut next = pin!(stream.next_record());
        let mut cx = Context::from_waker(Waker::noop());
        // Each poll after the first reads one byte: dropped after the first
        // of the two payload bytes left.
        for _ in 0..2 {
            assert!(next.as_mut().poll(&mut cx).is_pending());
        }
    }
    assert_eq!(stream.phase, Phase::InRecord);
    assert_eq!(stream.payload.0, 1);
    let mut record = block(stream.next_record()).unwrap().unwrap();
    assert_eq!(block(record.read_word()), Some(8));
    assert_eq!(stream.length, 3);
}

#[test]
fn a_head_dropped_midway_interrupts_the_stream() {
    let dribble = Dribble {
        data: &[1, 0, 2, 0, 9],
        ready: false,
    };
    let mut stream = block(RecordStream::read(ConstructedAccess::new(dribble)))
        .unwrap()
        .unwrap();
    {
        let mut next = pin!(stream.next_record());
        let mut cx = Context::from_waker(Waker::noop());
        // The second poll reads the payload byte, reaches the end of the
        // record and waits for the next length: dropped there.
        for _ in 0..2 {
            assert!(next.as_mut().poll(&mut cx).is_pending());
        }
    }
    assert_eq!(stream.phase, Phase::Interrupted);
    assert_eq!(
        block(stream.next_record()).err(),
        Some(NextError::Interrupted)
    );
}

/// A receiver that is not `Unpin`.
struct Fixed<'a> {
    data: &'a [u8],
    _pinned: PhantomPinned,
}
impl<'a> Fixed<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            _pinned: PhantomPinned,
        }
    }
}
family_receiver!(impl ['a] for Fixed<'a>);
impl Read for Fixed<'_> {
    type Error = Infallible;
    type Metadata = ();
    fn poll_read<'v, O: bytes::BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        _: &mut Context<'_>,
        destination: &mut O,
    ) -> Poll<Result<Option<nitori_io::ReadSegment<Self::Metadata>>, !>>
    where
        Self: 'v,
    {
        // SAFETY: `data` is not structurally pinned; nothing is moved out.
        let this = unsafe { host.get_pin_mut().get_unchecked_mut() };
        let length = destination.remaining_mut().min(this.data.len());
        destination.put_slice(&this.data[..length]);
        this.data = &this.data[length..];
        Poll::Ready(Ok(segment(length)))
    }
}

#[test]
fn a_receiver_pinned_on_the_stack() {
    let receiver = pin!(Fixed::new(INPUT));
    let mut stream = block(RecordStream::read(ConstructedAccess::new(receiver)))
        .unwrap()
        .unwrap();
    assert_eq!(block(stream.record().read_word()), Some(7));
    assert!(block(stream.next_record()).unwrap().is_some());
    assert_eq!(stream.length, 3);
}

#[test]
fn a_receiver_pinned_in_a_box() {
    fn open(data: &[u8]) -> RecordStream<ConstructedAccess<Pin<Box<Fixed<'_>>>>> {
        block(RecordStream::read(ConstructedAccess::new(Box::pin(
            Fixed::new(data),
        ))))
        .unwrap()
        .unwrap()
    }
    let mut stream = open(INPUT);
    assert_eq!(block(stream.record().read_word()), Some(7));
}

#[test]
fn a_pinned_receiver_for_one_stream_after_another() {
    let mut receiver = pin!(Fixed::new(INPUT));
    {
        let mut stream = block(RecordStream::read(ConstructedAccess::new(
            receiver.as_mut(),
        )))
        .unwrap()
        .unwrap();
        assert_eq!(block(stream.record().read_word()), Some(7));
    }
    let stream = block(RecordStream::read(ConstructedAccess::new(
        receiver.as_mut(),
    )))
    .unwrap()
    .unwrap();
    assert_eq!(stream.length, 3);
}

#[test]
fn send_when_the_receiver_is() {
    fn assert_send<T: Send>(_: &T) {}
    let mut stream = block(RecordStream::read(ConstructedAccess::new(INPUT)))
        .unwrap()
        .unwrap();
    assert_send(&stream.next_record());
}

#[test]
fn a_release_point_completes_at_once() {
    let mut access = ConstructedAccess::new(INPUT);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(pin!(access.release_view()).poll(&mut cx).is_ready());
    assert_eq!(block(access.read_array::<1>()).unwrap().value, [2]);
}

#[test]
fn the_view_is_at_hand_outside_a_poll() {
    let mut access = ConstructedAccess::new(INPUT);
    assert_eq!(
        access.with(|view| view.into_pin().get_pin_mut().len()),
        INPUT.len()
    );
    let mut stream = block(RecordStream::read(access)).unwrap().unwrap();
    let mut record = stream.record();
    assert_eq!(record.try_with(|_| ()), Some(()));
}

#[test]
fn items_of_an_operation_on_it() {
    let mut stream = block(RecordStream::read(ConstructedAccess::new(INPUT)))
        .unwrap()
        .unwrap();
    let mut record = block(stream.next_record()).unwrap().unwrap();
    let mut child = pin!(record.bytes());
    let mut items = Vec::new();
    let count = block(async {
        loop {
            match child.as_mut().step().await {
                std::ops::ControlFlow::Continue(byte) => items.push(byte),
                std::ops::ControlFlow::Break(count) => break count,
            }
        }
    });
    assert_eq!((items, count), (vec![0, 8, 0xff], 3));
}

#[test]
fn composed_once_and_twice() {
    let mut access = ConstructedAccess::new(INPUT);
    let mut outer = access.compose(ReadLimit(5usize));
    {
        let mut inner = outer.compose(ReadLimit(2usize));
        assert_eq!(block(inner.read_array::<2>()).unwrap().value, [2, 0]);
        assert!(block(inner.read_array::<1>()).is_err());
    }
    assert_eq!(block(outer.read_array::<3>()).unwrap().value, [7, 3, 0]);
    assert!(block(outer.read_array::<1>()).is_err());
    assert_eq!(outer.into_state().0, 0);
    assert_eq!(block(access.read_array::<1>()).unwrap().value, [8]);
}

fn segment(count: usize) -> Option<nitori_io::ReadSegment<()>> {
    core::num::NonZeroUsize::new(count).map(|count| nitori_io::ReadSegment {
        count,
        metadata: (),
    })
}
