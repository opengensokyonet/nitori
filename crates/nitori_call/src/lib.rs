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

mod receiver;
pub use receiver::{
    BorrowedReceiver, Direct, DirectView, HasReceiverFamily, Receiver, ReceiverFamily,
};
mod scope;
pub use scope::{BorrowedScope, ReceiverScope, ViewScope};
mod driver;
pub use driver::{Drive, DriveMode, Execution, ExecutionControl, ResourceDriver, drive};
mod loan;
pub use loan::{ViewLoan, ViewOperation};

pub trait CallOn<F: ReceiverFamily> {
    type Yield;
    type Return;
    fn poll_call<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn ReceiverScope<'visit, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        F: 'visit;
    fn poll_return<'view>(
        mut self: Pin<&mut Self>,
        mut scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Return>
    where
        F: 'view,
    {
        loop {
            match self.as_mut().poll_call(scope.as_mut(), cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(CoroutineState::Yielded(value)) => drop(value),
                Poll::Ready(CoroutineState::Complete(value)) => return Poll::Ready(value),
            }
        }
    }
}

pin_project_lite::pin_project! {
    /// An operation bound to a real host borrow. This adapter retains that borrow
    /// across Pending; the underlying CallOn operation itself does not require this.
    #[must_use = "bound calls do nothing until polled"]
    pub struct BoundCall<'host, H: ?Sized, Operation> {
        host: Pin<&'host mut H>,
        #[pin]
        operation: Operation,
        terminal: bool,
    }
}
impl<'host, H: Receiver + ?Sized, Operation: CallOn<H::Family>> BoundCall<'host, H, Operation> {
    pub fn new(host: Pin<&'host mut H>, operation: Operation) -> Self {
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
impl<H: Receiver + ?Sized, Operation: CallOn<H::Family>> Stream for BoundCall<'_, H, Operation> {
    type Item = CoroutineState<Operation::Yield, Operation::Return>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        if *this.terminal {
            return Poll::Ready(None);
        }
        // Poison on unwinding; no attempt is made to resume a panicked operation.
        *this.terminal = true;
        let result = {
            let mut scope = std::pin::pin!(BorrowedScope::new(this.host.as_mut()));
            this.operation.poll_call(scope.as_mut(), cx)
        };
        match result {
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
impl<H: Receiver + ?Sized, Operation: CallOn<H::Family>> Future for BoundCall<'_, H, Operation> {
    type Output = Operation::Return;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        assert!(!*this.terminal, "call polled after completion or panic");
        *this.terminal = true;
        let result = {
            let mut scope = std::pin::pin!(BorrowedScope::new(this.host.as_mut()));
            this.operation.poll_return(scope.as_mut(), cx)
        };
        if result.is_pending() {
            *this.terminal = false;
        }
        result
    }
}

/// Advance one event using a no-op waker.
///
/// All dependencies of the operation must complete immediately. Panics on
/// `Pending`; this is a contract violation, not an incomplete-input result.
/// After a panic, the caller must not resume the operation. The caller also
/// owns tracking completion and must not poll a completed operation again.
pub fn step_sync<H: Receiver + ?Sized, Operation: CallOn<H::Family>>(
    operation: Pin<&mut Operation>,
    host: Pin<&mut H>,
) -> CoroutineState<Operation::Yield, Operation::Return> {
    let mut cx = Context::from_waker(std::task::Waker::noop());
    let mut view = std::pin::pin!(BorrowedScope::new(host));
    match operation.poll_call(view.as_mut(), &mut cx) {
        Poll::Ready(event) => event,
        Poll::Pending => panic!("synchronous call returned Pending"),
    }
}

/// Execute an operation with no intermediate yields and return its result.
///
/// The operation is pinned on the stack. See [`step_sync`] for the synchronous
/// execution contract. Previously consumed or written data is not rolled back.
pub fn run_sync<H: Receiver + ?Sized, Operation>(
    host: Pin<&mut H>,
    operation: Operation,
) -> Operation::Return
where
    Operation: CallOn<H::Family, Yield = std::convert::Infallible>,
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
    pub struct SyncBoundCall<'host, H: ?Sized, Operation> {
        #[pin]
        bound: BoundCall<'host, H, Operation>,
        terminal: bool,
    }
}
impl<'host, H: Receiver + ?Sized, Operation: CallOn<H::Family>> SyncBoundCall<'host, H, Operation> {
    pub fn new(host: Pin<&'host mut H>, operation: Operation) -> Self {
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
impl<H: Receiver + ?Sized, Operation: CallOn<H::Family>> Iterator
    for Pin<&mut SyncBoundCall<'_, H, Operation>>
{
    type Item = CoroutineState<Operation::Yield, Operation::Return>;
    fn next(&mut self) -> Option<Self::Item> {
        SyncBoundCall::next(self.as_mut())
    }
}
impl<H: Receiver + ?Sized, Operation: CallOn<H::Family>> std::iter::FusedIterator
    for Pin<&mut SyncBoundCall<'_, H, Operation>>
{
}

/// Define named and anonymous operations through the public facade.
pub use nitori_call_macros::{call, call_closure};

/// Code-generation support, not an author-facing execution API.
#[doc(hidden)]
pub mod __private;

mod typed;
/// Owned child operations and receiver-aware await adapters.
pub use typed::{AwaitOn, Child, IntoAwaitOn, Next};

mod compose;
pub use compose::{
    Access, AdaptedCall, Arguments, CallTarget, Compose, Composed, Target, TargetExt, With,
    WithChild,
};

/// Poll using a resource; construct its view lazily and discard it before returning.
pub trait PollCallExt: Sized {
    fn poll_receiver<H: Receiver + ?Sized>(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Poll<CallEvent<H::Family, Self>>
    where
        Self: CallOn<H::Family>,
    {
        <Self as CallOn<H::Family>>::poll_call(self, std::pin::pin!(BorrowedScope::new(host)), cx)
    }
}
impl<O> PollCallExt for O {}

/// One event from an operation executing on a family.
pub type CallEvent<F, O> = CoroutineState<<O as CallOn<F>>::Yield, <O as CallOn<F>>::Return>;

mod acquisition;
pub use acquisition::{AcquiredScope, AcquisitionState, MapAcquisition, ViewAcquisition};
