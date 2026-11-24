//! Node numbers: one given out again only when nothing refers to the
//! node's head, the standard waker of a call that ended wakes nothing, and
//! a call that settled in the batch its node ended in does not wake.
use alloc::vec;

use frunk_core::hlist;

use super::{
    core,
    fixture::{Ask, Leaf, Level, QT, Step, clock, notes, take_wakers},
    outer, poll,
};
use crate::driver::Driver;

/// c: a leaf hands out its call's standard waker and ends; while the
/// waker lives a new node gets a new number; woken, the old waker runs
/// nothing; dropped, the number goes to the next node.
#[test]
fn a_number_is_given_again_once_the_old_waker_is_dropped() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    let first = core(&mut d)
        .add::<Leaf, _>(hlist![1], vec![Step::Hand(Level::Low)], Level::Low)
        .unwrap();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    let old = take_wakers(1).remove(0);
    core(&mut d).end_for::<Leaf, _>(first, &3);
    core(&mut d).after_lend();
    notes();
    // the old waker lives: its node's number is not given out
    let second = core(&mut d)
        .add::<Leaf, _>(hlist![2], vec![], Level::Low)
        .unwrap();
    assert_ne!(second, first);
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 2 #1"]);
    // woken, it wakes nothing
    old.wake_by_ref();
    let _ = poll(&mut d, &w);
    assert!(notes().is_empty());
    old.wake();
    let _ = poll(&mut d, &w);
    assert!(notes().is_empty());
    // dropped (`wake` took it), the number is free
    let third = core(&mut d)
        .add::<Leaf, _>(hlist![3], vec![], Level::Low)
        .unwrap();
    assert_eq!(third, first);
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 3 #1"]);
}

/// d: a call puts in the request that ends its own node, and settles: the
/// node ends in the same batch, and the call is not woken as settled.
#[test]
fn a_call_that_settled_is_not_woken_once_its_node_ended() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    core(&mut d)
        .add::<Leaf, _>(
            hlist![1],
            vec![Step::Settle(Ask::Leaf(1, 200), Level::High)],
            Level::Low,
        )
        .unwrap();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    assert_eq!(
        notes(),
        ["call 1 #1", "leaf 1 asked 200", "end 1 Reason(200)"]
    );
    // the number is given again, and what runs is the new call only
    core(&mut d)
        .add::<Leaf, _>(hlist![3], vec![], Level::Low)
        .unwrap();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 3 #1"]);
}
