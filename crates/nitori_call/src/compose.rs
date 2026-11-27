//! Scoped adaptation of the current view into another family.
use crate::{
    AccessPlan, ProvideAccess, ReceiverAccess, ReceiverFamily, ViewLoan, ViewOperation,
    access::{PlanSealed, ReleasePoint, ViewRequest, ViewVisitor},
    loan::Lent,
};
use core::{
    marker::PhantomData,
    pin::{Pin, pin},
};

// Public in a private module so associated types in public impls can use it.
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

/// A plan that projects a loan of the parent view.
pub(crate) struct Composed<A, S> {
    parent: A,
    state: S,
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

/// Build a child view from a parent view and persistent state.
pub trait ViewProjection<F: ReceiverFamily> {
    /// State borrowed for the lifetime of the child view.
    type State;
    /// The family of the child view.
    type Family: ReceiverFamily;

    /// Project the view using the current state.
    fn project<'v>(
        view: F::ReceiverView<'v>,
        state: &'v mut Self::State,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v;
}

/// State held for a [`ViewProjection`] used by [`drive_composed`](crate::drive_composed).
pub struct ProjectionState<P, S> {
    state: S,
    projection: PhantomData<fn() -> P>,
}

impl<P, S: core::fmt::Debug> core::fmt::Debug for ProjectionState<P, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProjectionState")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<P, S> ProjectionState<P, S> {
    /// Project with `P` using the given state.
    pub const fn new(state: S) -> Self {
        Self {
            state,
            projection: PhantomData,
        }
    }
}

impl<F, P, S> Compose<F> for ProjectionState<P, S>
where
    F: ReceiverFamily,
    P: ViewProjection<F, State = S>,
    S: Unpin,
{
    type Family = P::Family;

    fn compose<'v>(
        self: Pin<&'v mut Self>,
        view: F::ReceiverView<'v>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v,
    {
        P::project(view, &mut self.get_mut().state)
    }
}

struct BorrowedProjectionState<'a, P, S>(&'a mut S, PhantomData<fn() -> P>);

impl<F, P, S> Compose<F> for BorrowedProjectionState<'_, P, S>
where
    F: ReceiverFamily,
    P: ViewProjection<F, State = S>,
{
    type Family = P::Family;

    fn compose<'v>(
        self: Pin<&'v mut Self>,
        view: F::ReceiverView<'v>,
    ) -> <Self::Family as ReceiverFamily>::ReceiverView<'v>
    where
        F: 'v,
        Self::Family: 'v,
    {
        P::project(view, self.get_mut().0)
    }
}

/// Issue a plan for a projected view of the parent access.
///
/// The plan borrows the parent and the state exclusively and inherits the
/// parent's [`Lentness`](crate::Lentness).
pub fn project_access<'a, A, P>(
    parent: &'a mut A,
    state: &'a mut P::State,
) -> impl AccessPlan<'a, Family = P::Family, Lentness = <A as ProvideAccess>::Lentness>
where
    A: ReceiverAccess,
    P: ViewProjection<Lent<A::Family>>,
{
    Composed {
        parent,
        state: BorrowedProjectionState::<P, _>(state, PhantomData),
    }
}

impl<A: ReceiverAccess, S: Compose<Lent<A::Family>> + Unpin> PlanSealed for Composed<&mut A, S> {}

impl<'a, A: ReceiverAccess, S: Compose<Lent<A::Family>> + Unpin> AccessPlan<'a>
    for Composed<&'a mut A, S>
{
    type Family = S::Family;
    type Lentness = <A as ProvideAccess>::Lentness;

    fn visit<V: ViewVisitor<S::Family>>(self, visitor: V) -> V::Output {
        let mut state = self.state;
        self.parent.visit_view(ComposeChild {
            state: Pin::new(&mut state),
            inner: visitor,
            family: PhantomData,
        })
    }

    fn release(self) -> ReleasePoint<'a> {
        self.parent.release_point()
    }

    /// The parent tracks the values of its projection.
    fn track(self, event: crate::tracking::ValueEvent) {
        crate::tracking::report(self.parent, event);
    }
}
