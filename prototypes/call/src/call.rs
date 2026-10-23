//! A call: an operation together with the scope its caller lends on each poll.
use crate::{
    ReceiverAccess, ReceiverFamily, ReceiverScope, ReceiverSlot,
    access::{Dispatch, ScopeRequest, mode::Probe},
};
use std::{
    cell::Cell,
    convert::Infallible,
    future::{Future, poll_fn},
    marker::{PhantomData, PhantomPinned},
    mem,
    ops::ControlFlow,
    pin::Pin,
    ptr,
    task::{Context, Poll, ready},
};

/// What an operation produces over family `F`, independent of the access it
/// runs with: neither type can carry the access or its brand.
pub trait Operation<F: ReceiverFamily> {
    type Output;
    /// The type of emitted items; `Infallible` for operations that emit none.
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

/// Emits intermediate items from an operation as events of its call.
pub struct Emitter<'s, Y> {
    cell: *const Cell<Option<Y>>,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
}
// SAFETY: the item cell is read by the call only after the operation's poll
// returns, so an emitter moved to another thread during that poll never races
// with it. Items cross threads, hence `Y: Send`.
unsafe impl<Y: Send> Send for Emitter<'_, Y> {}
// SAFETY: shared references expose nothing.
unsafe impl<Y> Sync for Emitter<'_, Y> {}
impl<Y> Emitter<'_, Y> {
    /// Hand `item` to the call's poller. Completes on the poll after the item
    /// has been delivered; dropping it after the first poll still delivers it.
    pub fn emit(&mut self, item: Y) -> impl Future<Output = ()> + '_ {
        let mut item = Some(item);
        poll_fn(move |cx| {
            let Some(pending) = item.take() else {
                return Poll::Ready(());
            };
            // SAFETY: the call owns the cell and outlives its operation.
            let cell = unsafe { &*self.cell };
            match cell.take() {
                // An earlier emission in this poll is not yet delivered.
                Some(earlier) => {
                    cell.set(Some(earlier));
                    item = Some(pending);
                }
                None => cell.set(Some(pending)),
            }
            // Intermediate executors only repoll branches whose waker fired.
            cx.waker().wake_by_ref();
            Poll::Pending
        })
    }
}
impl Emitter<'_, Infallible> {
    /// An emitter for operations started outside a call; it can emit nothing.
    pub(crate) fn detached() -> Self {
        Self {
            cell: ptr::null(),
            brand: PhantomData,
        }
    }
}

struct Lend<F> {
    target: *mut (),
    dispatch: Dispatch<F>,
}
impl<F> Clone for Lend<F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F> Copy for Lend<F> {}

/// The scope argument of one poll, addressed by its lend.
struct Lent<'a, 'view, F: ReceiverFamily + 'view> {
    scope: Pin<&'a mut dyn ReceiverScope<'view, Family = F>>,
}
impl<'view, F: ReceiverFamily + 'view> Lent<'_, 'view, F> {
    unsafe fn dispatch(target: *mut (), cx: &mut Context<'_>, request: &mut dyn ScopeRequest<F>) {
        // SAFETY: poll_with pairs this function with a live Lent of exactly
        // these lifetimes and removes the pair before the Lent is dropped.
        let lent = unsafe { &mut *target.cast::<Self>() };
        request.run(lent.scope.as_mut(), cx);
    }
}
unsafe fn dispatch_root<F: ReceiverFamily>(
    target: *mut (),
    cx: &mut Context<'_>,
    request: &mut dyn ScopeRequest<F>,
) {
    // SAFETY: the slot points at the lend cell of its pinned call, which
    // outlives the operation; slots stay on the thread polling the call.
    let cell = unsafe { &*target.cast::<Cell<Option<Lend<F>>>>() };
    let lend = cell
        .take()
        .expect("receiver slot used outside its call's poll, or reentered");
    // SAFETY: poll_with installed a matching pair for this poll.
    unsafe { (lend.dispatch)(lend.target, cx, request) };
    cell.set(Some(lend));
}

enum State<Op, Fut, R> {
    Start(Op),
    Running(Fut),
    Returned(R),
    Finished,
}

type Running<F, Op> = <Op as Start<'static, ReceiverSlot<'static, F>>>::Fut;
type Output<F, Op> = <Op as Operation<F>>::Output;
type Yield<F, Op> = <Op as Operation<F>>::Yield;

/// An operation and its receiver cells, polled with a scope supplied by the
/// caller on each poll.
///
/// The scope may differ between polls, so a call can be stored apart from the
/// resource it uses. Each poll's view is acquired lazily by the scope and
/// released when the caller drops it.
pub struct Call<F, Op>
where
    F: ReceiverFamily,
    Op: Operation<F> + for<'s> Start<'s, ReceiverSlot<'s, F>>,
{
    // Dropped before the cells it points into.
    state: State<Op, Running<F, Op>, Output<F, Op>>,
    lend: Cell<Option<Lend<F>>>,
    emitted: Cell<Option<Yield<F, Op>>>,
    poisoned: Cell<bool>,
    _pinned: PhantomPinned,
}
// SAFETY: the running future is the operation's future for the call's slot.
// The probe access is unnameable outside this crate, so an operation that
// starts with it is generic over the access, and its probe future differs only
// where the slot or accesses derived from it appear. Those never leave the
// polling thread: slots are not Send, and between polls they hold only
// pointers into this call, whose lend cell is then empty.
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

struct PollGuard<'a, F> {
    lend: &'a Cell<Option<Lend<F>>>,
    poisoned: &'a Cell<bool>,
}
impl<F> Drop for PollGuard<'_, F> {
    fn drop(&mut self) {
        self.lend.set(None);
        if std::thread::panicking() {
            self.poisoned.set(true);
        }
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
            state: State::Start(operation),
            lend: Cell::new(None),
            emitted: Cell::new(None),
            poisoned: Cell::new(false),
            _pinned: PhantomPinned,
        }
    }

    /// Advance the operation with this poll's scope. Returns `Continue` for an
    /// emitted item and `Break` for the result, after which the call is done.
    pub fn poll_with<'view>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<Output<F, Op>, Yield<F, Op>>>
    where
        F: 'view,
    {
        // SAFETY: nothing is moved out of the pinned call; the running future
        // is pinned in place until it is dropped in place.
        let this: *mut Self = unsafe { self.get_unchecked_mut() };
        // The running future keeps pointers into these cells, so they are
        // only ever accessed through shared references.
        // SAFETY: `this` is valid; the fields are disjoint.
        let (lend, emitted, poisoned, state) = unsafe {
            (
                &(*this).lend,
                &(*this).emitted,
                &(*this).poisoned,
                &mut (*this).state,
            )
        };
        assert!(!poisoned.get(), "call polled after a panic");
        match state {
            State::Returned(_) => {
                let State::Returned(result) = mem::replace(state, State::Finished) else {
                    unreachable!()
                };
                return Poll::Ready(ControlFlow::Break(result));
            }
            State::Finished => panic!("call polled after completion"),
            State::Start(_) | State::Running(_) => {}
        }

        let mut lent = Lent { scope };
        lend.set(Some(Lend {
            target: ptr::from_mut(&mut lent).cast(),
            dispatch: Lent::<'_, 'view, F>::dispatch,
        }));
        let guard = PollGuard { lend, poisoned };
        if let State::Start(_) = state {
            let State::Start(operation) = mem::replace(state, State::Finished) else {
                unreachable!()
            };
            // SAFETY: the call is pinned, so the cells keep their address, and
            // the future that holds these handles is dropped before them.
            // `Op: for<'s> Start` makes the operation unable to rely on the
            // 'static brand chosen for storage.
            let slot = unsafe {
                ReceiverSlot::<'static, F>::from_raw(
                    ptr::from_ref(lend).cast_mut().cast(),
                    dispatch_root::<F>,
                )
            };
            let emitter = Emitter {
                cell: emitted,
                brand: PhantomData,
            };
            *state = State::Running(operation.start(slot, emitter));
        }
        let State::Running(future) = state else {
            unreachable!()
        };
        // SAFETY: the future stays in place until it is dropped in place.
        let poll = unsafe { Pin::new_unchecked(future) }.poll(cx);
        drop(guard);

        match poll {
            Poll::Ready(result) => {
                *state = State::Finished;
                match emitted.take() {
                    Some(item) => {
                        *state = State::Returned(result);
                        Poll::Ready(ControlFlow::Continue(item))
                    }
                    None => Poll::Ready(ControlFlow::Break(result)),
                }
            }
            Poll::Pending => match emitted.take() {
                Some(item) => Poll::Ready(ControlFlow::Continue(item)),
                None => Poll::Pending,
            },
        }
    }

    /// Advance to the result within this poll's scope, discarding emitted items.
    pub fn poll_return_with<'view>(
        mut self: Pin<&mut Self>,
        mut scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Output<F, Op>>
    where
        F: 'view,
    {
        loop {
            match ready!(self.as_mut().poll_with(scope.as_mut(), cx)) {
                ControlFlow::Continue(_) => {}
                ControlFlow::Break(result) => return Poll::Ready(result),
            }
        }
    }
}

/// The dyn-compatible interface of [`Call`], for storing calls of different types.
pub trait DynCall<F: ReceiverFamily, Y, R> {
    fn poll_with<'view>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<R, Y>>
    where
        F: 'view;
    fn poll_return_with<'view>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<R>
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
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<Output<F, Op>, Yield<F, Op>>>
    where
        F: 'view,
    {
        Call::poll_with(self, scope, cx)
    }
    fn poll_return_with<'view>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Output<F, Op>>
    where
        F: 'view,
    {
        Call::poll_return_with(self, scope, cx)
    }
}
