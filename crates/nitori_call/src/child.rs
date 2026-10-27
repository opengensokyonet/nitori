//! An emitting operation started on its parent's access.
use crate::{
    Emitter, Operation, ReceiverAccess, Start,
    call::{InPlace, ItemCell, State, poll_events, remaining},
};
use futures_core::{FusedFuture, FusedStream, Stream};
use std::{
    fmt,
    future::Future,
    marker::PhantomPinned,
    ops::ControlFlow,
    pin::Pin,
    task::{Context, Poll, ready},
};

type Running<A, Op> = <Op as Start<'static, A>>::Fut;
type Output<A, Op> = <Op as Operation<<A as ReceiverAccess>::Family>>::Output;
type Yield<A, Op> = <Op as Operation<<A as ReceiverAccess>::Family>>::Yield;
type ChildState<A, Op> = State<(Op, A), Running<A, Op>, Output<A, Op>>;

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
    state: InPlace<ChildState<A, Op>>,
    emitted: ItemCell<Yield<A, Op>>,
    finished: bool,
    _pinned: PhantomPinned,
}
// SAFETY: a shared reference only reads whether the child has finished, which
// changes only through `Pin<&mut Self>`.
unsafe impl<A, Op> Sync for Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
}
impl<A, Op> fmt::Debug for Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Child")
            .field("terminated", &self.finished)
            .finish_non_exhaustive()
    }
}
impl<A, Op> Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
    pub fn new(operation: Op, access: A) -> Self {
        Self {
            state: InPlace::new(State::Start((operation, access))),
            emitted: ItemCell::new(),
            finished: false,
            _pinned: PhantomPinned,
        }
    }

    /// The next event of the stream: an item, the final value, then `None`.
    pub fn next(
        mut self: Pin<&mut Self>,
    ) -> impl Future<Output = Option<ControlFlow<Output<A, Op>, Yield<A, Op>>>> {
        std::future::poll_fn(move |cx| self.as_mut().poll_next(cx))
    }

    /// The next event, without the end of the stream: an item, or the final
    /// value. Waiting for it after the final value panics.
    pub fn next_event(
        mut self: Pin<&mut Self>,
    ) -> impl Future<Output = ControlFlow<Output<A, Op>, Yield<A, Op>>> {
        std::future::poll_fn(move |cx| {
            assert!(!self.finished, "`next_event` after the final value");
            self.as_mut().poll_event(false, cx)
        })
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
        let (emitted, state) = (&this.emitted, this.state.get_mut());
        emitted.set_discard(discard);
        // `Op: for<'e> Start` keeps the operation from relying on the 'static
        // brand chosen for storage; the future is dropped before the cell.
        let start = |(operation, access): (Op, A)| operation.start(access, Emitter::new(emitted));
        // SAFETY: the state is a field of the pinned child.
        let poll = unsafe { poll_events(state, emitted, start, cx) };
        this.finished = matches!(poll, Poll::Ready(ControlFlow::Break(_)));
        poll
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
        if self.finished {
            return Poll::Ready(None);
        }
        self.poll_event(false, cx).map(Some)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        remaining(self.finished)
    }
}
impl<A, Op> FusedFuture for Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
    fn is_terminated(&self) -> bool {
        self.finished
    }
}
impl<A, Op> FusedStream for Child<A, Op>
where
    A: ReceiverAccess,
    Op: for<'e> Start<'e, A>,
{
    fn is_terminated(&self) -> bool {
        self.finished
    }
}
