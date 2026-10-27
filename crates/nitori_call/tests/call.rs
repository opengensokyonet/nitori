#![feature(impl_trait_in_assoc_type)]
mod support;

use futures_core::{FusedStream, Stream};
use nitori_call::{
    Call, CallPoll, Child, Compose, Direct, DirectView, DynCall, Emitter, Operation, Receiver,
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
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
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

/// Poll for the final value, discarding items.
fn poll<F: ReceiverFamily, Y, R>(
    call: Pin<&mut (impl DynCall<F, Y, R> + ?Sized)>,
    host: Pin<&mut impl Receiver<Family = F>>,
) -> Poll<R> {
    match call.poll_return_with(
        Some(pin!(host.view())),
        &mut Context::from_waker(Waker::noop()),
    ) {
        CallPoll::Ready(event) => Poll::Ready(event),
        CallPoll::Pending => Poll::Pending,
        CallPoll::NeedsView => unreachable!("a call given a view asked for one"),
    }
}

#[test]
fn read_record_from_memory() {
    let mut input = b"\x03cat\x03dog".as_slice();
    let mut output = Vec::new();
    {
        let mut call = pin!(Call::new(ReadRecord::new(&mut output)));
        assert!(matches!(
            poll(call.as_mut(), Pin::new(&mut input)),
            Poll::Ready(Ok(3))
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
        Poll::Ready(Err(ReadError::Incomplete { completed: 1 }))
    ));
    assert_eq!(input, b"\x02BC");
    let mut second = pin!(Call::new(ReadLimitedWord::new()));
    assert!(matches!(
        poll(second.as_mut(), Pin::new(&mut input)),
        Poll::Ready(Ok(word)) if word == *b"BC"
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
        Poll::Ready(Err(ReadError::Incomplete { completed: 1 }))
    ));
    assert_eq!(input, b"B");
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
            Poll::Ready(Ok(3))
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
    assert!(matches!(poll_connection(), Poll::Ready(Ok(2))));
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
    // One view for the whole pass, constructed once.
    let mut view = pin!(Pin::new(&mut resource).view());
    let mut cx = Context::from_waker(Waker::noop());
    let results: Vec<_> = calls
        .iter_mut()
        .map(|call| call.as_mut().poll_return_with(Some(view.as_mut()), &mut cx))
        .collect();
    assert!(matches!(results[0], CallPoll::Ready(Ok(pair)) if pair == *b"ab"));
    assert!(matches!(results[1], CallPoll::Ready(Ok(pair)) if pair == *b"cd"));
    assert_eq!(views.get(), 1);
}

/// An operation that emits, written by hand: two-byte chunks of `total`
/// bytes, then the number of bytes read.
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
fn emitted_items_precede_the_final_value() {
    let mut input = b"abcde".as_slice();
    let mut call = pin!(Call::<Memory, _>::new(ReadChunks { total: 5 }));
    let mut events = Vec::new();
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        let view = pin!(Pin::new(&mut input).view());
        match call.as_mut().poll_with(Some(view), &mut cx) {
            CallPoll::Ready(ControlFlow::Continue(chunk)) => events.push(chunk),
            CallPoll::Ready(ControlFlow::Break(result)) => {
                assert_eq!(result, Ok(5));
                break;
            }
            CallPoll::Pending | CallPoll::NeedsView => panic!("memory input does not wait"),
        }
    }
    assert_eq!(events, [b"ab".to_vec(), b"cd".to_vec(), b"e".to_vec()]);
}

#[test]
fn the_return_form_drops_items_without_suspending() {
    let views = Cell::new(0);
    let mut resource = Counted {
        input: b"abcde",
        views: &views,
    };
    let (wakes, waker) = WakeCount::new();
    let mut call = pin!(Call::<Memory, _>::new(ReadChunks { total: 5 }));
    let result = call.as_mut().poll_return_with(
        Some(pin!(Pin::new(&mut resource).view())),
        &mut Context::from_waker(&waker),
    );
    assert_eq!(result, CallPoll::Ready(Ok(5)));
    assert_eq!(views.get(), 1);
    assert_eq!(wakes.get(), 0);
}

#[test]
fn a_call_switches_from_its_items_to_its_final_value() {
    let mut input = b"abcde".as_slice();
    let (wakes, waker) = WakeCount::new();
    let mut cx = Context::from_waker(&waker);
    let mut call = pin!(Call::<Memory, _>::new(ReadChunks { total: 5 }));
    let first = call
        .as_mut()
        .poll_with(Some(pin!(Pin::new(&mut input).view())), &mut cx);
    assert_eq!(
        first,
        CallPoll::Ready(ControlFlow::Continue(b"ab".to_vec()))
    );
    let result = call
        .as_mut()
        .poll_return_with(Some(pin!(Pin::new(&mut input).view())), &mut cx);
    assert_eq!(result, CallPoll::Ready(Ok(5)));
    // The item made the first poll ready; nothing needed a wake.
    assert_eq!(wakes.get(), 0);
}

#[call]
async fn sum_child_chunks(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
) -> (usize, Record) {
    // A child with its own item type, started on the parent's access and
    // observed as a stream.
    let mut child = pin!(Child::new(ReadChunks { total: 4 }, &mut *io));
    let mut chunks = 0;
    let result = loop {
        match poll_fn(|cx| child.as_mut().poll_next(cx)).await {
            Some(ControlFlow::Continue(_)) => chunks += 1,
            Some(ControlFlow::Break(result)) => break result,
            None => unreachable!("the final value ends the stream"),
        }
    };
    assert!(poll_fn(|cx| child.as_mut().poll_next(cx)).await.is_none());
    (chunks, result)
}

#[test]
fn a_parent_observes_the_events_of_a_child() {
    let mut input = b"wxyz".as_slice();
    let mut call = pin!(Call::new(SumChildChunks::new()));
    assert!(matches!(
        poll(call.as_mut(), Pin::new(&mut input)),
        Poll::Ready((2, Ok(4)))
    ));
}

#[call]
async fn await_child(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) -> Record {
    // Awaited, a child discards its items.
    Child::new(ReadChunks { total: 3 }, io).await
}

#[test]
fn an_awaited_child_completes_with_its_final_value() {
    let mut input = b"pqr".as_slice();
    let (wakes, waker) = WakeCount::new();
    let mut call = pin!(Call::<Memory, _>::new(AwaitChild::new()));
    let result = call.as_mut().poll_return_with(
        Some(pin!(Pin::new(&mut input).view())),
        &mut Context::from_waker(&waker),
    );
    assert_eq!(result, CallPoll::Ready(Ok(3)));
    assert_eq!(wakes.get(), 0);
}

/// Uses its access while being dropped.
struct AccessOnDrop<'a, A: ReceiverAccess> {
    io: &'a mut A,
    pending: &'a Cell<bool>,
}
impl<A: ReceiverAccess> Drop for AccessOnDrop<'_, A> {
    fn drop(&mut self) {
        let mut cx = Context::from_waker(Waker::noop());
        let poll = self.io.poll_access(&mut cx, |_, _| Poll::Ready(()));
        self.pending.set(poll.is_pending());
    }
}

#[call]
async fn access_when_dropped(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    pending: &Cell<bool>,
) {
    let _guard = AccessOnDrop { io, pending };
    std::future::pending::<()>().await;
}

#[test]
fn an_access_outside_a_poll_has_no_view() {
    let pending = Cell::new(false);
    let mut input = b"".as_slice();
    let mut call = Box::pin(Call::new(AccessWhenDropped::new(&pending)));
    assert!(poll(call.as_mut(), Pin::new(&mut input)).is_pending());
    drop(call);
    assert!(pending.get());
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
fn assert_sync<T: Sync>(_: &T) {}

/// Counts bytes until the end of input through a borrow of its own local,
/// written before and after each wait.
#[call]
async fn count_bytes(io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>) -> usize {
    let mut count = 0;
    let counter = &mut count;
    while read_array::<1, _>(io).await.is_ok() {
        *counter += 1;
    }
    count
}

#[test]
fn a_shared_reference_to_a_running_call_leaves_it_intact() {
    let mut feed = Feed::new(b"ab");
    let mut call = pin!(Call::new(CountBytes::new()));
    assert!(poll(call.as_mut(), Pin::new(&mut feed)).is_pending());
    // Safe code can borrow a pinned call.
    let shared: &Call<_, _> = &call;
    assert!(!shared.is_terminated());
    assert_eq!(format!("{shared:?}"), "Call { terminated: false, .. }");
    feed.data.push_back(b'c');
    feed.closed = true;
    assert_eq!(poll(call.as_mut(), Pin::new(&mut feed)), Poll::Ready(3));
    assert!(call.is_terminated());
}

#[test]
fn a_call_is_sync_even_if_its_operation_is_not() {
    let pending = Cell::new(false);
    assert_sync(&Call::<Memory, _>::new(AccessWhenDropped::new(&pending)));
}

/// Emits a countdown and returns how many items it emitted, counting them
/// through a borrow of its own local written before each emission.
struct Countdown(u8);
impl<F: ReceiverFamily> Operation<F> for Countdown {
    type Output = u8;
    type Yield = u8;
}
impl<'s, A: ReceiverAccess> Start<'s, A> for Countdown {
    type Fut = impl Future<Output = u8>;
    fn start(self, _: A, mut emitter: Emitter<'s, u8>) -> Self::Fut {
        async move {
            let mut emitted = 0;
            let counter = &mut emitted;
            for item in (0..self.0).rev() {
                *counter += 1;
                emitter.emit(item).await;
            }
            emitted
        }
    }
}

/// Whether the child has terminated, and its size hint, before and after
/// each event.
#[call]
async fn observe_child(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
) -> Vec<(bool, (usize, Option<usize>))> {
    let mut child = pin!(Child::new(Countdown(2), &mut *io));
    let observe = |child: &Child<_, _>| (FusedStream::is_terminated(child), child.size_hint());
    let mut seen = vec![observe(&child)];
    while poll_fn(|cx| child.as_mut().poll_next(cx)).await.is_some() {
        seen.push(observe(&child));
    }
    assert_eq!(format!("{child:?}"), "Child { terminated: true, .. }");
    seen
}

#[test]
fn a_child_reports_when_it_has_terminated() {
    let mut input = b"".as_slice();
    let mut call = pin!(Call::new(ObserveChild::new()));
    let running = (false, (1, None));
    assert_eq!(
        poll(call.as_mut(), Pin::new(&mut input)),
        Poll::Ready(vec![running, running, running, (true, (0, Some(0)))])
    );
}

#[test]
fn a_call_over_a_generic_family_is_send() {
    fn build<F: Read<Error = Infallible>>() {
        let mut output = Vec::new();
        assert_send(&Call::<F, _>::new(ReadRecord::new(&mut output)));
        // Compositions and child streams derive from the slot, so they do not count.
        assert_send(&Call::<F, _>::new(ReadLimitedWord::new()));
        assert_send(&Call::<F, _>::new(SumChildChunks::new()));
        assert_send(&Call::<F, _>::new(ReadChunks { total: 1 }));
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
                Poll::Ready(Ok(2))
            ));
        });
    });
    assert_eq!(output, b"hi");
}

/// Polls without a view; if the call asks for one, polls again with a view.
fn lazily<F: ReceiverFamily, Y, R>(
    mut call: Pin<&mut (impl DynCall<F, Y, R> + ?Sized)>,
    mut host: Pin<&mut impl Receiver<Family = F>>,
) -> (bool, Poll<R>) {
    let mut cx = Context::from_waker(Waker::noop());
    let poll = match call.as_mut().poll_return_with(None, &mut cx) {
        CallPoll::NeedsView => call
            .as_mut()
            .poll_return_with(Some(pin!(host.as_mut().view())), &mut cx),
        poll => return (false, into_poll(poll)),
    };
    (true, into_poll(poll))
}
fn into_poll<T>(poll: CallPoll<T>) -> Poll<T> {
    match poll {
        CallPoll::Ready(event) => Poll::Ready(event),
        CallPoll::Pending => Poll::Pending,
        CallPoll::NeedsView => unreachable!("a call given a view asked for one"),
    }
}

#[test]
fn compositions_and_child_streams_ask_for_a_view_through_their_parent() {
    let mut input = b"\x01A\x02BC".as_slice();
    let mut call = pin!(Call::new(ReadLimitedWord::new()));
    let (requests, result) = lazily(call.as_mut(), Pin::new(&mut input));
    assert!(requests);
    assert!(matches!(result, Poll::Ready(Err(_))));

    let mut input = b"wxyz".as_slice();
    let mut call = pin!(Call::new(SumChildChunks::new()));
    let (requests, result) = lazily(call.as_mut(), Pin::new(&mut input));
    assert!(requests);
    assert!(matches!(result, Poll::Ready((2, Ok(4)))));
}

/// Polls its branch with a waker of its own, and only after that waker
/// fired, as executors such as `FuturesUnordered` do.
async fn only_when_woken<T>(branch: impl Future<Output = T>) -> T {
    let (wakes, waker) = WakeCount::new();
    let mut branch = pin!(branch);
    let mut seen = None;
    poll_fn(|_| {
        if seen == Some(wakes.get()) {
            return Poll::Pending;
        }
        seen = Some(wakes.get());
        branch.as_mut().poll(&mut Context::from_waker(&waker))
    })
    .await
}

#[call]
async fn read_record_in_branch(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    output: &mut Vec<u8>,
) -> Record {
    only_when_woken(read_record(io, output)).await
}

#[test]
fn a_branch_that_asks_is_polled_when_a_view_is_lent() {
    let mut input = b"\x02ok".as_slice();
    let mut output = Vec::new();
    let mut call = pin!(Call::<Memory, _>::new(ReadRecordInBranch::new(&mut output)));
    let (requests, result) = lazily(call.as_mut(), Pin::new(&mut input));
    assert!(requests);
    assert_eq!(result, Poll::Ready(Ok(2)));
}

/// Emits two items from inside a branch.
struct EmitInBranch;
impl<F: ReceiverFamily> Operation<F> for EmitInBranch {
    type Output = usize;
    type Yield = u8;
}
impl<'s, A: ReceiverAccess> Start<'s, A> for EmitInBranch {
    type Fut = impl Future<Output = usize>;
    fn start(self, _: A, mut emitter: Emitter<'s, u8>) -> Self::Fut {
        only_when_woken(async move {
            emitter.emit(1).await;
            emitter.emit(2).await;
            2
        })
    }
}

#[test]
fn an_item_from_a_branch_wakes_the_branch() {
    let mut input = b"".as_slice();
    let mut call = pin!(Call::<Memory, _>::new(EmitInBranch));
    let mut cx = Context::from_waker(Waker::noop());
    let mut events = Vec::new();
    for _ in 0..3 {
        match call
            .as_mut()
            .poll_with(Some(pin!(Pin::new(&mut input).view())), &mut cx)
        {
            CallPoll::Ready(event) => events.push(event),
            poll => panic!("the branch was not repolled: {poll:?}"),
        }
    }
    assert_eq!(
        events,
        [
            ControlFlow::Continue(1),
            ControlFlow::Continue(2),
            ControlFlow::Break(2)
        ]
    );
}

/// Polls each branch only after its waker fired. A branch's waker queues the
/// branch and wakes the executor's own waker, and a branch that woke itself
/// while polled makes the executor wake its waker before returning, as
/// `FuturesUnordered` does.
async fn branches<'a, T>(mut branches: Vec<Pin<Box<dyn Future<Output = T> + 'a>>>) -> Vec<T> {
    struct Branch {
        index: usize,
        queue: Arc<Mutex<Vec<usize>>>,
        parent: Arc<Mutex<Option<Waker>>>,
    }
    impl Wake for Branch {
        fn wake(self: Arc<Self>) {
            let mut queue = self.queue.lock().unwrap();
            if !queue.contains(&self.index) {
                queue.push(self.index);
            }
            drop(queue);
            if let Some(parent) = &*self.parent.lock().unwrap() {
                parent.wake_by_ref();
            }
        }
    }
    let queue = Arc::new(Mutex::new((0..branches.len()).collect::<Vec<_>>()));
    let parent = Arc::new(Mutex::new(None::<Waker>));
    let wakers: Vec<Waker> = (0..branches.len())
        .map(|index| {
            Waker::from(Arc::new(Branch {
                index,
                queue: queue.clone(),
                parent: parent.clone(),
            }))
        })
        .collect();
    let mut finished = vec![false; branches.len()];
    let mut results = Vec::new();
    poll_fn(|cx| {
        *parent.lock().unwrap() = Some(cx.waker().clone());
        let ready = std::mem::take(&mut *queue.lock().unwrap());
        for index in ready {
            if finished[index] {
                continue;
            }
            let poll = branches[index]
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[index]));
            if let Poll::Ready(result) = poll {
                finished[index] = true;
                results.push(result);
            }
        }
        if finished.iter().all(|finished| *finished) {
            return Poll::Ready(std::mem::take(&mut results));
        }
        if !queue.lock().unwrap().is_empty() {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    })
    .await
}

/// Opens once; waiting for it needs no resource.
#[derive(Default)]
struct Gate {
    open: Cell<bool>,
    waiter: RefCell<Option<Waker>>,
}
impl Gate {
    async fn wait(&self) {
        poll_fn(|cx| match self.open.get() {
            true => Poll::Ready(()),
            false => {
                *self.waiter.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
    fn open(&self) {
        self.open.set(true);
        if let Some(waiter) = self.waiter.take() {
            waiter.wake();
        }
    }
}

/// Each branch waits for its gate, then reads one byte.
#[call]
async fn read_after_gates<A: ReceiverAccess<Family: Read<Error = Infallible>>>(
    io: &mut A,
    gates: &[Gate],
) -> Vec<u8> {
    let io = RefCell::new(io);
    let io = &io;
    let reads = gates
        .iter()
        .map(|gate| {
            Box::pin(async move {
                gate.wait().await;
                let mut byte = [0];
                poll_fn(|cx| {
                    io.borrow_mut().poll_access(cx, |access, cx| {
                        <A::Family as Read>::poll_read(
                            access.into_pin(),
                            cx,
                            &mut byte.as_mut_slice(),
                        )
                    })
                })
                .await
                .unwrap();
                byte[0]
            }) as Pin<Box<dyn Future<Output = u8>>>
        })
        .collect();
    branches(reads).await
}

#[test]
fn branches_asking_on_different_polls_are_all_woken_when_a_view_is_lent() {
    let mut input = b"abc".as_slice();
    let gates: [Gate; 3] = Default::default();
    let (wakes, waker) = WakeCount::new();
    let mut cx = Context::from_waker(&waker);
    let mut call = pin!(Call::<Memory, _>::new(ReadAfterGates::new(&gates)));
    let without_view = |call: Pin<&mut Call<Memory, ReadAfterGates>>| {
        call.poll_return_with(None, &mut Context::from_waker(&waker))
    };
    assert_eq!(without_view(call.as_mut()), CallPoll::Pending);
    // Each gate wakes the task once; the branch it opens then asks for a
    // view, which wakes nothing while the view is being acquired.
    gates[0].open();
    assert_eq!(wakes.get(), 1);
    assert_eq!(without_view(call.as_mut()), CallPoll::NeedsView);
    gates[1].open();
    assert_eq!(wakes.get(), 2);
    assert_eq!(without_view(call.as_mut()), CallPoll::NeedsView);
    // The first branch's request is still waiting.
    assert_eq!(without_view(call.as_mut()), CallPoll::NeedsView);
    assert_eq!(wakes.get(), 2);
    // A view wakes both branches, which are then polled with it. The
    // executor wakes the call's waker for them, which does not reach the
    // task.
    let view = pin!(Pin::new(&mut input).view());
    assert_eq!(
        call.as_mut().poll_return_with(Some(view), &mut cx),
        CallPoll::Pending
    );
    assert_eq!(wakes.get(), 2);
    // Other wakes still do.
    gates[2].open();
    assert_eq!(wakes.get(), 3);
    assert_eq!(without_view(call.as_mut()), CallPoll::NeedsView);
    let view = pin!(Pin::new(&mut input).view());
    assert_eq!(
        call.as_mut().poll_return_with(Some(view), &mut cx),
        CallPoll::Ready(b"abc".to_vec())
    );
    assert_eq!(wakes.get(), 3);
}

/// Polls its future with a waker of its own, forwarding wakes, on every poll.
struct Substitute<F>(F);
impl<F: Future> Future for Substitute<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        struct Forward(Waker);
        impl Wake for Forward {
            fn wake(self: Arc<Self>) {
                self.0.wake_by_ref();
            }
        }
        let waker = Waker::from(Arc::new(Forward(cx.waker().clone())));
        // SAFETY: the future is a pinned field, never moved.
        unsafe { self.map_unchecked_mut(|this| &mut this.0) }.poll(&mut Context::from_waker(&waker))
    }
}

#[call]
async fn read_record_substituted(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    output: &mut Vec<u8>,
) -> Record {
    Substitute(read_record(io, output)).await
}

#[test]
fn a_wrapper_that_substitutes_the_waker_is_not_woken_by_requests() {
    let mut feed = Feed::new(b"\x02");
    let (wakes, waker) = WakeCount::new();
    let mut cx = Context::from_waker(&waker);
    let mut output = Vec::new();
    {
        let mut call = pin!(Call::new(ReadRecordSubstituted::new(&mut output)));
        let mut lend = |mut call: Pin<&mut Call<Direct<Feed>, ReadRecordSubstituted>>,
                        feed: &mut Feed| {
            match call.as_mut().poll_return_with(None, &mut cx) {
                CallPoll::NeedsView => {
                    call.poll_return_with(Some(pin!(Pin::new(feed).view())), &mut cx)
                }
                poll => poll,
            }
        };
        // The first view wakes the wrapper's waker, which the wrapper sent to
        // the task before the call intercepted.
        assert_eq!(lend(call.as_mut(), &mut feed), CallPoll::Pending);
        assert_eq!(wakes.get(), 1);
        // Polls without new data ask for a view again but wake nothing.
        for _ in 0..3 {
            assert_eq!(lend(call.as_mut(), &mut feed), CallPoll::Pending);
        }
        assert_eq!(wakes.get(), 1);
        feed.data.extend(*b"ok");
        feed.waker.take().unwrap().wake();
        assert_eq!(wakes.get(), 2);
        assert_eq!(lend(call.as_mut(), &mut feed), CallPoll::Ready(Ok(2)));
        assert_eq!(wakes.get(), 2);
    }
    assert_eq!(output, b"ok");
}
