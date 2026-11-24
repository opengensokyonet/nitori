//! One call written for any structure, lent on both: it names its view
//! from its needs and uses only what the traits of the views declare. The
//! two trees hold the same state, so each call gives the same results on
//! both, which one check compares.
use core::{fmt::Debug, time::Duration};

use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::{
    lend::{Visit, lend_visit},
    need::{Clock, Members, Mut, Pos, Ref, Under, View, WakerOf},
};

/// The results of one call on structure one and on structure two are the
/// same, and are `want`.
fn same_on_both<O: PartialEq + Debug>(one: O, two: O, want: O) {
    assert_eq!(one, two, "structure one and structure two differ");
    assert_eq!(one, want);
}

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
                (dropped, again, *waker == waker.clone())
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
