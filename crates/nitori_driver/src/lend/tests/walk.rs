//! What a lending walks: down the route to the lend point by key, and
//! below each anchor the nodes the needs take, each member given its own.
use alloc::{vec, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::need::{At, OneAtATime, Ref, Under};

#[test]
fn two_anchors_over_one_kind() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Under<SentRecords, Ref<Planner>>, Under<ConnCore, Ref<Planner>>],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![path, conn]| {
            let path: Vec<_> = path.iter().map(|(k, p)| (*k, p.next_pn)).collect();
            let conn: Vec<_> = conn.iter().map(|(k, p)| (*k, p.next_pn)).collect();
            (path, conn)
        }
    );
    assert_eq!(
        got,
        Some((
            vec![(LinkId(1), 100), (LinkId(2), 200)],
            vec![
                ((PathKey(10), LinkId(1)), 100),
                ((PathKey(10), LinkId(2)), 200),
                ((PathKey(20), LinkId(3)), 300)
            ]
        ))
    );
}

#[test]
fn a_lend_point_missing_on_the_way_down() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Under<ConnCore, Ref<Planner>>],
        hlist![PathKey(30), LinkId(1)],
        |_| ()
    );
    assert_eq!(got, None);
}

/// By logical path, the link cores below it, and every link core of the
/// connection.
type ByPath = HList![Under<
    ConnCore,
    At<
        SentRecords,
        HList![
            Ref<SentRecords>,
            Under<SentRecords, Ref<LinkCore>>,
            Under<ConnCore, Ref<LinkCore>>
        ],
    >,
>];

#[test]
fn nested_unders_at_and_above_each_member() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, ByPath, hlist![], |hlist_pat![v]| {
        v.iter()
            .map(|(p, hlist_pat![_, own, all])| (*p, own.len(), all.len()))
            .collect::<Vec<_>>()
    });
    assert_eq!(got, Some(vec![(PathKey(10), 2, 3), (PathKey(20), 1, 3)]));
}

/// [`ByPath`], one logical path at a time.
type ByPathOne = HList![Under<
    ConnCore,
    OneAtATime<
        At<
            SentRecords,
            HList![
                Ref<SentRecords>,
                Under<SentRecords, Ref<LinkCore>>,
                Under<ConnCore, Ref<LinkCore>>
            ],
        >,
    >,
>];

#[test]
fn nested_unders_one_member_at_a_time() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, ByPathOne, hlist![], |hlist_pat![
        mut v
    ]| {
        let mut seen = Vec::new();
        v.for_each_mut(|p, hlist_pat![_, own, all]| {
            seen.push((*p, own.keys().copied().collect::<Vec<_>>(), all.len()));
        });
        seen
    });
    assert_eq!(
        got,
        Some(vec![
            (PathKey(10), vec![LinkId(1), LinkId(2)], 3),
            (PathKey(20), vec![LinkId(3)], 3)
        ])
    );
}
