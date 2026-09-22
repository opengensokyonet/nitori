#![allow(clippy::multiple_bound_locations)] // pin-project-lite generated projections

use crate::{CallOn, HostContext, HostFamily};
use std::{
    ops::CoroutineState,
    pin::{Pin, pin},
    task::{Context, Poll, ready},
};

/// Build a target view borrowing a pinned parent view and receiver state.
///
/// The implementation chooses its own target representation. It need not, and
/// cannot in general, shorten an invariant parent view's inner lifetime or take
/// ownership of its guard. Identity receivers forward the context instead.
pub trait Projection {
    type Root: HostFamily;
    type Target: HostFamily;

    fn project<'scope, 'view>(
        self: Pin<&'scope mut Self>,
        root: Pin<&'scope mut <Self::Root as HostFamily>::HostView<'view>>,
    ) -> <Self::Target as HostFamily>::HostView<'scope>
    where
        Self::Root: 'view,
        Self::Target: 'scope,
        'view: 'scope;
}

impl<R: Projection + Unpin + ?Sized> Projection for &mut R {
    type Root = R::Root;
    type Target = R::Target;

    fn project<'scope, 'view>(
        self: Pin<&'scope mut Self>,
        root: Pin<&'scope mut <R::Root as HostFamily>::HostView<'view>>,
    ) -> <R::Target as HostFamily>::HostView<'scope>
    where
        R::Root: 'view,
        R::Target: 'scope,
        'view: 'scope,
    {
        Pin::new(&mut **self.get_mut()).project(root)
    }
}

impl<R: Projection + ?Sized> Projection for Pin<&mut R> {
    type Root = R::Root;
    type Target = R::Target;

    fn project<'scope, 'view>(
        self: Pin<&'scope mut Self>,
        root: Pin<&'scope mut <R::Root as HostFamily>::HostView<'view>>,
    ) -> <R::Target as HostFamily>::HostView<'scope>
    where
        R::Root: 'view,
        R::Target: 'scope,
        'view: 'scope,
    {
        self.get_mut().as_mut().project(root)
    }
}

pin_project_lite::pin_project! {
    /// One lazy receiver drive, borrowing parent context and receiver externally.
    ///
    /// Before readiness, the two loans are retained for retry. After readiness,
    /// they are consumed into the target view, which is initialized and pinned
    /// once. No field borrows another field of this context.
    pub struct ProjectedContext<'scope, 'view: 'scope, R: crate::Projection>
    where
        R::Root: 'view,
        R::Target: 'scope,
    {
        parent: Option<Pin<&'scope mut dyn HostContext<'view, Family = R::Root>>>,
        receiver: Option<Pin<&'scope mut R>>,
        #[pin]
        view: Option<<R::Target as HostFamily>::HostView<'scope>>,
    }
}

impl<'scope, 'view: 'scope, R: Projection> ProjectedContext<'scope, 'view, R>
where
    R::Root: 'view,
    R::Target: 'scope,
{
    pub fn new(
        parent: Pin<&'scope mut dyn HostContext<'view, Family = R::Root>>,
        receiver: Pin<&'scope mut R>,
    ) -> Self {
        Self {
            parent: Some(parent),
            receiver: Some(receiver),
            view: None,
        }
    }
}

impl<'scope, 'view: 'scope, R: Projection> HostContext<'scope>
    for ProjectedContext<'scope, 'view, R>
where
    R::Root: 'view,
    R::Target: 'scope,
{
    type Family = R::Target;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut this = self.project();
        if this.view.as_ref().get_ref().is_some() {
            return Poll::Ready(());
        }
        // A temporary reborrow only checks readiness. Pending keeps both full
        // loans available, so the next attempt can still borrow for 'scope.
        ready!(
            this.parent
                .as_mut()
                .expect("projection used after initialization panicked")
                .as_mut()
                .poll_ready(cx)
        );
        // Taking the original Pin references preserves their 'scope lifetime.
        // ready_view is only a loan of an existing view, not reconstruction.
        let parent = this.parent.take().unwrap();
        let receiver = this.receiver.take().unwrap();
        this.view.set(Some(receiver.project(parent.ready_view())));
        Poll::Ready(())
    }

    fn ready_view<'access>(
        self: Pin<&'access mut Self>,
    ) -> Pin<&'access mut <R::Target as HostFamily>::HostView<'scope>> {
        self.project().view.as_pin_mut().expect("view is not ready")
    }
}

pin_project_lite::pin_project! {
    /// Drive an operation through a receiver whose state may be coroutine-local.
    ///
    /// Each poll_call/poll_return owns a projection scope. Internally discarded
    /// yields stay in that scope; returning an event, Pending, completion or
    /// unwinding drops it before receiver state becomes accessible again.
    pub struct ReceivedCall<R, O> {
        #[pin]
        receiver: R,
        #[pin]
        operation: O,
        terminal: bool,
    }
}

impl<R, O> ReceivedCall<R, O> {
    pub fn new(receiver: R, operation: O) -> Self {
        Self {
            receiver,
            operation,
            terminal: false,
        }
    }
}

impl<R: Projection, O: CallOn<R::Target>> CallOn<R::Root> for ReceivedCall<R, O> {
    type Yield = O::Yield;
    type Return = O::Return;

    fn poll_call<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = R::Root>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        R::Root: 'view,
    {
        let this = self.project();
        assert!(
            !*this.terminal,
            "received call polled after completion or panic"
        );
        *this.terminal = true;
        let result = {
            let mut projected = pin!(ProjectedContext::new(context, this.receiver));
            this.operation.poll_call(projected.as_mut(), cx)
        };
        // Reset only after the view was successfully destroyed.
        if !matches!(&result, Poll::Ready(CoroutineState::Complete(_))) {
            *this.terminal = false;
        }
        result
    }

    fn poll_return<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = R::Root>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Return>
    where
        R::Root: 'view,
    {
        let this = self.project();
        assert!(
            !*this.terminal,
            "received call polled after completion or panic"
        );
        *this.terminal = true;
        let result = {
            let mut projected = pin!(ProjectedContext::new(context, this.receiver));
            this.operation.poll_return(projected.as_mut(), cx)
        };
        if result.is_pending() {
            *this.terminal = false;
        }
        result
    }
}
