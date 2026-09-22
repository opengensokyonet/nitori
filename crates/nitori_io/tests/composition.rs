#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]
#![allow(clippy::multiple_bound_locations)] // pin-project-lite generated projection bounds
use bytes::BufMut;
use nitori_call::{
    Access, Compose, PollCallExt, Receiver, ReceiverFamily, Target, TargetExt, call,
};
use nitori_io::{Read, TargetReadExt};
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
    struct View<'visit,'data,F:ReceiverFamily> where F:'visit {
        inner:nitori_call::ViewLoan<'visit,F>,
        state:Pin<&'visit mut Count<'data>>,
        invariant:PhantomData<fn(&'visit ())->&'visit ()>,
        #[pin] pinned:PhantomPinned,
    }
}
impl<'data, F: ReceiverFamily> ReceiverFamily for Counted<'data, F> {
    type ReceiverView<'visit>
        = View<'visit, 'data, F>
    where
        Self: 'visit;
}
impl<'data, F: ReceiverFamily> nitori_call::HasReceiverFamily for View<'_, 'data, F> {
    type Family = Counted<'data, F>;
}
impl<'data, F: ReceiverFamily> Compose<F> for Count<'data> {
    type Family = Counted<'data, F>;
    fn compose<'visit, 'parent>(
        self: Pin<&'visit mut Self>,
        inner: Pin<&'visit mut F::ReceiverView<'parent>>,
    ) -> View<'visit, 'data, F>
    where
        F: 'parent,
        'parent: 'visit,
        Self::Family: 'visit,
    {
        View {
            inner: nitori_call::ViewLoan::new(inner),
            state: self,
            invariant: PhantomData,
            pinned: PhantomPinned,
        }
    }
}
impl<F: Read> Read for Counted<'_, F> {
    type Error = F::Error;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
        cx: &mut Context<'_>,
        out: &mut B,
    ) -> Poll<Result<usize, Self::Error>>
    where
        Self: 'v,
    {
        let host = host.project();
        host.state.polls.set(host.state.polls.get() + 1);
        host.inner
            .with(|access| F::poll_read(access.into_pin(), cx, out))
    }
}
trait TaggedFamily: Read {
    type Tag;
    fn tag<'v>(host: Pin<&mut Self::ReceiverView<'v>>) -> Self::Tag
    where
        Self: 'v;
}
impl<'data, F: Read> TaggedFamily for Counted<'data, F> {
    type Tag = &'data str;
    fn tag<'v>(host: Pin<&mut Self::ReceiverView<'v>>) -> &'data str
    where
        Self: 'v,
    {
        host.project().state.label
    }
}
#[call(yields = F::Tag)]
async fn tagged<F: TaggedFamily>(
    io: Target<F>,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<F::Error>> {
    let tag = io.with(|a: Access<'_, '_, F>| F::tag(a.into_pin())).await;
    yield tag;
    io.read_array::<2>().await
}
#[call(yields = usize)]
async fn inner<F: Read>(
    io: Target<F>,
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
    io: Target<F>,
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
nitori_call::family_receiver!(impl ['data] for Delayed<'data>);
impl Read for Delayed<'_> {
    type Error = std::convert::Infallible;
    fn poll_read<'v, B: BufMut + ?Sized>(
        host: Pin<&mut Self::ReceiverView<'v>>,
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
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut host), &mut cx),
        Poll::Ready(CoroutineState::Yielded(5))
    ));
    assert!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut host), &mut cx)
            .is_pending()
    );
    assert!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut host), &mut cx)
            .is_pending()
    );
    assert!(matches!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut host), &mut cx),
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
    let _ = operation
        .as_mut()
        .poll_receiver(Pin::new(&mut host), &mut cx);
    assert!(
        operation
            .as_mut()
            .poll_receiver(Pin::new(&mut host), &mut cx)
            .is_pending()
    );
    drop(operation);
    assert_eq!(drops.get(), 2);
    assert_eq!(host.bytes, &[1, 2]);
}
#[test]
fn explicit_view_executes_without_reconstruction() {
    let mut bytes = &b"abcd"[..];
    let drops = Rc::new(Cell::new(0));
    let label = String::from("owned outside call");
    let mut state = pin!(Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops.clone(),
        _pin: PhantomPinned
    });
    let mut parent = pin!(Pin::new(&mut bytes).view());
    let mut view = pin!(<Count<'_> as Compose<nitori_call::Direct<&[u8]>>>::compose(
        state.as_mut(),
        parent.as_mut()
    ));
    use nitori_call::CallOn;
    let mut scope = pin!(nitori_call::ViewScope::<
        Counted<'_, nitori_call::Direct<&[u8]>>,
    >::new(view.as_mut()));
    let mut call = pin!(tagged::<Counted<'_, nitori_call::Direct<&[u8]>>>());
    assert!(matches!(
        call.as_mut()
            .poll_return(scope.as_mut(), &mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok([b'a', b'b']))
    ));
}

#[call]
async fn chained<F: Read>(
    io: Target<F>,
    drops: Rc<Cell<usize>>,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<F::Error>> {
    let label = String::from("chain");
    let a = Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops.clone(),
        _pin: PhantomPinned,
    };
    let b = Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops.clone(),
        _pin: PhantomPinned,
    };
    io.compose(a).compose(b).read_array::<2>().await
}
#[test]
fn chained_target_keeps_both_intermediate_views_alive() {
    let mut bytes = &b"abc"[..];
    let drops = Rc::new(Cell::new(0));
    let result = nitori_call::run_sync(
        Pin::new(&mut bytes),
        chained::<nitori_call::Direct<&[u8]>>(drops.clone()),
    );
    assert_eq!(result.unwrap(), *b"ab");
    assert_eq!(bytes, b"c");
    assert_eq!(drops.get(), 2);
}
