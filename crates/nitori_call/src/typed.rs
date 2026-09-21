//! Host-free operation values and type-directed awaiting.
use crate::{CallOn, HostFamily};
use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

/// Poll an awaitable using a fresh Host borrow and task context.
/// Successive polls must use the logical Host resources required by the operation.
pub trait AwaitOn<H: HostFamily> {
    type Output;
    fn poll_on<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut H::Host<'visit>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        H: 'visit;
}
impl<H: HostFamily, F: Future> AwaitOn<H> for F {
    type Output = F::Output;
    fn poll_on<'visit>(
        self: Pin<&mut Self>,
        _: Pin<&mut H::Host<'visit>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        H: 'visit,
    {
        self.poll(cx)
    }
}
/// Convert standard futures or owned host-aware values for a call await.
pub trait IntoAwaitOn<H: HostFamily> {
    type Awaitable: AwaitOn<H>;
    fn into_await_on(self) -> Self::Awaitable;
}
impl<H: HostFamily, F: IntoFuture> IntoAwaitOn<H> for F {
    type Awaitable = F::IntoFuture;
    fn into_await_on(self) -> Self::Awaitable {
        self.into_future()
    }
}
pin_project_lite::pin_project! {
 /// An owned operation with no Host borrow; pin before requesting events.
 pub struct Child<H: HostFamily,O>{
  #[pin] operation:O,
  terminal:bool,
  marker:PhantomData<fn(*mut H)->*mut H>,
 }
}
impl<H: HostFamily, O: CallOn<H>> Child<H, O> {
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
        host: Pin<&mut H::Host<'visit>>,
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
impl<H: HostFamily, O: CallOn<H>> IntoAwaitOn<H> for Child<H, O> {
    type Awaitable = Self;
    fn into_await_on(self) -> Self {
        self
    }
}
impl<H: HostFamily, O: CallOn<H>> AwaitOn<H> for Child<H, O> {
    type Output = O::Return;
    fn poll_on<'visit>(
        mut self: Pin<&mut Self>,
        mut host: Pin<&mut H::Host<'visit>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        H: 'visit,
    {
        loop {
            match self.as_mut().poll_event(host.as_mut(), cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(CoroutineState::Yielded(_))) => {}
                Poll::Ready(Some(CoroutineState::Complete(value))) => return Poll::Ready(value),
                Poll::Ready(None) => panic!("child awaited after completion or panic"),
            }
        }
    }
}
/// A single event request borrowing a pinned child; not a standard Future.
pub struct Next<'a, H: HostFamily, O> {
    child: Pin<&'a mut Child<H, O>>,
    terminal: bool,
}
impl<H: HostFamily, O> Unpin for Next<'_, H, O> {}
impl<H: HostFamily, O: CallOn<H>> IntoAwaitOn<H> for Next<'_, H, O> {
    type Awaitable = Self;
    fn into_await_on(self) -> Self {
        self
    }
}
impl<H: HostFamily, O: CallOn<H>> AwaitOn<H> for Next<'_, H, O> {
    type Output = Option<CoroutineState<O::Yield, O::Return>>;
    fn poll_on<'visit>(
        self: Pin<&mut Self>,
        host: Pin<&mut H::Host<'visit>>,
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
