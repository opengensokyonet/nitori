#![allow(dead_code)]

use std::cell::Cell;
use std::marker::{PhantomData, PhantomPinned};
use std::pin::Pin;
use std::task::{Context, Poll};

// Kept independent of the crate so compile failures isolate the lending contract.
pub trait HasFamily {
    type Family: HostFamily;
}

pub trait HostFamily: Sized {
    type HostView<'view>: HasFamily<Family = Self>
    where
        Self: 'view;
}

pub trait HostContext<'view> {
    type Family: HostFamily + 'view;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()>;

    fn ready_view<'access>(
        self: Pin<&'access mut Self>,
    ) -> Pin<&'access mut <Self::Family as HostFamily>::HostView<'view>>;

    fn poll_view<'access>(
        mut self: Pin<&'access mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Pin<&'access mut <Self::Family as HostFamily>::HostView<'view>>> {
        std::task::ready!(self.as_mut().poll_ready(cx));
        Poll::Ready(self.ready_view())
    }
}

pub struct BorrowedFamily<'owner>(PhantomData<&'owner str>);

pub struct View<'view, 'owner> {
    pub text: &'view &'owner str,
    pub visits: Cell<usize>,
    invariant: PhantomData<fn(&'view ()) -> &'view ()>,
    pinned: PhantomPinned,
}

impl<'view, 'owner> View<'view, 'owner> {
    pub fn new(text: &'view &'owner str) -> Self {
        Self {
            text,
            visits: Cell::new(0),
            invariant: PhantomData,
            pinned: PhantomPinned,
        }
    }
}

impl<'owner> HasFamily for View<'_, 'owner> {
    type Family = BorrowedFamily<'owner>;
}

impl<'owner> HostFamily for BorrowedFamily<'owner> {
    type HostView<'view>
        = View<'view, 'owner>
    where
        Self: 'view;
}

pub struct StoredContext<'view, 'owner> {
    view: Pin<Box<View<'view, 'owner>>>,
}

impl<'view, 'owner> StoredContext<'view, 'owner> {
    pub fn new(view: View<'view, 'owner>) -> Self {
        Self {
            view: Box::pin(view),
        }
    }
}

impl<'view, 'owner: 'view> HostContext<'view> for StoredContext<'view, 'owner> {
    type Family = BorrowedFamily<'owner>;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }

    fn ready_view<'access>(self: Pin<&'access mut Self>) -> Pin<&'access mut View<'view, 'owner>> {
        self.get_mut().view.as_mut()
    }
}
