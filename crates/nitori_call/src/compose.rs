//! Scoped adaptation of the current view into another family.
use crate::{
    AsAccess, LentAccess, Receiver, ReceiverAccess, ReceiverFamily, ViewLoan, ViewOperation,
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

/// An access adapted by composition state. It holds its parent by value:
/// [`ReceiverAccess::compose`] gives it a `&mut` borrow of the parent, so
/// the parent is unavailable while the adapted access exists, and
/// [`Composed::new`] takes whatever access it is given, such as one passed
/// into a function that returns the adapted access. The parent can also be
/// any [`AsAccess`] source, so a type that keeps a source and the state
/// keeps them as one `Composed` and lends it. On each access the state
/// composes on a [`ViewLoan`] of the parent's view, a view of [`Lent`], so
/// the child view ends with that access; what must last longer is kept in the
/// state.
#[derive(Debug)]
pub struct Composed<A, S> {
    parent: A,
    state: S,
}
impl<A, S> Composed<A, S> {
    /// Adapt the access `parent`, or the access it lends, by `state`. The state composes on [`Lent`]
    /// views of the parent's family; pass `pin!(state)` for state that is not
    /// `Unpin`.
    pub const fn new(parent: A, state: S) -> Self {
        Self { parent, state }
    }
    /// The parent.
    pub const fn parent(&self) -> &A {
        &self.parent
    }
    /// The parent, to use it directly, without the state.
    pub const fn parent_mut(&mut self) -> &mut A {
        &mut self.parent
    }
    /// The composition state.
    pub const fn state(&self) -> &S {
        &self.state
    }
    /// The composition state, to change it between accesses.
    pub const fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }
    /// Recover the composition state, dropping the parent access; when that
    /// is a `&mut` borrow, the borrow ends.
    pub fn into_state(self) -> S {
        self.state
    }
    /// End the adaptation, returning the parent access and the state.
    pub fn into_parts(self) -> (A, S) {
        (self.parent, self.state)
    }
}
impl<A: AsAccess, S: Compose<Lent<A::Family>> + Unpin> Sealed for Composed<A, S> {}
#[allow(private_bounds)]
impl<A: AsAccess, S: Compose<Lent<A::Family>> + Unpin> ReceiverAccess for Composed<A, S> {
    type Family = S::Family;
    fn visit_view<V: ViewVisitor<S::Family>>(&mut self, visitor: V) -> V::Output {
        self.parent.access().visit_view(ComposeChild {
            state: Pin::new(&mut self.state),
            inner: visitor,
            family: PhantomData,
        })
    }
    fn release_point(&mut self) -> ReleasePoint<'_> {
        self.parent.access().into_release_point()
    }
}
impl<A: AsAccess<Access: LentAccess>, S: Compose<Lent<A::Family>> + Unpin> LentAccess
    for Composed<A, S>
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
