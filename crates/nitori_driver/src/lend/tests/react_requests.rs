//! Reactions that put in requests: at every member of an `Under` lent at
//! once, and from a branch of a `+`; the requests keep the order they
//! were put in.
use frunk_core::{HList, hlist, hlist_pat};

use super::{fixtures::*, parts::*};
use crate::{
    lend::{Visit, lend_visit},
    need::{At, Cap, Members, Mut, Pos, Requests, Under, View},
    needs,
};

type PingEvery = HList![
    Under<ConnCore, At<LinkCore, HList![Mut<LinkCore>, Cap<Pinged>]>>,
    Requests<Close>,
];
/// Every link, lent at once, counts and is pinged; then the connection
/// closes.
struct PingAll;
impl Visit<PingEvery> for PingAll {
    type Out = usize;
    fn visit<'v, P: Pos>(self, view: View<'v, PingEvery, P>) -> usize {
        let hlist_pat![mut links, closes] = view;
        for (_, hlist_pat![link, pinged]) in links.iter_mut() {
            link.rx += 1;
            pinged.fire(());
        }
        closes.push(Close);
        links.len()
    }
}

type PingOne = needs![Requests<Close>, (Cap<Pinged> + Mut<LinkCore>)];
/// A close, the link pinged from a branch, the link counted from the
/// other, and a close again.
struct PingHere;
impl Visit<PingOne> for PingHere {
    type Out = u32;
    fn visit<'v, P: Pos>(self, view: View<'v, PingOne, P>) -> u32 {
        let hlist_pat![closes, mut plus] = view;
        closes.push(Close);
        plus.take::<Cap<Pinged>, _>().fire(());
        let link = plus.take::<Mut<LinkCore>, _>();
        link.rx += 1;
        closes.push(Close);
        link.rx
    }
}

type Requested = (
    Option<usize>,
    (alloc::vec::Vec<(u64, Ping)>, alloc::vec::Vec<(u64, Close)>),
    Option<u32>,
    (alloc::vec::Vec<(u64, Ping)>, alloc::vec::Vec<(u64, Close)>),
);

#[test]
fn reactions_put_in_requests() {
    let mut root = tree_one_as_two();
    let mut cx = test_cx();
    let every =
        lend_visit::<_, _, Conn1, _, PingEvery, _, _, _>(&mut root, &hlist![], &mut cx, PingAll);
    let every_requests = requests(&mut cx);
    let mut cx = test_cx();
    let here = lend_visit::<_, _, LinkPath, _, PingOne, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(2)],
        &mut cx,
        PingHere,
    );
    let one: Requested = (every, every_requests, here, requests(&mut cx));
    let mut root = tree_two();
    let mut cx = test_cx();
    let every =
        lend_visit::<_, _, Conn2, _, PingEvery, _, _, _>(&mut root, &hlist![], &mut cx, PingAll);
    let every_requests = requests(&mut cx);
    let mut cx = test_cx();
    let here = lend_visit::<_, _, LinkPath, _, PingOne, _, _, _>(
        &mut root,
        &hlist![LinkId(2)],
        &mut cx,
        PingHere,
    );
    let two: Requested = (every, every_requests, here, requests(&mut cx));
    same_on_both(
        one,
        two,
        (
            Some(2),
            (
                alloc::vec![(0, Ping(LinkId(1))), (1, Ping(LinkId(2)))],
                alloc::vec![(2, Close)],
            ),
            Some(2),
            (
                alloc::vec![(1, Ping(LinkId(2)))],
                alloc::vec![(0, Close), (2, Close)],
            ),
        ),
    );
}
