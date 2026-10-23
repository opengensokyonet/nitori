#![allow(clippy::multiple_bound_locations)] // pin-project-lite projection bounds
//! Scoped adaptation of the current view into another family.
use crate::{ReceiverAccess, ReceiverFamily, ReceiverScope, access::ScopeVisitor};
use std::{
    marker::PhantomData,
    ops::DerefMut,
    pin::{Pin, pin},
    task::{Context, Poll, ready},
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
impl<A: ReceiverAccess + ?Sized, S: Compose<A::Family> + Unpin> ReceiverAccess
    for Composed<'_, A, S>
{
    type Family = S::Family;
    type Mode = A::Mode;
    fn visit_scope<V: ScopeVisitor<S::Family>>(
        &mut self,
        cx: &mut Context<'_>,
        visitor: V,
    ) -> V::Output {
        self.parent.visit_scope(
            cx,
            ComposeVisitor {
                state: Pin::new(&mut self.state),
                inner: visitor,
                family: PhantomData,
            },
        )
    }
}

/// Wraps the parent's scope statically before handing it to the inner visitor.
struct ComposeVisitor<'q, F, S, V> {
    state: Pin<&'q mut S>,
    inner: V,
    family: PhantomData<fn(F) -> F>,
}
impl<F: ReceiverFamily, S: Compose<F>, V: ScopeVisitor<S::Family>> ScopeVisitor<F>
    for ComposeVisitor<'_, F, S, V>
{
    type Output = V::Output;
    fn visit<'view, P: ReceiverScope<'view, Family = F>>(
        self,
        parent: Pin<&mut P>,
        cx: &mut Context<'_>,
    ) -> V::Output
    where
        F: 'view,
    {
        let scope = pin!(ComposeScope {
            parent: Some(parent),
            state: Some(self.state),
            view: None,
            family: PhantomData,
        });
        self.inner.visit(scope, cx)
    }
}

pin_project_lite::pin_project! {
    struct ComposeScope<'s, 'p: 's, F, P, S>
    where
        F: ReceiverFamily,
        F: 'p,
        P: ReceiverScope<'p, Family = F>,
        S: Compose<F>,
        S::Family: 's,
    {
        parent: Option<Pin<&'s mut P>>,
        state: Option<Pin<&'s mut S>>,
        #[pin]
        view: Option<<S::Family as ReceiverFamily>::ReceiverView<'s>>,
        family: PhantomData<&'p F>,
    }
}
impl<'s, 'p, F, P, S> ReceiverScope<'s> for ComposeScope<'s, 'p, F, P, S>
where
    F: ReceiverFamily + 'p,
    P: ReceiverScope<'p, Family = F>,
    S: Compose<F>,
    S::Family: 's,
    'p: 's,
{
    type Family = S::Family;
    fn poll_view(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Pin<&mut <S::Family as ReceiverFamily>::ReceiverView<'s>>> {
        let mut this = self.project();
        if this.view.is_none() {
            ready!(
                this.parent
                    .as_mut()
                    .expect("composition initialization panicked")
                    .as_mut()
                    .poll_ready(cx)
            );
            let Poll::Ready(parent) = this.parent.take().unwrap().poll_view(cx) else {
                panic!("scope violated sticky readiness")
            };
            this.view
                .set(Some(this.state.take().unwrap().compose(parent)));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap())
    }
}
