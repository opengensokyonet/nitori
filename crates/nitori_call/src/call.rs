//! A call: an operation together with the view its caller lends on each poll.
use crate::{
    ReceiverAccess, ReceiverFamily, ReceiverSlot,
    access::{RootWaker, ViewCell, mode::Probe, polled_directly},
    loan::RawViewLoan,
};
use std::{
    cell::{Cell, UnsafeCell},
    convert::Infallible,
    fmt,
    future::{Future, poll_fn},
    marker::{PhantomData, PhantomPinned},
    mem,
    ops::ControlFlow,
    pin::Pin,
    ptr,
    sync::{Arc, Mutex, PoisonError},
    task::{Context, Poll, Wake, Waker},
};

/// What an operation produces over family `F`, independent of the access it
/// runs with: neither type can carry the access or its brand.
pub trait Operation<F: ReceiverFamily> {
    /// The final value.
    type Output;
    /// Intermediate items; `Infallible` for operations that emit none.
    type Yield;
}

/// Starting an operation with an access.
///
/// A [`Call`] requires `for<'s> Start<'s, ReceiverSlot<'s, F>>`, so the
/// operation must accept every brand. A call is `Send` when the operation's
/// future would be `Send` with a `Send` stand-in for the slot, which requires
/// `Start` to be implemented generically over the access.
pub trait Start<'s, A: ReceiverAccess>: Operation<A::Family> {
    type Fut: Future<Output = <Self as Operation<A::Family>>::Output>;
    fn start(
        self,
        access: A,
        emitter: Emitter<'s, <Self as Operation<A::Family>>::Yield>,
    ) -> Self::Fut;
}

/// Where an operation's emitted item waits for its caller, and whether the
/// current poll discards items instead.
pub(crate) struct ItemCell<Y> {
    item: Cell<Option<Y>>,
    discard: Cell<bool>,
    /// The waker of the current poll; null outside one.
    root: RootWaker,
}
impl<Y> ItemCell<Y> {
    pub(crate) fn new() -> Self {
        Self {
            item: Cell::new(None),
            discard: Cell::new(false),
            root: RootWaker::new(),
        }
    }
    pub(crate) fn set_discard(&self, discard: bool) {
        self.discard.set(discard);
    }
}

/// Emits intermediate items from an operation. `#[call(yields = T)]` turns
/// each `yield` of the function into an emission.
pub struct Emitter<'s, Y> {
    cell: *const ItemCell<Y>,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
}
// SAFETY: the item cell is read only after the operation's poll returns, so
// an emitter moved to another thread during that poll never races with it.
// Items cross threads, hence `Y: Send`.
unsafe impl<Y: Send> Send for Emitter<'_, Y> {}
// SAFETY: shared references expose nothing.
unsafe impl<Y> Sync for Emitter<'_, Y> {}
impl<Y> fmt::Debug for Emitter<'_, Y> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Emitter").finish_non_exhaustive()
    }
}
impl<Y> Emitter<'_, Y> {
    pub(crate) fn new(cell: &ItemCell<Y>) -> Self {
        Self {
            cell,
            brand: PhantomData,
        }
    }
    /// Hand `item` to whoever polls the operation. Completes on the poll after
    /// the item has been delivered; dropping it after the first poll still
    /// delivers it. When the caller discards items, drops `item` and
    /// completes at once.
    pub fn emit(&mut self, item: Y) -> impl Future<Output = ()> + '_ {
        let mut item = Some(item);
        poll_fn(move |cx| {
            let Some(pending) = item.take() else {
                return Poll::Ready(());
            };
            // SAFETY: the owner of the cell outlives its operation.
            let cell = unsafe { &*self.cell };
            if cell.discard.get() {
                return Poll::Ready(());
            }
            match cell.item.take() {
                // An earlier emission in this poll is not yet delivered.
                Some(earlier) => {
                    cell.item.set(Some(earlier));
                    item = Some(pending);
                }
                None => cell.item.set(Some(pending)),
            }
            // Intermediate executors only repoll branches whose waker fired.
            // With none, the item itself makes the poll ready.
            if !polled_directly(cell.root.get(), cx) {
                cx.waker().wake_by_ref();
            }
            Poll::Pending
        })
    }
}
impl Emitter<'_, Infallible> {
    /// An emitter for operations that emit nothing; it has no cell.
    pub(crate) fn detached() -> Self {
        Self {
            cell: ptr::null(),
            brand: PhantomData,
        }
    }
}

/// The result of polling a call.
#[derive(Debug, PartialEq, Eq)]
pub enum CallPoll<T> {
    Ready(T),
    Pending,
    /// The call was polled without a view, and its operation asked for one
    /// in this poll or an earlier one that had none either. Nothing else
    /// wakes the call for this: poll it again with a view, or arrange a
    /// wakeup for when one is available.
    NeedsView,
}
impl<T> CallPoll<T> {
    /// Treat a request for a view as a bug: for callers that always lend one.
    pub(crate) fn lent(self) -> Poll<T> {
        match self {
            CallPoll::Ready(value) => Poll::Ready(value),
            CallPoll::Pending => Poll::Pending,
            CallPoll::NeedsView => unreachable!("a call given a view asked for one"),
        }
    }
}

/// Clears the lent view at the end of a poll, and poisons the call when the
/// poll panics.
struct LendGuard<'a, F: ReceiverFamily> {
    cell: &'a ViewCell<F>,
    poisoned: &'a Cell<bool>,
}
impl<F: ReceiverFamily> Drop for LendGuard<'_, F> {
    fn drop(&mut self) {
        self.cell.view.set(None);
        self.cell.root.clear();
        if std::thread::panicking() {
            self.poisoned.set(true);
        }
    }
}

/// Lend `view` to the root slots reading `cell` while `poll` runs.
fn lend<'view, F: ReceiverFamily + 'view, T>(
    cell: &ViewCell<F>,
    poisoned: &Cell<bool>,
    view: Option<Pin<&mut F::ReceiverView<'view>>>,
    waker: &Waker,
    poll: impl FnOnce() -> Poll<T>,
) -> CallPoll<T> {
    assert!(!poisoned.get(), "call polled after a panic");
    // The view and the waker outlive this function, whose guard removes them.
    cell.view.set(view.map(RawViewLoan::new));
    cell.root.set(waker);
    cell.requested.set(false);
    let guard = LendGuard { cell, poisoned };
    let poll = poll();
    drop(guard);
    match poll {
        Poll::Ready(value) => CallPoll::Ready(value),
        Poll::Pending if cell.requested.get() => CallPoll::NeedsView,
        // A branch that asked on an earlier poll still waits for a view.
        Poll::Pending
            if cell
                .intercept
                .get()
                .is_some_and(|intercept| intercept.waiting()) =>
        {
            CallPoll::NeedsView
        }
        Poll::Pending => CallPoll::Pending,
    }
}

/// The branches of intermediate executors that asked a call for a view, and
/// the waker its operation sees once one has.
///
/// A branch that asks is not woken then: the call keeps its waker and wakes
/// it when the call is lent a view, right before polling the operation. The
/// executor then wakes the waker it was polled with; for this call that is
/// the intercepting waker, which drops wakes caused by that replay, since
/// the operation is polled next, and passes other wakes to the caller.
pub(crate) struct Intercept {
    caller: Mutex<Option<Waker>>,
    branches: Mutex<Vec<Waker>>,
}

thread_local! {
    /// The interception state of the call whose branches this thread is
    /// waking before polling its operation; null otherwise.
    static REPLAYING: Cell<*const Intercept> = const { Cell::new(ptr::null()) };
}

impl Intercept {
    pub(crate) fn new() -> Self {
        Self {
            caller: Mutex::new(None),
            branches: Mutex::new(Vec::new()),
        }
    }
    /// Keep the waker of a branch that asked for a view. Every branch that
    /// asked is kept until the call is lent a view.
    pub(crate) fn defer(&self, branch: &Waker) {
        let mut branches = self.branches.lock().unwrap_or_else(PoisonError::into_inner);
        if !branches.iter().any(|kept| kept.will_wake(branch)) {
            branches.push(branch.clone());
        }
    }
    fn waiting(&self) -> bool {
        !self
            .branches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }
    /// Wake the branches that asked for a view. The wakes this causes
    /// synchronously at this call's intercepting waker are dropped; wakes
    /// reaching other calls are not.
    fn replay(self: &Arc<Self>) {
        let branches =
            mem::take(&mut *self.branches.lock().unwrap_or_else(PoisonError::into_inner));
        if branches.is_empty() {
            return;
        }
        struct Restore(*const Intercept);
        impl Drop for Restore {
            fn drop(&mut self) {
                REPLAYING.set(self.0);
            }
        }
        let _restore = Restore(REPLAYING.replace(Arc::as_ptr(self)));
        for branch in branches {
            branch.wake();
        }
    }
    fn set_caller(&self, waker: &Waker) {
        let mut caller = self.caller.lock().unwrap_or_else(PoisonError::into_inner);
        if !caller
            .as_ref()
            .is_some_and(|caller| caller.will_wake(waker))
        {
            *caller = Some(waker.clone());
        }
    }
    fn wake_caller(&self) {
        let caller = self.caller.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(caller) = &*caller {
            caller.wake_by_ref();
        }
    }
}
impl Wake for Intercept {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if !ptr::eq(REPLAYING.get(), Arc::as_ptr(self)) {
            self.wake_caller();
        }
    }
}

/// Storage for a value pinned in place that may hold references into itself,
/// such as a running future. Safe code can take a shared reference to a
/// pinned owner through `Pin`; without the cell that reference would assert
/// that the value is not mutated, invalidating those references. Replace with
/// `UnsafePinned` once it is stable.
pub(crate) struct InPlace<T>(UnsafeCell<T>);
impl<T> InPlace<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }
    pub(crate) fn get_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
    pub(crate) fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut T> {
        // SAFETY: the value is pinned whenever its storage is.
        unsafe { self.map_unchecked_mut(|this| this.0.get_mut()) }
    }
}

/// The size hint of an operation's events: at least the final value until it
/// has been delivered.
pub(crate) fn remaining(terminated: bool) -> (usize, Option<usize>) {
    match terminated {
        true => (0, Some(0)),
        false => (1, None),
    }
}

pub(crate) enum State<Op, Fut, R> {
    Start(Op),
    Running(Fut),
    /// Finished while an emitted item was still undelivered.
    Returned(R),
    Finished,
}

/// Advance an operation's state by one event: an emitted item comes before
/// the result.
///
/// # Safety
/// `state` must be pinned: a running future is never moved.
pub(crate) unsafe fn poll_events<Op, Fut: Future, Y>(
    state: &mut State<Op, Fut, Fut::Output>,
    emitted: &ItemCell<Y>,
    start: impl FnOnce(Op) -> Fut,
    cx: &mut Context<'_>,
) -> Poll<ControlFlow<Fut::Output, Y>> {
    match state {
        State::Start(_) => {
            let State::Start(operation) = mem::replace(state, State::Finished) else {
                unreachable!()
            };
            *state = State::Running(start(operation));
        }
        State::Running(_) => {}
        State::Returned(_) => {
            let State::Returned(result) = mem::replace(state, State::Finished) else {
                unreachable!()
            };
            return Poll::Ready(ControlFlow::Break(result));
        }
        State::Finished => panic!("operation polled after completion"),
    }
    let State::Running(future) = state else {
        unreachable!()
    };
    /// Clears the root waker when the poll ends.
    struct Root<'a>(&'a RootWaker);
    impl Drop for Root<'_> {
        fn drop(&mut self) {
            self.0.clear();
        }
    }
    emitted.root.set(cx.waker());
    let root = Root(&emitted.root);
    // SAFETY: the caller keeps the state pinned.
    let poll = unsafe { Pin::new_unchecked(future) }.poll(cx);
    drop(root);
    match poll {
        Poll::Ready(result) => {
            *state = State::Finished;
            match emitted.item.take() {
                Some(item) => {
                    *state = State::Returned(result);
                    Poll::Ready(ControlFlow::Continue(item))
                }
                None => Poll::Ready(ControlFlow::Break(result)),
            }
        }
        Poll::Pending => match emitted.item.take() {
            Some(item) => Poll::Ready(ControlFlow::Continue(item)),
            None => Poll::Pending,
        },
    }
}

type Running<F, Op> = <Op as Start<'static, ReceiverSlot<'static, F>>>::Fut;
type Output<F, Op> = <Op as Operation<F>>::Output;
type Yield<F, Op> = <Op as Operation<F>>::Yield;
type CallState<F, Op> = State<Op, Running<F, Op>, Output<F, Op>>;

/// An operation and its cells, polled with a view supplied by the caller on
/// each poll.
///
/// The view may differ between polls, so a call can be stored apart from the
/// resource it uses. A caller that acquires lazily polls without a view; the
/// call asks for one when its operation first uses it. A call has two forms:
/// [`poll_with`](Self::poll_with) delivers each emitted item and then the
/// final value, and [`poll_return_with`](Self::poll_return_with) discards the
/// items.
pub struct Call<F, Op>
where
    F: ReceiverFamily,
    Op: Operation<F> + for<'s> Start<'s, ReceiverSlot<'s, F>>,
{
    // Dropped before the cells its future points into.
    state: InPlace<CallState<F, Op>>,
    cell: ViewCell<F>,
    emitted: ItemCell<Yield<F, Op>>,
    poisoned: Cell<bool>,
    finished: bool,
    _pinned: PhantomPinned,
}
// SAFETY: the running future is the operation's future for the call's slot.
// The probe access is unnameable outside this crate, so an operation that
// starts with it is generic over the access, and its probe future differs only
// where the slot or accesses derived from it appear. Those never leave the
// polling thread: slots are not Send, and between polls they hold only a
// pointer into this call, whose cell then lends nothing.
unsafe impl<F, Op> Send for Call<F, Op>
where
    F: ReceiverFamily,
    Op: Operation<F> + Send,
    Op: for<'s> Start<'s, ReceiverSlot<'s, F>> + for<'s> Start<'s, ReceiverSlot<'s, F, Probe>>,
    for<'s> <Op as Start<'s, ReceiverSlot<'s, F, Probe>>>::Fut: Send,
    Output<F, Op>: Send,
    Yield<F, Op>: Send,
{
}
// SAFETY: a shared reference only reads whether the call has finished, which
// changes only through `Pin<&mut Self>`.
unsafe impl<F, Op> Sync for Call<F, Op>
where
    F: ReceiverFamily,
    Op: Operation<F> + for<'s> Start<'s, ReceiverSlot<'s, F>>,
{
}
impl<F, Op> fmt::Debug for Call<F, Op>
where
    F: ReceiverFamily,
    Op: Operation<F> + for<'s> Start<'s, ReceiverSlot<'s, F>>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call")
            .field("terminated", &self.finished)
            .finish_non_exhaustive()
    }
}

impl<F, Op> Call<F, Op>
where
    F: ReceiverFamily,
    Op: Operation<F> + for<'s> Start<'s, ReceiverSlot<'s, F>>,
{
    /// Prepare a call. The operation starts on the first poll.
    pub fn new(operation: Op) -> Self {
        Self {
            state: InPlace::new(State::Start(operation)),
            cell: ViewCell::new(),
            emitted: ItemCell::new(),
            poisoned: Cell::new(false),
            finished: false,
            _pinned: PhantomPinned,
        }
    }

    /// Whether the call has delivered its final value and must not be polled
    /// again.
    pub fn is_terminated(&self) -> bool {
        self.finished
    }

    /// Advance the operation with this poll's view, if the caller has one.
    /// `Continue` holds an emitted item and `Break` the final value, after
    /// which the call is done.
    pub fn poll_with<'view>(
        self: Pin<&mut Self>,
        view: Option<Pin<&mut F::ReceiverView<'view>>>,
        cx: &mut Context<'_>,
    ) -> CallPoll<ControlFlow<Output<F, Op>, Yield<F, Op>>>
    where
        F: 'view,
    {
        self.poll_event(view, false, cx)
    }

    fn poll_event<'view>(
        self: Pin<&mut Self>,
        view: Option<Pin<&mut F::ReceiverView<'view>>>,
        discard: bool,
        cx: &mut Context<'_>,
    ) -> CallPoll<ControlFlow<Output<F, Op>, Yield<F, Op>>>
    where
        F: 'view,
    {
        // SAFETY: nothing is moved out of the pinned call; the running future
        // is pinned in place until it is dropped in place. The future keeps
        // pointers into the cells, so they are only accessed through shared
        // references.
        let this = unsafe { self.get_unchecked_mut() };
        let (cell, emitted, poisoned, state) = (
            &this.cell,
            &this.emitted,
            &this.poisoned,
            this.state.get_mut(),
        );
        if let State::Finished = state {
            panic!("call polled after completion");
        }
        emitted.set_discard(discard);
        // Once a branch has asked for a view, the operation sees the
        // intercepting waker, and a view wakes the branches that asked. The
        // operation is polled with a context of its own either way: a single
        // path keeps the common case as fast as before, and the caller's
        // context carries nothing else on stable Rust.
        let intercepting = cell
            .intercept
            .get()
            .map(|intercept| intercepting_waker(intercept, cx.waker(), view.is_some()));
        let waker = intercepting.as_ref().unwrap_or(cx.waker());
        let poll = advance(
            state,
            cell,
            emitted,
            poisoned,
            view,
            &mut Context::from_waker(waker),
        );
        this.finished = matches!(poll, CallPoll::Ready(ControlFlow::Break(_)));
        poll
    }

    /// Advance to the final value with this poll's view. Emitted items are
    /// dropped where they are emitted, without suspending the operation.
    pub fn poll_return_with<'view>(
        self: Pin<&mut Self>,
        view: Option<Pin<&mut F::ReceiverView<'view>>>,
        cx: &mut Context<'_>,
    ) -> CallPoll<Output<F, Op>>
    where
        F: 'view,
    {
        match self.poll_event(view, true, cx) {
            CallPoll::Ready(ControlFlow::Break(result)) => CallPoll::Ready(result),
            CallPoll::Ready(ControlFlow::Continue(_)) => {
                unreachable!("discarded items are not stored")
            }
            CallPoll::Pending => CallPoll::Pending,
            CallPoll::NeedsView => CallPoll::NeedsView,
        }
    }
}

/// Advance a call's operation by one event with this poll's view.
fn advance<'view, F, Op>(
    state: &mut CallState<F, Op>,
    cell: &ViewCell<F>,
    emitted: &ItemCell<Yield<F, Op>>,
    poisoned: &Cell<bool>,
    view: Option<Pin<&mut F::ReceiverView<'view>>>,
    cx: &mut Context<'_>,
) -> CallPoll<ControlFlow<Output<F, Op>, Yield<F, Op>>>
where
    F: ReceiverFamily + 'view,
    Op: Operation<F> + for<'s> Start<'s, ReceiverSlot<'s, F>>,
{
    lend(cell, poisoned, view, cx.waker(), || {
        let start = |operation: Op| {
            // SAFETY: the call is pinned, so the cells keep their address, and
            // the future holding the slot and the emitter is dropped before
            // them. `Op: for<'s> Start` keeps the operation from relying on
            // the 'static brand chosen for storage.
            let slot = unsafe { ReceiverSlot::<'static, F>::new(cell) };
            operation.start(slot, Emitter::new(emitted))
        };
        // SAFETY: the state is a field of the pinned call.
        unsafe { poll_events(state, emitted, start, cx) }
    })
}

/// The intercepting waker of a call, after waking the branches that asked
/// for a view if one is lent.
#[cold]
fn intercepting_waker(intercept: &Arc<Intercept>, caller: &Waker, lent: bool) -> Waker {
    intercept.set_caller(caller);
    if lent {
        intercept.replay();
    }
    Waker::from(intercept.clone())
}

/// The dyn-compatible interface of [`Call`], for storing calls of different types.
pub trait DynCall<F: ReceiverFamily, Y, R> {
    fn poll_with<'view>(
        self: Pin<&mut Self>,
        view: Option<Pin<&mut F::ReceiverView<'view>>>,
        cx: &mut Context<'_>,
    ) -> CallPoll<ControlFlow<R, Y>>
    where
        F: 'view;
    fn poll_return_with<'view>(
        self: Pin<&mut Self>,
        view: Option<Pin<&mut F::ReceiverView<'view>>>,
        cx: &mut Context<'_>,
    ) -> CallPoll<R>
    where
        F: 'view;
}
impl<F, Op> DynCall<F, Yield<F, Op>, Output<F, Op>> for Call<F, Op>
where
    F: ReceiverFamily,
    Op: Operation<F> + for<'s> Start<'s, ReceiverSlot<'s, F>>,
{
    fn poll_with<'view>(
        self: Pin<&mut Self>,
        view: Option<Pin<&mut F::ReceiverView<'view>>>,
        cx: &mut Context<'_>,
    ) -> CallPoll<ControlFlow<Output<F, Op>, Yield<F, Op>>>
    where
        F: 'view,
    {
        Call::poll_with(self, view, cx)
    }
    fn poll_return_with<'view>(
        self: Pin<&mut Self>,
        view: Option<Pin<&mut F::ReceiverView<'view>>>,
        cx: &mut Context<'_>,
    ) -> CallPoll<Output<F, Op>>
    where
        F: 'view,
    {
        Call::poll_return_with(self, view, cx)
    }
}
