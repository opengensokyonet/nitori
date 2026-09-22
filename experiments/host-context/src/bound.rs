use crate::{CallEvent, CallOn, HostFamily, Round, ViewSource};
use futures_core::Stream;
use std::{
    future::Future,
    marker::PhantomData,
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll},
};

// Cancel before returning from completion or unwind, even if the terminal
// binding remains alive. Rounds borrowing this scope drop their view first.
struct AcquisitionScope<'source, 'view, S: ViewSource<'view>> {
    source: Pin<&'source mut S>,
    retain: bool,
    marker: PhantomData<fn() -> &'view ()>,
}
impl<'view, S: ViewSource<'view>> Drop for AcquisitionScope<'_, 'view, S> {
    fn drop(&mut self) {
        if !self.retain {
            self.source.as_mut().cancel_acquire();
        }
    }
}

pin_project_lite::pin_project! {
    /// Owns operation progress and acquisition progress. Each poll owns one round.
    /// Cancellation drops both, without rolling back completed IO.
    #[must_use = "bound calls do nothing until polled"]
    pub struct Bound<'view, F, S, O> {
        #[pin]
        source: S,
        #[pin]
        operation: O,
        terminal: bool,
        marker: PhantomData<fn() -> &'view F>,
    }
}
impl<'view, F, S, O> Bound<'view, F, S, O>
where
    F: HostFamily + 'view,
    S: ViewSource<'view, Family = F>,
    O: CallOn<F>,
{
    pub fn new(source: S, operation: O) -> Self {
        Self {
            source,
            operation,
            terminal: false,
            marker: PhantomData,
        }
    }
}
impl<'view, F, S, O> Stream for Bound<'view, F, S, O>
where
    F: HostFamily + 'view,
    S: ViewSource<'view, Family = F>,
    O: CallOn<F>,
{
    type Item = CallEvent<F, O>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        if *this.terminal {
            return Poll::Ready(None);
        }
        *this.terminal = true;
        let mut acquisition = AcquisitionScope {
            source: this.source,
            retain: false,
            marker: PhantomData,
        };
        let result = {
            let mut round = pin!(Round::new(acquisition.source.as_mut()));
            this.operation.poll_call(round.as_mut(), cx)
        };
        match result {
            Poll::Pending => {
                acquisition.retain = true;
                *this.terminal = false;
                Poll::Pending
            }
            Poll::Ready(event) => {
                if matches!(&event, CoroutineState::Yielded(_)) {
                    acquisition.retain = true;
                    *this.terminal = false;
                }
                Poll::Ready(Some(event))
            }
        }
    }
}
impl<'view, F, S, O> Future for Bound<'view, F, S, O>
where
    F: HostFamily + 'view,
    S: ViewSource<'view, Family = F>,
    O: CallOn<F>,
{
    type Output = O::Return;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        assert!(!*this.terminal, "call polled after completion or panic");
        *this.terminal = true;
        // Internal yields are discarded within this one round.
        let mut acquisition = AcquisitionScope {
            source: this.source,
            retain: false,
            marker: PhantomData,
        };
        let result = {
            let mut round = pin!(Round::new(acquisition.source.as_mut()));
            this.operation.poll_return(round.as_mut(), cx)
        };
        // Reset only after the view has been successfully destroyed.
        if result.is_pending() {
            acquisition.retain = true;
            *this.terminal = false;
        }
        result
    }
}
