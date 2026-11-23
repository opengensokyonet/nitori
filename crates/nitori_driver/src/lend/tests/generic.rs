//! One call written for any structure, lent on both: it names its view
//! from its needs and uses only what `Members` declares.
use frunk_core::{HList, hlist, hlist_pat};

use super::fixtures::{lend, *};
use crate::{
    lend::{Visit, lend_visit},
    need::{Members, Mut, Pos, Ref, Under, View},
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
fn one_call_on_structure_one() {
    let mut root = tree_one();
    let at = hlist![PathKey(10), LinkId(1)];
    let n = lend_visit::<_, _, LinkPath, _, RecvNeeds, _, _, _>(&mut root, &at, &mut (), Receive);
    assert_eq!(n, Some(3));
    let sent = lend!(
        &mut root,
        LogicalPath,
        HList![Ref<SentRecords>],
        hlist![PathKey(10)],
        |hlist_pat![s]| s.sent.clone()
    );
    assert_eq!(sent, Some(alloc::vec![3]));
    let rx = lend!(
        &mut root,
        Conn1,
        HList![Under<ConnCore, Ref<LinkCore>>],
        hlist![],
        |hlist_pat![v]| v.iter().map(|(_, l)| l.rx).collect::<alloc::vec::Vec<_>>()
    );
    assert_eq!(rx, Some(alloc::vec![1, 1, 1]));

    for (l, want) in [(1, (100, Some(1), 7)), (2, (200, None, 7))] {
        let got = lend_visit::<_, _, LinkPath, _, SendNeeds, _, _, _>(
            &mut root,
            &hlist![PathKey(10), LinkId(l)],
            &mut (),
            Send,
        );
        assert_eq!(got, Some(want));
    }
}

#[test]
fn one_call_on_structure_two() {
    let mut root = tree_two();
    let n = lend_visit::<_, _, LinkPath, _, RecvNeeds, _, _, _>(
        &mut root,
        &hlist![LinkId(2)],
        &mut (),
        Receive,
    );
    assert_eq!(n, Some(2));
    let sent = lend!(
        &mut root,
        Conn2,
        HList![Ref<SentRecords>],
        hlist![],
        |hlist_pat![s]| s.sent.clone()
    );
    assert_eq!(sent, Some(alloc::vec![2]));

    for (l, want) in [(1, (100, Some(1), 3)), (2, (200, None, 3))] {
        let got = lend_visit::<_, _, LinkPath, _, SendNeeds, _, _, _>(
            &mut root,
            &hlist![LinkId(l)],
            &mut (),
            Send,
        );
        assert_eq!(got, Some(want));
    }
}
