//! Access to the view lent for the current poll.
use crate::{Compose, Composed, ReceiverFamily, ViewOperation, loan::RawViewLoan};
use std::{
    cell::Cell,
    fmt,
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
}

/// The view lent to a call for its current poll, if any, whether its
/// operation asked for one, and the waker the call was polled with.
pub(crate) struct ViewCell<F: ReceiverFamily> {
    pub(crate) view: Cell<Option<RawViewLoan<F>>>,
    pub(crate) requested: Cell<bool>,
    pub(crate) root: RootWaker,
}
impl<F: ReceiverFamily> ViewCell<F> {
    pub(crate) fn new() -> Self {
        Self {
            view: Cell::new(None),
            requested: Cell::new(false),
            root: RootWaker::new(),
        }
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
    root: *const Waker,
}
impl ViewRequest<'_> {
    /// Ask for a view. An executor between the call and this point repolls
    /// only branches whose waker fired, so wake this branch for it; with
    /// none, the caller, told the call needs a view, polls again.
    fn ask<T>(self, cx: &mut Context<'_>) -> Poll<T> {
        self.requested.set(true);
        if !polled_directly(self.root, cx) {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

/// Receives the view of the current poll, or a request for one, once.
/// Not exported: every access is in this crate.
pub trait ViewVisitor<F: ReceiverFamily>: ViewOperation<F> {
    fn absent(self, request: ViewRequest<'_>) -> Self::Output;
}

pub(crate) mod mode {
    //! Unnameable outside this crate, so implementations of `Start` that cover
    //! the probe are generic over the access.

    /// Whether an access derives from a running call or from the probe that
    /// checks whether a call is `Send`.
    pub trait Mode: 'static {}
    /// Accesses of running calls.
    pub struct Local;
    /// Never constructed: stands in for the slot when a call checks `Send`.
    pub struct Probe;
    impl Mode for Local {}
    impl Mode for Probe {}
}
use mode::{Local, Mode, Probe};

mod sealed {
    /// Implemented only by this crate's accesses.
    pub trait Sealed {}
}
pub(crate) use sealed::Sealed;

/// A value that can lend a view of its family during the current poll.
///
/// Borrow it mutably for each use: at most one operation uses an access at a
/// time, as with any `&mut` resource. This crate's slots, compositions and
/// `&mut` borrows of them are the only accesses.
#[allow(private_bounds)]
pub trait ReceiverAccess: sealed::Sealed {
    type Family: ReceiverFamily;

    /// Distinguishes accesses derived from the probe of [`Call`](crate::Call)'s
    /// `Send` check. Accesses wrapping another access forward its mode.
    #[doc(hidden)]
    type Mode: Mode;

    /// Lend this poll's view to `visitor`, or give it a request for one.
    #[doc(hidden)]
    fn visit_view<V: ViewVisitor<Self::Family>>(&mut self, visitor: V) -> V::Output;

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

    /// Adapt this access with composition state for the borrow's duration.
    /// Pass `pin!(state)` for state that is not `Unpin`.
    fn compose<S: Compose<Self::Family> + Unpin>(&mut self, state: S) -> Composed<'_, Self, S>
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
    type Mode = A::Mode;
    fn visit_view<V: ViewVisitor<A::Family>>(&mut self, visitor: V) -> V::Output {
        (**self).visit_view(visitor)
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
/// slot can neither be returned nor stored outside the operation. A slot is
/// neither `Send` nor `Sync`: it is used only by the thread polling its call.
#[allow(private_bounds)]
pub struct ReceiverSlot<'s, F: ReceiverFamily, M: Mode = Local> {
    cell: *const ViewCell<F>,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
    mode: PhantomData<fn() -> M>,
}
// SAFETY: probe slots are never constructed; they only stand in for the slot
// when a call checks which other parts of its operation are `Send`.
unsafe impl<F: ReceiverFamily> Send for ReceiverSlot<'_, F, Probe> {}
// SAFETY: as above.
unsafe impl<F: ReceiverFamily> Sync for ReceiverSlot<'_, F, Probe> {}

#[allow(private_bounds)]
impl<F: ReceiverFamily, M: Mode> fmt::Debug for ReceiverSlot<'_, F, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiverSlot").finish_non_exhaustive()
    }
}
#[allow(private_bounds)]
impl<F: ReceiverFamily, M: Mode> ReceiverSlot<'_, F, M> {
    /// # Safety
    /// `cell` must belong to a pinned call that outlives the slot and is
    /// polled only on the thread that owns the slot.
    pub(crate) unsafe fn new(cell: &ViewCell<F>) -> Self {
        Self {
            cell,
            brand: PhantomData,
            mode: PhantomData,
        }
    }
}
impl<F: ReceiverFamily, M: Mode> Sealed for ReceiverSlot<'_, F, M> {}
#[allow(private_bounds)]
impl<F: ReceiverFamily, M: Mode> ReceiverAccess for ReceiverSlot<'_, F, M> {
    type Family = F;
    type Mode = M;
    fn visit_view<V: ViewVisitor<F>>(&mut self, visitor: V) -> V::Output {
        // SAFETY: new's contract; the slot is not Send, so this is the thread
        // polling the call.
        let cell = unsafe { &*self.cell };
        match cell.view.get() {
            // Outside a poll, which only an operation's destructor can reach,
            // there is no view either.
            None => visitor.absent(ViewRequest {
                requested: &cell.requested,
                root: cell.root.get(),
            }),
            // SAFETY: the call lent this view for its current poll. No other
            // use of it is in progress: the slot and every access derived
            // from it are borrowed mutably for each visit.
            Some(view) => unsafe { view.apply(visitor) },
        }
    }
}
