//! Requests handled by their kinds' handlers with edit handles: adding,
//! ending, replacing and moving children, a node ending itself, the root
//! ending the driver, and the order of what a handler puts in.
use alloc::{vec, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::{
    core,
    fixture::{Ask, Leaf, Level, QT, Step, Tip, Top, TopAsk, Twig, clock, notes},
    outer, poll,
};
use crate::{
    driver::{Driver, OperateError},
    need::{Requests, WakerOf, Wakes},
    request::NodeRequest,
    structure::KeyAddress,
};

/// A driver with the leaves `leaves`, each with nothing to do, and the
/// notes dropped.
fn with_leaves(leaves: &[u32]) -> Driver<QT> {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    for &k in leaves {
        core(&mut d)
            .add::<Leaf, _>(hlist![k], vec![], Level::Low)
            .expect("the key is free");
    }
    d
}
/// The address of the leaf `k`.
fn leaf_at(k: u32) -> KeyAddress<Top, Leaf> {
    KeyAddress::<Top, Top>::child::<Leaf, _>(k)
}
/// The address of the twig `t` below the leaf `l`.
fn twig_at(l: u32, t: u32) -> KeyAddress<Top, Twig> {
    KeyAddress::<Top, Top>::below::<Leaf, _, Twig>(l, KeyAddress::<Leaf, Leaf>::child::<Twig, _>(t))
}
/// Ask the top `asks`, in order, from outside.
fn ask(d: &mut Driver<QT>, asks: Vec<TopAsk>) {
    let out = d.operate::<Top, HList![Requests<NodeRequest<Top, Top>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![tops]| {
            for a in asks {
                tops.push(NodeRequest::new(KeyAddress::here(), a));
            }
        },
    );
    assert_eq!(out, Ok(Some(())));
}
/// Ask the leaf `k` the codes `codes`, in order, from outside.
fn ask_leaf(d: &mut Driver<QT>, k: u32, codes: &[u8]) {
    let out = d.operate::<Top, HList![Requests<NodeRequest<Top, Leaf>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![leaves]| {
            for &c in codes {
                leaves.push(NodeRequest::new(leaf_at(k), c));
            }
        },
    );
    assert_eq!(out, Ok(Some(())));
}

#[test]
fn ended_twice_the_second_request_is_dropped() {
    let mut d = with_leaves(&[1]);
    ask_leaf(&mut d, 1, &[200, 201]);
    assert_eq!(notes(), ["leaf 1 asked 200", "end 1 Reason(200)"]);
    let trace: Vec<bool> = core(&mut d).trace.iter().map(|t| t.2).collect();
    assert_eq!(trace, [true, false]);
}

#[test]
fn adding_under_a_key_kept_is_refused() {
    let mut d = with_leaves(&[]);
    ask(
        &mut d,
        vec![
            TopAsk::Add(1, vec![], Level::Low),
            TopAsk::Add(1, vec![], Level::Low),
        ],
    );
    assert_eq!(
        notes(),
        [
            "top Add(1, [], Low)",
            "top: Ok(())",
            "top Add(1, [], Low)",
            "top: Err(OccupiedError)"
        ]
    );
}

#[test]
fn a_node_added_runs_at_the_priority_it_was_added_with() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    let add = TopAsk::Add(5, vec![], Level::High);
    core(&mut d)
        .add::<Leaf, _>(
            hlist![1],
            vec![Step::Settle(Ask::Top(add), Level::Low)],
            Level::Low,
        )
        .unwrap();
    core(&mut d)
        .add::<Leaf, _>(hlist![2], vec![], Level::Low)
        .unwrap();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    // the request is handled before any other call; the new leaf's call
    // runs first, at the priority it was added with; the call that
    // settled low, after the low one queued before
    assert_eq!(
        notes(),
        [
            "call 1 #1",
            "top Add(5, [], High)",
            "top: Ok(())",
            "call 5 #1",
            "call 2 #1",
            "call 1 settled",
            "call 1 #2"
        ]
    );
}

#[test]
fn moved_a_twig_is_the_same_node_under_another_leaf() {
    let mut d = with_leaves(&[1, 2]);
    ask(&mut d, vec![TopAsk::AddTwig(1, 7, false)]);
    let waker = d
        .operate::<Twig, HList![WakerOf<Tip>], _, _, _>(&twig_at(1, 7), |hlist_pat![w]| w)
        .unwrap()
        .unwrap();
    notes();
    ask(
        &mut d,
        vec![
            TopAsk::Move {
                twig: 7,
                from: 1,
                to: 2,
            },
            TopAsk::Move {
                twig: 7,
                from: 1,
                to: 2,
            },
            TopAsk::Move {
                twig: 7,
                from: 2,
                to: 3,
            },
        ],
    );
    let outcomes: Vec<_> = notes()
        .into_iter()
        .filter(|n| n.starts_with("top:"))
        .collect();
    assert_eq!(
        outcomes,
        [
            "top: Ok(())",
            "top: Err(NoSource)",
            "top: Err(NoDestination)"
        ]
    );
    let at = |d: &mut Driver<QT>, l| {
        d.operate::<Twig, HList![WakerOf<Tip>], _, _, _>(&twig_at(l, 7), |hlist_pat![w]| w)
            .unwrap()
    };
    assert_eq!(at(&mut d, 1), None);
    // the same node: its waker from before wakes it, lent where it is now
    assert_eq!(at(&mut d, 2), Some(waker));
    d.operate::<Top, HList![Wakes<Level>], _, _, _>(&KeyAddress::here(), |hlist_pat![wakes]| {
        wakes.wake(&waker, Level::Low)
    })
    .unwrap();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    assert!(notes().iter().any(|n| n == "tip 7 under 2"));
    // a twig under the key at the destination: nothing moves
    ask(
        &mut d,
        vec![
            TopAsk::AddTwig(1, 7, false),
            TopAsk::Move {
                twig: 7,
                from: 1,
                to: 2,
            },
        ],
    );
    assert_eq!(
        notes().last().map(|s| s.as_str()),
        Some("top: Err(Occupied { source: OccupiedError })")
    );
    // it ends with the leaf it moved to
    ask(&mut d, vec![TopAsk::End(2, 3)]);
    assert_eq!(
        notes(),
        [
            "top End(2, 3)",
            "end twig 7 Above",
            "end 2 Reason(3)",
            "top: Ok(())"
        ]
    );
    assert_eq!(at(&mut d, 1).map(|_| ()), Some(()));
}

#[test]
fn replaced_a_leaf_ends_then_starts_again() {
    let mut d = with_leaves(&[1]);
    ask(&mut d, vec![TopAsk::AddTwig(1, 4, false)]);
    notes();
    ask(
        &mut d,
        vec![
            TopAsk::Replace(1, vec![], 9, Level::High),
            TopAsk::Replace(6, vec![], 9, Level::High),
        ],
    );
    assert_eq!(
        notes(),
        [
            "top Replace(1, [], 9, High)",
            "end twig 4 Above",
            "end 1 Reason(9)",
            "top: Ok(())",
            "top Replace(6, [], 9, High)",
            "top: Err(NoChildError)"
        ]
    );
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 1 #1"]);
}

#[test]
fn a_call_for_any_structure_ends_its_own_node_by_its_address() {
    let mut d = with_leaves(&[1]);
    ask(&mut d, vec![TopAsk::AddTwig(1, 8, true)]);
    notes();
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 1 #1", "tidy 8", "end twig 8 Reason(())"]);
    let gone = d.operate::<Twig, HList![WakerOf<Tip>], _, _, _>(&twig_at(1, 8), |hlist_pat![w]| w);
    assert_eq!(gone, Ok(None));
}

#[test]
fn what_a_handler_puts_in_comes_after_what_was_put_in_before() {
    let mut d = with_leaves(&[1]);
    ask_leaf(&mut d, 1, &[150, 2]);
    assert_eq!(
        notes(),
        [
            "leaf 1 asked 150",
            "leaf 1 asked 2",
            "leaf 1 asked 50",
            "top Note(150)"
        ]
    );
}

#[test]
fn the_root_ended_the_driver_is_done() {
    let mut d = with_leaves(&[1]);
    ask(&mut d, vec![TopAsk::EndSelf, TopAsk::Note(1)]);
    assert_eq!(
        notes(),
        ["top EndSelf", "end 1 Above", "end top Reason(())"]
    );
    let again = d.operate::<Top, HList![], _, _, _>(&KeyAddress::here(), |_| ());
    assert_eq!(again, Err(OperateError::Ended));
    let (_, w) = outer();
    assert!(poll(&mut d, &w).is_ready());
    drop(d);
    assert!(notes().is_empty());
}
