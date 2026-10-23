//! An outer executor that acquires an async lock first, then lends the locked
//! resource to every ready call in one pass.
#![feature(impl_trait_in_assoc_type)]
mod support;

use bytes::BufMut;
use nitori_call_prototype::{
    BorrowedScope, Call, Direct, DynCall, ReceiverAccess, call, direct_receiver,
};
use std::{
    cell::{Cell, RefCell, RefMut},
    collections::VecDeque,
    convert::Infallible,
    future::Future,
    pin::{Pin, pin},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};
use support::{Read, ReadError, read_array, read_exact};

/// Bytes that arrive over time; every waiting reader is woken on arrival.
#[derive(Default)]
struct Pipe {
    data: VecDeque<u8>,
    readers: Vec<Waker>,
}
impl Pipe {
    fn push(&mut self, bytes: &[u8]) {
        self.data.extend(bytes);
        self.readers.drain(..).for_each(Waker::wake);
    }
}
direct_receiver!(impl [] for Pipe);
impl Read for Direct<Pipe> {
    type Error = Infallible;
    fn poll_read<'visit, Output: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'visit>>,
        cx: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Infallible>>
    where
        Self: 'visit,
    {
        let pipe = host.get_mut().0.as_mut().get_mut();
        if pipe.data.is_empty() && destination.has_remaining_mut() {
            pipe.readers.push(cx.waker().clone());
            return Poll::Pending;
        }
        let count = pipe.data.len().min(destination.remaining_mut());
        for byte in pipe.data.drain(..count) {
            destination.put_u8(byte);
        }
        Poll::Ready(Ok(count))
    }
}

/// A single-threaded async lock that counts acquisitions.
#[derive(Default)]
struct Lock<T> {
    value: RefCell<T>,
    waiters: RefCell<Vec<Waker>>,
    acquisitions: Cell<usize>,
}
struct Guard<'l, T> {
    value: Option<RefMut<'l, T>>,
    lock: &'l Lock<T>,
}
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.value = None;
        self.lock.waiters.take().into_iter().for_each(Waker::wake);
    }
}
impl<T> Lock<T> {
    fn try_lock(&self) -> Option<Guard<'_, T>> {
        let value = self.value.try_borrow_mut().ok()?;
        self.acquisitions.set(self.acquisitions.get() + 1);
        Some(Guard {
            value: Some(value),
            lock: self,
        })
    }
    fn lock(&self) -> impl Future<Output = Guard<'_, T>> {
        std::future::poll_fn(|cx| match self.try_lock() {
            Some(guard) => Poll::Ready(guard),
            None => {
                self.waiters.borrow_mut().push(cx.waker().clone());
                Poll::Pending
            }
        })
    }
}

/// Queues its call when woken, then wakes the executor.
struct CallWaker {
    index: usize,
    shared: Arc<Shared>,
}
#[derive(Default)]
struct Shared {
    ready: Mutex<VecDeque<usize>>,
    executor: Mutex<Option<Waker>>,
}
impl Shared {
    fn schedule(&self, index: usize) {
        let mut ready = self.ready.lock().unwrap();
        if !ready.contains(&index) {
            ready.push_back(index);
        }
        drop(ready);
        if let Some(waker) = self.executor.lock().unwrap().as_ref() {
            waker.wake_by_ref();
        }
    }
}
impl Wake for CallWaker {
    fn wake(self: Arc<Self>) {
        self.shared.schedule(self.index);
    }
}

type Record = Result<usize, ReadError<Infallible>>;
type Entry<'a> = Pin<Box<dyn DynCall<Direct<Pipe>, Infallible, Record> + 'a>>;

/// Calls sharing one locked pipe. Each pass takes the lock once, lends it to
/// at most `budget` ready calls, and releases it before returning.
struct Executor<'a> {
    lock: &'a Lock<Pipe>,
    calls: Vec<(Entry<'a>, Waker)>,
    results: Vec<Option<Record>>,
    shared: Arc<Shared>,
    budget: usize,
    /// Calls polled so far, in order.
    polled: Vec<usize>,
}
impl<'a> Executor<'a> {
    fn new(lock: &'a Lock<Pipe>, calls: Vec<Entry<'a>>, budget: usize) -> Self {
        let shared = Arc::new(Shared::default());
        let count = calls.len();
        let calls = calls
            .into_iter()
            .enumerate()
            .map(|(index, call)| {
                let waker = Waker::from(Arc::new(CallWaker {
                    index,
                    shared: shared.clone(),
                }));
                (call, waker)
            })
            .collect();
        shared.ready.lock().unwrap().extend(0..count);
        Self {
            lock,
            calls,
            results: (0..count).map(|_| None).collect(),
            shared,
            budget,
            polled: Vec::new(),
        }
    }
}
impl Future for Executor<'_> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        *this.shared.executor.lock().unwrap() = Some(cx.waker().clone());
        if this.shared.ready.lock().unwrap().is_empty() {
            return match this.results.iter().all(Option::is_some) {
                true => Poll::Ready(()),
                false => Poll::Pending,
            };
        }
        // The acquisition registers the executor's waker, not a call's.
        let Poll::Ready(mut guard) = pin!(this.lock.lock()).poll(cx) else {
            return Poll::Pending;
        };
        let pipe = guard.value.as_mut().unwrap();
        let mut scope = pin!(BorrowedScope::new(Pin::new(&mut **pipe)));
        for _ in 0..this.budget {
            let Some(index) = this.shared.ready.lock().unwrap().pop_front() else {
                break;
            };
            let (call, waker) = &mut this.calls[index];
            this.polled.push(index);
            let mut call_cx = Context::from_waker(waker);
            if let Poll::Ready(result) =
                call.as_mut().poll_return_with(scope.as_mut(), &mut call_cx)
            {
                this.results[index] = Some(result);
            }
        }
        drop(guard);
        if !this.shared.ready.lock().unwrap().is_empty() {
            cx.waker().wake_by_ref();
        }
        match this.results.iter().all(Option::is_some) {
            true => Poll::Ready(()),
            false => Poll::Pending,
        }
    }
}

#[call]
async fn read_record(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    output: &mut Vec<u8>,
) -> Record {
    let [length] = read_array::<1, _>(io).await?;
    read_exact(io, output, length.into()).await?;
    Ok(length.into())
}

/// Counts wakeups of the executor.
#[derive(Default)]
struct Count(AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn ready_calls_share_one_acquisition_after_the_lock_is_released() {
    let lock = Lock::<Pipe>::default();
    let (mut first, mut second) = (Vec::new(), Vec::new());
    {
        let calls: Vec<Entry> = vec![
            Box::pin(Call::new(ReadRecord::new(&mut first))),
            Box::pin(Call::new(ReadRecord::new(&mut second))),
        ];
        let mut executor = pin!(Executor::new(&lock, calls, usize::MAX));
        let wakes = Arc::new(Count::default());
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);

        // Another holder has the lock: no call is polled.
        let mut holder = lock.try_lock().unwrap();
        assert!(executor.as_mut().poll(&mut cx).is_pending());
        assert!(executor.polled.is_empty());

        // Releasing wakes the executor, which lends one acquisition to both.
        holder.value.as_mut().unwrap().push(b"\x01a\x01b");
        drop(holder);
        assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
        let before = lock.acquisitions.get();
        assert!(executor.as_mut().poll(&mut cx).is_ready());
        assert_eq!(lock.acquisitions.get(), before + 1);
        assert_eq!(executor.polled, [0, 1]);
    }
    assert_eq!(
        (first.as_slice(), second.as_slice()),
        (&b"a"[..], &b"b"[..])
    );
}

#[test]
fn waiting_calls_do_not_hold_the_lock_and_the_budget_bounds_a_pass() {
    let lock = Lock::<Pipe>::default();
    let (mut first, mut second) = (Vec::new(), Vec::new());
    {
        let calls: Vec<Entry> = vec![
            Box::pin(Call::new(ReadRecord::new(&mut first))),
            Box::pin(Call::new(ReadRecord::new(&mut second))),
        ];
        let mut executor = pin!(Executor::new(&lock, calls, 1));
        let wakes = Arc::new(Count::default());
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);

        // Each pass polls one call; the second pass is requested by the first.
        assert!(executor.as_mut().poll(&mut cx).is_pending());
        assert!(executor.as_mut().poll(&mut cx).is_pending());
        assert_eq!(executor.polled, [0, 1]);
        assert_eq!(lock.acquisitions.get(), 2);

        // Both wait on the pipe, not on the lock: another holder gets it at once.
        let mut holder = lock.try_lock().expect("the executor released the lock");
        holder.value.as_mut().unwrap().push(b"\x01x\x01y");
        drop(holder);
        assert_eq!(*executor.shared.ready.lock().unwrap(), [0, 1]);

        // The budget splits the ready calls over two acquisitions.
        assert!(executor.as_mut().poll(&mut cx).is_pending());
        assert!(executor.as_mut().poll(&mut cx).is_ready());
        assert_eq!(executor.polled, [0, 1, 0, 1]);
        assert_eq!(lock.acquisitions.get(), 5);
    }
    assert_eq!(
        (first.as_slice(), second.as_slice()),
        (&b"x"[..], &b"y"[..])
    );
}
