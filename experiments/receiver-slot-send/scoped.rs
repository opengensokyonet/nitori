#![feature(async_fn_traits, unboxed_closures)]
#![allow(dead_code)]
use std::{
    cell::Cell,
    future::Future,
    marker::PhantomData,
    ops::AsyncFnOnce,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
    thread::{self, ThreadId},
};

struct Entry<T> {
    ptr: NonNull<T>,
    owner: Option<ThreadId>,
    check: fn(Option<ThreadId>) -> bool,
}
impl<T> Copy for Entry<T> {}
impl<T> Clone for Entry<T> {
    fn clone(&self) -> Self {
        *self
    }
}
struct Endpoint<T> {
    cell: NonNull<Cell<Option<Entry<T>>>>,
}
impl<T> Drop for Endpoint<T> {
    fn drop(&mut self) {
        // SAFETY: this is the allocation made by new, uniquely owned here.
        // Call drops its future before its endpoints, so no slot remains.
        unsafe {
            drop(Box::from_raw(self.cell.as_ptr()));
        }
    }
}
// SAFETY: this private owner never moves with a loan installed. lend holds
// &mut self until its stack guard has cleared the loan. Slots cannot escape
// the lifetime-polymorphic factory into an outer task or thread scope.
unsafe impl<T> Send for Endpoint<T> {}
struct ReceiverSlot<'s, T> {
    cell: *const Cell<Option<Entry<T>>>,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
    not_sync: PhantomData<Cell<()>>,
    invariant: PhantomData<fn(T) -> T>,
}
// SAFETY: the exclusive slot can only be moved to threads whose scope is
// inside the current poll (or destructor). It cannot escape through the
// factory's output or captures. Those threads finish before lend clears or
// changes the cell. !Sync and &mut access prevent concurrent slot access.
// A local loan checks its thread before accessing its pointee.
unsafe impl<T> Send for ReceiverSlot<'_, T> {}
#[derive(Debug, PartialEq, Eq)]
enum AccessError {
    Absent,
    WrongThread,
}
#[cfg(test)]
thread_local! { static CHECKS: Cell<usize> = const { Cell::new(0) }; }
fn local(owner: Option<ThreadId>) -> bool {
    #[cfg(test)]
    CHECKS.set(CHECKS.get() + 1);
    owner == Some(thread::current().id())
}
fn unrestricted(_: Option<ThreadId>) -> bool {
    true
}
impl<T> Endpoint<T> {
    fn new() -> Self {
        Self {
            cell: NonNull::new(Box::into_raw(Box::new(Cell::new(None)))).unwrap(),
        }
    }
    fn cell(&self) -> &Cell<Option<Entry<T>>> {
        // SAFETY: this owner keeps the allocation alive; all changes use Cell.
        unsafe { self.cell.as_ref() }
    }
    // Private: the caller must keep this allocation alive until the future
    // and its slots are destroyed. Only Call::new constructs slots.
    unsafe fn slot<'s>(&self) -> ReceiverSlot<'s, T> {
        ReceiverSlot {
            cell: self.cell.as_ptr(),
            brand: PhantomData,
            not_sync: PhantomData,
            invariant: PhantomData,
        }
    }
    fn lend<R>(
        &mut self,
        view: &mut T,
        owner: Option<ThreadId>,
        check: fn(Option<ThreadId>) -> bool,
        body: impl FnOnce() -> R,
    ) -> R {
        assert!(self.cell().get().is_none());
        self.cell().set(Some(Entry {
            ptr: NonNull::from(view),
            owner,
            check,
        }));
        struct Clear<'a, T>(&'a Cell<Option<Entry<T>>>);
        impl<T> Drop for Clear<'_, T> {
            fn drop(&mut self) {
                self.0.set(None);
            }
        }
        let _clear = Clear(self.cell());
        body()
    }
}
impl<T> ReceiverSlot<'_, T> {
    fn with<R>(&mut self, body: impl FnOnce(&mut T) -> R) -> Result<R, AccessError> {
        // SAFETY: factory lifetime confinement keeps the cell alive, and
        // neither the driver nor another slot accesses this cell concurrently.
        let mut entry = unsafe { &*self.cell }.get().ok_or(AccessError::Absent)?;
        if !(entry.check)(entry.owner) {
            return Err(AccessError::WrongThread);
        }
        // SAFETY: the loan remains alive during this synchronous callback;
        // its mutable borrow cannot escape in R. Thread access was checked.
        Ok(body(unsafe { entry.ptr.as_mut() }))
    }
}
type Running<F, T, U> =
    <F as AsyncFnOnce<(ReceiverSlot<'static, T>, ReceiverSlot<'static, U>)>>::CallOnceFuture;
struct Call<F, T, U, R>
where
    F: for<'s> AsyncFnOnce(ReceiverSlot<'s, T>, ReceiverSlot<'s, U>) -> R,
{
    // Dropped before the endpoint allocations.
    future: Pin<Box<Running<F, T, U>>>,
    a: Endpoint<T>,
    b: Endpoint<U>,
}
impl<F, T, U, R> Call<F, T, U, R>
where
    F: for<'s> AsyncFnOnce(ReceiverSlot<'s, T>, ReceiverSlot<'s, U>) -> R,
{
    fn new(factory: F) -> Self {
        let a = Endpoint::new();
        let b = Endpoint::new();
        // SAFETY: the higher-ranked factory cannot expose a slot through R
        // or outside captures. These stable allocations outlive its future.
        let future = Box::pin(factory(unsafe { a.slot() }, unsafe { b.slot() }));
        Self { future, a, b }
    }
    fn poll_local_send(&mut self, a: &mut T, b: &mut U, cx: &mut Context<'_>) -> Poll<R>
    where
        U: Send,
    {
        let Self {
            future,
            a: a_owner,
            b: b_owner,
        } = self;
        a_owner.lend(a, Some(thread::current().id()), local, || {
            b_owner.lend(b, None, unrestricted, || future.as_mut().poll(cx))
        })
    }
    fn poll_absent(&mut self, cx: &mut Context<'_>) -> Poll<R> {
        self.future.as_mut().poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{rc::Rc, task::Waker};
    fn assert_send<T: Send>(_: &T) {}
    async fn yield_once() {
        let mut done = false;
        std::future::poll_fn(move |cx| {
            if std::mem::replace(&mut done, true) {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
    }
    #[test]
    fn actual_async_future_and_call_are_send() {
        let capture = String::from("abc");
        let mut call = Call::new(
            async move |mut a: ReceiverSlot<'_, Rc<Cell<u64>>>, mut b: ReceiverSlot<'_, u64>| {
                let first = a.with(|a| a.get()).unwrap() + b.with(|b| *b).unwrap();
                yield_once().await;
                first
                    + a.with(|a| a.get()).unwrap()
                    + b.with(|b| *b).unwrap()
                    + capture.len() as u64
            },
        );
        assert_send(&call.future);
        assert_send(&call);
        let mut a = Rc::new(Cell::new(2));
        let mut b = 3;
        CHECKS.set(0);
        assert_eq!(
            call.poll_local_send(&mut a, &mut b, &mut Context::from_waker(Waker::noop())),
            Poll::Pending
        );
        assert_eq!(CHECKS.get(), 1);
        let result = thread::spawn(move || {
            let mut a = Rc::new(Cell::new(5));
            let mut b = 7;
            CHECKS.set(0);
            let result =
                call.poll_local_send(&mut a, &mut b, &mut Context::from_waker(Waker::noop()));
            assert_eq!(CHECKS.get(), 1);
            result
        })
        .join()
        .unwrap();
        assert_eq!(result, Poll::Ready(20));
    }
    #[test]
    fn scoped_threads_reject_local_and_accept_send_view() {
        let mut call = Call::new(
            async |mut a: ReceiverSlot<'_, Rc<Cell<u64>>>, mut b: ReceiverSlot<'_, u64>| {
                thread::scope(|scope| {
                    let a = scope.spawn(|| a.with(|a| a.set(99)));
                    let b = scope.spawn(|| b.with(|b| *b = 99));
                    assert_eq!(a.join().unwrap(), Err(AccessError::WrongThread));
                    assert_eq!(b.join().unwrap(), Ok(()));
                });
            },
        );
        let mut a = Rc::new(Cell::new(1));
        let mut b = 2;
        assert_eq!(
            call.poll_local_send(&mut a, &mut b, &mut Context::from_waker(Waker::noop())),
            Poll::Ready(())
        );
        assert_eq!(a.get(), 1);
        assert_eq!(b, 99);
    }
    #[test]
    fn nested_access_to_two_views() {
        let mut call = Call::new(
            async |mut a: ReceiverSlot<'_, Rc<Cell<u64>>>, mut b: ReceiverSlot<'_, u64>| {
                a.with(|a| b.with(|b| a.get() + *b)).unwrap().unwrap()
            },
        );
        let mut a = Rc::new(Cell::new(2));
        let mut b = 3;
        assert_eq!(
            call.poll_local_send(&mut a, &mut b, &mut Context::from_waker(Waker::noop())),
            Poll::Ready(5)
        );
    }
    #[test]
    fn panic_clears_views() {
        let mut call = Call::new(
            async |mut a: ReceiverSlot<'_, Rc<Cell<u64>>>, _: ReceiverSlot<'_, u64>| {
                a.with(|_| panic!("test")).unwrap()
            },
        );
        let mut a = Rc::new(Cell::new(2));
        let mut b = 3;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call.poll_local_send(&mut a, &mut b, &mut Context::from_waker(Waker::noop()))
        }));
        assert!(result.is_err());
        assert!(call.a.cell().get().is_none());
        assert!(call.b.cell().get().is_none());
    }
    #[test]
    fn cancellation_drops_slots_while_cells_still_exist() {
        struct OnDrop<'s>(ReceiverSlot<'s, Rc<Cell<u64>>>);
        impl Drop for OnDrop<'_> {
            fn drop(&mut self) {
                assert_eq!(self.0.with(|_| ()), Err(AccessError::Absent));
            }
        }
        let mut call = Call::new(
            async |a: ReceiverSlot<'_, Rc<Cell<u64>>>, _: ReceiverSlot<'_, u64>| {
                let kept = OnDrop(a);
                std::future::pending::<()>().await;
                drop(kept);
            },
        );
        assert_send(&call);
        let mut a = Rc::new(Cell::new(2));
        let mut b = 3;
        assert_eq!(
            call.poll_local_send(&mut a, &mut b, &mut Context::from_waker(Waker::noop())),
            Poll::Pending
        );
        thread::spawn(move || drop(call)).join().unwrap();
    }
    #[test]
    fn borrowed_capture_and_borrowed_receiver_type() {
        fn run<'data>(data: &'data [u8]) -> usize {
            let mut output = 0;
            {
                let mut call = Call::new(
                    async |mut a: ReceiverSlot<'_, &'data [u8]>, _: ReceiverSlot<'_, u64>| {
                        output = a.with(|data| data.len()).unwrap();
                    },
                );
                assert_send(&call);
                let mut view = data;
                let mut other = 0;
                assert_eq!(
                    call.poll_local_send(
                        &mut view,
                        &mut other,
                        &mut Context::from_waker(Waker::noop())
                    ),
                    Poll::Ready(())
                );
            }
            output
        }
        let data = vec![1, 2, 3];
        assert_eq!(run(&data), 3);
    }

    #[test]
    fn shared_reference_between_polls_with_self_borrow() {
        let mut call = Call::new(async |_: ReceiverSlot<'_, u64>, _: ReceiverSlot<'_, u64>| {
            let mut value = 0u64;
            let borrowed = &mut value;
            *borrowed = 1;
            yield_once().await;
            *borrowed += 1;
            *borrowed
        });
        assert_eq!(call.poll_absent(&mut Context::from_waker(Waker::noop())), Poll::Pending);
        std::hint::black_box(&call);
        assert_eq!(call.poll_absent(&mut Context::from_waker(Waker::noop())), Poll::Ready(2));
    }
}

#[cfg(reject_rc)]
fn main() {
    let call = Call::new(async |_: ReceiverSlot<'_, u64>, _: ReceiverSlot<'_, u64>| {
        let x = std::rc::Rc::new(1);
        std::future::pending::<()>().await;
        drop(x);
    });
    fn send<T: Send>(_: T) {}
    send(call);
}
#[cfg(reject_escape)]
fn main() {
    thread::scope(|outer| {
        let _call = Call::new(
            async |mut slot: ReceiverSlot<'_, u64>, _: ReceiverSlot<'_, u64>| {
                outer.spawn(move || slot.with(|_| ()));
            },
        );
    });
}
#[cfg(reject_return)]
fn main() {
    let _call = Call::new(async |slot: ReceiverSlot<'_, u64>, _: ReceiverSlot<'_, u64>| slot);
}
#[cfg(reject_view)]
fn main() {
    let _call = Call::new(
        async |mut slot: ReceiverSlot<'_, u64>, _: ReceiverSlot<'_, u64>| {
            let leaked = slot.with(|view| view).unwrap();
            std::future::pending::<()>().await;
            *leaked
        },
    );
}
#[cfg(reject_send_loan)]
fn main() {
    let mut call =
        Call::new(async |_: ReceiverSlot<'_, u64>, _: ReceiverSlot<'_, std::rc::Rc<u64>>| ());
    let mut a = 0;
    let mut b = std::rc::Rc::new(1);
    call.poll_local_send(
        &mut a,
        &mut b,
        &mut Context::from_waker(std::task::Waker::noop()),
    );
}
#[cfg(reject_sync)]
fn main() {
    let _call = Call::new(
        async |slot: ReceiverSlot<'_, u64>, _: ReceiverSlot<'_, u64>| {
            thread::scope(|scope| {
                let shared = &slot;
                scope.spawn(move || {
                    let _ = shared;
                    std::hint::black_box(shared);
                });
            });
        },
    );
}
