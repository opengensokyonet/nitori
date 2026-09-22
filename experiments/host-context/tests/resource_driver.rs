#![feature(
    coroutine_trait,
    coroutines,
    stmt_expr_attributes,
    impl_trait_in_assoc_type
)]
#![allow(clippy::multiple_bound_locations)]
//! A captured driver, including an owned lock, lives in an ordinary outer call.
use futures_core::Stream;
use host_context::{
    CallOn, Child, Execution, HasFamily, HostContext, HostFamily, NoReceiver, ResourceDriver,
    Round, Source,
    coroutine::{DriveMode, ResumeEnv, Suspend, build},
};
use std::{
    borrow::Borrow,
    cell::Cell,
    future::{Future, poll_fn},
    marker::{PhantomData, PhantomPinned},
    ops::CoroutineState,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker, ready},
};
use tokio::sync::{Mutex, MutexGuard};

#[derive(Default)]
struct Stats {
    starts: Cell<usize>,
    attempt_drops: Cell<usize>,
    builds: Cell<usize>,
    view_drops: Cell<usize>,
    driver_drops: Cell<usize>,
    ordinary_polls: Cell<usize>,
}
fn inc(n: &Cell<usize>) {
    n.set(n.get() + 1);
}
struct Data<'data> {
    text: &'data str,
    count: usize,
}
struct Locked<'data>(PhantomData<&'data str>);
pin_project_lite::pin_project! {
    struct View<'view, 'data> {
        guard: MutexGuard<'view, Data<'data>>,
        stats: &'view Stats,
        visits: usize,
        invariant: PhantomData<fn(&'view ()) -> &'view ()>,
        #[pin]
        pinned: PhantomPinned,
    }
    impl PinnedDrop for View<'_, '_> {
        fn drop(this: Pin<&mut Self>) { inc(&this.stats.view_drops); }
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

pin_project_lite::pin_project! {
    struct Attempt<A> {
        #[pin]
        future: A,
        stats: Rc<Stats>,
    }
    impl<A> PinnedDrop for Attempt<A> {
        fn drop(this: Pin<&mut Self>) { inc(&this.stats.attempt_drops); }
    }
}
impl<A: Future> Future for Attempt<A> {
    type Output = A::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}
struct Driver<'data> {
    lock: Mutex<Data<'data>>,
    stats: Rc<Stats>,
    delay: bool,
    _pinned: PhantomPinned,
}
impl Drop for Driver<'_> {
    fn drop(&mut self) {
        assert_eq!(self.stats.starts.get(), self.stats.attempt_drops.get());
        assert_eq!(self.stats.builds.get(), self.stats.view_drops.get());
        inc(&self.stats.driver_drops);
    }
}
impl<'data> Driver<'data> {
    fn new(text: &'data str, stats: Rc<Stats>, delay: bool) -> Self {
        Self {
            lock: Mutex::new(Data { text, count: 0 }),
            stats,
            delay,
            _pinned: PhantomPinned,
        }
    }
    fn into_execution<O: CallOn<Locked<'data>>>(
        self,
        operation: O,
    ) -> impl Future<Output = O::Return> + Stream<Item = CoroutineState<O::Yield, O::Return>> {
        Execution::new(driver_call(self, operation))
    }
}
impl<'data> ResourceDriver for Driver<'data> {
    type Family = Locked<'data>;
    type Execution<'driver, O>
        = impl Future<Output = O::Return> + Stream<Item = CoroutineState<O::Yield, O::Return>>
    where
        Self: 'driver,
        O: CallOn<Self::Family> + 'driver;

    fn execute<'driver, O>(
        self: Pin<&'driver mut Self>,
        operation: O,
    ) -> Self::Execution<'driver, O>
    where
        O: CallOn<Self::Family> + 'driver,
    {
        Execution::new(driver_call(self.into_ref().get_ref(), operation))
    }
}

fn driver_call<'data, D: Borrow<Driver<'data>>, O: CallOn<Locked<'data>>>(
    driver: D,
    operation: O,
) -> impl CallOn<NoReceiver, Yield = O::Yield, Return = O::Return> {
    // SAFETY: each token is used only in its supplying resume, exclusively by
    // poll_await, and consumed before every yield or completion. Captured driver
    // loans are independent of the token. Every round drops before suspension.
    let state = #[coroutine]
    static move |mut env: ResumeEnv<NoReceiver>| {
        let driver = driver;
        let driver_ref = driver.borrow();
        let source = Source::<Locked<'data>, _, _>::new(|| {
            inc(&driver_ref.stats.starts);
            Attempt {
                stats: driver_ref.stats.clone(),
                future: async {
                    let mut first = driver_ref.delay;
                    poll_fn(|cx| {
                        if std::mem::take(&mut first) {
                            cx.waker().wake_by_ref();
                            Poll::Pending
                        } else {
                            Poll::Ready(())
                        }
                    })
                    .await;
                    let guard = driver_ref.lock.lock().await;
                    inc(&driver_ref.stats.builds);
                    View {
                        guard,
                        stats: &driver_ref.stats,
                        visits: 0,
                        invariant: PhantomData,
                        pinned: PhantomPinned,
                    }
                },
            }
        });
        let mut source = pin!(source);
        let mut operation = pin!(operation);
        loop {
            let mode = env.mode();
            let result = {
                // Return Pending *as data*, after dropping the whole round.
                // Only the source's unfinished attempt lives across outer yield.
                let mut drive = pin!(poll_fn(|cx| {
                    let result = {
                        let mut scope = pin!(Round::new(source.as_mut()));
                        match mode {
                            DriveMode::Event => operation.as_mut().poll_call(scope.as_mut(), cx),
                            DriveMode::Return => operation
                                .as_mut()
                                .poll_return(scope.as_mut(), cx)
                                .map(CoroutineState::Complete),
                        }
                    };
                    Poll::Ready(result)
                }));
                match unsafe { env.poll_await(drive.as_mut()) } {
                    Poll::Ready(result) => result,
                    Poll::Pending => unreachable!("one-round wrapper returns immediately"),
                }
            };
            env.end();
            match result {
                Poll::Pending => env = yield Suspend::Pending,
                Poll::Ready(CoroutineState::Yielded(value)) => env = yield Suspend::Emit(value),
                Poll::Ready(CoroutineState::Complete(value)) => return value,
            }
        }
    };
    // SAFETY: the state follows the current-resume contract documented above.
    unsafe { build(state) }
}

#[derive(Clone, Copy)]
enum Step {
    Wait,
    TryReceiver,
    Emit,
    Read,
    Finish,
    Panic,
}
struct Script {
    steps: Vec<Step>,
    next: usize,
    stats: Rc<Stats>,
}
impl Script {
    fn new(steps: &[Step], stats: &Rc<Stats>) -> Self {
        Self {
            steps: steps.to_vec(),
            next: 0,
            stats: stats.clone(),
        }
    }
}
impl<'data> CallOn<Locked<'data>> for Script {
    type Yield = (usize, usize);
    type Return = usize;
    fn poll_call<'view>(
        self: Pin<&mut Self>,
        mut scope: Pin<&mut dyn HostContext<'view, Family = Locked<'data>>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, usize>>
    where
        Locked<'data>: 'view,
    {
        let this = self.get_mut();
        match this.steps[this.next] {
            Step::Wait => {
                inc(&this.stats.ordinary_polls);
                this.next += 1;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Step::TryReceiver => {
                assert!(scope.as_mut().poll_view(cx).is_pending());
                this.next += 1;
                Poll::Pending
            }
            Step::Read => {
                let view = ready!(scope.poll_view(cx));
                let view = view.project();
                assert_eq!(view.guard.text, "borrowed");
                view.guard.count += 1;
                *view.visits += 1;
                this.next += 1;
                Poll::Ready(CoroutineState::Yielded((view.guard.count, *view.visits)))
            }
            Step::Emit => {
                this.next += 1;
                Poll::Ready(CoroutineState::Yielded((0, 0)))
            }
            Step::Finish => {
                this.next += 1;
                Poll::Ready(CoroutineState::Complete(this.next))
            }
            Step::Panic => panic!("inner operation panic"),
        }
    }
}
fn scripted_call<'data>(
    script: Script,
) -> impl CallOn<Locked<'data>, Yield = (usize, usize), Return = usize> {
    let state = #[coroutine]
    static move |mut env: ResumeEnv<Locked<'data>>| {
        let mut child = pin!(Child::<Locked<'data>, _>::new(script));
        loop {
            let event = {
                let mut next = pin!(child.as_mut().next());
                loop {
                    // SAFETY: this resume owns the only live token access.
                    match unsafe { env.poll_await(next.as_mut()) } {
                        Poll::Ready(event) => break event.unwrap(),
                        Poll::Pending => {
                            env.end();
                            env = yield Suspend::Pending;
                        }
                    }
                }
            };
            env.end();
            match event {
                CoroutineState::Yielded(value) => env = yield Suspend::Emit(value),
                CoroutineState::Complete(value) => return value,
            }
        }
    };
    // SAFETY: every token is consumed before suspension or return and only
    // dispatched exclusively within its supplying resume.
    unsafe { build(state) }
}

fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

#[test]
fn owned_driver_keeps_borrowing_attempts_and_rebuilds_after_success() {
    let label = String::from("borrowed");
    let stats = Rc::new(Stats::default());
    let driver = Driver::new(&label, stats.clone(), true);
    let script = Script::new(
        &[Step::Wait, Step::Read, Step::Wait, Step::Read, Step::Finish],
        &stats,
    );
    let mut execution = pin!(driver.into_execution(scripted_call(script)));
    assert!(execution.as_mut().poll_next(&mut cx()).is_pending());
    assert_eq!(stats.starts.get(), 0);
    assert!(execution.as_mut().poll_next(&mut cx()).is_pending());
    assert_eq!((stats.starts.get(), stats.attempt_drops.get()), (1, 0));
    assert_eq!(
        execution.as_mut().poll_next(&mut cx()),
        Poll::Ready(Some(CoroutineState::Yielded((1, 1))))
    );
    assert_eq!((stats.builds.get(), stats.view_drops.get()), (1, 1));
    assert!(execution.as_mut().poll_next(&mut cx()).is_pending());
    assert_eq!(stats.starts.get(), 1);
    assert!(execution.as_mut().poll_next(&mut cx()).is_pending());
    assert_eq!(stats.starts.get(), 2);
    assert_eq!(
        execution.as_mut().poll_next(&mut cx()),
        Poll::Ready(Some(CoroutineState::Yielded((2, 1))))
    );
    assert_eq!(
        execution.as_mut().poll_next(&mut cx()),
        Poll::Ready(Some(CoroutineState::Complete(5)))
    );
    assert_eq!(
        (
            stats.attempt_drops.get(),
            stats.view_drops.get(),
            stats.driver_drops.get()
        ),
        (2, 2, 1)
    );
    assert_eq!(execution.as_mut().poll_next(&mut cx()), Poll::Ready(None));
}

#[test]
fn borrowed_driver_future_discards_yields_with_one_view_per_poll() {
    let label = String::from("borrowed");
    let stats = Rc::new(Stats::default());
    let mut driver = pin!(Driver::new(&label, stats.clone(), false));
    let script = Script::new(&[Step::Read, Step::Read, Step::Finish], &stats);
    {
        let mut execution = pin!(driver.as_mut().execute(script));
        assert_eq!(execution.as_mut().poll(&mut cx()), Poll::Ready(3));
        assert_eq!(
            (
                stats.starts.get(),
                stats.builds.get(),
                stats.view_drops.get()
            ),
            (1, 1, 1)
        );
        assert_eq!(execution.as_mut().poll_next(&mut cx()), Poll::Ready(None));
    }
    assert_eq!(driver.lock.try_lock().unwrap().count, 2);
}

#[test]
fn stream_then_future_switches_mode_without_restarting_execution() {
    let stats = Rc::new(Stats::default());
    let driver = Driver::new("borrowed", stats.clone(), false);
    let script = Script::new(&[Step::Read, Step::Read, Step::Read, Step::Finish], &stats);
    let mut execution = pin!(driver.into_execution(script));
    assert_eq!(
        execution.as_mut().poll_next(&mut cx()),
        Poll::Ready(Some(CoroutineState::Yielded((1, 1))))
    );
    assert_eq!(execution.as_mut().poll(&mut cx()), Poll::Ready(4));
    assert_eq!((stats.starts.get(), stats.view_drops.get()), (2, 2));
}

#[test]
fn pending_lock_survives_rounds_without_receiver_access_and_keeps_queue_position() {
    let stats = Rc::new(Stats::default());
    let driver = Driver::new("borrowed", stats.clone(), false);
    let held = driver.lock.try_lock().unwrap();
    let script = Script::new(
        &[Step::TryReceiver, Step::Wait, Step::Read, Step::Finish],
        &stats,
    );
    let mut execution = pin!(Execution::new(driver_call(&driver, script)));
    assert!(execution.as_mut().poll_next(&mut cx()).is_pending());
    let mut competitor = pin!(driver.lock.lock());
    assert!(competitor.as_mut().poll(&mut cx()).is_pending());
    drop(held);
    assert!(execution.as_mut().poll_next(&mut cx()).is_pending());
    assert_eq!(
        (
            stats.starts.get(),
            stats.attempt_drops.get(),
            stats.builds.get()
        ),
        (1, 0, 0)
    );
    assert!(competitor.as_mut().poll(&mut cx()).is_pending());
    assert_eq!(
        execution.as_mut().poll_next(&mut cx()),
        Poll::Ready(Some(CoroutineState::Yielded((1, 1))))
    );
    let Poll::Ready(guard) = competitor.as_mut().poll(&mut cx()) else {
        panic!("resource was not released")
    };
    assert_eq!(guard.count, 1);
    assert_eq!(execution.as_mut().poll(&mut cx()), Poll::Ready(4));
}

#[test]
fn completion_cancels_unfinished_acquisition_before_execution_is_dropped() {
    let stats = Rc::new(Stats::default());
    let driver = Driver::new("borrowed", stats.clone(), false);
    let held = driver.lock.try_lock().unwrap();
    let script = Script::new(&[Step::TryReceiver, Step::Wait, Step::Finish], &stats);
    let mut execution = pin!(Execution::new(driver_call(&driver, script)));
    assert!(execution.as_mut().poll(&mut cx()).is_pending());
    drop(held);
    assert!(execution.as_mut().poll(&mut cx()).is_pending());
    assert!(driver.lock.try_lock().is_err());
    assert_eq!(execution.as_mut().poll(&mut cx()), Poll::Ready(3));
    assert_eq!(stats.attempt_drops.get(), 1);
    assert!(driver.lock.try_lock().is_ok());
}

#[test]
fn pending_acquisition_survives_an_outward_event_and_poll_mode_change() {
    let stats = Rc::new(Stats::default());
    let driver = Driver::new("borrowed", stats.clone(), false);
    let held = driver.lock.try_lock().unwrap();
    let script = Script::new(
        &[Step::TryReceiver, Step::Emit, Step::Read, Step::Finish],
        &stats,
    );
    let mut execution = pin!(Execution::new(driver_call(&driver, script)));
    assert!(execution.as_mut().poll(&mut cx()).is_pending());
    assert_eq!(
        execution.as_mut().poll_next(&mut cx()),
        Poll::Ready(Some(CoroutineState::Yielded((0, 0))))
    );
    assert_eq!((stats.starts.get(), stats.attempt_drops.get()), (1, 0));
    drop(held);
    assert_eq!(execution.as_mut().poll(&mut cx()), Poll::Ready(4));
    assert_eq!(
        (
            stats.starts.get(),
            stats.attempt_drops.get(),
            stats.view_drops.get()
        ),
        (1, 1, 1)
    );
    assert!(driver.lock.try_lock().is_ok());
}

#[test]
fn cancellation_drops_pending_attempt_before_owned_driver() {
    let stats = Rc::new(Stats::default());
    let driver = Driver::new("borrowed", stats.clone(), true);
    let script = Script::new(&[Step::Read], &stats);
    {
        let mut execution = pin!(driver.into_execution(script));
        assert!(execution.as_mut().poll(&mut cx()).is_pending());
        assert_eq!(stats.attempt_drops.get(), 0);
    }
    assert_eq!(
        (stats.attempt_drops.get(), stats.driver_drops.get()),
        (1, 1)
    );
}

#[test]
fn panic_cancels_pending_attempt_and_poisoned_execution_cannot_resume() {
    let stats = Rc::new(Stats::default());
    let driver = Driver::new("borrowed", stats.clone(), false);
    let held = driver.lock.try_lock().unwrap();
    let script = Script::new(&[Step::TryReceiver, Step::Panic], &stats);
    let mut execution = pin!(Execution::new(driver_call(&driver, script)));
    assert!(execution.as_mut().poll(&mut cx()).is_pending());
    drop(held);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execution.as_mut().poll(&mut cx())
    }));
    assert!(result.is_err());
    assert_eq!(stats.attempt_drops.get(), 1);
    assert!(driver.lock.try_lock().is_ok());
    assert_eq!(execution.as_mut().poll_next(&mut cx()), Poll::Ready(None));
}
