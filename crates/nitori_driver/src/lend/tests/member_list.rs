//! A list of needs taken at each member: the members are the nodes of
//! the one kind that has every component the list takes.
use alloc::{vec, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, lend_in, *};
use crate::need::{
    At, Clock, KeyedView, Members, Mut, OneAtATime, OwnKey, Ref, Requests, Under, WakerOf,
};

/// Every link: its core and planner, its waker and key, and the sent
/// records above it, read from the link with `At`.
type EachLink = HList![Under<
    ConnCore,
    HList![
        Mut<LinkCore>,
        Ref<Planner>,
        WakerOf<LinkCore>,
        OwnKey<LinkId>,
        At<LinkCore, HList![Ref<SentRecords>]>,
    ],
>];

#[test]
fn several_needs_at_each_member() {
    let mut root = tree_one();
    let got = lend!(&mut root, Conn1, EachLink, hlist![], |hlist_pat![mut v]| {
        let _: &KeyedView<(PathKey, LinkId), _> = &v;
        let mut seen = Vec::new();
        for (k, hlist_pat![core, planner, waker, own, hlist_pat![sent]]) in v.iter_mut() {
            core.rx = planner.next_pn as u32;
            assert_eq!(*waker, waker.clone());
            seen.push((*k, *own, core.rx, sent.sent.len()));
        }
        seen
    });
    assert_eq!(
        got,
        Some(vec![
            ((PathKey(10), LinkId(1)), LinkId(1), 100, 0),
            ((PathKey(10), LinkId(2)), LinkId(2), 200, 0),
            ((PathKey(20), LinkId(3)), LinkId(3), 300, 0),
        ])
    );
    let mut root = tree_two();
    let got = lend!(
        &mut root,
        LinkPath,
        EachLink,
        hlist![LinkId(2)],
        |hlist_pat![mut v]| {
            let _: &KeyedView<LinkId, _> = &v;
            let (mut own, rest) = v.split_own();
            for (_, m) in own.iter_mut() {
                m.head.rx = 7;
            }
            (
                own.keys().copied().collect::<Vec<_>>(),
                rest.keys().copied().collect::<Vec<_>>(),
                v.get(&LinkId(2)).map(|m| m.head.rx),
            )
        }
    );
    assert_eq!(got, Some((vec![LinkId(2)], vec![LinkId(1)], Some(7))));
}

/// A list on the anchor itself, and one at most one member below it.
#[test]
fn a_list_on_the_anchor_and_below_it() {
    let mut root = tree_one();
    let got = lend!(
        &mut root,
        LinkPath,
        HList![
            Under<LinkCore, HList![Mut<LinkCore>, Ref<Planner>]>,
            Under<LinkCore, HList![Mut<Congestion>, WakerOf<Congestion>]>,
        ],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![mut link, mut recovery]| {
            let hlist_pat![core, planner] = &mut *link;
            core.rx = planner.next_pn as u32;
            let r = recovery.as_mut().map(|m| {
                m.head.in_flight += 1;
                m.head.in_flight
            });
            (link.len(), r)
        }
    );
    assert_eq!(got, Some((1, Some(1))));
}

/// A list taken one member at a time, with the clock and requests.
type Probes = HList![Under<
    ConnCore,
    OneAtATime<HList![
        Mut<Planner>,
        At<LinkCore, HList![Mut<SentRecords>]>,
        Clock<TestClock>,
        Requests<Ping>,
        OwnKey<LinkId>,
    ]>,
>];

#[test]
fn a_list_one_member_at_a_time() {
    let mut cx = test_cx();
    let mut root = tree_one();
    let got = lend_in!(&mut root, Conn1, Probes, hlist![], &mut cx, |hlist_pat![
        mut each
    ]| {
        let mut nows = Vec::new();
        each.for_each_mut(
            |_, hlist_pat![planner, hlist_pat![sent], clock, pings, own]| {
                planner.next_pn += 1;
                sent.acked += 1;
                pings.push(Ping(own));
                nows.push(clock.now());
            },
        );
        nows.len()
    });
    assert_eq!(got, Some(3));
    let hlist_pat![_, pings, _] = &mut cx;
    assert_eq!(
        pings.take(),
        vec![Ping(LinkId(1)), Ping(LinkId(2)), Ping(LinkId(3))]
    );
    let acked = lend!(
        &mut root,
        Conn1,
        HList![Under<ConnCore, Ref<SentRecords>>],
        hlist![],
        |hlist_pat![v]| v.iter().map(|(_, s)| s.acked).collect::<Vec<_>>()
    );
    assert_eq!(acked, Some(vec![2, 1]));
}
