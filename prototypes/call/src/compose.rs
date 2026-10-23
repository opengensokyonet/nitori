#![allow(clippy::multiple_bound_locations)] // pin-project-lite projection bounds
//! Scoped adaptation of the current view into another family.
use crate::{
    __private::{ScopeRequest, ThreadAnchor},
    ReceiverAccess, ReceiverFamily, ReceiverScope,
};
use std::{
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
impl<A: ReceiverAccess, S: Compose<A::Family> + Unpin> ReceiverAccess for Composed<'_, A, S> {
    type Family = S::Family;
    fn poll_scope(&mut self, cx: &mut Context<'_>, request: &mut dyn ScopeRequest<S::Family>) {
        let mut adapt = Adapt {
            state: Pin::new(&mut self.state),
            inner: request,
        };
        self.parent.poll_scope(cx, &mut adapt);
    }
    fn thread_anchor(&self) -> &ThreadAnchor {
        self.parent.thread_anchor()
    }
}

struct Adapt<'q, S, G> {
    state: Pin<&'q mut S>,
    inner: &'q mut dyn ScopeRequest<G>,
}
impl<F: ReceiverFamily, S: Compose<F>> ScopeRequest<F> for Adapt<'_, S, S::Family> {
    fn run<'view>(
        &mut self,
        parent: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'view,
    {
        let scope = pin!(ComposeScope {
            parent: Some(parent),
            state: Some(self.state.as_mut()),
            view: None
        });
        self.inner.run(scope, cx);
    }
}

pin_project_lite::pin_project! {
    struct ComposeScope<'s, 'p: 's, F: ReceiverFamily, S> where F: 'p, S: Compose<F>, S::Family: 's {
        parent: Option<Pin<&'s mut dyn ReceiverScope<'p, Family = F>>>,
        state: Option<Pin<&'s mut S>>,
        #[pin]
        view: Option<<S::Family as ReceiverFamily>::ReceiverView<'s>>,
    }
}
impl<'s, 'p, F: ReceiverFamily + 'p, S: Compose<F>> ReceiverScope<'s> for ComposeScope<'s, 'p, F, S>
where
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
