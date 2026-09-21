//! Host-free operation values and type-directed awaiting.
use crate::CallOn;
use std::{
    future::{Future, IntoFuture},
    marker::PhantomData,
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

pub struct Receiver<'host, H: ?Sized> {
    host: Pin<&'host mut H>,
}
impl<'host, H: ?Sized> Receiver<'host, H> {
    pub fn from_pin(host: Pin<&'host mut H>) -> Self {
        Self { host }
    }
    pub fn from_mut(host: &'host mut H) -> Self
    where
        H: Unpin,
    {
        Self::from_pin(Pin::new(host))
    }
    pub fn as_ref(&self) -> Pin<&H> {
        self.host.as_ref()
    }
    pub fn as_mut(&mut self) -> Pin<&mut H> {
        self.host.as_mut()
    }
    pub fn with<R>(&mut self, body: impl for<'a> FnOnce(Pin<&'a mut H>) -> R) -> R {
        body(self.host.as_mut())
    }
}

pub trait AwaitOn<H: ?Sized> {
    type Output;
    fn poll_on(self: Pin<&mut Self>, host: Pin<&mut H>, cx: &mut Context<'_>)
    -> Poll<Self::Output>;
}
impl<H: ?Sized, F: Future> AwaitOn<H> for F {
    type Output = F::Output;
    fn poll_on(self: Pin<&mut Self>, _: Pin<&mut H>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.poll(cx)
    }
}
pub trait IntoAwaitOn<H: ?Sized> {
    type Awaitable: AwaitOn<H>;
    fn into_await_on(self) -> Self::Awaitable;
}
impl<H: ?Sized, F: IntoFuture> IntoAwaitOn<H> for F {
    type Awaitable = F::IntoFuture;
    fn into_await_on(self) -> Self::Awaitable {
        self.into_future()
    }
}
pin_project_lite::pin_project! {
 pub struct Child<H:?Sized,O>{
  #[pin] operation:O,
  terminal:bool,
  marker:PhantomData<fn(*mut H)->*mut H>,
 }
}
impl<H: ?Sized, O: CallOn<H>> Child<H, O> {
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
    fn poll_event(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<CoroutineState<O::Yield, O::Return>>> {
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
impl<H: ?Sized, O: CallOn<H>> IntoAwaitOn<H> for Child<H, O> {
    type Awaitable = Self;
    fn into_await_on(self) -> Self {
        self
    }
}
impl<H: ?Sized, O: CallOn<H>> AwaitOn<H> for Child<H, O> {
    type Output = O::Return;
    fn poll_on(
        mut self: Pin<&mut Self>,
        mut host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output> {
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
pub struct Next<'a, H: ?Sized, O> {
    child: Pin<&'a mut Child<H, O>>,
    terminal: bool,
}
impl<H: ?Sized, O> Unpin for Next<'_, H, O> {}
impl<H: ?Sized, O: CallOn<H>> IntoAwaitOn<H> for Next<'_, H, O> {
    type Awaitable = Self;
    fn into_await_on(self) -> Self {
        self
    }
}
impl<H: ?Sized, O: CallOn<H>> AwaitOn<H> for Next<'_, H, O> {
    type Output = Option<CoroutineState<O::Yield, O::Return>>;
    fn poll_on(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output> {
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
