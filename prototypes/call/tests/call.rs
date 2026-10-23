#![feature(impl_trait_in_assoc_type)]
mod support;

use nitori_call_prototype::{
    BorrowedScope, Call, Compose, Direct, DirectView, DynCall, Emitter, Operation, Receiver,
    ReceiverAccess, ReceiverFamily, Start, call, direct_receiver,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    convert::Infallible,
    future::{Future, poll_fn},
    marker::PhantomPinned,
    ops::ControlFlow,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};
use support::*;

type Memory<'a> = Direct<&'a [u8]>;
type Record = Result<usize, ReadError<Infallible>>;
type Pair = Result<[u8; 2], ReadError<Infallible>>;
type Boxed<'a, F, R> = Pin<Box<dyn DynCall<F, Infallible, R> + 'a>>;

#[call]
async fn read_record(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    output: &mut Vec<u8>,
) -> Record {
    let [length] = read_array::<1, _>(io).await?;
    read_exact(io, output, length.into()).await?;
    Ok(length.into())
}

fn poll<F: ReceiverFamily, Y, R>(
    call: Pin<&mut (impl DynCall<F, Y, R> + ?Sized)>,
    host: Pin<&mut impl Receiver<Family = F>>,
) -> Poll<ControlFlow<R, Y>> {
    call.poll_with(
        pin!(BorrowedScope::new(host)),
        &mut Context::from_waker(Waker::noop()),
    )
}

#[test]
fn read_record_from_memory() {
    let mut input = b"\x03cat\x03dog".as_slice();
    let mut output = Vec::new();
    {
        let mut call = pin!(Call::new(ReadRecord::new(&mut output)));
        assert!(matches!(
            poll(call.as_mut(), Pin::new(&mut input)),
            Poll::Ready(ControlFlow::Break(Ok(3)))
        ));
    }
    assert_eq!(input, b"\x03dog");
    assert_eq!(output, b"cat");
}

#[call]
async fn read_limited_word(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) -> Pair {
    let [length] = read_array::<1, _>(io).await?;
    read_array::<2, _>(&mut io.compose(ReadLimit(length.into()))).await
}

#[test]
fn composition_keeps_a_child_inside_its_record() {
    // Without the boundary, a two-byte parser consumes the next record's length.
    let mut input = b"\x01A\x02BC".as_slice();
    let mut first = pin!(Call::new(ReadLimitedWord::new()));
    assert!(matches!(
        poll(first.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break(Err(ReadError::Incomplete {
            completed: 1
        })))
    ));
    assert_eq!(input, b"\x02BC");
    let mut second = pin!(Call::new(ReadLimitedWord::new()));
    assert!(matches!(
        poll(second.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break(Ok(word))) if word == *b"BC"
    ));
}

/// Composition state that must stay pinned while composed.
struct PinnedLimit {
    limit: ReadLimit,
    _pinned: PhantomPinned,
}
impl<F: ReceiverFamily> Compose<F> for PinnedLimit {
    type Family = Limited<F>;
    fn compose<'s, 'p>(
        self: Pin<&'s mut Self>,
        parent: Pin<&'s mut F::ReceiverView<'p>>,
    ) -> LimitedView<'s, F>
    where
        F: 'p,
        Self::Family: 's,
        'p: 's,
    {
        // SAFETY: the limit is never moved out of the pinned state.
        let limit = unsafe { self.map_unchecked_mut(|state| &mut state.limit) };
        Pin::new(limit.get_mut()).compose(parent)
    }
}

#[call]
async fn read_pinned_word(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) -> Pair {
    let state = pin!(PinnedLimit {
        limit: ReadLimit(1),
        _pinned: PhantomPinned,
    });
    read_array::<2, _>(&mut io.compose(state)).await
}

#[test]
fn pinned_state_composes_through_a_pin() {
    let mut input = b"AB".as_slice();
    let mut call = pin!(Call::new(ReadPinnedWord::new()));
    assert!(matches!(
        poll(call.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break(Err(ReadError::Incomplete {
            completed: 1
        })))
    ));
    assert_eq!(input, b"B");
}

#[call]
async fn read_erased_word(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) -> Pair {
    let mut limited = io.compose(ReadLimit(2));
    read_array::<2, _>(&mut limited.erase()).await
}

#[test]
fn an_erased_composition_is_a_slot() {
    let mut input = b"XYZ".as_slice();
    let mut call = pin!(Call::new(ReadErasedWord::new()));
    assert!(matches!(
        poll(call.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break(Ok(word))) if word == *b"XY"
    ));
    assert_eq!(input, b"Z");
}

#[test]
fn a_call_may_use_a_different_host_on_each_poll() {
    // The call is stored apart from its hosts and resumes with whichever is lent.
    let mut output = Vec::new();
    let mut second = Feed::new(b"t!");
    {
        let mut call = pin!(Call::new(ReadRecord::new(&mut output)));
        let mut first = Feed::new(b"\x03ca");
        assert!(poll(call.as_mut(), Pin::new(&mut first)).is_pending());
        assert!(first.waker.is_some());
        assert!(matches!(
            poll(call.as_mut(), Pin::new(&mut second)),
            Poll::Ready(ControlFlow::Break(Ok(3)))
        ));
    }
    assert_eq!(second.data, b"!");
    assert_eq!(output, b"cat");
}

/// The call's type is nameable, so it can be a field beside its resource.
struct Connection<'a> {
    input: Feed,
    reader: Call<Direct<Feed>, ReadRecord<'a>>,
}

#[test]
fn a_call_is_stored_beside_the_resource_it_uses() {
    let mut output = Vec::new();
    let mut connection = Box::pin(Connection {
        input: Feed::new(b"\x02h"),
        reader: Call::new(ReadRecord::new(&mut output)),
    });
    let mut poll_connection = || {
        // SAFETY: the reader stays pinned inside the pinned connection.
        let Connection { input, reader } = unsafe { connection.as_mut().get_unchecked_mut() };
        let reader = unsafe { Pin::new_unchecked(reader) };
        let result = poll(reader, Pin::new(&mut *input));
        input.data.extend(*b"i");
        result
    };
    assert!(poll_connection().is_pending());
    assert!(matches!(
        poll_connection(),
        Poll::Ready(ControlFlow::Break(Ok(2)))
    ));
    drop(connection);
    assert_eq!(output, b"hi");
}

/// A buffer that counts how often a view is constructed.
struct Counted<'a> {
    input: &'a [u8],
    views: &'a Cell<usize>,
}
impl<'a> Receiver for Counted<'a> {
    type Family = Memory<'a>;
    fn view<'visit>(self: Pin<&'visit mut Self>) -> DirectView<'visit, &'a [u8]>
    where
        Self::Family: 'visit,
    {
        let this = self.get_mut();
        this.views.set(this.views.get() + 1);
        DirectView(Pin::new(&mut this.input))
    }
}

#[call]
async fn read_pair(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) -> Pair {
    let [first] = read_array::<1, _>(io).await?;
    let [second] = read_array::<1, _>(io).await?;
    Ok([first, second])
}

#[test]
fn an_executor_lends_one_view_to_several_calls_in_one_poll() {
    let views = Cell::new(0);
    let mut resource = Counted {
        input: b"abcd",
        views: &views,
    };
    let mut calls: Vec<Boxed<'_, Memory, Pair>> = vec![
        Box::pin(Call::new(ReadPair::new())),
        Box::pin(Call::new(ReadPair::new())),
    ];
    // One scope for the whole poll: the view is constructed on first use.
    let mut scope = pin!(BorrowedScope::new(Pin::new(&mut resource)));
    let mut cx = Context::from_waker(Waker::noop());
    let results: Vec<_> = calls
        .iter_mut()
        .map(|call| call.as_mut().poll_with(scope.as_mut(), &mut cx))
        .collect();
    assert!(matches!(results[0], Poll::Ready(ControlFlow::Break(Ok(pair))) if pair == *b"ab"));
    assert!(matches!(results[1], Poll::Ready(ControlFlow::Break(Ok(pair))) if pair == *b"cd"));
    assert_eq!(views.get(), 1);
}

/// An operation that emits, written by hand.
struct ReadChunks {
    total: usize,
}
impl<F: Read<Error = Infallible>> Operation<F> for ReadChunks {
    type Output = Record;
    type Yield = Vec<u8>;
}
impl<'s, A: ReceiverAccess<Family: Read<Error = Infallible>>> Start<'s, A> for ReadChunks {
    type Fut = impl Future<Output = Record>;
    fn start(self, mut io: A, mut emitter: Emitter<'s, Vec<u8>>) -> Self::Fut {
        async move {
            let mut completed = 0;
            while completed < self.total {
                let mut chunk = Vec::new();
                read_exact(&mut io, &mut chunk, 2.min(self.total - completed)).await?;
                completed += chunk.len();
                emitter.emit(chunk).await;
            }
            Ok(completed)
        }
    }
}

#[test]
fn emitted_items_precede_the_result() {
    let mut input = b"abcde".as_slice();
    let mut call = pin!(Call::new(ReadChunks { total: 5 }));
    let mut events = Vec::new();
    loop {
        match poll(call.as_mut(), Pin::new(&mut input)) {
            Poll::Ready(ControlFlow::Continue(chunk)) => events.push(chunk),
            Poll::Ready(ControlFlow::Break(result)) => {
                assert_eq!(result, Ok(5));
                break;
            }
            Poll::Pending => panic!("memory input does not wait"),
        }
    }
    assert_eq!(events, [b"ab".to_vec(), b"cd".to_vec(), b"e".to_vec()]);
}

#[test]
fn return_mode_discards_items_within_one_scope() {
    let views = Cell::new(0);
    let mut resource = Counted {
        input: b"abcde",
        views: &views,
    };
    let mut call = pin!(Call::new(ReadChunks { total: 5 }));
    let result = call.as_mut().poll_return_with(
        pin!(BorrowedScope::new(Pin::new(&mut resource))),
        &mut Context::from_waker(Waker::noop()),
    );
    assert!(matches!(result, Poll::Ready(Ok(5))));
    assert_eq!(views.get(), 1);
}

#[call]
async fn sum_child_chunks(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
) -> (usize, Record) {
    // A nested call with its own item type, driven through the parent's access.
    let mut child = pin!(Call::new(ReadChunks { total: 4 }));
    let mut chunks = 0;
    let result = loop {
        match poll_fn(|cx| io.poll_call(child.as_mut(), cx)).await {
            ControlFlow::Continue(_) => chunks += 1,
            ControlFlow::Break(result) => break result,
        }
    };
    (chunks, result)
}

#[test]
fn a_parent_observes_the_events_of_a_nested_call() {
    let mut input = b"wxyz".as_slice();
    let mut call = pin!(Call::new(SumChildChunks::new()));
    assert!(matches!(
        poll(call.as_mut(), Pin::new(&mut input)),
        Poll::Ready(ControlFlow::Break((2, Ok(4))))
    ));
}

#[call]
async fn panic_after_read(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) {
    let _ = read_array::<1, _>(io).await;
    panic!("operation failed");
}

#[test]
fn a_call_is_poisoned_by_a_panic() {
    let mut input = b"a".as_slice();
    let mut call = Box::pin(Call::new(PanicAfterRead::new()));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poll(call.as_mut(), Pin::new(&mut input))
    }));
    assert!(unwound.is_err());
    let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poll(call.as_mut(), Pin::new(&mut input))
    }));
    let message = again.unwrap_err();
    assert_eq!(
        message.downcast_ref::<&str>(),
        Some(&"call polled after a panic")
    );
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn a_call_over_a_generic_family_is_send() {
    fn build<F: Read<Error = Infallible>>() {
        let mut output = Vec::new();
        assert_send(&Call::<F, _>::new(ReadRecord::new(&mut output)));
        // Erasing and composing derive from the slot, so they do not count.
        assert_send(&Call::<F, _>::new(ReadErasedWord::new()));
        assert_send(&Call::<F, _>::new(SumChildChunks::new()));
    }
    build::<Memory<'_>>();
}

/// Bytes shared within one thread: neither the resource nor its views are Send.
#[derive(Default)]
struct Shared(Rc<RefCell<VecDeque<u8>>>);
direct_receiver!(impl [] for Shared);
impl Read for Direct<Shared> {
    type Error = Infallible;
    fn poll_read<'visit, Output: bytes::BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        _: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'visit,
    {
        let mut data = host.get_mut().0.0.borrow_mut();
        if data.is_empty() && destination.has_remaining_mut() {
            return Poll::Pending;
        }
        let count = data.len().min(destination.remaining_mut());
        for byte in data.drain(..count) {
            destination.put_u8(byte);
        }
        Poll::Ready(Ok(count))
    }
}

#[test]
fn a_call_over_a_resource_that_is_not_send_moves_between_threads() {
    let mut output = Vec::new();
    let mut call = Box::pin(Call::new(ReadRecord::new(&mut output)));
    let first = Shared::default();
    first.0.borrow_mut().extend(*b"\x02h");
    assert!(poll(call.as_mut(), Pin::new(&mut { first })).is_pending());
    std::thread::scope(|scope| {
        scope.spawn(move || {
            // A resource created on this thread finishes the call.
            let second = Shared::default();
            second.0.borrow_mut().extend(*b"i");
            assert!(matches!(
                poll(call.as_mut(), Pin::new(&mut { second })),
                Poll::Ready(ControlFlow::Break(Ok(2)))
            ));
        });
    });
    assert_eq!(output, b"hi");
}
