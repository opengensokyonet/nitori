//! Priorities: what is woken while another item is handled, woken more
//! than once, woken by a standard waker at the call's priority, settled,
//! woken by itself, and a call that never stops waking itself.
use alloc::{vec, vec::Vec};

use frunk_core::hlist;

use super::{
    core,
    fixture::{Ask, Level, QT, Step, clock, notes, take_wakers},
    outer, poll,
};
use crate::driver::Driver;

/// A driver with the leaves `leaves`, each added at `Low` in order
/// with its script, and the notes of adding them dropped.
fn with_leaves(leaves: Vec<(u32, Vec<Step>)>) -> Driver<QT> {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    for (k, script) in leaves {
        core(&mut d)
            .add::<super::fixture::Leaf, _>(hlist![k], script, Level::Low)
            .expect("the key is free");
        core(&mut d).after_lend();
    }
    d
}

#[test]
fn a_higher_component_woken_meanwhile_is_next() {
    let mut d = with_leaves(vec![
        (1, vec![Step::Wake(vec![Level::High])]),
        (2, vec![Step::Pend]),
    ]);
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 1 #1", "ctl 1", "call 2 #1"]);
}

#[test]
fn woken_twice_is_handled_once_at_the_highest() {
    let mut d = with_leaves(vec![
        (
            1,
            vec![Step::Wake(vec![Level::Low, Level::High, Level::Mid])],
        ),
        (2, vec![Step::Pend]),
    ]);
    // the first call runs first; the second waits at `Mid`, so the
    // component comes before it only at the highest of its wakes
    raise(&mut d, 1, Level::High);
    raise(&mut d, 2, Level::Mid);
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 1 #1", "ctl 1", "call 2 #1"]);
}

/// Wake the call of the node `node` at `p` in the queue.
fn raise(d: &mut Driver<QT>, node: u32, p: Level) {
    core(d).ready.wake(
        crate::driver::ready::Item {
            node,
            target: crate::driver::ready::Target::Call(0),
        },
        p,
    );
}

#[test]
fn a_standard_waker_wakes_at_the_priority_set() {
    let mut d = with_leaves(vec![
        (1, vec![Step::Hand(Level::Low)]),
        (2, vec![Step::Hand(Level::High)]),
    ]);
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 1 #1", "call 2 #1"]);
    // woken outside, the first one first: the priority set is read when
    // the wake is handled
    take_wakers(1).remove(0).wake();
    take_wakers(2).remove(0).wake();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 2 #2", "call 1 #2"]);
}

#[test]
fn settling_goes_on_after_the_requests_by_priority() {
    // settling high, the call goes on before the low one still queued
    let mut d = with_leaves(vec![
        (1, vec![Step::Settle(Ask::Leaf(2, 7), Level::High)]),
        (2, vec![Step::Pend]),
    ]);
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    // the request is handled before the call goes on
    assert_eq!(
        notes(),
        [
            "call 1 #1",
            "leaf 2 asked 7",
            "call 1 settled",
            "call 1 #2",
            "call 2 #1"
        ]
    );
    assert_eq!(core(&mut d).trace, [(0, 1, true)]);
    // settling low, it waits behind the low one queued before
    let mut d = with_leaves(vec![
        (1, vec![Step::Settle(Ask::Leaf(9, 7), Level::Low)]),
        (2, vec![Step::Pend]),
    ]);
    let _ = poll(&mut d, &w);
    assert_eq!(
        notes(),
        ["call 1 #1", "call 2 #1", "call 1 settled", "call 1 #2"]
    );
    assert_eq!(core(&mut d).trace, [(0, 1, false)]);
}

#[test]
fn woken_by_itself_it_is_queued_again() {
    let mut d = with_leaves(vec![
        (1, vec![Step::WakeSelf, Step::Pend]),
        (2, vec![Step::Pend]),
    ]);
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    // the call's own wake queues it behind the other call; the component
    // wakes itself once more when woken (one `WakeSelf` in its script)
    assert_eq!(notes(), ["call 1 #1", "call 2 #1", "call 1 #2"]);
    let mut d = with_leaves(vec![(
        1,
        vec![Step::WakeSelf, Step::Wake(vec![Level::Low])],
    )]);
    let _ = poll(&mut d, &w);
    // the component, woken, wakes itself once more
    assert_eq!(notes(), ["call 1 #1", "call 1 #2", "ctl 1", "ctl 1"]);
}

#[test]
fn a_spinning_call_gives_way_at_the_budget() {
    let mut d = with_leaves(vec![(1, vec![Step::Spin]), (2, vec![Step::Pend])]);
    let (count, w) = outer();
    assert!(poll(&mut d, &w).is_pending());
    let first = notes();
    assert_eq!(first.len(), crate::driver::ITEMS_PER_POLL);
    assert_eq!(first[..2], ["call 1 #1", "call 2 #1"]);
    assert!(count.0.load(core::sync::atomic::Ordering::SeqCst) >= 1);
    assert!(poll(&mut d, &w).is_pending());
    let second = notes();
    assert_eq!(second.len(), crate::driver::ITEMS_PER_POLL);
    assert!(second.iter().all(|n| n.starts_with("call 1 #")));
}
