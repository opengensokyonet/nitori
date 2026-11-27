//! Record operations over either a receiver or a call slot.
#![feature(impl_trait_in_assoc_type, yield_expr)]
use futures_util::StreamExt as _;
use nitori_call::{AccessMode, AccessOperation, Receiver, ReceiverAccess, call, family_receiver};
use nitori_io::{Read, adapters::ReadLimit, prelude::*};
use std::{
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

/// Records of one length byte and a payload.
struct RecordStream<I> {
    io: I,
    state: RecordState,
}

struct RecordState {
    payload: usize,
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

struct ReadHead;

impl<A: ReceiverAccess<Family: Read>> AccessOperation<A> for ReadHead {
    type Output = Result<Option<u8>, ReadError<A>>;

    fn run_on(self, io: &mut A) -> impl Future<Output = Self::Output> {
        read_length(io)
    }
}

struct NextRecord<'a>(&'a mut RecordState);

impl<A: ReceiverAccess<Family: Read>> AccessOperation<A> for NextRecord<'_> {
    type Output = Result<bool, NextError<ReadError<A>>>;

    async fn run_on(self, io: &mut A) -> Self::Output {
        let state = self.0;
        match state.phase {
            Phase::Ended => return Ok(false),
            Phase::Interrupted => return Err(NextError::Interrupted),
            Phase::InRecord => {}
        }
        drain(&mut ReadLimit::borrowing(&mut *io, &mut state.payload))
            .await
            .map_err(NextError::Read)?;
        state.phase = Phase::Interrupted;
        match read_length(io).await.map_err(NextError::Read)? {
            None => {
                state.phase = Phase::Ended;
                Ok(false)
            }
            Some(length) => {
                state.payload = usize::from(length);
                state.length = length;
                state.phase = Phase::InRecord;
                Ok(true)
            }
        }
    }
}

struct ReadPayload<'a, const N: usize>(&'a mut usize);

impl<A, const N: usize> AccessOperation<A> for ReadPayload<'_, N>
where
    A: ReceiverAccess<Family: Read<Metadata: nitori_io::Combine>>,
{
    type Output = Option<[u8; N]>;

    async fn run_on(self, io: &mut A) -> Self::Output {
        ReadLimit::borrowing(io, self.0)
            .read_array::<N>()
            .await
            .ok()
            .map(|read| read.value)
    }
}

impl<I> RecordStream<I> {
    async fn read<How: AccessMode, E>(mut io: I) -> Result<Option<Self>, E>
    where
        ReadHead: AccessOperation<I, How, Output = Result<Option<u8>, E>>,
    {
        Ok(ReadHead.run_on(&mut io).await?.map(|length| Self {
            io,
            state: RecordState {
                payload: usize::from(length),
                length,
                phase: Phase::InRecord,
            },
        }))
    }

    fn next_record<'a, How: AccessMode>(
        &'a mut self,
    ) -> impl Future<Output = <NextRecord<'a> as AccessOperation<I, How>>::Output>
    where
        NextRecord<'a>: AccessOperation<I, How>,
    {
        NextRecord(&mut self.state).run_on(&mut self.io)
    }

    async fn read_word<'a, How: AccessMode>(&'a mut self) -> Option<u16>
    where
        ReadPayload<'a, 2>: AccessOperation<I, How, Output = Option<[u8; 2]>>,
    {
        ReadPayload(&mut self.state.payload)
            .run_on(&mut self.io)
            .await
            .map(u16::from_be_bytes)
    }

    async fn read_byte<'a, How: AccessMode>(&'a mut self) -> Option<u8>
    where
        ReadPayload<'a, 1>: AccessOperation<I, How, Output = Option<[u8; 1]>>,
    {
        let [byte] = ReadPayload(&mut self.state.payload)
            .run_on(&mut self.io)
            .await?;
        Some(byte)
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
    words.push(stream.read_word().await);
    while let Ok(true) = stream.next_record().await {
        words.push(stream.read_word().await);
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
    let mut stream = block(RecordStream::read(INPUT)).unwrap().unwrap();
    assert_eq!(block(stream.read_word()), Some(7));
    assert!(block(stream.next_record()).unwrap());
    assert_eq!(block(stream.read_byte()), Some(0));
    // What is left of the record is kept in the stream.
    assert_eq!(block(stream.read_byte()), Some(8));
    assert!(block(stream.next_record()).unwrap());
    assert_eq!(stream.state.length, 0);
    assert_eq!(block(stream.read_word()), None);
    assert!(block(stream.next_record()).unwrap());
    assert_eq!(block(stream.read_word()), Some(9));
    assert!(!block(stream.next_record()).unwrap());
    assert_eq!(stream.state.phase, Phase::Ended);
    assert!(!block(stream.next_record()).unwrap());
    assert!(stream.io.is_empty());
}

#[test]
fn on_a_borrowed_receiver() {
    let mut input = INPUT;
    {
        let mut stream = block(RecordStream::read(&mut input)).unwrap().unwrap();
        assert_eq!(block(stream.read_word()), Some(7));
    }
    assert_eq!(input, &INPUT[3..]);
}

#[test]
fn the_receiver_between_calls() {
    let mut input = INPUT;
    assert_eq!(block(ReadHead.run_on(&mut input)).unwrap(), Some(2));
    assert_eq!(input.len(), INPUT.len() - 1);
    input = &INPUT[1..];
    assert_eq!(input.sync_read_word_unpin(), Some(7));
}

/// One byte per read, each after a Pending.
struct Dribble<'a> {
    data: &'a [u8],
    ready: bool,
}
family_receiver!(impl ['a] for Dribble<'a>);
impl Read for Dribble<'_> {
    type Error = !;
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
    let mut stream = block(RecordStream::read(dribble)).unwrap().unwrap();
    {
        let mut next = pin!(stream.next_record());
        let mut cx = Context::from_waker(Waker::noop());
        // Each poll after the first reads one byte: dropped after the first
        // of the two payload bytes left.
        for _ in 0..2 {
            assert!(next.as_mut().poll(&mut cx).is_pending());
        }
    }
    assert_eq!(stream.state.phase, Phase::InRecord);
    assert_eq!(stream.state.payload, 1);
    assert!(block(stream.next_record()).unwrap());
    assert_eq!(block(stream.read_word()), Some(8));
    assert_eq!(stream.state.length, 3);
}

#[test]
fn a_head_dropped_midway_interrupts_the_stream() {
    let dribble = Dribble {
        data: &[1, 0, 2, 0, 9],
        ready: false,
    };
    let mut stream = block(RecordStream::read(dribble)).unwrap().unwrap();
    {
        let mut next = pin!(stream.next_record());
        let mut cx = Context::from_waker(Waker::noop());
        // The second poll reads the payload byte, reaches the end of the
        // record and waits for the next length: dropped there.
        for _ in 0..2 {
            assert!(next.as_mut().poll(&mut cx).is_pending());
        }
    }
    assert_eq!(stream.state.phase, Phase::Interrupted);
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
    type Error = !;
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
    let mut stream = block(RecordStream::read(receiver)).unwrap().unwrap();
    assert_eq!(block(stream.read_word()), Some(7));
    assert!(block(stream.next_record()).unwrap());
    assert_eq!(stream.state.length, 3);
}

#[test]
fn a_receiver_pinned_in_a_box() {
    fn open(data: &[u8]) -> RecordStream<Pin<Box<Fixed<'_>>>> {
        block(RecordStream::read(Box::pin(Fixed::new(data))))
            .unwrap()
            .unwrap()
    }
    let mut stream = open(INPUT);
    assert_eq!(block(stream.read_word()), Some(7));
}

#[test]
fn a_pinned_receiver_for_one_stream_after_another() {
    let mut receiver = pin!(Fixed::new(INPUT));
    {
        let mut stream = block(RecordStream::read(receiver.as_mut()))
            .unwrap()
            .unwrap();
        assert_eq!(block(stream.read_word()), Some(7));
    }
    let stream = block(RecordStream::read(receiver.as_mut()))
        .unwrap()
        .unwrap();
    assert_eq!(stream.state.length, 3);
}

async fn read_through_access<A: ReceiverAccess<Family: Read<Metadata: nitori_io::Combine>>>(
    io: &mut A,
) -> Option<u16> {
    RecordStream::read(io).await.ok()??.read_word().await
}

async fn read_through_receiver<
    R: Receiver<Family: Read<Metadata: nitori_io::Combine>> + Unpin + Send,
>(
    io: &mut R,
) -> Option<u16> {
    RecordStream::read(io).await.ok()??.read_word().await
}

#[call(sync)]
async fn generic_access<F: Read<Metadata: nitori_io::Combine>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Option<u16> {
    read_through_access(io).await
}

#[test]
fn inferred_modes_for_generic_access_and_receiver() {
    let mut input = INPUT;
    assert_eq!(input.sync_generic_access_unpin(), Some(7));
    let mut input = INPUT;
    fn assert_send<F: Future + Send>(future: F) -> F {
        future
    }
    assert_eq!(
        block(assert_send(read_through_receiver(&mut input))),
        Some(7)
    );
    let mut stream = block(RecordStream::read(INPUT)).unwrap().unwrap();
    assert!(block(assert_send(stream.next_record())).unwrap());
}

#[call(sync)]
async fn release<F: Read<Metadata = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> [u8; 1] {
    io.release_view().await;
    io.read_array::<1>().await.unwrap().value
}

#[test]
fn a_release_point_completes_at_once_with_a_fixed_view() {
    let mut input = INPUT;
    assert_eq!(input.sync_release_unpin(), [2]);
}

#[test]
fn items_of_an_operation_on_the_current_record() {
    let mut stream = block(RecordStream::read(INPUT)).unwrap().unwrap();
    assert!(block(stream.next_record()).unwrap());
    let mut receiver = ReadLimit::borrowing(&mut stream.io, &mut stream.state.payload);
    let mut operation = pin!(receiver.bytes_unpin());
    let mut items = Vec::new();
    let count = block(async {
        loop {
            match operation.as_mut().next().await.unwrap() {
                std::ops::ControlFlow::Continue(byte) => items.push(byte),
                std::ops::ControlFlow::Break(count) => break count,
            }
        }
    });
    assert_eq!((items, count), (vec![0, 8, 0xff], 3));
}

#[call(sync)]
async fn check_nested_limits<F: Read<Metadata = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) {
    let mut outer = ReadLimit::new(&mut *io, 5usize);
    {
        let mut inner = ReadLimit::new(&mut outer, 2usize);
        assert_eq!(inner.read_array::<2>().await.unwrap().value, [2, 0]);
        assert!(inner.read_array::<1>().await.is_err());
    }
    assert_eq!(outer.read_array::<3>().await.unwrap().value, [7, 3, 0]);
    assert!(outer.read_array::<1>().await.is_err());
    assert_eq!(outer.into_parts().1, 0);
    assert_eq!(io.read_array::<1>().await.unwrap().value, [8]);
}

#[test]
fn composed_once_and_twice() {
    let mut input = INPUT;
    input.sync_check_nested_limits_unpin();
}

fn segment(count: usize) -> Option<nitori_io::ReadSegment<()>> {
    core::num::NonZeroUsize::new(count).map(|count| nitori_io::ReadSegment {
        count,
        metadata: (),
    })
}

struct CountedInput<'a> {
    data: &'a [u8],
    views: usize,
}

impl<'a> Receiver for CountedInput<'a> {
    type Family = nitori_call::Direct<&'a [u8]>;

    fn view<'v>(self: Pin<&'v mut Self>) -> nitori_call::DirectView<'v, &'a [u8]>
    where
        Self::Family: 'v,
    {
        let this = self.get_mut();
        this.views += 1;
        nitori_call::DirectView(Pin::new(&mut this.data))
    }
}

#[test]
fn one_view_for_the_skip_and_header_of_a_record_operation() {
    let bytes = [3, 0, 7, 0xff, 2, 0, 9];
    let mut receiver = CountedInput {
        data: &bytes,
        views: 0,
    };
    assert_eq!(receiver.sync_words_unpin(), [Some(7), Some(9)]);
    assert_eq!(receiver.views, 1);

    let receiver = CountedInput {
        data: &bytes,
        views: 0,
    };
    let mut stream = block(RecordStream::read(receiver)).unwrap().unwrap();
    assert_eq!(block(stream.read_word()), Some(7));
    assert!(block(stream.next_record()).unwrap());
    assert_eq!(block(stream.read_word()), Some(9));
    assert_eq!(stream.io.views, 4);
    assert!(stream.io.data.is_empty());
}
