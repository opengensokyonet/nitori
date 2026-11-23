//! Moving the lend point: on the way up, at each member, and nested.
use alloc::vec::Vec;

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::need::{At, KeyedView, Mut, Ref, Under};

/// From a link path, the logical path (or, in structure two, the
/// connection) becomes the lend point.
type MoveUp = HList![At<SentRecords, HList![Mut<SentRecords>, Ref<Keys>]>];

#[test]
fn up_the_chain() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        MoveUp,
        hlist![PathKey(10), LinkId(2)],
        |hlist_pat![hlist_pat![sent, keys]]| {
            sent.sent.push(u64::from(keys.epoch));
            sent.sent.len()
        }
    );
    assert_eq!(got, Some(1));
    let mut root = tree_two();
    let got = lend!(&mut root, LinkPath, MoveUp, hlist![LinkId(2)], |hlist_pat![
        hlist_pat![sent, keys]
    ]| {
        sent.sent.push(u64::from(keys.epoch));
        sent.sent.clone()
    });
    assert_eq!(got, Some(alloc::vec![3]));
}

/// Each recovery its own congestion, and the connection's factory shared.
type EachRecovery =
    HList![Under<ConnCore, At<Congestion, HList![Mut<Congestion>, Ref<RecoveryFactory>]>>];

#[test]
fn at_each_member_with_a_shared_ancestor() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        EachRecovery,
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![mut v]| {
            let _: &KeyedView<(PathKey, LinkId), _> = &v;
            let mut made = 0;
            for (_, hlist_pat![c, f]) in v.members_mut() {
                c.in_flight += 10;
                made += f.made;
            }
            (v.keys().copied().collect::<Vec<_>>(), made)
        }
    );
    assert_eq!(
        got,
        Some((
            alloc::vec![(PathKey(10), LinkId(1)), (PathKey(20), LinkId(3))],
            10
        ))
    );
    let mut root = tree_two();
    let got = lend!(
        &mut root,
        LinkPath,
        EachRecovery,
        hlist![LinkId(2)],
        |hlist_pat![v]| {
            let _: &KeyedView<LinkId, _> = &v;
            v.keys().copied().collect::<Vec<_>>()
        }
    );
    assert_eq!(got, Some(alloc::vec![LinkId(1)]));
}

/// Two levels: by logical path, its own sent records and its recoveries by
/// link.
type Nested = HList![Under<
    ConnCore,
    At<SentRecords, HList![Mut<SentRecords>, Under<SentRecords, Mut<Congestion>>]>,
>];

#[test]
fn nested_views() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, Nested, hlist![], |hlist_pat![mut v]| {
        let _: &KeyedView<PathKey, HList![&mut SentRecords, KeyedView<LinkId, &mut Congestion>]> =
            &v;
        let mut seen = Vec::new();
        for (p, hlist_pat![sent, inner]) in v.members_mut() {
            for (l, c) in inner.members_mut() {
                c.in_flight += 100;
                sent.acked += 1;
                seen.push((*p, *l));
            }
        }
        seen
    });
    assert_eq!(
        got,
        Some(alloc::vec![
            (PathKey(10), LinkId(1)),
            (PathKey(20), LinkId(3))
        ])
    );
    let mut root = tree_two();
    let got = lend!(&mut root, Conn2, Nested, hlist![], |hlist_pat![v]| {
        let _: &HList![&mut SentRecords, KeyedView<LinkId, &mut Congestion>] = &v;
        let hlist_pat![sent, inner] = v;
        sent.acked = inner.member_count() as u32;
        sent.acked
    });
    assert_eq!(got, Some(1));
}
