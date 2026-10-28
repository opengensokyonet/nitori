#![allow(clippy::multiple_bound_locations)] // pin-project-lite projection bounds
//! Executors: values that run operations with their own access policy, so
//! that `executor.operation(..).await` works whether the executor can lend a
//! view at once or must acquire one.
use crate::{
    Call, CallPoll, Compose, DynCall, Lend, Operation, Receiver, ReceiverFamily, Receivers,
    RootOperation, ViewLoan,
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

/// Chooses how operations run on a value: how each poll obtains its views.
///
/// The execution is a future of the final value, discarding emitted items,
/// and a stream that delivers each item as `Continue` and then the final
/// value as `Break`. Every [`Receiver`] is an executor that constructs a view
/// on each poll; other executors usually acquire views with [`drive`].
pub trait CallExecutor {
    /// The family the operations run on, or a tuple of them: always
    /// [`Receivers`]. Bounding it here would keep the slots of a single
    /// family from being known in code generic over the executor.
    type Receivers;
    type Execution<'a, Op>: Future<Output = Output<Self::Receivers, Op>>
        + Stream<Item = Event<Self::Receivers, Op>>
    where
        Self: 'a,
        Self::Receivers: Receivers,
        Op: RootOperation<Self::Receivers> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
    where
        Self::Receivers: Receivers,
        Op: RootOperation<Self::Receivers> + 'a;
}
impl<R: Receiver + ?Sized> CallExecutor for R {
    type Receivers = R::Family;
    type Execution<'a, Op>
        = BoundCall<'a, R, Op>
    where
        Self: 'a,
        Op: RootOperation<R::Family> + 'a;
    fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> BoundCall<'a, R, Op>
    where
        Op: RootOperation<R::Family> + 'a,
    {
        BoundCall {
            host: self,
            call: Call::new(operation),
        }
    }
}

mod hosts {
    //! The receivers a `BoundCall` constructs views of. Unnameable outside
    //! this crate: a receiver, or a tuple of them.
    use crate::{CallPoll, Lend, Receiver, Receivers, RootOperation, ViewLoan};
    use std::{
        ops::ControlFlow,
        pin::{Pin, pin},
        task::Context,
    };

    /// Receives the lends of one poll.
    pub trait LendsVisitor<R: Receivers> {
        type Output;
        fn visit<'x>(self, lends: R::Lends<'x>) -> Self::Output
        where
            R: 'x;
    }

    /// Polls a call with the lends it is given, for its next event, or with
    /// `DISCARD` for its final value.
    pub struct PollCall<'c, 'x, R: Receivers, Op: RootOperation<R>, const DISCARD: bool> {
        pub call: Pin<&'c mut crate::Call<R, Op>>,
        pub cx: &'c mut Context<'x>,
    }
    impl<R: Receivers, Op: RootOperation<R>, const DISCARD: bool> LendsVisitor<R>
        for PollCall<'_, '_, R, Op, DISCARD>
    {
        type Output = CallPoll<
            ControlFlow<<Op as crate::Operation<R>>::Output, <Op as crate::Operation<R>>::Yield>,
        >;
        #[inline(always)]
        fn visit<'x>(self, lends: R::Lends<'x>) -> Self::Output
        where
            R: 'x,
        {
            match DISCARD {
                true => match self.call.poll_return_with_views(lends, self.cx) {
                    CallPoll::Ready(value) => CallPoll::Ready(ControlFlow::Break(value)),
                    CallPoll::Pending => CallPoll::Pending,
                    CallPoll::NeedsView => CallPoll::NeedsView,
                    CallPoll::Releasable => CallPoll::Releasable,
                },
                false => self.call.poll_with_views(lends, self.cx),
            }
        }
    }

    /// Constructs a view of each receiver for one poll.
    pub trait Hosts {
        type Receivers: Receivers;
        fn with_views<V: LendsVisitor<Self::Receivers>>(
            self: Pin<&mut Self>,
            visitor: V,
        ) -> V::Output;
    }
    impl<R: Receiver + ?Sized> Hosts for R {
        type Receivers = R::Family;
        #[inline(always)]
        fn with_views<V: LendsVisitor<R::Family>>(self: Pin<&mut Self>, visitor: V) -> V::Output {
            let mut view = pin!(self.view());
            visitor.visit(Some(Lend::Fixed(ViewLoan::new(view.as_mut()))))
        }
    }
    macro_rules! tuple_hosts {
        ($($name:ident $view:ident $index:tt),+) => {
            impl<$($name: Receiver),+> Hosts for ($($name,)+) {
                type Receivers = ($($name::Family,)+);
                #[inline]
                fn with_views<V: LendsVisitor<Self::Receivers>>(
                    self: Pin<&mut Self>,
                    visitor: V,
                ) -> V::Output {
                    // SAFETY: a tuple has no destructor and is `Unpin` only
                    // when every field is, so its fields are pinned with it;
                    // none is moved out.
                    let this = unsafe { self.get_unchecked_mut() };
                    $(let mut $view = pin!(unsafe { Pin::new_unchecked(&mut this.$index) }.view());)+
                    visitor.visit(($(Some(Lend::Fixed(ViewLoan::new($view.as_mut()))),)+))
                }
            }
            /// Runs operations over every receiver of the tuple, each lent a
            /// view constructed on each poll.
            impl<$($name: Receiver),+> crate::CallExecutor for ($($name,)+) {
                type Receivers = ($($name::Family,)+);
                type Execution<'a, Op>
                    = super::BoundCall<'a, Self, Op>
                where
                    Self: 'a,
                    Op: crate::RootOperation<Self::Receivers> + 'a;
                fn execute<'a, Op>(self: Pin<&'a mut Self>, operation: Op) -> Self::Execution<'a, Op>
                where
                    Op: crate::RootOperation<Self::Receivers> + 'a,
                {
                    super::BoundCall {
                        host: self,
                        call: crate::Call::new(operation),
                    }
                }
            }
        };
    }
    tuple_hosts!(A a 0, B b 1);
    tuple_hosts!(A a 0, B b 1, C c 2);
    tuple_hosts!(A a 0, B b 1, C c 2, D d 3);
}
use hosts::{Hosts, LendsVisitor, PollCall};

/// An operation run on a receiver, or a tuple of them, with a view of each
/// constructed on each poll.
pub struct BoundCall<'a, H, Op>
where
    H: Hosts + ?Sized,
    Op: RootOperation<H::Receivers>,
{
    host: Pin<&'a mut H>,
    call: Call<H::Receivers, Op>,
}
// SAFETY: a shared reference only reads whether the call has finished.
unsafe impl<H, Op> Sync for BoundCall<'_, H, Op>
where
    H: Hosts + ?Sized,
    Op: RootOperation<H::Receivers>,
{
}
impl<'a, H, Op> BoundCall<'a, H, Op>
where
    H: Hosts + ?Sized,
    Op: RootOperation<H::Receivers>,
{
    fn project(self: Pin<&mut Self>) -> (&mut Pin<&'a mut H>, Pin<&mut Call<H::Receivers, Op>>) {
        // SAFETY: the call is structurally pinned and never moved out; the
        // host is not pinned.
        let this = unsafe { self.get_unchecked_mut() };
        (&mut this.host, unsafe {
            Pin::new_unchecked(&mut this.call)
        })
    }
}
impl<H, Op> fmt::Debug for BoundCall<'_, H, Op>
where
    H: Hosts + ?Sized,
    Op: RootOperation<H::Receivers>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundCall")
            .field("terminated", &self.call.is_terminated())
            .finish_non_exhaustive()
    }
}
impl<H, Op> Future for BoundCall<'_, H, Op>
where
    H: Hosts + ?Sized,
    Op: RootOperation<H::Receivers>,
{
    type Output = Output<H::Receivers, Op>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (host, call) = self.project();
        let poll = host
            .as_mut()
            .with_views(PollCall::<_, _, true> { call, cx });
        poll.lent().map(|event| match event {
            ControlFlow::Break(value) => value,
            ControlFlow::Continue(_) => unreachable!("discarded items are not stored"),
        })
    }
}
impl<H, Op> Stream for BoundCall<'_, H, Op>
where
    H: Hosts + ?Sized,
    Op: RootOperation<H::Receivers>,
{
    type Item = Event<H::Receivers, Op>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let (host, call) = self.project();
        if call.is_terminated() {
            return Poll::Ready(None);
        }
        let poll = host
            .as_mut()
            .with_views(PollCall::<_, _, false> { call, cx });
        poll.lent().map(Some)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        remaining(self.call.is_terminated())
    }
}
impl<H, Op> FusedFuture for BoundCall<'_, H, Op>
where
    H: Hosts + ?Sized,
    Op: RootOperation<H::Receivers>,
{
    fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }
}
impl<H, Op> FusedStream for BoundCall<'_, H, Op>
where
    H: Hosts + ?Sized,
    Op: RootOperation<H::Receivers>,
{
    fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }
}

mod lend {
    //! How `Drive` lends what its acquisitions delivered. Unnameable outside
    //! this crate: lending each view as it is, or composing it first.
    use crate::{Compose, Lend, ReceiverFamily, ViewLoan};
    use std::pin::{Pin, pin};

    /// Continues with the lend of one slot.
    pub trait SlotVisitor<F: ReceiverFamily> {
        type Output;
        fn visit<'x>(self, lend: Option<Lend<ViewLoan<'x, F>>>) -> Self::Output
        where
            F: 'x;
    }

    /// Lends a delivered view to one slot, as it is or as a view derived
    /// from it for the duration of the lend.
    pub trait Lender<F: ReceiverFamily> {
        type Family: ReceiverFamily;
        fn lend<'p, V: SlotVisitor<Self::Family>>(
            self: Pin<&mut Self>,
            view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>,
            visitor: V,
        ) -> V::Output
        where
            F: 'p;
    }

    /// Lend the acquired view itself.
    #[derive(Debug, Default)]
    pub struct AsAcquired;
    impl<F: ReceiverFamily> Lender<F> for AsAcquired {
        type Family = F;
        #[inline(always)]
        fn lend<'p, V: SlotVisitor<F>>(
            self: Pin<&mut Self>,
            view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>,
            visitor: V,
        ) -> V::Output
        where
            F: 'p,
        {
            visitor.visit(view.map(|view| view.map(ViewLoan::new)))
        }
    }

    /// How one slot of a tuple driven by `drive_composed` lends its view:
    /// composed by a [`Compose`] state.
    pub trait SlotState<F: ReceiverFamily> {
        type Family: ReceiverFamily;
        fn lend_slot<'p, V: SlotVisitor<Self::Family>>(
            self: Pin<&mut Self>,
            view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>,
            visitor: V,
        ) -> V::Output
        where
            F: 'p;
    }
    impl<F: ReceiverFamily, C: Compose<F>> SlotState<F> for C {
        type Family = C::Family;
        #[inline]
        fn lend_slot<'p, V: SlotVisitor<C::Family>>(
            self: Pin<&mut Self>,
            view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>,
            visitor: V,
        ) -> V::Output
        where
            F: 'p,
        {
            compose(self, view, visitor)
        }
    }
    /// Lend the view composed by `state` from the delivered one.
    #[inline]
    pub fn compose<'p, F, C, V>(
        state: Pin<&mut C>,
        view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>,
        visitor: V,
    ) -> V::Output
    where
        F: ReceiverFamily + 'p,
        C: Compose<F>,
        V: SlotVisitor<C::Family>,
    {
        let Some(view) = view else {
            return visitor.visit(None);
        };
        let releasable = view.is_releasable();
        // The composed view borrows the delivered one for this lend only.
        let mut composed = pin!(state.compose(view.into_inner()));
        let composed = ViewLoan::new(composed.as_mut());
        visitor.visit(Some(match releasable {
            true => Lend::Releasable(composed),
            false => Lend::Fixed(composed),
        }))
    }
}
use lend::{AsAcquired, Lender, SlotState, SlotVisitor};

/// Lends a view composed from the acquired one, with the state of a
/// [`Compose`], or a tuple with a state for each of a tuple of acquisitions;
/// see [`drive_composed`].
#[derive(Debug)]
pub struct Composing<S> {
    state: S,
}
impl<F: ReceiverFamily, S: Compose<F>> Lender<F> for Composing<S> {
    type Family = S::Family;
    #[inline(always)]
    fn lend<'p, V: SlotVisitor<S::Family>>(
        self: Pin<&mut Self>,
        view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>,
        visitor: V,
    ) -> V::Output
    where
        F: 'p,
    {
        // SAFETY: the state is structurally pinned and never moved out.
        let state = unsafe { self.map_unchecked_mut(|this| &mut this.state) };
        lend::compose(state, view, visitor)
    }
}

/// Builds a view of a receiver on each lend, for a slot of a tuple driven
/// by [`drive`] whose other slots acquire their views: it is lent fixed
/// whenever the call is polled, without waiting to be asked.
#[derive(Debug)]
pub struct Constructed<'h, R: ?Sized> {
    host: Pin<&'h mut R>,
}
impl<'h, R: Receiver + ?Sized> Constructed<'h, R> {
    pub fn new(host: Pin<&'h mut R>) -> Self {
        Self { host }
    }
}

mod source {
    //! Where the views of one slot of a `Drive` come from. Unnameable
    //! outside this crate.
    use super::{Constructed, ViewAcquisition, ViewOf};
    use crate::{Lend, Receiver, ReceiverFamily};
    use std::{
        pin::{Pin, pin},
        task::{Context, Poll},
    };

    /// The views of one slot: acquired on request and held for the rest of
    /// the poll, or constructed on each lend.
    pub trait Source<'view> {
        type Family: ReceiverFamily;
        /// What the slot holds during a poll.
        type Held;
        fn released() -> Self::Held;
        fn holds(held: &Self::Held) -> bool;
        /// Acquire a view for a request; whether one is now held.
        fn fill(self: Pin<&mut Self>, held: Pin<&mut Self::Held>, cx: &mut Context<'_>) -> bool;
        /// Answer an offer at a release point.
        fn answer(self: Pin<&mut Self>, held: Pin<&mut Self::Held>);
        /// End a poll in which the slot was not asked for a view.
        fn idle(self: Pin<&mut Self>, held: &Self::Held, cx: &mut Context<'_>);
        /// Give the delivered view, if any, to `visitor`.
        fn deliver<V: Deliver<Self::Family>>(
            self: Pin<&mut Self>,
            held: Pin<&mut Self::Held>,
            visitor: V,
        ) -> V::Output;
    }

    /// Receives the view delivered to one slot.
    pub trait Deliver<F: ReceiverFamily> {
        type Output;
        fn visit<'p>(self, view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>) -> Self::Output
        where
            F: 'p;
    }

    impl<'view, S: ViewAcquisition<'view>> Source<'view> for S {
        type Family = S::Family;
        type Held = Option<Lend<ViewOf<'view, S::Family>>>;
        #[inline(always)]
        fn released() -> Self::Held {
            None
        }
        #[inline(always)]
        fn holds(held: &Self::Held) -> bool {
            held.is_some()
        }
        #[inline(always)]
        fn fill(
            self: Pin<&mut Self>,
            mut held: Pin<&mut Self::Held>,
            cx: &mut Context<'_>,
        ) -> bool {
            match ViewAcquisition::poll_acquire(self, cx) {
                Poll::Ready(view) => {
                    held.set(Some(view));
                    true
                }
                Poll::Pending => false,
            }
        }
        #[inline(always)]
        fn answer(self: Pin<&mut Self>, mut held: Pin<&mut Self::Held>) {
            let Some(Lend::Releasable(view)) = held.as_mut().as_pin_mut().map(Lend::as_pin_mut)
            else {
                unreachable!("a call offered back a view that is not releasable")
            };
            if self.should_release(view) {
                held.set(None);
            }
        }
        #[inline(always)]
        fn idle(self: Pin<&mut Self>, held: &Self::Held, cx: &mut Context<'_>) {
            if held.is_none() {
                self.advance_unrequested(cx);
            }
        }
        #[inline(always)]
        fn deliver<V: Deliver<S::Family>>(
            self: Pin<&mut Self>,
            held: Pin<&mut Self::Held>,
            visitor: V,
        ) -> V::Output {
            visitor.visit(held.as_pin_mut().map(Lend::as_pin_mut))
        }
    }

    impl<'view, R: Receiver + ?Sized> Source<'view> for Constructed<'_, R> {
        type Family = R::Family;
        type Held = ();
        fn released() {}
        fn holds(_: &()) -> bool {
            true
        }
        fn fill(self: Pin<&mut Self>, _: Pin<&mut ()>, _: &mut Context<'_>) -> bool {
            true
        }
        fn answer(self: Pin<&mut Self>, _: Pin<&mut ()>) {
            unreachable!("a call offered back a fixed view")
        }
        fn idle(self: Pin<&mut Self>, _: &(), _: &mut Context<'_>) {}
        #[inline(always)]
        fn deliver<V: Deliver<R::Family>>(
            self: Pin<&mut Self>,
            _: Pin<&mut ()>,
            visitor: V,
        ) -> V::Output {
            let view = pin!(self.get_mut().host.as_mut().view());
            visitor.visit(Some(Lend::Fixed(view)))
        }
    }
}
use source::{Deliver, Source};

/// Lends a delivered view through a lender to a slot visitor.
struct Through<'r, L, V> {
    lender: Pin<&'r mut L>,
    visitor: V,
}
impl<F: ReceiverFamily, L: Lender<F>, V: SlotVisitor<L::Family>> Deliver<F> for Through<'_, L, V> {
    type Output = V::Output;
    #[inline(always)]
    fn visit<'p>(self, view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>) -> V::Output
    where
        F: 'p,
    {
        self.lender.lend(view, self.visitor)
    }
}

mod acquisitions {
    pub trait Sealed {}
}

/// The acquisitions a [`Drive`] takes views from, lent through `L`: one
/// [`ViewAcquisition`] or [`Constructed`] receiver, or a tuple of them, one
/// for each receiver of the operation.
pub trait Acquisitions<'view, L = AsAcquired>: acquisitions::Sealed {
    /// The families lent to the operation: one, or a tuple.
    type Receivers: Receivers;
    #[doc(hidden)]
    type Held;
    #[doc(hidden)]
    fn released() -> Self::Held;
    /// Acquire a view for each slot that needs one and holds none; whether
    /// any was acquired.
    #[doc(hidden)]
    fn acquire(
        self: Pin<&mut Self>,
        held: Pin<&mut Self::Held>,
        needed: <Self::Receivers as Receivers>::Flags,
        cx: &mut Context<'_>,
    ) -> bool;
    #[doc(hidden)]
    fn release(
        self: Pin<&mut Self>,
        held: Pin<&mut Self::Held>,
        offered: <Self::Receivers as Receivers>::Flags,
    );
    #[doc(hidden)]
    fn advance_unrequested(self: Pin<&mut Self>, held: Pin<&mut Self::Held>, cx: &mut Context<'_>);
    #[doc(hidden)]
    fn lend<V: LendsVisitor<Self::Receivers>>(
        self: Pin<&mut Self>,
        lender: Pin<&mut L>,
        held: Pin<&mut Self::Held>,
        visitor: V,
    ) -> V::Output;
}

/// The lends of a single family, received one slot at a time.
struct Single<V>(V);
impl<F: ReceiverFamily, V: LendsVisitor<F>> SlotVisitor<F> for Single<V> {
    type Output = V::Output;
    #[inline(always)]
    fn visit<'x>(self, lend: Option<Lend<ViewLoan<'x, F>>>) -> V::Output
    where
        F: 'x,
    {
        self.0.visit(lend)
    }
}

impl<'view, S: Source<'view>> acquisitions::Sealed for S {}
impl<'view, S: Source<'view>, L: Lender<S::Family>> Acquisitions<'view, L> for S {
    type Receivers = L::Family;
    type Held = S::Held;
    #[inline(always)]
    fn released() -> S::Held {
        S::released()
    }
    #[inline(always)]
    fn acquire(
        self: Pin<&mut Self>,
        held: Pin<&mut S::Held>,
        _needed: bool,
        cx: &mut Context<'_>,
    ) -> bool {
        // The only slot is the one that needs a view.
        S::holds(&held) || self.fill(held, cx)
    }
    #[inline(always)]
    fn release(self: Pin<&mut Self>, held: Pin<&mut S::Held>, offered: bool) {
        if offered {
            self.answer(held);
        }
    }
    #[inline(always)]
    fn advance_unrequested(self: Pin<&mut Self>, held: Pin<&mut S::Held>, cx: &mut Context<'_>) {
        self.idle(&held, cx);
    }
    #[inline(always)]
    fn lend<V: LendsVisitor<L::Family>>(
        self: Pin<&mut Self>,
        lender: Pin<&mut L>,
        held: Pin<&mut S::Held>,
        visitor: V,
    ) -> V::Output {
        self.deliver(
            held,
            Through {
                lender,
                visitor: Single(visitor),
            },
        )
    }
}

/// One slot of a tuple, pinned for one lend.
struct Part<'r, 'view, S: Source<'view>, L> {
    source: Pin<&'r mut S>,
    held: Pin<&'r mut S::Held>,
    lender: Pin<&'r mut L>,
    view: PhantomData<&'view ()>,
}
impl<'view, S: Source<'view>, L: Lender<S::Family>> Part<'_, 'view, S, L> {
    #[inline(always)]
    fn lend<V: SlotVisitor<L::Family>>(self, visitor: V) -> V::Output {
        self.source.deliver(
            self.held,
            Through {
                lender: self.lender,
                visitor,
            },
        )
    }
}

/// Lends the view of one slot of a tuple with its state.
struct WithState<'r, C>(Pin<&'r mut C>);
impl<F: ReceiverFamily, C: SlotState<F>> Lender<F> for WithState<'_, C> {
    type Family = C::Family;
    #[inline]
    fn lend<'p, V: SlotVisitor<C::Family>>(
        self: Pin<&mut Self>,
        view: Option<Lend<Pin<&mut F::ReceiverView<'p>>>>,
        visitor: V,
    ) -> V::Output
    where
        F: 'p,
    {
        self.get_mut().0.as_mut().lend_slot(view, visitor)
    }
}

/// Lends the slots of a tuple one after another: each lend stays alive while
/// the later slots are lent and the call is polled. A chain lends its first
/// slot, then the rest through the next shorter chain, and prepends the
/// first lend to theirs.
mod chain1 {
    use super::*;
    #[inline]
    pub(super) fn lend<'view, S, L, V>((part,): (Part<'_, 'view, S, L>,), visitor: V) -> V::Output
    where
        S: Source<'view>,
        L: Lender<S::Family>,
        V: LendsVisitor<L::Family>,
    {
        part.lend(Single(visitor))
    }
}
macro_rules! chain {
    (
        $name:ident, $shorter:ident;
        $first_source:ident $first_lender:ident $first_family:ident;
        $($source:ident $lender:ident $family:ident $lend:ident),+;
        $rest:pat
    ) => {
        mod $name {
            use super::*;
            #[inline]
            pub(super) fn lend<'view, $first_source, $first_lender, $($source, $lender,)+ V>(
                (first, $($lend,)+): (Part<'_, 'view, $first_source, $first_lender>, $(Part<'_, 'view, $source, $lender>,)+),
                visitor: V,
            ) -> V::Output
            where
                $first_source: Source<'view>,
                $first_lender: Lender<$first_source::Family>,
                $($source: Source<'view>, $lender: Lender<$source::Family>,)+
                V: LendsVisitor<($first_lender::Family, $($lender::Family,)+)>,
            {
                first.lend(Head { rest: ($($lend,)+), visitor })
            }
            struct Head<'r, 'view, $($source: Source<'view>, $lender,)+ V> {
                rest: ($(Part<'r, 'view, $source, $lender>,)+),
                visitor: V,
            }
            impl<'view, $first_family: ReceiverFamily, $($source: Source<'view>, $lender: Lender<$source::Family>,)+ V>
                SlotVisitor<$first_family> for Head<'_, 'view, $($source, $lender,)+ V>
            where
                V: LendsVisitor<($first_family, $($lender::Family,)+)>,
            {
                type Output = V::Output;
                #[inline]
                fn visit<'x>(self, first: Option<Lend<ViewLoan<'x, $first_family>>>) -> V::Output
                where
                    $first_family: 'x,
                {
                    $shorter::lend(self.rest, Prepend { first, visitor: self.visitor })
                }
            }
            struct Prepend<'x, $first_family: ReceiverFamily, V> {
                first: Option<Lend<ViewLoan<'x, $first_family>>>,
                visitor: V,
            }
            impl<$first_family: ReceiverFamily, $($family: ReceiverFamily,)+ V>
                LendsVisitor<chain!(@rest $($family),+)> for Prepend<'_, $first_family, V>
            where
                V: LendsVisitor<($first_family, $($family,)+)>,
            {
                type Output = V::Output;
                #[inline]
                fn visit<'y>(self, rest: <chain!(@rest $($family),+) as Receivers>::Lends<'y>) -> V::Output
                where
                    chain!(@rest $($family),+): 'y,
                {
                    let $rest = rest;
                    self.visitor.visit((self.first, $($lend,)+))
                }
            }
        }
    };
    (@rest $family:ident) => { $family };
    (@rest $($family:ident),+) => { ($($family,)+) };
}
chain!(chain2, chain1; S1 L1 F1; S2 L2 F2 b; b);
chain!(chain3, chain2; S1 L1 F1; S2 L2 F2 b, S3 L3 F3 c; (b, c));
chain!(chain4, chain3; S1 L1 F1; S2 L2 F2 b, S3 L3 F3 c, S4 L4 F4 d; (b, c, d));

macro_rules! tuple_acquisitions {
    (
        $chain:ident;
        $($source:ident $family:ident $lender:ident $state:ident $index:tt),+
    ) => {
        impl<'view, $($source: Source<'view>),+> acquisitions::Sealed for ($($source,)+) {}

        impl<'view, $($source: Source<'view>),+> Acquisitions<'view, AsAcquired> for ($($source,)+) {
            type Receivers = ($($source::Family,)+);
            type Held = ($($source::Held,)+);
            fn released() -> Self::Held {
                ($($source::released(),)+)
            }
            fn acquire(
                self: Pin<&mut Self>,
                held: Pin<&mut Self::Held>,
                needed: <Self::Receivers as Receivers>::Flags,
                cx: &mut Context<'_>,
            ) -> bool {
                // SAFETY: tuples have no destructor and are `Unpin` only when
                // every field is; their fields are pinned structurally.
                let (this, held) = unsafe { (self.get_unchecked_mut(), held.get_unchecked_mut()) };
                let mut acquired = false;
                $(
                    if needed.$index && !$source::holds(&held.$index) {
                        acquired |= unsafe {
                            Pin::new_unchecked(&mut this.$index)
                                .fill(Pin::new_unchecked(&mut held.$index), cx)
                        };
                    }
                )+
                acquired
            }
            fn release(
                self: Pin<&mut Self>,
                held: Pin<&mut Self::Held>,
                offered: <Self::Receivers as Receivers>::Flags,
            ) {
                // SAFETY: as in `acquire`.
                let (this, held) = unsafe { (self.get_unchecked_mut(), held.get_unchecked_mut()) };
                $(
                    if offered.$index {
                        unsafe {
                            Pin::new_unchecked(&mut this.$index)
                                .answer(Pin::new_unchecked(&mut held.$index))
                        };
                    }
                )+
            }
            fn advance_unrequested(
                self: Pin<&mut Self>,
                held: Pin<&mut Self::Held>,
                cx: &mut Context<'_>,
            ) {
                // SAFETY: as in `acquire`.
                let (this, held) = unsafe { (self.get_unchecked_mut(), held.get_unchecked_mut()) };
                $(unsafe { Pin::new_unchecked(&mut this.$index) }.idle(&held.$index, cx);)+
            }
            fn lend<V: LendsVisitor<Self::Receivers>>(
                self: Pin<&mut Self>,
                _: Pin<&mut AsAcquired>,
                held: Pin<&mut Self::Held>,
                visitor: V,
            ) -> V::Output {
                // SAFETY: as in `acquire`.
                let (this, held) = unsafe { (self.get_unchecked_mut(), held.get_unchecked_mut()) };
                $(let mut $lender = AsAcquired;)+
                $chain::lend(
                    ($(Part {
                        source: unsafe { Pin::new_unchecked(&mut this.$index) },
                        held: unsafe { Pin::new_unchecked(&mut held.$index) },
                        lender: Pin::new(&mut $lender),
                        view: PhantomData,
                    },)+),
                    visitor,
                )
            }
        }

        impl<'view, $($source: Source<'view>, $state: SlotState<$source::Family>),+>
            Acquisitions<'view, Composing<($($state,)+)>> for ($($source,)+)
        {
            type Receivers = ($(<$state as SlotState<$source::Family>>::Family,)+);
            type Held = ($($source::Held,)+);
            fn released() -> Self::Held {
                ($($source::released(),)+)
            }
            fn acquire(
                self: Pin<&mut Self>,
                held: Pin<&mut Self::Held>,
                needed: <Self::Receivers as Receivers>::Flags,
                cx: &mut Context<'_>,
            ) -> bool {
                <Self as Acquisitions<'view, AsAcquired>>::acquire(self, held, needed, cx)
            }
            fn release(
                self: Pin<&mut Self>,
                held: Pin<&mut Self::Held>,
                offered: <Self::Receivers as Receivers>::Flags,
            ) {
                <Self as Acquisitions<'view, AsAcquired>>::release(self, held, offered)
            }
            fn advance_unrequested(
                self: Pin<&mut Self>,
                held: Pin<&mut Self::Held>,
                cx: &mut Context<'_>,
            ) {
                <Self as Acquisitions<'view, AsAcquired>>::advance_unrequested(self, held, cx)
            }
            fn lend<V: LendsVisitor<Self::Receivers>>(
                self: Pin<&mut Self>,
                lender: Pin<&mut Composing<($($state,)+)>>,
                held: Pin<&mut Self::Held>,
                visitor: V,
            ) -> V::Output {
                // SAFETY: as in `acquire`; the states are pinned structurally
                // in their `Composing`.
                let (this, held, states) = unsafe {
                    (
                        self.get_unchecked_mut(),
                        held.get_unchecked_mut(),
                        &mut lender.get_unchecked_mut().state,
                    )
                };
                $(let mut $lender = WithState(unsafe { Pin::new_unchecked(&mut states.$index) });)+
                $chain::lend(
                    ($(Part {
                        source: unsafe { Pin::new_unchecked(&mut this.$index) },
                        held: unsafe { Pin::new_unchecked(&mut held.$index) },
                        lender: Pin::new(&mut $lender),
                        view: PhantomData,
                    },)+),
                    visitor,
                )
            }
        }
    };
}
tuple_acquisitions!(chain2; A FA a SA 0, B FB b SB 1);
tuple_acquisitions!(chain3; A FA a SA 0, B FB b SB 1, C FC c SC 2);
tuple_acquisitions!(chain4; A FA a SA 0, B FB b SB 1, C FC c SC 2, D FD d SD 3);

/// An operation run with views from acquisitions, acquiring a view for a
/// receiver only on polls whose operation asks for it and releasing it at the
/// end of each, or at a release point of the operation when the view is
/// releasable and its acquisition chooses to. Built by [`drive`], which lends
/// the acquired views themselves, or by [`drive_composed`], which composes
/// each first. A tuple of acquisitions drives an operation over several
/// receivers, each acquired on its own.
pub struct Drive<'view, S, Op, L = AsAcquired>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
    acquisition: InPlace<S>,
    lender: InPlace<L>,
    call: Call<S::Receivers, Op>,
    view: PhantomData<&'view ()>,
}
// SAFETY: a shared reference only reads whether the call has finished.
unsafe impl<'view, S, Op, L> Sync for Drive<'view, S, Op, L>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
}
impl<'view, S, Op, L> Drive<'view, S, Op, L>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
    fn project(self: Pin<&mut Self>) -> Parts<'_, S, L, Call<S::Receivers, Op>> {
        // SAFETY: the acquisition, the lender and the call are structurally
        // pinned and never moved out.
        let this = unsafe { self.get_unchecked_mut() };
        unsafe {
            (
                Pin::new_unchecked(this.acquisition.get_mut()),
                Pin::new_unchecked(this.lender.get_mut()),
                Pin::new_unchecked(&mut this.call),
            )
        }
    }
}
impl<'view, S, Op, L> fmt::Debug for Drive<'view, S, Op, L>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Drive")
            .field("terminated", &self.call.is_terminated())
            .finish_non_exhaustive()
    }
}

/// The pinned acquisition, lender and call of a `Drive`.
type Parts<'a, S, L, C> = (Pin<&'a mut S>, Pin<&'a mut L>, Pin<&'a mut C>);

impl<'view, S, Op, L> Future for Drive<'view, S, Op, L>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
    type Output = Output<S::Receivers, Op>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (acquisition, lender, call) = self.project();
        poll_acquiring::<_, _, _, true>(acquisition, lender, call, cx).map(|event| match event {
            ControlFlow::Break(value) => value,
            ControlFlow::Continue(_) => unreachable!("discarded items are not stored"),
        })
    }
}
impl<'view, S, Op, L> Stream for Drive<'view, S, Op, L>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
    type Item = Event<S::Receivers, Op>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let (acquisition, lender, call) = self.project();
        if call.is_terminated() {
            return Poll::Ready(None);
        }
        poll_acquiring::<_, _, _, false>(acquisition, lender, call, cx).map(Some)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        remaining(self.call.is_terminated())
    }
}
impl<'view, S, Op, L> FusedFuture for Drive<'view, S, Op, L>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
    fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }
}
impl<'view, S, Op, L> FusedStream for Drive<'view, S, Op, L>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
    fn is_terminated(&self) -> bool {
        self.call.is_terminated()
    }
}

/// Poll a call with views from `acquisition`, lent through `lender`: first
/// with no acquired view, then with one acquired for each receiver whose slot
/// asks, and without it again when it is released at a release point.
/// Acquired views are released at the end of the poll, so none is held while
/// the task waits for another.
#[inline]
fn poll_acquiring<'view, S, L, Op, const DISCARD: bool>(
    mut acquisition: Pin<&mut S>,
    mut lender: Pin<&mut L>,
    mut call: Pin<&mut Call<S::Receivers, Op>>,
    cx: &mut Context<'_>,
) -> Poll<Event<S::Receivers, Op>>
where
    S: Acquisitions<'view, L>,
    Op: RootOperation<S::Receivers>,
{
    let mut held = pin!(S::released());
    loop {
        let event = acquisition.as_mut().lend(
            lender.as_mut(),
            held.as_mut(),
            PollCall::<_, _, DISCARD> {
                call: call.as_mut(),
                cx: &mut *cx,
            },
        );
        match event {
            CallPoll::Ready(event) => return Poll::Ready(event),
            CallPoll::Pending => {
                acquisition.advance_unrequested(held, cx);
                return Poll::Pending;
            }
            CallPoll::NeedsView => {
                // A pending acquisition arranges its own wakeup.
                if !acquisition
                    .as_mut()
                    .acquire(held.as_mut(), call.views_needed(), cx)
                {
                    return Poll::Pending;
                }
            }
            CallPoll::Releasable => acquisition
                .as_mut()
                .release(held.as_mut(), call.views_offered()),
        }
    }
}

/// Run `operation` with views from `acquisition`: one acquisition, or a
/// tuple of them for an operation over several receivers.
pub fn drive<'view, S, Op>(acquisition: S, operation: Op) -> Drive<'view, S, Op>
where
    S: Acquisitions<'view>,
    Op: RootOperation<S::Receivers>,
{
    Drive {
        acquisition: InPlace::new(acquisition),
        lender: InPlace::new(AsAcquired),
        call: Call::new(operation),
        view: PhantomData,
    }
}

/// Run `operation` with views from `acquisition`, each composed by `state`
/// first: the operation runs on the composed family, and the composed view
/// borrows the acquired one while it is lent. A releasable acquired view
/// makes the composed view releasable. For a tuple of acquisitions, `state`
/// is a tuple with a state for each.
///
/// With [`Guarded`](crate::Guarded) the composed view is the view of the
/// value behind a lock guard; other states can reach a part of the value,
/// such as one stream of a connection.
pub fn drive_composed<'view, S, C, Op>(
    acquisition: S,
    state: C,
    operation: Op,
) -> Drive<'view, S, Op, Composing<C>>
where
    S: Acquisitions<'view, Composing<C>>,
    Op: RootOperation<S::Receivers>,
{
    Drive {
        acquisition: InPlace::new(acquisition),
        lender: InPlace::new(Composing { state }),
        call: Call::new(operation),
        view: PhantomData,
    }
}
