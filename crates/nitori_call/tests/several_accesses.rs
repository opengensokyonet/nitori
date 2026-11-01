//! Which parameters of `#[call]` are accesses, and accesses sharing a family.
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{
    Call, CallPoll, Constructed, Direct, Lend, LentAccess, LentCall, Operation, Receiver,
    ReceiverAccess, ReceiverFamily, ViewLoan, call, capability, direct_receiver, drive,
};
use std::{
    future::Future,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

#[capability]
pub trait Counter: ReceiverFamily {
    type Amount: From<u32>;
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, by: u32) -> u32
    where
        Self: 'v;
    fn poll_total<'v>(view: Pin<&mut Self::ReceiverView<'v>>, cx: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v;
}

#[derive(Default)]
pub struct Total(u32);
#[derive(Default)]
pub struct Doubled(u32);
direct_receiver!(impl [] for Total);
direct_receiver!(impl [] for Doubled);
impl Counter for Direct<Total> {
    type Amount = u64;
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, by: u32) -> u32
    where
        Self: 'v,
    {
        let total = view.get_pin_mut().get_mut();
        total.0 += by;
        total.0
    }
    fn poll_total<'v>(view: Pin<&mut Self::ReceiverView<'v>>, _: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v,
    {
        Poll::Ready(view.get_pin_mut().get_mut().0)
    }
}
impl Counter for Direct<Doubled> {
    type Amount = u64;
    fn add<'v>(view: Pin<&mut Self::ReceiverView<'v>>, by: u32) -> u32
    where
        Self: 'v,
    {
        let total = view.get_pin_mut().get_mut();
        total.0 += by;
        total.0 * 2
    }
    fn poll_total<'v>(view: Pin<&mut Self::ReceiverView<'v>>, _: &mut Context<'_>) -> Poll<u32>
    where
        Self: 'v,
    {
        Poll::Ready(view.get_pin_mut().get_mut().0 * 2)
    }
}

type Totals = (Direct<Total>, Direct<Total>);

#[call(sync)]
async fn sum_impl<F: Counter>(
    first: &mut impl ReceiverAccess<Family = F>,
    second: &mut impl ReceiverAccess<Family = F>,
    scale: u32,
) -> u32 {
    first.total().await + second.total().await * scale
}

#[call(sync)]
async fn sum_named<F: Counter, A: ReceiverAccess<Family = F>, B: ReceiverAccess<Family = F>>(
    first: &mut A,
    second: &mut B,
    scale: u32,
) -> u32 {
    first.total().await + second.total().await * scale
}

#[call]
async fn step_impl<F: Counter>(
    first: &mut impl LentAccess<Family = F>,
    second: &mut impl LentAccess<Family = F>,
) -> u32 {
    first.add(1) + second.add(2)
}

#[call]
async fn step_named<F, A, B>(first: &mut A, second: &mut B) -> u32
where
    F: Counter,
    A: LentAccess<Family = F>,
    B: LentAccess<Family = F>,
{
    first.add(1) + second.add(2)
}

#[call]
async fn mixed_impl<F: Counter>(
    plain: &mut impl ReceiverAccess<Family = F>,
    lent: &mut impl LentAccess<Family = F>,
) -> u32 {
    let total = plain.total().await;
    lent.add(total)
}

#[call]
async fn mixed_named<F, A, B>(lent: &mut A, plain: &mut B) -> u32
where
    F: Counter,
    A: LentAccess<Family = F>,
    B: ReceiverAccess<Family = F>,
{
    let total = plain.total().await;
    lent.add(total)
}

/// Lends a view to each slot of a call over two totals as the slot asks.
macro_rules! assert_each_slot_asks {
    ($operation:expr) => {{
        let mut first = Total(1);
        let mut second = Total(2);
        let mut call = pin!(Call::<Totals, _>::new($operation));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(
            call.as_mut().poll_return_with_views((None, None), &mut cx),
            CallPoll::NeedsView
        );
        assert_eq!(call.views_needed(), (true, false));
        {
            let mut view = pin!(Pin::new(&mut first).view());
            let lends = (Some(Lend::Fixed(ViewLoan::new(view.as_mut()))), None);
            assert_eq!(
                call.as_mut().poll_return_with_views(lends, &mut cx),
                CallPoll::NeedsView
            );
        }
        assert_eq!(call.views_needed(), (false, true));
        let mut view = pin!(Pin::new(&mut second).view());
        let lends = (None, Some(Lend::Fixed(ViewLoan::new(view.as_mut()))));
        assert_eq!(
            call.as_mut().poll_return_with_views(lends, &mut cx),
            CallPoll::Ready(21)
        );
    }};
}

/// Runs a lent operation over two totals to its final value.
macro_rules! lent_output {
    ($operation:expr, $first:expr, $second:expr) => {{
        let mut first = pin!(Pin::new(&mut $first).view());
        let mut second = pin!(Pin::new(&mut $second).view());
        let mut call = pin!(LentCall::<Totals, _>::new($operation));
        match call.as_mut().poll_with_views(
            (first.as_mut(), second.as_mut()),
            &mut Context::from_waker(Waker::noop()),
        ) {
            Poll::Ready(ControlFlow::Break(output)) => output,
            _ => panic!("nothing here waits"),
        }
    }};
}

#[test]
fn accesses_sharing_a_family_each_have_a_slot() {
    assert_each_slot_asks!(SumImpl::new(10));
    assert_each_slot_asks!(SumNamed::new(10));
}

#[test]
fn accesses_sharing_a_family_each_have_a_lent_view() {
    let (mut first, mut second) = (Total(1), Total(2));
    assert_eq!(lent_output!(StepImpl::new(), first, second), 2 + 4);
    assert_eq!(lent_output!(StepNamed::new(), first, second), 3 + 6);
    assert_eq!((first.0, second.0), (3, 6));
}

#[test]
fn plain_and_lent_accesses_share_a_family() {
    let (mut first, mut second) = (Total(1), Total(2));
    assert_eq!(lent_output!(MixedImpl::new(), first, second), 3);
    assert_eq!(second.0, 3);
    assert_eq!(lent_output!(MixedNamed::new(), first, second), 4);
    assert_eq!(first.0, 4);
}

#[test]
fn views_are_constructed_for_accesses_sharing_a_family() {
    let (mut first, mut second) = (Total(1), Total(2));
    let mut driven = pin!(drive(
        (
            Constructed::new(Pin::new(&mut first)),
            Constructed::new(Pin::new(&mut second)),
        ),
        SumImpl::new(10),
    ));
    assert_eq!(
        driven
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(21)
    );
}

#[test]
fn executors_have_host_methods_over_a_shared_family() {
    let mut hosts = (Total(1), Total(2));
    assert_eq!(hosts.sync_sum_impl_unpin(10), 21);
    assert_eq!(hosts.sync_sum_named_unpin(100), 201);
    let mut pinned = pin!((Total(3), Total(4)));
    assert_eq!(pinned.as_mut().sync_sum_impl(10), 43);
}

/// A shared family is one parameter of the host trait and two of the receivers.
#[call(sync)]
async fn around<F: Counter, G: Counter>(
    first: &mut impl ReceiverAccess<Family = F>,
    middle: &mut impl ReceiverAccess<Family = G>,
    last: &mut impl ReceiverAccess<Family = F>,
) -> u32 {
    first.total().await + middle.total().await * 10 + last.total().await * 100
}

#[test]
fn a_shared_family_may_surround_another() {
    assert_eq!((Total(1), Doubled(1), Total(3)).sync_around_unpin(), 321);
    fn operation<O: Operation<(Direct<Total>, Direct<Doubled>, Direct<Total>)>>(_: O) {}
    operation(Around::new());
}

/// The output names the shared family by its parameter and through an access.
#[call(sync)]
async fn amount<F: Counter>(
    first: &mut impl ReceiverAccess<Family = F>,
    second: &mut impl ReceiverAccess<Family = F>,
) -> F::Amount {
    F::Amount::from(first.total().await + second.total().await)
}
#[call(sync)]
async fn amount_through_access<F, A, B>(
    first: &mut A,
    second: &mut B,
) -> <B::Family as Counter>::Amount
where
    F: Counter,
    A: ReceiverAccess<Family = F>,
    B: ReceiverAccess<Family = F>,
{
    F::Amount::from(first.total().await + second.total().await)
}

#[test]
fn the_output_names_a_shared_family() {
    let mut hosts = (Total(1), Total(2));
    assert_eq!(hosts.sync_amount_unpin(), 3_u64);
    assert_eq!(hosts.sync_amount_through_access_unpin(), 3_u64);
}

/// Only bounds make a parameter an access, not the text of its type.
pub struct ReceiverAccessLog(u32);
pub struct LentAccessLog(u32);
#[call(sync)]
async fn one_access_then_arguments<F: Counter, T: Into<u32>>(
    io: &mut impl ReceiverAccess<Family = F>,
    log: &ReceiverAccessLog,
    entry: T,
    other: impl Into<u32>,
) -> u32 {
    io.total().await + log.0 + entry.into() + other.into()
}
#[call(sync)]
async fn two_accesses_then_arguments<F: Counter, T: Into<u32>>(
    first: &mut impl ReceiverAccess<Family = F>,
    second: &mut impl ReceiverAccess<Family = F>,
    log: LentAccessLog,
    entry: T,
    other: impl Into<u32>,
) -> u32 {
    first.total().await + second.total().await + log.0 + entry.into() + other.into()
}

#[test]
fn other_parameters_stay_arguments() {
    let mut total = Total(1);
    let log = ReceiverAccessLog(2);
    assert_eq!(
        total.sync_one_access_then_arguments_unpin(&log, 3_u8, 4_u16),
        10
    );
    let mut hosts = (Total(1), Total(2));
    assert_eq!(
        hosts.sync_two_accesses_then_arguments_unpin(LentAccessLog(3), 4_u8, 5_u16),
        15
    );
}
