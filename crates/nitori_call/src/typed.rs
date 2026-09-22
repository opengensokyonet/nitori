//! Receiver-free operation values and type-directed awaiting.
use crate::{CallOn, ReceiverFamily};
use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

/// Poll an awaitable using a receiver scope and task context.
/// Successive polls must provide the logical resources required by the operation.
pub trait AwaitOn<H: ReceiverFamily> {
    type Output;
    fn poll_on<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn crate::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        H: 'visit;
}
impl<H: ReceiverFamily, F: Future> AwaitOn<H> for F {
    type Output = F::Output;
    fn poll_on<'visit>(
        self: Pin<&mut Self>,
        _: Pin<&mut dyn crate::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        H: 'visit,
    {
        self.poll(cx)
    }
}
/// Convert standard futures or owned host-aware values for a call await.
pub trait IntoAwaitOn<H: ReceiverFamily> {
    type Awaitable: AwaitOn<H>;
    fn into_await_on(self) -> Self::Awaitable;
}
impl<H: ReceiverFamily, F: IntoFuture> IntoAwaitOn<H> for F {
    type Awaitable = F::IntoFuture;
    fn into_await_on(self) -> Self::Awaitable {
        self.into_future()
    }
}
pin_project_lite::pin_project! {
 /// An owned operation with no Receiver borrow; pin before requesting events.
 pub struct Child<H: ReceiverFamily,O>{
  #[pin] operation:O,
  terminal:bool,
  marker:PhantomData<fn(*mut H)->*mut H>,
 }
}
impl<H: ReceiverFamily, O: CallOn<H>> Child<H, O> {
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            terminal: false,
            marker: PhantomData,
        }
    }
    pub fn next(self: Pin<&mut Self>) -> Next<'_, H, O> {
        Next {
            child: self,
            terminal: false,
        }
    }
    fn poll_event<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn crate::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<CoroutineState<O::Yield, O::Return>>>
    where
        H: 'visit,
    {
        let this = self.project();
        if *this.terminal {
            return Poll::Ready(None);
        }
        *this.terminal = true;
        match this.operation.poll_call(host, cx) {
            Poll::Pending => {
                *this.terminal = false;
                Poll::Pending
            }
            Poll::Ready(event) => {
                if matches!(&event, CoroutineState::Yielded(_)) {
                    *this.terminal = false;
                }
                Poll::Ready(Some(event))
            }
        }
    }
}
impl<H: ReceiverFamily, O: CallOn<H>> IntoAwaitOn<H> for Child<H, O> {
    type Awaitable = Self;
    fn into_await_on(self) -> Self {
        self
    }
}
impl<H: ReceiverFamily, O: CallOn<H>> AwaitOn<H> for Child<H, O> {
    type Output = O::Return;
    fn poll_on<'visit>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn crate::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        H: 'visit,
    {
        let this = self.project();
        assert!(!*this.terminal, "child awaited after completion or panic");
        *this.terminal = true;
        let result = this.operation.poll_return(scope, cx);
        if result.is_pending() {
            *this.terminal = false;
        }
        result
    }
}
/// A single event request borrowing a pinned child; not a standard Future.
pub struct Next<'a, H: ReceiverFamily, O> {
    child: Pin<&'a mut Child<H, O>>,
    terminal: bool,
}
impl<H: ReceiverFamily, O> Unpin for Next<'_, H, O> {}
impl<H: ReceiverFamily, O: CallOn<H>> IntoAwaitOn<H> for Next<'_, H, O> {
    type Awaitable = Self;
    fn into_await_on(self) -> Self {
        self
    }
}
impl<H: ReceiverFamily, O: CallOn<H>> AwaitOn<H> for Next<'_, H, O> {
    type Output = Option<CoroutineState<O::Yield, O::Return>>;
    fn poll_on<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn crate::ReceiverScope<'visit, Family = H>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        H: 'visit,
    {
        let this = self.get_mut();
        assert!(!this.terminal, "next awaited after completion or panic");
        this.terminal = true;
        match this.child.as_mut().poll_event(host, cx) {
            Poll::Pending => {
                this.terminal = false;
                Poll::Pending
            }
            ready => ready,
        }
    }
}
