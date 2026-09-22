#![feature(coroutine_trait)]
#![allow(clippy::multiple_bound_locations)] // pin-project-lite generated projections

use futures_core::Stream;
use host_context::{
    AwaitOn, Bound, CallOn, Child, HasFamily, HostContext, HostFamily, MapSource, Source,
    ViewSource,
};
use std::{
    cell::Cell,
    future::Future,
    marker::{PhantomData, PhantomPinned},
    ops::CoroutineState,
    pin::{Pin, pin},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker, ready},
};
use tokio::sync::{Mutex, MutexGuard, mpsc};

struct Signal(AtomicUsize);
impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct Data<'data> {
    label: &'data str,
    bytes: Vec<u8>,
}
struct Locked<'data>(PhantomData<&'data str>);
struct Dropped<'a>(&'a Cell<usize>);
impl Drop for Dropped<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
pin_project_lite::pin_project! {
    struct View<'view, 'data> {
        guard: MutexGuard<'view, Data<'data>>,
        drop_count: Dropped<'view>,
        visits: usize,
        invariant: PhantomData<fn(&'view ()) -> &'view ()>,
        #[pin]
        pinned: PhantomPinned,
    }
}
impl<'data> HostFamily for Locked<'data> {
    type HostView<'view>
        = View<'view, 'data>
    where
        Self: 'view;
}
impl<'data> HasFamily for View<'_, 'data> {
    type Family = Locked<'data>;
}
// Capabilities operate directly on the existing view; no reconstruction method.
trait Write: HostFamily {
    fn poll_write<'view>(view: Pin<&mut Self::HostView<'view>>, bytes: &[u8]) -> usize
    where
        Self: 'view;
}
impl Write for Locked<'_> {
    fn poll_write<'view>(view: Pin<&mut Self::HostView<'view>>, bytes: &[u8]) -> usize
    where
        Self: 'view,
    {
        let this = view.project();
        *this.visits += 1;
        assert!(!this.guard.label.is_empty());
        let count = bytes.len().min(2);
        this.guard.bytes.extend_from_slice(&bytes[..count]);
        count
    }
}
fn source<'view, 'data: 'view>(
    lock: &'view Mutex<Data<'data>>,
    starts: &'view Cell<usize>,
    builds: &'view Cell<usize>,
    drops: &'view Cell<usize>,
) -> impl ViewSource<'view, Family = Locked<'data>> {
    Source::<Locked<'data>, _, _>::new(move || {
        starts.set(starts.get() + 1);
        async move {
            let guard = lock.lock().await;
            // An explicit post-acquisition initialization step, once per view.
            builds.set(builds.get() + 1);
            View {
                guard,
                drop_count: Dropped(drops),
                visits: 0,
                invariant: PhantomData,
                pinned: PhantomPinned,
            }
        }
    })
}

struct Pump {
    input: mpsc::UnboundedReceiver<Vec<u8>>,
    current: Option<Vec<u8>>,
    offset: usize,
    written: usize,
}
impl<'data> CallOn<Locked<'data>> for Pump {
    type Yield = std::convert::Infallible;
    type Return = usize;
    fn poll_call<'view>(
        self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'view, Family = Locked<'data>>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, usize>>
    where
        Locked<'data>: 'view,
    {
        let this = self.get_mut();
        loop {
            if this.current.is_none() {
                let Some(bytes) = ready!(this.input.poll_recv(cx)) else {
                    return Poll::Ready(CoroutineState::Complete(this.written));
                };
                this.current = Some(bytes);
                this.offset = 0;
            }
            let bytes = this.current.as_ref().unwrap();
            let view = ready!(context.as_mut().poll_view(cx));
            let count = Locked::poll_write(view, &bytes[this.offset..]);
            this.offset += count;
            this.written += count;
            if this.offset == bytes.len() {
                this.current = None;
            }
        }
    }
}

#[test]
fn async_input_does_not_acquire_writer_and_partial_writes_reuse_one_view() {
    let label = String::from("writer");
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let (starts, builds, drops) = (Cell::new(0), Cell::new(0), Cell::new(0));
    let (send, recv) = mpsc::unbounded_channel();
    let signal = Arc::new(Signal(AtomicUsize::new(0)));
    let waker = Waker::from(signal.clone());
    let mut cx = Context::from_waker(&waker);
    let mut call = pin!(Bound::new(
        source(&lock, &starts, &builds, &drops),
        Pump {
            input: recv,
            current: None,
            offset: 0,
            written: 0
        }
    ));
    let held = lock.try_lock().unwrap();
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert_eq!(starts.get(), 0);
    send.send(b"abcdef".to_vec()).unwrap();
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert_eq!(starts.get(), 1); // acquisition future survives both round drops
    assert_eq!(builds.get(), 0);
    let wakes = signal.0.load(Ordering::SeqCst);
    drop(held);
    assert!(signal.0.load(Ordering::SeqCst) > wakes);
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert_eq!(builds.get(), 1);
    assert_eq!(drops.get(), 1);
    assert_eq!(lock.try_lock().unwrap().bytes, b"abcdef");
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert_eq!(starts.get(), 1); // waiting for more input needs no view
    send.send(b"ghi".to_vec()).unwrap();
    assert!(call.as_mut().poll(&mut cx).is_pending());
    assert_eq!(starts.get(), 2);
    assert_eq!(builds.get(), 2);
    assert_eq!(drops.get(), 2);
    drop(send);
    assert_eq!(call.as_mut().poll(&mut cx), Poll::Ready(9));
    assert_eq!(starts.get(), 2);
    assert_eq!(lock.try_lock().unwrap().bytes, b"abcdefghi");
}

struct Events {
    remaining: usize,
}
impl<'data> CallOn<Locked<'data>> for Events {
    type Yield = usize;
    type Return = usize;
    fn poll_call<'view>(
        self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'view, Family = Locked<'data>>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<usize, usize>>
    where
        Locked<'data>: 'view,
    {
        let this = self.get_mut();
        let mut view = ready!(context.as_mut().poll_view(cx));
        Locked::poll_write(view.as_mut(), b"x");
        let visits = view.visits;
        this.remaining -= 1;
        Poll::Ready(if this.remaining == 0 {
            CoroutineState::Complete(visits)
        } else {
            CoroutineState::Yielded(visits)
        })
    }
}
#[test]
fn future_discards_yields_in_one_round_but_stream_events_end_the_round() {
    let label = String::from("events");
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let (starts, builds, drops) = (Cell::new(0), Cell::new(0), Cell::new(0));
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = pin!(Bound::new(
        source(&lock, &starts, &builds, &drops),
        Events { remaining: 3 }
    ));
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(3));
    assert_eq!((starts.get(), builds.get(), drops.get()), (1, 1, 1));
    let mut stream = pin!(Bound::new(
        source(&lock, &starts, &builds, &drops),
        Events { remaining: 3 }
    ));
    assert_eq!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(CoroutineState::Yielded(1)))
    );
    assert_eq!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(CoroutineState::Yielded(1)))
    );
    assert_eq!(
        stream.as_mut().poll_next(&mut cx),
        Poll::Ready(Some(CoroutineState::Complete(1)))
    );
    assert_eq!(stream.as_mut().poll_next(&mut cx), Poll::Ready(None));
    assert_eq!((starts.get(), builds.get(), drops.get()), (4, 4, 4));
}

struct StopWaiting {
    stop: Rc<Cell<bool>>,
    panic: bool,
}
impl<'data> CallOn<Locked<'data>> for StopWaiting {
    type Yield = std::convert::Infallible;
    type Return = ();
    fn poll_call<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = Locked<'data>>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, ()>>
    where
        Locked<'data>: 'view,
    {
        if self.stop.get() {
            assert!(!self.panic, "operation panicked");
            return Poll::Ready(CoroutineState::Complete(()));
        }
        let _ = ready!(context.poll_view(cx));
        Poll::Ready(CoroutineState::Complete(()))
    }
}
#[test]
fn completion_cancels_waiter_even_when_binding_is_retained() {
    let label = String::from("cancel");
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let (starts, builds, drops) = (Cell::new(0), Cell::new(0), Cell::new(0));
    let stop = Rc::new(Cell::new(false));
    let mut call = pin!(Bound::new(
        source(&lock, &starts, &builds, &drops),
        StopWaiting {
            stop: stop.clone(),
            panic: false
        }
    ));
    let mut cx = Context::from_waker(Waker::noop());
    let held = lock.try_lock().unwrap();
    assert!(call.as_mut().poll(&mut cx).is_pending());
    let mut follower = pin!(lock.lock());
    assert!(follower.as_mut().poll(&mut cx).is_pending());
    drop(held); // the first acquisition now has the queued permit
    stop.set(true);
    assert_eq!(call.as_mut().poll(&mut cx), Poll::Ready(()));
    assert!(follower.as_mut().poll(&mut cx).is_ready());
    assert_eq!(builds.get(), 0);
}
#[test]
fn panic_cancels_waiter_and_poisoned_binding_cannot_resume() {
    let label = String::from("panic");
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let (starts, builds, drops) = (Cell::new(0), Cell::new(0), Cell::new(0));
    let stop = Rc::new(Cell::new(false));
    let mut call = pin!(Bound::new(
        source(&lock, &starts, &builds, &drops),
        StopWaiting {
            stop: stop.clone(),
            panic: true
        }
    ));
    let mut cx = Context::from_waker(Waker::noop());
    let held = lock.try_lock().unwrap();
    assert!(call.as_mut().poll(&mut cx).is_pending());
    let mut follower = pin!(lock.lock());
    assert!(follower.as_mut().poll(&mut cx).is_pending());
    drop(held);
    stop.set(true);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call.as_mut().poll(&mut cx)))
            .is_err()
    );
    assert!(follower.as_mut().poll(&mut cx).is_ready());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call.as_mut().poll(&mut cx)))
            .is_err()
    );
}
#[test]
fn dropping_binding_cancels_queued_acquisition() {
    let label = String::from("drop");
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let (starts, builds, drops) = (Cell::new(0), Cell::new(0), Cell::new(0));
    let mut cx = Context::from_waker(Waker::noop());
    let held = lock.try_lock().unwrap();
    let mut follower = pin!(lock.lock());
    {
        let mut call = pin!(Bound::new(
            source(&lock, &starts, &builds, &drops),
            StopWaiting {
                stop: Rc::new(Cell::new(false)),
                panic: false
            }
        ));
        assert!(call.as_mut().poll(&mut cx).is_pending());
        assert!(follower.as_mut().poll(&mut cx).is_pending());
    }
    drop(held);
    assert!(follower.as_mut().poll(&mut cx).is_ready());
    assert_eq!(builds.get(), 0);
}

struct LayerFamily<F>(PhantomData<fn() -> F>);
pin_project_lite::pin_project! {
    struct LayerView<'view, F: HostFamily> where F: 'view {
        #[pin]
        inner: F::HostView<'view>,
        writes: &'view Cell<usize>,
    }
}
impl<F: HostFamily> HostFamily for LayerFamily<F> {
    type HostView<'view>
        = LayerView<'view, F>
    where
        Self: 'view;
}
impl<F: HostFamily> HasFamily for LayerView<'_, F> {
    type Family = LayerFamily<F>;
}
impl<F: Write> Write for LayerFamily<F> {
    fn poll_write<'view>(view: Pin<&mut Self::HostView<'view>>, bytes: &[u8]) -> usize
    where
        Self: 'view,
    {
        let this = view.project();
        this.writes.set(this.writes.get() + 1);
        F::poll_write(this.inner, bytes)
    }
}
struct WritePair;
impl<F: Write> CallOn<F> for WritePair {
    type Yield = std::convert::Infallible;
    type Return = ();
    fn poll_call<'view>(
        self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, ()>>
    where
        F: 'view,
    {
        let mut view = ready!(context.as_mut().poll_view(cx));
        F::poll_write(view.as_mut(), b"ab");
        F::poll_write(view.as_mut(), b"cd");
        Poll::Ready(CoroutineState::Complete(()))
    }
}
#[test]
fn preconfigured_nested_views_are_built_once_before_pinning() {
    let label = String::from("layers");
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let (starts, builds, drops) = (Cell::new(0), Cell::new(0), Cell::new(0));
    let (inner_builds, outer_builds, inner_writes, outer_writes) =
        (Cell::new(0), Cell::new(0), Cell::new(0), Cell::new(0));
    let inner = MapSource::<_, _, LayerFamily<Locked<'_>>>::new(
        source(&lock, &starts, &builds, &drops),
        |view| {
            inner_builds.set(inner_builds.get() + 1);
            LayerView {
                inner: view,
                writes: &inner_writes,
            }
        },
    );
    let outer = MapSource::<_, _, LayerFamily<LayerFamily<Locked<'_>>>>::new(inner, |view| {
        outer_builds.set(outer_builds.get() + 1);
        LayerView {
            inner: view,
            writes: &outer_writes,
        }
    });
    let mut call = pin!(Bound::new(outer, WritePair));
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(call.as_mut().poll(&mut cx), Poll::Ready(()));
    assert_eq!(
        (builds.get(), inner_builds.get(), outer_builds.get()),
        (1, 1, 1)
    );
    assert_eq!((inner_writes.get(), outer_writes.get()), (2, 2));
    assert_eq!(drops.get(), 1);
    assert_eq!(lock.try_lock().unwrap().bytes, b"abcd");
}

struct PanicFamily;
struct PanicView {
    panic_on_drop: Rc<Cell<bool>>,
}
impl HostFamily for PanicFamily {
    type HostView<'view> = PanicView;
}
impl HasFamily for PanicView {
    type Family = PanicFamily;
}
impl Drop for PanicView {
    fn drop(&mut self) {
        assert!(!self.panic_on_drop.replace(false), "view drop panicked");
    }
}
struct ViewThenPending(Rc<Cell<usize>>);
impl CallOn<PanicFamily> for ViewThenPending {
    type Yield = ();
    type Return = ();
    fn poll_call<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = PanicFamily>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<(), ()>>
    where
        PanicFamily: 'view,
    {
        self.0.set(self.0.get() + 1);
        let _ = ready!(context.poll_view(cx));
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
#[test]
fn view_destructor_panic_keeps_future_terminal() {
    let panic_on_drop = Rc::new(Cell::new(true));
    let polls = Rc::new(Cell::new(0));
    let source = Source::<PanicFamily, _, _>::new(|| {
        std::future::ready(PanicView {
            panic_on_drop: panic_on_drop.clone(),
        })
    });
    let mut call = pin!(Bound::new(source, ViewThenPending(polls.clone())));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call.as_mut().poll(&mut cx)))
            .is_err()
    );
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call.as_mut().poll(&mut cx)))
            .is_err()
    );
    assert_eq!(polls.get(), 1);
}

struct Bomb;
impl Drop for Bomb {
    fn drop(&mut self) {
        panic!("yield drop panicked");
    }
}
struct YieldBomb(Rc<Cell<usize>>);
impl CallOn<PanicFamily> for YieldBomb {
    type Yield = Bomb;
    type Return = ();
    fn poll_call<'view>(
        self: Pin<&mut Self>,
        _: Pin<&mut dyn HostContext<'view, Family = PanicFamily>>,
        _: &mut Context<'_>,
    ) -> Poll<CoroutineState<Bomb, ()>>
    where
        PanicFamily: 'view,
    {
        self.0.set(self.0.get() + 1);
        Poll::Ready(CoroutineState::Yielded(Bomb))
    }
}
struct NoAccess;
impl<'view> HostContext<'view> for NoAccess {
    type Family = PanicFamily;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        panic!("unexpected host access");
    }
    fn ready_view(self: Pin<&mut Self>) -> Pin<&mut PanicView> {
        panic!("unexpected host access");
    }
}
#[test]
fn discarded_child_yield_panic_keeps_child_terminal() {
    let polls = Rc::new(Cell::new(0));
    let mut child = pin!(Child::new(YieldBomb(polls.clone())));
    let mut context = pin!(NoAccess);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| child
            .as_mut()
            .poll_on(context.as_mut(), &mut cx)))
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| child
            .as_mut()
            .poll_on(context.as_mut(), &mut cx)))
        .is_err()
    );
    assert_eq!(polls.get(), 1);
}
