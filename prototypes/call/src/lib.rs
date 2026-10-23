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

/// Support for implementing [`ReceiverAccess`]; not a stable interface.
#[doc(hidden)]
pub mod __private {
    pub use crate::access::{ScopeRequest, ThreadAnchor};
}
