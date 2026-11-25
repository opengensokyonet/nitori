//! A tuple of locks whose operation asks one slot, then another: the
//! acquisition left pending in the first slot is still advanced, so the
//! first lock is not held by a request nobody polls.
#![feature(impl_trait_in_assoc_type)]
#![cfg(any(feature = "tokio-sync", feature = "futures-lock"))]
mod support;

use nitori_call::{
    Emitter, Guarded, LockAcquisition, Operation, ReceiverAccess, Receivers, Start, drive_composed,
};
use nitori_io::prelude::*;
use std::{
    cell::Cell,
    convert::Infallible,
    future::{Future, poll_fn},
    pin::pin,
};

/// Writes `a` to the first receiver while `first` is set, and `b` to the
/// second once it is not; the first write is left pending when it switches.
struct WriteFirstThenSecond<'a> {
    first: &'a Cell<bool>,
}
impl<A: Receivers, B: Receivers> Operation<(A, B)> for WriteFirstThenSecond<'_> {
    type Output = ();
    type Yield = Infallible;
}
impl<'s, A, B> Start<'s, (A, B)> for WriteFirstThenSecond<'_>
where
    A: ReceiverAccess<Family: Write<Report: nitori_io::Combine>>,
    B: ReceiverAccess<Family: Write<Report: nitori_io::Combine>>,
{
    type Fut = impl Future<Output = ()>;
    fn start(self, (mut a, mut b): (A, B), _: Emitter<'s, Infallible>) -> Self::Fut {
        async move {
            let mut first = pin!(a.write_all(&b"a"[..]));
            let mut second = pin!(b.write_all(&b"b"[..]));
            poll_fn(|cx| match self.first.get() {
                true => first.as_mut().poll(cx).map(|_| ()),
                false => second.as_mut().poll(cx).map(|_| ()),
            })
            .await;
        }
    }
}

#[cfg(feature = "tokio-sync")]
mod tokio_sync {
    use super::*;
    use support::poll_once;
    use tokio::sync::Mutex;

    #[test]
    fn a_lock_granted_while_another_slot_waits_is_released() {
        let (m0, m1) = (Mutex::new(Vec::new()), Mutex::new(Vec::new()));
        let (hold0, hold1) = (m0.try_lock().unwrap(), m1.try_lock().unwrap());
        let first = Cell::new(true);
        let mut write = pin!(drive_composed(
            (LockAcquisition::new(&m0), LockAcquisition::new(&m1)),
            (Guarded, Guarded),
            WriteFirstThenSecond { first: &first },
        ));
        // The first slot asks and queues on the first lock.
        assert!(poll_once(write.as_mut()).is_pending());
        // The operation turns to the second slot, which waits too; the
        // first lock is then granted to the queued acquisition.
        first.set(false);
        assert!(poll_once(write.as_mut()).is_pending());
        drop(hold0);
        assert!(poll_once(write.as_mut()).is_pending());
        assert!(m0.try_lock().is_ok(), "held while waiting for the second");
        drop(hold1);
        assert!(poll_once(write.as_mut()).is_ready());
        assert_eq!(*m1.try_lock().unwrap(), b"b");
    }

    #[test]
    fn a_lock_granted_by_the_poll_that_completes_is_released() {
        let (m0, m1) = (Mutex::new(Vec::new()), Mutex::new(Vec::new()));
        let (hold0, hold1) = (m0.try_lock().unwrap(), m1.try_lock().unwrap());
        let first = Cell::new(true);
        let mut write = pin!(drive_composed(
            (LockAcquisition::new(&m0), LockAcquisition::new(&m1)),
            (Guarded, Guarded),
            WriteFirstThenSecond { first: &first },
        ));
        assert!(poll_once(write.as_mut()).is_pending());
        first.set(false);
        drop(hold0);
        drop(hold1);
        assert!(poll_once(write.as_mut()).is_ready());
        assert!(m0.try_lock().is_ok(), "held after the operation completed");
    }
}

#[cfg(feature = "futures-lock")]
mod futures_lock {
    use super::*;
    use futures_util::lock::Mutex;
    use std::task::Context;
    use support::{WakeCount, poll_once};

    #[test]
    fn a_wakeup_meant_for_another_slot_is_passed_on() {
        let (m0, m1) = (Mutex::new(Vec::new()), Mutex::new(Vec::new()));
        let (hold0, hold1) = (m0.try_lock().unwrap(), m1.try_lock().unwrap());
        let first = Cell::new(true);
        let mut write = pin!(drive_composed(
            (LockAcquisition::new(&m0), LockAcquisition::new(&m1)),
            (Guarded, Guarded),
            WriteFirstThenSecond { first: &first },
        ));
        // The first slot waits on the first lock, ahead of another task.
        assert!(poll_once(write.as_mut()).is_pending());
        let (other_wakes, other_waker) = WakeCount::new();
        let mut other = pin!(m0.lock());
        assert!(
            other
                .as_mut()
                .poll(&mut Context::from_waker(&other_waker))
                .is_pending()
        );
        // The operation turns to the second slot. Unlocking the first lock
        // wakes only the first waiter, the acquisition of the first slot;
        // advancing it takes and releases the lock, which wakes the other
        // task.
        first.set(false);
        assert!(poll_once(write.as_mut()).is_pending());
        drop(hold0);
        assert!(poll_once(write.as_mut()).is_pending());
        assert_eq!(other_wakes.get(), 1, "the other task is not woken");
        drop(hold1);
        assert!(poll_once(write.as_mut()).is_ready());
    }
}
