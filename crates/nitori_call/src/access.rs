//! Access to the view of the current poll: lent by a call, or constructed
//! from a held receiver.
use crate::{
    Compose, Composed, Lent, LentAccess, Receiver, ReceiverFamily, ViewOperation, loan::RawViewLoan,
};
use alloc::{boxed::Box, vec::Vec};
use core::{
    cell::Cell,
    fmt,
    future::Future,
    marker::PhantomData,
    pin::{Pin, pin},
    ptr,
    task::{Context, Poll, Waker},
};

/// A nominal lifetime boundary for one synchronous use of the current view.
///
/// Borrowed families need not have views valid for every lifetime, and a
/// callback result cannot borrow the temporary access.
pub struct Access<'a, 'h, F: ReceiverFamily + 'h> {
    view: Pin<&'a mut F::ReceiverView<'h>>,
}
impl<'h, F: ReceiverFamily + 'h> fmt::Debug for Access<'_, 'h, F>
where
    F::ReceiverView<'h>: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Access").field("view", &self.view).finish()
    }
}
impl<'a, 'h, F: ReceiverFamily + 'h> Access<'a, 'h, F> {
    pub fn new(view: Pin<&'a mut F::ReceiverView<'h>>) -> Self {
        Self { view }
    }
    pub fn into_pin(self) -> Pin<&'a mut F::ReceiverView<'h>> {
        self.view
    }
    /// Reborrow the view, for several uses within one access.
    pub fn as_pin(&mut self) -> Pin<&mut F::ReceiverView<'h>> {
        self.view.as_mut()
    }
}

/// The view lent to one slot of a call for its current poll, if any, whether
/// the operation asked for one or offered it back, and the waker the call was
/// polled with. Each slot has a cell of its own, so slots on different
/// threads share no state.
pub struct ViewCell<F: ReceiverFamily> {
    pub(crate) view: Cell<Option<RawViewLoan<F>>>,
    /// The thread that lent the view: the only one allowed to use it. Read
    /// only while a view is lent, so it keeps the last lender between polls.
    pub(crate) lender: Cell<ThreadMark>,
    /// Whether the lender may release this poll's view at a release point.
    pub(crate) releasable: Cell<bool>,
    pub(crate) requested: Cell<bool>,
    /// Whether the operation reached a release point with a releasable view;
    /// the next poll answers it.
    pub(crate) release: Cell<bool>,
    /// Whether this poll answers a release point by keeping the view.
    pub(crate) kept: Cell<bool>,
    /// Whether this poll lends a view, or answers an offer, while branches
    /// wait for it; they are woken before the operation is polled.
    pub(crate) answered: Cell<bool>,
    /// The branches of intermediate executors waiting for this slot's view,
    /// or for the answer to its offer; allocated when the first one asks.
    branches: Branches,
    pub(crate) root: RootWaker,
}
impl<F: ReceiverFamily> ViewCell<F> {
    pub(crate) fn new() -> Self {
        Self {
            view: Cell::new(None),
            lender: Cell::new(ThreadMark::NONE),
            releasable: Cell::new(false),
            requested: Cell::new(false),
            release: Cell::new(false),
            kept: Cell::new(false),
            answered: Cell::new(false),
            branches: Cell::new(None),
            root: RootWaker::new(),
        }
    }
    /// Whether a branch still waits for this slot's view or answer.
    /// Only reads, so a shared reference to the call may ask.
    #[inline]
    pub(crate) fn waiting(&self) -> bool {
        // SAFETY: the branches change only through take and set, and no
        // reference to them outlives this read. The list exists only while
        // it is not empty.
        unsafe { &*self.branches.as_ptr() }.is_some()
    }
    /// Move the waiting branches to `out`.
    #[cold]
    pub(crate) fn take_branches(&self, out: &mut Vec<Waker>) {
        if let Some(mut branches) = self.branches.take() {
            out.append(&mut branches);
        }
    }
    fn release_point(&self) -> ReleasePoint<'_> {
        ReleasePoint(Some(Offer {
            lent: self.view.get().is_some(),
            releasable: self.releasable.get(),
            kept: self.kept.get(),
            release: &self.release,
            branches: &self.branches,
            root: self.root.get(),
        }))
    }
}

/// The branches waiting for a slot: present only while some branch waits, so
/// that an idle cell spends one word on them, as most never have any.
#[allow(clippy::box_collection)]
type Branches = Cell<Option<Box<Vec<Waker>>>>;

/// Keep the waker of a branch waiting for a slot's view or answer. Every
/// branch that waits is kept until the slot is answered.
fn defer(branches: &Branches, branch: &Waker) {
    let mut kept = branches.take().unwrap_or_default();
    if !kept.iter().any(|kept| kept.will_wake(branch)) {
        kept.push(branch.clone());
    }
    branches.set(Some(kept));
}

/// Identifies a thread while it runs: no two running threads share a mark.
/// A call's lender is running for as long as its view is lent.
#[cfg(feature = "std")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThreadMark(*const u8);
#[cfg(feature = "std")]
std::thread_local! {
    static MARK: u8 = const { 0 };
}
#[cfg(feature = "std")]
impl ThreadMark {
    pub(crate) const NONE: Self = Self(ptr::null());
    #[inline]
    pub(crate) fn current() -> Self {
        Self(MARK.with(ptr::from_ref))
    }
}
/// Without `std` a slot is not `Send` and never leaves the thread polling
/// its call, so every use of a view is on the lender's thread and there is
/// nothing to tell apart.
#[cfg(not(feature = "std"))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThreadMark;
#[cfg(not(feature = "std"))]
impl ThreadMark {
    pub(crate) const NONE: Self = Self;
    #[inline]
    pub(crate) fn current() -> Self {
        Self
    }
}

/// The waker a call or child is being polled with; null outside a poll.
pub(crate) struct RootWaker(Cell<*const Waker>);
// SAFETY: the pointer is non-null only during a poll, while its owner is
// borrowed mutably on the polling thread, and is not used outside one.
unsafe impl Send for RootWaker {}
impl RootWaker {
    pub(crate) fn new() -> Self {
        Self(Cell::new(ptr::null()))
    }
    /// Record `waker` for a poll; it must outlive the poll, which ends with
    /// `clear`.
    pub(crate) fn set(&self, waker: &Waker) {
        self.0.set(waker);
    }
    pub(crate) fn clear(&self) {
        self.0.set(ptr::null());
    }
    pub(crate) fn get(&self) -> *const Waker {
        self.0.get()
    }
}

/// Whether `cx` carries the waker its call was polled with: no executor
/// between the call and this point has substituted a branch waker. `root` is
/// null outside a poll, and otherwise points to that waker.
pub(crate) fn polled_directly(root: *const Waker, cx: &Context<'_>) -> bool {
    // SAFETY: a non-null root outlives the poll that set it.
    !root.is_null() && cx.waker().will_wake(unsafe { &*root })
}

/// The right to ask the caller of the current call for a view.
pub struct ViewRequest<'a> {
    requested: &'a Cell<bool>,
    branches: &'a Branches,
    root: *const Waker,
}
impl ViewRequest<'_> {
    /// Ask for a view. Nothing is woken: the caller, told the call needs a
    /// view, lends one or arranges a wakeup for when it can. An executor
    /// between the call and this point repolls only branches whose waker
    /// fired, so the call keeps this branch's waker and wakes it when it is
    /// lent a view, right before polling the operation; a list keeps every
    /// branch that asked.
    fn ask<T>(self, cx: &mut Context<'_>) -> Poll<T> {
        self.requested.set(true);
        if !polled_directly(self.root, cx) {
            defer(self.branches, cx.waker());
        }
        Poll::Pending
    }
}

/// Where an operation offers back the view of the current poll: nowhere for
/// an access that is never lent a view.
pub struct ReleasePoint<'a>(Option<Offer<'a>>);
/// The state of a slot's cell that answers a release point.
struct Offer<'a> {
    lent: bool,
    releasable: bool,
    kept: bool,
    release: &'a Cell<bool>,
    branches: &'a Branches,
    root: *const Waker,
}
impl ReleasePoint<'_> {
    /// The release point of an access that is never lent a view, and so has
    /// none to offer back: it completes at once.
    fn fixed() -> ReleasePoint<'static> {
        ReleasePoint(None)
    }
    /// Offer the view back unless there is none to release. `offered`
    /// records whether this release point already did. Like a request, an
    /// offer wakes nothing: the caller answers at once, and a branch of an
    /// executor between the call and this point is woken before the call
    /// is polled with the answer.
    fn poll(self, offered: &mut bool, cx: &mut Context<'_>) -> Poll<()> {
        let Some(offer) = self.0 else {
            return Poll::Ready(());
        };
        if !offer.lent || !offer.releasable || (*offered && offer.kept) {
            return Poll::Ready(());
        }
        *offered = true;
        offer.release.set(true);
        if !polled_directly(offer.root, cx) {
            defer(offer.branches, cx.waker());
        }
        Poll::Pending
    }
}

/// Receives the view of the current poll, or a request for one, once.
/// Not exported: every access is in this crate.
pub trait ViewVisitor<F: ReceiverFamily>: ViewOperation<F> {
    fn absent(self, request: ViewRequest<'_>) -> Self::Output;
}

mod sealed {
    /// Implemented only by this crate's accesses.
    pub trait Sealed {}
}
pub(crate) use sealed::Sealed;

/// A value that can lend a view of its family during the current poll.
///
/// Borrow it mutably for each use: at most one operation uses an access at a
/// time, as with any `&mut` resource. This crate's slots,
/// [`ConstructedAccess`], compositions, [`BorrowedAccess`](crate::BorrowedAccess),
/// and `&mut` borrows of accesses implement this trait.
#[allow(private_bounds)]
pub trait ReceiverAccess: sealed::Sealed {
    type Family: ReceiverFamily;

    /// Lend this poll's view to `visitor`, or give it a request for one.
    #[doc(hidden)]
    fn visit_view<V: ViewVisitor<Self::Family>>(&mut self, visitor: V) -> V::Output;

    /// Where this poll's view would be offered back.
    #[doc(hidden)]
    fn release_point(&mut self) -> ReleasePoint<'_>;

    /// Run `body` once with the current view. Without one, ask the caller of
    /// the call for a view and return Pending.
    fn poll_access<R, B>(&mut self, cx: &mut Context<'_>, body: B) -> Poll<R>
    where
        Self: Sized,
        B: for<'a, 'h> FnOnce(Access<'a, 'h, Self::Family>, &mut Context<'_>) -> Poll<R>,
    {
        self.visit_view(PollAccess {
            body,
            cx,
            output: PhantomData,
        })
    }

    /// Mark a release point: what follows does not need the view for a
    /// while, such as a computation between accesses. If the view lent for
    /// this poll is releasable, the executor decides whether to release it
    /// and continues at once without suspending the task; the next access
    /// then acquires a view again. Without a view, or with a fixed one, the
    /// release point completes at once.
    fn release_view(&mut self) -> ReleaseView<'_, Self>
    where
        Self: Sized,
    {
        ReleaseView {
            access: self,
            offered: false,
        }
    }

    /// Adapt this access with composition state for the borrow's duration.
    /// The state composes on [`Lent`] views of this access's family. Pass
    /// `pin!(state)` for state that is not `Unpin`. [`Composed::new`]
    /// adapts an access taken by value instead.
    fn compose<S: Compose<Lent<Self::Family>> + Unpin>(
        &mut self,
        state: S,
    ) -> Composed<&mut Self, S>
    where
        Self: Sized,
    {
        Composed::new(self, state)
    }
}

impl<A: ReceiverAccess + ?Sized> Sealed for &mut A {}
#[allow(private_bounds)]
impl<A: ReceiverAccess + ?Sized> ReceiverAccess for &mut A {
    type Family = A::Family;
    fn visit_view<V: ViewVisitor<A::Family>>(&mut self, visitor: V) -> V::Output {
        (**self).visit_view(visitor)
    }
    fn release_point(&mut self) -> ReleasePoint<'_> {
        (**self).release_point()
    }
}

/// The future of [`ReceiverAccess::release_view`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct ReleaseView<'a, A> {
    access: &'a mut A,
    offered: bool,
}
impl<A: ReceiverAccess> Future for ReleaseView<'_, A> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        this.access.release_point().poll(&mut this.offered, cx)
    }
}

/// The visitor of `poll_access`.
struct PollAccess<'c, 'x, B, R> {
    body: B,
    cx: &'c mut Context<'x>,
    output: PhantomData<fn() -> R>,
}
impl<F: ReceiverFamily, B, R> ViewOperation<F> for PollAccess<'_, '_, B, R>
where
    B: for<'a, 'h> FnOnce(Access<'a, 'h, F>, &mut Context<'_>) -> Poll<R>,
{
    type Output = Poll<R>;
    fn run<'h>(self, view: Pin<&mut F::ReceiverView<'h>>) -> Poll<R>
    where
        F: 'h,
    {
        (self.body)(Access::new(view), self.cx)
    }
}
impl<F: ReceiverFamily, B, R> ViewVisitor<F> for PollAccess<'_, '_, B, R>
where
    B: for<'a, 'h> FnOnce(Access<'a, 'h, F>, &mut Context<'_>) -> Poll<R>,
{
    fn absent(self, request: ViewRequest<'_>) -> Poll<R> {
        request.ask(self.cx)
    }
}

/// An exclusive handle to the views lent to one call.
///
/// The operation of a [`Call`](crate::Call) receives its root slot by value.
/// `'s` is a branded lifetime: the operation must accept every `'s`, so the
/// slot can neither be returned nor stored outside the operation.
///
/// With the `std` feature a slot is `Send`, so an operation that keeps one is
/// `Send` whenever the rest of what it keeps is. Within a poll it may move to
/// a scoped thread, which can ask for a view or offer it back; using the view
/// itself there panics, since only the thread polling the call may use it.
/// Without `std` there is no thread to check against, and a slot is not
/// `Send`. A slot is not `Sync`.
pub struct ReceiverSlot<'s, F: ReceiverFamily> {
    cell: *const ViewCell<F>,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
}
// SAFETY: a slot holds no view: it points into the cell of its call, which
// lends a view only while the call is polled. The brand keeps the slot inside
// the call's future, so between polls it moves only with the call, and a
// thread it reaches during a poll is a scoped one, joined before the poll
// returns; the lender clears the cell after that. The slot is not Sync and is
// used through `&mut`, so one thread at a time uses the cell's request and
// release state. The view itself is used only after checking that the
// current thread lent it, which needs `std` to tell threads apart.
#[cfg(feature = "std")]
unsafe impl<F: ReceiverFamily> Send for ReceiverSlot<'_, F> {}
// Without `std` a slot is not `Send`: the item below is ambiguous, and fails
// to compile, if it is.
#[cfg(not(feature = "std"))]
const _: () = {
    trait AmbiguousIfSend<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    struct IsSend;
    impl<T: ?Sized + Send> AmbiguousIfSend<IsSend> for T {}
    let _ = <ReceiverSlot<'static, crate::Direct<()>> as AmbiguousIfSend<_>>::some_item;
};

impl<F: ReceiverFamily> fmt::Debug for ReceiverSlot<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiverSlot").finish_non_exhaustive()
    }
}
impl<F: ReceiverFamily> ReceiverSlot<'_, F> {
    /// # Safety
    /// `cell` must belong to a pinned call that outlives the slot, and the
    /// slot must stay inside the call's future.
    pub(crate) unsafe fn new(cell: &ViewCell<F>) -> Self {
        Self {
            cell,
            brand: PhantomData,
        }
    }
}
impl<F: ReceiverFamily> Sealed for ReceiverSlot<'_, F> {}
#[allow(private_bounds)]
impl<F: ReceiverFamily> ReceiverAccess for ReceiverSlot<'_, F> {
    type Family = F;
    fn visit_view<V: ViewVisitor<F>>(&mut self, visitor: V) -> V::Output {
        // SAFETY: new's contract. No other thread uses the cell meanwhile:
        // see the Send implementation.
        let cell = unsafe { &*self.cell };
        match cell.view.get() {
            // Outside a poll, which only an operation's destructor can reach,
            // there is no view either.
            None => visitor.absent(ViewRequest {
                requested: &cell.requested,
                branches: &cell.branches,
                root: cell.root.get(),
            }),
            Some(view) => {
                // Checked before any reference to the view exists: it need
                // not be Send.
                assert!(
                    cell.lender.get() == ThreadMark::current(),
                    "a view lent to a call used outside the thread polling it"
                );
                // SAFETY: the call lent this view for its current poll, on
                // this thread. No other use of it is in progress: the slot
                // and every access derived from it are borrowed mutably for
                // each visit.
                unsafe { view.apply(visitor) }
            }
        }
    }
    fn release_point(&mut self) -> ReleasePoint<'_> {
        // SAFETY: as in `visit_view`.
        unsafe { &*self.cell }.release_point()
    }
}

/// An access that holds a receiver and constructs the receiver's view at
/// each access, instead of being lent one by a call.
///
/// A type that holds its access, such as a stream that reads one record after
/// another, is written once for every [`ReceiverAccess`]. Inside an operation
/// it holds the operation's access; elsewhere it holds a `ConstructedAccess`
/// of a receiver, and lives across as many awaits as its owner wants, with no
/// call in between. Every operation written for an access runs on it.
///
/// The view is always at hand: an access never waits for it, a release point
/// completes at once, and [`LentAccess::with`] works outside any poll too, a
/// destructor included, where a slot has no view. Each access constructs a
/// view of its own. The access is `Send` and `Sync` when the receiver is,
/// with or without `std`, and its views may be used on any thread it reaches.
///
/// The receiver is held by value or by `&mut`, and must be `Unpin`, since
/// the access moves with whatever holds it. Pin one that is not first:
/// `ConstructedAccess::new(pin!(receiver))` borrows it pinned on the stack,
/// `ConstructedAccess::new(Box::pin(receiver))` owns it pinned in a box.
///
/// ```compile_fail,E0277
/// # use nitori_call::{ConstructedAccess, Direct, ReceiverAccess};
/// # use std::marker::PhantomPinned;
/// struct Fixed(PhantomPinned);
/// nitori_call::direct_receiver!(impl [] for Fixed);
/// fn access(_: impl ReceiverAccess<Family = Direct<Fixed>>) {}
/// access(ConstructedAccess::new(Fixed(PhantomPinned)));
/// ```
#[derive(Debug)]
pub struct ConstructedAccess<R> {
    receiver: R,
}
impl<R> ConstructedAccess<R> {
    /// Access `receiver` by constructing its view at each access.
    pub fn new(receiver: R) -> Self {
        Self { receiver }
    }
    /// The receiver.
    pub fn get_ref(&self) -> &R {
        &self.receiver
    }
    /// The receiver, for use between accesses.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.receiver
    }
    /// End the access, returning the receiver.
    pub fn into_inner(self) -> R {
        self.receiver
    }
}
impl<R: Receiver + Unpin> Sealed for ConstructedAccess<R> {}
#[allow(private_bounds)]
impl<R: Receiver + Unpin> ReceiverAccess for ConstructedAccess<R> {
    type Family = R::Family;
    fn visit_view<V: ViewVisitor<R::Family>>(&mut self, visitor: V) -> V::Output {
        visitor.run(pin!(Pin::new(&mut self.receiver).view()))
    }
    fn release_point(&mut self) -> ReleasePoint<'_> {
        ReleasePoint::fixed()
    }
}
impl<R: Receiver + Unpin> LentAccess for ConstructedAccess<R> {}
