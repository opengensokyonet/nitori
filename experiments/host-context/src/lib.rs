#![feature(coroutine_trait)]
#![deny(unsafe_op_in_unsafe_fn)]
//! Lazy, round-scoped host views independent of view reconstruction.

use std::{
    ops::CoroutineState,
    pin::Pin,
    task::{Context, Poll},
};

/// Capability identity; this does not require constructing another view.
pub trait HasFamily {
    type Family: HostFamily;
}

/// A stable capability family and its lifetime-indexed access representation.
pub trait HostFamily: Sized {
    type HostView<'view>: HasFamily<Family = Self>
    where
        Self: 'view;
}

/// Borrow the same acquired view throughout this context's scope.
///
/// The inner lifetime describes external resources, not the context's lifetime.
/// A ready context keeps its view pinned until the context is dropped. Pending
/// must arrange a wakeup and must not manufacture or expose an incomplete view.
/// Readiness is separate from lending so a projection can first check readiness
/// with a short reborrow, then transfer its entire parent loan into a cached view.
pub trait HostContext<'view> {
    type Family: HostFamily + 'view;
    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()>;

    /// Lend the initialized view. May panic if readiness was not established.
    /// Once ready, readiness and view identity persist until this context drops.
    fn ready_view<'access>(
        self: Pin<&'access mut Self>,
    ) -> Pin<&'access mut <Self::Family as HostFamily>::HostView<'view>>;

    fn poll_view<'access>(
        mut self: Pin<&'access mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Pin<&'access mut <Self::Family as HostFamily>::HostView<'view>>> {
        std::task::ready!(self.as_mut().poll_ready(cx));
        Poll::Ready(self.ready_view())
    }
}

/// Persistent operation state, independent of the source and each view borrow.
pub trait CallOn<F: HostFamily> {
    type Yield;
    type Return;
    fn poll_call<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        F: 'view;

    /// Drive through discarded yields using one context. This is equivalent to
    /// repeatedly calling poll_call, dropping each yielded value in order, until
    /// Pending or Complete. Overrides must preserve that operation order.
    /// Adapters introducing a projected context keep it alive across those yields.
    fn poll_return<'view>(
        mut self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Return>
    where
        F: 'view,
    {
        loop {
            match self.as_mut().poll_call(context.as_mut(), cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(CoroutineState::Yielded(_)) => (),
                Poll::Ready(CoroutineState::Complete(value)) => return Poll::Ready(value),
            }
        }
    }
}

pub type CallEvent<F, O> = CoroutineState<<O as CallOn<F>>::Yield, <O as CallOn<F>>::Return>;

mod source;
pub use source::{MapSource, Round, Source, ViewSource};
mod bound;
pub use bound::Bound;
mod typed;
pub use typed::{AwaitOn, Child, Next, Receiver};

mod projection;
pub use projection::{ProjectedContext, Projection, ReceivedCall};

pub mod coroutine;

mod driver;
pub use driver::{Execution, NoReceiver, ResourceDriver};
