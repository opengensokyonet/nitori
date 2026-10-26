//! Pinned views whose inner lifetime is hidden behind a visit.
use crate::{Access, ReceiverFamily};
use std::{fmt, marker::PhantomData, pin::Pin};

/// A synchronous operation on a view. Its result cannot borrow the view.
pub trait ViewOperation<F: ReceiverFamily> {
    type Output;
    fn run<'parent>(self, view: Pin<&mut F::ReceiverView<'parent>>) -> Self::Output
    where
        F: 'parent;
}
struct ClosureOperation<B>(B);
impl<F: ReceiverFamily, B, R> ViewOperation<F> for ClosureOperation<B>
where
    B: for<'access, 'parent> FnOnce(Access<'access, 'parent, F>) -> R,
{
    type Output = R;
    fn run<'parent>(self, view: Pin<&mut F::ReceiverView<'parent>>) -> R
    where
        F: 'parent,
    {
        (self.0)(Access::new(view))
    }
}

/// The dyn form of a view operation. It quantifies only a lifetime, so the
/// family stays an exact type.
trait DynOperation<F: ReceiverFamily> {
    fn run<'parent>(&mut self, view: Pin<&mut F::ReceiverView<'parent>>)
    where
        F: 'parent;
}
/// Runs a view operation once through its dyn form.
struct Once<F: ReceiverFamily, A: ViewOperation<F>> {
    operation: Option<A>,
    output: Option<A::Output>,
    family: PhantomData<fn(F) -> F>,
}
impl<F: ReceiverFamily, A: ViewOperation<F>> DynOperation<F> for Once<F, A> {
    fn run<'parent>(&mut self, view: Pin<&mut F::ReceiverView<'parent>>)
    where
        F: 'parent,
    {
        let operation = self.operation.take().expect("view operation ran twice");
        self.output = Some(operation.run(view));
    }
}

/// A pinned view and the function that restores its exact type. Copying it
/// does not copy the borrow: its users keep the view live and exclusive.
pub(crate) struct RawViewLoan<F: ReceiverFamily> {
    view: *mut (),
    restore: unsafe fn(*mut (), &mut dyn DynOperation<F>),
}
impl<F: ReceiverFamily> Clone for RawViewLoan<F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F: ReceiverFamily> Copy for RawViewLoan<F> {}
/// Carries the view's inner lifetime to the restoring function.
struct Restore<'parent, F: ReceiverFamily + 'parent>(PhantomData<&'parent F>);
impl<'parent, F: ReceiverFamily + 'parent> Restore<'parent, F> {
    unsafe fn run(view: *mut (), operation: &mut dyn DynOperation<F>) {
        // SAFETY: RawViewLoan::new pairs this function with a pointer to
        // exactly F::ReceiverView<'parent>, and RawViewLoan::apply's caller
        // keeps that borrow live and exclusive. The pointee stays pinned.
        let view = unsafe { Pin::new_unchecked(&mut *view.cast::<F::ReceiverView<'parent>>()) };
        operation.run(view);
    }
}
impl<F: ReceiverFamily> RawViewLoan<F> {
    pub(crate) fn new<'parent>(view: Pin<&mut F::ReceiverView<'parent>>) -> Self
    where
        F: 'parent,
    {
        // SAFETY: taking a raw pointer does not move the pinned pointee; only
        // the paired function uses it, and it recreates a pinned borrow.
        let view = unsafe { view.get_unchecked_mut() as *mut F::ReceiverView<'parent> }.cast();
        Self {
            view,
            restore: Restore::<'parent, F>::run,
        }
    }
    /// # Safety
    /// The view borrow given to `new` must be live, and no other use of it
    /// may be in progress.
    pub(crate) unsafe fn apply<A: ViewOperation<F>>(self, operation: A) -> A::Output {
        let mut once = Once::<F, A> {
            operation: Some(operation),
            output: None,
            family: PhantomData,
        };
        // SAFETY: the caller's contract; the function and pointer are paired.
        unsafe { (self.restore)(self.view, &mut once) };
        once.output.unwrap()
    }
}

/// An exclusive pinned view loan that hides its parent's inner lifetime.
///
/// The exact family remains invariant. No capability registry, allocation or
/// reconstruction is needed. Access is scoped to `with` or `apply`; references
/// into that temporary access cannot escape. This type is neither Send nor Sync.
pub struct ViewLoan<'scope, F: ReceiverFamily> {
    raw: RawViewLoan<F>,
    borrow: PhantomData<&'scope mut ()>,
}
impl<F: ReceiverFamily> fmt::Debug for ViewLoan<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewLoan").finish_non_exhaustive()
    }
}
impl<'scope, F: ReceiverFamily + 'scope> ViewLoan<'scope, F> {
    pub fn new<'parent: 'scope>(view: Pin<&'scope mut F::ReceiverView<'parent>>) -> Self
    where
        F: 'parent,
    {
        Self {
            raw: RawViewLoan::new(view),
            borrow: PhantomData,
        }
    }
    pub fn apply<A: ViewOperation<F>>(&mut self, operation: A) -> A::Output {
        // SAFETY: the lifetime marker keeps the original pinned loan alive,
        // and &mut self excludes every other use through this wrapper.
        unsafe { self.raw.apply(operation) }
    }
    pub fn with<B, R>(&mut self, body: B) -> R
    where
        B: for<'access, 'parent> FnOnce(Access<'access, 'parent, F>) -> R,
    {
        self.apply(ClosureOperation(body))
    }
}
