//! Tuples of receivers and of accesses, at their largest and nested.
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{
    Call, CallPoll, Constructed, Direct, Emitter, Guarded, LentAccess, LentAccesses, LentCall,
    LockAcquisition, Operation, Receiver, ReceiverAccess, ReceiverFamily, Receivers, Start, call,
    capability, direct_receiver, drive, drive_composed,
};
use std::{
    cell::Cell,
    convert::Infallible,
    future::{Future, pending},
    ops::ControlFlow,
    pin::{Pin, pin},
    rc::Rc,
    sync::Mutex,
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

#[derive(Default)]
pub struct Total(u32);
direct_receiver!(impl [] for Total);
impl Counter for Direct<Total> {
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

macro_rules! direct_total {
    ($name:ident) => {
        Direct<Total>
    };
}
macro_rules! one {
    ($name:ident) => {
        1
    };
}

macro_rules! largest_tuple {
    ($($first:ident)+; $($second:ident)+) => {
        type Half = ($(direct_total!($first),)+);
        type Totals = ($(direct_total!($first),)+ $(direct_total!($second),)+);

        #[allow(clippy::too_many_arguments)]
        #[call(sync)]
        async fn add_up($($first: &mut impl ReceiverAccess<Family: Counter>,)+ $($second: &mut impl ReceiverAccess<Family: Counter>),+) -> u32 {
            0 $(+ $first.total().await)+ $(+ $second.total().await)+
        }

        #[allow(clippy::too_many_arguments)]
        #[call]
        async fn step_all($($first: &mut impl LentAccess<Family: Counter>,)+ $($second: &mut impl LentAccess<Family: Counter>),+) -> u32 {
            ($($first,)+ $($second,)+).with(|($(mut $first,)+ $(mut $second,)+)| {
                0 $(+ $first.add(1))+ $(+ $second.add(1))+
            })
        }

        /// One step over two halves, each a tuple of its own.
        struct StepHalves;
        impl<R: Receivers> Operation<R> for StepHalves {
            type Output = u32;
            type Yield = Infallible;
        }
        #[allow(non_camel_case_types)]
        impl<'s, $($first,)+ $($second,)+> Start<'s, (($($first,)+), ($($second,)+))> for StepHalves
        where
            $($first: LentAccess<Family: Counter> + 's,)+
            $($second: LentAccess<Family: Counter> + 's,)+
        {
            type Fut = impl Future<Output = u32>;
            fn start(
                self,
                accesses: (($($first,)+), ($($second,)+)),
                _: Emitter<'s, Infallible>,
            ) -> Self::Fut {
                async move {
                    accesses.with(|(first, second)| {
                        first.with(|($(mut $first,)+)| {
                            second.with(|($(mut $second,)+)| {
                                0 $(+ $first.add(1))+ $(+ $second.add(1))+
                            })
                        })
                    })
                }
            }
        }

        #[test]
        fn an_executor_lends_every_receiver() {
            let mut totals = ($(Total(one!($first)),)+ $(Total(one!($second)),)+);
            assert_eq!(totals.sync_add_up_unpin(), 12);
        }

        #[test]
        fn a_call_keeps_a_flag_for_every_receiver() {
            let ($($first,)+ $($second,)+) = ($(Total(one!($first)),)+ $(Total(one!($second)),)+);
            let _ = ($($first,)+ $($second,)+);
            let mut call = pin!(Call::<Totals, _>::new(AddUp::new()));
            let none = ($({ let _ = one!($first); None },)+ $({ let _ = one!($second); None },)+);
            assert!(matches!(
                call.as_mut().poll_with_views(none, &mut Context::from_waker(Waker::noop())),
                CallPoll::NeedsView
            ));
            let needed = call.views_needed();
            assert!(needed.0);
            assert!(!needed.11);
        }

        #[test]
        fn views_are_constructed_for_every_receiver() {
            $(let mut $first = Total(one!($first));)+
            $(let mut $second = Total(one!($second));)+
            let mut driven = pin!(drive(
                (
                    $(Constructed::new(Pin::new(&mut $first)),)+
                    $(Constructed::new(Pin::new(&mut $second)),)+
                ),
                AddUp::new(),
            ));
            assert_eq!(
                driven.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(12)
            );
        }

        #[test]
        fn views_are_composed_for_every_receiver() {
            $(let $first = Mutex::new(Total(one!($first)));)+
            $(let $second = Mutex::new(Total(one!($second)));)+
            let mut driven = pin!(drive_composed(
                (
                    $(LockAcquisition::new(&$first),)+
                    $(LockAcquisition::new(&$second),)+
                ),
                ($({ let _ = &$first; Guarded },)+ $({ let _ = &$second; Guarded },)+),
                AddUp::new(),
            ));
            assert_eq!(
                driven.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(12)
            );
        }

        #[test]
        fn a_lent_call_lends_every_receiver_to_one_closure() {
            $(let mut $first = Total::default();)+
            $(let mut $second = Total::default();)+
            $(let mut $first = pin!(Pin::new(&mut $first).view());)+
            $(let mut $second = pin!(Pin::new(&mut $second).view());)+
            let mut call = pin!(LentCall::<Totals, _>::new(StepAll::new()));
            assert!(matches!(
                call.as_mut().poll_with_views(
                    ($($first.as_mut(),)+ $($second.as_mut(),)+),
                    &mut Context::from_waker(Waker::noop()),
                ),
                Poll::Ready(ControlFlow::Break(12))
            ));
        }

        #[test]
        fn each_inner_tuple_opens_in_its_own_closure() {
            $(let mut $first = Total::default();)+
            $(let mut $second = Total::default();)+
            $(let mut $first = pin!(Pin::new(&mut $first).view());)+
            $(let mut $second = pin!(Pin::new(&mut $second).view());)+
            let mut call = pin!(LentCall::<(Half, Half), _>::new(StepHalves));
            assert!(matches!(
                call.as_mut().poll_with_views(
                    (($($first.as_mut(),)+), ($($second.as_mut(),)+)),
                    &mut Context::from_waker(Waker::noop()),
                ),
                Poll::Ready(ControlFlow::Break(12))
            ));
        }
    };
}
largest_tuple!(a b c d e f; g h i j k l);

macro_rules! beyond_a_tuple {
    ($x:ident; $($y:ident)+; $($z:ident)+; $($rest:ident)+) => {
        type Beyond = (
            direct_total!($x),
            ($(direct_total!($y),)+ ($(direct_total!($z),)+)),
            $(direct_total!($rest),)+
        );

        /// More accesses than a tuple has elements, each level in its closure.
        struct StepNested;
        impl<R: Receivers> Operation<R> for StepNested {
            type Output = u32;
            type Yield = Infallible;
        }
        #[allow(non_camel_case_types)]
        impl<'s, $x, $($y,)+ $($z,)+ $($rest,)+>
            Start<'s, ($x, ($($y,)+ ($($z,)+)), $($rest,)+)> for StepNested
        where
            $x: LentAccess<Family: Counter> + 's,
            $($y: LentAccess<Family: Counter> + 's,)+
            $($z: LentAccess<Family: Counter> + 's,)+
            $($rest: LentAccess<Family: Counter> + 's,)+
        {
            type Fut = impl Future<Output = u32>;
            fn start(
                self,
                accesses: ($x, ($($y,)+ ($($z,)+)), $($rest,)+),
                _: Emitter<'s, Infallible>,
            ) -> Self::Fut {
                async move {
                    accesses.with(|(mut $x, group, $(mut $rest,)+)| {
                        group.with(|($(mut $y,)+ inner)| {
                            inner.with(|($(mut $z,)+)| {
                                $x.add(1) $(+ $y.add(1))+ $(+ $z.add(1))+ $(+ $rest.add(1))+
                            })
                        })
                    })
                }
            }
        }

        #[test]
        fn nesting_reaches_more_accesses_than_a_tuple_has_elements() {
            let mut $x = Total::default();
            $(let mut $y = Total::default();)+
            $(let mut $z = Total::default();)+
            $(let mut $rest = Total::default();)+
            let mut $x = pin!(Pin::new(&mut $x).view());
            $(let mut $y = pin!(Pin::new(&mut $y).view());)+
            $(let mut $z = pin!(Pin::new(&mut $z).view());)+
            $(let mut $rest = pin!(Pin::new(&mut $rest).view());)+
            let mut call = pin!(LentCall::<Beyond, _>::new(StepNested));
            assert!(matches!(
                call.as_mut().poll_with_views(
                    (
                        $x.as_mut(),
                        ($($y.as_mut(),)+ ($($z.as_mut(),)+)),
                        $($rest.as_mut(),)+
                    ),
                    &mut Context::from_waker(Waker::noop()),
                ),
                Poll::Ready(ControlFlow::Break(24))
            ));
        }
    };
}
beyond_a_tuple!(x1; y1 y2 y3 y4 y5 y6 y7 y8 y9 y10 y11; z1 z2; x3 x4 x5 x6 x7 x8 x9 x10 x11 x12);

/// An unopened access still has its methods, one use at a time.
#[call]
async fn with_a_group(
    first: &mut impl LentAccess<Family: Counter>,
    second: &mut impl LentAccess<Family: Counter>,
    third: &mut impl LentAccess<Family: Counter>,
) -> u32 {
    (first, (second, third))
        .with(|(mut first, (second, third))| first.add(1) + second.add(1) + third.add(1))
}

#[test]
fn an_unopened_access_keeps_its_methods() {
    let [mut a, mut b, mut c] = [(); 3].map(|()| Total::default());
    let mut a = pin!(Pin::new(&mut a).view());
    let mut b = pin!(Pin::new(&mut b).view());
    let mut c = pin!(Pin::new(&mut c).view());
    type Three = (Direct<Total>, Direct<Total>, Direct<Total>);
    let mut call = pin!(LentCall::<Three, _>::new(WithAGroup::new()));
    assert!(matches!(
        call.as_mut().poll_with_views(
            (a.as_mut(), b.as_mut(), c.as_mut()),
            &mut Context::from_waker(Waker::noop()),
        ),
        Poll::Ready(ControlFlow::Break(3))
    ));
}

/// Tuples nest on either side and to any depth, and an access can be used
/// again after the closure that held it.
#[call]
async fn in_groups<F, G, H, I>(
    first: &mut impl LentAccess<Family = F>,
    second: &mut impl LentAccess<Family = G>,
    third: &mut impl LentAccess<Family = H>,
    fourth: &mut impl LentAccess<Family = I>,
) -> [u32; 3]
where
    F: Counter,
    G: Counter,
    H: Counter,
    I: Counter,
{
    let right =
        (&mut *first, (&mut *second, (&mut *third, &mut *fourth))).with(|(mut first, rest)| {
            rest.with(|(mut second, rest)| {
                rest.with(|(mut third, mut fourth)| {
                    first.add(1) + second.add(2) + third.add(3) + fourth.add(4)
                })
            })
        });
    let left =
        (((&mut *first, &mut *second), &mut *third), &mut *fourth).with(|(rest, mut fourth)| {
            rest.with(|(rest, mut third)| {
                rest.with(|(mut first, mut second)| {
                    first.add(1) + second.add(1) + third.add(1) + fourth.add(1)
                })
            })
        });
    let both = ((&mut *first, &mut *second), (&mut *third, &mut *fourth)).with(|(left, right)| {
        left.with(|(_, mut second)| right.with(|(mut third, _)| second.add(1) + third.add(1)))
    });
    [right, left, both]
}

#[test]
fn tuples_nest_in_any_shape() {
    type Four = (Direct<Total>, Direct<Total>, Direct<Total>, Direct<Total>);
    let [mut a, mut b, mut c, mut d] = [(); 4].map(|()| Total::default());
    let mut a = pin!(Pin::new(&mut a).view());
    let mut b = pin!(Pin::new(&mut b).view());
    let mut c = pin!(Pin::new(&mut c).view());
    let mut d = pin!(Pin::new(&mut d).view());
    let mut call = pin!(LentCall::<Four, _>::new(InGroups::new()));
    assert!(matches!(
        call.as_mut().poll_with_views(
            (a.as_mut(), b.as_mut(), c.as_mut(), d.as_mut()),
            &mut Context::from_waker(Waker::noop()),
        ),
        Poll::Ready(ControlFlow::Break([10, 14, 9]))
    ));
}

struct Farewell<'x, A: LentAccess<Family: Counter>, B: LentAccess<Family: Counter>> {
    first: &'x mut A,
    group: (&'x mut B, &'x mut B),
    seen: Rc<Cell<Option<Option<u32>>>>,
}
impl<A, B> Drop for Farewell<'_, A, B>
where
    A: LentAccess<Family: Counter>,
    B: LentAccess<Family: Counter>,
{
    fn drop(&mut self) {
        let last = (&mut *self.first, (&mut *self.group.0, &mut *self.group.1))
            .try_with(|(mut first, group)| {
                let first = first.add(1);
                group.try_with(|(mut second, mut third)| first + second.add(1) + third.add(1))
            })
            .flatten();
        self.seen.set(Some(last));
    }
}
struct SayFarewell {
    seen: Rc<Cell<Option<Option<u32>>>>,
    wait: bool,
}
type Three = (Direct<Total>, (Direct<Total>, Direct<Total>));
impl<R: Receivers> Operation<R> for SayFarewell {
    type Output = ();
    type Yield = Infallible;
}
impl<'s, A, B> Start<'s, (A, (B, B))> for SayFarewell
where
    A: LentAccess<Family: Counter> + 's,
    B: LentAccess<Family: Counter> + 's,
{
    type Fut = impl Future<Output = ()>;
    fn start(
        self,
        (mut first, (mut second, mut third)): (A, (B, B)),
        _: Emitter<'s, Infallible>,
    ) -> Self::Fut {
        async move {
            let farewell = Farewell {
                first: &mut first,
                group: (&mut second, &mut third),
                seen: self.seen,
            };
            if self.wait {
                pending::<()>().await;
            }
            drop(farewell);
        }
    }
}

#[test]
fn a_nested_tuple_has_its_views_only_in_a_poll() {
    for (wait, expected) in [(false, Some(3)), (true, None)] {
        let seen = Rc::new(Cell::new(None));
        let [mut a, mut b, mut c] = [(); 3].map(|()| Total::default());
        let mut call = Box::pin(LentCall::<Three, _>::new(SayFarewell {
            seen: seen.clone(),
            wait,
        }));
        let mut a = pin!(Pin::new(&mut a).view());
        let mut b = pin!(Pin::new(&mut b).view());
        let mut c = pin!(Pin::new(&mut c).view());
        let _ = call.as_mut().poll_with_views(
            (a.as_mut(), (b.as_mut(), c.as_mut())),
            &mut Context::from_waker(Waker::noop()),
        );
        drop(call);
        assert_eq!(seen.get(), Some(expected));
    }
}
