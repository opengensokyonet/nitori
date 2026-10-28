//! Isolated experiment, not an implementation of nitori's request protocol.
use std::{
    marker::PhantomData,
    ptr::NonNull,
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    thread::{self, ThreadId},
};

struct Entry<T: ?Sized> {
    pointer: NonNull<T>,
    owner: Option<ThreadId>,
    check: fn(Option<ThreadId>) -> bool,
}
struct State<T: ?Sized> {
    entry: Option<Entry<T>>,
}
// SAFETY: no pointee is owned by this state. A live loan is accessed only
// under the mutex, through the checked operation below. An unchecked loan
// can only be installed with T: Send. Other loans check the borrowing thread
// before touching T. The loan's stack guard locks the same mutex and removes
// the pointer before the original mutable borrow ends, even during unwinding.
unsafe impl<T: ?Sized> Send for State<T> {}

struct ReceiverSlot<T: ?Sized> {
    state: Arc<Mutex<State<T>>>,
    // Invariant in T, but does not claim ownership of a T.
    invariant: PhantomData<fn(*mut T) -> *mut T>,
}
struct Lender<T: ?Sized> {
    state: Arc<Mutex<State<T>>>,
    invariant: PhantomData<fn(*mut T) -> *mut T>,
}
#[derive(Debug, PartialEq, Eq)]
enum AccessError {
    Absent,
    WrongThread,
    Busy,
}

fn pair<T: ?Sized>() -> (Lender<T>, ReceiverSlot<T>) {
    let state = Arc::new(Mutex::new(State { entry: None }));
    (
        Lender {
            state: state.clone(),
            invariant: PhantomData,
        },
        ReceiverSlot {
            state,
            invariant: PhantomData,
        },
    )
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
thread_local! { static THREAD_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn checked(owner: Option<ThreadId>) -> bool {
    #[cfg(test)]
    THREAD_CHECKS.with(|count| count.set(count.get() + 1));
    owner == Some(thread::current().id())
}
fn unchecked(_: Option<ThreadId>) -> bool {
    true
}

impl<T: ?Sized> Lender<T> {
    fn lend_local<R>(&mut self, value: &mut T, body: impl FnOnce() -> R) -> R {
        self.lend(value, Some(thread::current().id()), checked, body)
    }
    fn lend_send<R>(&mut self, value: &mut T, body: impl FnOnce() -> R) -> R
    where
        T: Send,
    {
        self.lend(value, None, unchecked, body)
    }
    fn lend<R>(
        &mut self,
        value: &mut T,
        owner: Option<ThreadId>,
        check: fn(Option<ThreadId>) -> bool,
        body: impl FnOnce() -> R,
    ) -> R {
        let mut state = lock(&self.state);
        assert!(state.entry.is_none());
        state.entry = Some(Entry {
            pointer: NonNull::from(value),
            owner,
            check,
        });
        drop(state);
        struct Clear<'a, T: ?Sized>(&'a Mutex<State<T>>);
        impl<T: ?Sized> Drop for Clear<'_, T> {
            fn drop(&mut self) {
                lock(self.0).entry = None;
            }
        }
        let _clear = Clear(&self.state);
        body()
    }
}

impl<T: ?Sized> ReceiverSlot<T> {
    fn with<R>(&mut self, body: impl FnOnce(&mut T) -> R) -> Result<R, AccessError> {
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(poison)) => poison.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(AccessError::Busy),
        };
        let entry = state.entry.as_mut().ok_or(AccessError::Absent)?;
        if !(entry.check)(entry.owner) {
            return Err(AccessError::WrongThread);
        }
        // SAFETY: Clear cannot revoke the loan while this mutex is held.
        // The entry was installed from a live &mut T; the callback's borrow
        // cannot escape in R. The check enforces the T: !Send restriction.
        Ok(body(unsafe { entry.pointer.as_mut() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        future::{Future, poll_fn},
        pin::Pin,
        rc::Rc,
        task::{Context, Poll, Waker},
    };
    fn assert_send<T: Send>(_: &T) {}
    fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn thread_check_only_for_local_loan() {
        let (mut owner, mut slot) = pair();
        let mut value = 1u64;
        THREAD_CHECKS.set(0);
        owner.lend_send(&mut value, || assert_eq!(slot.with(|v| *v += 1), Ok(())));
        assert_eq!(THREAD_CHECKS.get(), 0);
        owner.lend_local(&mut value, || assert_eq!(slot.with(|v| *v += 1), Ok(())));
        assert_eq!(THREAD_CHECKS.get(), 1);
        assert_eq!(value, 3);
        assert_eq!(slot.with(|v| *v), Err(AccessError::Absent));
    }

    #[test]
    fn local_loan_cannot_be_used_from_other_thread() {
        let (mut owner, mut slot) = pair();
        let mut value = Rc::new(Cell::new(7));
        assert_send(&slot);
        owner.lend_local(&mut value, || {
            thread::scope(|scope| {
                let result = scope.spawn(|| slot.with(|v| v.set(99))).join().unwrap();
                assert_eq!(result, Err(AccessError::WrongThread));
            })
        });
        assert_eq!(value.get(), 7);
    }

    #[test]
    fn send_loan_can_be_used_from_other_thread() {
        let (mut owner, mut slot) = pair();
        let mut value = 7u64;
        owner.lend_send(&mut value, || {
            thread::scope(|scope| {
                scope
                    .spawn(|| slot.with(|v| *v = 99).unwrap())
                    .join()
                    .unwrap();
            })
        });
        assert_eq!(value, 99);
    }

    #[test]
    fn ordinary_future_with_two_slots_moves_between_polls() {
        let (mut a_owner, mut a) = pair::<Rc<Cell<u64>>>();
        let (mut b_owner, mut b) = pair::<u64>();
        let mut future = Box::pin(async move {
            let first = a.with(|v| v.get()).unwrap() + b.with(|v| *v).unwrap();
            let mut yielded = false;
            poll_fn(move |cx| {
                if std::mem::replace(&mut yielded, true) {
                    Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            })
            .await;
            first + a.with(|v| v.get()).unwrap() + b.with(|v| *v).unwrap()
        });
        assert_send(&future);
        let mut local = Rc::new(Cell::new(2));
        let mut other = 3;
        a_owner.lend_local(&mut local, || {
            b_owner.lend_send(&mut other, || {
                assert_eq!(poll(future.as_mut()), Poll::Pending)
            })
        });
        let result = thread::spawn(move || {
            let mut local = Rc::new(Cell::new(5));
            let mut other = 7;
            a_owner.lend_local(&mut local, || {
                b_owner.lend_send(&mut other, || poll(future.as_mut()))
            })
        })
        .join()
        .unwrap();
        assert_eq!(result, Poll::Ready(17));
    }

    #[test]
    fn callback_panic_clears_and_releases_the_view() {
        let (mut owner, mut slot) = pair();
        let mut value = Rc::new(Cell::new(1));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            owner.lend_local(&mut value, || {
                let _ = slot.with(|_| panic!("test"));
            });
        }));
        assert!(result.is_err());
        assert_eq!(slot.with(|_| ()), Err(AccessError::Absent));
        owner.lend_local(&mut value, || assert_eq!(slot.with(|v| v.get()), Ok(1)));
    }

    #[test]
    fn nested_access_to_different_slots() {
        let (mut a_owner, mut a) = pair();
        let (mut b_owner, mut b) = pair();
        let mut x = 2;
        let mut y = 3;
        a_owner.lend_send(&mut x, || {
            b_owner.lend_send(&mut y, || {
                assert_eq!(a.with(|a| b.with(|b| *a + *b)), Ok(Ok(5)));
            })
        });
    }

    #[test]
    fn cancellation_can_drop_slot_inside_live_loan() {
        let (mut owner, slot) = pair::<Rc<Cell<u64>>>();
        let mut value = Rc::new(Cell::new(8));
        owner.lend_local(&mut value, || drop(slot));
        assert_eq!(Rc::strong_count(&value), 1);
    }

    #[test]
    fn lend_scope_waits_for_an_already_started_access() {
        let (mut owner, mut slot) = pair();
        let mut value = 0;
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let worker = owner.lend_send(&mut value, || {
            let worker = thread::spawn(move || {
                slot.with(|v| {
                    started_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                    *v = 42;
                })
                .unwrap();
                slot
            });
            started_rx.recv().unwrap();
            finish_tx.send(()).unwrap();
            worker
        });
        assert_eq!(value, 42);
        let mut slot = worker.join().unwrap();
        assert_eq!(slot.with(|_| ()), Err(AccessError::Absent));
    }

    #[test]
    fn reentrant_access_is_rejected_instead_of_aliasing() {
        // Deliberate duplicate only inside this private unit test: public
        // handles have no Clone implementation.
        let (mut owner, mut slot) = pair();
        let alias = RefCell::new(ReceiverSlot {
            state: slot.state.clone(),
            invariant: PhantomData,
        });
        let mut value = 1;
        owner.lend_send(&mut value, || {
            assert_eq!(
                slot.with(|_| alias.borrow_mut().with(|_| ())),
                Ok(Err(AccessError::Busy))
            );
        });
    }
}
