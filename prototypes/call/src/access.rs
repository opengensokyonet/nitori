//! Access to the view lent for the current poll.
use crate::{Compose, Composed, DynCall, ReceiverFamily, ReceiverScope};
use std::{
    marker::PhantomData,
    ops::ControlFlow,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
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

/// Receives the scope lent for the current poll.
pub trait ScopeRequest<F: ReceiverFamily> {
    fn run<'view>(
        &mut self,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'view;
}

/// Identifies the thread currently polling one call; zero between polls.
pub struct ThreadAnchor(AtomicUsize);
impl ThreadAnchor {
    pub(crate) const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }
    pub(crate) fn enter(&self) {
        self.0.store(current_thread(), Relaxed);
    }
    pub(crate) fn leave(&self) {
        self.0.store(0, Relaxed);
    }
    /// Whether this thread is inside the poll that lends the call's scope.
    pub fn is_current(&self) -> bool {
        self.0.load(Relaxed) == current_thread()
    }
}
fn current_thread() -> usize {
    thread_local! { static ANCHOR: u8 = const { 0 } }
    ANCHOR.with(|anchor| anchor as *const u8 as usize)
}

/// A value that can lend a view of its family during the current poll.
///
/// Borrow it mutably for each use: at most one operation uses an access at a
/// time, as with any `&mut` resource.
pub trait ReceiverAccess {
    type Family: ReceiverFamily;

    /// Lend the current scope to `request` exactly once.
    #[doc(hidden)]
    fn poll_scope(&mut self, cx: &mut Context<'_>, request: &mut dyn ScopeRequest<Self::Family>);

    /// The anchor of the call whose poll lends this access.
    #[doc(hidden)]
    fn thread_anchor(&self) -> &ThreadAnchor;

    /// Run `body` once with the current view, or return Pending while the
    /// caller's scope is still acquiring it.
    fn poll_access<R, B>(&mut self, cx: &mut Context<'_>, body: B) -> Poll<R>
    where
        Self: Sized,
        B: for<'a, 'h> FnOnce(Access<'a, 'h, Self::Family>, &mut Context<'_>) -> Poll<R>,
    {
        let mut request = AccessRequest::<Self::Family, B, R> {
            body: Some(body),
            output: None,
            family: PhantomData,
        };
        self.poll_scope(cx, &mut request);
        request
            .output
            .expect("receiver access did not lend its scope")
    }

    /// Adapt this access with composition state for the borrow's duration.
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
        let mut request = CallRequest {
            call,
            output: None,
            marker: PhantomData,
        };
        self.poll_scope(cx, &mut request);
        request
            .output
            .expect("receiver access did not lend its scope")
    }

    /// Erase this access into a slot, for operations that accept only slots.
    fn erase(&mut self) -> ReceiverSlot<'_, Self::Family>
    where
        Self: Sized,
    {
        ReceiverSlot::erased(self)
    }
}

struct AccessRequest<F, B, R> {
    body: Option<B>,
    output: Option<Poll<R>>,
    family: PhantomData<fn(F) -> F>,
}
impl<F: ReceiverFamily, B, R> ScopeRequest<F> for AccessRequest<F, B, R>
where
    B: for<'a, 'h> FnOnce(Access<'a, 'h, F>, &mut Context<'_>) -> Poll<R>,
{
    fn run<'view>(
        &mut self,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'view,
    {
        self.output = Some(match scope.poll_view(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(view) => {
                (self.body.take().expect("scope request ran twice"))(Access::new(view), cx)
            }
        });
    }
}

struct CallRequest<'c, C: ?Sized, Y, R> {
    call: Pin<&'c mut C>,
    output: Option<Poll<ControlFlow<R, Y>>>,
    marker: PhantomData<fn() -> (Y, R)>,
}
impl<F: ReceiverFamily, C: DynCall<F, Y, R> + ?Sized, Y, R> ScopeRequest<F>
    for CallRequest<'_, C, Y, R>
{
    fn run<'view>(
        &mut self,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'view,
    {
        self.output = Some(self.call.as_mut().poll_with(scope, cx));
    }
}

pub(crate) type Dispatch<F> = unsafe fn(*mut (), &mut Context<'_>, &mut dyn ScopeRequest<F>);

/// An exclusive handle to the views lent to one call.
///
/// The operation of a [`Call`](crate::Call) receives its root slot by value.
/// `'s` is a branded lifetime: the operation must accept every `'s`, so the
/// slot can neither be returned nor stored outside the operation. A slot
/// panics if used on a thread other than the one currently polling its call.
pub struct ReceiverSlot<'s, F: ReceiverFamily> {
    target: *mut (),
    dispatch: Dispatch<F>,
    anchor: *const ThreadAnchor,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
    family: PhantomData<fn(F) -> F>,
}
// SAFETY: `target` is dereferenced only after checking that the current thread
// is inside the poll that lends it. Other threads only read the atomic anchor,
// which outlives the slot.
unsafe impl<F: ReceiverFamily> Send for ReceiverSlot<'_, F> {}
// SAFETY: shared references expose only the atomic anchor.
unsafe impl<F: ReceiverFamily> Sync for ReceiverSlot<'_, F> {}

impl<'s, F: ReceiverFamily> ReceiverSlot<'s, F> {
    /// # Safety
    /// For `'s`, `anchor` must stay valid, and whenever it is current,
    /// `dispatch(target, ..)` must lend a valid scope exactly once.
    pub(crate) unsafe fn from_raw(
        target: *mut (),
        dispatch: Dispatch<F>,
        anchor: *const ThreadAnchor,
    ) -> Self {
        Self {
            target,
            dispatch,
            anchor,
            brand: PhantomData,
            family: PhantomData,
        }
    }
    fn erased<A: ReceiverAccess<Family = F>>(access: &'s mut A) -> Self {
        unsafe fn dispatch<A: ReceiverAccess>(
            target: *mut (),
            cx: &mut Context<'_>,
            request: &mut dyn ScopeRequest<A::Family>,
        ) {
            // SAFETY: the pointer comes from an exclusive borrow that outlives
            // the slot, and the slot is borrowed mutably for this call.
            unsafe { &mut *target.cast::<A>() }.poll_scope(cx, request)
        }
        let anchor: *const ThreadAnchor = access.thread_anchor();
        // SAFETY: the anchor and the access stay borrowed for 's.
        unsafe { Self::from_raw((access as *mut A).cast(), dispatch::<A>, anchor) }
    }
}
impl<F: ReceiverFamily> ReceiverAccess for ReceiverSlot<'_, F> {
    type Family = F;
    fn poll_scope(&mut self, cx: &mut Context<'_>, request: &mut dyn ScopeRequest<F>) {
        assert!(
            self.thread_anchor().is_current(),
            "receiver slot used outside the poll of its call"
        );
        // SAFETY: from_raw's contract, checked by the anchor above.
        unsafe { (self.dispatch)(self.target, cx, request) }
    }
    fn thread_anchor(&self) -> &ThreadAnchor {
        // SAFETY: from_raw requires the anchor to outlive the slot.
        unsafe { &*self.anchor }
    }
}
