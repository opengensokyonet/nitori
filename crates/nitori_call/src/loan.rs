use crate::{Access, ReceiverFamily};
use std::{marker::PhantomData, pin::Pin};
// The callback quantifies only a lifetime. F remains an exact, fixed type.
trait Visitor<F: ReceiverFamily> {
    fn visit<'parent>(&mut self, view: Pin<&mut F::ReceiverView<'parent>>)
    where
        F: 'parent;
}

/// A synchronous operation whose result cannot borrow the temporary view access.
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
struct Request<F: ReceiverFamily, A: ViewOperation<F>> {
    operation: Option<A>,
    output: Option<A::Output>,
    family: PhantomData<fn(F) -> F>,
}
impl<F: ReceiverFamily, A: ViewOperation<F>> Visitor<F> for Request<F, A> {
    fn visit<'parent>(&mut self, view: Pin<&mut F::ReceiverView<'parent>>)
    where
        F: 'parent,
    {
        self.output = Some(self.operation.take().unwrap().run(view));
    }
}

/// An exclusive pinned view loan that hides its parent's inner lifetime.
///
/// The exact family remains invariant. No capability registry, allocation or
/// reconstruction is needed. Access is scoped to `with` or `apply`; references
/// into that temporary access cannot escape. This type is neither Send nor Sync.
pub struct ViewLoan<'scope, F: ReceiverFamily> {
    pointer: *mut (),
    dispatch: unsafe fn(*mut (), &mut dyn Visitor<F>),
    borrow: PhantomData<&'scope mut ()>,
    family: PhantomData<fn(F) -> F>,
}
struct Dispatch<'parent, F: ReceiverFamily + 'parent>(PhantomData<&'parent F>);
impl<'parent, F: ReceiverFamily + 'parent> Dispatch<'parent, F> {
    unsafe fn visit(pointer: *mut (), visitor: &mut dyn Visitor<F>) {
        // SAFETY: new pairs this function with the exact F::ReceiverView<'parent>
        // pointer. ViewLoan's exclusive borrow is live for the whole visit.
        // The pointee remains pinned and its inner lifetime is not substituted.
        let view = unsafe { Pin::new_unchecked(&mut *pointer.cast::<F::ReceiverView<'parent>>()) };
        visitor.visit(view);
    }
}
impl<'scope, F: ReceiverFamily + 'scope> ViewLoan<'scope, F> {
    pub fn new<'parent: 'scope>(view: Pin<&'scope mut F::ReceiverView<'parent>>) -> Self
    where
        F: 'parent,
    {
        // SAFETY: obtaining a raw pointer does not move the pinned pointee.
        // Only the matched dispatcher can use it and recreates a pinned borrow.
        let pointer = unsafe { view.get_unchecked_mut() as *mut F::ReceiverView<'parent> }.cast();
        Self {
            pointer,
            dispatch: Dispatch::<'parent, F>::visit,
            borrow: PhantomData,
            family: PhantomData,
        }
    }
    pub fn apply<A: ViewOperation<F>>(&mut self, operation: A) -> A::Output {
        let mut request = Request::<F, A> {
            operation: Some(operation),
            output: None,
            family: PhantomData,
        };
        // SAFETY: the lifetime marker keeps the original pinned loan alive.
        // &mut self excludes every other call through this wrapper. Private
        // fields guarantee that the function and pointer remain correctly paired.
        unsafe { (self.dispatch)(self.pointer, &mut request) };
        request.output.unwrap()
    }
    pub fn with<B, R>(&mut self, body: B) -> R
    where
        B: for<'access, 'parent> FnOnce(Access<'access, 'parent, F>) -> R,
    {
        self.apply(ClosureOperation(body))
    }
}
