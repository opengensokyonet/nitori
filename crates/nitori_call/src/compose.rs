//! Scoped adaptation of the current view into another family.
use crate::{
    ReceiverAccess, ReceiverFamily, ViewOperation,
    access::{ReleasePoint, Sealed, ViewRequest, ViewVisitor},
};
use std::{
    marker::PhantomData,
    ops::DerefMut,
    pin::{Pin, pin},
};

/// One projection; the parent view keeps its original inner lifetime.
pub trait Compose<F: ReceiverFamily> {
    type Family: ReceiverFamily;
    fn compose<'scope, 'parent>(
        self: Pin<&'scope mut Self>,
        parent: Pin<&'scope mut F::ReceiverView<'parent>>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'scope>
    where
        F: 'parent,
        Self::Family: 'scope,
        'parent: 'scope;
}
impl<F: ReceiverFamily, S: Compose<F> + Unpin + ?Sized> Compose<F> for &mut S {
    type Family = S::Family;
    fn compose<'s, 'p>(
        self: Pin<&'s mut Self>,
        parent: Pin<&'s mut F::ReceiverView<'p>>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'s>
    where
        F: 'p,
        Self::Family: 's,
        'p: 's,
    {
        Pin::new(&mut **self.get_mut()).compose(parent)
    }
}
impl<F: ReceiverFamily, P: DerefMut + Unpin> Compose<F> for Pin<P>
where
    P::Target: Compose<F>,
{
    type Family = <P::Target as Compose<F>>::Family;
    fn compose<'s, 'p>(
        self: Pin<&'s mut Self>,
        parent: Pin<&'s mut F::ReceiverView<'p>>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'s>
    where
        F: 'p,
        Self::Family: 's,
        'p: 's,
    {
        self.get_mut().as_mut().compose(parent)
    }
}

/// An access adapted by composition state. It borrows its parent mutably,
/// so the parent is unavailable while the adapted access exists.
#[derive(Debug)]
pub struct Composed<'p, A: ?Sized, S> {
    parent: &'p mut A,
    state: S,
}
impl<'p, A: ?Sized, S> Composed<'p, A, S> {
    pub(crate) fn new(parent: &'p mut A, state: S) -> Self {
        Self { parent, state }
    }
    /// Recover the composition state and end the parent borrow.
    pub fn into_state(self) -> S {
        self.state
    }
}
impl<A: ReceiverAccess + ?Sized, S: Compose<A::Family> + Unpin> Sealed for Composed<'_, A, S> {}
#[allow(private_bounds)]
impl<A: ReceiverAccess + ?Sized, S: Compose<A::Family> + Unpin> ReceiverAccess
    for Composed<'_, A, S>
{
    type Family = S::Family;
    fn visit_view<V: ViewVisitor<S::Family>>(&mut self, visitor: V) -> V::Output {
        self.parent.visit_view(ComposeChild {
            state: Pin::new(&mut self.state),
            inner: visitor,
            family: PhantomData,
        })
    }
    fn release_point(&mut self) -> ReleasePoint<'_> {
        self.parent.release_point()
    }
}

/// Builds the child view from the parent view before visiting it.
struct ComposeChild<'q, F, S, V> {
    state: Pin<&'q mut S>,
    inner: V,
    family: PhantomData<fn(F) -> F>,
}
impl<F: ReceiverFamily, S: Compose<F>, V: ViewVisitor<S::Family>> ViewOperation<F>
    for ComposeChild<'_, F, S, V>
{
    type Output = V::Output;
    fn run<'h>(self, parent: Pin<&mut F::ReceiverView<'h>>) -> V::Output
    where
        F: 'h,
    {
        let child = pin!(self.state.compose(parent));
        self.inner.run(child)
    }
}
impl<F: ReceiverFamily, S: Compose<F>, V: ViewVisitor<S::Family>> ViewVisitor<F>
    for ComposeChild<'_, F, S, V>
{
    fn absent(self, request: ViewRequest<'_>) -> V::Output {
        self.inner.absent(request)
    }
}
