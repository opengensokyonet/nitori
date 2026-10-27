#![allow(clippy::multiple_bound_locations)] // pin-project-lite projection bounds
//! Executors: values that run operations with their own access policy, so
//! that `executor.operation(..).await` works whether the executor can lend a
//! view at once or must acquire one.
use crate::{
    Call, CallPoll, DynCall, Lend, Operation, Receiver, ReceiverFamily, ReceiverSlot, Start,
    call::{InPlace, remaining},
};
use futures_core::{FusedFuture, FusedStream, Stream};
use std::{
    fmt,
    future::Future,
    marker::PhantomData,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, ready},
};

type ViewOf<'view, F> = <F as ReceiverFamily>::ReceiverView<'view>;

/// A view acquired lazily for one poll. Ready is sticky until the scope is
/// dropped or releases the view; Pending must arrange a wakeup.
pub trait ReceiverScope<'view> {
    type Family: ReceiverFamily + 'view;
    fn poll_view(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Lend<Pin<&mut <Self::Family as ReceiverFamily>::ReceiverView<'view>>>>;

    /// At a release point of an operation lent a releasable view: release
    /// the view if that is worthwhile now, and report whether it was
    /// released. The next `poll_view` then acquires a new one. The default
    /// keeps it.
    fn release(self: Pin<&mut Self>) -> bool {
        false
    }

    /// Poll `call` for one event without a view, then with this scope's view
    /// if its operation asks for one, and without it again when the scope
    /// releases it at a release point.
    fn poll_call<C, Y, R>(
        mut self: Pin<&mut Self>,
        mut call: Pin<&mut C>,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<R, Y>>
    where
        Self: Sized,
        C: DynCall<Self::Family, Y, R> + ?Sized,
    {
        let mut event = call.as_mut().poll_with(None, cx);
        loop {
            event = match event {
                CallPoll::Ready(event) => return Poll::Ready(event),
                CallPoll::Pending => return Poll::Pending,
                CallPoll::Releasable if self.as_mut().release() => {
                    call.as_mut().poll_with(None, cx)
                }
                CallPoll::NeedsView | CallPoll::Releasable => {
                    let view = ready!(self.as_mut().poll_view(cx));
                    call.as_mut().poll_with(Some(view), cx)
                }
            }
        }
    }
}

/// Acquires complete views, keeping a pending acquisition across polls. A
/// new acquisition starts after each delivered view; dropping the source
/// cancels a pending one.
///
/// An acquired view is either fixed, kept until the end of the poll, or
/// releasable, which lets the executor release it early at a release point of
/// the operation ([`ReceiverAccess::release_view`](crate::ReceiverAccess::release_view)).
/// Releasing pays off for resources that others contend for, such as a lock,
/// while the operation computes.
pub trait ViewAcquisition<'view> {
    type Family: ReceiverFamily + 'view;
    fn poll_acquire(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Lend<<Self::Family as ReceiverFamily>::ReceiverView<'view>>>;

    /// At a release point of an operation lent a releasable view from this
    /// acquisition: whether to release `view` now, for example only while
    /// others wait for the resource. A released view is dropped, and the next
    /// request acquires a new one. The default releases it: an acquisition
    /// that delivers releasable views has chosen to.
    fn should_release(
        self: Pin<&mut Self>,
        view: Pin<&mut <Self::Family as ReceiverFamily>::ReceiverView<'view>>,
    ) -> bool {
        let _ = view;
        true
    }

    /// Advance a pending acquisition without starting one, on a poll whose
    /// operation did not ask for a view. An acquisition that completes here
    /// releases its view at once, so a resource granted while the operation
    /// waits for something else is not held until its next request; one that
    /// is still pending keeps its place and may complete in a later
    /// [`poll_acquire`](Self::poll_acquire). The default does nothing, for
    /// acquisitions that hold nothing while pending.
    fn advance_unrequested(self: Pin<&mut Self>, cx: &mut Context<'_>) {
        let _ = cx;
    }
}

pin_project_lite::pin_project! {
    /// Acquires views by starting a future from `make` whenever none is pending.
    pub struct AcquisitionState<F, Make, Acquire> {
        make: Make,
        #[pin]
        pending: InPlace<Option<Acquire>>,
        releasable: bool,
        family: PhantomData<fn() -> F>,
    }
}
impl<F, Make, Acquire> AcquisitionState<F, Make, Acquire> {
    /// Deliver fixed views, kept until the end of each poll.
    pub fn new(make: Make) -> Self {
        Self {
            make,
            pending: InPlace::new(None),
            releasable: false,
            family: PhantomData,
        }
    }
    /// Deliver releasable views, released at every release point of the
    /// operation.
    pub fn releasable(make: Make) -> Self {
        Self {
            releasable: true,
            ..Self::new(make)
        }
    }
}
impl<F, Make, Acquire> fmt::Debug for AcquisitionState<F, Make, Acquire> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcquisitionState").finish_non_exhaustive()
    }
}
impl<'view, F, Make, Acquire> ViewAcquisition<'view> for AcquisitionState<F, Make, Acquire>
where
    F: ReceiverFamily + 'view,
    Make: FnMut() -> Acquire,
    Acquire: Future<Output = F::ReceiverView<'view>>,
{
    type Family = F;
    fn poll_acquire(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Lend<F::ReceiverView<'view>>> {
        let this = self.project();
        let mut pending = this.pending.get_pin_mut();
        if pending.is_none() {
            pending.set(Some((this.make)()));
        }
        let view = ready!(pending.as_mut().as_pin_mut().unwrap().poll(cx));
        pending.set(None);
        Poll::Ready(match this.releasable {
            true => Lend::Releasable(view),
            false => Lend::Fixed(view),
        })
    }
    fn advance_unrequested(self: Pin<&mut Self>, cx: &mut Context<'_>) {
        let mut pending = self.project().pending.get_pin_mut();
        if let Some(acquire) = pending.as_mut().as_pin_mut()
            && acquire.poll(cx).is_ready()
        {
            pending.set(None);
        }
    }
}

pin_project_lite::pin_project! {
    /// One poll's scope over an acquisition. The view it acquires is
    /// released when the scope is dropped at the end of the poll. On polls
    /// whose call did not ask for a view, the executor calls
    /// [`ViewAcquisition::advance_unrequested`] instead.
    pub struct AcquiredScope<'source, 'view, S: ?Sized, F: ReceiverFamily>
    where
        F: 'view,
    {
        source: Pin<&'source mut S>,
        #[pin]
        view: Option<Lend<F::ReceiverView<'view>>>,
    }
}
impl<'source, 'view, S: ?Sized, F: ReceiverFamily + 'view> AcquiredScope<'source, 'view, S, F> {
    pub fn new(source: Pin<&'source mut S>) -> Self {
        Self { source, view: None }
    }
}
impl<'view, S: ?Sized, F: ReceiverFamily + 'view> fmt::Debug for AcquiredScope<'_, 'view, S, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcquiredScope")
            .field("acquired", &self.view.is_some())
            .finish_non_exhaustive()
    }
}
impl<'view, S, F> ReceiverScope<'view> for AcquiredScope<'_, 'view, S, F>
where
    S: ViewAcquisition<'view, Family = F> + ?Sized,
    F: ReceiverFamily + 'view,
{
    type Family = F;
    fn poll_view(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Lend<Pin<&mut F::ReceiverView<'view>>>> {
        let mut this = self.project();
        if this.view.is_none() {
            let view = ready!(this.source.as_mut().poll_acquire(cx));
            this.view.set(Some(view));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap().as_pin_mut())
    }
    fn release(self: Pin<&mut Self>) -> bool {
        let mut this = self.project();
        let Some(view) = this.view.as_mut().as_pin_mut() else {
            return false;
        };
        let release = match view.as_pin_mut() {
            Lend::Releasable(view) => this.source.as_mut().should_release(view),
            Lend::Fixed(_) => false,
        };
        if release {
            this.view.set(None);
        }
        release
    }
}

type Output<F, Op> = <Op as Operation<F>>::Output;
type Event<F, Op> = ControlFlow<<Op as Operation<F>>::Output, <Op as Operation<F>>::Yield>;

/// Chooses how operations run on a value: how each poll obtains a view.
///
/// The execution is a future of the final value, discarding emitted items,
/// and a stream that delivers each item as `Continue` and then the final
/// value as `Break`. Every [`Receiver`] is an executor that constructs a view
/// on each poll; other executors usually acquire views with [`drive`].
pub trait CallExecutor {
    type Family: ReceiverFamily;
    type Execution<'a, Op>: Future<Output = Output<Self::Family, Op>>
        + Stream<Item = Event<Self::Family, Op>>
    where
        Self: 'a,
        Op: Operation<Self::Family> + for<'s> Start<'s, ReceiverSlot<'s, Self::Family>> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
    where
        Op: Operation<Self::Family> + for<'s> Start<'s, ReceiverSlot<'s, Self::Family>> + 'a;
}
impl<R: Receiver + ?Sized> CallExecutor for R {
    type Family = R::Family;
    type Execution<'a, Op>
        = BoundCall<'a, R, Op>
    where
        Self: 'a,
        Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> BoundCall<'a, R, Op>
    where
        Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>> + 'a,
    {
        BoundCall {
            host: self,
            call: Call::new(operation),
        }
    }
}

/// An operation run on a receiver, with a view constructed on each poll.
pub struct BoundCall<'a, R, Op>
where
    R: Receiver + ?Sized,
    Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>>,
{
    host: Pin<&'a mut R>,
    call: Call<R::Family, Op>,
}
// SAFETY: a shared reference only reads whether the call has finished.
unsafe impl<R, Op> Sync for BoundCall<'_, R, Op>
where
    R: Receiver + ?Sized,
    Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>>,
{
}
impl<'a, R, Op> BoundCall<'a, R, Op>
where
    R: Receiver + ?Sized,
    Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>>,
{
    fn project(self: Pin<&mut Self>) -> (&mut Pin<&'a mut R>, Pin<&mut Call<R::Family, Op>>) {
        // SAFETY: the call is structurally pinned and never moved out; the
        // host is not pinned.
        let this = unsafe { self.get_unchecked_mut() };
        (&mut this.host, unsafe {
            Pin::new_unchecked(&mut this.call)
        })
    }
}
impl<R, Op> fmt::Debug for BoundCall<'_, R, Op>
where
    R: Receiver + ?Sized,
    Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundCall")
            .field("terminated", &self.call.is_terminated())
            .finish_non_exhaustive()
    }
}
impl<R, Op> Future for BoundCall<'_, R, Op>
where
    R: Receiver + ?Sized,
    Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>>,
{
    type Output = Output<R::Family, Op>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (host, call) = self.project();
        let view = pin!(host.as_mut().view());
        call.poll_return_with(Some(Lend::Fixed(view)), cx).lent()
    }
}
impl<R, Op> Stream for BoundCall<'_, R, Op>
where
    R: Receiver + ?Sized,
    Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>>,
{
    type Item = Event<R::Family, Op>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let (host, call) = self.project();
        if call.is_terminated() {
            return Poll::Ready(None);
        }
        let view = pin!(host.as_mut().view());
        call.poll_with(Some(Lend::Fixed(view)), cx).lent().map(Some)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        remaining(self.call.is_terminated())
    }
}
impl<R, Op> FusedFuture for BoundCall<'_, R, Op>
where
    R: Receiver + ?Sized,
    Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>>,
{
    fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }
}
impl<R, Op> FusedStream for BoundCall<'_, R, Op>
where
    R: Receiver + ?Sized,
    Op: Operation<R::Family> + for<'s> Start<'s, ReceiverSlot<'s, R::Family>>,
{
    fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }
}

/// An operation run with views from an acquisition, acquiring one only on
/// polls whose operation asks for it and releasing it at the end of each, or
/// at a release point of the operation when the view is releasable and the
/// acquisition chooses to.
pub struct Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    acquisition: InPlace<S>,
    call: Call<S::Family, Op>,
    view: PhantomData<&'view ()>,
}
// SAFETY: a shared reference only reads whether the call has finished.
unsafe impl<'view, S, Op> Sync for Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
}
impl<'view, S, Op> Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    fn project(self: Pin<&mut Self>) -> (Pin<&mut S>, Pin<&mut Call<S::Family, Op>>) {
        // SAFETY: the acquisition and the call are structurally pinned and
        // never moved out.
        let this = unsafe { self.get_unchecked_mut() };
        unsafe {
            (
                Pin::new_unchecked(this.acquisition.get_mut()),
                Pin::new_unchecked(&mut this.call),
            )
        }
    }
}
impl<'view, S, Op> fmt::Debug for Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Drive")
            .field("terminated", &self.call.is_terminated())
            .finish_non_exhaustive()
    }
}
impl<'view, S, Op> Future for Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    type Output = Output<S::Family, Op>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (acquisition, mut call) = self.project();
        poll_acquiring(acquisition, cx, |view, cx| {
            call.as_mut().poll_return_with(view, cx)
        })
    }
}
impl<'view, S, Op> Stream for Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    type Item = Event<S::Family, Op>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let (acquisition, mut call) = self.project();
        if call.is_terminated() {
            return Poll::Ready(None);
        }
        poll_acquiring(acquisition, cx, |view, cx| {
            call.as_mut().poll_with(view, cx)
        })
        .map(Some)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        remaining(self.call.is_terminated())
    }
}
impl<'view, S, Op> FusedFuture for Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }
}
impl<'view, S, Op> FusedStream for Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }
}

/// Poll a call with views from `acquisition`: first without one, then with
/// one acquired when the operation asks, and without it again when it is
/// released at a release point. A view is released at the end of the poll.
fn poll_acquiring<'view, S, T>(
    mut acquisition: Pin<&mut S>,
    cx: &mut Context<'_>,
    mut poll: impl FnMut(
        Option<Lend<Pin<&mut ViewOf<'view, S::Family>>>>,
        &mut Context<'_>,
    ) -> CallPoll<T>,
) -> Poll<T>
where
    S: ViewAcquisition<'view>,
{
    let mut held = pin!(None::<Lend<ViewOf<'view, S::Family>>>);
    let mut event = poll(None, cx);
    loop {
        match event {
            CallPoll::Ready(value) => return Poll::Ready(value),
            CallPoll::Pending => {
                if held.is_none() {
                    acquisition.advance_unrequested(cx);
                }
                return Poll::Pending;
            }
            CallPoll::NeedsView => {
                held.set(Some(ready!(acquisition.as_mut().poll_acquire(cx))));
            }
            CallPoll::Releasable => {
                let Some(Lend::Releasable(view)) = held.as_mut().as_pin_mut().map(Lend::as_pin_mut)
                else {
                    unreachable!("a call offered back a view that is not releasable")
                };
                if acquisition.as_mut().should_release(view) {
                    held.set(None);
                }
            }
        }
        event = poll(held.as_mut().as_pin_mut().map(Lend::as_pin_mut), cx);
    }
}

/// Run `operation` with views from `acquisition`.
pub fn drive<'view, S, Op>(acquisition: S, operation: Op) -> Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    Drive {
        acquisition: InPlace::new(acquisition),
        call: Call::new(operation),
        view: PhantomData,
    }
}
