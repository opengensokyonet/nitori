#![allow(clippy::multiple_bound_locations)] // pin-project-lite generated projections

use crate::{HostContext, HostFamily};
use std::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, ready},
};

/// Delivers ownership of a complete view, retaining pending acquisition state.
///
/// Outputs may borrow external resources, but cannot borrow the temporary poll
/// of this source. A new acquisition starts after each successful delivery.
pub trait ViewSource<'view> {
    type Family: HostFamily + 'view;
    fn poll_acquire(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<<Self::Family as HostFamily>::HostView<'view>>;
    /// Cancel an acquisition that will no longer be polled. Already delivered
    /// views are owned by their round and are unaffected. This must be idempotent
    /// and valid when no acquisition is pending, including before the first poll.
    fn cancel_acquire(self: Pin<&mut Self>);
}

pin_project_lite::pin_project! {
    /// A future factory and the acquisition currently waiting for readiness.
    /// Dropping this source cancels any outstanding acquisition.
    pub struct Source<F, Make, Acquire> {
        make: Make,
        #[pin]
        pending: Option<Acquire>,
        marker: PhantomData<fn() -> F>,
    }
}
impl<F, Make, Acquire> Source<F, Make, Acquire> {
    pub fn new(make: Make) -> Self {
        Self {
            make,
            pending: None,
            marker: PhantomData,
        }
    }
}
impl<'view, F, Make, Acquire> ViewSource<'view> for Source<F, Make, Acquire>
where
    F: HostFamily + 'view,
    Make: FnMut() -> Acquire,
    Acquire: Future<Output = F::HostView<'view>>,
{
    type Family = F;
    fn poll_acquire(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::HostView<'view>> {
        let mut this = self.project();
        if this.pending.as_ref().get_ref().is_none() {
            this.pending.set(Some((this.make)()));
        }
        let view = ready!(this.pending.as_mut().as_pin_mut().unwrap().poll(cx));
        this.pending.set(None);
        Poll::Ready(view)
    }
    fn cancel_acquire(self: Pin<&mut Self>) {
        self.project().pending.set(None);
    }
}

pin_project_lite::pin_project! {
    /// One driver's poll. Only the source survives its destruction.
    #[allow(clippy::multiple_bound_locations)]
    pub struct Round<'source, 'view, S: ?Sized, F: HostFamily> where F: 'view {
        source: Pin<&'source mut S>,
        #[pin]
        view: Option<F::HostView<'view>>,
    }
}
impl<'source, 'view, S: ?Sized, F: HostFamily + 'view> Round<'source, 'view, S, F> {
    pub fn new(source: Pin<&'source mut S>) -> Self {
        Self { source, view: None }
    }
}
impl<'view, S: ViewSource<'view, Family = F> + ?Sized, F: HostFamily + 'view> HostContext<'view>
    for Round<'_, 'view, S, F>
{
    type Family = F;
    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut this = self.project();
        if this.view.as_ref().get_ref().is_none() {
            let view = ready!(this.source.as_mut().poll_acquire(cx));
            this.view.set(Some(view));
        }
        Poll::Ready(())
    }

    fn ready_view<'access>(self: Pin<&'access mut Self>) -> Pin<&'access mut F::HostView<'view>> {
        self.project().view.as_pin_mut().expect("view is not ready")
    }
}

pin_project_lite::pin_project! {
    /// Build an owning outer view before either view is pinned by the round.
    /// This describes a preconfigured acquisition chain, not a projection from
    /// an already pinned parent context.
    pub struct MapSource<S, Map, F> {
        #[pin]
        source: S,
        map: Map,
        marker: PhantomData<fn() -> F>,
    }
}
impl<S, Map, F> MapSource<S, Map, F> {
    pub fn new(source: S, map: Map) -> Self {
        Self {
            source,
            map,
            marker: PhantomData,
        }
    }
}
impl<'view, S, Map, F> ViewSource<'view> for MapSource<S, Map, F>
where
    S: ViewSource<'view>,
    F: HostFamily + 'view,
    Map: FnMut(<S::Family as HostFamily>::HostView<'view>) -> F::HostView<'view>,
{
    type Family = F;
    fn poll_acquire(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::HostView<'view>> {
        let this = self.project();
        this.source.poll_acquire(cx).map(this.map)
    }
    fn cancel_acquire(self: Pin<&mut Self>) {
        self.project().source.cancel_acquire();
    }
}
