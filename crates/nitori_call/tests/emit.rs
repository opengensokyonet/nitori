//! An emission completes once its item is delivered, and an emission dropped
//! after its first poll still delivers its item.
#![feature(impl_trait_in_assoc_type)]

use futures_util::stream::{FuturesUnordered, StreamExt as _};
use nitori_call::{
    Call, CallPoll, Direct, Emitter, Operation, ReceiverAccess, ReceiverFamily, ReceiverSlot, Start,
};
use std::{
    cell::Cell,
    future::{Future, pending, poll_fn},
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

type Memory<'a> = Direct<&'a [u8]>;

/// Polls the call once with a no-op waker; the call never asks for a view.
fn poll_call<F, Op>(call: Pin<&mut Call<F, Op>>) -> Poll<ControlFlow<usize, u8>>
where
    F: ReceiverFamily,
    Op: Operation<F, Output = usize, Yield = u8> + for<'s> Start<'s, ReceiverSlot<'s, F>>,
{
    match call.poll_with(None, &mut Context::from_waker(Waker::noop())) {
        CallPoll::Ready(event) => Poll::Ready(event),
        CallPoll::Pending => Poll::Pending,
        poll => panic!("unexpected {poll:?}"),
    }
}

/// Emits 1, 2 and 3 from a branch of a `FuturesUnordered` beside two
/// branches that never finish, counting the items whose emission completed
/// in `progress`. Once `stop` is set it drops the branches and returns the
/// count.
struct EmitInUnordered<'a> {
    progress: &'a Cell<usize>,
    stop: &'a Cell<bool>,
}
impl<F: ReceiverFamily> Operation<F> for EmitInUnordered<'_> {
    type Output = usize;
    type Yield = u8;
}
impl<'s, A: ReceiverAccess> Start<'s, A> for EmitInUnordered<'_> {
    type Fut = impl Future<Output = usize>;
    fn start(self, _: A, mut emitter: Emitter<'s, u8>) -> Self::Fut {
        async move {
            let progress = self.progress;
            let emitting = async move {
                for item in 1..=3 {
                    emitter.emit(item).await;
                    progress.set(usize::from(item));
                }
                pending::<()>().await;
            };
            let mut branches = FuturesUnordered::new();
            branches.push(Box::pin(emitting) as Pin<Box<dyn Future<Output = ()>>>);
            branches.push(Box::pin(pending()));
            branches.push(Box::pin(pending()));
            poll_fn(|cx| {
                if self.stop.get() {
                    return Poll::Ready(());
                }
                let _ = branches.poll_next_unpin(cx);
                Poll::Pending
            })
            .await;
            progress.get()
        }
    }
}

#[test]
fn an_emission_in_futures_unordered_completes_after_delivery() {
    let (progress, stop) = (Cell::new(0), Cell::new(false));
    let mut call = pin!(Call::<Memory, _>::new(EmitInUnordered {
        progress: &progress,
        stop: &stop,
    }));
    assert_eq!(
        poll_call(call.as_mut()),
        Poll::Ready(ControlFlow::Continue(1))
    );
    assert_eq!(
        progress.get(),
        0,
        "ran on before the first item was delivered"
    );
    assert_eq!(
        poll_call(call.as_mut()),
        Poll::Ready(ControlFlow::Continue(2))
    );
    assert_eq!(
        progress.get(),
        1,
        "ran on before the second item was delivered"
    );
    // The branches are dropped with no item left undelivered.
    stop.set(true);
    assert_eq!(poll_call(call.as_mut()), Poll::Ready(ControlFlow::Break(1)));
}

/// Polls `emit(1)` once and drops it, so its item waits in the cell; polls
/// `emit(2)` once, which finds the cell taken; then drops that one on the
/// next poll.
struct DropWaitingEmission;
impl<F: ReceiverFamily> Operation<F> for DropWaitingEmission {
    type Output = usize;
    type Yield = u8;
}
impl<'s, A: ReceiverAccess> Start<'s, A> for DropWaitingEmission {
    type Fut = impl Future<Output = usize>;
    fn start(self, _: A, mut emitter: Emitter<'s, u8>) -> Self::Fut {
        async fn poll_once(future: Pin<&mut impl Future>) {
            let mut future = Some(future);
            poll_fn(|cx| {
                let _ = future.take().unwrap().poll(cx);
                Poll::Ready(())
            })
            .await;
        }
        /// Ends the operation's poll once, without a wakeup: the caller
        /// polls again after an item.
        async fn suspend() {
            let mut suspended = false;
            poll_fn(|_| match suspended {
                true => Poll::Ready(()),
                false => {
                    suspended = true;
                    Poll::Pending
                }
            })
            .await;
        }
        async move {
            let mut first = Box::pin(emitter.emit(1));
            poll_once(first.as_mut()).await;
            drop(first);
            let mut second = Box::pin(emitter.emit(2));
            poll_once(second.as_mut()).await;
            suspend().await;
            drop(second);
            suspend().await;
            3
        }
    }
}

#[test]
fn an_emission_dropped_while_waiting_still_delivers() {
    let mut call = pin!(Call::<Memory, _>::new(DropWaitingEmission));
    assert_eq!(
        poll_call(call.as_mut()),
        Poll::Ready(ControlFlow::Continue(1))
    );
    assert_eq!(
        poll_call(call.as_mut()),
        Poll::Ready(ControlFlow::Continue(2))
    );
    assert_eq!(poll_call(call.as_mut()), Poll::Ready(ControlFlow::Break(3)));
}
