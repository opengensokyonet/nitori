//! The members of an `Under` one at a time, sharing what they take
//! exclusive above them.
use alloc::{vec, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::need::{At, Mut, OneAtATime, OneView, Ref, Under};

/// Each link path's link core with the sent records above it, both
/// exclusive.
type Each = HList![Under<
    ConnCore,
    OneAtATime<At<LinkCore, HList![Mut<LinkCore>, Mut<SentRecords>]>>,
>];

#[test]
fn members_share_an_exclusive_ancestor() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, Each, hlist![], |hlist_pat![mut v]| {
        let _: &OneView<'_, _, (PathKey, LinkId), _> = &v;
        v.for_each_mut(|_, hlist_pat![link, sent]| {
            sent.acked += 1;
            link.rx = sent.acked;
        });
        let hlist_pat![link, _] = v.get_mut(&(PathKey(20), LinkId(3))).unwrap();
        link.rx += 10;
        let mut seen = Vec::new();
        v.for_each_mut(|k, hlist_pat![link, _]| seen.push((*k, link.rx)));
        (seen, v.len())
    });
    assert_eq!(
        got,
        Some((
            vec![
                ((PathKey(10), LinkId(1)), 1),
                ((PathKey(10), LinkId(2)), 2),
                ((PathKey(20), LinkId(3)), 11)
            ],
            3
        ))
    );
    let mut root = tree_two();
    let got = lend!(&mut root, LinkPath, Each, hlist![LinkId(1)], |hlist_pat![
        mut v
    ]| {
        let _: &OneView<'_, _, LinkId, _> = &v;
        v.for_each_mut(|_, hlist_pat![_, sent]| sent.acked += 1);
        assert!(v.get_mut(&LinkId(9)).is_none());
        let hlist_pat![_, sent] = v.get_mut(&LinkId(2)).unwrap();
        (sent.acked, v.keys().copied().collect::<Vec<_>>())
    });
    assert_eq!(got, Some((2, vec![LinkId(1), LinkId(2)])));
}

#[test]
fn one_component_at_each_member() {
    let mut root = tree_two();
    let got = lend!(
        &mut root,
        Conn2,
        HList![Under<ConnCore, OneAtATime<Mut<LinkCore>>>, Ref<Keys>],
        hlist![],
        |hlist_pat![mut v, keys]| {
            v.for_each_mut(|k, link| link.rx = k.0 + keys.epoch);
            let mut seen = Vec::new();
            v.for_each_mut(|k, link| seen.push((*k, link.rx)));
            seen
        }
    );
    assert_eq!(got, Some(vec![(LinkId(1), 4), (LinkId(2), 5)]));
    let got = lend!(
        &mut root,
        LinkPath,
        HList![Under<LinkCore, OneAtATime<Ref<Congestion>>>],
        hlist![LinkId(1)],
        |hlist_pat![mut v]| v.get_mut(&()).map(|c| c.in_flight)
    );
    assert_eq!(got, Some(Some(0)));
}
