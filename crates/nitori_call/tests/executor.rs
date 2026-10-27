//! An outer executor that acquires an async lock only for calls that ask for
//! a view, then lends one view to all of them.
#![feature(impl_trait_in_assoc_type)]
mod support;

use bytes::BufMut;
use nitori_call::{
    Call, CallPoll, Direct, DynCall, Lend, Receiver, ReceiverAccess, call, direct_receiver,
};
use std::{
    cell::{Cell, RefCell, RefMut},
    collections::VecDeque,
    convert::Infallible,
    future::Future,
    future::poll_fn,
    pin::{Pin, pin},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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

/// Queues its call when woken, then wakes the executor unless it is polling,
/// in which case it sees the queue before returning.
struct CallWaker {
    index: usize,
    shared: Arc<Shared>,
}
#[derive(Default)]
struct Shared {
    ready: Mutex<VecDeque<usize>>,
    executor: Mutex<Option<Waker>>,
    polling: AtomicBool,
}
impl Shared {
    fn schedule(&self, index: usize) {
        let mut ready = self.ready.lock().unwrap();
        if !ready.contains(&index) {
            ready.push_back(index);
        }
        drop(ready);
        if self.polling.load(Ordering::SeqCst) {
            return;
        }
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

/// Calls sharing one locked pipe. Each pass polls at most `budget` ready
/// calls without a view. Only if some ask for one does it take the lock, once,
/// and lend one view to all of them before releasing it.
struct Executor<'a> {
    lock: &'a Lock<Pipe>,
    calls: Vec<(Entry<'a>, Waker)>,
    results: Vec<Option<Record>>,
    shared: Arc<Shared>,
    budget: usize,
    /// Calls that asked for a view, kept while the lock is unavailable.
    asking: Vec<usize>,
    /// Calls polled so far, in order, and whether they had a view.
    polled: Vec<(usize, bool)>,
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
            asking: Vec::new(),
            polled: Vec::new(),
        }
    }
}
impl Future for Executor<'_> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        *this.shared.executor.lock().unwrap() = Some(cx.waker().clone());
        this.shared.polling.store(true, Ordering::SeqCst);
        for _ in 0..this.budget {
            let Some(index) = this.shared.ready.lock().unwrap().pop_front() else {
                break;
            };
            let (call, waker) = &mut this.calls[index];
            this.polled.push((index, false));
            match call
                .as_mut()
                .poll_return_with(None, &mut Context::from_waker(waker))
            {
                CallPoll::Ready(result) => this.results[index] = Some(result),
                CallPoll::Pending => {}
                CallPoll::Releasable => unreachable!("a fixed view is not offered back"),
                CallPoll::NeedsView => {
                    // The executor serves the request itself: drop any wake
                    // that an executor inside the operation passed on.
                    this.shared
                        .ready
                        .lock()
                        .unwrap()
                        .retain(|&queued| queued != index);
                    this.asking.push(index);
                }
            }
        }
        if !this.asking.is_empty() {
            // The acquisition registers the executor's waker, not a call's.
            if let Poll::Ready(mut guard) = pin!(this.lock.lock()).poll(cx) {
                let pipe = guard.value.as_mut().unwrap();
                let mut view = pin!(Pin::new(&mut **pipe).view());
                for index in this.asking.drain(..) {
                    let (call, waker) = &mut this.calls[index];
                    this.polled.push((index, true));
                    let poll = call.as_mut().poll_return_with(
                        Some(Lend::Fixed(view.as_mut())),
                        &mut Context::from_waker(waker),
                    );
                    match poll {
                        CallPoll::Ready(result) => this.results[index] = Some(result),
                        CallPoll::Pending => {}
                        CallPoll::Releasable => unreachable!("a fixed view is not offered back"),
                        CallPoll::NeedsView => unreachable!(),
                    }
                }
            }
        }
        this.shared.polling.store(false, Ordering::SeqCst);
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
fn calls_asking_for_a_view_share_one_acquisition_after_the_lock_is_released() {
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

        // Another holder has the lock: both calls ask, and wait for the lock.
        let mut holder = lock.try_lock().unwrap();
        assert!(executor.as_mut().poll(&mut cx).is_pending());
        assert_eq!(executor.polled, [(0, false), (1, false)]);
        assert!(executor.shared.ready.lock().unwrap().is_empty());

        // The calls' own wakes for their requests did not reach the executor.
        assert_eq!(wakes.0.load(Ordering::SeqCst), 0);

        // Releasing wakes the executor, which lends one acquisition to both.
        holder.value.as_mut().unwrap().push(b"\x01a\x01b");
        drop(holder);
        assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
        let before = lock.acquisitions.get();
        assert!(executor.as_mut().poll(&mut cx).is_ready());
        assert_eq!(lock.acquisitions.get(), before + 1);
        assert_eq!(executor.polled[2..], [(0, true), (1, true)]);
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
        assert_eq!(
            executor.polled,
            [(0, false), (0, true), (1, false), (1, true)]
        );
        assert_eq!(lock.acquisitions.get(), 2);

        // Both wait on the pipe, not on the lock: another holder gets it at once.
        let mut holder = lock.try_lock().expect("the executor released the lock");
        holder.value.as_mut().unwrap().push(b"\x01x\x01y");
        drop(holder);
        assert_eq!(*executor.shared.ready.lock().unwrap(), [0, 1]);

        // The budget splits the ready calls over two acquisitions.
        assert!(executor.as_mut().poll(&mut cx).is_pending());
        assert!(executor.as_mut().poll(&mut cx).is_ready());
        assert_eq!(lock.acquisitions.get(), 5);
    }
    assert_eq!(
        (first.as_slice(), second.as_slice()),
        (&b"x"[..], &b"y"[..])
    );
}

/// A permission that opens later; waiting for it needs no resource.
#[derive(Default)]
struct Gate {
    open: Cell<bool>,
    waiter: RefCell<Option<Waker>>,
}
impl Gate {
    fn open(&self) {
        self.open.set(true);
        if let Some(waker) = self.waiter.take() {
            waker.wake();
        }
    }
    async fn wait(&self) {
        poll_fn(|cx| match self.open.get() {
            true => Poll::Ready(()),
            false => {
                *self.waiter.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

#[call]
async fn read_record_after(
    io: &mut impl ReceiverAccess<Family: Read<Error = Infallible>>,
    gate: &Gate,
    output: &mut Vec<u8>,
) -> Record {
    gate.wait().await;
    read_record(io, output).await
}

#[test]
fn waiting_for_something_else_acquires_nothing() {
    let lock = Lock::<Pipe>::default();
    lock.try_lock()
        .unwrap()
        .value
        .as_mut()
        .unwrap()
        .push(b"\x01z");
    let before = lock.acquisitions.get();
    let gate = Gate::default();
    let mut output = Vec::new();
    {
        let calls: Vec<Entry> = vec![Box::pin(Call::new(ReadRecordAfter::new(
            &gate,
            &mut output,
        )))];
        let mut executor = pin!(Executor::new(&lock, calls, usize::MAX));
        let mut cx = Context::from_waker(Waker::noop());

        assert!(executor.as_mut().poll(&mut cx).is_pending());
        assert_eq!(lock.acquisitions.get(), before);

        gate.open();
        assert!(executor.as_mut().poll(&mut cx).is_ready());
        assert_eq!(lock.acquisitions.get(), before + 1);
        assert_eq!(executor.polled, [(0, false), (0, false), (0, true)]);
    }
    assert_eq!(output, b"z");
}
