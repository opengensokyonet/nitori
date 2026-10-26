//! An emitting operation started on its parent's access.
use crate::{
    Emitter, Operation, ReceiverAccess, Start,
    call::{ItemCell, State, poll_events},
};
use futures_core::Stream;
use std::{
    future::Future,
    marker::PhantomPinned,
    ops::ControlFlow,
    pin::Pin,
    task::{Context, Poll, ready},
};

type Running<A, Op> = <Op as Start<'static, A>>::Fut;
type Output<A, Op> = <Op as Operation<<A as ReceiverAccess>::Family>>::Output;
type Yield<A, Op> = <Op as Operation<<A as ReceiverAccess>::Family>>::Yield;

/// An emitting operation running on an access borrowed from its parent, with
/// a cell of its own for its items.
///
/// As a future it drops the items where they are emitted and completes with
/// the final value; as a stream it delivers `Continue` for each item, then
/// `Break` with the final value, then ends.
pub struct Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
    // Dropped before the cell its future points into.
    state: State<(Op, A), Running<A, Op>, Output<A, Op>>,
    emitted: ItemCell<Yield<A, Op>>,
    _pinned: PhantomPinned,
}
impl<A, Op> Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
    pub fn new(operation: Op, access: A) -> Self {
        Self {
            state: State::Start((operation, access)),
            emitted: ItemCell::new(),
            _pinned: PhantomPinned,
        }
    }

    fn poll_event(
        self: Pin<&mut Self>,
        discard: bool,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<Output<A, Op>, Yield<A, Op>>> {
        // SAFETY: nothing is moved out of the pinned child; the future keeps a
        // pointer into the cell, which is only accessed through shared
        // references.
        let this = unsafe { self.get_unchecked_mut() };
        let (emitted, state) = (&this.emitted, &mut this.state);
        emitted.set_discard(discard);
        // `Op: for<'e> Start` keeps the operation from relying on the 'static
        // brand chosen for storage; the future is dropped before the cell.
        let start = |(operation, access): (Op, A)| operation.start(access, Emitter::new(emitted));
        // SAFETY: the state is a field of the pinned child.
        unsafe { poll_events(state, emitted, start, cx) }
    }
}
impl<A, Op> Future for Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
    type Output = Output<A, Op>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match ready!(self.poll_event(true, cx)) {
            ControlFlow::Break(result) => Poll::Ready(result),
            ControlFlow::Continue(_) => unreachable!("discarded items are not stored"),
        }
    }
}
impl<A, Op> Stream for Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
    type Item = ControlFlow<Output<A, Op>, Yield<A, Op>>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // SAFETY: as in `poll_event`. A shared reference to the whole child
        // would invalidate the running future's references into itself.
        let this = unsafe { self.get_unchecked_mut() };
        if let State::Finished = this.state {
            return Poll::Ready(None);
        }
        // SAFETY: the child was pinned and is not moved.
        unsafe { Pin::new_unchecked(this) }
            .poll_event(false, cx)
            .map(Some)
    }
}
