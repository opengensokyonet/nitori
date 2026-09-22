#![feature(coroutine_trait, coroutines, stmt_expr_attributes)]
#![allow(clippy::multiple_bound_locations)] // pin-project-lite generated projections

use host_context::{
    Bound, CallOn, Child, HasFamily, HostContext, HostFamily, Projection, ReceivedCall, Round,
    Source, ViewSource,
    coroutine::{ResumeEnv, Suspend, build},
};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    marker::{PhantomData, PhantomPinned},
    ops::CoroutineState,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::{Pin, pin},
    task::{Context, Poll, Waker, ready},
};
use tokio::sync::{Mutex, MutexGuard};

#[derive(Default)]
struct Observed {
    acquisitions: Cell<usize>,
    root_builds: Cell<usize>,
    lane_builds: Cell<usize>,
    leaf_builds: Cell<usize>,
    panic_on_drop: Cell<Option<&'static str>>,
    drops: RefCell<Vec<&'static str>>,
    leaf_addresses: RefCell<Vec<usize>>,
}

struct DropMark<'a>(&'a Observed, &'static str);
impl Drop for DropMark<'_> {
    fn drop(&mut self) {
        self.0.drops.borrow_mut().push(self.1);
        if self.0.panic_on_drop.get() == Some(self.1) {
            self.0.panic_on_drop.set(None);
            panic!("projected view destructor panic");
        }
    }
}

struct Data<'data> {
    label: &'data str,
    bytes: Vec<u8>,
}

struct Root<'data>(PhantomData<&'data str>);
struct Lane<'data>(PhantomData<&'data str>);
struct Leaf<'data>(PhantomData<&'data str>);

pin_project_lite::pin_project! {
    struct RootView<'view, 'data> {
        guard: MutexGuard<'view, Data<'data>>,
        drop_mark: DropMark<'view>,
        invariant: PhantomData<fn(&'view ()) -> &'view ()>,
        #[pin]
        pinned: PhantomPinned,
    }
}
pin_project_lite::pin_project! {
    struct LaneView<'view, 'data> {
        data: &'view mut Data<'data>,
        state: &'view mut usize,
        observed: &'data Observed,
        drop_mark: DropMark<'view>,
        invariant: PhantomData<fn(&'view ()) -> &'view ()>,
        #[pin]
        pinned: PhantomPinned,
    }
}
pin_project_lite::pin_project! {
    struct LeafView<'view, 'data> {
        data: &'view mut Data<'data>,
        first_state: &'view mut usize,
        second_state: &'view mut usize,
        observed: &'data Observed,
        drop_mark: DropMark<'view>,
        invariant: PhantomData<fn(&'view ()) -> &'view ()>,
        #[pin]
        pinned: PhantomPinned,
    }
}
impl<'data> HostFamily for Root<'data> {
    type HostView<'view>
        = RootView<'view, 'data>
    where
        Self: 'view;
}
impl<'data> HostFamily for Lane<'data> {
    type HostView<'view>
        = LaneView<'view, 'data>
    where
        Self: 'view;
}
impl<'data> HostFamily for Leaf<'data> {
    type HostView<'view>
        = LeafView<'view, 'data>
    where
        Self: 'view;
}
impl<'data> HasFamily for RootView<'_, 'data> {
    type Family = Root<'data>;
}
impl<'data> HasFamily for LaneView<'_, 'data> {
    type Family = Lane<'data>;
}
impl<'data> HasFamily for LeafView<'_, 'data> {
    type Family = Leaf<'data>;
}

struct ToLane<'state, 'data> {
    state: &'state mut usize,
    observed: &'data Observed,
}
impl<'data> Projection for ToLane<'_, 'data> {
    type Root = Root<'data>;
    type Target = Lane<'data>;

    fn project<'scope, 'view>(
        self: Pin<&'scope mut Self>,
        root: Pin<&'scope mut RootView<'view, 'data>>,
    ) -> LaneView<'scope, 'data>
    where
        Self::Root: 'view,
        Self::Target: 'scope,
        'view: 'scope,
    {
        let this = self.get_mut();
        this.observed
            .lane_builds
            .set(this.observed.lane_builds.get() + 1);
        LaneView {
            data: &mut *root.project().guard,
            state: this.state,
            observed: this.observed,
            drop_mark: DropMark(this.observed, "lane"),
            invariant: PhantomData,
            pinned: PhantomPinned,
        }
    }
}

struct ToLeaf<'state, 'data> {
    state: &'state mut usize,
    observed: &'data Observed,
}
impl<'data> Projection for ToLeaf<'_, 'data> {
    type Root = Lane<'data>;
    type Target = Leaf<'data>;

    fn project<'scope, 'view>(
        self: Pin<&'scope mut Self>,
        root: Pin<&'scope mut LaneView<'view, 'data>>,
    ) -> LeafView<'scope, 'data>
    where
        Self::Root: 'view,
        Self::Target: 'scope,
        'view: 'scope,
    {
        let this = self.get_mut();
        this.observed
            .leaf_builds
            .set(this.observed.leaf_builds.get() + 1);
        let root = root.project();
        LeafView {
            data: root.data,
            first_state: root.state,
            second_state: this.state,
            observed: this.observed,
            drop_mark: DropMark(this.observed, "leaf"),
            invariant: PhantomData,
            pinned: PhantomPinned,
        }
    }
}

fn source<'view, 'data: 'view>(
    lock: &'view Mutex<Data<'data>>,
    observed: &'view Observed,
) -> impl ViewSource<'view, Family = Root<'data>> {
    Source::<Root<'data>, _, _>::new(move || {
        observed.acquisitions.set(observed.acquisitions.get() + 1);
        async move {
            let guard = lock.lock().await;
            observed.root_builds.set(observed.root_builds.get() + 1);
            RootView {
                guard,
                drop_mark: DropMark(observed, "root"),
                invariant: PhantomData,
                pinned: PhantomPinned,
            }
        }
    })
}

// The local receiver state is constructed only after this asynchronous input.
struct Input(bool);
impl Future for Input {
    type Output = u8;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u8> {
        if std::mem::replace(&mut self.0, true) {
            Poll::Ready(b'x')
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

macro_rules! await_on {
    ($env:ident, $value:expr) => {{
        let mut awaited = pin!($value);
        loop {
            // SAFETY: only the current resume's token is used. Every Pending
            // consumes it before yielding and receives a new token on resume.
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

struct WriteEvents {
    byte: u8,
    count: usize,
    pause_after_first: bool,
    panic_after_first: bool,
}
impl<'data> CallOn<Leaf<'data>> for WriteEvents {
    type Yield = usize;
    type Return = usize;
    fn poll_call<'view>(
        mut self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'view, Family = Leaf<'data>>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<usize, usize>>
    where
        Leaf<'data>: 'view,
    {
        if self.count == 1 && std::mem::take(&mut self.pause_after_first) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if self.count == 1 && self.panic_after_first {
            panic!("projected operation panic");
        }
        let view = ready!(context.as_mut().poll_view(cx));
        let address = &*view as *const LeafView<'_, '_> as usize;
        let view = view.project();
        assert_eq!(view.data.label, "borrowed writer");
        view.observed.leaf_addresses.borrow_mut().push(address);
        view.data.bytes.push(self.byte);
        **view.first_state += 1;
        **view.second_state += 10;
        self.count += 1;
        if self.count == 3 {
            Poll::Ready(CoroutineState::Complete(self.count))
        } else {
            Poll::Ready(CoroutineState::Yielded(self.count))
        }
    }
}

#[test]
fn coroutine_local_nested_receivers_cache_views_and_siblings_share_root() {
    let label = String::from("borrowed writer");
    let observed = Observed::default();
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let seen = &observed;
    let state = #[coroutine]
    static move |mut env: ResumeEnv<Root<'_>>| {
        let byte = await_on!(env, Input(false));
        for _ in 0..2 {
            let mut first = 0;
            let mut second = 0;
            let operation = ReceivedCall::new(
                ToLane {
                    state: &mut first,
                    observed: seen,
                },
                ReceivedCall::new(
                    ToLeaf {
                        state: &mut second,
                        observed: seen,
                    },
                    WriteEvents {
                        byte,
                        count: 0,
                        pause_after_first: false,
                        panic_after_first: false,
                    },
                ),
            );
            assert_eq!(await_on!(env, Child::<Root<'_>, _>::new(operation)), 3);
            // Child completion has destroyed both cached views before local
            // state is accessed or another sibling receiver borrows the root.
            assert_eq!((first, second), (3, 30));
            assert_eq!(seen.drops.borrow().last(), Some(&"lane"));
        }
        env.end();
        6
    };
    // SAFETY: the coroutine confines each environment to its own resume and
    // consumes it on every suspension and completion path.
    let operation = unsafe { build::<Root<'_>, _, ()>(state) };
    let mut bound = pin!(Bound::new(source(&lock, &observed), operation));
    let mut cx = Context::from_waker(Waker::noop());
    let held = lock.try_lock().unwrap();

    assert!(bound.as_mut().poll(&mut cx).is_pending());
    assert_eq!(observed.acquisitions.get(), 0);
    assert_eq!(
        (observed.lane_builds.get(), observed.leaf_builds.get()),
        (0, 0)
    );
    assert!(bound.as_mut().poll(&mut cx).is_pending());
    assert_eq!(observed.acquisitions.get(), 1);
    assert_eq!(observed.root_builds.get(), 0);
    assert_eq!(
        (observed.lane_builds.get(), observed.leaf_builds.get()),
        (0, 0)
    );
    assert!(bound.as_mut().poll(&mut cx).is_pending());
    assert_eq!(observed.acquisitions.get(), 1);
    drop(held);

    assert_eq!(bound.as_mut().poll(&mut cx), Poll::Ready(6));
    assert_eq!(observed.root_builds.get(), 1);
    assert_eq!(
        (observed.lane_builds.get(), observed.leaf_builds.get()),
        (2, 2)
    );
    assert_eq!(
        &*observed.drops.borrow(),
        &["leaf", "lane", "leaf", "lane", "root"]
    );
    let addresses = observed.leaf_addresses.borrow();
    assert_eq!(addresses.len(), 6);
    for sibling in addresses.as_chunks::<3>().0 {
        assert!(sibling.iter().all(|address| *address == sibling[0]));
    }
    assert_eq!(lock.try_lock().unwrap().bytes, b"xxxxxx");
}

#[test]
fn pending_drops_projected_views_and_next_poll_rebuilds_from_persistent_local_state() {
    let label = String::from("borrowed writer");
    let observed = Observed::default();
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let mut first = 0;
    let mut second = 0;
    let operation = ReceivedCall::new(
        ToLane {
            state: &mut first,
            observed: &observed,
        },
        ReceivedCall::new(
            ToLeaf {
                state: &mut second,
                observed: &observed,
            },
            WriteEvents {
                byte: b'p',
                count: 0,
                pause_after_first: true,
                panic_after_first: false,
            },
        ),
    );
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut bound = pin!(Bound::new(source(&lock, &observed), operation));
        assert!(bound.as_mut().poll(&mut cx).is_pending());
        assert_eq!(&*observed.drops.borrow(), &["leaf", "lane", "root"]);
        assert_eq!(lock.try_lock().unwrap().bytes, b"p");
        assert_eq!(bound.as_mut().poll(&mut cx), Poll::Ready(3));
    }
    assert_eq!((first, second), (3, 30));
    assert_eq!(
        (
            observed.root_builds.get(),
            observed.lane_builds.get(),
            observed.leaf_builds.get()
        ),
        (2, 2, 2)
    );
    assert_eq!(
        &*observed.drops.borrow(),
        &["leaf", "lane", "root", "leaf", "lane", "root"]
    );
    assert_eq!(lock.try_lock().unwrap().bytes, b"ppp");
}

#[test]
fn panic_releases_nested_views_and_keeps_binding_terminal() {
    let label = String::from("borrowed writer");
    let observed = Observed::default();
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let mut first = 0;
    let mut second = 0;
    let operation = ReceivedCall::new(
        ToLane {
            state: &mut first,
            observed: &observed,
        },
        ReceivedCall::new(
            ToLeaf {
                state: &mut second,
                observed: &observed,
            },
            WriteEvents {
                byte: b'!',
                count: 0,
                pause_after_first: false,
                panic_after_first: true,
            },
        ),
    );
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut bound = pin!(Bound::new(source(&lock, &observed), operation));
        for _ in 0..2 {
            assert!(catch_unwind(AssertUnwindSafe(|| bound.as_mut().poll(&mut cx))).is_err());
        }
        assert_eq!(&*observed.drops.borrow(), &["leaf", "lane", "root"]);
        assert_eq!(lock.try_lock().unwrap().bytes, b"!");
    }
    assert_eq!((first, second), (1, 10));
    assert_eq!(
        (
            observed.root_builds.get(),
            observed.lane_builds.get(),
            observed.leaf_builds.get()
        ),
        (1, 1, 1)
    );
}

#[test]
fn visible_child_events_rebuild_projected_views_while_root_stays_in_one_round() {
    let label = String::from("borrowed writer");
    let observed = Observed::default();
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let seen = &observed;
    let state = #[coroutine]
    static move |mut env: ResumeEnv<Root<'_>>| {
        let mut first = 0;
        let mut second = 0;
        {
            let operation = ReceivedCall::new(
                ToLane {
                    state: &mut first,
                    observed: seen,
                },
                ReceivedCall::new(
                    ToLeaf {
                        state: &mut second,
                        observed: seen,
                    },
                    WriteEvents {
                        byte: b'e',
                        count: 0,
                        pause_after_first: false,
                        panic_after_first: false,
                    },
                ),
            );
            let mut child = pin!(Child::<Root<'_>, _>::new(operation));
            for count in 1..=3 {
                let event = await_on!(env, child.as_mut().next());
                let expected = if count < 3 {
                    CoroutineState::Yielded(count)
                } else {
                    CoroutineState::Complete(count)
                };
                assert_eq!(event, Some(expected));
                // An event handed to the parent ends this receiver drive scope
                // even though the enclosing Bound poll has not returned.
                assert_eq!(seen.drops.borrow().len(), count * 2);
                assert_eq!(seen.drops.borrow().last(), Some(&"lane"));
                assert_eq!(seen.root_builds.get(), 1);
                assert_eq!(
                    (seen.lane_builds.get(), seen.leaf_builds.get()),
                    (count, count)
                );
            }
            assert_eq!(await_on!(env, child.as_mut().next()), None);
        }
        assert_eq!((first, second), (3, 30));
        env.end();
    };
    // SAFETY: all current-resume tokens are consumed before suspension and
    // completion; next() never retains the environment or view borrow.
    let operation = unsafe { build::<Root<'_>, _, ()>(state) };
    let mut bound = pin!(Bound::new(source(&lock, &observed), operation));
    let mut cx = Context::from_waker(Waker::noop());

    assert_eq!(bound.as_mut().poll(&mut cx), Poll::Ready(()));
    assert_eq!(observed.acquisitions.get(), 1);
    assert_eq!(
        &*observed.drops.borrow(),
        &["leaf", "lane", "leaf", "lane", "leaf", "lane", "root"]
    );
    assert_eq!(lock.try_lock().unwrap().bytes, b"eee");
}

#[test]
fn erased_call_dispatch_preserves_projected_cache_while_draining_yields() {
    let label = String::from("borrowed writer");
    let observed = Observed::default();
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let mut first = 0;
    let mut second = 0;
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut acquisition = pin!(source(&lock, &observed));
        let mut context = pin!(Round::new(acquisition.as_mut()));
        let mut operation = pin!(ReceivedCall::new(
            ToLane {
                state: &mut first,
                observed: &observed
            },
            ReceivedCall::new(
                ToLeaf {
                    state: &mut second,
                    observed: &observed
                },
                WriteEvents {
                    byte: b'd',
                    count: 0,
                    pause_after_first: false,
                    panic_after_first: false,
                },
            ),
        ));
        let operation: Pin<&mut dyn CallOn<Root<'_>, Yield = usize, Return = usize>> =
            operation.as_mut();
        assert_eq!(
            operation.poll_return(context.as_mut(), &mut cx),
            Poll::Ready(3)
        );
        assert_eq!(
            (
                observed.root_builds.get(),
                observed.lane_builds.get(),
                observed.leaf_builds.get()
            ),
            (1, 1, 1)
        );
        assert_eq!(&*observed.drops.borrow(), &["leaf", "lane"]);
    }
    assert_eq!((first, second), (3, 30));
    assert_eq!(&*observed.drops.borrow(), &["leaf", "lane", "root"]);
    assert_eq!(lock.try_lock().unwrap().bytes, b"ddd");
}

#[test]
fn projected_destructor_panic_keeps_received_call_terminal_after_pending() {
    let label = String::from("borrowed writer");
    let observed = Observed::default();
    observed.panic_on_drop.set(Some("leaf"));
    let lock = Mutex::new(Data {
        label: &label,
        bytes: Vec::new(),
    });
    let mut first = 0;
    let mut second = 0;
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut acquisition = pin!(source(&lock, &observed));
        let mut context = pin!(Round::new(acquisition.as_mut()));
        let mut operation = pin!(ReceivedCall::new(
            ToLane {
                state: &mut first,
                observed: &observed
            },
            ReceivedCall::new(
                ToLeaf {
                    state: &mut second,
                    observed: &observed
                },
                WriteEvents {
                    byte: b'!',
                    count: 0,
                    pause_after_first: true,
                    panic_after_first: false,
                },
            ),
        ));
        // poll_return discards the first event, then the leaf operation returns
        // Pending. Its view's destructor unwinds before either adapter can
        // restore resumability. No Bound or Child wraps this ReceivedCall.
        let failure = catch_unwind(AssertUnwindSafe(|| {
            operation.as_mut().poll_return(context.as_mut(), &mut cx)
        }))
        .unwrap_err();
        assert_eq!(
            failure.downcast_ref::<&str>(),
            Some(&"projected view destructor panic")
        );
        assert_eq!(&*observed.drops.borrow(), &["leaf", "lane"]);
        let retry = catch_unwind(AssertUnwindSafe(|| {
            operation.as_mut().poll_return(context.as_mut(), &mut cx)
        }))
        .unwrap_err();
        assert_eq!(
            retry.downcast_ref::<&str>(),
            Some(&"received call polled after completion or panic")
        );
        assert_eq!(
            (
                observed.root_builds.get(),
                observed.lane_builds.get(),
                observed.leaf_builds.get()
            ),
            (1, 1, 1)
        );
        assert_eq!(&*observed.drops.borrow(), &["leaf", "lane"]);
    }
    assert_eq!((first, second), (1, 10));
    assert_eq!(&*observed.drops.borrow(), &["leaf", "lane", "root"]);
    assert_eq!(lock.try_lock().unwrap().bytes, b"!");
}
