#![doc = include_str!("../README.md")]
//!
//! ## Exclusive access
//!
//! A slot is borrowed mutably by each use, so steps run one after another:
//!
//! ```
//! # use nitori_call::{Direct, ReceiverAccess, ReceiverSlot};
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
//! # use nitori_call::{Direct, ReceiverAccess, ReceiverSlot};
//! # use std::{future::poll_fn, task::Poll};
//! async fn concurrent(io: &mut ReceiverSlot<'_, Direct<Vec<u8>>>) {
//!     let first = poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(())));
//!     let second = poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(())));
//!     let _ = (first, second);
//! }
//! ```
//!
//! ## The brand
//!
//! No operation returns its slot for every brand:
//!
//! ```compile_fail
//! # use nitori_call::{Emitter, Operation, ReceiverFamily, ReceiverSlot, Start};
//! # use std::{convert::Infallible, future::{Ready, ready}};
//! struct Leak;
//! impl<F: ReceiverFamily> Operation<F> for Leak {
//!     type Output = ReceiverSlot<'static, F>;
//!     type Yield = Infallible;
//! }
//! impl<'s, F: ReceiverFamily> Start<'s, ReceiverSlot<'s, F>> for Leak {
//!     type Fut = Ready<ReceiverSlot<'static, F>>;
//!     fn start(self, io: ReceiverSlot<'s, F>, _: Emitter<'s, Infallible>) -> Self::Fut {
//!         ready(io)
//!     }
//! }
//! ```
//!
//! and a call rejects an operation that accepts only one brand, such as one
//! that stores its slot in outside state:
//!
//! ```compile_fail
//! # use nitori_call::{Call, Direct, Emitter, Operation, ReceiverSlot, Start};
//! # use std::{convert::Infallible, future::{Ready, ready}};
//! type F = Direct<Vec<u8>>;
//! struct Keep<'x>(&'x mut Option<ReceiverSlot<'x, F>>);
//! impl Operation<F> for Keep<'_> {
//!     type Output = ();
//!     type Yield = Infallible;
//! }
//! impl<'x> Start<'x, ReceiverSlot<'x, F>> for Keep<'x> {
//!     type Fut = Ready<()>;
//!     fn start(self, io: ReceiverSlot<'x, F>, _: Emitter<'x, Infallible>) -> Ready<()> {
//!         *self.0 = Some(io);
//!         ready(())
//!     }
//! }
//! fn store<'x>(kept: &'x mut Option<ReceiverSlot<'x, F>>) {
//!     let _ = Call::new(Keep(kept));
//! }
//! ```
//!
//! ## Threads
//!
//! A slot stays on the thread polling its call:
//!
//! ```compile_fail
//! # use nitori_call::{Direct, ReceiverAccess, ReceiverSlot};
//! # use std::task::{Context, Poll, Waker};
//! fn elsewhere(io: &mut ReceiverSlot<'_, Direct<Vec<u8>>>) {
//!     std::thread::scope(|scope| {
//!         scope.spawn(|| io.poll_access(&mut Context::from_waker(Waker::noop()), |_, _| Poll::Ready(())));
//!     });
//! }
//! ```
//!
//! A call is `Send` when everything its operation keeps apart from the slot is
//! `Send`. The resource and its views need not be: they are used only
//! during a poll, on the polling thread.
//!
//! ```
//! #![feature(impl_trait_in_assoc_type)]
//! # use nitori_call::{Call, Direct, Emitter, Operation, ReceiverAccess, ReceiverFamily, Start};
//! # use std::{convert::Infallible, future::{Future, poll_fn}, rc::Rc, task::Poll};
//! struct Touch;
//! impl<F: ReceiverFamily> Operation<F> for Touch {
//!     type Output = ();
//!     type Yield = Infallible;
//! }
//! impl<'s, A: ReceiverAccess> Start<'s, A> for Touch {
//!     type Fut = impl Future<Output = ()>;
//!     fn start(self, mut io: A, _: Emitter<'s, Infallible>) -> Self::Fut {
//!         async move { poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(()))).await }
//!     }
//! }
//! fn assert_send<T: Send>(_: T) {}
//! assert_send(Call::<Direct<Rc<u8>>, _>::new(Touch));
//! ```
//!
//! An operation that keeps something else that is not `Send` across a wait
//! makes its call not `Send`:
//!
//! ```compile_fail
//! #![feature(impl_trait_in_assoc_type)]
//! # use nitori_call::{Call, Direct, Emitter, Operation, ReceiverAccess, ReceiverFamily, Start};
//! # use std::{convert::Infallible, future::{Future, poll_fn}, rc::Rc, task::Poll};
//! struct Hold;
//! impl<F: ReceiverFamily> Operation<F> for Hold {
//!     type Output = ();
//!     type Yield = Infallible;
//! }
//! impl<'s, A: ReceiverAccess> Start<'s, A> for Hold {
//!     type Fut = impl Future<Output = ()>;
//!     fn start(self, mut io: A, _: Emitter<'s, Infallible>) -> Self::Fut {
//!         async move {
//!             let kept = Rc::new(());
//!             poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(()))).await;
//!             drop(kept);
//!         }
//!     }
//! }
//! fn assert_send<T: Send>(_: T) {}
//! assert_send(Call::<Direct<Vec<u8>>, _>::new(Hold));
//! ```
#![deny(unsafe_op_in_unsafe_fn)]

mod receiver;
pub use receiver::{BorrowedReceiver, Direct, DirectView, Receiver, ReceiverFamily};
mod loan;
pub use loan::{ViewLoan, ViewOperation};
mod access;
pub use access::{Access, ReceiverAccess, ReceiverSlot};
mod compose;
pub use compose::{Compose, Composed};
mod call;
pub use call::{Call, CallPoll, DynCall, Emitter, Operation, Start};
mod child;
pub use child::Child;
mod driver;
pub use driver::{
    AcquiredScope, AcquisitionState, BoundCall, CallExecutor, Drive, ReceiverScope,
    ViewAcquisition, drive,
};

/// Generate an operation and its conveniences from an async function taking a
/// receiver access first.
///
/// The operation is a struct named after the function in CamelCase, with the
/// remaining arguments as fields. An argument written as a pattern, such as
/// `(start, end): (u8, u8)` or `_: u8`, is passed to the function whole and
/// named `argument_N` after its position in the parameter list, counting the
/// access as 0. Arguments may mention the access only as `A::Family`; when one
/// does, the struct takes the family as a last type parameter `Family`. Users
/// of the macro enable `impl_trait_in_assoc_type`.
///
/// With `yields = Item`, each `yield value` written directly in the function
/// body emits an item, and the return value is the final value. The crate
/// enables `yield_expr` as well. `yield` in closures (including coroutines),
/// async blocks, nested items and macro arguments is left as written. The
/// function gains a last parameter `&mut Emitter<'_, Item>`, and its access
/// method returns a [`Child`].
///
/// Arguments:
///
/// - `module = name` generates the operation in a new module `name`;
///   `module = self`, the default, in the current module.
/// - `sync` also generates `sync_*` host methods, which return the final value.
/// - `yields = Item` declares the type of the items.
///
/// A `yield` needs the item type declared:
///
/// ```compile_fail
/// #![feature(impl_trait_in_assoc_type, yield_expr)]
/// # use nitori_call::{ReceiverAccess, call};
/// #[call]
/// async fn undeclared(io: &mut impl ReceiverAccess<Family: Sized>) {
///     yield 1;
/// }
/// ```
///
/// The crate must enable `yield_expr`, since the compiler checks the syntax
/// before the macro runs:
///
/// ```compile_fail
/// #![feature(impl_trait_in_assoc_type)]
/// # use nitori_call::{ReceiverAccess, call};
/// #[call(yields = u8)]
/// async fn ungated(io: &mut impl ReceiverAccess<Family: Sized>) {
///     yield 1;
/// }
/// ```
///
/// and a `yield` inside a closure is not an item:
///
/// ```compile_fail
/// #![feature(impl_trait_in_assoc_type, yield_expr)]
/// # use nitori_call::{ReceiverAccess, call};
/// #[call(yields = u8)]
/// async fn nested(io: &mut impl ReceiverAccess<Family: Sized>) {
///     let emit = || yield 1;
/// }
/// ```
///
/// A parameter may not take the name of a destructured argument:
///
/// ```compile_fail
/// #![feature(impl_trait_in_assoc_type)]
/// # use nitori_call::{ReceiverAccess, call};
/// #[call]
/// async fn clash(io: &mut impl ReceiverAccess<Family: Sized>, (a, b): (u8, u8), argument_1: u8) {}
/// ```
///
/// Without `sync`, there are no synchronous host methods:
///
/// ```compile_fail
/// #![feature(impl_trait_in_assoc_type)]
/// # use nitori_call::{ReceiverAccess, call};
/// #[call]
/// async fn nothing(io: &mut impl ReceiverAccess<Family: Sized>) {}
/// let mut host = 0_u8;
/// host.sync_nothing_unpin();
/// ```
pub use nitori_call_macros::call;
/// Generate operations and conveniences from a capability trait's poll methods.
///
/// Each `poll_name` method becomes an operation struct `Name`, which is also
/// the future it starts. Type and const parameters of the trait become
/// parameters of the structs and of the convenience traits. A method whose
/// arguments mention `Self` gives its struct a type parameter `Family`.
/// Arguments:
///
/// - `module = name` generates the operations in module `name`; by default
///   the module is the trait's name in snake case. `module = self` generates
///   them in the current module.
/// - `sync` also generates `sync_*` host methods.
pub use nitori_call_macros::capability;

/// Support for generated code; not a stable interface.
#[doc(hidden)]
pub mod __private {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    /// An emitter for operations that emit nothing.
    pub fn detached() -> crate::Emitter<'static, std::convert::Infallible> {
        crate::Emitter::detached()
    }

    /// Complete a future that must not wait.
    pub fn run_sync<T>(future: impl Future<Output = T>) -> T {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("synchronous operation returned Pending"),
        }
    }
}
