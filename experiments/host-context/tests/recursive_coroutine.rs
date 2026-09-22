#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]
#![deny(unsafe_op_in_unsafe_fn)]
//! Capability erasure at coroutine boundaries, independent of lazy projection.
use std::{
    cell::Cell,
    future::Future,
    marker::{PhantomData, PhantomPinned},
    ops::{Coroutine, CoroutineState},
    pin::{Pin, pin},
    sync::{Mutex, MutexGuard},
    task::{Context, Poll, Waker},
};

trait Family {}
struct Root<'data>(PhantomData<&'data str>);
impl Family for Root<'_> {}
struct Counted<F: Family>(PhantomData<fn() -> F>);
impl<F: Family> Family for Counted<F> {}
trait View {
    type Family: Family;
}
trait Read: View {
    fn read(self: Pin<&mut Self>) -> u8;
}
trait ReadAt<V: View + ?Sized>: Family {
    fn read(view: Pin<&mut V>) -> u8;
}
impl<V: View + ?Sized> Read for V
where
    V::Family: ReadAt<V>,
{
    fn read(self: Pin<&mut Self>) -> u8 {
        <V::Family as ReadAt<V>>::read(self)
    }
}
struct RootView<'h, 'data> {
    _label: &'data str,
    guard: MutexGuard<'h, Cell<u8>>,
    invariant: PhantomData<fn(&'h ()) -> &'h ()>,
    _pinned: PhantomPinned,
}
impl<'data> View for RootView<'_, 'data> {
    type Family = Root<'data>;
}
impl<'data> ReadAt<RootView<'_, 'data>> for Root<'data> {
    fn read(view: Pin<&mut RootView<'_, 'data>>) -> u8 {
        let cell = &view.as_ref().get_ref().guard;
        let result = cell.get();
        cell.set(result + 1);
        result
    }
}
struct Layer<'a, V: Read + ?Sized> {
    inner: Pin<&'a mut V>,
    count: &'a mut usize,
}
impl<V: Read + ?Sized> View for Layer<'_, V> {
    type Family = Counted<V::Family>;
}
impl<V: Read + ?Sized> ReadAt<Layer<'_, V>> for Counted<V::Family> {
    fn read(view: Pin<&mut Layer<'_, V>>) -> u8 {
        let this = view.get_mut();
        *this.count += 1;
        this.inner.as_mut().read()
    }
}
trait HostContext {
    type View: Read + ?Sized;
    fn poll_view(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Pin<&mut Self::View>>;
}
struct ReadyContext<'a, V: Read + ?Sized> {
    view: Pin<&'a mut V>,
    polls: &'a Cell<usize>,
}
impl<V: Read + ?Sized> HostContext for ReadyContext<'_, V> {
    type View = V;
    fn poll_view(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Pin<&mut V>> {
        let this = self.get_mut();
        this.polls.set(this.polls.get() + 1);
        Poll::Ready(this.view.as_mut())
    }
}
trait CallOnView<V: Read + ?Sized> {
    fn poll_call(
        &mut self,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>;
}
struct ReadOne;
impl<V: Read + ?Sized> CallOnView<V> for ReadOne {
    fn poll_call(
        &mut self,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8> {
        Poll::Ready(std::task::ready!(context.poll_view(cx)).read())
    }
}
// This receiver deliberately acquires the parent before polling its child.
// The test isolates coroutine dispatch; lazy projected caching is a separate
// mechanism and is not established by this receiver.
struct Decorate<O> {
    operation: O,
    count: usize,
}
impl<O> Decorate<O> {
    fn new(operation: O) -> Self {
        Self {
            operation,
            count: 0,
        }
    }
}
impl<V: Read + ?Sized, O> CallOnView<V> for Decorate<O>
where
    for<'a> O: CallOnView<Layer<'a, V>>,
{
    fn poll_call(
        &mut self,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8> {
        let parent = std::task::ready!(context.poll_view(cx));
        let mut view = pin!(Layer {
            inner: parent,
            count: &mut self.count
        });
        let polls = Cell::new(0);
        let mut context = pin!(ReadyContext {
            view: view.as_mut(),
            polls: &polls
        });
        self.operation.poll_call(context.as_mut(), cx)
    }
}

// Only this adapter erases the concrete, recursively nested View type.
struct EraseContext<'a, V: Read> {
    context: Pin<&'a mut dyn HostContext<View = V>>,
}
impl<'a, V: Read + 'a> HostContext for EraseContext<'a, V> {
    type View = dyn Read<Family = V::Family> + 'a;
    fn poll_view(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Pin<&mut Self::View>> {
        self.get_mut()
            .context
            .as_mut()
            .poll_view(cx)
            .map(|view| view as Pin<&mut Self::View>)
    }
}
trait AwaitOn<F: Family> {
    fn poll_on<'v>(
        &mut self,
        context: Pin<&mut dyn HostContext<View = dyn Read<Family = F> + 'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>
    where
        F: 'v;
}
struct Child<O>(O);
impl<F: Family, O> AwaitOn<F> for Child<O>
where
    for<'v> O: CallOnView<dyn Read<Family = F> + 'v>,
{
    fn poll_on<'v>(
        &mut self,
        context: Pin<&mut dyn HostContext<View = dyn Read<Family = F> + 'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>
    where
        F: 'v,
    {
        self.0.poll_call(context, cx)
    }
}
struct External<Fut>(Pin<Box<Fut>>);
impl<F: Family, Fut: Future<Output = u8>> AwaitOn<F> for External<Fut> {
    fn poll_on<'v>(
        &mut self,
        _: Pin<&mut dyn HostContext<View = dyn Read<Family = F> + 'v>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8>
    where
        F: 'v,
    {
        self.0.as_mut().poll(cx)
    }
}
trait Request<F: Family> {
    fn execute<'v>(
        &mut self,
        context: Pin<&mut dyn HostContext<View = dyn Read<Family = F> + 'v>>,
        cx: &mut Context<'_>,
    ) where
        F: 'v;
}
struct PollRequest<'a, F: Family, A: AwaitOn<F>> {
    value: &'a mut A,
    result: Option<Poll<u8>>,
    marker: PhantomData<fn() -> F>,
}
impl<F: Family, A: AwaitOn<F>> Request<F> for PollRequest<'_, F, A> {
    fn execute<'v>(
        &mut self,
        context: Pin<&mut dyn HostContext<View = dyn Read<Family = F> + 'v>>,
        cx: &mut Context<'_>,
    ) where
        F: 'v,
    {
        self.result = Some(self.value.poll_on(context, cx));
    }
}
struct ResumeEnv<F: Family> {
    slot: *mut (),
    dispatch: unsafe fn(*mut (), &mut dyn Request<F>),
}
impl<F: Family> ResumeEnv<F> {
    fn end(self) {}
    /// Poll through the context belonging to the currently executing resume.
    ///
    /// # Safety
    /// The token must be the one supplied to the current resume on this thread.
    /// Its supplying resume must not have yielded, completed, or unwound. Access
    /// to its context and task-context slot must be exclusive, and no token or
    /// pointer may be retained for a later resume or used from a destructor.
    unsafe fn poll_await<A: AwaitOn<F>>(&mut self, value: &mut A) -> Poll<u8> {
        let mut request = PollRequest::<F, A> {
            value,
            result: None,
            marker: PhantomData,
        };
        // SAFETY: the caller provides current-resume exclusive access. The
        // request is stack-local and stores only the owned u8 poll result.
        unsafe { (self.dispatch)(self.slot, &mut request) };
        request.result.unwrap()
    }
}
struct Slot<'a, 'v, 'w, F: Family + 'v> {
    context: Pin<&'a mut dyn HostContext<View = dyn Read<Family = F> + 'v>>,
    cx: &'a mut Context<'w>,
}
struct Dispatcher<'v, F: Family + 'v>(PhantomData<&'v F>);
impl<'v, F: Family + 'v> Dispatcher<'v, F> {
    // SAFETY: this function may only be installed with a pointer to a live
    // Slot having this exact view lifetime. The caller must satisfy poll_await's
    // exclusive-access and current-resume requirements.
    unsafe fn dispatch(slot: *mut (), request: &mut dyn Request<F>) {
        // SAFETY: poll_call pairs this dispatcher with this exact Slot type;
        // the resume completes before the stack-local Slot is destroyed.
        let slot = unsafe { &mut *slot.cast::<Slot<'_, 'v, '_, F>>() };
        request.execute(slot.context.as_mut(), slot.cx);
    }
}
// Heap pinning keeps this test independent of a structural pin-projection API.
struct StackCall<F: Family, S> {
    state: Pin<Box<S>>,
    terminal: bool,
    marker: PhantomData<fn() -> F>,
}
impl<F: Family, S> StackCall<F, S>
where
    S: Coroutine<ResumeEnv<F>, Yield = (), Return = u8>,
{
    /// Construct a coroutine that follows the resume token protocol.
    ///
    /// # Safety
    /// Every token must be accessed only during its supplying resume, on the
    /// same thread, with exclusive access. Consume it with end before each yield
    /// and completion. No token or pointer may escape, remain in coroutine state
    /// across suspension, or be accessed during destruction or unwinding.
    unsafe fn new(state: S) -> Self {
        Self {
            state: Box::pin(state),
            terminal: false,
            marker: PhantomData,
        }
    }
}
impl<F: Family, V: Read<Family = F>, S> CallOnView<V> for StackCall<F, S>
where
    S: Coroutine<ResumeEnv<F>, Yield = (), Return = u8>,
{
    fn poll_call(
        &mut self,
        context: Pin<&mut dyn HostContext<View = V>>,
        cx: &mut Context<'_>,
    ) -> Poll<u8> {
        assert!(!self.terminal, "call polled after completion or panic");
        self.terminal = true;
        let mut erased = pin!(EraseContext { context });
        let mut slot = Slot::<F> {
            context: erased.as_mut(),
            cx,
        };
        let env = ResumeEnv {
            slot: (&mut slot as *mut Slot<'_, '_, '_, F>).cast(),
            dispatch: Dispatcher::<'_, F>::dispatch,
        };
        match self.state.as_mut().resume(env) {
            CoroutineState::Yielded(()) => {
                self.terminal = false;
                Poll::Pending
            }
            CoroutineState::Complete(value) => Poll::Ready(value),
        }
    }
}
struct OncePending(bool);
impl Future for OncePending {
    type Output = u8;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u8> {
        let this = self.get_mut();
        if this.0 {
            Poll::Ready(5)
        } else {
            this.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

fn make_read_coroutine<F: Family>()
-> StackCall<F, impl Coroutine<ResumeEnv<F>, Yield = (), Return = u8>> {
    let state = #[coroutine]
    |mut env: ResumeEnv<F>| {
        let mut child = Child(ReadOne);
        // SAFETY: the fresh token is used only during this resume and consumed
        // before either yielding or completing.
        let value = loop {
            match unsafe { env.poll_await(&mut child) } {
                Poll::Pending => {
                    env.end();
                    env = yield ();
                }
                Poll::Ready(value) => break value,
            }
        };
        env.end();
        value
    };
    // SAFETY: the coroutine uses only its current token and consumes it before
    // every yield and completion; nothing retains the token or dispatch pointer.
    unsafe { StackCall::new(state) }
}

#[test]
fn recursive_children_and_nested_coroutines_keep_family_dispatch() {
    let label = String::from("borrowed family and closure capture");
    let mutex = Mutex::new(Cell::new(7));
    let polls = Cell::new(0);
    // The root already owns its guard: this test measures view access, not
    // source acquisition. In particular, it does not model pending acquisition.
    let mut root = pin!(RootView {
        _label: &label,
        guard: mutex.lock().unwrap(),
        invariant: PhantomData,
        _pinned: PhantomPinned
    });
    let mut context = pin!(ReadyContext {
        view: root.as_mut(),
        polls: &polls
    });
    let mut captures = 0;
    let captured = &mut captures;
    let state = #[coroutine]
    move |mut env: ResumeEnv<Root<'_>>| {
        *captured += 1;
        let mut input = External(Box::pin(OncePending(false)));
        // SAFETY: the token belongs to this resume; each suspension consumes it
        // and replaces it with the fresh token supplied by the next resume.
        let external = loop {
            match unsafe { env.poll_await(&mut input) } {
                Poll::Pending => {
                    env.end();
                    env = yield ();
                }
                Poll::Ready(value) => break value,
            }
        };
        let mut child = Child(Decorate::new(Decorate::new(make_read_coroutine::<
            Counted<Counted<Root<'_>>>,
        >())));
        // SAFETY: the fresh token is used only during this resume and consumed
        // before either yielding or completing.
        let value = loop {
            match unsafe { env.poll_await(&mut child) } {
                Poll::Pending => {
                    env.end();
                    env = yield ();
                }
                Poll::Ready(value) => break value,
            }
        };
        assert_eq!(child.0.count, 1);
        assert_eq!(child.0.operation.count, 1);
        *captured += 1;
        env.end();
        value + external
    };
    // SAFETY: all branches consume the token before yielding or completing;
    // neither nested child calls nor the external future retain the token.
    let mut operation = unsafe { StackCall::new(state) };
    let mut cx = Context::from_waker(Waker::noop());
    let erased: &mut dyn CallOnView<RootView<'_, '_>> = &mut operation;
    assert_eq!(erased.poll_call(context.as_mut(), &mut cx), Poll::Pending);
    assert_eq!(
        polls.get(),
        0,
        "ordinary async Pending must not request a view"
    );
    assert_eq!(
        operation.poll_call(context.as_mut(), &mut cx),
        Poll::Ready(12)
    );
    assert_eq!(polls.get(), 1);
    drop(operation);
    assert_eq!(captures, 2);
}
