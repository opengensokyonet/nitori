#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
#![allow(clippy::multiple_bound_locations)] // pin-project-lite generated projection bounds
use bytes::BufMut;
use nitori_call::{Access, Compose, Host, HostFamily, PollCallExt, Receiver, ReceiverExt, call};
use nitori_io::{Read, ReceiverReadExt};
use std::{
    cell::Cell,
    marker::{PhantomData, PhantomPinned},
    ops::CoroutineState,
    pin::{Pin, pin},
    rc::Rc,
    task::{Context, Poll, Waker},
};

struct Count<'data> {
    label: &'data str,
    polls: Cell<usize>,
    dropped: Rc<Cell<usize>>,
    _pin: PhantomPinned,
}
impl Drop for Count<'_> {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}
struct Counted<'data, F>(PhantomData<fn() -> (&'data str, F)>);
pin_project_lite::pin_project! {
    // pin-project-lite parses the trait and lifetime bounds separately.
    #[allow(clippy::multiple_bound_locations)]
    struct View<'visit,'data,F:HostFamily> where F:'visit {
        #[pin] inner:F::Host<'visit>,
        state:Pin<&'visit mut Count<'data>>,
        invariant:PhantomData<fn(&'visit ())->&'visit ()>,
        #[pin] pinned:PhantomPinned,
    }
}
impl<'data, F: HostFamily> HostFamily for Counted<'data, F> {
    type Host<'visit>
        = View<'visit, 'data, F>
    where
        Self: 'visit;
}
impl<'data, F: HostFamily> Host for View<'_, 'data, F> {
    type Family = Counted<'data, F>;
    fn view<'visit>(self: Pin<&'visit mut Self>) -> View<'visit, 'data, F>
    where
        Self::Family: 'visit,
    {
        let this = self.project();
        View {
            inner: this.inner.view(),
            state: this.state.as_mut(),
            invariant: PhantomData,
            pinned: PhantomPinned,
        }
    }
}
impl<'data, F: HostFamily> Compose<F> for Count<'data> {
    type Family = Counted<'data, F>;
    fn compose<'visit>(
        self: Pin<&'visit mut Self>,
        inner: F::Host<'visit>,
    ) -> View<'visit, 'data, F>
    where
        F: 'visit,
        Self::Family: 'visit,
    {
        View {
            inner,
            state: self,
            invariant: PhantomData,
            pinned: PhantomPinned,
        }
    }
}
impl<F: Read> Read for Counted<'_, F> {
    type Error = F::Error;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::Host<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        let host = host.project();
        host.state.polls.set(host.state.polls.get() + 1);
        F::poll_read(host.inner, cx, out)
    }
}
trait TaggedFamily: Read {
    type Tag;
    fn tag<'v>(host: Pin<&mut Self::Host<'v>>) -> Self::Tag
    where
        Self: 'v;
}
impl<'data, F: Read> TaggedFamily for Counted<'data, F> {
    type Tag = &'data str;
    fn tag<'v>(host: Pin<&mut Self::Host<'v>>) -> &'data str
    where
        Self: 'v,
    {
        host.project().state.label
    }
}
#[call(yields = F::Tag)]
async fn tagged<F: TaggedFamily>(
    io: Receiver<F>,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<F::Error>> {
    let tag = io.with(|a: Access<'_, '_, F>| F::tag(a.into_pin())).await;
    yield tag;
    io.read_array::<2>().await
}
#[call(yields = usize)]
async fn inner<F: Read>(
    io: Receiver<F>,
    drops: Rc<Cell<usize>>,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<F::Error>> {
    let label = String::from("inner");
    let mut state = pin!(Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops,
        _pin: PhantomPinned
    });
    let mut composed = io.compose(state.as_mut());
    let mut child = pin!((&mut composed).tagged());
    while let Some(event) = child.as_mut().next().await {
        match event {
            CoroutineState::Yielded(tag) => {
                yield tag.len();
            }
            CoroutineState::Complete(result) => return result,
        }
    }
    unreachable_result()
}
fn unreachable_result<T>() -> T {
    panic!("child completed without a result")
}
#[call(yields=usize)]
async fn outer<F: Read>(
    io: Receiver<F>,
    drops: Rc<Cell<usize>>,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<F::Error>> {
    let label = String::from("outer");
    let mut state = pin!(Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops.clone(),
        _pin: PhantomPinned
    });
    let mut child = pin!(io.compose(state.as_mut()).inner(drops));
    while let Some(event) = child.as_mut().next().await {
        match event {
            CoroutineState::Yielded(n) => {
                yield n;
            }
            CoroutineState::Complete(result) => return result,
        }
    }
    unreachable_result()
}
struct Delayed<'data> {
    bytes: &'data [u8],
    waiting: bool,
}
nitori_call::family_host!(impl ['data] for Delayed<'data>);
impl Read for Delayed<'_> {
    type Error = std::convert::Infallible;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::Host<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        let this = host.get_mut().0.as_mut().get_mut();
        if !this.waiting {
            this.waiting = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        this.waiting = false;
        let count = out.remaining_mut().min(this.bytes.len()).min(1);
        out.put_slice(&this.bytes[..count]);
        this.bytes = &this.bytes[count..];
        Poll::Ready(Ok(count))
    }
}
#[test]
fn nested_child_local_state_nonstatic_family_and_invariant_pinned_views() {
    let data = vec![1, 2, 3];
    let mut host = Delayed {
        bytes: &data,
        waiting: false,
    };
    let drops = Rc::new(Cell::new(0));
    let mut operation = Box::pin(outer::<Delayed<'_>>(drops.clone()));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        operation.as_mut().poll_host(Pin::new(&mut host), &mut cx),
        Poll::Ready(CoroutineState::Yielded(5))
    ));
    assert!(
        operation
            .as_mut()
            .poll_host(Pin::new(&mut host), &mut cx)
            .is_pending()
    );
    assert!(
        operation
            .as_mut()
            .poll_host(Pin::new(&mut host), &mut cx)
            .is_pending()
    );
    assert!(matches!(
        operation.as_mut().poll_host(Pin::new(&mut host), &mut cx),
        Poll::Ready(CoroutineState::Complete(Ok([1, 2])))
    ));
    assert_eq!(host.bytes, &[3]);
    drop(operation);
    assert_eq!(drops.get(), 2);
}
#[test]
fn cancellation_drops_local_states_without_consuming_source() {
    let mut host = Delayed {
        bytes: &[1, 2],
        waiting: false,
    };
    let drops = Rc::new(Cell::new(0));
    let mut operation = Box::pin(outer::<Delayed<'_>>(drops.clone()));
    let mut cx = Context::from_waker(Waker::noop());
    let _ = operation.as_mut().poll_host(Pin::new(&mut host), &mut cx);
    assert!(
        operation
            .as_mut()
            .poll_host(Pin::new(&mut host), &mut cx)
            .is_pending()
    );
    drop(operation);
    assert_eq!(drops.get(), 2);
    assert_eq!(host.bytes, &[1, 2]);
}
#[test]
fn generated_extension_is_available_on_author_defined_view() {
    let mut bytes = &b"abcd"[..];
    let drops = Rc::new(Cell::new(0));
    let label = String::from("owned outside call");
    let mut state = pin!(Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops.clone(),
        _pin: PhantomPinned
    });
    let mut view = pin!(<Count<'_> as Compose<nitori_call::Direct<&[u8]>>>::compose(
        state.as_mut(),
        Pin::new(&mut bytes).view()
    ));
    use std::future::Future;
    let mut call = pin!(view.as_mut().tagged());
    assert!(matches!(
        call.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok([b'a', b'b']))
    ));
}
