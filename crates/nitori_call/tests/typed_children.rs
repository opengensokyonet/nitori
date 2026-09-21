#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{CallOn, Receiver, call, run_sync};
use std::{
    cell::Cell,
    convert::Infallible,
    future::{Future, IntoFuture, Ready, ready},
    marker::PhantomPinned,
    ops::CoroutineState,
    pin::{Pin, pin},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

struct Host {
    count: Rc<Cell<usize>>,
    _pin: PhantomPinned,
}
impl Host {
    fn count(&self) -> usize {
        self.count.get()
    }
}
struct Pause(bool);
impl Future for Pause {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
#[call(yields=usize)]
async fn decode(io: Pin<&mut Host>, base: usize) -> usize {
    let local = [base, base + 1];
    let borrowed = &local;
    io.with(|host| host.count.set(host.count() + 1));
    yield borrowed[0];
    Pause(false).await;
    io.with(|host| host.count.set(host.count() + 1));
    yield borrowed[1];
    base + 2
}
fn identity<T>(value: T) -> T {
    value
}
struct Custom;
impl IntoFuture for Custom {
    type Output = usize;
    type IntoFuture = Ready<usize>;
    fn into_future(self) -> Self::IntoFuture {
        ready(5)
    }
}
#[call(yields=usize)]
async fn stream(io: Receiver<'_, Host>) -> usize {
    let (first, second) = (io.decode(10), io.decode(20));
    let mut container = Some(if ready(true).await { first } else { second });
    let selected = identity(container.take().unwrap());
    let factory = move || selected;
    let selected = factory();
    let mut child = pin!(selected);
    // These are ordinary Rust local values and method calls, not recognized names.
    let borrowed = child.as_mut();
    let next = borrowed.next();
    let event = identity(next).await;
    match event {
        Some(CoroutineState::Yielded(value)) => {
            yield value;
        }
        _ => return 1000,
    }
    let mut completion = 0;
    while let Some(event) = child.as_mut().next().await {
        match event {
            CoroutineState::Yielded(value) => {
                io.with(|host| host.count.set(host.count() + 10));
                yield value;
            }
            CoroutineState::Complete(value) => completion = value,
        }
    }
    completion + Custom.await + io.as_ref().count()
}
#[call]
async fn discard(io: Receiver<'_, Host>) -> usize {
    let child = io.decode(3);
    child.await
}
#[call]
async fn direct(io: Receiver<'_, Host>) -> usize {
    io.decode(4).await
}
#[call(yields=usize)]
async fn inline_pin(io: Receiver<'_, Host>) -> usize {
    let mut child = std::pin::pin!(io.decode(6));
    while let Some(event) = child.as_mut().next().await {
        match event {
            CoroutineState::Yielded(value) => {
                yield value;
            }
            CoroutineState::Complete(value) => return value,
        }
    }
    0
}
#[call]
async fn input_buffer<'data>(io: Pin<&mut Host>, data: &'data mut [u8]) -> usize {
    data[0] = 7;
    Pause(false).await;
    data[1] = 9;
    io.with(|host| host.count())
}
#[call]
async fn local_input(io: Receiver<'_, Host>) -> [u8; 2] {
    let mut buffer = [0; 2];
    let view = buffer.as_mut_slice();
    let child = io.input_buffer(view);
    identity(child).await;
    buffer
}
#[call(sync)]
async fn instant(io: Pin<&mut Host>) -> usize {
    io.with(|host| host.count())
}
#[call(sync)]
async fn synchronous(io: Receiver<'_, Host>) -> usize {
    let value = io.sync_instant();
    io.with(|host| host.count.set(value + 1));
    io.as_ref().count()
}
struct Counter(AtomicUsize);
impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn poll<O: CallOn<Host>>(
    op: Pin<&mut O>,
    host: Pin<&mut Host>,
    waker: &Waker,
) -> Poll<CoroutineState<O::Yield, O::Return>> {
    op.poll_call(host, &mut Context::from_waker(waker))
}
fn host() -> Host {
    Host {
        count: Rc::new(Cell::new(0)),
        _pin: PhantomPinned,
    }
}
#[test]
fn events_are_pulled_without_prefetch_and_resume_with_fresh_host_access() {
    let mut host = pin!(host());
    let count = host.count.clone();
    let mut operation = pin!(Stream::new());
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    let waker = Waker::from(counter.clone());
    assert_eq!(count.get(), 0);
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), &waker),
        Poll::Ready(CoroutineState::Yielded(10))
    );
    assert_eq!(count.get(), 1);
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), &waker),
        Poll::Pending
    );
    assert_eq!(count.get(), 1);
    assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), &waker),
        Poll::Ready(CoroutineState::Yielded(11))
    );
    assert_eq!(count.get(), 12);
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), &waker),
        Poll::Ready(CoroutineState::Complete(29))
    );
}
fn check_discard<O: CallOn<Host, Yield = Infallible, Return = usize>>(
    operation: O,
    expected: usize,
) {
    let mut host = pin!(host());
    let mut operation = pin!(operation);
    let waker = Waker::noop();
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), waker),
        Poll::Pending
    );
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), waker),
        Poll::Ready(CoroutineState::Complete(expected))
    );
    assert_eq!(host.count(), 2);
}
#[test]
fn direct_and_moved_await_discard_child_yields() {
    check_discard(Discard::new(), 5);
    check_discard(Direct::new(), 6);
}
#[test]
fn cancellation_preserves_only_consumed_prefix() {
    let mut host = pin!(host());
    let mut operation = Box::pin(InlinePin::new());
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), Waker::noop()),
        Poll::Ready(CoroutineState::Yielded(6))
    );
    drop(operation);
    assert_eq!(host.count(), 1);
}
#[test]
fn mutable_local_input_survives_nested_wait() {
    let mut host = pin!(host());
    let mut operation = pin!(LocalInput::new());
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), Waker::noop()),
        Poll::Pending
    );
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), Waker::noop()),
        Poll::Ready(CoroutineState::Complete([7, 9]))
    );
}
#[test]
fn sync_and_receiver_access() {
    let mut host = pin!(host());
    assert_eq!(run_sync(host.as_mut(), Synchronous::new()), 1);
    let mut receiver = Receiver::from_pin(host.as_mut());
    assert_eq!(receiver.as_ref().count(), 1);
    receiver.with(|host| host.count.set(2));
    assert_eq!(receiver.as_mut().count(), 2);
    let mut number = 1;
    let mut receiver = Receiver::from_mut(&mut number);
    *receiver.as_mut() = 3;
    assert_eq!(*receiver.as_ref(), 3);
}

#[call(sync, yields = usize)]
async fn sync_events(io: Receiver<'_, Host>) -> usize {
    yield io.sync_instant();
    io.as_ref().count()
}
#[call(sync)]
async fn generic_echo<T: Copy>(io: Receiver<'_, Host>, value: T) -> T {
    value
}
#[call(sync)]
async fn generic_sync_caller(io: Receiver<'_, Host>) -> usize {
    io.sync_generic_echo(42usize)
}
#[test]
fn generated_receiver_entries_support_reborrows_and_sync_events() {
    let mut host = pin!(host());
    let mut receiver = Receiver::from_pin(host.as_mut());
    assert_eq!(receiver.sync_instant(), 0);
    assert_eq!(receiver.sync_generic_echo(7usize), 7);
    assert_eq!(receiver.sync_generic_sync_caller(), 42);
    {
        let mut future = pin!(receiver.decode(10));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(12));
    }
    let mut events = pin!(receiver.sync_sync_events());
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(2)));
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Complete(2)));
    assert_eq!(events.as_mut().next(), None);
}
