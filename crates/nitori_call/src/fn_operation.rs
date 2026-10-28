//! Operations written as async closures.
use crate::{Emitter, Operation, Receivers, Start};
use std::{convert::Infallible, fmt, marker::PhantomData, ops::AsyncFnOnce};

/// An operation that runs an async closure over the slots of `R`; see
/// [`fn_operation`].
pub struct FnOperation<R, C> {
    closure: C,
    receivers: PhantomData<fn() -> R>,
}
impl<R, C> fmt::Debug for FnOperation<R, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FnOperation").finish_non_exhaustive()
    }
}

/// An operation that runs `closure` with the slots of `R`: one slot for a
/// family, a tuple of slots for a tuple of families. The closure must accept
/// slots of every brand, so it can neither return a slot nor hand one to
/// state outside its future; it emits no items.
///
/// ```
/// # use nitori_call::{Call, Direct, ReceiverAccess, ReceiverSlot, fn_operation};
/// # use std::{future::poll_fn, task::Poll};
/// let add = fn_operation::<(Direct<u8>, Direct<u8>), _>(
///     async |(mut a, mut b): (ReceiverSlot<'_, Direct<u8>>, ReceiverSlot<'_, Direct<u8>>)| {
///         let x = poll_fn(|cx| a.poll_access(cx, |v, _| Poll::Ready(*v.into_pin().get_pin_mut()))).await;
///         let y = poll_fn(|cx| b.poll_access(cx, |v, _| Poll::Ready(*v.into_pin().get_pin_mut()))).await;
///         x + y
///     },
/// );
/// let _call = Call::new(add);
/// ```
///
/// A closure cannot return its slot,
///
/// ```compile_fail
/// # use nitori_call::{Call, Direct, ReceiverSlot, fn_operation};
/// let _call = Call::new(fn_operation::<Direct<u8>, _>(async |io: ReceiverSlot<'_, Direct<u8>>| io));
/// ```
///
/// nor hand it to a thread scope outside its future:
///
/// ```compile_fail
/// # use nitori_call::{Call, Direct, ReceiverSlot, fn_operation};
/// std::thread::scope(|outer| {
///     let _call = Call::new(fn_operation::<Direct<u8>, _>(
///         async |io: ReceiverSlot<'_, Direct<u8>>| {
///             outer.spawn(move || drop(io));
///         },
///     ));
/// });
/// ```
///
/// A closure that keeps something that is not `Send` across a wait makes its
/// call not `Send`:
///
/// ```compile_fail
/// # use nitori_call::{Call, Direct, ReceiverSlot, fn_operation};
/// # use std::{future::poll_fn, rc::Rc, task::Poll};
/// let call = Call::new(fn_operation::<Direct<u8>, _>(async |_: ReceiverSlot<'_, Direct<u8>>| {
///     let kept = Rc::new(());
///     poll_fn(|cx| { cx.waker().wake_by_ref(); Poll::<()>::Pending }).await;
///     drop(kept);
/// }));
/// fn assert_send<T: Send>(_: T) {}
/// assert_send(call);
/// ```
pub fn fn_operation<R, C>(closure: C) -> FnOperation<R, C>
where
    R: Receivers,
    C: for<'s> AsyncFnOnce<(R::Slots<'s>,)>,
{
    FnOperation {
        closure,
        receivers: PhantomData,
    }
}

impl<R, C, T> Operation<R> for FnOperation<R, C>
where
    R: Receivers,
    C: for<'s> AsyncFnOnce<(R::Slots<'s>,), Output = T>,
{
    type Output = T;
    type Yield = Infallible;
}
impl<'s, R, C, T> Start<'s, R::Slots<'s>> for FnOperation<R, C>
where
    R: Receivers,
    C: for<'x> AsyncFnOnce<(R::Slots<'x>,), Output = T>,
{
    type Fut = <C as AsyncFnOnce<(R::Slots<'s>,)>>::CallOnceFuture;
    fn start(self, slots: R::Slots<'s>, _: Emitter<'s, Infallible>) -> Self::Fut {
        self.closure.async_call_once((slots,))
    }
}
