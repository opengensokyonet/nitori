//! The needs that read the lending: the clock and the request boxes.
use core::time::Duration;

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend_in, *};
use crate::{
    need::{At, Clock, LentClock, Mut, OneAtATime, Ref, RequestBox, Requests, Under},
    needs,
};

/// A link sends a ping for itself, timed from the lending's instant.
type Probe = HList![Mut<Planner>, Clock<TestClock>, Requests<Ping>, OwnKeyOf];
type OwnKeyOf = crate::need::OwnKey<LinkId>;

#[test]
fn clock_and_requests() {
    let mut cx = test_cx();
    let mut root = tree_one();
    let got = lend_in!(
        &mut root,
        LinkPath,
        Probe,
        hlist![PathKey(10), LinkId(2)],
        &mut cx,
        |hlist_pat![planner, clock, pings, own]| {
            let _: &LentClock<TestClock> = clock;
            let _: &mut RequestBox<Ping> = pings;
            pings.push(Ping(own));
            planner.next_pn += 1;
            (
                clock.now(),
                clock.checked_add(clock.now(), Duration::from_secs(2)),
                clock.checked_add(clock.now(), Duration::from_secs(20)),
            )
        }
    );
    assert_eq!(
        got,
        Some((Duration::from_secs(1), Some(Duration::from_secs(3)), None))
    );
    let mut root = tree_two();
    let got = lend_in!(
        &mut root,
        LinkPath,
        Probe,
        hlist![LinkId(1)],
        &mut cx,
        |hlist_pat![_planner, clock, pings, own]| {
            pings.push(Ping(own));
            clock.now()
        }
    );
    assert_eq!(got, Some(Duration::from_secs(1)));
    let hlist_pat![_, pings, closes] = &mut cx;
    assert_eq!(pings.take(), alloc::vec![Ping(LinkId(2)), Ping(LinkId(1))]);
    assert!(closes.take().is_empty());
}

/// Every link reads the clock; one at a time, each sends a ping.
type EachLink = HList![
    Under<ConnCore, At<LinkCore, HList![Ref<LinkCore>, Clock<TestClock>]>>,
    Under<ConnCore, OneAtATime<At<Planner, HList![Mut<Planner>, Requests<Ping>]>>>,
    Requests<Close>,
];

#[test]
fn in_members() {
    let mut cx = test_cx();
    let mut root = tree_one();
    let got = lend_in!(&mut root, Conn1, EachLink, hlist![], &mut cx, |hlist_pat![
        read,
        mut each,
        closes
    ]| {
        let nows = read
            .iter()
            .map(|(_, hlist_pat![_, c])| c.now())
            .collect::<alloc::vec::Vec<_>>();
        each.for_each_mut(|k, hlist_pat![planner, pings]| {
            planner.next_pn += 1;
            pings.push(Ping(k.1));
        });
        closes.push(Close);
        nows
    });
    assert_eq!(got, Some(alloc::vec![Duration::from_secs(1); 3]));
    let mut root = tree_two();
    let got = lend_in!(&mut root, Conn2, EachLink, hlist![], &mut cx, |hlist_pat![
        read,
        mut each,
        _closes
    ]| {
        if let Some(hlist_pat![_, pings]) = each.get_mut(&LinkId(2)) {
            pings.push(Ping(LinkId(20)));
        }
        read.len()
    });
    assert_eq!(got, Some(2));
    let hlist_pat![_, pings, closes] = &mut cx;
    assert_eq!(
        pings.take(),
        alloc::vec![
            Ping(LinkId(1)),
            Ping(LinkId(2)),
            Ping(LinkId(3)),
            Ping(LinkId(20))
        ]
    );
    assert_eq!(closes.take(), alloc::vec![Close]);
}

/// Both branches of a `+` put pings in; one also reads the clock.
type Either = needs![
    (HList![Mut<LinkCore>, Requests<Ping>, Clock<TestClock>]
        + HList![Requests<Ping>, Ref<Planner>]),
    Requests<Close>,
];

#[test]
fn in_branches() {
    let mut cx = test_cx();
    let mut root = tree_one();
    let got = lend_in!(
        &mut root,
        LinkPath,
        Either,
        hlist![PathKey(20), LinkId(3)],
        &mut cx,
        |hlist_pat![mut plus, closes]| {
            let hlist_pat![link, pings, clock] =
                plus.take::<HList![Mut<LinkCore>, Requests<Ping>, Clock<TestClock>], _>();
            link.rx += 1;
            pings.push(Ping(LinkId(link.rx)));
            let now = clock.now();
            let hlist_pat![pings, planner] = plus.take::<HList![Requests<Ping>, Ref<Planner>], _>();
            pings.push(Ping(LinkId(planner.next_pn as u32)));
            closes.push(Close);
            now
        }
    );
    assert_eq!(got, Some(Duration::from_secs(1)));
    let mut root = tree_two();
    let got = lend_in!(
        &mut root,
        LinkPath,
        Either,
        hlist![LinkId(2)],
        &mut cx,
        |hlist_pat![mut plus, _closes]| {
            let hlist_pat![pings, planner] = plus.take::<HList![Requests<Ping>, Ref<Planner>], _>();
            pings.push(Ping(LinkId(planner.next_pn as u32)));
        }
    );
    assert_eq!(got, Some(()));
    let hlist_pat![_, pings, closes] = &mut cx;
    assert_eq!(
        pings.take(),
        alloc::vec![Ping(LinkId(1)), Ping(LinkId(300)), Ping(LinkId(200))]
    );
    assert_eq!(closes.take(), alloc::vec![Close]);
}
