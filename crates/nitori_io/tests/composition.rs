#![feature(impl_trait_in_assoc_type, yield_expr)]
#![allow(clippy::multiple_bound_locations)] // pin-project-lite generated projection bounds
use bytes::BufMut;
use nitori_call::{Call, CallPoll, Compose, Lend, Receiver, ReceiverAccess, ReceiverFamily, call};
use nitori_io::{Read, calls::ReadArrayExt as _};
use std::{
    cell::Cell,
    future::poll_fn,
    marker::{PhantomData, PhantomPinned},
    ops::ControlFlow,
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
#[call(yields = <A::Family as TaggedFamily>::Tag)]
async fn tagged<A: ReceiverAccess<Family: TaggedFamily>>(
    io: &mut A,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<<A::Family as Read>::Error>> {
    let tag = poll_fn(|cx| {
        io.poll_access(cx, |access, _| {
            Poll::Ready(<A::Family as TaggedFamily>::tag(access.into_pin()))
        })
    })
    .await;
    yield tag;
    io.read_array::<2>().await
}
#[call(yields = usize)]
async fn inner<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
    drops: Rc<Cell<usize>>,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<<A::Family as Read>::Error>> {
    let label = String::from("inner");
    let mut state = pin!(Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops,
        _pin: PhantomPinned
    });
    let mut composed = io.compose(state.as_mut());
    let mut child = pin!(composed.tagged());
    while let Some(event) = child.as_mut().next().await {
        match event {
            ControlFlow::Continue(tag) => {
                yield tag.len();
            }
            ControlFlow::Break(result) => return result,
        }
    }
    unreachable_result()
}
fn unreachable_result<T>() -> T {
    panic!("child completed without a result")
}
#[call(yields = usize)]
async fn outer<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
    drops: Rc<Cell<usize>>,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<<A::Family as Read>::Error>> {
    let label = String::from("outer");
    let mut state = pin!(Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops.clone(),
        _pin: PhantomPinned
    });
    let mut composed = io.compose(state.as_mut());
    let mut child = pin!(composed.inner(drops));
    while let Some(event) = child.as_mut().next().await {
        match event {
            ControlFlow::Continue(n) => {
                yield n;
            }
            ControlFlow::Break(result) => return result,
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
        let this = host.get_pin_mut().get_mut();
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
fn poll<'data, Op>(
    call: Pin<&mut Call<Delayed<'data>, Op>>,
    host: &mut Delayed<'data>,
) -> CallPoll<
    ControlFlow<
        <Op as nitori_call::Operation<Delayed<'data>>>::Output,
        <Op as nitori_call::Operation<Delayed<'data>>>::Yield,
    >,
>
where
    Op: nitori_call::Operation<Delayed<'data>>
        + for<'s> nitori_call::Start<'s, nitori_call::ReceiverSlot<'s, Delayed<'data>>>,
{
    call.poll_with(
        Some(Lend::Fixed(pin!(Pin::new(host).view()))),
        &mut Context::from_waker(Waker::noop()),
    )
}
#[test]
fn nested_child_local_state_nonstatic_family_and_invariant_pinned_views() {
    let data = vec![1, 2, 3];
    let mut host = Delayed {
        bytes: &data,
        waiting: false,
    };
    let drops = Rc::new(Cell::new(0));
    let mut call = Box::pin(Call::<Delayed<'_>, _>::new(Outer::new(drops.clone())));
    assert!(matches!(
        poll(call.as_mut(), &mut host),
        CallPoll::Ready(ControlFlow::Continue(5))
    ));
    assert!(matches!(poll(call.as_mut(), &mut host), CallPoll::Pending));
    assert!(matches!(poll(call.as_mut(), &mut host), CallPoll::Pending));
    assert!(matches!(
        poll(call.as_mut(), &mut host),
        CallPoll::Ready(ControlFlow::Break(Ok([1, 2])))
    ));
    assert_eq!(host.bytes, &[3]);
    drop(call);
    assert_eq!(drops.get(), 2);
}
#[test]
fn cancellation_drops_local_states_without_consuming_source() {
    let mut host = Delayed {
        bytes: &[1, 2],
        waiting: false,
    };
    let drops = Rc::new(Cell::new(0));
    let mut call = Box::pin(Call::<Delayed<'_>, _>::new(Outer::new(drops.clone())));
    let _ = poll(call.as_mut(), &mut host);
    assert!(matches!(poll(call.as_mut(), &mut host), CallPoll::Pending));
    drop(call);
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
    let call = pin!(Call::<Counted<'_, nitori_call::Direct<&[u8]>>, _>::new(
        Tagged::new()
    ));
    assert!(matches!(
        call.poll_return_with(
            Some(Lend::Fixed(view.as_mut())),
            &mut Context::from_waker(Waker::noop())
        ),
        CallPoll::Ready(Ok([b'a', b'b']))
    ));
}

#[call(sync)]
async fn chained<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
    drops: Rc<Cell<usize>>,
) -> Result<[u8; 2], nitori_io::calls::ReadArrayError<<A::Family as Read>::Error>> {
    let label = String::from("chain");
    let mut a = pin!(Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops.clone(),
        _pin: PhantomPinned,
    });
    let mut b = pin!(Count {
        label: &label,
        polls: Cell::new(0),
        dropped: drops.clone(),
        _pin: PhantomPinned,
    });
    let mut first = io.compose(a.as_mut());
    let mut second = first.compose(b.as_mut());
    second.read_array::<2>().await
}
#[test]
fn chained_composition_keeps_both_intermediate_views_alive() {
    let mut bytes = &b"abc"[..];
    let drops = Rc::new(Cell::new(0));
    let result = bytes.sync_chained_unpin(drops.clone());
    assert_eq!(result.unwrap(), *b"ab");
    assert_eq!(bytes, b"c");
    assert_eq!(drops.get(), 2);
}
