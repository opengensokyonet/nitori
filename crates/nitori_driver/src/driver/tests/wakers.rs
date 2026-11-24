//! Wakers: a component's waker of a node that ended does not wake the
//! node added again under its key; an old outer waker woken first does
//! not take the new outer waker's notification.
use alloc::vec;

use frunk_core::{HList, hlist, hlist_pat};

use super::{
    core,
    fixture::{Ctl, Leaf, Level, QT, Step, Top, clock, notes, take_wakers},
    outer, poll,
};
use crate::{
    driver::Driver,
    lend::tests::fixtures::{lend, lend_in},
    need::{WakerOf, Wakes},
};

#[test]
fn an_old_component_waker_does_not_wake_a_new_node() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    let c = core(&mut d);
    let n = c.add::<Leaf, _>(hlist![5], vec![], Level::Low).unwrap();
    let old = lend!(
        &mut c.root,
        Leaf,
        HList![WakerOf<Ctl>],
        hlist![5u32],
        |hlist_pat![w]| w
    )
    .expect("the leaf is there");
    c.end_for::<Leaf, _>(n, &3);
    c.after_lend();
    c.add::<Leaf, _>(hlist![5], vec![], Level::Low).unwrap();
    let new = lend!(
        &mut c.root,
        Leaf,
        HList![WakerOf<Ctl>],
        hlist![5u32],
        |hlist_pat![w]| w
    )
    .expect("the leaf is there");
    assert_ne!(old, new);
    // the calls of both leaves ran their one step and wait
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    notes();
    let c = core(&mut d);
    let wake = |c: &mut super::Core<QT>, waker| {
        lend_in!(
            &mut c.root,
            Top,
            HList![Wakes<Level>],
            hlist![],
            &mut c.cx,
            |hlist_pat![wakes]| wakes.wake(&waker, Level::High)
        )
        .expect("the top is there");
        c.after_lend();
    };
    wake(c, old);
    let _ = poll(&mut d, &w);
    assert!(notes().is_empty());
    wake(core(&mut d), new);
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["ctl 5"]);
}

#[test]
fn a_new_outer_waker_polls_every_call_again() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    core(&mut d)
        .add::<Leaf, _>(
            hlist![1],
            vec![Step::Hand(Level::Low), Step::Hand(Level::Low)],
            Level::Low,
        )
        .unwrap();
    let (_, w1) = outer();
    let _ = poll(&mut d, &w1);
    let (count2, w2) = outer();
    let _ = poll(&mut d, &w2);
    assert_eq!(notes(), ["call 1 #1", "call 1 #2"]);
    // the call's new waker wakes the new outer waker
    take_wakers(1).remove(0).wake();
    assert_eq!(count2.0.load(core::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn an_old_waker_woken_first_leaves_the_new_outer_waker_to_be_woken() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    core(&mut d)
        .add::<Leaf, _>(
            hlist![1],
            vec![Step::Hand(Level::Low), Step::Hand(Level::Low)],
            Level::Low,
        )
        .unwrap();
    let (count1, w1) = outer();
    let _ = poll(&mut d, &w1);
    let (count2, w2) = outer();
    let _ = poll(&mut d, &w2);
    notes();
    // a resource keeps the waker made with the old outer waker, and the
    // new one
    let mut handed = take_wakers(1);
    let new = handed.remove(0);
    let old = handed.remove(0);
    let before = count1.0.load(core::sync::atomic::Ordering::SeqCst);
    old.wake_by_ref();
    assert_eq!(
        count1.0.load(core::sync::atomic::Ordering::SeqCst),
        before + 1
    );
    new.wake_by_ref();
    assert_eq!(count2.0.load(core::sync::atomic::Ordering::SeqCst), 1);
    let _ = poll(&mut d, &w2);
    assert_eq!(notes(), ["call 1 #3"]);
}
