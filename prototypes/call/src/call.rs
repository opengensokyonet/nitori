//! A call: an operation together with the scope its caller lends on each poll.
use crate::{
    __private::{ScopeRequest, ThreadAnchor},
    ReceiverFamily, ReceiverScope, ReceiverSlot,
    access::Dispatch,
};
use std::{
    cell::Cell,
    future::{Future, poll_fn},
    marker::{PhantomData, PhantomPinned},
    mem,
    ops::ControlFlow,
    pin::Pin,
    ptr,
    task::{Context, Poll, ready},
};

/// The work of a call apart from its receiver: given the call's slot and
/// emitter, start the future that performs it.
///
/// Requiring `for<'s> Operation<'s, F, Y, R>` fixes `R` outside the brand, so
/// the result cannot carry the slot. Implemented for functions taking the slot
/// and emitter, and for a function paired with the arguments it takes last.
pub trait Operation<'s, F: ReceiverFamily, Y, R> {
    type Fut: Future<Output = R>;
    fn start(self, slot: ReceiverSlot<'s, F>, emitter: Emitter<'s, Y>) -> Self::Fut;
}
impl<'s, F: ReceiverFamily, Y, R, Op, Fut> Operation<'s, F, Y, R> for Op
where
    Op: FnOnce(ReceiverSlot<'s, F>, Emitter<'s, Y>) -> Fut,
    Fut: Future<Output = R>,
{
    type Fut = Fut;
    fn start(self, slot: ReceiverSlot<'s, F>, emitter: Emitter<'s, Y>) -> Fut {
        self(slot, emitter)
    }
}
impl<'s, F: ReceiverFamily, Y, R, Function, Arguments, Fut> Operation<'s, F, Y, R>
    for (Function, Arguments)
where
    Function: FnOnce(ReceiverSlot<'s, F>, Emitter<'s, Y>, Arguments) -> Fut,
    Fut: Future<Output = R>,
{
    type Fut = Fut;
    fn start(self, slot: ReceiverSlot<'s, F>, emitter: Emitter<'s, Y>) -> Fut {
        (self.0)(slot, emitter, self.1)
    }
}

/// Emits intermediate items from an operation as events of its call.
pub struct Emitter<'s, Y> {
    cell: *const Cell<Option<Y>>,
    anchor: *const ThreadAnchor,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
}
// SAFETY: the item cell is touched only after checking that the current thread
// is polling the call; other threads only read the atomic anchor.
unsafe impl<Y> Send for Emitter<'_, Y> {}
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
            // SAFETY: the anchor lives in the call, which outlives its operation.
            let anchor = unsafe { &*self.anchor };
            assert!(
                anchor.is_current(),
                "emitter used outside the poll of its call"
            );
            // SAFETY: the current thread is polling the call, which owns the cell.
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
    // SAFETY: the slot checked its anchor, so the call is inside poll_with on
    // this thread and its lend cell is alive.
    let cell = unsafe { &*target.cast::<Cell<Option<Lend<F>>>>() };
    let lend = cell.take().expect("receiver slot reentered its own scope");
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

/// An operation and its receiver cells, polled with a scope supplied by the
/// caller on each poll.
///
/// The scope may differ between polls, so a call can be stored apart from the
/// resource it uses. Each poll's view is acquired lazily by the scope and
/// released when the caller drops it. `Y` is the type of emitted items.
pub struct Call<F: ReceiverFamily, Y, R, Op: for<'s> Operation<'s, F, Y, R>> {
    // Dropped before the cells it points into.
    state: State<Op, <Op as Operation<'static, F, Y, R>>::Fut, R>,
    lend: Cell<Option<Lend<F>>>,
    emitted: Cell<Option<Y>>,
    anchor: ThreadAnchor,
    poisoned: Cell<bool>,
    _pinned: PhantomPinned,
}
// SAFETY: between polls the lend cell is empty and the anchor is zero. During a
// poll the call is borrowed exclusively by the polling thread.
unsafe impl<F, Y, R, Op> Send for Call<F, Y, R, Op>
where
    F: ReceiverFamily,
    Y: Send,
    R: Send,
    Op: Send + for<'s> Operation<'s, F, Y, R>,
    <Op as Operation<'static, F, Y, R>>::Fut: Send,
{
}

struct PollGuard<'a, F> {
    lend: &'a Cell<Option<Lend<F>>>,
    anchor: &'a ThreadAnchor,
    poisoned: &'a Cell<bool>,
}
impl<F> Drop for PollGuard<'_, F> {
    fn drop(&mut self) {
        self.lend.set(None);
        self.anchor.leave();
        if std::thread::panicking() {
            self.poisoned.set(true);
        }
    }
}

impl<F: ReceiverFamily, Y, R, Op: for<'s> Operation<'s, F, Y, R>> Call<F, Y, R, Op> {
    /// Prepare a call. The operation starts on the first poll.
    pub fn new(operation: Op) -> Self {
        Self {
            state: State::Start(operation),
            lend: Cell::new(None),
            emitted: Cell::new(None),
            anchor: ThreadAnchor::new(),
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
    ) -> Poll<ControlFlow<R, Y>>
    where
        F: 'view,
    {
        // SAFETY: nothing is moved out of the pinned call; the running future
        // is pinned in place until it is dropped in place.
        let this: *mut Self = unsafe { self.get_unchecked_mut() };
        // The running future keeps pointers into these cells, so they are
        // only ever accessed through shared references.
        // SAFETY: `this` is valid; the fields are disjoint.
        let (lend, emitted, anchor, poisoned, state) = unsafe {
            (
                &(*this).lend,
                &(*this).emitted,
                &(*this).anchor,
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
        anchor.enter();
        let guard = PollGuard {
            lend,
            anchor,
            poisoned,
        };
        if let State::Start(_) = state {
            let State::Start(operation) = mem::replace(state, State::Finished) else {
                unreachable!()
            };
            // SAFETY: the call is pinned, so the cells keep their address, and
            // the future that holds these handles is dropped before them.
            // `Op: for<'s> Operation` makes the operation unable to rely on the
            // 'static brand chosen for storage.
            let slot = unsafe {
                ReceiverSlot::<'static, F>::from_raw(
                    ptr::from_ref(lend).cast_mut().cast(),
                    dispatch_root::<F>,
                    anchor,
                )
            };
            let emitter = Emitter {
                cell: emitted,
                anchor,
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
    ) -> Poll<R>
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
impl<F: ReceiverFamily, Y, R, Op: for<'s> Operation<'s, F, Y, R>> DynCall<F, Y, R>
    for Call<F, Y, R, Op>
{
    fn poll_with<'view>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<ControlFlow<R, Y>>
    where
        F: 'view,
    {
        Call::poll_with(self, scope, cx)
    }
    fn poll_return_with<'view>(
        self: Pin<&mut Self>,
        scope: Pin<&mut dyn ReceiverScope<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<R>
    where
        F: 'view,
    {
        Call::poll_return_with(self, scope, cx)
    }
}
