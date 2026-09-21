#![feature(coroutines, coroutine_trait)]
#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    marker::PhantomData,
    ops::{Coroutine, CoroutineState},
    pin::Pin,
    task::{Context, Poll},
};

pub trait Family {
    type Host<'host>
    where
        Self: 'host;
}

pub struct Fixed<H>(PhantomData<fn() -> H>);

impl<H> Family for Fixed<H> {
    type Host<'host>
        = H
    where
        Self: 'host;
}

pub trait Call<F: Family> {
    type Yield;
    type Return;

    fn poll<'host>(
        self: Pin<&mut Self>,
        host: Pin<&mut F::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        F: 'host;
}

// Lifetime-generic methods remain object safe. The erased dispatch invokes this
// method at the original host's inner lifetime, without rebinding that lifetime.
trait Request<F: Family> {
    fn execute<'host>(&mut self, host: Pin<&mut F::Host<'host>>, cx: &mut Context<'_>)
    where
        F: 'host;
}

struct PollRequest<'operation, F: Family, O: Call<F>> {
    operation: Pin<&'operation mut O>,
    result: Option<Poll<CoroutineState<O::Yield, O::Return>>>,
    marker: PhantomData<fn() -> F>,
}

impl<F: Family, O: Call<F>> Request<F> for PollRequest<'_, F, O> {
    fn execute<'host>(&mut self, host: Pin<&mut F::Host<'host>>, cx: &mut Context<'_>)
    where
        F: 'host,
    {
        self.result = Some(self.operation.as_mut().poll(host, cx));
    }
}

/// A fixed resume type carrying a scoped dispatcher, not a re-lifetimed host.
pub struct Resume<F: Family> {
    slot: *mut (),
    dispatch: unsafe fn(*mut (), &mut dyn Request<F>),
}

struct Slot<'access, 'host, 'waker, F: Family + 'host> {
    host: Pin<&'access mut F::Host<'host>>,
    cx: &'access mut Context<'waker>,
}

// The explicit 'host is selected when the slot is created. It is not replaced
// with 'static or shortened through an arbitrary, possibly invariant GAT.
struct Dispatcher<'host, F: Family + 'host>(PhantomData<&'host F>);

impl<'host, F: Family + 'host> Dispatcher<'host, F> {
    unsafe fn dispatch(slot: *mut (), request: &mut dyn Request<F>) {
        // SAFETY: the caller supplies the matching, live Slot exclusively during
        // its original resume. Only the outer access borrow is shortened here.
        let slot = unsafe { &mut *slot.cast::<Slot<'_, 'host, '_, F>>() };
        request.execute(slot.host.as_mut(), slot.cx);
    }
}

impl<F: Family> Resume<F> {
    /// # Safety
    /// Call only during the original resume, on its thread, without overlapping
    /// dispatches. Never use this value after suspension or from Drop.
    pub unsafe fn poll<O: Call<F>>(
        &mut self,
        operation: Pin<&mut O>,
    ) -> Poll<CoroutineState<O::Yield, O::Return>> {
        let mut request = PollRequest {
            operation,
            result: None,
            marker: PhantomData,
        };
        // SAFETY: inherited from this method's current-resume contract.
        unsafe { (self.dispatch)(self.slot, &mut request) };
        request.result.expect("dispatcher did not execute request")
    }

    pub fn end(self) {}
}

pub enum Suspend<T> {
    Pending,
    Yield(T),
}

pub struct Stack<F: Family, C> {
    coroutine: C,
    terminal: bool,
    marker: PhantomData<fn() -> F>,
}

/// # Safety
/// The coroutine must discard each Resume before yielding. It may only call
/// Resume::poll according to that method's contract, and may not expose Resume.
pub unsafe fn stack<F: Family, C, Y>(coroutine: C) -> Stack<F, C>
where
    C: Coroutine<Resume<F>, Yield = Suspend<Y>>,
{
    Stack {
        coroutine,
        terminal: false,
        marker: PhantomData,
    }
}

impl<F: Family, C, Y> Call<F> for Stack<F, C>
where
    C: Coroutine<Resume<F>, Yield = Suspend<Y>>,
{
    type Yield = Y;
    type Return = C::Return;

    fn poll<'host>(
        self: Pin<&mut Self>,
        host: Pin<&mut F::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Y, C::Return>>
    where
        F: 'host,
    {
        // SAFETY: coroutine is structurally pinned, never moved or exposed
        // unpinned. The remaining fields are not structurally pinned.
        let this = unsafe { self.get_unchecked_mut() };
        assert!(!this.terminal, "call resumed after completion or panic");
        this.terminal = true;
        let mut slot = Slot { host, cx };
        let resume = Resume {
            slot: (&mut slot as *mut Slot<'_, 'host, '_, F>).cast(),
            dispatch: Dispatcher::<'host, F>::dispatch,
        };
        // SAFETY: coroutine remains pinned and the live slot outlasts resume.
        let event = unsafe { Pin::new_unchecked(&mut this.coroutine) }.resume(resume);
        match event {
            CoroutineState::Yielded(suspend) => {
                this.terminal = false;
                match suspend {
                    Suspend::Pending => Poll::Pending,
                    Suspend::Yield(value) => Poll::Ready(CoroutineState::Yielded(value)),
                }
            }
            CoroutineState::Complete(value) => Poll::Ready(CoroutineState::Complete(value)),
        }
    }
}

pub trait Read {
    type Error;
    fn read(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u8, Self::Error>>;
}

pub trait ReadFamily: Family {
    type Error;
    fn read<'host>(
        host: Pin<&mut Self::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<u8, Self::Error>>
    where
        Self: 'host;
}

impl<H: Read> ReadFamily for Fixed<H> {
    type Error = H::Error;
    fn read<'host>(host: Pin<&mut H>, cx: &mut Context<'_>) -> Poll<Result<u8, Self::Error>>
    where
        Self: 'host,
    {
        host.read(cx)
    }
}

pub mod lending;

pub struct ReadOne;
impl<F: ReadFamily> Call<F> for ReadOne {
    type Yield = std::convert::Infallible;
    type Return = Result<u8, F::Error>;
    fn poll<'host>(
        self: Pin<&mut Self>,
        host: Pin<&mut F::Host<'host>>,
        cx: &mut Context<'_>,
    ) -> Poll<CoroutineState<Self::Yield, Self::Return>>
    where
        F: 'host,
    {
        F::read(host, cx).map(CoroutineState::Complete)
    }
}

/// A persistent compiler-generated coroutine containing pinned child calls.
pub fn decode<F: ReadFamily>() -> impl Call<F, Yield = u8, Return = Result<[u8; 2], F::Error>> {
    // SAFETY: each suspension consumes the old environment, each host request
    // is synchronous, and no environment escapes into output or Drop.
    unsafe {
        stack(
            #[coroutine]
            static |mut env: Resume<F>| {
                let mut result = [0; 2];
                for byte in &mut result {
                    let mut child = std::pin::pin!(ReadOne);
                    *byte = loop {
                        match env.poll(child.as_mut()) {
                            Poll::Pending => {
                                env.end();
                                env = yield Suspend::Pending;
                            }
                            Poll::Ready(CoroutineState::Complete(value)) => break value?,
                            Poll::Ready(CoroutineState::Yielded(never)) => match never {},
                        }
                    };
                    env.end();
                    env = yield Suspend::Yield(*byte);
                }
                env.end();
                Ok(result)
            },
        )
    }
}
