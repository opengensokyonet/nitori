//! The shape of the result of `Under` follows from the structure.
use alloc::vec::Vec;

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::need::{At, KeyedView, Mut, Ref, SingleView, Under};

#[test]
fn keyed_by_one_key_and_by_two() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LogicalPath,
        HList![Under<ConnCore, Mut<SentRecords>>, Under<ConnCore, Mut<LinkCore>>],
        hlist![PathKey(10)],
        |hlist_pat![mut sent, mut links]| {
            let _: &KeyedView<PathKey, &mut SentRecords> = &sent;
            let _: &KeyedView<(PathKey, LinkId), &mut LinkCore> = &links;
            for (_, s) in sent.iter_mut() {
                s.acked += 1;
            }
            links.get_mut(&(PathKey(20), LinkId(3))).unwrap().rx = 9;
            (
                sent.keys().copied().collect::<Vec<_>>(),
                links.keys().copied().collect::<Vec<_>>(),
            )
        }
    );
    assert_eq!(
        got,
        Some((
            alloc::vec![PathKey(10), PathKey(20)],
            alloc::vec![
                (PathKey(10), LinkId(1)),
                (PathKey(10), LinkId(2)),
                (PathKey(20), LinkId(3))
            ]
        ))
    );
    let rx = lend!(
        &mut root,
        LinkPath,
        HList![Ref<LinkCore>, Ref<SentRecords>],
        hlist![PathKey(20), LinkId(3)],
        |hlist_pat![l, s]| (l.rx, s.acked)
    );
    assert_eq!(rx, Some((9, 1)));
}

#[test]
fn the_anchor_itself_and_at_most_one() {
    let mut root = tree_two();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Under<ConnCore, Mut<SentRecords>>, Under<LinkCore, Mut<Congestion>>],
        hlist![LinkId(1)],
        |hlist_pat![mut sent, congestion]| {
            let _: &SingleView<&mut SentRecords> = &sent;
            let _: &Option<&mut Congestion> = &congestion;
            sent.acked += 1;
            congestion.map(|c| {
                c.in_flight += 5;
                c.in_flight
            })
        }
    );
    assert_eq!(got, Some(Some(5)));
    let none = lend!(
        &mut root,
        LinkPath,
        HList![Under<LinkCore, Mut<Congestion>>],
        hlist![LinkId(2)],
        |hlist_pat![c]| c.is_none()
    );
    assert_eq!(none, Some(true));
}

#[test]
fn taking_at_each_member_has_the_same_view() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        Conn1,
        HList![
            Under<ConnCore, Mut<SentRecords>>,
            Under<ConnCore, At<LinkCore, Mut<LinkCore>>>
        ],
        hlist![],
        |hlist_pat![sent, links]| {
            let _: (
                &KeyedView<PathKey, &mut SentRecords>,
                &KeyedView<(PathKey, LinkId), &mut LinkCore>,
            ) = (&sent, &links);
            (sent.len(), links.len())
        }
    );
    assert_eq!(got, Some((2, 3)));
}

#[test]
fn a_lend_point_not_in_the_tree() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Mut<Planner>],
        hlist![PathKey(10), LinkId(9)],
        |_| ()
    );
    assert_eq!(got, None);
    let got = lend!(
        &mut root,
        Recovery,
        HList![Mut<Congestion>],
        hlist![PathKey(10), LinkId(2)],
        |_| ()
    );
    assert_eq!(got, None);
}
