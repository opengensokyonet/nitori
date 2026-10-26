#![allow(clippy::multiple_bound_locations)] // pin-project-lite projection bounds
//! Executors: values that run operations with their own access policy, so
//! that `executor.operation(..).await` works whether the executor can lend a
//! view at once or must acquire one.
use crate::{
    Call, CallPoll, DynCall, Operation, Receiver, ReceiverFamily, ReceiverSlot, Start,
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

/// A view acquired lazily for one poll. Ready is sticky until the scope is
/// dropped; Pending must arrange a wakeup.
pub trait ReceiverScope<'view> {
    type Family: ReceiverFamily + 'view;
    fn poll_view(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Pin<&mut <Self::Family as ReceiverFamily>::ReceiverView<'view>>>;

    /// Poll `call` for one event without a view, then with this scope's view
    /// if its operation asks for one.
    fn poll_call<C, Y, R>(
        mut self: Pin<&mut Self>,
        mut call: Pin<&mut C>,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<R, Y>>
    where
        Self: Sized,
        C: DynCall<Self::Family, Y, R> + ?Sized,
    {
        match call.as_mut().poll_with(None, cx) {
            CallPoll::Ready(event) => Poll::Ready(event),
            CallPoll::Pending => Poll::Pending,
            CallPoll::NeedsView => {
                let view = ready!(self.as_mut().poll_view(cx));
                call.poll_with(Some(view), cx).lent()
            }
        }
    }
}

/// Poll `call` to its final value as `poll_call` does, keeping the view for
/// the rest of the poll once acquired.
fn poll_return_call<'view, S, C, Y, R>(
    mut scope: Pin<&mut S>,
    mut call: Pin<&mut C>,
    cx: &mut Context<'_>,
) -> Poll<R>
where
    S: ReceiverScope<'view>,
    C: DynCall<S::Family, Y, R> + ?Sized,
{
    match call.as_mut().poll_return_with(None, cx) {
        CallPoll::Ready(result) => Poll::Ready(result),
        CallPoll::Pending => Poll::Pending,
        CallPoll::NeedsView => {
            let view = ready!(scope.as_mut().poll_view(cx));
            call.poll_return_with(Some(view), cx).lent()
        }
    }
}

/// Acquires complete views, keeping a pending acquisition across polls. A
/// new acquisition starts after each delivered view; dropping the source
/// cancels a pending one.
pub trait ViewAcquisition<'view> {
    type Family: ReceiverFamily + 'view;
    fn poll_acquire(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<<Self::Family as ReceiverFamily>::ReceiverView<'view>>;
}

pin_project_lite::pin_project! {
    /// Acquires views by starting a future from `make` whenever none is pending.
    pub struct AcquisitionState<F, Make, Acquire> {
        make: Make,
        #[pin]
        pending: InPlace<Option<Acquire>>,
        family: PhantomData<fn() -> F>,
    }
}
impl<F, Make, Acquire> AcquisitionState<F, Make, Acquire> {
    pub fn new(make: Make) -> Self {
        Self {
            make,
            pending: InPlace::new(None),
            family: PhantomData,
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
    fn poll_acquire(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::ReceiverView<'view>> {
        let this = self.project();
        let mut pending = this.pending.get_pin_mut();
        if pending.is_none() {
            pending.set(Some((this.make)()));
        }
        let view = ready!(pending.as_mut().as_pin_mut().unwrap().poll(cx));
        pending.set(None);
        Poll::Ready(view)
    }
}

pin_project_lite::pin_project! {
    /// One poll's scope over an acquisition. The view it acquires is
    /// released when the scope is dropped at the end of the poll.
    pub struct AcquiredScope<'source, 'view, S: ?Sized, F: ReceiverFamily>
    where
        F: 'view,
    {
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
    ) -> Poll<Pin<&mut F::ReceiverView<'view>>> {
        let mut this = self.project();
        if this.view.is_none() {
            let view = ready!(this.source.as_mut().poll_acquire(cx));
            this.view.set(Some(view));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap())
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
        call.poll_return_with(Some(view), cx).lent()
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
        call.poll_with(Some(view), cx).lent().map(Some)
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
/// polls whose operation asks for it and releasing it at the end of each.
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
        let (acquisition, call) = self.project();
        let scope = pin!(AcquiredScope::<_, S::Family>::new(acquisition));
        poll_return_call(scope, call, cx)
    }
}
impl<'view, S, Op> Stream for Drive<'view, S, Op>
where
    S: ViewAcquisition<'view>,
    Op: Operation<S::Family> + for<'s> Start<'s, ReceiverSlot<'s, S::Family>>,
{
    type Item = Event<S::Family, Op>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let (acquisition, call) = self.project();
        if call.is_terminated() {
            return Poll::Ready(None);
        }
        let scope = pin!(AcquiredScope::<_, S::Family>::new(acquisition));
        scope.poll_call(call, cx).map(Some)
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
