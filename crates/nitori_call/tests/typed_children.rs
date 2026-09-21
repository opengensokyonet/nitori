#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
use nitori_call::{CallOn, call, run_sync};
use nitori_call::{PollCallExt as _, ReceiverExt as _};
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
async fn decode(io: nitori_call::Receiver<Host>, base: usize) -> usize {
    let local = [base, base + 1];
    let borrowed = &local;
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.count.set(host.count() + 1)
    })
    .await;
    yield borrowed[0];
    Pause(false).await;
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.count.set(host.count() + 1)
    })
    .await;
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
async fn stream(io: nitori_call::Receiver<Host>) -> usize {
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
                io.with(|__access| {
                    let host = __access.into_pin().get_mut().0.as_mut();
                    host.count.set(host.count() + 10)
                })
                .await;
                yield value;
            }
            CoroutineState::Complete(value) => completion = value,
        }
    }
    completion
        + Custom.await
        + io.with(|access| access.into_pin().as_ref().get_ref().0.count())
            .await
}
#[call]
async fn discard(io: nitori_call::Receiver<Host>) -> usize {
    let child = io.decode(3);
    child.await
}
#[call]
async fn direct(io: nitori_call::Receiver<Host>) -> usize {
    io.decode(4).await
}
#[call(yields=usize)]
async fn inline_pin(io: nitori_call::Receiver<Host>) -> usize {
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
async fn input_buffer<'data>(io: nitori_call::Receiver<Host>, data: &'data mut [u8]) -> usize {
    data[0] = 7;
    Pause(false).await;
    data[1] = 9;
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.count()
    })
    .await
}
#[call]
async fn local_input(io: nitori_call::Receiver<Host>) -> [u8; 2] {
    let mut buffer = [0; 2];
    let view = buffer.as_mut_slice();
    let child = io.input_buffer(view);
    identity(child).await;
    buffer
}
#[call(sync)]
async fn instant(io: nitori_call::Receiver<Host>) -> usize {
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.count()
    })
    .await
}
#[call(sync)]
async fn synchronous(io: nitori_call::Receiver<Host>) -> usize {
    let value = io.instant().await;
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.count.set(value + 1)
    })
    .await;
    io.with(|access| access.into_pin().as_ref().get_ref().0.count())
        .await
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
    op.poll_host(host, &mut Context::from_waker(waker))
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
    assert_eq!(host.as_mut().sync_instant(), 1);
}

#[call(sync, yields = usize)]
async fn sync_events(io: nitori_call::Receiver<Host>) -> usize {
    yield io.instant().await;
    io.with(|access| access.into_pin().as_ref().get_ref().0.count())
        .await
}
#[call(sync)]
async fn generic_echo<T: Copy>(io: nitori_call::Receiver<Host>, value: T) -> T {
    value
}
#[call(sync)]
async fn generic_sync_caller(io: nitori_call::Receiver<Host>) -> usize {
    io.generic_echo(42usize).await
}
#[test]
fn generated_receiver_entries_support_reborrows_and_sync_events() {
    let mut host = pin!(host());
    let mut receiver = host.as_mut();
    assert_eq!(receiver.as_mut().sync_instant(), 0);
    assert_eq!(receiver.as_mut().sync_generic_echo(7usize), 7);
    assert_eq!(receiver.as_mut().sync_generic_sync_caller(), 42);
    {
        let mut future = pin!(receiver.as_mut().decode(10));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(12));
    }
    let mut events = pin!(receiver.as_mut().sync_sync_events());
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(2)));
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Complete(2)));
    assert_eq!(events.as_mut().next(), None);
}
// Returns a real host-free child through a normal function/closure boundary.
#[call]
async fn escaped(
    io: nitori_call::Receiver<Host>,
) -> nitori_call::Child<Host, nitori_call::Routed<nitori_call::Receiver<Host>, Decode>> {
    let value: nitori_call::Child<Host, nitori_call::Routed<nitori_call::Receiver<Host>, Decode>> =
        io.decode(8);
    value
}
#[call]
async fn reentered(io: nitori_call::Receiver<Host>) -> usize {
    let value = io.escaped().await;
    let factory = move || value;
    factory().await
}

#[test]
fn higher_order_call_returns_an_owned_child() {
    check_discard(Reentered::new(), 10);
}

#[call]
async fn input_factory<'data>(
    io: nitori_call::Receiver<Host>,
    data: &'data mut [u8],
) -> nitori_call::Child<Host, nitori_call::Routed<nitori_call::Receiver<Host>, InputBuffer<'data>>>
{
    io.input_buffer(data)
}
#[call]
async fn higher_input(io: nitori_call::Receiver<Host>) -> [u8; 2] {
    let mut buffer = [0; 2];
    let child = io.input_factory(&mut buffer).await;
    child.await;
    buffer
}
#[call(yields = nitori_call::Child<Host, nitori_call::Routed<nitori_call::Receiver<Host>, Decode>>)]
async fn children(io: nitori_call::Receiver<Host>) {
    yield io.decode(1);
    yield io.decode(4);
}
#[call]
async fn flattened(io: nitori_call::Receiver<Host>) -> usize {
    let mut source = pin!(io.children());
    let mut sum = 0;
    while let Some(event) = source.as_mut().next().await {
        match event {
            CoroutineState::Yielded(child) => sum += child.await,
            CoroutineState::Complete(()) => {}
        }
    }
    sum
}
#[test]
fn returned_child_keeps_borrowed_input_and_yielded_children_can_be_awaited() {
    let mut host = pin!(host());
    let mut input = pin!(HigherInput::new());
    assert_eq!(
        poll(input.as_mut(), host.as_mut(), Waker::noop()),
        Poll::Pending
    );
    assert_eq!(
        poll(input.as_mut(), host.as_mut(), Waker::noop()),
        Poll::Ready(CoroutineState::Complete([7, 9]))
    );
    let mut operation = pin!(Flattened::new());
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), Waker::noop()),
        Poll::Pending
    );
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), Waker::noop()),
        Poll::Pending
    );
    assert_eq!(
        poll(operation.as_mut(), host.as_mut(), Waker::noop()),
        Poll::Ready(CoroutineState::Complete(9))
    );
    assert_eq!(host.count(), 4);
}

#[call(sync, yields = usize)]
async fn counter_events(io: nitori_call::Receiver<Host>) -> usize {
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.count.set(host.count() + 1)
    })
    .await;
    yield io
        .with(|access| access.into_pin().as_ref().get_ref().0.count())
        .await;
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        host.count.set(host.count() + 1)
    })
    .await;
    yield io
        .with(|access| access.into_pin().as_ref().get_ref().0.count())
        .await;
    io.with(|access| access.into_pin().as_ref().get_ref().0.count())
        .await
}

#[call(sync, yields = usize)]
async fn relay_sync_events(io: nitori_call::Receiver<Host>) -> usize {
    let mut child = pin!(io.counter_events());
    while let Some(event) = child.as_mut().next().await {
        match event {
            CoroutineState::Yielded(value) => {
                io.with(|__access| {
                    let host = __access.into_pin().get_mut().0.as_mut();
                    host.count.set(host.count() + 10)
                })
                .await;
                yield value;
            }
            CoroutineState::Complete(value) => return value,
        }
    }
    0
}

#[call(sync)]
async fn collect_sync_events(io: nitori_call::Receiver<Host>) -> Vec<CoroutineState<usize, usize>> {
    io.with(|__access| {
        let host = __access.into_pin().get_mut().0.as_mut();
        {
            let mut events = std::pin::pin!(host.sync_counter_events());
            events.as_mut().collect()
        }
    })
    .await
}

#[test]
fn sync_yielding_children_are_lazy_and_allow_parent_host_access() {
    let mut host = pin!(host());
    let count = host.count.clone();
    let mut receiver = host.as_mut();
    // UFCS also verifies the public Receiver<Name>Ext spelling.
    let mut events = pin!(RelaySyncEventsExt::sync_relay_sync_events(
        receiver.as_mut()
    ));
    assert_eq!(count.get(), 0);
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(1)));
    assert_eq!(count.get(), 11);
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Yielded(12)));
    assert_eq!(count.get(), 22);
    assert_eq!(events.as_mut().next(), Some(CoroutineState::Complete(22)));
    assert_eq!(events.as_mut().next(), None);
}

#[test]
fn with_can_consume_a_sync_event_binding_and_return_owned_events() {
    let mut host = pin!(host());
    assert_eq!(
        run_sync(host.as_mut(), CollectSyncEvents::new()),
        vec![
            CoroutineState::Yielded(1),
            CoroutineState::Yielded(2),
            CoroutineState::Complete(2),
        ]
    );
    assert_eq!(host.count(), 2);
}

nitori_call::family_host!(impl [] for Host);
