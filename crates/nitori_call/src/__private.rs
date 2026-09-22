//! No shared slot, TLS, per-poll allocation, or mandatory scheduling budget.
//! Unsafe internals are intended only for the audited call macro expansion.
/// Projection support for generated calls; consumers need no direct dependency.
#[doc(hidden)]
pub use pin_project_lite::pin_project;

use crate::{CallOn, ReceiverFamily};
use std::{
    marker::PhantomData,
    ops::{Coroutine, CoroutineState},
    pin::Pin,
    task::{Context, Poll},
};

pub enum Suspend<Item> {
    Pending,
    Emit(Item),
}

/// Normalize a nominal suspension wrapper without exposing its item in TAIT bounds.
pub trait Suspension {
    type Item;
    fn into_suspend(self) -> Suspend<Self::Item>;
}
impl<Item> Suspension for Suspend<Item> {
    type Item = Item;
    fn into_suspend(self) -> Self {
        self
    }
}

/// An inert current-resume dispatch token. Never exposed by the macros.
pub struct ResumeContext<F: ReceiverFamily> {
    slot: *mut (),
    dispatch: unsafe fn(*mut (), &mut dyn Request<F>),
}
// SAFETY: pointers are inert; only unsafe current-resume methods access them.
unsafe impl<F: ReceiverFamily> Send for ResumeContext<F> {}
// SAFETY: no safe method accesses the pointee; dispatch requires exclusivity.
unsafe impl<F: ReceiverFamily> Sync for ResumeContext<F> {}
trait Request<F: ReceiverFamily> {
    fn execute<'host>(
        &mut self,
        host: Pin<&mut dyn crate::ReceiverScope<'host, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'host;
}
struct PollRequest<'a, F: ReceiverFamily, A: crate::AwaitOn<F> + ?Sized> {
    value: Pin<&'a mut A>,
    result: Option<Poll<A::Output>>,
}
impl<F: ReceiverFamily, A: crate::AwaitOn<F> + ?Sized> Request<F> for PollRequest<'_, F, A> {
    fn execute<'host>(
        &mut self,
        host: Pin<&mut dyn crate::ReceiverScope<'host, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'host,
    {
        self.result = Some(self.value.as_mut().poll_on(host, cx));
    }
}
struct Slot<'access, 'host, 'waker, F: ReceiverFamily + 'host> {
    host: Pin<&'access mut dyn crate::ReceiverScope<'host, Family = F>>,
    cx: &'access mut Context<'waker>,
}
struct Dispatcher<'host, F: ReceiverFamily + 'host>(PhantomData<&'host F>);
impl<'host, F: ReceiverFamily + 'host> Dispatcher<'host, F> {
    unsafe fn dispatch(slot: *mut (), request: &mut dyn Request<F>) {
        // SAFETY: created for this exact host lifetime by poll_call, used only
        // during its synchronous resume. Neither reference escapes execute.
        let slot = unsafe { &mut *slot.cast::<Slot<'_, 'host, '_, F>>() };
        request.execute(slot.host.as_mut(), slot.cx);
    }
}
impl<F: ReceiverFamily> ResumeContext<F> {
    pub fn end(self) {}
    pub fn target(&self) -> crate::Target<F> {
        crate::Target::new()
    }
    pub fn prepare_await<A: crate::IntoAwaitOn<F>>(&self, value: A) -> A::Awaitable {
        value.into_await_on()
    }
    /// # Safety
    /// Use only on the supplying resume's thread, before it returns, with no
    /// overlapping access. Never retain or expose the token across suspension.
    pub unsafe fn poll_await<A: crate::AwaitOn<F> + ?Sized>(
        &mut self,
        value: Pin<&mut A>,
    ) -> Poll<A::Output> {
        let mut request = PollRequest::<F, A> {
            value,
            result: None,
        };
        // SAFETY: caller guarantees current-resume exclusive access.
        unsafe { (self.dispatch)(self.slot, &mut request) };
        request.result.expect("resume dispatch completed")
    }
}
pin_project_lite::pin_project! {
    /// Only the compiler-generated persistent state and completion flag are retained.
    pub struct StackCall<Receiver: ReceiverFamily, State> {
        #[pin]
        state: State,
        terminal: bool,
        marker: PhantomData<fn(*mut Receiver) -> *mut Receiver>,
    }
}
/// Build an inline pinned coroutine adapter, with no allocation.
///
/// # Safety
/// The coroutine may dereference a ResumeContext only during the same resume that
/// supplied it and on that thread. It must consume the environment before each
/// suspension, must not expose it or use it during Drop, and must not manufacture
/// overlapping host/context references. User expressions must not inherit an
/// unsafe context from generated helper calls. The call macros enforce these rules
/// by hiding the environment and rejecting opaque suspension-generating syntax.
#[doc(hidden)]
pub unsafe fn build<Receiver: ReceiverFamily, State, Item>(
    state: State,
) -> StackCall<Receiver, State>
where
    State: Coroutine<ResumeContext<Receiver>, Yield = Suspend<Item>>,
{
    // SAFETY: identical resume-environment contract, supplied by our caller.
    unsafe { build_mapped(state) }
}
/// Build a named operation whose suspension has a nominal type boundary.
///
/// # Safety
/// The caller must uphold exactly the same resume-environment contract as build.
pub unsafe fn build_mapped<Receiver: ReceiverFamily, State>(
    state: State,
) -> StackCall<Receiver, State>
where
    State: Coroutine<ResumeContext<Receiver>>,
    State::Yield: Suspension,
{
    StackCall {
        state,
        terminal: false,
        marker: PhantomData,
    }
}
impl<Receiver: ReceiverFamily, State, Item> CallOn<Receiver> for StackCall<Receiver, State>
where
    State: Coroutine<ResumeContext<Receiver>>,
    State::Yield: Suspension<Item = Item>,
{
    type Yield = Item;
    type Return = State::Return;
    fn poll_call<'host>(
        self: Pin<&mut Self>,
        host: Pin<&mut dyn crate::ReceiverScope<'host, Family = Receiver>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Item, State::Return>>
    where
        Receiver: 'host,
    {
        let this = self.project();
        assert!(!*this.terminal, "call polled after completion or panic");
        *this.terminal = true;
        let mut slot = Slot::<Receiver> { host, cx };
        let environment = ResumeContext {
            slot: (&mut slot as *mut Slot<'_, 'host, '_, Receiver>).cast(),
            dispatch: Dispatcher::<'host, Receiver>::dispatch,
        };
        // build's contract ensures environment access ends before resume returns.
        let state = this.state.resume(environment);
        match state {
            CoroutineState::Yielded(value) => {
                *this.terminal = false;
                match value.into_suspend() {
                    Suspend::Pending => Poll::Pending,
                    Suspend::Emit(item) => Poll::Ready(CoroutineState::Yielded(item)),
                }
            }
            CoroutineState::Complete(value) => Poll::Ready(CoroutineState::Complete(value)),
        }
    }
}

pin_project_lite::pin_project! {
    /// Map suspension and completion without moving the pinned coroutine.
    pub struct MapCoroutine<State, YieldMap, ReturnMap> {
        #[pin]
        state: State,
        yield_map: YieldMap,
        return_map: ReturnMap,
    }
}
pub fn map_coroutine<State, YieldMap, ReturnMap>(
    state: State,
    yield_map: YieldMap,
    return_map: ReturnMap,
) -> MapCoroutine<State, YieldMap, ReturnMap> {
    MapCoroutine {
        state,
        yield_map,
        return_map,
    }
}
impl<
    Resume,
    State: Coroutine<Resume>,
    YieldMap: FnMut(State::Yield) -> Item,
    ReturnMap: FnMut(State::Return) -> Output,
    Item,
    Output,
> Coroutine<Resume> for MapCoroutine<State, YieldMap, ReturnMap>
{
    type Yield = Item;
    type Return = Output;
    fn resume(self: Pin<&mut Self>, argument: Resume) -> CoroutineState<Item, Output> {
        let this = self.project();
        match this.state.resume(argument) {
            CoroutineState::Yielded(value) => CoroutineState::Yielded((this.yield_map)(value)),
            CoroutineState::Complete(value) => CoroutineState::Complete((this.return_map)(value)),
        }
    }
}
