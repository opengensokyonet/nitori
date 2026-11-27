//! Run access-side operations on an existing access or a receiver.
use crate::{CallExecutor, Emitter, Operation, Receiver, ReceiverFamily, ReceiverSlot, Start};
use core::{future::Future, pin::Pin};

mod sealed {
    pub trait Sealed {}
}

/// How an operation obtains access: [`OnAccess`] or [`OnReceiver`].
pub trait AccessMode: sealed::Sealed {}

/// The operation runs on the access it is given.
pub struct OnAccess;
/// The operation runs as a call on the receiver it is given.
pub struct OnReceiver;
impl sealed::Sealed for OnAccess {}
impl sealed::Sealed for OnReceiver {}
impl AccessMode for OnAccess {}
impl AccessMode for OnReceiver {}

type Slot<'a, R> = ReceiverSlot<'a, <R as Receiver>::Family>;

/// An operation written once on an access `S`: run as it is on an access
/// ([`OnAccess`]), or as one call on a receiver ([`OnReceiver`]), which the
/// framework provides for every operation on the call's slot.
///
/// To open a call on a receiver that is pinned, pass its pinned pointer.
/// A receiver passed directly must be `Unpin`.
///
/// ```compile_fail,E0277
/// use core::marker::PhantomPinned;
/// use nitori_call::{AccessOperation, ReceiverAccess, direct_receiver};
///
/// struct Fixed(PhantomPinned);
/// direct_receiver!(impl [] for Fixed);
///
/// struct Observe;
/// impl<A: ReceiverAccess> AccessOperation<A> for Observe {
///     type Output = ();
///     async fn run_on(self, _: &mut A) {}
/// }
///
/// let mut receiver = Fixed(PhantomPinned);
/// Observe.run_on(&mut receiver);
/// ```
pub trait AccessOperation<S, How: AccessMode = OnAccess> {
    /// What the operation gives.
    type Output;
    /// Run the operation on `source`.
    fn run_on(self, source: &mut S) -> impl Future<Output = Self::Output>;
}

/// The call that runs an [`AccessOperation`] on a receiver's slot.
struct OnSlot<Op>(Op);

impl<F, Op, T> Operation<F> for OnSlot<Op>
where
    F: ReceiverFamily,
    for<'s> Op: AccessOperation<ReceiverSlot<'s, F>, Output = T>,
{
    type Output = T;
    type Yield = !;
}

impl<'s, F, Op, T> Start<'s, ReceiverSlot<'s, F>> for OnSlot<Op>
where
    F: ReceiverFamily,
    for<'t> Op: AccessOperation<ReceiverSlot<'t, F>, Output = T>,
{
    type Fut = impl Future<Output = T>;
    fn start(self, slot: ReceiverSlot<'s, F>, _: Emitter<'s, !>) -> Self::Fut {
        let op = self.0;
        async move {
            let mut slot = slot;
            op.run_on(&mut slot).await
        }
    }
}

impl<R, Op, T> AccessOperation<R, OnReceiver> for Op
where
    R: Receiver + Unpin,
    for<'s> Op: AccessOperation<Slot<'s, R>, Output = T>,
{
    type Output = T;
    async fn run_on(self, receiver: &mut R) -> T {
        Pin::new(receiver).execute(OnSlot(self)).await
    }
}
