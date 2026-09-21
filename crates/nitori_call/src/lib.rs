#![feature(coroutine_trait)]
#![deny(unsafe_op_in_unsafe_fn)]
#![doc = include_str!("../README.md")]
use std::{
    future::{Future, poll_fn},
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

/// Standard Stream contract exposed for consumers of bound operations.
pub use futures_core::Stream;

pub trait CallOn<Host: ?Sized> {
    type Yield;
    type Return;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut Host>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>;
}

pin_project_lite::pin_project! {
    /// An operation bound to a real host borrow. This adapter retains that borrow
    /// across Pending; the underlying CallOn operation itself does not require this.
    #[must_use = "bound calls do nothing until polled"]
    pub struct BoundCall<'host, Host: ?Sized, Operation> {
        host: Pin<&'host mut Host>,
        #[pin]
        operation: Operation,
        terminal: bool,
    }
}
impl<'host, Host: ?Sized, Operation: CallOn<Host>> BoundCall<'host, Host, Operation> {
    pub fn new(host: Pin<&'host mut Host>, operation: Operation) -> Self {
        Self {
            host,
            operation,
            terminal: false,
        }
    }

    /// Poll one event without requiring the operation to implement Unpin.
    /// Complete is delivered once, followed by None on subsequent calls.
    pub fn next(
        mut self: Pin<&mut Self>,
    ) -> impl Future<Output = Option<CoroutineState<Operation::Yield, Operation::Return>>> {
        poll_fn(move |cx| self.as_mut().poll_next(cx))
    }
}
impl<Host: ?Sized, Operation: CallOn<Host>> Stream for BoundCall<'_, Host, Operation> {
    type Item = CoroutineState<Operation::Yield, Operation::Return>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        if *this.terminal {
            return Poll::Ready(None);
        }
        // Poison on unwinding; no attempt is made to resume a panicked operation.
        *this.terminal = true;
        match this.operation.poll_call(this.host.as_mut(), cx) {
            Poll::Pending => {
                *this.terminal = false;
                Poll::Pending
            }
            Poll::Ready(CoroutineState::Yielded(item)) => {
                *this.terminal = false;
                Poll::Ready(Some(CoroutineState::Yielded(item)))
            }
            Poll::Ready(CoroutineState::Complete(value)) => {
                Poll::Ready(Some(CoroutineState::Complete(value)))
            }
        }
    }
}
/// Awaiting a bound call discards all intermediate yields and returns completion.
impl<Host: ?Sized, Operation: CallOn<Host>> Future for BoundCall<'_, Host, Operation> {
    type Output = Operation::Return;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match self.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(CoroutineState::Yielded(_))) => (),
                Poll::Ready(Some(CoroutineState::Complete(value))) => return Poll::Ready(value),
                Poll::Ready(None) => panic!("call polled after completion or panic"),
            }
        }
    }
}

/// Advance one event using a no-op waker.
///
/// All dependencies of the operation must complete immediately. Panics on
/// `Pending`; this is a contract violation, not an incomplete-input result.
/// After a panic, the caller must not resume the operation. The caller also
/// owns tracking completion and must not poll a completed operation again.
pub fn step_sync<Host: ?Sized, Operation: CallOn<Host>>(
    operation: Pin<&mut Operation>,
    host: Pin<&mut Host>,
) -> CoroutineState<Operation::Yield, Operation::Return> {
    let mut cx = Context::from_waker(std::task::Waker::noop());
    match operation.poll_call(host, &mut cx) {
        Poll::Ready(event) => event,
        Poll::Pending => panic!("synchronous call returned Pending"),
    }
}

/// Execute an operation with no intermediate yields and return its result.
///
/// The operation is pinned on the stack. See [`step_sync`] for the synchronous
/// execution contract. Previously consumed or written data is not rolled back.
pub fn run_sync<Host: ?Sized, Operation>(
    host: Pin<&mut Host>,
    operation: Operation,
) -> Operation::Return
where
    Operation: CallOn<Host, Yield = std::convert::Infallible>,
{
    match step_sync(std::pin::pin!(operation), host) {
        CoroutineState::Complete(value) => value,
        CoroutineState::Yielded(never) => match never {},
    }
}

pin_project_lite::pin_project! {
    /// A lazily executed synchronous call that preserves intermediate yields.
    ///
    /// Pin this object, then iterate over its pinned mutable reference. Each
    /// step delivers one yield or the unique completion event. Construction
    /// does no work; dropping the object cancels remaining work without rollback.
    /// The host remains borrowed for the lifetime of the binding.
    #[must_use = "synchronous event calls do nothing until advanced"]
    pub struct SyncBoundCall<'host, Host: ?Sized, Operation> {
        #[pin]
        bound: BoundCall<'host, Host, Operation>,
        terminal: bool,
    }
}
impl<'host, Host: ?Sized, Operation: CallOn<Host>> SyncBoundCall<'host, Host, Operation> {
    pub fn new(host: Pin<&'host mut Host>, operation: Operation) -> Self {
        Self {
            bound: BoundCall::new(host, operation),
            terminal: false,
        }
    }

    /// Synchronously obtain one event; completion is followed by `None`.
    ///
    /// Panics on `Pending` and terminates this adapter on any unwinding panic.
    /// No event is discarded and no external readiness is awaited.
    pub fn next(
        self: Pin<&mut Self>,
    ) -> Option<CoroutineState<Operation::Yield, Operation::Return>> {
        let this = self.project();
        if *this.terminal {
            return None;
        }
        *this.terminal = true;
        let mut cx = Context::from_waker(std::task::Waker::noop());
        match this.bound.poll_next(&mut cx) {
            Poll::Pending => panic!("synchronous call returned Pending"),
            Poll::Ready(event) => {
                if matches!(&event, Some(CoroutineState::Yielded(_))) {
                    *this.terminal = false;
                }
                event
            }
        }
    }
}
impl<Host: ?Sized, Operation: CallOn<Host>> Iterator
    for Pin<&mut SyncBoundCall<'_, Host, Operation>>
{
    type Item = CoroutineState<Operation::Yield, Operation::Return>;
    fn next(&mut self) -> Option<Self::Item> {
        SyncBoundCall::next(self.as_mut())
    }
}
impl<Host: ?Sized, Operation: CallOn<Host>> std::iter::FusedIterator
    for Pin<&mut SyncBoundCall<'_, Host, Operation>>
{
}

/// Define named and anonymous operations through the public facade.
pub use nitori_call_macros::{call, call_closure};

/// Code-generation support, not an author-facing execution API.
#[doc(hidden)]
pub mod __private;

mod typed;
/// Host receivers, owned child operations and host-aware await adapters.
pub use typed::{AwaitOn, Child, IntoAwaitOn, Next, Receiver};
