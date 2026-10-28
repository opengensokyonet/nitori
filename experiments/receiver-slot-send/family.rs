#![feature(async_fn_traits, unboxed_closures)]
#![allow(dead_code)]
use std::{
    cell::{Cell, RefCell},
    future::Future,
    marker::PhantomData,
    ops::AsyncFnOnce,
    pin::Pin,
    ptr::{self, NonNull},
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    thread::{self, ThreadId},
};

trait ReceiverFamily: Sized {
    type ReceiverView<'v>
    where
        Self: 'v;
}
struct Access<'a, 'v, F: ReceiverFamily + 'v> {
    view: Pin<&'a mut F::ReceiverView<'v>>,
}
impl<'a, 'v, F: ReceiverFamily + 'v> Access<'a, 'v, F> {
    fn new(view: Pin<&'a mut F::ReceiverView<'v>>) -> Self {
        Self { view }
    }
    fn into_pin(self) -> Pin<&'a mut F::ReceiverView<'v>> {
        self.view
    }
}
/// A synchronous operation on a view. Its result cannot borrow the view.
trait ViewOperation<F: ReceiverFamily> {
    type Output;
    fn run<'parent>(self, view: Pin<&mut F::ReceiverView<'parent>>) -> Self::Output
    where
        F: 'parent;
}
struct ClosureOperation<B>(B);
impl<F: ReceiverFamily, B, R> ViewOperation<F> for ClosureOperation<B>
where
    B: for<'access, 'parent> FnOnce(Access<'access, 'parent, F>) -> R,
{
    type Output = R;
    fn run<'parent>(self, view: Pin<&mut F::ReceiverView<'parent>>) -> R
    where
        F: 'parent,
    {
        (self.0)(Access::new(view))
    }
}

/// The dyn form of a view operation. It quantifies only a lifetime, so the
/// family stays an exact type.
trait DynOperation<F: ReceiverFamily> {
    fn run<'parent>(&mut self, view: Pin<&mut F::ReceiverView<'parent>>)
    where
        F: 'parent;
}
/// Runs a view operation once through its dyn form.
struct Once<F: ReceiverFamily, A: ViewOperation<F>> {
    operation: Option<A>,
    output: Option<A::Output>,
    family: PhantomData<fn(F) -> F>,
}
impl<F: ReceiverFamily, A: ViewOperation<F>> DynOperation<F> for Once<F, A> {
    fn run<'parent>(&mut self, view: Pin<&mut F::ReceiverView<'parent>>)
    where
        F: 'parent,
    {
        let operation = self.operation.take().expect("view operation ran twice");
        self.output = Some(operation.run(view));
    }
}

/// A pinned view and the function that restores its exact type. Copying it
/// does not copy the borrow: its users keep the view live and exclusive.
struct RawViewLoan<F: ReceiverFamily> {
    view: *mut (),
    restore: unsafe fn(*mut (), &mut dyn DynOperation<F>),
}
impl<F: ReceiverFamily> Clone for RawViewLoan<F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F: ReceiverFamily> Copy for RawViewLoan<F> {}
/// Carries the view's inner lifetime to the restoring function.
struct Restore<'parent, F: ReceiverFamily + 'parent>(PhantomData<&'parent F>);
impl<'parent, F: ReceiverFamily + 'parent> Restore<'parent, F> {
    unsafe fn run(view: *mut (), operation: &mut dyn DynOperation<F>) {
        // SAFETY: RawViewLoan::new pairs this function with a pointer to
        // exactly F::ReceiverView<'parent>, and RawViewLoan::apply's caller
        // keeps that borrow live and exclusive. The pointee stays pinned.
        let view = unsafe { Pin::new_unchecked(&mut *view.cast::<F::ReceiverView<'parent>>()) };
        operation.run(view);
    }
}
impl<F: ReceiverFamily> RawViewLoan<F> {
    fn new<'parent>(view: Pin<&mut F::ReceiverView<'parent>>) -> Self
    where
        F: 'parent,
    {
        // SAFETY: taking a raw pointer does not move the pinned pointee; only
        // the paired function uses it, and it recreates a pinned borrow.
        let view = unsafe { view.get_unchecked_mut() as *mut F::ReceiverView<'parent> }.cast();
        Self {
            view,
            restore: Restore::<'parent, F>::run,
        }
    }
    /// # Safety
    /// The view borrow given to `new` must be live, and no other use of it
    /// may be in progress.
    unsafe fn apply<A: ViewOperation<F>>(self, operation: A) -> A::Output {
        let mut once = Once::<F, A> {
            operation: Some(operation),
            output: None,
            family: PhantomData,
        };
        // SAFETY: the caller's contract; the function and pointer are paired.
        unsafe { (self.restore)(self.view, &mut once) };
        once.output.unwrap()
    }
}

// Only the shared wake forwarding path needs synchronization. Per-slot state
// has one accessor during poll; the driver accesses it only outside poll.
struct Intercept {
    caller: Mutex<Option<Waker>>,
}
thread_local! {
 static REPLAYING:Cell<*const Intercept> = const {Cell::new(ptr::null())};
 static CHECKS:Cell<usize> = const {Cell::new(0)};
}
impl Intercept {
    fn replay(self: &Arc<Self>, branches: impl IntoIterator<Item = Waker>) {
        struct Reset(*const Intercept);
        impl Drop for Reset {
            fn drop(&mut self) {
                REPLAYING.set(self.0);
            }
        }
        let _reset = Reset(REPLAYING.replace(Arc::as_ptr(self)));
        for waker in branches {
            waker.wake();
        }
    }
}
impl Wake for Intercept {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if !ptr::eq(REPLAYING.get(), Arc::as_ptr(self)) {
            // Never invoke external waker code with our mutex held.
            let caller = self.caller.lock().unwrap().clone();
            if let Some(caller) = caller {
                caller.wake();
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    Idle,
    NeedsView,
    Releasable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AccessError {
    WrongThread,
}
struct State<F: ReceiverFamily> {
    loan: Cell<Option<RawViewLoan<F>>>,
    owner: Cell<Option<ThreadId>>,
    check: Cell<fn(Option<ThreadId>) -> bool>,
    signal: Cell<Signal>,
    releasable: Cell<bool>,
    answered_release: Cell<bool>,
    branch: RefCell<Option<Waker>>,
    root: RefCell<Option<Waker>>,
}
fn local(owner: Option<ThreadId>) -> bool {
    CHECKS.set(CHECKS.get() + 1);
    owner == Some(thread::current().id())
}
fn unrestricted(_: Option<ThreadId>) -> bool {
    true
}
impl<F: ReceiverFamily> State<F> {
    fn ask(&self, signal: Signal, cx: &Context<'_>) {
        self.signal.set(signal);
        let direct = self
            .root
            .borrow()
            .as_ref()
            .is_some_and(|w| cx.waker().will_wake(w));
        // One exclusive pending operation per slot. Re-polling replaces a stale
        // branch waker; different slots retain independent branch wakers.
        *self.branch.borrow_mut() = (!direct).then(|| cx.waker().clone());
    }
    fn cancel(&self) {
        self.signal.set(Signal::Idle);
        self.answered_release.set(false);
        self.branch.borrow_mut().take();
    }
}
struct Endpoint<F: ReceiverFamily> {
    state: NonNull<State<F>>,
}
impl<F: ReceiverFamily> Endpoint<F> {
    fn new() -> Self {
        Self {
            state: NonNull::new(Box::into_raw(Box::new(State {
                loan: Cell::new(None),
                owner: Cell::new(None),
                check: Cell::new(unrestricted),
                signal: Cell::new(Signal::Idle),
                releasable: Cell::new(false),
                answered_release: Cell::new(false),
                branch: RefCell::new(None),
                root: RefCell::new(None),
            })))
            .unwrap(),
        }
    }
    fn state(&self) -> &State<F> {
        // SAFETY: stable raw-owned allocation, freed only after the actual future.
        unsafe { self.state.as_ref() }
    }
    fn lend_local<'v, R>(
        &mut self,
        view: Pin<&mut F::ReceiverView<'v>>,
        release: bool,
        intercept: &Arc<Intercept>,
        body: impl FnOnce() -> R,
    ) -> R
    where
        F: 'v,
    {
        self.lend(
            view,
            release,
            Some(thread::current().id()),
            local,
            intercept,
            body,
        )
    }
    fn lend_send<'v, R>(
        &mut self,
        view: Pin<&mut F::ReceiverView<'v>>,
        release: bool,
        intercept: &Arc<Intercept>,
        body: impl FnOnce() -> R,
    ) -> R
    where
        F: 'v,
        F::ReceiverView<'v>: Send,
    {
        // Proof applies to this exact lent type/lifetime, not to all family views.
        self.lend(view, release, None, unrestricted, intercept, body)
    }
    fn lend<'v, R>(
        &mut self,
        view: Pin<&mut F::ReceiverView<'v>>,
        release: bool,
        owner: Option<ThreadId>,
        check: fn(Option<ThreadId>) -> bool,
        intercept: &Arc<Intercept>,
        body: impl FnOnce() -> R,
    ) -> R
    where
        F: 'v,
    {
        let state = self.state();
        assert!(state.loan.get().is_none());
        struct Clear<'a, F: ReceiverFamily>(&'a State<F>);
        impl<F: ReceiverFamily> Drop for Clear<'_, F> {
            fn drop(&mut self) {
                self.0.loan.set(None);
                self.0.owner.set(None);
                self.0.releasable.set(false);
            }
        }
        let _clear = Clear(state);
        state.owner.set(owner);
        state.check.set(check);
        state.releasable.set(release);
        state.loan.set(Some(RawViewLoan::new(view)));
        if state.signal.get() == Signal::NeedsView {
            state.signal.set(Signal::Idle);
            let branch = state.branch.borrow_mut().take();
            intercept.replay(branch);
        }
        body()
    }
}
impl<F: ReceiverFamily> Drop for Endpoint<F> {
    fn drop(&mut self) {
        unsafe {
            drop(Box::from_raw(self.state.as_ptr()));
        }
    }
}
// SAFETY: loans are cleared before this owner can move, including on unwind.
// The lifetime-polymorphic factory prevents branded slots escaping the call.
unsafe impl<F: ReceiverFamily> Send for Endpoint<F> {}
struct ReceiverSlot<'s, F: ReceiverFamily> {
    state: *const State<F>,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
    family: PhantomData<fn(F) -> F>,
    not_sync: PhantomData<Cell<()>>,
}
// SAFETY: unique !Sync handle, methods require &mut; scoped work must join
// before poll/drop returns. Driver doesn't inspect any state during that work.
// A non-Send view is checked BEFORE creating a reference to its pointee.
unsafe impl<F: ReceiverFamily> Send for ReceiverSlot<'_, F> {}
impl<F: ReceiverFamily> Drop for ReceiverSlot<'_, F> {
    fn drop(&mut self) {
        self.state().cancel();
    }
}
impl<'s, F: ReceiverFamily> ReceiverSlot<'s, F> {
    fn state(&self) -> &State<F> {
        unsafe { &*self.state }
    }
    fn access<B, R>(&mut self, body: B) -> Read<'_, 's, F, B, R>
    where
        B: for<'a, 'v> FnOnce(Access<'a, 'v, F>) -> R,
    {
        Read {
            slot: self,
            body: Some(body),
            output: PhantomData,
        }
    }
    fn release(&mut self) -> Release<'_, 's, F> {
        Release {
            slot: self,
            offered: false,
        }
    }
}
struct Read<'a, 's, F: ReceiverFamily, B, R> {
    slot: &'a mut ReceiverSlot<'s, F>,
    body: Option<B>,
    output: PhantomData<fn() -> R>,
}
// Body isn't structurally pinned: it is taken before invocation.
impl<F: ReceiverFamily, B, R> Unpin for Read<'_, '_, F, B, R> {}
impl<F: ReceiverFamily, B, R> Future for Read<'_, '_, F, B, R>
where
    B: for<'a, 'v> FnOnce(Access<'a, 'v, F>) -> R,
{
    type Output = Result<R, AccessError>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let state = this.slot.state();
        let Some(loan) = state.loan.get() else {
            state.ask(Signal::NeedsView, cx);
            return Poll::Pending;
        };
        state.cancel();
        if !(state.check.get())(state.owner.get()) {
            return Poll::Ready(Err(AccessError::WrongThread));
        }
        let body = this.body.take().expect("read polled after completion");
        // SAFETY: lend keeps the exact pinned view borrowed until this poll ends;
        // exclusive slot access and the HRTB operation prevent aliasing/escape.
        Poll::Ready(Ok(unsafe { loan.apply(ClosureOperation(body)) }))
    }
}
impl<F: ReceiverFamily, B, R> Drop for Read<'_, '_, F, B, R> {
    fn drop(&mut self) {
        self.slot.state().cancel();
    }
}
struct Release<'a, 's, F: ReceiverFamily> {
    slot: &'a mut ReceiverSlot<'s, F>,
    offered: bool,
}
impl<F: ReceiverFamily> Future for Release<'_, '_, F> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let state = this.slot.state();
        if !state.releasable.get() || (this.offered && state.answered_release.get()) {
            state.cancel();
            return Poll::Ready(());
        }
        this.offered = true;
        state.ask(Signal::Releasable, cx);
        Poll::Pending
    }
}
impl<F: ReceiverFamily> Drop for Release<'_, '_, F> {
    fn drop(&mut self) {
        self.slot.state().cancel();
    }
}
// Private recursive tuple construction allows independently typed slots.
// No runtime registry or fixed slot-count is required for construction.
trait Endpoints {
    type Slots<'s>;
    unsafe fn slots<'s>(&self) -> Self::Slots<'s>;
    fn prepare(&mut self, root: &Waker, intercept: &Arc<Intercept>);
    fn clear(&mut self);
    fn signals(&self, out: &mut Vec<Signal>);
}
impl<F: ReceiverFamily> Endpoints for Endpoint<F> {
    type Slots<'s> = ReceiverSlot<'s, F>;
    unsafe fn slots<'s>(&self) -> Self::Slots<'s> {
        ReceiverSlot {
            state: self.state.as_ptr(),
            brand: PhantomData,
            family: PhantomData,
            not_sync: PhantomData,
        }
    }
    fn prepare(&mut self, root: &Waker, intercept: &Arc<Intercept>) {
        let state = self.state();
        *state.root.borrow_mut() = Some(root.clone());
        let answer = state.signal.get() == Signal::Releasable;
        if answer {
            // An intermediate executor may skip this branch during the answering poll.
            // Keep the answer until the exclusive release operation consumes/cancels it.
            state.answered_release.set(true);
            state.signal.set(Signal::Idle);
            let branch = state.branch.borrow_mut().take();
            intercept.replay(branch);
        }
    }
    fn clear(&mut self) {
        self.state().root.borrow_mut().take();
    }
    fn signals(&self, out: &mut Vec<Signal>) {
        out.push(self.state().signal.get());
    }
}
impl<L: Endpoints, R: Endpoints> Endpoints for (L, R) {
    type Slots<'s> = (L::Slots<'s>, R::Slots<'s>);
    unsafe fn slots<'s>(&self) -> Self::Slots<'s> {
        unsafe { (self.0.slots(), self.1.slots()) }
    }
    fn prepare(&mut self, root: &Waker, intercept: &Arc<Intercept>) {
        self.0.prepare(root, intercept);
        self.1.prepare(root, intercept);
    }
    fn clear(&mut self) {
        self.0.clear();
        self.1.clear();
    }
    fn signals(&self, out: &mut Vec<Signal>) {
        self.0.signals(out);
        self.1.signals(out);
    }
}
type Running<C, E> = <C as AsyncFnOnce<(<E as Endpoints>::Slots<'static>,)>>::CallOnceFuture;
struct Call<C, E: Endpoints, R>
where
    C: for<'s> AsyncFnOnce(E::Slots<'s>) -> R,
{
    // Field order is a safety invariant: destroy slots before their allocations.
    future: Pin<Box<Running<C, E>>>,
    endpoints: E,
    intercept: Arc<Intercept>,
    poisoned: bool,
    finished: bool,
}
impl<C, E: Endpoints, R> Call<C, E, R>
where
    C: for<'s> AsyncFnOnce(E::Slots<'s>) -> R,
{
    fn new(endpoints: E, factory: C) -> Self {
        let future = Box::pin(factory(unsafe { endpoints.slots() }));
        Self {
            future,
            endpoints,
            intercept: Arc::new(Intercept {
                caller: Mutex::new(None),
            }),
            poisoned: false,
            finished: false,
        }
    }
    fn poll_with(
        &mut self,
        cx: &mut Context<'_>,
        lend: impl FnOnce(&mut E, &Arc<Intercept>, &mut dyn FnMut() -> Poll<R>) -> Poll<R>,
    ) -> Poll<R> {
        assert!(
            !self.poisoned && !self.finished,
            "call is poisoned or finished"
        );
        self.poisoned = true;
        *self.intercept.caller.lock().unwrap() = Some(cx.waker().clone());
        let root = Waker::from(self.intercept.clone());
        // Clear root state on panic, including panic in waker replay/loan setup.
        struct Clear<'a, E: Endpoints>(&'a mut E);
        impl<E: Endpoints> Drop for Clear<'_, E> {
            fn drop(&mut self) {
                self.0.clear();
            }
        }
        let clear = Clear(&mut self.endpoints);
        clear.0.prepare(&root, &self.intercept);
        let mut inner = Context::from_waker(&root);
        let mut polled = false;
        let result = lend(clear.0, &self.intercept, &mut || {
            assert!(!polled, "poll callback invoked twice");
            polled = true;
            let result = self.future.as_mut().poll(&mut inner);
            self.finished = result.is_ready();
            result
        });
        drop(clear);
        self.poisoned = false;
        result
    }
    fn poll_absent(&mut self, cx: &mut Context<'_>) -> Poll<R> {
        self.poll_with(cx, |_, _, poll| poll())
    }
    fn signals(&self) -> Vec<Signal> {
        let mut out = Vec::new();
        self.endpoints.signals(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
    use std::{
        future::poll_fn,
        marker::PhantomPinned,
        rc::Rc,
        sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering::SeqCst},
        },
    };
    struct Direct<T>(PhantomData<fn(T) -> T>);
    struct View<'v, T>(&'v mut T);
    impl<T> ReceiverFamily for Direct<T> {
        type ReceiverView<'v>
            = View<'v, T>
        where
            Self: 'v;
    }
    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, SeqCst);
        }
    }
    fn counter() -> (Arc<Count>, Waker) {
        let c = Arc::new(Count(AtomicUsize::new(0)));
        (c.clone(), Waker::from(c))
    }
    fn assert_send<T: Send>(_: &T) {}
    async fn yield_once() {
        let mut done = false;
        poll_fn(move |cx| {
            if std::mem::replace(&mut done, true) {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await
    }

    #[test]
    fn gat_local_view_and_actual_future_migrate() {
        let mut call = Call::new(
            (
                Endpoint::<Direct<Rc<Cell<u64>>>>::new(),
                Endpoint::<Direct<u64>>::new(),
            ),
            async |(mut a, mut b): (
                ReceiverSlot<'_, Direct<Rc<Cell<u64>>>>,
                ReceiverSlot<'_, Direct<u64>>,
            )| {
                let first = a.access(|v| v.into_pin().get_mut().0.get()).await.unwrap();
                yield_once().await;
                first
                    + b.access(|v| *v.into_pin().get_mut().0).await.unwrap()
                    + a.access(|v| v.into_pin().get_mut().0.get()).await.unwrap()
            },
        );
        assert_send(&call);
        assert_send(&call.future);
        CHECKS.set(0);
        let mut a = Rc::new(Cell::new(2));
        let mut b = 3;
        assert!(
            call.poll_with(
                &mut Context::from_waker(Waker::noop()),
                |(a_owner, b_owner), intercept, poll| {
                    a_owner.lend_local(Pin::new(&mut View(&mut a)), false, intercept, || {
                        b_owner.lend_send(Pin::new(&mut View(&mut b)), false, intercept, poll)
                    })
                }
            )
            .is_pending()
        );
        assert_eq!(CHECKS.get(), 1);
        thread::spawn(move || {
            CHECKS.set(0);
            let mut a = Rc::new(Cell::new(5));
            let mut b = 7;
            assert_eq!(
                call.poll_with(
                    &mut Context::from_waker(Waker::noop()),
                    |(a_owner, b_owner), intercept, poll| {
                        a_owner.lend_local(Pin::new(&mut View(&mut a)), false, intercept, || {
                            b_owner.lend_send(Pin::new(&mut View(&mut b)), false, intercept, poll)
                        })
                    }
                ),
                Poll::Ready(14)
            );
            assert_eq!(CHECKS.get(), 1);
        })
        .join()
        .unwrap();
    }

    // Family and view carry independent, genuinely borrowed lifetimes.
    struct Borrowed<'data>(PhantomData<&'data [u8]>);
    struct BorrowedView<'v, 'data> {
        data: &'data [u8],
        count: &'v mut u64,
        _pin: PhantomPinned,
    }
    impl<'data> ReceiverFamily for Borrowed<'data> {
        type ReceiverView<'v>
            = BorrowedView<'v, 'data>
        where
            Self: 'v;
    }
    #[test]
    fn borrowed_family_and_non_unpin_view() {
        fn run<'data>(data: &'data [u8]) -> u64 {
            let mut output = 0;
            let mut call = Call::new(
                Endpoint::<Borrowed<'data>>::new(),
                async |mut slot: ReceiverSlot<'_, Borrowed<'data>>| {
                    output = slot
                        .access(|view| {
                            let view = view.into_pin();
                            let view = view.as_ref().get_ref();
                            view.data.len() as u64 + *view.count
                        })
                        .await
                        .unwrap();
                },
            );
            let mut count = 9;
            let view = BorrowedView {
                data,
                count: &mut count,
                _pin: PhantomPinned,
            };
            let mut view = std::pin::pin!(view);
            let address = &*view as *const _;
            assert_eq!(
                call.poll_with(
                    &mut Context::from_waker(Waker::noop()),
                    |owner, intercept, poll| owner.lend_send(view.as_mut(), false, intercept, poll)
                ),
                Poll::Ready(())
            );
            assert_eq!(&*view as *const _, address);
            drop(call);
            output
        }
        let bytes = vec![1, 2, 3, 4];
        assert_eq!(run(&bytes), 13);
    }

    #[test]
    fn three_slots_in_futures_unordered_wake_only_supplied_branches() {
        let counts = Arc::new([
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        ]);
        let branch_counts = counts.clone();
        let mut call = Call::new(
            (
                Endpoint::<Direct<u64>>::new(),
                (
                    Endpoint::<Direct<u64>>::new(),
                    Endpoint::<Direct<u64>>::new(),
                ),
            ),
            async move |(a, (b, c)): (
                ReceiverSlot<'_, Direct<u64>>,
                (ReceiverSlot<'_, Direct<u64>>, ReceiverSlot<'_, Direct<u64>>),
            )| {
                let mut futures = FuturesUnordered::new();
                for (index, mut slot) in [a, b, c].into_iter().enumerate() {
                    let counts = branch_counts.clone();
                    futures.push(
                        async move {
                            let mut read = slot.access(|v| *v.into_pin().get_mut().0);
                            poll_fn(|cx| {
                                counts[index].fetch_add(1, SeqCst);
                                Pin::new(&mut read).poll(cx)
                            })
                            .await
                            .unwrap()
                        }
                        .boxed(),
                    );
                }
                let mut sum = 0;
                while let Some(value) = futures.next().await {
                    sum += value;
                }
                sum
            },
        );
        assert_send(&call);
        let (wakes, waker) = counter();
        let mut cx = Context::from_waker(&waker);
        assert!(call.poll_absent(&mut cx).is_pending());
        assert_eq!(call.signals(), vec![Signal::NeedsView; 3]);
        assert_eq!(counts.each_ref().map(|x| x.load(SeqCst)), [1, 1, 1]);
        let baseline = wakes.0.load(SeqCst);
        // Polling with no answer must not re-poll any sleeping FU branch.
        assert!(call.poll_absent(&mut cx).is_pending());
        assert_eq!(counts.each_ref().map(|x| x.load(SeqCst)), [1, 1, 1]);
        let mut a = 2;
        assert!(
            call.poll_with(&mut cx, |(ao, _), i, poll| ao.lend_send(
                Pin::new(&mut View(&mut a)),
                false,
                i,
                poll
            ))
            .is_pending()
        );
        assert_eq!(counts.each_ref().map(|x| x.load(SeqCst)), [2, 1, 1]);
        assert_eq!(
            call.signals(),
            vec![Signal::Idle, Signal::NeedsView, Signal::NeedsView]
        );
        let mut b = 3;
        let mut c = 5;
        assert_eq!(
            call.poll_with(&mut cx, |(_, (bo, co)), i, poll| bo.lend_send(
                Pin::new(&mut View(&mut b)),
                false,
                i,
                || co.lend_send(Pin::new(&mut View(&mut c)), false, i, poll)
            )),
            Poll::Ready(10)
        );
        assert_eq!(counts.each_ref().map(|x| x.load(SeqCst)), [2, 2, 2]);
        assert_eq!(wakes.0.load(SeqCst), baseline);
    }

    #[test]
    fn distinct_slot_requests_can_be_written_concurrently() {
        let mut call = Call::new(
            (
                Endpoint::<Direct<u64>>::new(),
                Endpoint::<Direct<u64>>::new(),
            ),
            async |(mut a, mut b): (
                ReceiverSlot<'_, Direct<u64>>,
                ReceiverSlot<'_, Direct<u64>>,
            )| {
                let mut ra = a.access(|v| *v.into_pin().get_mut().0);
                let mut rb = b.access(|v| *v.into_pin().get_mut().0);
                poll_fn(|cx| {
                    let barrier = Arc::new(Barrier::new(2));
                    let barrier_b = barrier.clone();
                    let (a, b) = thread::scope(|scope| {
                        let wa = cx.waker().clone();
                        let wb = wa.clone();
                        let ra = &mut ra;
                        let rb = &mut rb;
                        let a = scope.spawn(move || {
                            barrier.wait();
                            Pin::new(ra).poll(&mut Context::from_waker(&wa))
                        });
                        let b = scope.spawn(move || {
                            barrier_b.wait();
                            Pin::new(rb).poll(&mut Context::from_waker(&wb))
                        });
                        (a.join().unwrap(), b.join().unwrap())
                    });
                    match (a, b) {
                        (Poll::Ready(Ok(a)), Poll::Ready(Ok(b))) => Poll::Ready(a + b),
                        (Poll::Pending, Poll::Pending) => Poll::Pending,
                        _ => panic!("unexpected partial response"),
                    }
                })
                .await
            },
        );
        let (wakes, waker) = counter();
        let mut cx = Context::from_waker(&waker);
        assert!(call.poll_absent(&mut cx).is_pending());
        assert_eq!(call.signals(), vec![Signal::NeedsView; 2]);
        assert_eq!(wakes.0.load(SeqCst), 0);
        CHECKS.set(0);
        let mut a = 4;
        let mut b = 6;
        assert_eq!(
            call.poll_with(&mut cx, |(ao, bo), i, poll| ao.lend_send(
                Pin::new(&mut View(&mut a)),
                false,
                i,
                || bo.lend_send(Pin::new(&mut View(&mut b)), false, i, poll)
            )),
            Poll::Ready(10)
        );
        assert_eq!(CHECKS.get(), 0);
    }

    #[test]
    fn wrong_thread_is_a_ready_error_without_view_access() {
        let mut call = Call::new(
            Endpoint::<Direct<Rc<Cell<u64>>>>::new(),
            async |mut slot: ReceiverSlot<'_, Direct<Rc<Cell<u64>>>>| {
                poll_fn(|cx| {
                    thread::scope(|scope| {
                        let w = cx.waker().clone();
                        let slot = &mut slot;
                        scope
                            .spawn(move || {
                                let mut read =
                                    slot.access(|_| panic!("must not access non-Send view"));
                                Pin::new(&mut read).poll(&mut Context::from_waker(&w))
                            })
                            .join()
                            .unwrap()
                    })
                })
                .await
            },
        );
        let mut local = Rc::new(Cell::new(42));
        assert_eq!(
            call.poll_with(&mut Context::from_waker(Waker::noop()), |owner, i, poll| {
                owner.lend_local(Pin::new(&mut View(&mut local)), false, i, poll)
            }),
            Poll::Ready(Err(AccessError::WrongThread))
        );
    }

    #[test]
    fn cancelling_pending_access_clears_its_request() {
        let mut call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |mut slot: ReceiverSlot<'_, Direct<u64>>| {
                let mut read = slot.access(|v| *v.into_pin().get_mut().0);
                poll_fn(|cx| {
                    assert!(Pin::new(&mut read).poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                drop(read);
                std::future::pending::<()>().await;
            },
        );
        assert!(
            call.poll_absent(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(call.signals(), vec![Signal::Idle]);
    }

    #[test]
    fn release_through_futures_unordered_answers_keep_or_drop() {
        for keep in [false, true] {
            let mut call = Call::new(
                Endpoint::<Direct<u64>>::new(),
                async |mut slot: ReceiverSlot<'_, Direct<u64>>| {
                    let mut futures = FuturesUnordered::new();
                    futures.push(async move {
                        slot.access(|v| *v.into_pin().get_mut().0).await.unwrap();
                        slot.release().await;
                        9
                    });
                    futures.next().await.unwrap()
                },
            );
            let (wakes, waker) = counter();
            let mut cx = Context::from_waker(&waker);
            let mut value = 2;
            assert!(
                call.poll_with(&mut cx, |o, i, poll| o.lend_send(
                    Pin::new(&mut View(&mut value)),
                    true,
                    i,
                    poll
                ))
                .is_pending()
            );
            assert_eq!(call.signals(), vec![Signal::Releasable]);
            let baseline = wakes.0.load(SeqCst);
            let result = if keep {
                call.poll_with(&mut cx, |o, i, poll| {
                    o.lend_send(Pin::new(&mut View(&mut value)), true, i, poll)
                })
            } else {
                call.poll_absent(&mut cx)
            };
            assert_eq!(result, Poll::Ready(9));
            assert_eq!(wakes.0.load(SeqCst), baseline);
        }
    }

    #[test]
    fn poll_panic_clears_loans_and_poison_prevents_reentry() {
        let mut call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |mut slot: ReceiverSlot<'_, Direct<u64>>| {
                slot.access(|_| panic!("operation panic")).await.unwrap()
            },
        );
        let mut value = 4;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call.poll_with(&mut Context::from_waker(Waker::noop()), |o, i, poll| {
                o.lend_send(Pin::new(&mut View(&mut value)), false, i, poll)
            })
        }));
        assert!(result.is_err());
        assert!(call.endpoints.state().loan.get().is_none());
        assert!(call.endpoints.state().root.borrow().is_none());
        assert!(call.poisoned);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || call.poll_absent(&mut Context::from_waker(Waker::noop()))
            ))
            .is_err()
        );
    }
    #[test]
    fn release_answer_survives_a_poll_that_does_not_reach_the_branch() {
        let mut call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |mut slot: ReceiverSlot<'_, Direct<u64>>| {
                let mut futures = FuturesUnordered::new();
                futures.push(async move {
                    slot.release().await;
                    9
                });
                let mut polls = 0;
                poll_fn(|cx| {
                    polls += 1;
                    if polls == 2 {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    } else {
                        futures.poll_next_unpin(cx)
                    }
                })
                .await
                .unwrap()
            },
        );
        let (_, waker) = counter();
        let mut cx = Context::from_waker(&waker);
        let mut value = 2;
        assert!(
            call.poll_with(&mut cx, |o, i, poll| o.lend_send(
                Pin::new(&mut View(&mut value)),
                true,
                i,
                poll
            ))
            .is_pending()
        );
        assert_eq!(call.signals(), vec![Signal::Releasable]);
        assert!(
            call.poll_with(&mut cx, |o, i, poll| o.lend_send(
                Pin::new(&mut View(&mut value)),
                true,
                i,
                poll
            ))
            .is_pending()
        );
        assert_eq!(
            call.poll_with(&mut cx, |o, i, poll| o.lend_send(
                Pin::new(&mut View(&mut value)),
                true,
                i,
                poll
            )),
            Poll::Ready(9)
        );
    }

    #[test]
    fn replay_only_suppresses_same_intercept_on_same_thread() {
        let (parent_count, parent) = counter();
        let (other_count, other) = counter();
        let intercept = Arc::new(Intercept {
            caller: Mutex::new(Some(parent)),
        });
        let same = Waker::from(intercept.clone());
        struct FanOut {
            same: Waker,
            other: Waker,
        }
        impl Wake for FanOut {
            fn wake(self: Arc<Self>) {
                self.same.wake_by_ref(); // Synchronous echo at replaying call: suppressed.
                self.other.wake_by_ref(); // Unrelated call: must pass.
                thread::scope(|scope| {
                    scope.spawn(|| self.same.wake_by_ref()).join().unwrap();
                });
            }
        }
        intercept.replay([Waker::from(Arc::new(FanOut {
            same: same.clone(),
            other,
        }))]);
        assert_eq!(parent_count.0.load(SeqCst), 1); // Concurrent wake survives replay.
        assert_eq!(other_count.0.load(SeqCst), 1);
        same.wake_by_ref();
        assert_eq!(parent_count.0.load(SeqCst), 2);
    }

    #[test]
    fn cancellation_drop_after_migration_sees_no_installed_view() {
        struct OnDrop<'s>(ReceiverSlot<'s, Direct<Rc<Cell<u64>>>>);
        impl Drop for OnDrop<'_> {
            fn drop(&mut self) {
                let mut read = self.0.access(|_| panic!("cancelled call retains a view"));
                assert!(
                    Pin::new(&mut read)
                        .poll(&mut Context::from_waker(Waker::noop()))
                        .is_pending()
                );
            }
        }
        let mut call = Call::new(
            Endpoint::<Direct<Rc<Cell<u64>>>>::new(),
            async |slot: ReceiverSlot<'_, Direct<Rc<Cell<u64>>>>| {
                let mut held = OnDrop(slot);
                held.0
                    .access(|v| v.into_pin().get_mut().0.get())
                    .await
                    .unwrap();
                std::future::pending::<()>().await;
                drop(held);
            },
        );
        let mut value = Rc::new(Cell::new(3));
        assert!(
            call.poll_with(&mut Context::from_waker(Waker::noop()), |o, i, poll| o
                .lend_local(Pin::new(&mut View(&mut value)), false, i, poll))
                .is_pending()
        );
        drop(value);
        thread::spawn(move || drop(call)).join().unwrap();
    }

    #[test]
    fn internal_self_borrow_survives_shared_reference_between_polls() {
        let mut call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |mut slot: ReceiverSlot<'_, Direct<u64>>| {
                let mut local = 5;
                let borrowed = &mut local;
                yield_once().await;
                *borrowed += slot.access(|v| *v.into_pin().get_mut().0).await.unwrap();
                *borrowed
            },
        );
        let mut cx = Context::from_waker(Waker::noop());
        assert!(call.poll_absent(&mut cx).is_pending());
        std::hint::black_box(&call);
        let mut value = 7;
        assert_eq!(
            call.poll_with(&mut cx, |o, i, poll| o.lend_send(
                Pin::new(&mut View(&mut value)),
                false,
                i,
                poll
            )),
            Poll::Ready(12)
        );
    }

    // Each cfg below must fail compilation for its named safety boundary.
    #[cfg(reject_rc)]
    #[test]
    fn rejects_unrelated_rc_across_await() {
        let call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |_: ReceiverSlot<'_, Direct<u64>>| {
                let rc = Rc::new(2);
                yield_once().await;
                *rc
            },
        );
        assert_send(&call);
    }
    #[cfg(reject_return)]
    #[test]
    fn rejects_returning_slot() {
        let _call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |slot: ReceiverSlot<'_, Direct<u64>>| slot,
        );
    }
    #[cfg(reject_escape)]
    #[test]
    fn rejects_outer_scoped_thread() {
        thread::scope(|scope| {
            let _call = Call::new(
                Endpoint::<Direct<u64>>::new(),
                async |mut slot: ReceiverSlot<'_, Direct<u64>>| {
                    scope.spawn(move || {
                        let _read = slot.access(|v| *v.into_pin().get_mut().0);
                    });
                },
            );
        });
    }
    #[cfg(reject_view)]
    #[test]
    fn rejects_escaping_inner_view() {
        let _call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |mut slot: ReceiverSlot<'_, Direct<u64>>| {
                let view = slot.access(|v| v.into_pin()).await.unwrap();
                yield_once().await;
                drop(view);
            },
        );
    }
    #[cfg(reject_sync)]
    #[test]
    fn rejects_shared_slot_threads() {
        let _call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |slot: ReceiverSlot<'_, Direct<u64>>| {
                thread::scope(|scope| {
                    let shared = &slot;
                    scope.spawn(move || std::hint::black_box(shared));
                });
            },
        );
    }
    #[cfg(reject_send_loan)]
    #[test]
    fn rejects_non_send_fast_loan() {
        let mut call = Call::new(
            Endpoint::<Direct<Rc<Cell<u64>>>>::new(),
            async |_: ReceiverSlot<'_, Direct<Rc<Cell<u64>>>>| (),
        );
        let mut value = Rc::new(Cell::new(2));
        call.poll_with(&mut Context::from_waker(Waker::noop()), |o, i, poll| {
            o.lend_send(Pin::new(&mut View(&mut value)), false, i, poll)
        });
    }
    #[test]
    fn dropping_slot_cancels_a_forgotten_access_request() {
        let mut call = Call::new(
            Endpoint::<Direct<u64>>::new(),
            async |mut slot: ReceiverSlot<'_, Direct<u64>>| {
                let mut read = slot.access(|v| *v.into_pin().get_mut().0);
                poll_fn(|cx| {
                    assert!(Pin::new(&mut read).poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                std::mem::forget(read);
                drop(slot);
                std::future::pending::<()>().await;
            },
        );
        assert!(
            call.poll_absent(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(call.signals(), vec![Signal::Idle]);
    }
}
