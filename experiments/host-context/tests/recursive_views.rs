#![feature(coroutine_trait)]
#![allow(clippy::multiple_bound_locations)] // pin-project-lite projections
//! A separate representation candidate: every layer retains its parent's type.
//! These private interfaces do not replace the library's family-indexed API.

use std::{
    cell::Cell,
    future::Future,
    marker::{PhantomData, PhantomPinned},
    ops::CoroutineState,
    pin::{Pin, pin},
    sync::{Mutex, MutexGuard},
    task::{Context, Poll, Waker, ready},
};

trait Family {}
trait View {
    type Family: Family;
}

// Capability implementations still belong to the logical family. Their view
// parameter now identifies the actual representation, including its ancestors.
trait ReadAt<V: View<Family = Self> + ?Sized>: Family {
    fn read(view: Pin<&mut V>) -> u8;
}
trait ReadView: View {
    fn read(self: Pin<&mut Self>) -> u8;
}
impl<V: View + ?Sized> ReadView for V
where
    V::Family: ReadAt<V>,
{
    fn read(self: Pin<&mut Self>) -> u8 {
        V::Family::read(self)
    }
}

trait HostContext {
    type View: View + ?Sized;
    // Ready is sticky and lends the same pinned value until this context drops.
    fn poll_view(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Pin<&mut Self::View>>;
    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.poll_view(cx).map(drop)
    }
}

// Both input borrows are type arguments, so their well-formedness carries the
// outlives relations. There is no Target::View<'scope> which erases the parent.
trait Project<Input> {
    type Output: View;
    fn project(self, input: Input) -> Self::Output;
}
type Target<'scope, R, V> = <Pin<&'scope mut R> as Project<Pin<&'scope mut V>>>::Output;

pin_project_lite::pin_project! {
    struct Projected<'scope, R, V: ?Sized>
    where
        V: View,
        Pin<&'scope mut R>: Project<Pin<&'scope mut V>>,
    {
        parent: Option<Pin<&'scope mut dyn HostContext<View = V>>>,
        receiver: Option<Pin<&'scope mut R>>,
        #[pin]
        view: Option<Target<'scope, R, V>>,
    }
}
impl<'scope, R, V: View + ?Sized> HostContext for Projected<'scope, R, V>
where
    Pin<&'scope mut R>: Project<Pin<&'scope mut V>>,
{
    type View = Target<'scope, R, V>;
    fn poll_view(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Pin<&mut Self::View>> {
        let mut this = self.project();
        if this.view.is_none() {
            ready!(this.parent.as_mut().unwrap().as_mut().poll_ready(cx));
            // The short readiness borrow ended. Consume the original loan to
            // preserve 'scope; the second poll only lends the cached parent.
            let Poll::Ready(parent) = this.parent.take().unwrap().poll_view(cx) else {
                panic!("context violated sticky readiness");
            };
            let receiver = this.receiver.take().unwrap();
            this.view.set(Some(receiver.project(parent)));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap())
    }
}

trait CallOnView<V: View + ?Sized> {
    fn poll_call(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<u8, u8>>;

    fn poll_return(
        mut self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8> {
        loop {
            match ready!(self.as_mut().poll_call(context.as_mut(), cx)) {
                CoroutineState::Yielded(_) => (),
                CoroutineState::Complete(value) => return Poll::Ready(value),
            }
        }
    }
}

// Canonical root representation; recursive intermediate views are not rebuilt
// from this mapping. The reference type supplies the outlives relation.
trait RootAt<Input>: Family {
    type View: View<Family = Self>;
}
trait RootFamily: Family + for<'view> RootAt<&'view Self> {}
type RootAtView<'view, F> = <F as RootAt<&'view F>>::View;
trait CallOn<F: RootFamily> {
    fn poll<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<View = RootAtView<'view, F>>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>
    where
        F: RootAt<&'view F>;
}
impl<F: RootFamily, O> CallOn<F> for O
where
    for<'view> O: CallOnView<RootAtView<'view, F>>,
{
    fn poll<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<View = RootAtView<'view, F>>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>
    where
        F: RootAt<&'view F>,
    {
        self.poll_return(context, cx)
    }
}

pin_project_lite::pin_project! {
    struct Received<R, O> {
        #[pin]
        receiver: R,
        #[pin]
        operation: O,
        terminal: bool,
    }
}
impl<R, O> Received<R, O> {
    fn new(receiver: R, operation: O) -> Self {
        Self {
            receiver,
            operation,
            terminal: false,
        }
    }
}
impl<V: View + ?Sized, R, O> CallOnView<V> for Received<R, O>
where
    for<'scope> Pin<&'scope mut R>: Project<Pin<&'scope mut V>>,
    for<'scope> O: CallOnView<Target<'scope, R, V>>,
{
    fn poll_call(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<u8, u8>> {
        let this = self.project();
        assert!(!*this.terminal);
        *this.terminal = true;
        let result = {
            let mut child = pin!(Projected {
                parent: Some(context),
                receiver: Some(this.receiver),
                view: None,
            });
            this.operation.poll_call(child.as_mut(), cx)
        };
        if !matches!(result, Poll::Ready(CoroutineState::Complete(_))) {
            *this.terminal = false;
        }
        result
    }
    fn poll_return(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8> {
        let this = self.project();
        assert!(!*this.terminal);
        *this.terminal = true;
        let result = {
            let mut child = pin!(Projected {
                parent: Some(context),
                receiver: Some(this.receiver),
                view: None,
            });
            this.operation.poll_return(child.as_mut(), cx)
        };
        if result.is_pending() {
            *this.terminal = false;
        }
        result
    }
}

#[derive(Default)]
struct Stats {
    polls: Cell<usize>,
    created: Cell<usize>,
    reads: Cell<usize>,
    dropped: Cell<usize>,
}
fn increment(value: &Cell<usize>) {
    value.set(value.get() + 1);
}
struct Root<'data>(PhantomData<&'data str>);
impl Family for Root<'_> {}
impl RootFamily for Root<'_> {}
impl<'view, 'data> RootAt<&'view Root<'data>> for Root<'data> {
    type View = RootView<'view, 'data>;
}
struct Counted<F: Family>(PhantomData<fn() -> F>);
impl<F: Family> Family for Counted<F> {}

struct RootView<'lock, 'data> {
    guard: MutexGuard<'lock, Cell<u8>>,
    label: &'data str,
    stats: &'data Stats,
    invariant: PhantomData<fn(&'lock ()) -> &'lock ()>,
    _pinned: PhantomPinned,
}
impl<'data> View for RootView<'_, 'data> {
    type Family = Root<'data>;
}
impl<'lock, 'data> ReadAt<RootView<'lock, 'data>> for Root<'data> {
    fn read(view: Pin<&mut RootView<'lock, 'data>>) -> u8 {
        assert_eq!(view.label, "borrowed family");
        increment(&view.stats.reads);
        let byte = view.guard.get();
        view.guard.set(byte + 1);
        byte
    }
}
impl Drop for RootView<'_, '_> {
    fn drop(&mut self) {
        increment(&self.stats.dropped);
    }
}

pin_project_lite::pin_project! {
    struct RootContext<'lock, 'data> {
        mutex: &'lock Mutex<Cell<u8>>,
        label: &'data str,
        stats: &'data Stats,
        pending_once: bool,
        #[pin]
        view: Option<RootView<'lock, 'data>>,
    }
}
impl<'lock, 'data> RootContext<'lock, 'data> {
    fn new(mutex: &'lock Mutex<Cell<u8>>, label: &'data str, stats: &'data Stats) -> Self {
        Self {
            mutex,
            label,
            stats,
            pending_once: false,
            view: None,
        }
    }
}
impl<'lock, 'data> HostContext for RootContext<'lock, 'data> {
    type View = RootView<'lock, 'data>;
    fn poll_view(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Pin<&mut Self::View>> {
        let mut this = self.project();
        increment(&this.stats.polls);
        if this.view.is_none() {
            if *this.pending_once {
                *this.pending_once = false;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            increment(&this.stats.created);
            this.view.set(Some(RootView {
                guard: this.mutex.lock().unwrap(),
                label: this.label,
                stats: this.stats,
                invariant: PhantomData,
                _pinned: PhantomPinned,
            }));
        }
        Poll::Ready(this.view.as_pin_mut().unwrap())
    }
}

struct Counter<'data>(&'data Stats);
pin_project_lite::pin_project! {
    struct Layer<'scope, 'data, V: ?Sized> {
        inner: Pin<&'scope mut V>,
        state: &'scope mut Counter<'data>,
        #[pin]
        pinned: PhantomPinned,
    }
    impl<V: ?Sized> PinnedDrop for Layer<'_, '_, V> {
        fn drop(this: Pin<&mut Self>) {
            increment(&this.state.0.dropped);
        }
    }
}
impl<V: View + ?Sized> View for Layer<'_, '_, V> {
    type Family = Counted<V::Family>;
}
impl<V: ReadView + ?Sized> ReadAt<Layer<'_, '_, V>> for Counted<V::Family> {
    fn read(view: Pin<&mut Layer<'_, '_, V>>) -> u8 {
        let this = view.project();
        increment(&this.state.0.reads);
        this.inner.as_mut().read()
    }
}
impl<'scope, 'data, V: View + ?Sized> Project<Pin<&'scope mut V>>
    for Pin<&'scope mut Counter<'data>>
{
    type Output = Layer<'scope, 'data, V>;
    fn project(self, input: Pin<&'scope mut V>) -> Self::Output {
        increment(&self.0.created);
        Layer {
            inner: input,
            state: self.get_mut(),
            pinned: PhantomPinned,
        }
    }
}

struct Burst(u8);
impl<V: ReadView + ?Sized> CallOnView<V> for Burst {
    fn poll_call(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<u8, u8>> {
        let value = ready!(context.poll_view(cx)).read();
        let this = self.get_mut();
        this.0 += 1;
        Poll::Ready(if this.0 == 3 {
            CoroutineState::Complete(value)
        } else {
            CoroutineState::Yielded(value)
        })
    }
}

pin_project_lite::pin_project! {
    struct InputThenRead<F> {
        input_done: bool,
        #[pin]
        input: F,
    }
}
impl<V: ReadView + ?Sized, F: Future<Output = ()>> CallOnView<V> for InputThenRead<F> {
    fn poll_call(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<u8, u8>> {
        let this = self.project();
        if !*this.input_done {
            ready!(this.input.poll(cx));
            *this.input_done = true;
        }
        Poll::Ready(CoroutineState::Complete(
            ready!(context.poll_view(cx)).read(),
        ))
    }
}

#[test]
fn recursive_nonstatic_views_cache_across_discarded_yields() {
    let label = String::from("borrowed family");
    let mutex = Mutex::new(Cell::new(7));
    let root = Stats::default();
    let layers = [Stats::default(), Stats::default(), Stats::default()];
    let mut operation = pin!(Received::new(
        Counter(&layers[0]),
        Received::new(
            Counter(&layers[1]),
            Received::new(Counter(&layers[2]), Burst(0)),
        )
    ));
    {
        let mut context = pin!(RootContext::new(&mutex, &label, &root));
        let erased: Pin<&mut dyn CallOn<Root<'_>>> = operation.as_mut();
        assert_eq!(
            erased.poll(context.as_mut(), &mut Context::from_waker(Waker::noop())),
            Poll::Ready(9)
        );
        for stats in &layers {
            assert_eq!(
                (stats.created.get(), stats.reads.get(), stats.dropped.get()),
                (1, 3, 1)
            );
        }
        assert_eq!(
            (root.created.get(), root.reads.get(), root.dropped.get()),
            (1, 3, 0)
        );
        assert!(mutex.try_lock().is_err());
    }
    assert_eq!(root.dropped.get(), 1);
    assert_eq!(mutex.try_lock().unwrap().get(), 10);
}

#[test]
fn ordinary_future_pending_leaves_every_context_uninitialized() {
    let label = String::from("borrowed family");
    let mutex = Mutex::new(Cell::new(7));
    let root = Stats::default();
    let layers = [Stats::default(), Stats::default(), Stats::default()];
    let mut pending = true;
    let input = std::future::poll_fn(move |cx| {
        if std::mem::take(&mut pending) {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    });
    let mut operation = pin!(Received::new(
        Counter(&layers[0]),
        Received::new(
            Counter(&layers[1]),
            Received::new(
                Counter(&layers[2]),
                InputThenRead {
                    input_done: false,
                    input
                }
            ),
        )
    ));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut context = pin!(RootContext::new(&mutex, &label, &root));
        assert_eq!(
            operation.as_mut().poll_return(context.as_mut(), &mut cx),
            Poll::Pending
        );
    }
    assert_eq!(
        (root.polls.get(), root.created.get(), root.dropped.get()),
        (0, 0, 0)
    );
    for stats in &layers {
        assert_eq!(stats.created.get(), 0);
    }
    {
        let mut context = pin!(RootContext::new(&mutex, &label, &root));
        assert_eq!(
            operation.as_mut().poll_return(context.as_mut(), &mut cx),
            Poll::Ready(7)
        );
    }
    for stats in &layers {
        assert_eq!((stats.created.get(), stats.dropped.get()), (1, 1));
    }
    assert_eq!((root.created.get(), root.dropped.get()), (1, 1));
    assert!(mutex.try_lock().is_ok());
}

#[test]
fn parent_pending_preserves_the_original_loans_for_retry() {
    let label = String::from("borrowed family");
    let mutex = Mutex::new(Cell::new(7));
    let root = Stats::default();
    let layer = Stats::default();
    let mut context = RootContext::new(&mutex, &label, &root);
    context.pending_once = true;
    let mut context = pin!(context);
    let mut receiver = pin!(Counter(&layer));
    let mut projected = pin!(Projected {
        parent: Some(context.as_mut()),
        receiver: Some(receiver.as_mut()),
        view: None,
    });
    let mut cx = Context::from_waker(Waker::noop());
    assert!(projected.as_mut().poll_view(&mut cx).is_pending());
    assert_eq!(layer.created.get(), 0);
    for expected in [7, 8] {
        let Poll::Ready(view) = projected.as_mut().poll_view(&mut cx) else {
            panic!("ready")
        };
        assert_eq!(view.read(), expected);
    }
    assert_eq!((root.created.get(), layer.created.get()), (1, 1));
}

pin_project_lite::pin_project! {
    struct ReadInputRead<F> {
        read_first: bool,
        input_done: bool,
        #[pin]
        input: F,
    }
}
impl<V: ReadView + ?Sized, F: Future<Output = ()>> CallOnView<V> for ReadInputRead<F> {
    fn poll_call(
        self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<u8, u8>> {
        let this = self.project();
        if !*this.read_first {
            assert_eq!(ready!(context.as_mut().poll_view(cx)).read(), 7);
            *this.read_first = true;
        }
        if !*this.input_done {
            ready!(this.input.poll(cx));
            *this.input_done = true;
        }
        Poll::Ready(CoroutineState::Complete(
            ready!(context.poll_view(cx)).read(),
        ))
    }
}

#[test]
fn ordinary_future_pending_after_read_releases_all_views() {
    let label = String::from("borrowed family");
    let mutex = Mutex::new(Cell::new(7));
    let root = Stats::default();
    let layers = [Stats::default(), Stats::default(), Stats::default()];
    let mut pending = true;
    let input = std::future::poll_fn(move |cx| {
        if std::mem::take(&mut pending) {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    });
    let mut operation = pin!(Received::new(
        Counter(&layers[0]),
        Received::new(
            Counter(&layers[1]),
            Received::new(
                Counter(&layers[2]),
                ReadInputRead {
                    read_first: false,
                    input_done: false,
                    input
                }
            ),
        )
    ));
    let mut cx = Context::from_waker(Waker::noop());
    // One Family-only dynamic object survives both distinct root rounds.
    let mut erased: Pin<&mut dyn CallOn<Root<'_>>> = operation.as_mut();
    {
        let mut context = pin!(RootContext::new(&mutex, &label, &root));
        assert_eq!(
            erased.as_mut().poll(context.as_mut(), &mut cx),
            Poll::Pending
        );
    }
    assert_eq!(
        (root.created.get(), root.reads.get(), root.dropped.get()),
        (1, 1, 1)
    );
    for stats in &layers {
        assert_eq!((stats.created.get(), stats.dropped.get()), (1, 1));
    }
    // No loan or lock is retained inside the suspended operation.
    assert_eq!(mutex.try_lock().unwrap().get(), 8);
    {
        let mut context = pin!(RootContext::new(&mutex, &label, &root));
        assert_eq!(
            erased.as_mut().poll(context.as_mut(), &mut cx),
            Poll::Ready(8)
        );
    }
    assert_eq!(
        (root.created.get(), root.reads.get(), root.dropped.get()),
        (2, 2, 2)
    );
    for stats in &layers {
        assert_eq!(
            (stats.created.get(), stats.reads.get(), stats.dropped.get()),
            (2, 2, 2)
        );
    }
    assert_eq!(mutex.try_lock().unwrap().get(), 9);
}

#[test]
fn ready_input_is_not_polled_again_after_host_pending() {
    let label = String::from("borrowed family");
    let mutex = Mutex::new(Cell::new(7));
    let root = Stats::default();
    let layer = Stats::default();
    let polls = Cell::new(0);
    let input = std::future::poll_fn(|_| {
        increment(&polls);
        assert_eq!(polls.get(), 1, "completed future polled again");
        Poll::Ready(())
    });
    let mut operation = pin!(Received::new(
        Counter(&layer),
        InputThenRead {
            input_done: false,
            input
        }
    ));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut context = RootContext::new(&mutex, &label, &root);
        context.pending_once = true;
        let mut context = pin!(context);
        assert_eq!(
            operation.as_mut().poll_return(context.as_mut(), &mut cx),
            Poll::Pending
        );
    }
    assert_eq!(
        (polls.get(), root.created.get(), layer.created.get()),
        (1, 0, 0)
    );
    {
        let mut context = pin!(RootContext::new(&mutex, &label, &root));
        assert_eq!(
            operation.as_mut().poll_return(context.as_mut(), &mut cx),
            Poll::Ready(7)
        );
    }
    assert_eq!(
        (polls.get(), root.created.get(), layer.created.get()),
        (1, 1, 1)
    );
}
