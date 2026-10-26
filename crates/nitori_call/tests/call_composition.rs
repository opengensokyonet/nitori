//! Run the calls of a source in order, without constructing the next one early.
#![feature(impl_trait_in_assoc_type, yield_expr)]

mod support;

use futures_core::Stream;
use nitori_call::{
    CallExecutor, Child, Direct, Emitter, Operation, ReceiverAccess, ReceiverFamily, Start, call,
};
use std::{
    future::{Future, poll_fn},
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

/// The next event of a stream that never waits.
fn next<S: Stream + ?Sized>(stream: Pin<&mut S>) -> Option<S::Item> {
    match stream.poll_next(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(event) => event,
        Poll::Pending => panic!("memory resources do not wait"),
    }
}

/// Adapt an iterator, including `from_fn` factories, without prefetching.
pub struct IterCalls<Source>(pub Source);
// The iterator is never structurally pinned.
impl<Source> Unpin for IterCalls<Source> {}
impl<Source: Iterator> Stream for IterCalls<Source> {
    type Item = Source::Item;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().0.next())
    }
}

// `#[call]` cannot name the access in a bound on another operation, so these
// operations, generic over the calls they run, are written by hand.

/// Yields each call's final value, dropping its items. A final value such as
/// `Err` is an item here, not a reason to stop.
pub struct CallResults<Source>(pub Source);
impl<F: ReceiverFamily, Source: Stream<Item: Operation<F>>> Operation<F> for CallResults<Source> {
    type Output = ();
    type Yield = <Source::Item as Operation<F>>::Output;
}
impl<'s, A, Source> Start<'s, A> for CallResults<Source>
where
    A: ReceiverAccess,
    Source: Stream<Item: Operation<A::Family> + for<'e, 'x> Start<'e, &'x mut A>>,
{
    type Fut = impl Future<Output = ()>;
    fn start(self, mut io: A, mut emitter: Emitter<'s, Output<A, Source::Item>>) -> Self::Fut {
        async move {
            let mut source = pin!(self.0);
            while let Some(operation) = poll_fn(|cx| source.as_mut().poll_next(cx)).await {
                let output = Child::new(operation, &mut io).await;
                emitter.emit(output).await;
            }
        }
    }
}

/// Yields each call's events, its final value included, in execution order.
/// Completes only when the source ends.
pub struct CallEvents<Source>(pub Source);
impl<F: ReceiverFamily, Source: Stream<Item: Operation<F>>> Operation<F> for CallEvents<Source> {
    type Output = ();
    type Yield =
        ControlFlow<<Source::Item as Operation<F>>::Output, <Source::Item as Operation<F>>::Yield>;
}
impl<'s, A, Source> Start<'s, A> for CallEvents<Source>
where
    A: ReceiverAccess,
    Source: Stream<Item: Operation<A::Family> + for<'e, 'x> Start<'e, &'x mut A>>,
{
    type Fut = impl Future<Output = ()>;
    fn start(
        self,
        mut io: A,
        mut emitter: Emitter<'s, ControlFlow<Output<A, Source::Item>, Yield<A, Source::Item>>>,
    ) -> Self::Fut {
        async move {
            let mut source = pin!(self.0);
            while let Some(operation) = poll_fn(|cx| source.as_mut().poll_next(cx)).await {
                let mut child = pin!(Child::new(operation, &mut io));
                while let Some(event) = child.as_mut().next().await {
                    emitter.emit(event).await;
                }
            }
        }
    }
}

type Output<A, Op> = <Op as Operation<<A as ReceiverAccess>::Family>>::Output;
type Yield<A, Op> = <Op as Operation<<A as ReceiverAccess>::Family>>::Yield;

/// A counter family for the operations below.
pub trait Counter: ReceiverFamily {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, amount: usize) -> usize
    where
        Self: 'v;
}
impl Counter for Direct<usize> {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, amount: usize) -> usize
    where
        Self: 'v,
    {
        let mut total = view.get_mut().0.as_mut();
        *total += amount;
        *total
    }
}

async fn add_now<A: ReceiverAccess<Family: Counter>>(io: &mut A, amount: usize) -> usize {
    poll_fn(|cx| {
        io.poll_access(cx, |access, _| {
            Poll::Ready(A::Family::add(access.into_pin(), amount))
        })
    })
    .await
}

/// Yield the previous total, then add.
#[call(yields = usize)]
async fn add<A: ReceiverAccess<Family: Counter>>(io: &mut A, amount: usize) -> usize {
    yield add_now(io, 0).await;
    add_now(io, amount).await
}

#[test]
fn results_and_events_follow_the_source() {
    let mut total = 0usize;
    let mut amounts = [2, 3].into_iter();
    let factory = std::iter::from_fn(move || amounts.next().map(Add::new));
    {
        let mut results = pin!(Pin::new(&mut total).execute(CallResults(IterCalls(factory))));
        assert_eq!(next(results.as_mut()), Some(ControlFlow::Continue(2)));
        assert_eq!(next(results.as_mut()), Some(ControlFlow::Continue(5)));
        assert_eq!(next(results.as_mut()), Some(ControlFlow::Break(())));
        assert_eq!(next(results.as_mut()), None);
    }
    assert_eq!(total, 5);
    let mut events =
        pin!(Pin::new(&mut total).execute(CallEvents(IterCalls([Add::new(4)].into_iter()))));
    assert_eq!(
        next(events.as_mut()),
        Some(ControlFlow::Continue(ControlFlow::Continue(5)))
    );
    assert_eq!(
        next(events.as_mut()),
        Some(ControlFlow::Continue(ControlFlow::Break(9)))
    );
    assert_eq!(next(events.as_mut()), Some(ControlFlow::Break(())));
}

#[test]
fn awaiting_keeps_only_the_final_value() {
    let mut total = 0usize;
    let mut cx = Context::from_waker(Waker::noop());
    let empty =
        pin!(Pin::new(&mut total).execute(CallResults(IterCalls(std::iter::empty::<Add>()))));
    assert_eq!(empty.poll(&mut cx), Poll::Ready(()));
    let events = CallEvents(IterCalls([Add::new(1), Add::new(2)].into_iter()));
    assert_eq!(
        pin!(Pin::new(&mut total).execute(events)).poll(&mut cx),
        Poll::Ready(())
    );
    assert_eq!(total, 3);
}

#[test]
fn dropping_the_outer_call_constructs_no_further_call() {
    let produced = std::cell::Cell::new(0);
    let factory = std::iter::from_fn(|| {
        produced.set(produced.get() + 1);
        Some(Add::new(7))
    });
    let mut total = 10usize;
    {
        let mut events = pin!(Pin::new(&mut total).execute(CallEvents(IterCalls(factory))));
        assert_eq!(produced.get(), 0);
        assert_eq!(
            next(events.as_mut()),
            Some(ControlFlow::Continue(ControlFlow::Continue(10)))
        );
        assert_eq!(produced.get(), 1);
    }
    assert_eq!(total, 10);
    assert_eq!(produced.get(), 1);
}

/// A source that waits once before each item, counting its polls.
struct DelayedSource<Op> {
    operations: std::vec::IntoIter<Op>,
    polls: std::rc::Rc<std::cell::Cell<usize>>,
    waiting: bool,
}
impl<Op> Unpin for DelayedSource<Op> {}
impl<Op> Stream for DelayedSource<Op> {
    type Item = Op;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Op>> {
        let this = self.get_mut();
        this.polls.set(this.polls.get() + 1);
        if !this.waiting {
            this.waiting = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.waiting = false;
        Poll::Ready(this.operations.next())
    }
}

/// Wait once, yield the amount, then add it.
#[call(yields = usize)]
async fn delayed_add<A: ReceiverAccess<Family: Counter>>(io: &mut A, amount: usize) -> usize {
    let mut waited = false;
    poll_fn(|cx| {
        if waited {
            Poll::Ready(())
        } else {
            waited = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
    yield amount;
    add_now(io, amount).await
}

#[test]
fn source_and_child_wait_without_prefetching_and_keep_each_final_value() {
    let polls = std::rc::Rc::new(std::cell::Cell::new(0));
    let source = DelayedSource {
        operations: vec![DelayedAdd::new(2), DelayedAdd::new(3)].into_iter(),
        polls: polls.clone(),
        waiting: false,
    };
    let mut total = 0usize;
    let (wakes, waker) = support::WakeCount::new();
    let mut cx = Context::from_waker(&waker);
    let mut events = pin!(Pin::new(&mut total).execute(CallEvents(source)));
    for (amount, sum) in [(2, 2), (3, 5)] {
        // The source waits, then the new child waits.
        assert!(events.as_mut().poll_next(&mut cx).is_pending());
        assert!(events.as_mut().poll_next(&mut cx).is_pending());
        let source_polls = polls.get();
        assert_eq!(
            events.as_mut().poll_next(&mut cx),
            Poll::Ready(Some(ControlFlow::Continue(ControlFlow::Continue(amount))))
        );
        assert_eq!(
            events.as_mut().poll_next(&mut cx),
            Poll::Ready(Some(ControlFlow::Continue(ControlFlow::Break(sum))))
        );
        // The next call is not requested while the current one runs.
        assert_eq!(polls.get(), source_polls);
    }
    assert!(events.as_mut().poll_next(&mut cx).is_pending());
    assert_eq!(
        events.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(ControlFlow::Break(())))
    );
    // Only the waits wake; items polled directly do not.
    assert_eq!(wakes.get(), 5);
}
