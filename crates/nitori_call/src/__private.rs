//! No shared slot, TLS, per-poll allocation, or mandatory scheduling budget.
//! Unsafe internals are intended only for the audited call macro expansion.
/// Projection support for generated calls; consumers need no direct dependency.
#[doc(hidden)]
pub use pin_project_lite::pin_project;

use crate::CallOn;
use std::{
    marker::PhantomData,
    ops::{Coroutine, CoroutineState},
    pin::Pin,
    ptr::NonNull,
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

/// Opaque, inert pointers. No safe API dereferences these pointers.
/// The macro consumes this value before every suspension.
#[doc(hidden)]
pub struct ResumeEnv<Host: ?Sized> {
    host: NonNull<Host>,
    context: NonNull<Context<'static>>,
}
// SAFETY: carrying inert pointer bits does not access their pointees. Every
// dereferencing method is unsafe and requires same-thread, current-resume use.
// A safe generated call never exposes this value or sends it while active.
unsafe impl<Host: ?Sized> Send for ResumeEnv<Host> {}
// SAFETY: shared access cannot dereference anything through a safe method.
// Unsafe methods require exclusive &mut access and the current resume scope.
unsafe impl<Host: ?Sized> Sync for ResumeEnv<Host> {}
impl<Host: ?Sized> ResumeEnv<Host> {
    /// Consume the current environment before suspending; does not dereference it.
    pub fn end(self) {}
    unsafe fn enter<Output>(
        &mut self,
        body: impl for<'visit, 'waker> FnOnce(
            Pin<&'visit mut Host>,
            &'visit mut Context<'waker>,
        ) -> Output,
    ) -> Output {
        // SAFETY: callers uphold the current-resume/exclusivity contract below.
        // The HRTB prevents the fresh host/context borrows from escaping.
        unsafe {
            body(
                Pin::new_unchecked(self.host.as_mut()),
                &mut *self.context.as_ptr().cast::<Context<'_>>(),
            )
        }
    }
    pub fn prepare_await<A: crate::IntoAwaitOn<Host>>(&self, value: A) -> A::Awaitable {
        value.into_await_on()
    }
    /// # Safety
    /// Must run synchronously on the thread of the resume that supplied self,
    /// before that resume ends. No overlapping use of its host/context is
    /// allowed. The environment must not be accessed from captured user code.
    pub unsafe fn with<Output>(
        &mut self,
        body: impl for<'visit> FnOnce(Pin<&'visit mut Host>) -> Output,
    ) -> Output {
        // SAFETY: propagated from this method's caller; references remain local.
        unsafe { self.enter(|host, _| body(host)) }
    }
    /// # Safety
    /// Same current-resume and exclusive-access requirements as with.
    pub unsafe fn poll_await<A: crate::AwaitOn<Host> + ?Sized>(
        &mut self,
        value: Pin<&mut A>,
    ) -> Poll<A::Output> {
        // SAFETY: the caller supplies the current exclusive resume access.
        unsafe { self.enter(|host, cx| value.poll_on(host, cx)) }
    }
}
/// Supply the callback's expected type without borrowing a resume environment.
#[doc(hidden)]
pub fn prepare<Host: ?Sized, Output, Body>(body: Body) -> Body
where
    Body: for<'visit> FnOnce(Pin<&'visit mut Host>) -> Output,
{
    body
}
pin_project_lite::pin_project! {
    /// Only the compiler-generated persistent state and completion flag are retained.
    pub struct StackCall<Host: ?Sized, State> {
        #[pin]
        state: State,
        terminal: bool,
        marker: PhantomData<fn(*mut Host) -> *mut Host>,
    }
}
/// Build an inline pinned coroutine adapter, with no allocation.
///
/// # Safety
/// The coroutine may dereference a ResumeEnv only during the same resume that
/// supplied it and on that thread. It must consume the environment before each
/// suspension, must not expose it or use it during Drop, and must not manufacture
/// overlapping host/context references. User expressions must not inherit an
/// unsafe context from generated helper calls. The call macros enforce these rules
/// by hiding the environment and rejecting opaque suspension-generating syntax.
#[doc(hidden)]
pub unsafe fn build<Host: ?Sized, State, Item>(state: State) -> StackCall<Host, State>
where
    State: Coroutine<ResumeEnv<Host>, Yield = Suspend<Item>>,
{
    // SAFETY: identical resume-environment contract, supplied by our caller.
    unsafe { build_mapped(state) }
}
/// Build a named operation whose suspension has a nominal type boundary.
///
/// # Safety
/// The caller must uphold exactly the same resume-environment contract as build.
pub unsafe fn build_mapped<Host: ?Sized, State>(state: State) -> StackCall<Host, State>
where
    State: Coroutine<ResumeEnv<Host>>,
    State::Yield: Suspension,
{
    StackCall {
        state,
        terminal: false,
        marker: PhantomData,
    }
}
impl<Host: ?Sized, State, Item> CallOn<Host> for StackCall<Host, State>
where
    State: Coroutine<ResumeEnv<Host>>,
    State::Yield: Suspension<Item = Item>,
{
    type Yield = Item;
    type Return = State::Return;
    fn poll_call(
        self: Pin<&mut Self>,
        host: Pin<&mut Host>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Item, State::Return>> {
        let this = self.project();
        assert!(!*this.terminal, "call polled after completion or panic");
        *this.terminal = true;
        let environment = ResumeEnv {
            // SAFETY: only extract the pointer, preserving the host's pinning.
            host: NonNull::from(unsafe { host.get_unchecked_mut() }),
            context: NonNull::from(cx).cast(),
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
