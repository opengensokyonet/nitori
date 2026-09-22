//! Sequential composition of parameterized calls from a stream or iterator.
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

use nitori_call::TargetExt as _;
use nitori_call::{CallOn, Child, Stream, call};
use std::{
    future::poll_fn,
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll},
};

/// Adapt an iterator, including `from_fn` factories, without prefetching.
pub struct IterCalls<Source>(pub Source);
// The iterator is never structurally pinned or exposed through a pinned reference.
impl<Source> Unpin for IterCalls<Source> {}
impl<Source: Iterator> Stream for IterCalls<Source> {
    type Item = Source::Item;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().0.next())
    }
}

/// Yield each call's final result, discarding its intermediate yields.
/// A return value such as `Err` is a value here, not a stop policy.
#[call(sync, yields = Operation::Return)]
pub async fn call_results<Receiver, Source, Operation>(
    _io: nitori_call::Target<Receiver>,
    source: Source,
) where
    Receiver: nitori_call::ReceiverFamily,
    Source: Stream<Item = Operation>,
    Operation: CallOn<Receiver>,
{
    let mut source = pin!(source);
    while let Some(operation) = poll_fn(|cx| source.as_mut().poll_next(cx)).await {
        let child = Child::<Receiver, Operation>::new(operation);
        yield child.await;
    }
}

/// Yield each child's events, including its completion, in execution order.
/// The outer operation completes only when the source returns `None`.
#[call(sync, yields = CoroutineState<Operation::Yield, Operation::Return>)]
pub async fn call_events<Receiver, Source, Operation>(
    _io: nitori_call::Target<Receiver>,
    source: Source,
) where
    Receiver: nitori_call::ReceiverFamily,
    Source: Stream<Item = Operation>,
    Operation: CallOn<Receiver>,
{
    let mut source = pin!(source);
    while let Some(operation) = poll_fn(|cx| source.as_mut().poll_next(cx)).await {
        let mut child = pin!(Child::<Receiver, Operation>::new(operation));
        while let Some(event) = child.as_mut().next().await {
            yield event;
        }
    }
}

#[call(yields = usize)]
async fn add(io: nitori_call::Target<nitori_call::Direct<usize>>, amount: usize) -> usize {
    let previous = io
        .with(|__access| {
            let host = __access.into_pin().get_mut().0.as_mut();
            *host
        })
        .await;
    yield previous;
    io.with(|__access| {
        let mut host = __access.into_pin().get_mut().0.as_mut();
        {
            *host += amount;
            *host
        }
    })
    .await
}

fn main() {
    let mut host = 0usize;
    let mut amounts = [2, 3].into_iter();
    let factory = std::iter::from_fn(move || amounts.next().map(Add::new));
    {
        let mut results = pin!(host.sync_call_results_unpin(IterCalls(factory)));
        assert_eq!(results.as_mut().next(), Some(CoroutineState::Yielded(2)));
        assert_eq!(results.as_mut().next(), Some(CoroutineState::Yielded(5)));
        assert_eq!(results.as_mut().next(), Some(CoroutineState::Complete(())));
        assert_eq!(results.as_mut().next(), None);
    }
    assert_eq!(host, 5);
    let mut events = pin!(host.sync_call_events_unpin(IterCalls([Add::new(4)].into_iter())));
    assert_eq!(
        events.as_mut().next(),
        Some(CoroutineState::Yielded(CoroutineState::Yielded(5)))
    );
    assert_eq!(
        events.as_mut().next(),
        Some(CoroutineState::Yielded(CoroutineState::Complete(9)))
    );
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Complete(())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use nitori_call::PollCallExt;
    use std::{
        cell::Cell,
        marker::PhantomPinned,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Wake, Waker},
    };

    struct WakeCount(AtomicUsize);
    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    pin_project_lite::pin_project! {
        struct DelayedSource<Operation> {
            operations: std::vec::IntoIter<Operation>,
            polls: Rc<Cell<usize>>,
            waiting: bool,
            #[pin]
            pinned: PhantomPinned,
        }
    }
    impl<Operation> Stream for DelayedSource<Operation> {
        type Item = Operation;
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Operation>> {
            let this = self.project();
            this.polls.set(this.polls.get() + 1);
            if !*this.waiting {
                *this.waiting = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            *this.waiting = false;
            Poll::Ready(this.operations.next())
        }
    }

    #[call(yields = usize)]
    async fn delayed_add(
        io: nitori_call::Target<nitori_call::Direct<usize>>,
        amount: usize,
    ) -> usize {
        let mut waiting = false;
        poll_fn(|cx| {
            if waiting {
                Poll::Ready(())
            } else {
                waiting = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
        yield amount;
        io.with(|__access| {
            let mut host = __access.into_pin().get_mut().0.as_mut();
            {
                *host += amount;
                *host
            }
        })
        .await
    }

    #[test]
    fn source_and_child_wait_without_prefetch_and_completion_is_preserved() {
        let polls = Rc::new(Cell::new(0));
        let source = DelayedSource {
            operations: vec![DelayedAdd::new(2), DelayedAdd::new(3)].into_iter(),
            polls: polls.clone(),
            waiting: false,
            pinned: PhantomPinned,
        };
        let mut operation = pin!(call_events::<nitori_call::Direct<usize>, _, _>(source));
        let mut host = 0usize;
        let wakes = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);
        for (amount, total, previous) in [(2, 2, 0), (3, 5, 2)] {
            assert_eq!(
                operation
                    .as_mut()
                    .poll_receiver(Pin::new(&mut host), &mut cx),
                Poll::Pending
            );
            assert_eq!(
                operation
                    .as_mut()
                    .poll_receiver(Pin::new(&mut host), &mut cx),
                Poll::Pending
            );
            let source_polls = polls.get();
            assert_eq!(
                operation
                    .as_mut()
                    .poll_receiver(Pin::new(&mut host), &mut cx),
                Poll::Ready(CoroutineState::Yielded(CoroutineState::Yielded(amount)))
            );
            assert_eq!(host, previous);
            assert_eq!(polls.get(), source_polls);
            assert_eq!(
                operation
                    .as_mut()
                    .poll_receiver(Pin::new(&mut host), &mut cx),
                Poll::Ready(CoroutineState::Yielded(CoroutineState::Complete(total)))
            );
            assert_eq!(host, total);
            assert_eq!(polls.get(), source_polls);
        }
        assert_eq!(
            operation
                .as_mut()
                .poll_receiver(Pin::new(&mut host), &mut cx),
            Poll::Pending
        );
        assert_eq!(
            operation
                .as_mut()
                .poll_receiver(Pin::new(&mut host), &mut cx),
            Poll::Ready(CoroutineState::Complete(()))
        );
        assert_eq!(wakes.0.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn cancellation_does_not_construct_next_call_or_finish_current_call() {
        let produced = Rc::new(Cell::new(0));
        let counter = produced.clone();
        let factory = std::iter::from_fn(move || {
            counter.set(counter.get() + 1);
            Some(Add::new(7))
        });
        let mut host = 10usize;
        {
            let mut events = pin!(host.sync_call_events_unpin(IterCalls(factory)));
            assert_eq!(produced.get(), 0);
            assert_eq!(
                events.as_mut().next(),
                Some(CoroutineState::Yielded(CoroutineState::Yielded(10)))
            );
            assert_eq!(produced.get(), 1);
        }
        assert_eq!(host, 10);
        assert_eq!(produced.get(), 1);
    }

    #[test]
    fn results_discard_intermediate_values_and_finish_on_source_end() {
        super::main();
        let mut host = 0usize;
        let mut empty = pin!(host.sync_call_results_unpin(IterCalls(std::iter::empty::<Add>())));
        assert_eq!(empty.as_mut().next(), Some(CoroutineState::Complete(())));
        assert_eq!(empty.as_mut().next(), None);
    }
}
