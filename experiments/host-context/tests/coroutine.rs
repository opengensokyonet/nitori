#![feature(coroutine_trait, coroutines, stmt_expr_attributes)]

use host_context::{
    AwaitOn, Bound, CallOn, Child, HasFamily, HostContext, HostFamily, Source,
    coroutine::{ResumeEnv, Suspend, build},
};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    ops::CoroutineState,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker, ready},
};

// Family and view are deliberately !Send. Only actual persistent coroutine
// captures, not transient dispatch to these resources, determine its Send bound.
struct CounterFamily {
    _not_send: Rc<()>,
}

impl HostFamily for CounterFamily {
    type HostView<'view> = CounterView;
}

#[derive(Default)]
struct Observed {
    requests: usize,
    constructions: usize,
    addresses: Vec<usize>,
    writes: Vec<usize>,
}

struct CounterView {
    observed: Rc<RefCell<Observed>>,
}

impl HasFamily for CounterView {
    type Family = CounterFamily;
}

struct CounterContext {
    observed: Rc<RefCell<Observed>>,
    view: Option<CounterView>,
    pending_once: bool,
}

impl CounterContext {
    fn new(observed: Rc<RefCell<Observed>>, pending_once: bool) -> Self {
        Self {
            observed,
            view: None,
            pending_once,
        }
    }
}

impl<'view> HostContext<'view> for CounterContext {
    type Family = CounterFamily;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        this.observed.borrow_mut().requests += 1;
        if std::mem::take(&mut this.pending_once) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.view.get_or_insert_with(|| {
            this.observed.borrow_mut().constructions += 1;
            CounterView {
                observed: this.observed.clone(),
            }
        });
        Poll::Ready(())
    }

    fn ready_view(self: Pin<&mut Self>) -> Pin<&mut CounterView> {
        Pin::new(self.get_mut().view.as_mut().expect("view is not ready"))
    }
}

struct Write(usize);

impl AwaitOn<CounterFamily> for Write {
    type Output = ();

    fn poll_on<'view>(
        self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'view, Family = CounterFamily>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output>
    where
        CounterFamily: 'view,
    {
        let view = ready!(context.as_mut().poll_view(cx));
        let address = &*view as *const CounterView as usize;
        let mut observed = view.observed.borrow_mut();
        observed.addresses.push(address);
        observed.writes.push(self.0);
        Poll::Ready(())
    }
}

struct PendingTwice(u8);

impl Future for PendingTwice {
    type Output = usize;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.0 < 2 {
            self.0 += 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(7)
        }
    }
}

// Hand-written expansion of the await portion of a call macro. The environment
// is consumed before every suspension, and every resume supplies a fresh token.
macro_rules! await_on {
    ($env:ident, $value:expr) => {{
        let mut awaited = pin!($value);
        loop {
            // SAFETY: this is the current resume's exclusive token; poll does
            // not retain it, and Pending consumes it before yielding.
            match unsafe { $env.poll_await(awaited.as_mut()) } {
                Poll::Ready(value) => break value,
                Poll::Pending => {
                    $env.end();
                    $env = yield Suspend::Pending;
                }
            }
        }
    }};
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn source_waits_do_not_request_view_and_child_reuses_it() {
    let state = #[coroutine]
    static move |mut env: ResumeEnv<CounterFamily>| {
        let value = await_on!(env, PendingTwice(0));
        let child_state = #[coroutine]
        static move |mut env: ResumeEnv<CounterFamily>| {
            await_on!(env, Write(value));
            env.end();
            env = yield Suspend::Emit("child event");
            await_on!(env, Write(value + 1));
            env.end();
            23
        };
        // SAFETY: the inner state only uses current-resume tokens, consumes
        // them before suspension/completion, and never exposes them.
        let child = unsafe { build(child_state) };
        let returned = await_on!(env, Child::<CounterFamily, _>::new(child));
        await_on!(env, Write(returned));
        env.end();
        42
    };
    // SAFETY: every environment path obeys the same token contract above.
    let operation = unsafe { build::<CounterFamily, _, ()>(state) };
    assert_send(&operation);
    let mut operation = pin!(operation);
    let observed = Rc::new(RefCell::new(Observed::default()));
    let mut context = pin!(CounterContext::new(observed.clone(), false));
    let mut cx = Context::from_waker(Waker::noop());

    for _ in 0..2 {
        assert!(
            operation
                .as_mut()
                .poll_call(context.as_mut(), &mut cx)
                .is_pending()
        );
        assert_eq!(observed.borrow().requests, 0);
        assert_eq!(observed.borrow().constructions, 0);
    }

    assert_eq!(
        operation.as_mut().poll_call(context.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Complete(42))
    );
    let observed = observed.borrow();
    assert_eq!(observed.writes, [7, 8, 23]);
    assert_eq!(observed.requests, 3);
    assert_eq!(observed.constructions, 1);
    assert!(observed.addresses.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn pending_acquisition_resumes_without_repeating_prior_effects() {
    let entries = Rc::new(Cell::new(0));
    let count = entries.clone();
    let state = #[coroutine]
    static move |mut env: ResumeEnv<CounterFamily>| {
        count.set(count.get() + 1);
        await_on!(env, Write(17));
        env.end();
    };
    // SAFETY: the state confines and consumes each token in its own resume.
    let mut operation = pin!(unsafe { build::<CounterFamily, _, ()>(state) });
    let observed = Rc::new(RefCell::new(Observed::default()));
    let mut context = pin!(CounterContext::new(observed.clone(), true));
    let mut cx = Context::from_waker(Waker::noop());

    assert!(
        operation
            .as_mut()
            .poll_call(context.as_mut(), &mut cx)
            .is_pending()
    );
    assert_eq!(entries.get(), 1);
    assert!(observed.borrow().writes.is_empty());
    assert_eq!(
        operation.as_mut().poll_call(context.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Complete(()))
    );
    assert_eq!(entries.get(), 1);
    assert_eq!(observed.borrow().writes, [17]);
}

#[test]
fn visible_yield_releases_resume_token_before_using_a_new_context() {
    let state = #[coroutine]
    static move |mut env: ResumeEnv<CounterFamily>| {
        await_on!(env, Write(1));
        env.end();
        env = yield Suspend::Emit("event");
        await_on!(env, Write(2));
        env.end();
    };
    // SAFETY: the yield consumes the old environment and installs a new one.
    let mut operation = pin!(unsafe { build(state) });
    let mut cx = Context::from_waker(Waker::noop());
    let first = Rc::new(RefCell::new(Observed::default()));
    {
        let mut context = pin!(CounterContext::new(first.clone(), false));
        assert_eq!(
            operation.as_mut().poll_call(context.as_mut(), &mut cx),
            Poll::Ready(CoroutineState::Yielded("event"))
        );
    }
    // The same logical resource is made available through a fresh context.
    // Reacquisition must be observed instead of using the expired first slot.
    let mut context = pin!(CounterContext::new(first.clone(), true));
    assert!(
        operation
            .as_mut()
            .poll_call(context.as_mut(), &mut cx)
            .is_pending()
    );
    assert_eq!(first.borrow().writes, [1]);
    assert_eq!(
        operation.as_mut().poll_call(context.as_mut(), &mut cx),
        Poll::Ready(CoroutineState::Complete(()))
    );
    assert_eq!(first.borrow().writes, [1, 2]);
    assert_eq!(first.borrow().constructions, 2);
}

#[test]
fn panicking_await_poisons_the_call_without_requesting_a_view() {
    let entries = Rc::new(Cell::new(0));
    let count = entries.clone();
    let state = #[coroutine]
    static move |mut env: ResumeEnv<CounterFamily>| {
        await_on!(
            env,
            std::future::poll_fn(move |_| -> Poll<()> {
                count.set(count.get() + 1);
                panic!("await panic")
            })
        );
        env.end();
    };
    // SAFETY: the token is never accessed during unwinding or destruction.
    let mut operation = pin!(unsafe { build::<CounterFamily, _, ()>(state) });
    let observed = Rc::new(RefCell::new(Observed::default()));
    let mut context = pin!(CounterContext::new(observed.clone(), false));
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..2 {
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                operation.as_mut().poll_call(context.as_mut(), &mut cx)
            }))
            .is_err()
        );
    }
    assert_eq!(entries.get(), 1);
    assert_eq!(observed.borrow().requests, 0);
}

#[test]
fn suspended_call_can_move_threads_without_its_non_send_context() {
    let state = #[coroutine]
    static move |mut env: ResumeEnv<CounterFamily>| {
        let value = await_on!(env, PendingTwice(0));
        await_on!(env, Write(value));
        env.end();
        value
    };
    // SAFETY: each suspension consumes its current-resume token. The second
    // thread can only access a fresh slot created by its own poll_call.
    let mut operation = Box::pin(unsafe { build::<CounterFamily, _, ()>(state) });
    {
        let observed = Rc::new(RefCell::new(Observed::default()));
        let mut context = pin!(CounterContext::new(observed.clone(), false));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            operation
                .as_mut()
                .poll_call(context.as_mut(), &mut cx)
                .is_pending()
        );
        assert_eq!(observed.borrow().requests, 0);
    }
    std::thread::spawn(move || {
        let observed = Rc::new(RefCell::new(Observed::default()));
        let mut context = pin!(CounterContext::new(observed.clone(), false));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            operation
                .as_mut()
                .poll_call(context.as_mut(), &mut cx)
                .is_pending()
        );
        assert_eq!(observed.borrow().requests, 0);
        assert_eq!(
            operation.as_mut().poll_call(context.as_mut(), &mut cx),
            Poll::Ready(CoroutineState::Complete(7))
        );
        assert_eq!(observed.borrow().writes, [7]);
    })
    .join()
    .unwrap();
}

struct LockedCounter;

// Ownership of the access handle lives in the view. There is deliberately no
// Host/view reconstruction capability on this type, only its family identity.
struct LockedView<'view> {
    guard: tokio::sync::MutexGuard<'view, u32>,
    drops: &'view Cell<usize>,
    visits: usize,
}

impl Drop for LockedView<'_> {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

impl HostFamily for LockedCounter {
    type HostView<'view> = LockedView<'view>;
}

impl HasFamily for LockedView<'_> {
    type Family = LockedCounter;
}

struct AddLocked(u32);

impl CallOn<LockedCounter> for AddLocked {
    type Yield = std::convert::Infallible;
    type Return = usize;

    fn poll_call<'view>(
        self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'view, Family = LockedCounter>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        LockedCounter: 'view,
    {
        let mut view = ready!(context.as_mut().poll_view(cx));
        *view.guard += self.0;
        view.visits += 1;
        Poll::Ready(CoroutineState::Complete(view.visits))
    }
}

#[test]
fn bound_coroutine_waits_for_data_then_children_share_one_async_guard_view() {
    let lock = tokio::sync::Mutex::new(0);
    let starts = Cell::new(0);
    let builds = Cell::new(0);
    let drops = Cell::new(0);
    let source = Source::<LockedCounter, _, _>::new(|| {
        starts.set(starts.get() + 1);
        async {
            let guard = lock.lock().await;
            // Construction is an explicit step after acquisition.
            builds.set(builds.get() + 1);
            LockedView {
                guard,
                drops: &drops,
                visits: 0,
            }
        }
    });
    let state = #[coroutine]
    static move |mut env: ResumeEnv<LockedCounter>| {
        let input = await_on!(env, PendingTwice(0)) as u32;
        let first = await_on!(env, Child::<LockedCounter, _>::new(AddLocked(input)));
        let second = await_on!(env, Child::<LockedCounter, _>::new(AddLocked(input + 1)));
        assert_eq!((first, second), (1, 2));
        env.end();
        input * 2 + 1
    };
    // SAFETY: every await consumes the token before yielding and completion
    // consumes it explicitly; each resume uses only its supplied stack slot.
    let operation = unsafe { build::<LockedCounter, _, ()>(state) };
    let mut bound = pin!(Bound::new(source, operation));
    let mut cx = Context::from_waker(Waker::noop());
    let held = lock.try_lock().unwrap();

    for _ in 0..2 {
        assert!(bound.as_mut().poll(&mut cx).is_pending());
        assert_eq!((starts.get(), builds.get(), drops.get()), (0, 0, 0));
    }
    // Input is now ready, and the first child starts an actual async lock wait.
    assert!(bound.as_mut().poll(&mut cx).is_pending());
    assert_eq!((starts.get(), builds.get(), drops.get()), (1, 0, 0));
    drop(held);

    assert_eq!(bound.as_mut().poll(&mut cx), Poll::Ready(15));
    assert_eq!((starts.get(), builds.get(), drops.get()), (1, 1, 1));
    assert_eq!(*lock.try_lock().unwrap(), 15);
}
