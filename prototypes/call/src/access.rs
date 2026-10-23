//! Access to the view lent for the current poll.
use crate::{Compose, Composed, DynCall, ReceiverFamily, ReceiverScope};
use std::{
    marker::PhantomData,
    ops::ControlFlow,
    pin::Pin,
    task::{Context, Poll},
};

/// A nominal lifetime boundary for one synchronous use of the current view.
///
/// Borrowed families need not have views valid for every lifetime, and a
/// callback result cannot borrow the temporary access.
pub struct Access<'a, 'h, F: ReceiverFamily + 'h> {
    view: Pin<&'a mut F::ReceiverView<'h>>,
}
impl<'a, 'h, F: ReceiverFamily + 'h> Access<'a, 'h, F> {
    pub fn new(view: Pin<&'a mut F::ReceiverView<'h>>) -> Self {
        Self { view }
    }
    pub fn into_pin(self) -> Pin<&'a mut F::ReceiverView<'h>> {
        self.view
    }
}

/// Receives the scope lent for the current poll, once.
///
/// The scope type stays static through compositions; only the root of a call
/// lends an erased scope.
pub trait ScopeVisitor<F: ReceiverFamily> {
    type Output;
    fn visit<'view, S: ReceiverScope<'view, Family = F>>(
        self,
        scope: Pin<&mut S>,
        cx: &mut Context<'_>,
    ) -> Self::Output
    where
        F: 'view;
}

/// The dyn-compatible form of a visitor, for erased slots.
pub(crate) trait ScopeRequest<F: ReceiverFamily> {
    fn run<'view>(
        &mut self,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'view;
}

/// Runs a visitor from an erased request.
struct Visit<F: ReceiverFamily, V: ScopeVisitor<F>> {
    visitor: Option<V>,
    output: Option<V::Output>,
}
impl<F: ReceiverFamily, V: ScopeVisitor<F>> ScopeRequest<F> for Visit<F, V> {
    fn run<'view>(
        &mut self,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'view,
    {
        let mut scope = scope;
        let visitor = self.visitor.take().expect("scope request ran twice");
        self.output = Some(visitor.visit(Pin::new(&mut scope), cx));
    }
}

/// Forwards a visit to an erased request.
struct Forward<'r, F: ReceiverFamily>(&'r mut dyn ScopeRequest<F>);
impl<F: ReceiverFamily> ScopeVisitor<F> for Forward<'_, F> {
    type Output = ();
    fn visit<'view, S: ReceiverScope<'view, Family = F>>(
        self,
        scope: Pin<&mut S>,
        cx: &mut Context<'_>,
    ) where
        F: 'view,
    {
        self.0.run(scope, cx);
    }
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

/// A value that can lend a view of its family during the current poll.
///
/// Borrow it mutably for each use: at most one operation uses an access at a
/// time, as with any `&mut` resource.
pub trait ReceiverAccess {
    type Family: ReceiverFamily;

    /// Distinguishes accesses derived from the probe of [`Call`](crate::Call)'s
    /// `Send` check. Accesses wrapping another access forward its mode.
    #[doc(hidden)]
    #[allow(private_bounds)]
    type Mode: Mode = Local;

    /// Lend the current scope to `visitor` exactly once.
    fn visit_scope<V: ScopeVisitor<Self::Family>>(
        &mut self,
        cx: &mut Context<'_>,
        visitor: V,
    ) -> V::Output;

    /// Run `body` once with the current view, or return Pending while the
    /// caller's scope is still acquiring it.
    fn poll_access<R, B>(&mut self, cx: &mut Context<'_>, body: B) -> Poll<R>
    where
        Self: Sized,
        B: for<'a, 'h> FnOnce(Access<'a, 'h, Self::Family>, &mut Context<'_>) -> Poll<R>,
    {
        self.visit_scope(
            cx,
            AccessVisitor {
                body,
                output: PhantomData,
            },
        )
    }

    /// Adapt this access with composition state for the borrow's duration.
    /// Pass `pin!(state)` for state that is not `Unpin`.
    fn compose<S: Compose<Self::Family> + Unpin>(&mut self, state: S) -> Composed<'_, Self, S>
    where
        Self: Sized,
    {
        Composed::new(self, state)
    }

    /// Poll a nested call with this access's scope, observing its events.
    fn poll_call<C, Y, R>(
        &mut self,
        call: Pin<&mut C>,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<R, Y>>
    where
        Self: Sized,
        C: DynCall<Self::Family, Y, R> + ?Sized,
    {
        self.visit_scope(
            cx,
            CallVisitor {
                call,
                marker: PhantomData,
            },
        )
    }

    /// Erase this access into a slot. Generic operations given erased
    /// accesses are instantiated once per family rather than per access type.
    fn erase(&mut self) -> ReceiverSlot<'_, Self::Family, Self::Mode>
    where
        Self: Sized,
    {
        ReceiverSlot::erased(self)
    }
}

impl<A: ReceiverAccess + ?Sized> ReceiverAccess for &mut A {
    type Family = A::Family;
    type Mode = A::Mode;
    fn visit_scope<V: ScopeVisitor<A::Family>>(
        &mut self,
        cx: &mut Context<'_>,
        visitor: V,
    ) -> V::Output {
        (**self).visit_scope(cx, visitor)
    }
}

struct AccessVisitor<B, R> {
    body: B,
    output: PhantomData<fn() -> R>,
}
impl<F: ReceiverFamily, B, R> ScopeVisitor<F> for AccessVisitor<B, R>
where
    B: for<'a, 'h> FnOnce(Access<'a, 'h, F>, &mut Context<'_>) -> Poll<R>,
{
    type Output = Poll<R>;
    fn visit<'view, S: ReceiverScope<'view, Family = F>>(
        self,
        scope: Pin<&mut S>,
        cx: &mut Context<'_>,
    ) -> Poll<R>
    where
        F: 'view,
    {
        match scope.poll_view(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(view) => (self.body)(Access::new(view), cx),
        }
    }
}

struct CallVisitor<'c, C: ?Sized, Y, R> {
    call: Pin<&'c mut C>,
    marker: PhantomData<fn() -> (Y, R)>,
}
impl<F: ReceiverFamily, C: DynCall<F, Y, R> + ?Sized, Y, R> ScopeVisitor<F>
    for CallVisitor<'_, C, Y, R>
{
    type Output = Poll<ControlFlow<R, Y>>;
    fn visit<'view, S: ReceiverScope<'view, Family = F>>(
        self,
        scope: Pin<&mut S>,
        cx: &mut Context<'_>,
    ) -> Self::Output
    where
        F: 'view,
    {
        self.call.poll_with(scope, cx)
    }
}

pub(crate) type Dispatch<F> = unsafe fn(*mut (), &mut Context<'_>, &mut dyn ScopeRequest<F>);

/// An exclusive handle to the views lent to one call.
///
/// The operation of a [`Call`](crate::Call) receives its root slot by value.
/// `'s` is a branded lifetime: the operation must accept every `'s`, so the
/// slot can neither be returned nor stored outside the operation. A slot is
/// neither `Send` nor `Sync`: it is used only by the thread polling its call.
#[allow(private_bounds)]
pub struct ReceiverSlot<'s, F: ReceiverFamily, M: Mode = Local> {
    target: *mut (),
    dispatch: Dispatch<F>,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
    family: PhantomData<fn(F) -> F>,
    mode: PhantomData<fn() -> M>,
}
// SAFETY: probe slots are never constructed; they only stand in for the slot
// when a call checks which other parts of its operation are `Send`.
unsafe impl<F: ReceiverFamily> Send for ReceiverSlot<'_, F, Probe> {}
// SAFETY: as above.
unsafe impl<F: ReceiverFamily> Sync for ReceiverSlot<'_, F, Probe> {}

#[allow(private_bounds)]
impl<'s, F: ReceiverFamily, M: Mode> ReceiverSlot<'s, F, M> {
    /// # Safety
    /// For `'s`, `dispatch(target, ..)` must be sound to call on the thread
    /// that owns the slot, and must lend a valid scope exactly once or panic.
    pub(crate) unsafe fn from_raw(target: *mut (), dispatch: Dispatch<F>) -> Self {
        Self {
            target,
            dispatch,
            brand: PhantomData,
            family: PhantomData,
            mode: PhantomData,
        }
    }
    fn erased<A: ReceiverAccess<Family = F, Mode = M>>(access: &'s mut A) -> Self {
        unsafe fn dispatch<A: ReceiverAccess>(
            target: *mut (),
            cx: &mut Context<'_>,
            request: &mut dyn ScopeRequest<A::Family>,
        ) {
            // SAFETY: the pointer comes from an exclusive borrow that outlives
            // the slot, and the slot is borrowed mutably for this call.
            unsafe { &mut *target.cast::<A>() }.visit_scope(cx, Forward(request))
        }
        // SAFETY: the access stays exclusively borrowed for 's, and the slot
        // stays on the thread of that borrow.
        unsafe { Self::from_raw((access as *mut A).cast(), dispatch::<A>) }
    }
}
#[allow(private_bounds)]
impl<F: ReceiverFamily, M: Mode> ReceiverAccess for ReceiverSlot<'_, F, M> {
    type Family = F;
    type Mode = M;
    fn visit_scope<V: ScopeVisitor<F>>(&mut self, cx: &mut Context<'_>, visitor: V) -> V::Output {
        let mut request = Visit::<F, V> {
            visitor: Some(visitor),
            output: None,
        };
        // SAFETY: from_raw's contract; the slot is not Send, so this is the
        // thread that owns it.
        unsafe { (self.dispatch)(self.target, cx, &mut request) };
        request
            .output
            .expect("receiver access did not lend its scope")
    }
}
