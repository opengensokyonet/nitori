#![allow(clippy::multiple_bound_locations)] // pin-project-lite generated projections

use crate::{ReceiverFamily, ReceiverScope};
use std::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, ready},
};

/// Delivers ownership of a complete view, retaining pending acquisition state.
///
/// Implementations must cancel pending acquisition on destruction as well as
/// when `cancel_acquire` is called. Outputs may borrow external resources, but cannot borrow the temporary poll
/// of this source. A new acquisition starts after each successful delivery.
pub trait ViewAcquisition<'view> {
    type Family: ReceiverFamily + 'view;
    fn poll_acquire(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<<Self::Family as ReceiverFamily>::ReceiverView<'view>>;
    /// Cancel an acquisition that will no longer be polled. Already delivered
    /// views are owned by their round and are unaffected. This must be idempotent
    /// and valid when no acquisition is pending, including before the first poll.
    fn cancel_acquire(self: Pin<&mut Self>);
}

pin_project_lite::pin_project! {
    /// A future factory and the acquisition currently waiting for readiness.
    /// Dropping this source cancels any outstanding acquisition.
    pub struct AcquisitionState<F, Make, Acquire> {
        make: Make,
        #[pin]
        pending: Option<Acquire>,
        marker: PhantomData<fn() -> F>,
    }
}
impl<F, Make, Acquire> AcquisitionState<F, Make, Acquire> {
    pub fn new(make: Make) -> Self {
        Self {
            make,
            pending: None,
            marker: PhantomData,
        }
    }
}
impl<'view, F, Make, Acquire> ViewAcquisition<'view> for AcquisitionState<F, Make, Acquire>
where
    F: ReceiverFamily + 'view,
    Make: FnMut() -> Acquire,
    Acquire: Future<Output = F::ReceiverView<'view>>,
{
    type Family = F;
    fn poll_acquire(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::ReceiverView<'view>> {
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
    pub struct AcquiredScope<'source, 'view, S: ?Sized, F: ReceiverFamily> where F: 'view {
        source: Pin<&'source mut S>,
        #[pin]
        view: Option<F::ReceiverView<'view>>,
    }
}
impl<'source, 'view, S: ?Sized, F: ReceiverFamily + 'view> AcquiredScope<'source, 'view, S, F> {
    pub fn new(source: Pin<&'source mut S>) -> Self {
        Self { source, view: None }
    }
}
impl<'view, S: ViewAcquisition<'view, Family = F> + ?Sized, F: ReceiverFamily + 'view>
    ReceiverScope<'view> for AcquiredScope<'_, 'view, S, F>
{
    type Family = F;
    fn poll_view(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Pin<&mut F::ReceiverView<'view>>> {
        let mut this = self.project();
        if this.view.is_none() {
            let view = ready!(this.source.as_mut().poll_acquire(cx));
            this.view.set(Some(view));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap())
    }
}

pin_project_lite::pin_project! {
    /// Build an owning outer view before either view is pinned by the round.
    /// This describes a preconfigured acquisition chain, not a projection from
    /// an already pinned parent context.
    pub struct MapAcquisition<S, Map, F> {
        #[pin]
        source: S,
        map: Map,
        marker: PhantomData<fn() -> F>,
    }
}
impl<S, Map, F> MapAcquisition<S, Map, F> {
    pub fn new(source: S, map: Map) -> Self {
        Self {
            source,
            map,
            marker: PhantomData,
        }
    }
}
impl<'view, S, Map, F> ViewAcquisition<'view> for MapAcquisition<S, Map, F>
where
    S: ViewAcquisition<'view>,
    F: ReceiverFamily + 'view,
    Map: FnMut(<S::Family as ReceiverFamily>::ReceiverView<'view>) -> F::ReceiverView<'view>,
{
    type Family = F;
    fn poll_acquire(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::ReceiverView<'view>> {
        let this = self.project();
        this.source.poll_acquire(cx).map(this.map)
    }
    fn cancel_acquire(self: Pin<&mut Self>) {
        self.project().source.cancel_acquire();
    }
}
