#![feature(coroutine_trait)]
#![deny(unsafe_op_in_unsafe_fn)]
//! Operations are parameterized by their execution host. Binding a real host
//! is optional; managed callers can supply a fresh short borrow to every poll.
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

/// An operation bound to a real host borrow. This adapter retains that borrow
/// across Pending; the underlying CallOn operation itself does not require this.
#[must_use = "bound calls do nothing until polled"]
#[pin_project::pin_project]
pub struct BoundCall<'host, Host: ?Sized, Operation> {
    host: Pin<&'host mut Host>,
    #[pin]
    operation: Operation,
    terminal: bool,
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

/// Define named and anonymous operations through the public facade.
pub use sakuya_call_macros::{call, call_closure};

/// Code-generation support, not an author-facing execution API.
#[doc(hidden)]
pub mod __private;
