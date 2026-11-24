//! What the driver keeps of a call and lends with it: the priority its
//! waker is handled at, and the wait for the requests it put in.
use alloc::sync::Arc;
use core::{
    cell::Cell,
    future::Future,
    pin::Pin,
    sync::atomic::Ordering::SeqCst,
    task::{Context, Poll},
};

use crate::driver::NodeHead;

/// The driver's side of one call: the priority its standard waker is
/// handled at, and the priority it settles at, when it asked to.
///
/// Not reachable from outside the crate; `pub` because the pack names it.
pub struct CallHost<P> {
    wake_priority: Cell<P>,
    settle: Cell<Option<P>>,
    head: Arc<NodeHead>,
    bit: u64,
}
impl<P: Copy> Clone for CallHost<P> {
    fn clone(&self) -> Self {
        Self {
            wake_priority: Cell::new(self.wake_priority.get()),
            settle: Cell::new(self.settle.get()),
            head: self.head.clone(),
            bit: self.bit,
        }
    }
}
impl<P: Copy> CallHost<P> {
    /// The call at the bit `bit` of the node of `head`, its waker handled
    /// at `p` until it sets another priority.
    pub(crate) fn new(p: P, head: Arc<NodeHead>, bit: u64) -> Self {
        Self {
            wake_priority: Cell::new(p),
            settle: Cell::new(None),
            head,
            bit,
        }
    }
    /// The priority its standard waker is handled at.
    pub(crate) fn wake_priority(&self) -> P {
        self.wake_priority.get()
    }
    /// Set the priority its standard waker is handled at.
    pub(crate) fn set_wake_priority(&self, p: P) {
        self.wake_priority.set(p);
    }
    /// Ask to settle at `p`; the wait that the driver sees once it is
    /// polled.
    pub(crate) fn settle(&self, p: P) -> Settle {
        self.settle.set(Some(p));
        Settle {
            head: self.head.clone(),
            bit: self.bit,
            polled: false,
        }
    }
    /// The priority it settles at, when it waited in a [`Settle`] since
    /// the driver last looked; the wait is then seen.
    pub(crate) fn take_settle(&self) -> Option<P> {
        if self.head.settles.fetch_and(!self.bit, SeqCst) & self.bit == 0 {
            return None;
        }
        // The last priority asked for: every wait of the call settles at
        // it, whichever of them is awaited.
        self.settle.get()
    }
}

/// The wait of a call for the requests it put in:
/// [`Loan::settle`](super::Loan::settle).
///
/// The first poll gives way; the driver handles every request put in so
/// far, then ranks the call at the priority it settles at, and the second
/// poll is ready. The requests are handled first; after them, what runs
/// next is decided by priority only, so a call that settles at a high
/// priority runs before calls of lower priority still waiting, and one that
/// settles at a low priority waits for them.
#[must_use = "a call settles only when it awaits the wait"]
pub struct Settle {
    head: Arc<NodeHead>,
    bit: u64,
    polled: bool,
}
impl Future for Settle {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if self.polled {
            return Poll::Ready(());
        }
        self.polled = true;
        self.head.settles.fetch_or(self.bit, SeqCst);
        Poll::Pending
    }
}
