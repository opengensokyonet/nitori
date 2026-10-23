//! Operations that use a resource lent by their caller for one poll at a time.
//!
//! A [`Call`] owns an operation and receives a [`ReceiverScope`] on every poll.
//! The operation's future reaches the current view through its
//! [`ReceiverSlot`]; the caller decides how long each view stays acquired.
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
//! No operation returns its slot for every brand:
//!
//! ```compile_fail
//! # use nitori_call_prototype::{Emitter, Operation, ReceiverFamily, ReceiverSlot, Start};
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
//! # use nitori_call_prototype::{Call, Direct, Emitter, Operation, ReceiverFamily, ReceiverSlot, Start};
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
//! # Threads
//!
//! A slot stays on the thread polling its call:
//!
//! ```compile_fail
//! # use nitori_call_prototype::{Direct, ReceiverAccess, ReceiverSlot};
//! # use std::task::{Context, Poll, Waker};
//! fn elsewhere(io: &mut ReceiverSlot<'_, Direct<Vec<u8>>>) {
//!     std::thread::scope(|scope| {
//!         scope.spawn(|| io.poll_access(&mut Context::from_waker(Waker::noop()), |_, _| Poll::Ready(())));
//!     });
//! }
//! ```
//!
//! A call is `Send` when everything its operation keeps apart from the slot is
//! `Send`. The resource, its views and scopes need not be: they are used only
//! during a poll, on the polling thread.
//!
//! ```
//! #![feature(impl_trait_in_assoc_type)]
//! # use nitori_call_prototype::{Call, Direct, Emitter, Operation, ReceiverAccess, ReceiverFamily, Start};
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
//! # use nitori_call_prototype::{Call, Direct, Emitter, Operation, ReceiverAccess, ReceiverFamily, Start};
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
#![feature(associated_type_defaults)]
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
pub use access::{Access, ReceiverAccess, ReceiverSlot, ScopeVisitor};
mod compose;
pub use compose::{Compose, Composed};
mod call;
pub use call::{Call, DynCall, Emitter, Operation, Start};

/// Generate an operation and its conveniences from an async function taking a
/// receiver access first.
///
/// The operation is a struct named after the function in CamelCase, with the
/// remaining arguments as fields. Users of the macro enable
/// `impl_trait_in_assoc_type`.
///
/// An operation that emits takes `&mut Emitter<'_, Item>` second. Its access
/// method takes the caller's emitter, and it has no host methods, since those
/// return only the result.
///
/// Arguments:
///
/// - `module = name` generates the operation in a new module `name`;
///   `module = self`, the default, in the current module.
/// - `sync` also generates `sync_*` host methods.
///
/// Without `sync`, there are no synchronous host methods:
///
/// ```compile_fail
/// #![feature(impl_trait_in_assoc_type)]
/// # use nitori_call_prototype::{ReceiverAccess, call};
/// #[call]
/// async fn nothing(io: &mut impl ReceiverAccess<Family: Sized>) {}
/// let mut host = 0_u8;
/// host.sync_nothing_unpin();
/// ```
pub use nitori_call_prototype_macros::call;
/// Generate operations and conveniences from a capability trait's poll methods.
///
/// Each `poll_name` method becomes an operation struct `Name`, which is also
/// the future it starts. Type and const parameters of the trait become
/// parameters of the structs and of the convenience traits. Arguments:
///
/// - `module = name` generates the operations in module `name`; by default
///   the module is the trait's name in snake case. `module = self` generates
///   them in the current module.
/// - `sync` also generates `sync_*` host methods.
pub use nitori_call_prototype_macros::capability;

/// Support for generated code; not a stable interface.
#[doc(hidden)]
pub mod __private {
    use crate::{BorrowedScope, Call, Emitter, Operation, Receiver, ReceiverSlot, Start};
    use std::{
        convert::Infallible,
        future::{Future, poll_fn},
        pin::{Pin, pin},
        task::{Context, Poll, Waker},
    };

    /// Run an operation on a host, lending a fresh lazy scope on each poll.
    pub async fn bind<H, Op>(
        host: Pin<&mut H>,
        operation: Op,
    ) -> <Op as Operation<H::Family>>::Output
    where
        H: Receiver + ?Sized,
        Op: Operation<H::Family, Yield = Infallible>
            + for<'s> Start<'s, ReceiverSlot<'s, H::Family>>,
    {
        let mut host = host;
        let mut call = pin!(Call::new(operation));
        poll_fn(|cx| {
            call.as_mut()
                .poll_return_with(pin!(BorrowedScope::new(host.as_mut())), cx)
        })
        .await
    }

    /// An emitter for operations started outside a call; it can emit nothing.
    pub fn detached() -> Emitter<'static, Infallible> {
        Emitter::detached()
    }

    /// Complete a future that must not wait.
    pub fn run_sync<T>(future: impl Future<Output = T>) -> T {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("synchronous operation returned Pending"),
        }
    }
}
