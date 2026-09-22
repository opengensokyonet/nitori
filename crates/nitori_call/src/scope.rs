#![allow(clippy::multiple_bound_locations)] // pin-project-lite projection bounds
//! Lazy receiver access for one drive. Acquired views never survive this scope.
use crate::{Receiver, ReceiverFamily};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Lend one stable pinned view. Ready is sticky until this scope is destroyed.
/// Pending must arrange a wakeup; acquiring has no separate error channel.
pub trait ReceiverScope<'view> {
    type Family: ReceiverFamily + 'view;
    fn poll_view(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Pin<&mut <Self::Family as ReceiverFamily>::ReceiverView<'view>>>;
    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.poll_view(cx).map(drop)
    }
}
pin_project_lite::pin_project! {
    pub struct BorrowedScope<'view, R: ?Sized> where R: Receiver, R::Family: 'view {
        receiver: Option<Pin<&'view mut R>>,
        #[pin]
        view: Option<<R::Family as ReceiverFamily>::ReceiverView<'view>>,
    }
}
impl<'view, R: Receiver + ?Sized> BorrowedScope<'view, R>
where
    R::Family: 'view,
{
    pub fn new(receiver: Pin<&'view mut R>) -> Self {
        Self {
            receiver: Some(receiver),
            view: None,
        }
    }
}
impl<'view, R: Receiver + ?Sized> ReceiverScope<'view> for BorrowedScope<'view, R>
where
    R::Family: 'view,
{
    type Family = R::Family;
    fn poll_view(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Pin<&mut <R::Family as ReceiverFamily>::ReceiverView<'view>>> {
        let mut this = self.project();
        if this.view.is_none() {
            this.view.set(Some(
                this.receiver
                    .take()
                    .expect("view initialization panicked")
                    .view(),
            ));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap())
    }
}

/// Scope over an already pinned view, without reconstructing or owning it.
pub struct ViewScope<'access, 'view, F: ReceiverFamily + 'view> {
    view: Pin<&'access mut F::ReceiverView<'view>>,
}
impl<F: ReceiverFamily> Unpin for ViewScope<'_, '_, F> {}
impl<'access, 'view, F: ReceiverFamily + 'view> ViewScope<'access, 'view, F> {
    pub fn new(view: Pin<&'access mut F::ReceiverView<'view>>) -> Self {
        Self { view }
    }
}
impl<'view, F: ReceiverFamily + 'view> ReceiverScope<'view> for ViewScope<'_, 'view, F> {
    type Family = F;
    fn poll_view(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Pin<&mut F::ReceiverView<'view>>> {
        Poll::Ready(self.get_mut().view.as_mut())
    }
}
