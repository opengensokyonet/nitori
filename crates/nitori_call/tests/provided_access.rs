//! Calls and lent calls over several receivers and wrapped accesses.
#![feature(impl_trait_in_assoc_type, yield_expr)]
use nitori_call::{
    AccessPlan, Call, CallPoll, Child, Direct, Lend, Lent, LentAccess, LentAccesses, LentCall,
    ProvideAccess, Receiver, ReceiverAccess, ReceiverFamily, ViewLoan, ViewProjection, call,
    capability, direct_receiver, project_access,
};
use nitori_io::prelude::*;
use std::{
    future::poll_fn,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

#[capability]
pub trait Counter: ReceiverFamily {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, by: u32) -> u32
    where
        Self: 'v;
    fn poll_total<'v>(view: Pin<&mut Self::ReceiverView<'v>>, cx: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v;
}
#[derive(Default, Debug)]
pub struct Total(u32);
direct_receiver!(impl [] for Total);
impl Counter for Direct<Total> {
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, by: u32) -> u32
    where
        Self: 'v,
    {
        let t = view.get_pin_mut().get_mut();
        t.0 += by;
        t.0
    }
    fn poll_total<'v>(view: Pin<&mut Self::ReceiverView<'v>>, _: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v,
    {
        Poll::Ready(view.get_pin_mut().get_mut().0)
    }
}

#[call]
async fn add_twice<F: Counter>(io: &mut impl LentAccess<Family = F>, by: u32) -> u32 {
    io.add(by);
    io.add(by)
}

#[call(yields = u32)]
async fn add_each<F: Counter, A: LentAccess<Family = F>>(io: &mut A, values: Vec<u32>) -> usize {
    let count = values.len();
    for value in values {
        yield io.add(value);
    }
    count
}

#[call]
async fn sum_two<F: Counter>(
    first: &mut impl ReceiverAccess<Family = F>,
    second: &mut impl ReceiverAccess<Family = F>,
) -> u32 {
    first.total().await + second.total().await
}

#[call]
async fn total_after_adds(io: &mut impl LentAccess<Family: Counter>) -> u32 {
    assert_eq!(io.add_twice(3).await, 6);
    io.add(1)
}

#[test]
fn lent_operation_method_on_a_lent_slot() {
    let mut total = Total(0);
    let view = pin!(Pin::new(&mut total).view());
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(TotalAfterAdds::new()));
    let result = call
        .as_mut()
        .poll_return_with_views(view, &mut Context::from_waker(Waker::noop()));
    assert_eq!(result, Poll::Ready(7));
    assert_eq!(total.0, 7);
}

#[call]
async fn lent_tuple<F: Counter>(
    a: &mut impl LentAccess<Family = F>,
    b: &mut impl LentAccess<Family = F>,
    c: &mut impl LentAccess<Family = F>,
) -> u32 {
    let r = (&mut *a, &mut *b).with(|(mut a, mut b)| a.add(1) + b.add(1));
    assert_eq!(r, 13);
    (a, (b, c)).with(|(mut a, rest)| a.add(0) + rest.with(|(mut b, mut c)| b.add(0) + c.add(0)))
}

#[test]
fn views_of_nested_lent_accesses() {
    let (mut a, mut b, mut c) = (Total(1), Total(10), Total(100));
    let mut call =
        pin!(LentCall::<(Direct<Total>, Direct<Total>, Direct<Total>), _>::new(LentTuple::new()));
    let result = call.as_mut().poll_return_with_views(
        (
            pin!(Pin::new(&mut a).view()),
            pin!(Pin::new(&mut b).view()),
            pin!(Pin::new(&mut c).view()),
        ),
        &mut Context::from_waker(Waker::noop()),
    );
    assert_eq!(result, Poll::Ready(113));
}

#[call(sync)]
async fn child_sum<F: Counter>(
    a: &mut impl ReceiverAccess<Family = F>,
    b: &mut impl ReceiverAccess<Family = F>,
) -> u32 {
    Child::new(SumTwo::new(), (a, b)).await
}

#[test]
fn several_slots_in_a_child() {
    let mut hosts = (Total(1), Total(2));
    assert_eq!(hosts.sync_child_sum_unpin(), 3);
}

/// A call on a local receiver can advance before the outer slot needs its view.
#[call(yields = u32)]
async fn mixed<F: Counter>(io: &mut impl ReceiverAccess<Family = F>) -> u32 {
    let mut local = Total(100);
    {
        let mut call = pin!(LentCall::<Direct<Total>, _>::new(AddEach::new(vec![1, 2])));
        while let ControlFlow::Continue(item) = poll_fn(|cx| {
            let view = pin!(Pin::new(&mut local).view());
            call.as_mut().poll_with_views(view, cx)
        })
        .await
        {
            yield item;
        }
    }
    io.total().await + local.0
}

#[test]
fn a_call_on_a_local_receiver_inside_a_call() {
    let mut call = pin!(Call::<Direct<Total>, _>::new(Mixed::new()));
    let mut cx = Context::from_waker(Waker::noop());
    let mut items = Vec::new();
    loop {
        match call.as_mut().poll_with(None, &mut cx) {
            CallPoll::Ready(ControlFlow::Continue(i)) => items.push(i),
            CallPoll::Ready(ControlFlow::Break(v)) => panic!("finished without a view: {v}"),
            CallPoll::NeedsView => break,
            CallPoll::Pending | CallPoll::Releasable => {}
        }
    }
    assert_eq!(items, [101, 103]);
    let mut total = Total(5);
    let view = pin!(Pin::new(&mut total).view());
    match call.as_mut().poll_with(Some(Lend::Fixed(view)), &mut cx) {
        CallPoll::Ready(ControlFlow::Break(v)) => assert_eq!(v, 108),
        _ => panic!("expected the final value"),
    }
}

/// Lend the same view with a counter for how often the wrapper projects it.
struct CountProjections;

impl<F: ReceiverFamily> ViewProjection<Lent<F>> for CountProjections {
    type Family = Lent<F>;
    type State = usize;

    fn project<'v>(view: ViewLoan<'v, F>, state: &'v mut usize) -> ViewLoan<'v, F>
    where
        Lent<F>: 'v,
    {
        *state += 1;
        view
    }
}

struct Projected<A> {
    parent: A,
    projections: usize,
}

impl<A: ReceiverAccess> ProvideAccess for Projected<A> {
    type AccessFamily = Lent<A::Family>;
    type Lentness = <A as ProvideAccess>::Lentness;

    fn access_plan<'a>(
        &'a mut self,
    ) -> impl AccessPlan<'a, Family = Self::AccessFamily, Lentness = Self::Lentness> {
        project_access::<_, CountProjections>(&mut self.parent, &mut self.projections)
    }
}

#[call]
async fn lent_and_projected<F: Counter>(
    a: &mut impl LentAccess<Family = F>,
    b: &mut impl LentAccess<Family = F>,
) -> u32 {
    let mut projected = Projected {
        parent: b,
        projections: 0,
    };
    let result = (a, &mut projected)
        .with(|(mut a, b)| a.add(1) + b.into_pin().get_mut().with(|mut b| b.add(1)));
    assert_eq!(projected.projections, 1);
    result
}

#[test]
fn a_projection_preserves_lentness() {
    let (mut a, mut b) = (Total(1), Total(100));
    let mut call = pin!(LentCall::<(Direct<Total>, Direct<Total>), _>::new(
        LentAndProjected::new()
    ));
    let result = call.as_mut().poll_return_with_views(
        (pin!(Pin::new(&mut a).view()), pin!(Pin::new(&mut b).view())),
        &mut Context::from_waker(Waker::noop()),
    );
    assert_eq!(result, Poll::Ready(103));
}

#[call(sync)]
async fn increment<F: Counter>(io: &mut impl ReceiverAccess<Family = F>) -> u32 {
    poll_fn(|cx| io.poll_access(cx, |mut view, _| Poll::Ready(view.add(1)))).await
}

#[test]
fn send_a_receiver_to_a_scoped_thread() {
    fn assert_sync<T: Sync>(_: &T) {}
    let mut total = Total(1);
    assert_sync(&total);
    std::thread::scope(|s| {
        s.spawn(|| assert_eq!(total.sync_increment_unpin(), 2));
    });
    assert_eq!(total.0, 2);
}

#[call(sync)]
async fn release_composed<F: Read<Metadata = (), Error = !>>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> [u8; 2] {
    let mut composed = nitori_io::adapters::ReadLimit::new(&mut *io, 2usize);
    composed.release_view().await;
    composed.read_array::<2>().await.unwrap().value
}

#[test]
fn release_point_through_composed() {
    let mut input = &b"abc"[..];
    assert_eq!(input.sync_release_composed_unpin(), *b"ab");
    assert_eq!(input, b"c");
}

/// Count host views across calls and across accesses in one call.
struct Counted(u32);
pub struct CountedView<'v>(std::marker::PhantomData<Pin<&'v mut Counted>>);
pub struct CountedFamily;
impl ReceiverFamily for CountedFamily {
    type ReceiverView<'v>
        = CountedView<'v>
    where
        Self: 'v;
}
impl Receiver for Counted {
    type Family = CountedFamily;
    fn view<'v>(mut self: Pin<&'v mut Self>) -> CountedView<'v>
    where
        Self::Family: 'v,
    {
        self.0 += 1;
        CountedView(std::marker::PhantomData)
    }
}

#[call(sync)]
async fn use_view_twice<F: ReceiverFamily>(io: &mut impl ReceiverAccess<Family = F>) {
    for _ in 0..2 {
        poll_fn(|cx| io.poll_access(cx, |_, _| Poll::Ready(()))).await;
    }
}

#[test]
fn one_view_per_host_poll() {
    let mut receiver = Counted(0);
    for _ in 0..3 {
        receiver.sync_use_view_twice_unpin();
    }
    assert_eq!(receiver.0, 3);
}
