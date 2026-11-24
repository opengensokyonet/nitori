//! Scoped adaptation of the current view into another family.
use crate::{
    LentAccess, Receiver, ReceiverAccess, ReceiverFamily, ViewLoan, ViewOperation,
    access::{ReleasePoint, Sealed, ViewRequest, ViewVisitor},
    loan::Lent,
};
use core::{
    marker::PhantomData,
    ops::DerefMut,
    pin::{Pin, pin},
};

/// Composition state: it adapts a view of `F` into a view of its own family.
///
/// The parent view is passed by value, and the child view may keep it, with
/// the state's borrow, for `'v`. Where the parent view is only borrowed for
/// one access, as in [`ReceiverAccess::compose`] and
/// [`drive_composed`](crate::drive_composed), the state composes on
/// [`Lent<F>`](Lent), whose view is a [`ViewLoan`] of the parent's view; on
/// a [`ComposedReceiver`], it composes on the inner receiver's family itself.
/// A state written for every `F` serves both.
pub trait Compose<F: ReceiverFamily> {
    type Family: ReceiverFamily;
    fn compose<'v>(
        self: Pin<&'v mut Self>,
        parent: F::ReceiverView<'v>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v;
}
impl<F: ReceiverFamily, S: Compose<F> + Unpin + ?Sized> Compose<F> for &mut S {
    type Family = S::Family;
    fn compose<'v>(
        self: Pin<&'v mut Self>,
        parent: F::ReceiverView<'v>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v,
    {
        Pin::new(&mut **self.get_mut()).compose(parent)
    }
}
impl<F: ReceiverFamily, P: DerefMut + Unpin> Compose<F> for Pin<P>
where
    P::Target: Compose<F>,
{
    type Family = <P::Target as Compose<F>>::Family;
    fn compose<'v>(
        self: Pin<&'v mut Self>,
        parent: F::ReceiverView<'v>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v,
    {
        self.get_mut().as_mut().compose(parent)
    }
}

/// An access adapted by composition state. It borrows its parent mutably,
/// so the parent is unavailable while the adapted access exists. On each
/// access the state composes on a [`ViewLoan`] of the parent's view, a view
/// of [`Lent`], so the child view ends with that access; what must last
/// longer is kept in the state.
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
impl<A: ReceiverAccess + ?Sized, S: Compose<Lent<A::Family>> + Unpin> Sealed
    for Composed<'_, A, S>
{
}
#[allow(private_bounds)]
impl<A: ReceiverAccess + ?Sized, S: Compose<Lent<A::Family>> + Unpin> ReceiverAccess
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
impl<A: LentAccess + ?Sized, S: Compose<Lent<A::Family>> + Unpin> LentAccess
    for Composed<'_, A, S>
{
}

/// Builds the child view from a loan of the parent view before visiting it.
struct ComposeChild<'q, F, S, V> {
    state: Pin<&'q mut S>,
    inner: V,
    family: PhantomData<fn(F) -> F>,
}
impl<F: ReceiverFamily, S: Compose<Lent<F>>, V: ViewVisitor<S::Family>> ViewOperation<F>
    for ComposeChild<'_, F, S, V>
{
    type Output = V::Output;
    fn run<'h>(self, parent: Pin<&mut F::ReceiverView<'h>>) -> V::Output
    where
        F: 'h,
    {
        let mut state = self.state;
        let child = pin!(state.as_mut().compose(ViewLoan::new(parent)));
        self.inner.run(child)
    }
}
impl<F: ReceiverFamily, S: Compose<Lent<F>>, V: ViewVisitor<S::Family>> ViewVisitor<F>
    for ComposeChild<'_, F, S, V>
{
    fn absent(self, request: ViewRequest<'_>) -> V::Output {
        self.inner.absent(request)
    }
}

pin_project_lite::pin_project! {
    /// A receiver adapted by composition state that lives as long as the
    /// receiver.
    ///
    /// Its view is the state composed on the inner receiver's view, which the
    /// state takes by value. Unlike the state of [`Composed`], which an
    /// operation holds, this state belongs to the receiver: it survives every
    /// call on the receiver, a call dropped midway included, and stays
    /// consistent with the inner receiver as long as the state records each
    /// poll's progress in that poll. Operations on it can compose again, on
    /// [`Lent`] views of its family.
    #[derive(Debug)]
    pub struct ComposedReceiver<T, S> {
        #[pin]
        inner: T,
        #[pin]
        state: S,
    }
}
impl<T, S> ComposedReceiver<T, S> {
    /// Adapt `inner` by `state`.
    pub fn new(inner: T, state: S) -> Self {
        Self { inner, state }
    }
    /// The inner receiver and the state.
    pub fn get_ref(&self) -> (&T, &S) {
        (&self.inner, &self.state)
    }
    /// End the adaptation, returning the inner receiver and the state.
    pub fn into_parts(self) -> (T, S) {
        (self.inner, self.state)
    }
}
impl<T: Receiver, S: Compose<T::Family>> Receiver for ComposedReceiver<T, S> {
    type Family = S::Family;
    fn view<'v>(self: Pin<&'v mut Self>) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        Self::Family: 'v,
    {
        let this = self.project();
        this.state.compose(this.inner.view())
    }
}
