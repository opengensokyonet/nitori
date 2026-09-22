//! Resource execution control is an ordinary receiver, not a resume flag.
use crate::{
    AcquiredScope, AwaitOn, BorrowedScope, CallEvent, CallOn, IntoAwaitOn, Receiver,
    ReceiverFamily, ReceiverScope, ViewAcquisition,
};
use futures_core::Stream;
use std::{
    future::{Future, poll_fn},
    marker::PhantomData,
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveMode {
    Event,
    Return,
}
/// Per-poll control supplied only to the resource-driving call.
#[derive(Clone, Copy)]
pub struct ExecutionControl {
    pub mode: DriveMode,
}
impl ReceiverFamily for ExecutionControl {
    type ReceiverView<'v> = Self;
}
impl Receiver for ExecutionControl {
    type Family = Self;
    fn view<'v>(self: Pin<&'v mut Self>) -> Self
    where
        Self::Family: 'v,
    {
        *self
    }
}
/// An implementation chooses its persistent execution state. Pending acquisition
/// survives all rounds until delivery or termination. Delivered views do not.
pub trait ResourceDriver {
    type Family: ReceiverFamily;
    type Execution<'a, O>: Future<Output = O::Return> + Stream<Item = CallEvent<Self::Family, O>>
    where
        Self: 'a,
        O: CallOn<Self::Family> + 'a;
    fn execute<'a, O>(self: Pin<&'a mut Self>, operation: O) -> Self::Execution<'a, O>
    where
        O: CallOn<Self::Family> + 'a;
}
impl<R: Receiver + ?Sized> ResourceDriver for R {
    type Family = R::Family;
    type Execution<'a, O>
        = crate::BoundCall<'a, R, O>
    where
        Self: 'a,
        O: CallOn<Self::Family> + 'a;
    fn execute<'a, O>(self: Pin<&'a mut Self>, operation: O) -> Self::Execution<'a, O>
    where
        O: CallOn<Self::Family> + 'a,
    {
        crate::BoundCall::new(self, operation)
    }
}
pin_project_lite::pin_project! {
 /// Bind a transparent resource-driving call to per-poll execution control.
 /// Event transformations belong in the driven business call: Return mode may
 /// consume that call's yields before the driving call sees them.
 #[must_use="executions do nothing until polled"]
 pub struct Execution<O> { #[pin] operation:O, terminal:bool }
}
impl<O: CallOn<ExecutionControl>> Execution<O> {
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            terminal: false,
        }
    }
    /// Request one event; dropping this temporary future does not cancel execution.
    pub fn next(
        mut self: Pin<&mut Self>,
    ) -> impl Future<Output = Option<CallEvent<ExecutionControl, O>>> {
        poll_fn(move |cx| self.as_mut().poll_next(cx))
    }
}
impl<O: CallOn<ExecutionControl>> Stream for Execution<O> {
    type Item = CallEvent<ExecutionControl, O>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        if *this.terminal {
            return Poll::Ready(None);
        }
        *this.terminal = true;
        let mut control = ExecutionControl {
            mode: DriveMode::Event,
        };
        let result = {
            let mut scope = pin!(BorrowedScope::new(Pin::new(&mut control)));
            this.operation.poll_call(scope.as_mut(), cx)
        };
        if !matches!(&result, Poll::Ready(CoroutineState::Complete(_))) {
            *this.terminal = false;
        }
        result.map(Some)
    }
}
impl<O: CallOn<ExecutionControl>> Future for Execution<O> {
    type Output = O::Return;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        assert!(
            !*this.terminal,
            "execution polled after completion or panic"
        );
        *this.terminal = true;
        let mut control = ExecutionControl {
            mode: DriveMode::Return,
        };
        let result = {
            let mut scope = pin!(BorrowedScope::new(Pin::new(&mut control)));
            this.operation.poll_return(scope.as_mut(), cx)
        };
        if result.is_pending() {
            *this.terminal = false;
        }
        result
    }
}
/// Await the next result under the current execution-control mode. Each poll
/// owns a fresh scope; Pending is propagated normally without losing acquisition.
pub struct Drive<'a, 'view, F: ReceiverFamily, S, O> {
    acquisition: Pin<&'a mut S>,
    operation: Pin<&'a mut O>,
    terminal: bool,
    marker: PhantomData<fn() -> &'view F>,
}
impl<F: ReceiverFamily, S, O> Unpin for Drive<'_, '_, F, S, O> {}
pub fn drive<'a, 'v, F: ReceiverFamily + 'v, S: ViewAcquisition<'v, Family = F>, O: CallOn<F>>(
    acquisition: Pin<&'a mut S>,
    operation: Pin<&'a mut O>,
) -> Drive<'a, 'v, F, S, O> {
    Drive {
        acquisition,
        operation,
        terminal: false,
        marker: PhantomData,
    }
}
impl<'v, F: ReceiverFamily + 'v, S: ViewAcquisition<'v, Family = F>, O: CallOn<F>>
    IntoAwaitOn<ExecutionControl> for Drive<'_, 'v, F, S, O>
{
    type Awaitable = Self;
    fn into_await_on(self) -> Self {
        self
    }
}
impl<'v, F: ReceiverFamily + 'v, S: ViewAcquisition<'v, Family = F>, O: CallOn<F>>
    AwaitOn<ExecutionControl> for Drive<'_, 'v, F, S, O>
{
    type Output = CallEvent<F, O>;
    fn poll_on<'control>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'control, Family = ExecutionControl>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        ExecutionControl: 'control,
    {
        let this = self.get_mut();
        assert!(!this.terminal, "drive awaited after completion or panic");
        this.terminal = true;
        let mode = match scope.poll_view(cx) {
            Poll::Ready(view) => view.mode,
            Poll::Pending => {
                this.terminal = false;
                return Poll::Pending;
            }
        };
        let result = {
            let mut scope = pin!(AcquiredScope::new(this.acquisition.as_mut()));
            match mode {
                DriveMode::Event => this.operation.as_mut().poll_call(scope.as_mut(), cx),
                DriveMode::Return => this
                    .operation
                    .as_mut()
                    .poll_return(scope.as_mut(), cx)
                    .map(CoroutineState::Complete),
            }
        };
        if result.is_pending() {
            this.terminal = false;
        }
        result
    }
}
