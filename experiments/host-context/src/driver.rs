//! Execute a resource driver as a call with an empty receiver.
use crate::{CallEvent, CallOn, HasFamily, HostContext, HostFamily};
use futures_core::Stream;
use std::{
    future::Future,
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll},
};

/// Empty family used only at the outer execution boundary.
pub struct NoReceiver;
impl HostFamily for NoReceiver {
    type HostView<'view> = ();
}
impl HasFamily for () {
    type Family = NoReceiver;
}
struct EmptyScope(());
impl<'view> HostContext<'view> for EmptyScope {
    type Family = NoReceiver;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }
    fn ready_view(self: Pin<&mut Self>) -> Pin<&mut ()> {
        Pin::new(&mut self.get_mut().0)
    }
}

/// A driver chooses its persistent execution state, including borrowed state.
///
/// Unfinished acquisition belongs to the whole execution, including rounds that
/// do not request a receiver. A successfully delivered view belongs only to its
/// current round and must be released before returning Pending or an event.
/// Completion, cancellation and unwinding end any unfinished acquisition.
///
/// Future polling discards yields within one round. Stream polling returns one
/// event, including completion exactly once, followed by None. Both interfaces
/// share progress; changing polling mode must not recreate the execution.
pub trait ResourceDriver {
    type Family: HostFamily;
    type Execution<'driver, O>: Future<Output = O::Return>
        + Stream<Item = CallEvent<Self::Family, O>>
    where
        Self: 'driver,
        O: CallOn<Self::Family> + 'driver;

    fn execute<'driver, O>(
        self: Pin<&'driver mut Self>,
        operation: O,
    ) -> Self::Execution<'driver, O>
    where
        O: CallOn<Self::Family> + 'driver;
}

pin_project_lite::pin_project! {
    /// Minimal Future/Stream binding: all resource state lives in the outer call.
    #[must_use = "executions do nothing until polled"]
    pub struct Execution<O> {
        #[pin]
        operation: O,
        terminal: bool,
    }
}
impl<O: CallOn<NoReceiver>> Execution<O> {
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            terminal: false,
        }
    }
}
impl<O: CallOn<NoReceiver>> Stream for Execution<O> {
    type Item = CallEvent<NoReceiver, O>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        if *this.terminal {
            return Poll::Ready(None);
        }
        *this.terminal = true;
        let mut scope = pin!(EmptyScope(()));
        let result = this.operation.poll_call(scope.as_mut(), cx);
        if !matches!(&result, Poll::Ready(CoroutineState::Complete(_))) {
            *this.terminal = false;
        }
        result.map(Some)
    }
}
impl<O: CallOn<NoReceiver>> Future for Execution<O> {
    type Output = O::Return;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        assert!(
            !*this.terminal,
            "execution polled after completion or panic"
        );
        *this.terminal = true;
        let mut scope = pin!(EmptyScope(()));
        let result = this.operation.poll_return(scope.as_mut(), cx);
        if result.is_pending() {
            *this.terminal = false;
        }
        result
    }
}
