//! Stack-local context dispatch for coroutine calls.
//!
//! The resume token carries no usable safe reference. Each poll supplies a fresh
//! token whose unsafe access is valid only until that particular resume returns.
//! No context, view, or task-context borrow enters persistent coroutine state.

use crate::{AwaitOn, CallOn, HostContext, HostFamily};
use std::{
    marker::PhantomData,
    ops::{Coroutine, CoroutineState},
    pin::Pin,
    task::{Context, Poll},
};

/// A task suspension or a visible call event.
pub enum Suspend<Item> {
    Pending,
    Emit(Item),
}

/// Permit a named suspension type without exposing its representation.
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

/// Whether this drive exposes events or discards them until completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveMode {
    Event,
    Return,
}

/// An inert token for the currently executing resume.
pub struct ResumeEnv<F: HostFamily> {
    mode: DriveMode,
    slot: *mut (),
    dispatch: unsafe fn(*mut (), &mut dyn Request<F>),
}

// SAFETY: no safe method accesses either pointer. Unsafe access must occur only
// during the supplying resume, on its thread, before any suspension or return.
unsafe impl<F: HostFamily> Send for ResumeEnv<F> {}
// SAFETY: unsafe dispatch additionally requires exclusive access to the slot.
unsafe impl<F: HostFamily> Sync for ResumeEnv<F> {}

trait Request<F: HostFamily> {
    fn execute<'view>(
        &mut self,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'view;
}

struct PollRequest<'a, F: HostFamily, A: AwaitOn<F> + ?Sized> {
    value: Pin<&'a mut A>,
    result: Option<Poll<A::Output>>,
}

impl<F: HostFamily, A: AwaitOn<F> + ?Sized> Request<F> for PollRequest<'_, F, A> {
    fn execute<'view>(
        &mut self,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) where
        F: 'view,
    {
        self.result = Some(self.value.as_mut().poll_on(context, cx));
    }
}

struct Slot<'access, 'view, 'waker, F: HostFamily + 'view> {
    context: Pin<&'access mut dyn HostContext<'view, Family = F>>,
    cx: &'access mut Context<'waker>,
}

struct Dispatcher<'view, F: HostFamily + 'view>(PhantomData<&'view F>);

impl<'view, F: HostFamily + 'view> Dispatcher<'view, F> {
    unsafe fn dispatch(slot: *mut (), request: &mut dyn Request<F>) {
        // SAFETY: poll_call creates this exact slot with this view lifetime.
        // build's contract confines dispatch to the exclusive supplying resume.
        let slot = unsafe { &mut *slot.cast::<Slot<'_, 'view, '_, F>>() };
        request.execute(slot.context.as_mut(), slot.cx);
    }
}

impl<F: HostFamily> ResumeEnv<F> {
    /// This mode belongs to this resume; it can change after a suspension.
    pub fn mode(&self) -> DriveMode {
        self.mode
    }

    /// Consume the current token before yielding or completing the coroutine.
    pub fn end(self) {}

    /// Poll one awaitable without eagerly asking the context for a view.
    ///
    /// # Safety
    /// This token must have been supplied to the currently executing resume, on
    /// this thread. Its supplying resume must not have returned, yielded, or
    /// panicked. No other access to its context/task-context slot may overlap.
    /// Neither this token nor a copy of its pointers may escape to another task,
    /// a destructor, or a later resume. Consume it with `end` before suspension.
    pub unsafe fn poll_await<A: AwaitOn<F> + ?Sized>(
        &mut self,
        value: Pin<&mut A>,
    ) -> Poll<A::Output> {
        let mut request = PollRequest::<F, A> {
            value,
            result: None,
        };
        // SAFETY: the caller guarantees exclusive current-resume access.
        unsafe { (self.dispatch)(self.slot, &mut request) };
        request.result.expect("resume dispatch completed")
    }
}

pin_project_lite::pin_project! {
    /// Persistent coroutine state plus a completion/panic guard.
    pub struct StackCall<F: HostFamily, State> {
        #[pin]
        state: State,
        terminal: bool,
        marker: PhantomData<fn(*mut F) -> *mut F>,
    }
}

/// Build a pinned coroutine adapter without allocating.
///
/// # Safety
/// The coroutine must use each supplied `ResumeEnv` only during that resume, on
/// the same thread, with exclusive access. It must consume the token before
/// every yield and completion; no token may be exposed, retained across a
/// suspension, or dereferenced during destruction. Any helper-generated unsafe
/// scope must not extend to unrelated user expressions. These obligations apply
/// to every branch, including nested awaits and panic paths.
pub unsafe fn build<F: HostFamily, State, Item>(state: State) -> StackCall<F, State>
where
    State: Coroutine<ResumeEnv<F>, Yield = Suspend<Item>>,
{
    // SAFETY: build_mapped has the same resume-token obligations.
    unsafe { build_mapped(state) }
}

/// Build a call with a custom suspension wrapper.
///
/// # Safety
/// The coroutine must obey all the resume-token obligations documented by
/// [`build`].
pub unsafe fn build_mapped<F: HostFamily, State>(state: State) -> StackCall<F, State>
where
    State: Coroutine<ResumeEnv<F>>,
    State::Yield: Suspension,
{
    StackCall {
        state,
        terminal: false,
        marker: PhantomData,
    }
}

impl<F: HostFamily, State> CallOn<F> for StackCall<F, State>
where
    State: Coroutine<ResumeEnv<F>>,
    State::Yield: Suspension,
{
    type Yield = <State::Yield as Suspension>::Item;
    type Return = State::Return;

    fn poll_call<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        F: 'view,
    {
        self.resume_once(context, cx, DriveMode::Event)
    }

    fn poll_return<'view>(
        mut self: Pin<&mut Self>,
        mut context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Return>
    where
        F: 'view,
    {
        loop {
            match self
                .as_mut()
                .resume_once(context.as_mut(), cx, DriveMode::Return)
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(CoroutineState::Complete(value)) => return Poll::Ready(value),
                Poll::Ready(CoroutineState::Yielded(value)) => drop(value),
            }
        }
    }
}

impl<F: HostFamily, State> StackCall<F, State>
where
    State: Coroutine<ResumeEnv<F>>,
    State::Yield: Suspension,
{
    fn resume_once<'view>(
        self: Pin<&mut Self>,
        context: Pin<&mut dyn HostContext<'view, Family = F>>,
        cx: &mut Context<'_>,
        mode: DriveMode,
    ) -> Poll<CoroutineState<<State::Yield as Suspension>::Item, State::Return>>
    where
        F: 'view,
    {
        let this = self.project();
        assert!(!*this.terminal, "call polled after completion or panic");
        *this.terminal = true;
        let mut slot = Slot::<F> { context, cx };
        let environment = ResumeEnv {
            mode,
            slot: (&mut slot as *mut Slot<'_, 'view, '_, F>).cast(),
            dispatch: Dispatcher::<'view, F>::dispatch,
        };

        // SAFETY of all dispatches is provided by build's caller. The slot stays
        // on this stack until resume returns; no view is acquired preemptively.
        match this.state.resume(environment) {
            CoroutineState::Yielded(value) => {
                let suspended = value.into_suspend();
                *this.terminal = false;
                match suspended {
                    Suspend::Pending => Poll::Pending,
                    Suspend::Emit(item) => Poll::Ready(CoroutineState::Yielded(item)),
                }
            }
            CoroutineState::Complete(value) => Poll::Ready(CoroutineState::Complete(value)),
        }
    }
}
