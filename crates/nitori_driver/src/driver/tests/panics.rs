//! A panic in a request handler (through `operate`) and in a component's
//! operation (through a poll): the driver then refuses `operate`, is
//! ready, and ends no component when dropped.
extern crate std;

use alloc::vec;
use std::panic::{AssertUnwindSafe, catch_unwind};

use frunk_core::{HList, hlist, hlist_pat};

use super::{
    core,
    fixture::{Leaf, Level, QT, Step, Top, TopAsk, clock, notes},
    outer, poll,
};
use crate::{
    driver::{Driver, OperateError},
    need::Requests,
    request::NodeRequest,
    structure::KeyAddress,
};

/// What a driver that panicked does: refuse, be ready, end nothing.
fn assert_dead(mut d: Driver<QT>) {
    let out = d.operate::<Top, HList![], _, _, _>(&KeyAddress::here(), |_| {
        unreachable!("nothing is lent after a panic")
    });
    assert_eq!(out, Err(OperateError::Panicked));
    let (_, w) = outer();
    assert!(poll(&mut d, &w).is_ready());
    notes();
    drop(d);
    assert!(notes().is_empty());
}

#[test]
fn a_handler_that_panics_in_operate_leaves_the_driver_refusing() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    core(&mut d)
        .add::<Leaf, _>(hlist![1], vec![], Level::Low)
        .unwrap();
    let unwound = catch_unwind(AssertUnwindSafe(|| {
        d.operate::<Top, HList![Requests<NodeRequest<Top, Top>>], _, _, _>(
            &KeyAddress::here(),
            |hlist_pat![tops]| {
                tops.push(NodeRequest::new(KeyAddress::here(), TopAsk::Panic));
            },
        )
    }));
    assert!(unwound.is_err());
    assert_eq!(notes(), ["top Panic"]);
    assert_dead(d);
}

#[test]
fn a_component_operation_that_panics_in_a_poll_leaves_the_driver_refusing() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    // the call wakes its leaf's component, whose wake panics
    core(&mut d)
        .add::<Leaf, _>(
            hlist![1],
            vec![Step::Wake(vec![Level::Low]), Step::PanicOnWake],
            Level::Low,
        )
        .unwrap();
    let (_, w) = outer();
    let unwound = catch_unwind(AssertUnwindSafe(|| poll(&mut d, &w)));
    assert!(unwound.is_err());
    assert_eq!(notes(), ["call 1 #1", "ctl 1"]);
    assert_dead(d);
}
