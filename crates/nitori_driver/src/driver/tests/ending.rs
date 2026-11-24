//! Ending: dropped, every node ends after the nodes below it, for
//! `Dropped`; after a panic through a poll, nothing is ended.
extern crate std;

use alloc::{string::String, vec, vec::Vec};

use frunk_core::hlist;

use super::{
    core,
    fixture::{Leaf, Level, QT, Step, clock, notes},
    outer, poll,
};
use crate::{
    call::tests::{Q1, Q2},
    driver::Driver,
    lend::tests::{
        fixtures::{LinkId, LinkPath, LogicalPath, PathKey, TestClock},
        parts::take_log,
    },
};

#[test]
fn dropped_ends_the_nodes_below_first() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    for k in [1, 2] {
        core(&mut d)
            .add::<Leaf, _>(hlist![k], vec![], Level::Low)
            .unwrap();
    }
    drop(d);
    assert_eq!(
        notes(),
        ["end 1 Dropped", "end 2 Dropped", "end top Dropped"]
    );
}

#[test]
fn ended_for_a_reason_the_nodes_below_end_for_above() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    let n = core(&mut d)
        .add::<Leaf, _>(hlist![4], vec![], Level::Low)
        .unwrap();
    core(&mut d).end_for::<Leaf, _>(n, &9);
    assert_eq!(notes(), ["end 4 Reason(9)"]);
    drop(d);
    assert_eq!(notes(), ["end top Dropped"]);
}

#[test]
fn after_a_panic_the_drop_ends_nothing() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    core(&mut d)
        .add::<Leaf, _>(hlist![1], vec![Step::Panic], Level::Low)
        .unwrap();
    let (_, w) = outer();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poll(&mut d, &w)));
    assert!(unwound.is_err());
    notes();
    assert!(poll(&mut d, &w).is_ready());
    drop(d);
    assert!(notes().is_empty());
}

/// The planners' ends noted on the structures of the lending's checks.
fn ends() -> Vec<String> {
    take_log()
        .into_iter()
        .filter(|l| l.starts_with("end "))
        .collect()
}

#[test]
fn dropped_on_both_structures() {
    let args = || {
        hlist![
            crate::lend::tests::parts::CoreArgs,
            crate::lend::tests::parts::LossArgs { window: 4 },
            crate::lend::tests::parts::WatchArgs
        ]
    };
    let clock = || TestClock {
        last: core::time::Duration::from_secs(10),
    };
    take_log();
    let mut d = Driver::<Q1>::new(clock(), (), args(), 0);
    let c = core(&mut d);
    c.add::<LogicalPath, _>(hlist![PathKey(10)], (), 0).unwrap();
    for l in [1, 2] {
        c.add::<LinkPath, _>(hlist![PathKey(10), LinkId(l)], 5, 0)
            .unwrap();
    }
    drop(d);
    let one = ends();
    take_log();
    let mut d = Driver::<Q2>::new(clock(), (), args(), 0);
    let c = core(&mut d);
    for l in [1, 2] {
        c.add::<LinkPath, _>(hlist![LinkId(l)], 5, 0).unwrap();
    }
    drop(d);
    let two = ends();
    crate::lend::tests::fixtures::same_on_both(
        one,
        two,
        vec![
            String::from("end Dropped pn=105 rx=4"),
            String::from("end Dropped pn=205 rx=4"),
        ],
    );
}
