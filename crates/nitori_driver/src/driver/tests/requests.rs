//! Requests: handled after the lending that put them in, in the order
//! they were put in across the kinds; one for a node that is not there is
//! dropped.
use alloc::vec;

use frunk_core::hlist;

use super::{
    core,
    fixture::{Ask, Leaf, Level, QT, Step, TopAsk, clock, notes},
    outer, poll,
};
use crate::driver::Driver;

#[test]
fn in_the_order_put_in_across_the_kinds() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    core(&mut d)
        .add::<Leaf, _>(
            hlist![1],
            vec![Step::Ask(vec![
                Ask::Leaf(1, 1),
                Ask::Top(TopAsk::Note(2)),
                Ask::Leaf(8, 3),
                Ask::Top(TopAsk::Note(4)),
                Ask::Leaf(1, 5),
            ])],
            Level::Low,
        )
        .unwrap();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    let c = core(&mut d);
    // the top is kind 0, a leaf kind 1; leaf 8 is not there
    assert_eq!(
        c.trace,
        [
            (0, 1, true),
            (1, 0, true),
            (2, 1, false),
            (3, 0, true),
            (4, 1, true)
        ]
    );
}
