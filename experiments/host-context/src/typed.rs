use crate::{CallOn, HostContext, HostFamily};
use std::{
    future::Future,
    marker::PhantomData,
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

pub trait AwaitOn<F: HostFamily> {
    type Output;
    fn poll_on<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        F: 'view;
}
impl<F: HostFamily, T: Future> AwaitOn<F> for T {
    type Output = T::Output;
    fn poll_on<'view>(
        self: Pin<&mut Self>,
        _: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<T::Output>
    where
        F: 'view,
    {
        self.poll(cx)
    }
}

/// The identity receiver forwards context without producing another view.
pub struct Receiver<F>(PhantomData<fn() -> F>);
impl<F> Copy for Receiver<F> {}
impl<F> Clone for Receiver<F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F> Default for Receiver<F> {
    fn default() -> Self {
        Self::new()
    }
}
impl<F> Receiver<F> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
    pub fn operation<O: CallOn<F>>(self, operation: O) -> Child<F, O>
    where
        F: HostFamily,
    {
        Child::new(operation)
    }
}
pin_project_lite::pin_project! {
    pub struct Child<F, O> {
        #[pin]
        operation: O,
        terminal: bool,
        marker: PhantomData<fn() -> F>,
    }
}
impl<F: HostFamily, O: CallOn<F>> Child<F, O> {
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            terminal: false,
            marker: PhantomData,
        }
    }
    pub fn next(self: Pin<&mut Self>) -> Next<'_, F, O> {
        Next {
            child: self,
            terminal: false,
        }
    }
    fn poll_event<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<CoroutineState<O::Yield, O::Return>>>
    where
        F: 'view,
    {
        let this = self.project();
        if *this.terminal {
            return Poll::Ready(None);
        }
        *this.terminal = true;
        match this.operation.poll_call(context, cx) {
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
impl<F: HostFamily, O: CallOn<F>> AwaitOn<F> for Child<F, O> {
    type Output = O::Return;
    fn poll_on<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        F: 'view,
    {
        let this = self.project();
        assert!(!*this.terminal, "child awaited after completion or panic");
        *this.terminal = true;
        let result = this.operation.poll_return(context, cx);
        if result.is_pending() {
            *this.terminal = false;
        }
        result
    }
}
pub struct Next<'a, F, O> {
    child: Pin<&'a mut Child<F, O>>,
    terminal: bool,
}
impl<F, O> Unpin for Next<'_, F, O> {}
impl<F: HostFamily, O: CallOn<F>> AwaitOn<F> for Next<'_, F, O> {
    type Output = Option<CoroutineState<O::Yield, O::Return>>;
    fn poll_on<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        F: 'view,
    {
        let this = self.get_mut();
        assert!(!this.terminal, "next polled after completion or panic");
        this.terminal = true;
        match this.child.as_mut().poll_event(context, cx) {
            Poll::Pending => {
                this.terminal = false;
                Poll::Pending
            }
            ready => ready,
        }
    }
}
