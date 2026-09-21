#![feature(coroutines, coroutine_trait)]

use host_family::{Call, Family, Fixed, Read, ReadFamily, Resume, Suspend, decode, stack};
use std::{
    cell::Cell,
    marker::{PhantomData, PhantomPinned},
    ops::CoroutineState::{Complete, Yielded},
    pin::{Pin, pin},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Input<'data> {
    data: &'data [u8],
    error: &'data str,
    offset: Cell<usize>,
    pending: Cell<bool>,
    address: Cell<*const ()>,
    drops: Rc<Cell<usize>>,
    _pin: PhantomPinned,
}

impl<'data> Input<'data> {
    fn new(data: &'data [u8], error: &'data str, drops: Rc<Cell<usize>>) -> Self {
        Self {
            data,
            error,
            offset: Cell::new(0),
            pending: Cell::new(true),
            address: Cell::new(std::ptr::null()),
            drops,
            _pin: PhantomPinned,
        }
    }
}
impl<'data> Read for Input<'data> {
    type Error = &'data str;
    fn read(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u8, Self::Error>> {
        let this = self.as_ref().get_ref();
        let address = std::ptr::from_ref(this).cast::<()>();
        let previous = this.address.replace(address);
        assert!(previous.is_null() || previous == address);
        if this.pending.replace(false) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        match this.data.get(this.offset.get()) {
            Some(value) => {
                this.offset.set(this.offset.get() + 1);
                this.pending.set(true);
                Poll::Ready(Ok(*value))
            }
            None => Poll::Ready(Err(this.error)),
        }
    }
}
impl Drop for Input<'_> {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

struct State {
    calls: Cell<usize>,
    _pin: PhantomPinned,
}
impl State {
    fn new() -> Self {
        Self {
            calls: Cell::new(0),
            _pin: PhantomPinned,
        }
    }
}

// Author-defined wrapper deliberately invariant in the temporary lifetime.
struct Meter<'visit, T> {
    inner: T,
    state: Pin<&'visit mut State>,
    invariant: PhantomData<fn(&'visit ()) -> &'visit ()>,
}
impl<T: Read> Read for Meter<'_, T> {
    type Error = T::Error;
    fn read(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u8, Self::Error>> {
        // SAFETY: inner is structurally pinned and never moved. Meter has no
        // custom Drop; state is a pinned pointer, never an unpinned State.
        let this = unsafe { self.get_unchecked_mut() };
        this.state.calls.set(this.state.calls.get() + 1);
        unsafe { Pin::new_unchecked(&mut this.inner) }.read(cx)
    }
}
impl<H: Read + ?Sized> Read for HostRef<'_, H> {
    type Error = H::Error;
    fn read(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u8, Self::Error>> {
        self.get_mut().0.as_mut().read(cx)
    }
}
struct HostRef<'visit, H: ?Sized>(Pin<&'visit mut H>);

struct Metered<H: ?Sized>(PhantomData<fn() -> H>);
impl<H: ?Sized> Family for Metered<H> {
    type Host<'visit>
        = Meter<'visit, HostRef<'visit, H>>
    where
        Self: 'visit;
}
impl<H: Read + ?Sized> ReadFamily for Metered<H> {
    type Error = H::Error;
    fn read<'visit>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<u8, Self::Error>>
    where
        Self: 'visit,
    {
        host.read(cx)
    }
}

struct DoubleMetered<H: ?Sized>(PhantomData<fn() -> H>);
impl<H: ?Sized> Family for DoubleMetered<H> {
    type Host<'visit>
        = Meter<'visit, Meter<'visit, HostRef<'visit, H>>>
    where
        Self: 'visit;
}
impl<H: Read + ?Sized> ReadFamily for DoubleMetered<H> {
    type Error = H::Error;
    fn read<'visit>(
        host: Pin<&mut Self::Host<'visit>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<u8, Self::Error>>
    where
        Self: 'visit,
    {
        host.read(cx)
    }
}

fn meter<'visit, T>(inner: T, state: Pin<&'visit mut State>) -> Meter<'visit, T> {
    Meter {
        inner,
        state,
        invariant: PhantomData,
    }
}

#[test]
fn fixed_family_retains_borrowed_error_and_pinned_non_send_host() {
    let bytes = vec![7];
    let message = String::from("end of input");
    let drops = Rc::new(Cell::new(0));
    {
        let mut input = pin!(Input::new(&bytes, &message, drops.clone()));
        let mut call = pin!(decode::<Fixed<Input<'_>>>());
        let wakes = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);
        assert_eq!(call.as_mut().poll(input.as_mut(), &mut cx), Poll::Pending);
        assert_eq!(
            call.as_mut().poll(input.as_mut(), &mut cx),
            Poll::Ready(Yielded(7))
        );
        assert_eq!(call.as_mut().poll(input.as_mut(), &mut cx), Poll::Pending);
        assert_eq!(
            call.as_mut().poll(input.as_mut(), &mut cx),
            Poll::Ready(Complete(Err(message.as_str())))
        );
        assert_eq!(wakes.0.load(Ordering::SeqCst), 2);
    }
    assert_eq!(drops.get(), 1);
}

#[test]
fn invariant_host_rebuilt_for_each_resume_of_same_coroutine() {
    let bytes = vec![3, 8];
    let message = String::from("eof");
    let mut input = pin!(Input::new(&bytes, &message, Rc::new(Cell::new(0))));
    let mut state = pin!(State::new());
    let mut call = pin!(decode::<Metered<Input<'_>>>());
    let mut cx = Context::from_waker(Waker::noop());
    let expected = [
        Poll::Pending,
        Poll::Ready(Yielded(3)),
        Poll::Pending,
        Poll::Ready(Yielded(8)),
        Poll::Ready(Complete(Ok([3, 8]))),
    ];
    for event in expected {
        {
            let mut host = pin!(meter(HostRef(input.as_mut()), state.as_mut()));
            assert_eq!(call.as_mut().poll(host.as_mut(), &mut cx), event);
        }
        // The view is gone, so state is directly accessible between resumes.
        state.calls.set(state.calls.get() + 10);
    }
    assert_eq!(state.calls.get(), 54);
}

#[test]
fn prebuilt_nested_views_preserve_both_local_states() {
    let bytes = vec![4, 9];
    let message = String::from("eof");
    let mut input = pin!(Input::new(&bytes, &message, Rc::new(Cell::new(0))));
    let mut inner = pin!(State::new());
    let mut outer = pin!(State::new());
    let mut call = pin!(decode::<DoubleMetered<Input<'_>>>());
    let mut cx = Context::from_waker(Waker::noop());
    let expected = [
        Poll::Pending,
        Poll::Ready(Yielded(4)),
        Poll::Pending,
        Poll::Ready(Yielded(9)),
        Poll::Ready(Complete(Ok([4, 9]))),
    ];
    for event in expected {
        let host = meter(HostRef(input.as_mut()), inner.as_mut());
        let mut host = pin!(meter(host, outer.as_mut()));
        assert_eq!(call.as_mut().poll(host.as_mut(), &mut cx), event);
    }
    assert_eq!(inner.calls.get(), 4);
    assert_eq!(outer.calls.get(), 4);
}

// A parent coroutine owns its wrapping state, reconstructing the view only
// while dispatching a request on its fixed underlying host.
struct Adapt<'state, H: Read + ?Sized, O> {
    state: Pin<&'state mut State>,
    child: O,
    marker: PhantomData<fn() -> H>,
}
impl<H: Read, O: Call<Metered<H>>> Call<Fixed<H>> for Adapt<'_, H, O> {
    type Yield = O::Yield;
    type Return = O::Return;
    fn poll<'host>(
        self: Pin<&mut Self>,
        host: Pin<&mut H>,
        cx: &mut Context<'_>,
    ) -> Poll<std::ops::CoroutineState<O::Yield, O::Return>>
    where
        Fixed<H>: 'host,
    {
        // SAFETY: child is structurally pinned and never moved.
        let this = unsafe { self.get_unchecked_mut() };
        let mut view = pin!(meter(HostRef(host), this.state.as_mut()));
        unsafe { Pin::new_unchecked(&mut this.child) }.poll(view.as_mut(), cx)
    }
}

fn parent<H: Read>(
    drops: Rc<Cell<usize>>,
) -> impl Call<Fixed<H>, Yield = u8, Return = Result<[u8; 2], H::Error>> {
    struct Guard(Rc<Cell<usize>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    // SAFETY: resume access is synchronous and every suspension ends its scope.
    unsafe {
        stack(
            #[coroutine]
            static move |mut env: Resume<Fixed<H>>| {
                let _guard = Guard(drops);
                let mut state = pin!(State::new());
                let mut adapted = pin!(Adapt::<H, _> {
                    state: state.as_mut(),
                    child: decode::<Metered<H>>(),
                    marker: PhantomData,
                });
                loop {
                    match env.poll(adapted.as_mut()) {
                        Poll::Pending => {
                            env.end();
                            env = yield Suspend::Pending;
                        }
                        Poll::Ready(Yielded(value)) => {
                            env.end();
                            env = yield Suspend::Yield(value);
                        }
                        Poll::Ready(Complete(value)) => {
                            env.end();
                            break value;
                        }
                    }
                }
            },
        )
    }
}

#[test]
fn parent_coroutine_wraps_host_for_child_and_cancellation_drops_only_owned_state() {
    let bytes = vec![1, 2];
    let message = String::from("eof");
    let host_drops = Rc::new(Cell::new(0));
    let state_drops = Rc::new(Cell::new(0));
    let mut input = pin!(Input::new(&bytes, &message, host_drops.clone()));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut call = pin!(parent::<Input<'_>>(state_drops.clone()));
        assert_eq!(call.as_mut().poll(input.as_mut(), &mut cx), Poll::Pending);
        assert_eq!(
            call.as_mut().poll(input.as_mut(), &mut cx),
            Poll::Ready(Yielded(1))
        );
    }
    assert_eq!(state_drops.get(), 1);
    assert_eq!(host_drops.get(), 0);
    assert_eq!(input.offset.get(), 1);
    {
        let mut call = pin!(parent::<Input<'_>>(state_drops.clone()));
        assert_eq!(call.as_mut().poll(input.as_mut(), &mut cx), Poll::Pending);
        assert_eq!(
            call.as_mut().poll(input.as_mut(), &mut cx),
            Poll::Ready(Yielded(2))
        );
        assert_eq!(call.as_mut().poll(input.as_mut(), &mut cx), Poll::Pending);
        assert_eq!(
            call.as_mut().poll(input.as_mut(), &mut cx),
            Poll::Ready(Complete(Err(message.as_str())))
        );
    }
    assert_eq!(state_drops.get(), 2);
}

#[test]
fn panic_poisoning_prevents_reusing_resume_slot() {
    let mut host = 0_u8;
    let mut call = pin!(unsafe {
        stack::<Fixed<u8>, _, ()>(
            #[coroutine]
            static |env: Resume<Fixed<u8>>| {
                env.end();
                if true {
                    panic!("intentional");
                }
                yield Suspend::Pending;
            },
        )
    });
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call.as_mut().poll(Pin::new(&mut host), &mut cx)
        }))
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call.as_mut().poll(Pin::new(&mut host), &mut cx)
        }))
        .is_err()
    );
}

#[test]
fn unsized_host_metadata_survives_erased_slot_dispatch() {
    let bytes = vec![6, 2];
    let message = String::from("eof");
    let mut input = pin!(Input::new(&bytes, &message, Rc::new(Cell::new(0))));
    let mut host: Pin<&mut dyn Read<Error = &str>> = input.as_mut();
    let mut call = pin!(decode::<
        host_family::lending::Root<dyn Read<Error = &str> + '_>,
    >());
    let mut cx = Context::from_waker(Waker::noop());
    for expected in [
        Poll::Pending,
        Poll::Ready(Yielded(6)),
        Poll::Pending,
        Poll::Ready(Yielded(2)),
        Poll::Ready(Complete(Ok([6, 2]))),
    ] {
        let mut view = host.as_mut();
        assert_eq!(call.as_mut().poll(Pin::new(&mut view), &mut cx), expected);
    }
}

#[test]
fn panic_inside_dispatch_drops_child_without_touching_expired_slot() {
    struct PanicInput;
    impl Read for PanicInput {
        type Error = std::convert::Infallible;
        fn read(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<u8, Self::Error>> {
            panic!("host read panicked");
        }
    }
    let drops = Rc::new(Cell::new(0));
    let mut host = PanicInput;
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut call = pin!(parent::<PanicInput>(drops.clone()));
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                call.as_mut().poll(Pin::new(&mut host), &mut cx)
            }))
            .is_err()
        );
    }
    assert_eq!(drops.get(), 1);
}
