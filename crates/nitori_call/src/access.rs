//! Access to the view a call lends for the current poll.
use crate::{Compose, Composed, Lent, ReceiverFamily, ViewOperation, loan::RawViewLoan};
use alloc::{boxed::Box, vec::Vec};
use core::{
    cell::Cell,
    fmt,
    future::Future,
    marker::PhantomData,
    pin::Pin,
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
        ReleasePoint(Offer {
            lent: self.view.get().is_some(),
            releasable: self.releasable.get(),
            kept: self.kept.get(),
            release: &self.release,
            branches: &self.branches,
            root: self.root.get(),
        })
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

/// Where an operation offers back the view of the current poll.
pub struct ReleasePoint<'a>(Offer<'a>);
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
    /// Offer the view back unless there is none to release. `offered`
    /// records whether this release point already did. Like a request, an
    /// offer wakes nothing: the caller answers at once, and a branch of an
    /// executor between the call and this point is woken before the call
    /// is polled with the answer.
    fn poll(self, offered: &mut bool, cx: &mut Context<'_>) -> Poll<()> {
        let offer = self.0;
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

mod plan_sealed {
    pub trait Sealed {}
}
pub(crate) use plan_sealed::Sealed as PlanSealed;

mod lentness_sealed {
    pub trait Sealed {}
}
/// Whether an access always has its view at hand: [`AtHand`] or
/// [`OnPoll`]. Only this crate's plans decide it.
pub trait Lentness: lentness_sealed::Sealed {}
/// The view is at hand whenever the access is used: a lent access.
pub struct AtHand;
/// The view is lent for a poll, and may be missing.
pub struct OnPoll;
impl lentness_sealed::Sealed for AtHand {}
impl lentness_sealed::Sealed for OnPoll {}
impl Lentness for AtHand {}
impl Lentness for OnPoll {}

/// A sealed plan issued by the framework to visit or release the current view.
pub trait AccessPlan<'a>: PlanSealed {
    /// The family of the view reached by this plan.
    type Family: ReceiverFamily;
    /// Whether the plan's view is always at hand.
    type Lentness: Lentness;

    #[doc(hidden)]
    fn visit<V: ViewVisitor<Self::Family>>(self, visitor: V) -> V::Output;
    #[doc(hidden)]
    fn release(self) -> ReleasePoint<'a>;
}

/// Provide an access by issuing a framework plan for the current view.
///
/// A wrapper can forward the plan of its stored access, or use
/// [`project_access`](crate::project_access) to expose an adapted view.
/// The framework implements [`ReceiverAccess`] for every implementation.
///
/// ```
/// use nitori_call::{AccessPlan, ProvideAccess, ReceiverAccess};
///
/// struct Payload<A> {
///     io: A,
/// }
///
/// impl<A: ReceiverAccess> ProvideAccess for Payload<A> {
///     type AccessFamily = A::Family;
///     type Lentness = A::Lentness;
///
///     fn access_plan<'a>(
///         &'a mut self,
///     ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
///         self.io.access_plan()
///     }
/// }
///
/// fn accepts_access(_: &mut impl ReceiverAccess) {}
///
/// fn read_payload<A: ReceiverAccess>(io: A) {
///     accepts_access(&mut Payload { io });
/// }
/// ```
pub trait ProvideAccess {
    /// The family of views the plan makes available.
    type AccessFamily: ReceiverFamily;
    /// Whether the plans have their view at hand: the plan's own, which a
    /// wrapper states as its parent's.
    type Lentness: Lentness;
    /// Issue the plan for this exclusive borrow.
    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness>;
}

/// An access is lent when its plans have their view at hand.
impl<A: ProvideAccess<Lentness = AtHand> + ?Sized> crate::LentAccess for A {}

impl<A: ProvideAccess + ?Sized> Sealed for A {}

impl<A: ProvideAccess + ?Sized> ReceiverAccess for A {
    type Family = A::AccessFamily;

    fn visit_view<V: ViewVisitor<Self::Family>>(&mut self, visitor: V) -> V::Output {
        self.access_plan().visit(visitor)
    }

    fn release_point(&mut self) -> ReleasePoint<'_> {
        self.access_plan().release()
    }
}

impl<A: ProvideAccess + ?Sized> ProvideAccess for &mut A {
    type AccessFamily = A::AccessFamily;
    type Lentness = A::Lentness;

    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        (**self).access_plan()
    }
}

// The framework retains direct control of built-in accesses.
pub(crate) trait RawAccess {
    type Family: ReceiverFamily;
    type Lentness: Lentness;
    fn raw_visit<V: ViewVisitor<Self::Family>>(&mut self, visitor: V) -> V::Output;
    fn raw_release(&mut self) -> ReleasePoint<'_>;
}

pub(crate) struct RawPlan<'a, A>(pub(crate) &'a mut A);

impl<A: RawAccess> PlanSealed for RawPlan<'_, A> {}

impl<'a, A: RawAccess> AccessPlan<'a> for RawPlan<'a, A> {
    type Family = A::Family;
    type Lentness = A::Lentness;

    fn visit<V: ViewVisitor<Self::Family>>(self, visitor: V) -> V::Output {
        self.0.raw_visit(visitor)
    }

    fn release(self) -> ReleasePoint<'a> {
        self.0.raw_release()
    }
}

impl<F: ReceiverFamily> ProvideAccess for ReceiverSlot<'_, F> {
    type AccessFamily = F;
    type Lentness = OnPoll;

    fn access_plan<'a>(&'a mut self) -> impl AccessPlan<'a, Family = F, Lentness = OnPoll> {
        RawPlan(self)
    }
}

/// A value that can lend a view of its family during the current poll.
///
/// Borrow it mutably for each use: at most one operation uses an access at a
/// time, as with any `&mut` resource. The framework implements this trait
/// for each type that issues a plan through [`ProvideAccess`], including
/// slots, compositions, and mutable borrows of accesses.
#[allow(private_bounds)]
pub trait ReceiverAccess:
    sealed::Sealed + ProvideAccess<AccessFamily = <Self as ReceiverAccess>::Family>
{
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
#[allow(private_bounds)]
impl<F: ReceiverFamily> RawAccess for ReceiverSlot<'_, F> {
    type Family = F;
    type Lentness = OnPoll;
    fn raw_visit<V: ViewVisitor<F>>(&mut self, visitor: V) -> V::Output {
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
    fn raw_release(&mut self) -> ReleasePoint<'_> {
        // SAFETY: as in `visit_view`.
        unsafe { &*self.cell }.release_point()
    }
}
