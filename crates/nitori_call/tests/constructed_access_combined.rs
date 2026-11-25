//! `ConstructedAccess` together with lent operations, tuples, calls, lent
//! calls and children.
#![feature(impl_trait_in_assoc_type, yield_expr)]
use nitori_call::{
    Call, CallPoll, Child, ConstructedAccess, Direct, Lend, LentAccess, LentAccesses, LentCall,
    Receiver, ReceiverAccess, ReceiverFamily, call, capability, direct_receiver,
};
use nitori_io::prelude::*;
use std::{
    future::Future,
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

fn block<T>(future: impl Future<Output = T>) -> T {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..10_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
    panic!("the future did not finish")
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

#[test]
fn lent_operation_method_on_constructed() {
    let mut access = ConstructedAccess::new(Total(0));
    assert_eq!(block(access.add_twice(3)), 6);
    assert_eq!(access.add(1), 7);
}

#[test]
fn lent_tuple_with_constructed() {
    let mut a = ConstructedAccess::new(Total(1));
    let mut b = ConstructedAccess::new(Total(10));
    let r = (&mut a, &mut b).with(|(mut a, mut b)| a.add(1) + b.add(1));
    assert_eq!(r, 13);
    let r = (&mut a, (&mut b, &mut ConstructedAccess::new(Total(100))))
        .with(|(mut a, rest)| a.add(0) + rest.with(|(mut b, mut c)| b.add(0) + c.add(0)));
    assert_eq!(r, 2 + 11 + 100);
}

#[test]
fn several_constructed_in_a_child() {
    let mut a = ConstructedAccess::new(Total(1));
    let mut b = ConstructedAccess::new(Total(2));
    let child = pin!(Child::new(SumTwo::new(), (&mut a, &mut b)));
    assert_eq!(block(child), 3);
}

/// An operation run in a Call that also holds a ConstructedAccess of a
/// local receiver: the slot asks for a view, the constructed one never does.
#[call(yields = u32)]
async fn mixed<F: Counter>(io: &mut impl ReceiverAccess<Family = F>) -> u32 {
    let mut local = ConstructedAccess::new(Total(100));
    {
        let mut child = pin!(local.add_each(vec![1, 2]));
        while let ControlFlow::Continue(item) = child.as_mut().step().await {
            yield item;
        }
    }
    io.total().await + local.get_ref().0
}

#[test]
fn constructed_inside_a_call() {
    let mut call = pin!(Call::<Direct<Total>, _>::new(Mixed::new()));
    let mut cx = Context::from_waker(Waker::noop());
    let mut items = Vec::new();
    let mut needs = 0;
    let mut total = Total(5);
    loop {
        match call.as_mut().poll_with(None, &mut cx) {
            CallPoll::Ready(ControlFlow::Continue(i)) => items.push(i),
            CallPoll::Ready(ControlFlow::Break(v)) => panic!("finished without a view: {v}"),
            CallPoll::NeedsView => {
                needs += 1;
                break;
            }
            CallPoll::Pending | CallPoll::Releasable => {}
        }
    }
    assert_eq!(items, [101, 103]);
    let view = pin!(Pin::new(&mut total).view());
    match call.as_mut().poll_with(Some(Lend::Fixed(view)), &mut cx) {
        CallPoll::Ready(ControlFlow::Break(v)) => assert_eq!(v, 5 + 103),
        _ => panic!("expected the final value"),
    }
    assert_eq!(needs, 1);
}

/// Inside a LentCall, a ConstructedAccess of a local receiver and the lent
/// slot in one tuple.
#[call]
async fn lent_and_local<F: Counter>(io: &mut impl LentAccess<Family = F>) -> u32 {
    let mut local = ConstructedAccess::new(Total(100));
    (&mut *io, &mut local).with(|(mut a, mut b)| a.add(1) + b.add(1))
}

#[test]
fn lent_slot_and_constructed_in_a_tuple() {
    let mut call = pin!(LentCall::<Direct<Total>, _>::new(LentAndLocal::new()));
    let mut total = Total(1);
    let mut view = pin!(Pin::new(&mut total).view());
    let r = call
        .as_mut()
        .poll_return_with_views(view.as_mut(), &mut Context::from_waker(Waker::noop()));
    assert!(matches!(r, Poll::Ready(103)));
}

#[test]
fn send_to_a_scoped_thread_and_sync() {
    fn assert_sync<T: Sync>(_: &T) {}
    let mut access = ConstructedAccess::new(Total(1));
    assert_sync(&access);
    std::thread::scope(|s| {
        s.spawn(|| assert_eq!(access.add(1), 2));
    });
    assert_eq!(access.into_inner().0, 2);
}

#[test]
fn release_point_through_composed() {
    let mut access = ConstructedAccess::new(b"abc".as_slice());
    let mut cx = Context::from_waker(Waker::noop());
    let mut composed = access.compose(nitori_io::adapters::ReadLimit(2usize));
    assert!(pin!(composed.release_view()).poll(&mut cx).is_ready());
    assert_eq!(block(composed.read_array::<2>()).unwrap().value, *b"ab");
}

/// A view is constructed at each access: per-view state lasts one access.
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

#[test]
fn one_view_per_access() {
    let mut access = ConstructedAccess::new(Counted(0));
    for _ in 0..3 {
        access.with(|_| ());
    }
    let mut cx = Context::from_waker(Waker::noop());
    let _ = access.poll_access(&mut cx, |_, _| Poll::Ready(()));
    assert_eq!(access.get_ref().0, 4);
}
