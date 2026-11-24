//! One call written for any structure, lent on both: it names its view
//! from its needs and uses only what the traits of the views declare. The
//! two trees hold the same state, so each call gives the same results on
//! both, which one check compares.
use core::time::Duration;

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::{
    lend::{Visit, lend_visit},
    need::{At, Clock, Members, Mut, OwnKey, Pos, Ref, Requests, Under, View, WakerOf},
};

type RecvNeeds = HList![Under<ConnCore, Mut<LinkCore>>, Mut<SentRecords>];

/// Every link of the connection received one more datagram; the sent
/// records of the lend point note how many links there were.
struct Receive;
impl Visit<RecvNeeds> for Receive {
    type Out = usize;
    fn visit<'v, P: Pos>(self, view: View<'v, RecvNeeds, P>) -> usize {
        let hlist_pat![mut links, sent] = view;
        for (_, link) in links.iter_mut() {
            link.rx += 1;
        }
        let n = links.len();
        sent.sent.push(n as u64);
        n
    }
}

/// What a receive did: its result, the sent records it noted in and every
/// link's count.
type Received = (
    Option<usize>,
    Option<alloc::vec::Vec<u64>>,
    Option<alloc::vec::Vec<u32>>,
);

#[test]
fn receive() {
    let mut root = tree_one_as_two();
    let n = lend_visit::<_, _, LinkPath, _, RecvNeeds, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(1)],
        &mut (),
        Receive,
    );
    let one: Received = (
        n,
        lend!(
            &mut root,
            LogicalPath,
            HList![Ref<SentRecords>],
            hlist![PathKey(10)],
            |hlist_pat![s]| s.sent.clone()
        ),
        lend!(
            &mut root,
            Conn1,
            HList![Under<ConnCore, Ref<LinkCore>>],
            hlist![],
            |hlist_pat![v]| v.iter().map(|(_, l)| l.rx).collect()
        ),
    );
    let mut root = tree_two();
    let n = lend_visit::<_, _, LinkPath, _, RecvNeeds, _, _, _>(
        &mut root,
        &hlist![LinkId(1)],
        &mut (),
        Receive,
    );
    let two: Received = (
        n,
        lend!(
            &mut root,
            Conn2,
            HList![Ref<SentRecords>],
            hlist![],
            |hlist_pat![s]| s.sent.clone()
        ),
        lend!(
            &mut root,
            Conn2,
            HList![Under<ConnCore, Ref<LinkCore>>],
            hlist![],
            |hlist_pat![v]| v.iter().map(|(_, l)| l.rx).collect()
        ),
    );
    same_on_both(
        one,
        two,
        (Some(2), Some(alloc::vec![2]), Some(alloc::vec![1, 1])),
    );
}

type SendNeeds = HList![
    Mut<Planner>,
    Under<LinkCore, Mut<Congestion>>,
    Ref<Keys>,
];

/// A packet sent on the lend point: the next packet number, the
/// congestion window when the link has recovery, and the key epoch.
struct Send;
impl Visit<SendNeeds> for Send {
    type Out = (u64, Option<u32>, u32);
    fn visit<'v, P: Pos>(self, view: View<'v, SendNeeds, P>) -> Self::Out {
        let hlist_pat![planner, mut congestion, keys] = view;
        let pn = planner.next_pn;
        planner.next_pn += 1;
        let in_flight = congestion.iter_mut().next().map(|(_, c)| {
            c.in_flight += 1;
            c.in_flight
        });
        (pn, in_flight, keys.epoch)
    }
}

#[test]
fn send() {
    let mut root = tree_one_as_two();
    let one = [1, 2, 1].map(|l| {
        lend_visit::<_, _, LinkPath, _, SendNeeds, _, _, _>(
            &mut root,
            &hlist![PathKey(10), LinkId(l)],
            &mut (),
            Send,
        )
    });
    let mut root = tree_two();
    let two = [1, 2, 1].map(|l| {
        lend_visit::<_, _, LinkPath, _, SendNeeds, _, _, _>(
            &mut root,
            &hlist![LinkId(l)],
            &mut (),
            Send,
        )
    });
    same_on_both(
        one,
        two,
        [
            Some((100, Some(1), 3)),
            Some((200, None, 3)),
            Some((101, Some(2), 3)),
        ],
    );
}

type PtoNeeds = HList![
    Clock<TestClock>,
    Under<LinkCore, HList![Mut<Congestion>, WakerOf<Congestion>]>,
    Ref<RecoveryFactory>,
];

/// The probe timeout of the lend point expired: the congestion of its
/// recovery, where it has one, drops what is in flight and is woken again
/// a factory's worth of seconds after the lending's instant.
struct Pto;
impl Visit<PtoNeeds> for Pto {
    type Out = (Duration, Option<(u32, Option<Duration>, bool)>);
    fn visit<'v, P: Pos>(self, view: View<'v, PtoNeeds, P>) -> Self::Out {
        let hlist_pat![clock, mut recovery, factory] = view;
        let now = clock.now();
        let fired = recovery
            .iter_mut()
            .next()
            .map(|(_, hlist_pat![congestion, waker])| {
                let dropped = congestion.in_flight;
                congestion.in_flight = 0;
                let again = clock.checked_add(now, Duration::from_secs(u64::from(factory.made)));
                let copied = *waker;
                (dropped, again, copied == *waker)
            });
        (now, fired)
    }
}

#[test]
fn pto_expired() {
    let mut cx = test_cx();
    let mut root = tree_one_as_two();
    lend_visit::<_, _, LinkPath, _, SendNeeds, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(1)],
        &mut (),
        Send,
    );
    let one = [1, 2].map(|l| {
        lend_visit::<_, _, LinkPath, _, PtoNeeds, _, _, _>(
            &mut root,
            &hlist![PathKey(10), LinkId(l)],
            &mut cx,
            Pto,
        )
    });
    let mut root = tree_two();
    lend_visit::<_, _, LinkPath, _, SendNeeds, _, _, _>(
        &mut root,
        &hlist![LinkId(1)],
        &mut (),
        Send,
    );
    let two = [1, 2].map(|l| {
        lend_visit::<_, _, LinkPath, _, PtoNeeds, _, _, _>(
            &mut root,
            &hlist![LinkId(l)],
            &mut cx,
            Pto,
        )
    });
    let second = Duration::from_secs(1);
    same_on_both(
        one,
        two,
        [
            Some((second, Some((1, Some(Duration::from_secs(8)), true)))),
            Some((second, None)),
        ],
    );
}

type CloseNeeds = HList![
    Under<
        ConnCore,
        At<
            LinkCore,
            HList![
                Mut<LinkCore>,
                WakerOf<LinkCore>,
                OwnKey<LinkId>,
                Requests<Ping>,
            ],
        >,
    >,
    Requests<Close>,
];

/// The connection closes: every link, lent all at once, counts one more
/// datagram and puts in a last ping for itself, and the connection puts in
/// its close after them.
struct CloseAll;
impl Visit<CloseNeeds> for CloseAll {
    type Out = (usize, bool);
    fn visit<'v, P: Pos>(self, view: View<'v, CloseNeeds, P>) -> Self::Out {
        let hlist_pat![mut links, closes] = view;
        let mut wakers = true;
        for (_, hlist_pat![link, waker, own, pings]) in links.iter_mut() {
            link.rx += 1;
            pings.push(Ping(*own));
            let copied = *waker;
            wakers &= copied == *waker;
        }
        closes.push(Close);
        (links.len(), wakers)
    }
}

/// What a close did: its result, the requests put in and every link's
/// count.
type Closed = (
    Option<(usize, bool)>,
    (alloc::vec::Vec<(u64, Ping)>, alloc::vec::Vec<(u64, Close)>),
    Option<alloc::vec::Vec<u32>>,
);

#[test]
fn closed_fan_out() {
    let mut cx = test_cx();
    let mut root = tree_one_as_two();
    let out =
        lend_visit::<_, _, Conn1, _, CloseNeeds, _, _, _>(&mut root, &hlist![], &mut cx, CloseAll);
    let one: Closed = (
        out,
        requests(&mut cx),
        lend!(
            &mut root,
            Conn1,
            HList![Under<ConnCore, Ref<LinkCore>>],
            hlist![],
            |hlist_pat![v]| v.iter().map(|(_, l)| l.rx).collect()
        ),
    );
    let mut cx = test_cx();
    let mut root = tree_two();
    let out =
        lend_visit::<_, _, Conn2, _, CloseNeeds, _, _, _>(&mut root, &hlist![], &mut cx, CloseAll);
    let two: Closed = (
        out,
        requests(&mut cx),
        lend!(
            &mut root,
            Conn2,
            HList![Under<ConnCore, Ref<LinkCore>>],
            hlist![],
            |hlist_pat![v]| v.iter().map(|(_, l)| l.rx).collect()
        ),
    );
    same_on_both(
        one,
        two,
        (
            Some((2, true)),
            (
                alloc::vec![(0, Ping(LinkId(1))), (1, Ping(LinkId(2)))],
                alloc::vec![(2, Close)],
            ),
            Some(alloc::vec![1, 1]),
        ),
    );
}
