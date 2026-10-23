//! Operations that use a resource lent by their caller for one poll at a time.
//!
//! A [`Call`] owns an operation and receives a [`ReceiverScope`] on every poll.
//! The operation is an ordinary future that reaches the current view through
//! its [`ReceiverSlot`]; the caller decides how long each view stays acquired.
//!
//! # Exclusive access
//!
//! A slot is borrowed mutably by each use, so steps run one after another:
//!
//! ```
//! # use nitori_call_prototype::{Direct, ReceiverAccess, ReceiverSlot};
//! # use std::{future::poll_fn, task::Poll};
//! async fn sequential(io: &mut ReceiverSlot<'_, Direct<Vec<u8>>>) {
//!     poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(()))).await;
//!     poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(()))).await;
//! }
//! ```
//!
//! Two concurrent uses of one slot are rejected; sharing must be explicit.
//!
//! ```compile_fail
//! # use nitori_call_prototype::{Direct, ReceiverAccess, ReceiverSlot};
//! # use std::{future::poll_fn, task::Poll};
//! async fn concurrent(io: &mut ReceiverSlot<'_, Direct<Vec<u8>>>) {
//!     let first = poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(())));
//!     let second = poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(())));
//!     let _ = (first, second);
//! }
//! ```
//!
//! # The brand
//!
//! An operation cannot return its slot:
//!
//! ```compile_fail
//! # use nitori_call_prototype::{Call, Direct, Emitter, ReceiverSlot};
//! type F = Direct<Vec<u8>>;
//! async fn leak<'s>(io: ReceiverSlot<'s, F>, _: Emitter<'s, ()>) -> ReceiverSlot<'s, F> {
//!     io
//! }
//! let _ = Call::new(leak);
//! ```
//!
//! nor store it in state that outlives the operation:
//!
//! ```compile_fail
//! # use nitori_call_prototype::{Call, Direct, Emitter, ReceiverSlot};
//! type F = Direct<Vec<u8>>;
//! async fn keep<'x>(
//!     io: ReceiverSlot<'x, F>,
//!     _: Emitter<'x, ()>,
//!     kept: &'x mut Option<ReceiverSlot<'x, F>>,
//! ) {
//!     *kept = Some(io);
//! }
//! fn store<'x>(kept: &'x mut Option<ReceiverSlot<'x, F>>) {
//!     let _ = Call::new((keep, kept));
//! }
//! ```
#![deny(unsafe_op_in_unsafe_fn)]

mod receiver;
pub use receiver::{
    BorrowedReceiver, Direct, DirectView, HasReceiverFamily, Receiver, ReceiverFamily,
};
mod scope;
pub use scope::{BorrowedScope, ReceiverScope, ViewScope};
mod loan;
pub use loan::{ViewLoan, ViewOperation};
mod access;
pub use access::{Access, ReceiverAccess, ReceiverSlot};
mod compose;
pub use compose::{Compose, Composed};
mod call;
pub use call::{Call, DynCall, Emitter, Operation};

/// Generate conveniences for an async function taking a receiver access first.
pub use nitori_call_prototype_macros::call;
/// Generate operations and conveniences from a capability trait's poll methods.
pub use nitori_call_prototype_macros::capability;

/// Support for implementing [`ReceiverAccess`] and for generated code; not a
/// stable interface.
#[doc(hidden)]
pub mod __private {
    pub use crate::access::{ScopeRequest, ThreadAnchor};
    use crate::{BorrowedScope, Call, Operation, Receiver};
    use std::{
        convert::Infallible,
        future::{Future, poll_fn},
        pin::{Pin, pin},
        task::{Context, Poll, Waker},
    };

    /// Run an operation on a host, lending a fresh lazy scope on each poll.
    pub async fn bind<H, R, Op>(host: Pin<&mut H>, operation: Op) -> R
    where
        H: Receiver + ?Sized,
        Op: for<'s> Operation<'s, H::Family, Infallible, R>,
    {
        let mut host = host;
        let mut call = pin!(Call::new(operation));
        poll_fn(|cx| {
            call.as_mut()
                .poll_return_with(pin!(BorrowedScope::new(host.as_mut())), cx)
        })
        .await
    }

    /// Complete a future that must not wait.
    pub fn run_sync<T>(future: impl Future<Output = T>) -> T {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("synchronous operation returned Pending"),
        }
    }
}
