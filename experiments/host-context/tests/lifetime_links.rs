#![feature(coroutines, stmt_expr_attributes)]
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::multiple_bound_locations)] // pin-project-lite projections
//! Each composition hides one parent-view lifetime while retaining its Family.

use host_context::{
    AwaitOn, Bound, CallOn, Child, HasFamily, HostContext, HostFamily, Projection, ReceivedCall,
    Source,
    coroutine::{ResumeEnv, Suspend, build},
};
use std::{
    cell::Cell,
    future::Future,
    marker::{PhantomData, PhantomPinned},
    pin::{Pin, pin},
    sync::{Mutex, MutexGuard},
    task::{Context, Poll, Waker, ready},
};

// The callback quantifies only a lifetime. F remains an exact, fixed type.
trait Visitor<F: HostFamily> {
    fn visit<'parent>(&mut self, view: Pin<&mut F::HostView<'parent>>)
    where
        F: 'parent;
}

// An operation's output is independent of the hidden parent/view-access loans.
trait ViewOperation<F: HostFamily> {
    type Output;
    fn run<'parent>(self, view: Pin<&mut F::HostView<'parent>>) -> Self::Output
    where
        F: 'parent;
}
// The nominal argument carries F: 'parent without requiring F: 'static.
struct Access<'access, 'parent, F: HostFamily + 'parent> {
    view: Pin<&'access mut F::HostView<'parent>>,
}
struct ClosureOperation<B>(B);
impl<F: HostFamily, B, R> ViewOperation<F> for ClosureOperation<B>
where
    B: for<'access, 'parent> FnOnce(Access<'access, 'parent, F>) -> R,
{
    type Output = R;
    fn run<'parent>(self, view: Pin<&mut F::HostView<'parent>>) -> R
    where
        F: 'parent,
    {
        (self.0)(Access { view })
    }
}
struct Request<F: HostFamily, A: ViewOperation<F>> {
    operation: Option<A>,
    output: Option<A::Output>,
    family: PhantomData<fn(F) -> F>,
}
impl<F: HostFamily, A: ViewOperation<F>> Visitor<F> for Request<F, A> {
    fn visit<'parent>(&mut self, view: Pin<&mut F::HostView<'parent>>)
    where
        F: 'parent,
    {
        self.output = Some(self.operation.take().unwrap().run(view));
    }
}

// Owning a loan, not its pointee. The raw pointer is never exposed or dereferenced
// without exclusive access to the wrapper. Its Family is deliberately invariant.
struct ViewLoan<'scope, F: HostFamily> {
    pointer: *mut (),
    dispatch: unsafe fn(*mut (), &mut dyn Visitor<F>),
    borrow: PhantomData<&'scope mut ()>,
    family: PhantomData<fn(F) -> F>,
}
struct Dispatch<'parent, F: HostFamily + 'parent>(PhantomData<&'parent F>);
impl<'parent, F: HostFamily + 'parent> Dispatch<'parent, F> {
    unsafe fn visit(pointer: *mut (), visitor: &mut dyn Visitor<F>) {
        // SAFETY: new pairs this function with the exact F::HostView<'parent>
        // pointer. ViewLoan's exclusive borrow is live for the whole visit.
        // The pointee remains pinned and its inner lifetime is not substituted.
        let view = unsafe { Pin::new_unchecked(&mut *pointer.cast::<F::HostView<'parent>>()) };
        visitor.visit(view);
    }
}
impl<'scope, F: HostFamily + 'scope> ViewLoan<'scope, F> {
    fn new<'parent: 'scope>(view: Pin<&'scope mut F::HostView<'parent>>) -> Self
    where
        F: 'parent,
    {
        // SAFETY: obtaining a raw pointer does not move the pinned pointee.
        // Only the matched dispatcher can use it and recreates a pinned borrow.
        let pointer = unsafe { view.get_unchecked_mut() as *mut F::HostView<'parent> }.cast();
        Self {
            pointer,
            dispatch: Dispatch::<'parent, F>::visit,
            borrow: PhantomData,
            family: PhantomData,
        }
    }
    fn apply<A: ViewOperation<F>>(&mut self, operation: A) -> A::Output {
        let mut request = Request::<F, A> {
            operation: Some(operation),
            output: None,
            family: PhantomData,
        };
        // SAFETY: the lifetime marker keeps the original pinned loan alive.
        // &mut self excludes every other call through this wrapper. Private
        // fields guarantee that the function and pointer remain correctly paired.
        unsafe { (self.dispatch)(self.pointer, &mut request) };
        request.output.unwrap()
    }
    fn with<B, R>(&mut self, body: B) -> R
    where
        B: for<'access, 'parent> FnOnce(Access<'access, 'parent, F>) -> R,
    {
        self.apply(ClosureOperation(body))
    }
}

#[derive(Default)]
struct Stats {
    acquisitions: Cell<usize>,
    builds: Cell<usize>,
    drops: Cell<usize>,
}
fn inc(value: &Cell<usize>) {
    value.set(value.get() + 1);
}
struct Data<'data> {
    label: &'data str,
    byte: u8,
}
struct Root<'data>(PhantomData<&'data str>);
pin_project_lite::pin_project! {
    struct RootView<'parent, 'data> {
        guard: MutexGuard<'parent, Data<'data>>,
        stats: &'parent Stats,
        invariant: PhantomData<fn(&'parent ()) -> &'parent ()>,
        #[pin]
        pinned: PhantomPinned,
    }
    impl PinnedDrop for RootView<'_, '_> {
        fn drop(this: Pin<&mut Self>) { inc(&this.stats.drops); }
    }
}
impl<'data> HostFamily for Root<'data> {
    type HostView<'parent>
        = RootView<'parent, 'data>
    where
        Self: 'parent;
}
impl<'data> HasFamily for RootView<'_, 'data> {
    type Family = Root<'data>;
}

struct Counted<'state, F>(PhantomData<&'state ()>, PhantomData<fn() -> F>);
pin_project_lite::pin_project! {
    struct CountView<'scope, 'state, F: HostFamily> where F: 'scope {
        parent: ViewLoan<'scope, F>,
        count: &'scope mut usize,
        stats: &'state Stats,
        #[pin]
        pinned: PhantomPinned,
    }
    impl<F: HostFamily> PinnedDrop for CountView<'_, '_, F> {
        fn drop(this: Pin<&mut Self>) { inc(&this.stats.drops); }
    }
}
impl<'state, F: HostFamily> HostFamily for Counted<'state, F> {
    type HostView<'scope>
        = CountView<'scope, 'state, F>
    where
        Self: 'scope;
}
impl<'state, F: HostFamily> HasFamily for CountView<'_, 'state, F> {
    type Family = Counted<'state, F>;
}
struct Counter<'state, F> {
    count: usize,
    stats: &'state Stats,
    family: PhantomData<fn() -> F>,
}
impl<'state, F> Counter<'state, F> {
    fn new(stats: &'state Stats) -> Self {
        Self {
            count: 0,
            stats,
            family: PhantomData,
        }
    }
}
impl<'state, F: HostFamily> Projection for Counter<'state, F> {
    type Root = F;
    type Target = Counted<'state, F>;
    fn project<'scope, 'parent>(
        self: Pin<&'scope mut Self>,
        root: Pin<&'scope mut F::HostView<'parent>>,
    ) -> CountView<'scope, 'state, F>
    where
        F: 'parent,
        Self::Target: 'scope,
        'parent: 'scope,
    {
        let this = self.get_mut();
        inc(&this.stats.builds);
        CountView {
            parent: ViewLoan::new(root),
            count: &mut this.count,
            stats: this.stats,
            pinned: PhantomPinned,
        }
    }
}

// Both capabilities are ordinary Family traits with their original canonical
// HostView argument. Neither trait is mentioned by ViewLoan or its dispatcher.
trait Read: HostFamily {
    fn read<'v>(view: Pin<&mut Self::HostView<'v>>) -> u8
    where
        Self: 'v;
}
trait LabelLen: HostFamily {
    fn label_len<'v>(view: Pin<&mut Self::HostView<'v>>) -> usize
    where
        Self: 'v;
}
// This capability has a type-generic method and cannot serve as a dyn interface.
// Its caller still knows the result and callback types before invoking the loan.
trait Transform: HostFamily {
    fn transform<'v, T, C: FnOnce(u8) -> T>(view: Pin<&mut Self::HostView<'v>>, callback: C) -> T
    where
        Self: 'v;
}
impl Read for Root<'_> {
    fn read<'v>(view: Pin<&mut Self::HostView<'v>>) -> u8
    where
        Self: 'v,
    {
        let this = view.project();
        let byte = this.guard.byte;
        this.guard.byte += 1;
        byte
    }
}
impl LabelLen for Root<'_> {
    fn label_len<'v>(view: Pin<&mut Self::HostView<'v>>) -> usize
    where
        Self: 'v,
    {
        view.guard.label.len()
    }
}
impl Transform for Root<'_> {
    fn transform<'v, T, C: FnOnce(u8) -> T>(view: Pin<&mut Self::HostView<'v>>, callback: C) -> T
    where
        Self: 'v,
    {
        callback(view.guard.byte)
    }
}
struct ReadParent<F>(PhantomData<fn() -> F>);
impl<F: Read> ViewOperation<F> for ReadParent<F> {
    type Output = u8;
    fn run<'v>(self, view: Pin<&mut F::HostView<'v>>) -> u8
    where
        F: 'v,
    {
        F::read(view)
    }
}
struct MeasureParent<F>(PhantomData<fn() -> F>);
impl<F: LabelLen> ViewOperation<F> for MeasureParent<F> {
    type Output = usize;
    fn run<'v>(self, view: Pin<&mut F::HostView<'v>>) -> usize
    where
        F: 'v,
    {
        F::label_len(view)
    }
}
impl<F: Read> Read for Counted<'_, F> {
    fn read<'v>(view: Pin<&mut Self::HostView<'v>>) -> u8
    where
        Self: 'v,
    {
        let this = view.project();
        **this.count += 1;
        this.parent.apply(ReadParent::<F>(PhantomData))
    }
}
impl<F: LabelLen> LabelLen for Counted<'_, F> {
    fn label_len<'v>(view: Pin<&mut Self::HostView<'v>>) -> usize
    where
        Self: 'v,
    {
        view.project().parent.apply(MeasureParent::<F>(PhantomData))
    }
}
impl<F: Transform> Transform for Counted<'_, F> {
    fn transform<'v, T, C: FnOnce(u8) -> T>(view: Pin<&mut Self::HostView<'v>>, callback: C) -> T
    where
        Self: 'v,
    {
        view.project()
            .parent
            .with(|access| F::transform(access.view, callback))
    }
}

struct Input(bool);
impl Future for Input {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if std::mem::replace(&mut self.get_mut().0, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
struct ReadOne;
impl<F: Read + LabelLen + Transform> AwaitOn<F> for ReadOne {
    type Output = u8;
    fn poll_on<'v>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'v, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>
    where
        F: 'v,
    {
        let mut view = ready!(context.poll_view(cx));
        assert_eq!(F::label_len(view.as_mut()), 8);
        let local = String::from("borrowed callback result");
        let calls = Cell::new(0);
        let borrowed_result = F::transform(view.as_mut(), |_| {
            inc(&calls);
            local.as_str()
        });
        assert_eq!(borrowed_result, "borrowed callback result");
        assert_eq!(calls.get(), 1);
        Poll::Ready(F::read(view))
    }
}
macro_rules! await_on {
    ($env:ident, $value:expr) => {{
        let mut value = pin!($value);
        loop {
            // SAFETY: the current token is consumed before every suspension;
            // awaitables receive temporary context loans and cannot retain it.
            match unsafe { $env.poll_await(value.as_mut()) } {
                Poll::Pending => {
                    $env.end();
                    $env = yield Suspend::Pending;
                }
                Poll::Ready(value) => break value,
            }
        }
    }};
}
fn inner_call<F: Read + LabelLen + Transform>(
    pause_after_read: bool,
) -> impl CallOn<F, Yield = (), Return = (u8, u8)> {
    let state = #[coroutine]
    static move |mut env: ResumeEnv<F>| {
        await_on!(env, Input(false));
        let first = await_on!(env, ReadOne);
        if pause_after_read {
            await_on!(env, Input(false));
        }
        let second = await_on!(env, ReadOne);
        env.end();
        (first, second)
    };
    // SAFETY: all resume tokens are used locally and consumed before suspension.
    unsafe { build(state) }
}

fn exercise(pause_after_read: bool) {
    let label = String::from("borrowed");
    let lock = Mutex::new(Data {
        label: &label,
        byte: 7,
    });
    let root_stats = Stats::default();
    let layers = [Stats::default(), Stats::default(), Stats::default()];
    let seen = &layers;
    let state = #[coroutine]
    static move |mut env: ResumeEnv<Root<'_>>| {
        await_on!(env, Input(false));
        let first = Counter::<Root<'_>>::new(&seen[0]);
        let second = Counter::<Counted<'_, Root<'_>>>::new(&seen[1]);
        let third = Counter::<Counted<'_, Counted<'_, Root<'_>>>>::new(&seen[2]);
        let child = ReceivedCall::new(
            first,
            ReceivedCall::new(
                second,
                ReceivedCall::new(
                    third,
                    inner_call::<Counted<'_, Counted<'_, Counted<'_, Root<'_>>>>>(pause_after_read),
                ),
            ),
        );
        let result = await_on!(env, Child::<Root<'_>, _>::new(child));
        env.end();
        result
    };
    // SAFETY: the same scoped resume protocol applies to the outer coroutine.
    let operation = unsafe { build::<Root<'_>, _, ()>(state) };
    let source = Source::<Root<'_>, _, _>::new(|| {
        inc(&root_stats.acquisitions);
        inc(&root_stats.builds);
        std::future::ready(RootView {
            guard: lock.lock().unwrap(),
            stats: &root_stats,
            invariant: PhantomData,
            pinned: PhantomPinned,
        })
    });
    let mut bound = pin!(Bound::new(source, operation));
    let mut cx = Context::from_waker(Waker::noop());
    // Outer input and then inner input both suspend without acquiring the root
    // or constructing any of the three projected lifetime links.
    assert!(bound.as_mut().poll(&mut cx).is_pending());
    assert!(bound.as_mut().poll(&mut cx).is_pending());
    assert_eq!(root_stats.acquisitions.get(), 0);
    for layer in &layers {
        assert_eq!(layer.builds.get(), 0);
    }
    assert_eq!(lock.try_lock().unwrap().byte, 7);
    if pause_after_read {
        assert!(bound.as_mut().poll(&mut cx).is_pending());
        assert_eq!(lock.try_lock().unwrap().byte, 8);
        assert_eq!(root_stats.drops.get(), 1);
        for layer in &layers {
            assert_eq!((layer.builds.get(), layer.drops.get()), (1, 1));
        }
    }
    assert_eq!(bound.as_mut().poll(&mut cx), Poll::Ready((7, 8)));
    let rounds = if pause_after_read { 2 } else { 1 };
    assert_eq!(
        (
            root_stats.acquisitions.get(),
            root_stats.builds.get(),
            root_stats.drops.get()
        ),
        (rounds, rounds, rounds)
    );
    for layer in &layers {
        assert_eq!((layer.builds.get(), layer.drops.get()), (rounds, rounds));
    }
    assert_eq!(lock.try_lock().unwrap().byte, 9);
}

#[test]
fn three_lifetime_links_preserve_static_capabilities_and_cache_identity() {
    exercise(false);
}

#[test]
fn lifetime_links_drop_before_coroutine_suspension_and_rebuild_next_round() {
    exercise(true);
}

#[test]
fn lifetime_erased_loan_is_usable_after_a_visitor_panics() {
    struct Panic;
    impl<F: HostFamily> ViewOperation<F> for Panic {
        type Output = ();
        fn run<'v>(self, _: Pin<&mut F::HostView<'v>>)
        where
            F: 'v,
        {
            panic!("visitor panic");
        }
    }
    let label = String::from("borrowed");
    let lock = Mutex::new(Data {
        label: &label,
        byte: 7,
    });
    let stats = Stats::default();
    let mut root = pin!(RootView {
        guard: lock.lock().unwrap(),
        stats: &stats,
        invariant: PhantomData,
        pinned: PhantomPinned
    });
    let mut loan = ViewLoan::<Root<'_>>::new(root.as_mut());
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loan.apply(Panic)));
    assert!(failed.is_err());
    assert_eq!(loan.apply(ReadParent::<Root<'_>>(PhantomData)), 7);
    assert_eq!(loan.apply(MeasureParent::<Root<'_>>(PhantomData)), 8);
}
